//! Herdr's live event stream: one connection to its socket holding an
//! `events.subscribe` request open, so status changes arrive the moment they
//! happen instead of with the next `herdr api snapshot`. Herdr serves one
//! request per connection, so changing the subscribed panes means opening a
//! new stream. Messages arrive on a channel the monitor drains each frame.
//!
//! A saved SSH machine's server is reached the same way through one
//! long-lived `ssh` whose remote end pipes to that server's socket (Herdr's
//! `--machine` serves no stream of its own).

use crate::herdr::herdr_subscriptions;
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::collections::HashMap;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

pub enum StreamMessage {
    Open,
    Event(Value),
    Close,
}

/// Where the stream connects: this computer's socket, or a machine's over SSH.
#[derive(Clone)]
enum Source {
    Socket(String),
    Ssh { target: String, session: String },
}

pub struct HerdrStream {
    source: Source,
    subscriptions: Vec<Value>,
    connection: Arc<Mutex<Option<UnixStream>>>,
    child: Arc<Mutex<Option<Child>>>,
    requests: Requester,
    stopped: Arc<Mutex<bool>>,
    /// Which stream a message came from, so a replaced stream's are ignored.
    pub generation: u64,
}

impl HerdrStream {
    pub fn new(socket_path: &str, panes: &[String], generation: u64) -> HerdrStream {
        HerdrStream::with(Source::Socket(socket_path.to_string()), panes, generation)
    }

    /// The stream of a saved SSH machine's Herdr session (`target` as
    /// `herdr machine list` gives it: what `ssh` connects to).
    pub fn remote(target: &str, session: &str, panes: &[String], generation: u64) -> HerdrStream {
        HerdrStream::with(Source::Ssh { target: target.to_string(), session: session.to_string() }, panes, generation)
    }

    fn with(source: Source, panes: &[String], generation: u64) -> HerdrStream {
        HerdrStream {
            source,
            subscriptions: herdr_subscriptions(panes),
            connection: Arc::new(Mutex::new(None)),
            child: Arc::new(Mutex::new(None)),
            requests: Requester::default(),
            stopped: Arc::new(Mutex::new(false)),
            generation,
        }
    }

    pub fn start(&self, sender: Sender<(u64, StreamMessage)>) {
        let source = self.source.clone();
        let request = serde_json::json!({ "id": "hpd-events", "method": "events.subscribe", "params": { "subscriptions": self.subscriptions } });
        let connection = self.connection.clone();
        let child = self.child.clone();
        let requests = self.requests.clone();
        let stopped = self.stopped.clone();
        let generation = self.generation;
        std::thread::Builder::new()
            .name("herdr-events".into())
            .spawn(move || {
                let quiet = run(&source, &request, &connection, &child, &requests, &stopped, generation, &sender);
                *connection.lock().unwrap_or_else(|e| e.into_inner()) = None;
                if let Some(mut child) = child.lock().unwrap_or_else(|e| e.into_inner()).take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                if !quiet {
                    let _ = sender.send((generation, StreamMessage::Close));
                }
            })
            .ok();
    }

    /// Requests through a machine's bridge (always closed for a local stream).
    pub fn requester(&self) -> Requester {
        self.requests.clone()
    }

    pub fn stop(&self) {
        *self.stopped.lock().unwrap_or_else(|e| e.into_inner()) = true;
        if let Some(stream) = self.connection.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        if let Some(child) = self.child.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            let _ = child.kill();
        }
    }
}

impl Drop for HerdrStream {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Requests sent through a machine's bridge beside its event stream (the
/// terminal reads of the chat panel): each goes to the machine's Herdr on a
/// connection of its own and its reply comes back by id, a round trip over
/// the open SSH connection instead of a new `herdr --machine` each time.
#[derive(Clone, Default)]
pub struct Requester {
    input: Arc<Mutex<Option<ChildStdin>>>,
    pending: Arc<Mutex<HashMap<String, Sender<Value>>>>,
    next: Arc<AtomicU64>,
    open: Arc<AtomicBool>,
}

impl Requester {
    /// Send one request; None while the bridge is not open. The reply is
    /// Herdr's whole answer (`result` or `error`); the receiver disconnects
    /// if the bridge closes first.
    pub fn request(&self, method: &str, params: Value) -> Option<Receiver<Value>> {
        if !self.open.load(Ordering::Relaxed) { return None; }
        let id = format!("hpd-{}", self.next.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = channel();
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone(), tx);
        let mut line = serde_json::json!({ "id": id, "method": method, "params": params }).to_string();
        line.push('\n');
        let sent = self.input.lock().unwrap_or_else(|e| e.into_inner()).as_mut().is_some_and(|input| input.write_all(line.as_bytes()).and_then(|_| input.flush()).is_ok());
        if !sent {
            self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
            return None;
        }
        Some(rx)
    }

