//! The socket between the service and its clients (the widget, the CLI):
//! one JSON object per line, one request per connection, the way Herdr
//! does it. A `guild.subscribe` request answers `subscription_started`
//! and then streams events after the given sequence until the client
//! hangs up.

use crate::domain::Event;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use crate::ipc::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq)]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

impl ApiError {
    pub fn new(code: &str, message: impl Into<String>) -> ApiError { ApiError { code: code.into(), message: message.into() } }
    pub fn invalid(message: impl Into<String>) -> ApiError { ApiError::new("invalid_request", message) }
    pub fn not_found(message: impl Into<String>) -> ApiError { ApiError::new("not_found", message) }
    pub fn conflict(message: impl Into<String>) -> ApiError { ApiError::new("conflict", message) }
    pub fn internal(message: impl Into<String>) -> ApiError { ApiError::new("internal", message) }
}

impl From<String> for ApiError {
    fn from(s: String) -> ApiError { ApiError::internal(s) }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { write!(f, "{}: {}", self.code, self.message) }
}

pub type ApiResult = Result<Value, ApiError>;

/// What the server needs from the service: run a command, and hand out
/// the events after a sequence plus a live feed.
pub trait Handler: Send + Sync {
    fn handle(&self, method: &str, params: &Value) -> ApiResult;
    /// Subscribe: the receiver gets every event persisted after `after_seq`,
    /// in order, including a replay of what is already stored.
    fn subscribe(&self, after_seq: u64, sender: Sender<Event>) -> Result<u64, ApiError>;
}

