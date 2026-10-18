//! Herdr's live event stream: one connection to its socket holding an
//! `events.subscribe` request open, so status changes arrive the moment they
//! happen instead of with the next `herdr api snapshot`. Herdr serves one
//! request per connection, so changing the subscribed panes means opening a
//! new stream. Messages arrive on a channel the monitor drains each frame.

use crate::herdr::herdr_subscriptions;
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use transport::Conn;

#[cfg(unix)]
mod transport {
    pub use guild::ipc::UnixStream as Conn;

    pub fn connect(path: &str) -> std::io::Result<Conn> { Conn::connect(path) }

    pub fn close(conn: &Conn) { let _ = conn.shutdown(std::net::Shutdown::Both); }
}

#[cfg(windows)]
mod transport {
    use std::io::{Read, Write};
    use std::os::windows::io::AsRawHandle;
    use std::sync::Arc;

    // Shared, not duplicated: CancelIoEx only cancels a read issued on the same handle.
    #[derive(Clone)]
    pub struct Conn(Arc<std::fs::File>);

    impl Conn {
        pub fn try_clone(&self) -> std::io::Result<Conn> { Ok(self.clone()) }
    }

    impl Read for Conn {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> { (&*self.0).read(buf) }
    }

    impl Write for Conn {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> { (&*self.0).write(buf) }
        fn flush(&mut self) -> std::io::Result<()> { (&*self.0).flush() }
    }

    pub fn connect(path: &str) -> std::io::Result<Conn> {
        let prefix = r"\\.\pipe\";
        let pipe = if path.starts_with(prefix) { path.to_string() } else { format!("{prefix}{path}") };
        let file = std::fs::OpenOptions::new().read(true).write(true).open(pipe)?;
        Ok(Conn(Arc::new(file)))
    }

    pub fn close(conn: &Conn) {
        use windows_sys::Win32::System::IO::CancelIoEx;
        unsafe { CancelIoEx(conn.0.as_raw_handle(), std::ptr::null()); }
    }
}

pub enum StreamMessage {
    Open,
    Event(Value),
    Close,
}

pub struct HerdrStream {
    socket_path: String,
    subscriptions: Vec<Value>,
    connection: Arc<Mutex<Option<Conn>>>,
    stopped: Arc<Mutex<bool>>,
    /// Which stream a message came from, so a replaced stream's are ignored.
    pub generation: u64,
}

impl HerdrStream {
    pub fn new(socket_path: &str, panes: &[String], generation: u64) -> HerdrStream {
        HerdrStream {
            socket_path: socket_path.to_string(),
            subscriptions: herdr_subscriptions(panes),
            connection: Arc::new(Mutex::new(None)),
            stopped: Arc::new(Mutex::new(false)),
            generation,
        }
    }

    pub fn start(&self, sender: Sender<(u64, StreamMessage)>) {
        let path = self.socket_path.clone();
        let request = serde_json::json!({ "id": "hpd-events", "method": "events.subscribe", "params": { "subscriptions": self.subscriptions } });
        let connection = self.connection.clone();
        let stopped = self.stopped.clone();
        let generation = self.generation;
        std::thread::Builder::new()
            .name("herdr-events".into())
            .spawn(move || {
                let quiet = run(&path, &request, &connection, &stopped, generation, &sender);
                *connection.lock().unwrap_or_else(|e| e.into_inner()) = None;
                if !quiet {
                    let _ = sender.send((generation, StreamMessage::Close));
                }
            })
            .ok();
    }

    pub fn stop(&self) {
        *self.stopped.lock().unwrap_or_else(|e| e.into_inner()) = true;
        if let Some(stream) = self.connection.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            transport::close(stream);
        }
    }
}

/// Serve the stream until it ends; true when it was stopped on purpose.
fn run(path: &str, request: &Value, connection: &Arc<Mutex<Option<Conn>>>, stopped: &Arc<Mutex<bool>>, generation: u64, sender: &Sender<(u64, StreamMessage)>) -> bool {
    let Ok(mut stream) = transport::connect(path) else { return *stopped.lock().unwrap() };
    let Ok(reader) = stream.try_clone() else { return *stopped.lock().unwrap() };
    {
        let mut slot = connection.lock().unwrap_or_else(|e| e.into_inner());
        if *stopped.lock().unwrap() { return true; }
        *slot = Some(reader);
    }
    let mut line = request.to_string();
    line.push('\n');
    if stream.write_all(line.as_bytes()).is_err() { return *stopped.lock().unwrap(); }
    // CancelIoEx misses a read that has not started yet, so a stop() before it is caught here.
    if *stopped.lock().unwrap_or_else(|e| e.into_inner()) { return true; }
    let mut opened = false;
    for row in BufReader::new(stream).lines() {
        if *stopped.lock().unwrap_or_else(|e| e.into_inner()) { break; }
        let Ok(row) = row else { break };
        let Ok(object) = serde_json::from_str::<Value>(&row) else { continue };
        if !opened {
            // The first reply confirms the subscription; an error ends the stream.
            let started = object.get("result").and_then(|r| r.get("type")).and_then(Value::as_str) == Some("subscription_started");
            if !started { break; }
            opened = true;
            let _ = sender.send((generation, StreamMessage::Open));
        } else {
            let _ = sender.send((generation, StreamMessage::Event(object)));
        }
    }
    *stopped.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{channel, RecvTimeoutError};
    use std::time::Duration;

    #[test]
    #[ignore = "needs a running Herdr: cargo test -- --ignored"]
    fn subscribes_to_a_running_herdr_and_stops_on_demand() {
        let socket = crate::herdr::herdr_socket("default").expect("a running Herdr session");
        let (tx, rx) = channel();
        let stream = HerdrStream::new(&socket, &[], 1);
        stream.start(tx);
        assert!(matches!(rx.recv_timeout(Duration::from_secs(3)), Ok((1, StreamMessage::Open))), "the subscription did not open");
        stream.stop();
        loop {
            match rx.recv_timeout(Duration::from_secs(3)) {
                Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => panic!("stop() did not end the stream"),
                Ok((_, StreamMessage::Close)) => panic!("a stream stopped on purpose stays quiet"),
                Ok(_) => continue,
            }
        }
    }
}