    fn close(&self) {
        self.open.store(false, Ordering::Relaxed);
        *self.input.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

/// Serve the stream until it ends; true when it was stopped on purpose.
fn run(source: &Source, request: &Value, connection: &Arc<Mutex<Option<UnixStream>>>, child: &Arc<Mutex<Option<Child>>>, requests: &Requester, stopped: &Arc<Mutex<bool>>, generation: u64, sender: &Sender<(u64, StreamMessage)>) -> bool {
    let mut line = request.to_string();
    line.push('\n');
    let reader: Box<dyn Read> = match source {
        Source::Socket(path) => {
            let Ok(mut stream) = UnixStream::connect(path) else { return *stopped.lock().unwrap() };
            let Ok(reader) = stream.try_clone() else { return *stopped.lock().unwrap() };
            {
                let mut slot = connection.lock().unwrap_or_else(|e| e.into_inner());
                if *stopped.lock().unwrap() { return true; }
                *slot = Some(reader);
            }
            if stream.write_all(line.as_bytes()).is_err() { return *stopped.lock().unwrap(); }
            Box::new(stream)
        }
        Source::Ssh { target, session } => {
            // HPD_SSH stands in for ssh in the self-test.
            let mut command = Command::new(std::env::var("HPD_SSH").unwrap_or_else(|_| "ssh".into()));
            command
                .args(["-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=3"])
                .arg(target)
                .arg(bridge_command(session))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            let mut slot = child.lock().unwrap_or_else(|e| e.into_inner());
            if *stopped.lock().unwrap() { return true; }
            let Ok(mut process) = command.spawn() else { return false };
            let (Some(mut input), Some(output)) = (process.stdin.take(), process.stdout.take()) else { let _ = process.kill(); return false };
            *slot = Some(process);
            if input.write_all(line.as_bytes()).and_then(|_| input.flush()).is_err() { return *stopped.lock().unwrap(); }
            // The bridge stops when its input closes: it stays open, for the
            // requests, until the stream ends.
            *requests.input.lock().unwrap_or_else(|e| e.into_inner()) = Some(input);
            Box::new(output)
        }
    };
    let mut opened = false;
    for row in BufReader::new(reader).lines() {
        let Ok(row) = row else { break };
        let Ok(object) = serde_json::from_str::<Value>(&row) else { continue };
        match object.get("id").and_then(Value::as_str) {
            // The first reply confirms the subscription; an error ends the stream.
            Some("hpd-events") if !opened => {
                let started = object.get("result").and_then(|r| r.get("type")).and_then(Value::as_str) == Some("subscription_started");
                if !started { break; }
                opened = true;
                requests.open.store(true, Ordering::Relaxed);
                let _ = sender.send((generation, StreamMessage::Open));
            }
            Some(id) => {
                let waiting = requests.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
                if let Some(waiting) = waiting { let _ = waiting.send(object); }
            }
            None if opened => { let _ = sender.send((generation, StreamMessage::Event(object))); }
            None => {}
        }
    }
    requests.close();
    *stopped.lock().unwrap_or_else(|e| e.into_inner())
}

/// The remote end of a machine's stream: a few lines of Python that find
/// the session's socket (as the machine's own `herdr session list` says, or
/// Herdr's default place) and send each request line to it on a connection
/// of its own: the event subscription's replies keep coming, any other
/// request gets its one reply. Sent hex-encoded, so no remote shell (sh,
/// fish, zsh) has anything to quote.
const BRIDGE: &str = r#"import json,os,shutil,socket,subprocess,sys,threading
name=SESSION
path=os.environ.get("HERDR_SOCKET_PATH") if name=="default" else None
herdr=shutil.which("herdr") or os.path.expanduser("~/.local/bin/herdr")
if not path:
    try:
        out=subprocess.run([herdr,"session","list","--json"],stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,timeout=10).stdout
        for row in json.loads(out).get("sessions",[]):
            if row.get("name")==name: path=row.get("socket_path")
    except Exception: pass
if not path and name=="default":
    path=os.path.join(os.environ.get("XDG_CONFIG_HOME") or os.path.expanduser("~/.config"),"herdr","herdr.sock")
lock=threading.Lock()
def emit(row):
    with lock:
        sys.stdout.buffer.write(row if row.endswith(b"\n") else row+b"\n")
        sys.stdout.flush()
def serve(row):
    try: request=json.loads(row)
    except Exception: return
    stream=request.get("method")=="events.subscribe"
    try:
        c=socket.socket(socket.AF_UNIX)
        c.connect(path)
        c.sendall(row)
        for reply in c.makefile("rb"):
            emit(reply)
            if not stream: break
        c.close()
    except Exception as e:
        try: emit(json.dumps({"id":request.get("id"),"error":{"code":"bridge","message":str(e)}}).encode())
        except Exception: pass
    if stream: os._exit(0)
for row in sys.stdin.buffer:
    threading.Thread(target=serve,args=(row,),daemon=True).start()
os._exit(0)
"#;

pub fn bridge_command(session: &str) -> String {
    let script = BRIDGE.replace("SESSION", &serde_json::to_string(session).unwrap_or_else(|_| "\"default\"".into()));
    let hex: String = script.bytes().map(|b| format!("{b:02x}")).collect();
    format!("python3 -c 'exec(bytes.fromhex(\"{hex}\"))'")
}