pub fn serve(path: &Path, handler: Arc<dyn Handler>, stop: Arc<std::sync::atomic::AtomicBool>) -> std::io::Result<()> {
    // A stale socket from a dead service; a live one answered the lock check before we got here.
    let _ = std::fs::remove_file(path);
    if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
    let listener = UnixListener::bind(path)?;
    listener.set_nonblocking(true)?;
    loop {
        if stop.load(std::sync::atomic::Ordering::Relaxed) { break; }
        match listener.accept() {
            Ok((stream, _)) => {
                let handler = handler.clone();
                std::thread::Builder::new().name("guild-api".into()).spawn(move || serve_connection(stream, handler)).ok();
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    }
    let _ = std::fs::remove_file(path);
    Ok(())
}

fn serve_connection(mut stream: UnixStream, handler: Arc<dyn Handler>) {
    let _ = stream.set_nonblocking(false);
    let Ok(reader) = stream.try_clone() else { return };
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 { return; }
    let request: Value = match serde_json::from_str(&line) {
        Ok(v) => v,
        Err(e) => {
            let _ = write_line(&mut stream, &json!({"id": null, "error": {"code": "invalid_json", "message": e.to_string()}}));
            return;
        }
    };
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request.get("method").and_then(Value::as_str).unwrap_or("").to_string();
    let params = request.get("params").cloned().unwrap_or(json!({}));
    if method == "guild.subscribe" {
        let after = params.get("afterSeq").and_then(Value::as_u64).unwrap_or(0);
        let (tx, rx) = std::sync::mpsc::channel();
        match handler.subscribe(after, tx) {
            Ok(last_seq) => {
                if write_line(&mut stream, &json!({"id": id, "result": {"type": "subscription_started", "lastSeq": last_seq, "protocolVersion": PROTOCOL_VERSION}})).is_err() { return; }
            }
            Err(e) => {
                let _ = write_line(&mut stream, &json!({"id": id, "error": {"code": e.code, "message": e.message}}));
                return;
            }
        }
        // Stream until the client closes; a failed write is how we learn it did.
        loop {
            match rx.recv_timeout(Duration::from_secs(15)) {
                Ok(event) => {
                    if write_line(&mut stream, &json!({"event": event})).is_err() { break; }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if write_line(&mut stream, &json!({"heartbeat": true})).is_err() { break; }
                }
                Err(_) => break,
            }
        }
        return;
    }
    let response = match handler.handle(&method, &params) {
        Ok(result) => json!({"id": id, "result": result}),
        Err(e) => json!({"id": id, "error": {"code": e.code, "message": e.message}}),
    };
    let _ = write_line(&mut stream, &response);
}

fn write_line(stream: &mut UnixStream, value: &Value) -> std::io::Result<()> {
    let mut s = value.to_string();
    s.push('\n');
    stream.write_all(s.as_bytes())?;
    stream.flush()
}

/// A client of the service's socket.
#[derive(Clone, Debug)]
pub struct Client {
    pub socket: std::path::PathBuf,
}

pub enum StreamMessage {
    Open { last_seq: u64 },
    Event(Event),
    Close,
}

impl Client {
    pub fn new(socket: &Path) -> Client { Client { socket: socket.to_path_buf() } }

    pub fn default_socket() -> Client { Client::new(&crate::paths::socket_path()) }

    pub fn is_up(&self) -> bool { self.call_timeout("guild.ping", json!({}), Duration::from_secs(2)).is_ok() }

    pub fn call(&self, method: &str, params: Value) -> Result<Value, ApiError> { self.call_timeout(method, params, Duration::from_secs(120)) }

    pub fn call_timeout(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, ApiError> {
        let mut stream = UnixStream::connect(&self.socket).map_err(|e| ApiError::new("unavailable", format!("guild service not reachable at {}: {e}", self.socket.display())))?;
        let _ = stream.set_read_timeout(Some(timeout));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
        let id = crate::domain::new_id();
        write_line(&mut stream, &json!({"id": id, "method": method, "params": params})).map_err(|e| ApiError::new("unavailable", e.to_string()))?;
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).map_err(|e| ApiError::new("unavailable", e.to_string()))?;
        let response: Value = serde_json::from_str(&line).map_err(|e| ApiError::new("invalid_response", e.to_string()))?;
        if let Some(err) = response.get("error") {
            return Err(ApiError::new(err.get("code").and_then(Value::as_str).unwrap_or("error"), err.get("message").and_then(Value::as_str).unwrap_or("")));
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }

    /// Open the event stream on a thread; messages arrive on `sender`
    /// tagged with `generation` so a replaced stream can be told apart.
    /// Returns the stream handle to close it.
    pub fn subscribe(&self, after_seq: u64, generation: u64, sender: Sender<(u64, StreamMessage)>) -> Result<StreamHandle, ApiError> {
        let stream = UnixStream::connect(&self.socket).map_err(|e| ApiError::new("unavailable", format!("guild service not reachable: {e}")))?;
        let handle = StreamHandle { stream: stream.try_clone().map_err(|e| ApiError::new("unavailable", e.to_string()))? };
        let mut writer = stream.try_clone().map_err(|e| ApiError::new("unavailable", e.to_string()))?;
        std::thread::Builder::new()
            .name("guild-events".into())
            .spawn(move || {
                if write_line(&mut writer, &json!({"id": "subscribe", "method": "guild.subscribe", "params": {"afterSeq": after_seq}})).is_err() {
                    let _ = sender.send((generation, StreamMessage::Close));
                    return;
                }
                let mut opened = false;
                for row in BufReader::new(stream).lines() {
                    let Ok(row) = row else { break };
                    let Ok(object) = serde_json::from_str::<Value>(&row) else { continue };
                    if !opened {
                        let last_seq = object.pointer("/result/lastSeq").and_then(Value::as_u64);
                        match last_seq {
                            Some(n) => {
                                opened = true;
                                let _ = sender.send((generation, StreamMessage::Open { last_seq: n }));
                            }
                            None => break,
                        }
                    } else if let Some(event) = object.get("event") {
                        if let Ok(event) = serde_json::from_value::<Event>(event.clone()) {
                            let _ = sender.send((generation, StreamMessage::Event(event)));
                        }
                    }
                }
                let _ = sender.send((generation, StreamMessage::Close));
            })
            .map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(handle)
    }
}

pub struct StreamHandle {
    stream: UnixStream,
}

impl StreamHandle {
    pub fn close(&self) { let _ = self.stream.shutdown(std::net::Shutdown::Both); }
}
