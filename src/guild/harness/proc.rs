//! A child process that talks in lines: stdin and stdout piped, stderr
//! kept in a log under the guild's home, started in its own process
//! group so stopping it takes its helpers too and nothing else. Shared
//! by the adapters that drive a CLI (Codex, Claude Code).

use super::{HarnessError, HarnessResult};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct LineProc {
    child: Mutex<Child>,
    pub pgid: i32,
    stdin: Mutex<Option<ChildStdin>>,
    stdout: Mutex<Option<ChildStdout>>,
    pub log_path: PathBuf,
}

impl LineProc {
    /// Start `cmd` in `cwd` with its stderr in `<home>/logs/<log_name>.stderr.log`.
    pub fn spawn(mut cmd: Command, cwd: &Path, log_name: &str) -> HarnessResult<Arc<LineProc>> {
        let logs = crate::paths::home().join("logs");
        let _ = std::fs::create_dir_all(&logs);
        let log_path = logs.join(format!("{log_name}.stderr.log"));
        let stderr = std::fs::File::create(&log_path).map_err(|e| HarnessError::RuntimeFailed(e.to_string()))?;
        cmd.current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::from(stderr));
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd.spawn().map_err(|e| HarnessError::RuntimeFailed(format!("cannot start {:?}: {e}", cmd.get_program())))?;
        let pgid = child.id() as i32;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        Ok(Arc::new(LineProc { child: Mutex::new(child), pgid, stdin: Mutex::new(stdin), stdout: Mutex::new(stdout), log_path }))
    }

    pub fn write_line(&self, line: &str) -> HarnessResult<()> {
        let mut guard = self.stdin.lock().unwrap();
        let Some(stdin) = guard.as_mut() else { return Err(HarnessError::TransportLost("stdin closed".into())) };
        stdin.write_all(line.as_bytes()).and_then(|_| stdin.write_all(b"\n")).and_then(|_| stdin.flush()).map_err(|e| HarnessError::TransportLost(e.to_string()))
    }

    /// Close stdin: the CLI sees end of input.
    pub fn close_stdin(&self) {
        *self.stdin.lock().unwrap() = None;
    }

    /// The stdout to read from; once.
    pub fn take_stdout(&self) -> Option<ChildStdout> {
        self.stdout.lock().unwrap().take()
    }

    /// The exit code if the process ended.
    pub fn exit_code(&self) -> Option<Option<i32>> {
        match self.child.lock().unwrap().try_wait() {
            Ok(Some(status)) => Some(status.code()),
            _ => None,
        }
    }

    /// Ask the process group to end, then force it. True when it ended.
    pub fn kill(&self) -> bool {
        self.close_stdin();
        unsafe {
            libc::killpg(self.pgid, libc::SIGTERM);
        }
        let mut child = self.child.lock().unwrap();
        for _ in 0..30 {
            if let Ok(Some(_)) = child.try_wait() { return true; }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = child.kill();
        unsafe {
            libc::killpg(self.pgid, libc::SIGKILL);
        }
        matches!(child.try_wait(), Ok(Some(_)))
    }

    /// Send SIGINT to the group, as ctrl-c would.
    pub fn interrupt(&self) {
        unsafe {
            libc::killpg(self.pgid, libc::SIGINT);
        }
    }
}

/// Find a CLI: `$env_key`, then PATH, then the usual places.
pub fn find_binary(env_key: &str, name: &str, candidates: &[&str]) -> PathBuf {
    if let Ok(p) = std::env::var(env_key) {
        if !p.trim().is_empty() { return PathBuf::from(p); }
    }
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            let p = Path::new(dir).join(name);
            if p.is_file() { return p; }
        }
    }
    let home = dirs::home_dir().unwrap_or_default();
    for c in candidates {
        let p = if let Some(rest) = c.strip_prefix("~/") { home.join(rest) } else { PathBuf::from(c) };
        if p.is_file() { return p; }
    }
    PathBuf::from(name)
}

/// The environment a nested CLI should not inherit from a Claude Code
/// session running the guild.
pub fn scrub_env(cmd: &mut Command) {
    for (k, _) in std::env::vars() {
        if k == "CLAUDECODE" || k.starts_with("CLAUDE_CODE_") || k == "CLAUDE_PID" || k == "CLAUDE_EFFORT" {
            cmd.env_remove(k);
        }
    }
}

/// A one-line summary of a shell command for the room's transcript.
pub fn command_title(command: &str) -> String {
    let mut s = command.trim();
    for prefix in ["/bin/zsh -lc ", "/bin/bash -lc ", "zsh -lc ", "bash -lc ", "sh -c "] {
        if let Some(rest) = s.strip_prefix(prefix) { s = rest.trim_matches('"').trim_matches('\''); break; }
    }
    let line = s.lines().next().unwrap_or("").trim();
    let mut out: String = line.chars().take(80).collect();
    if line.chars().count() > 80 || s.lines().count() > 1 { out.push('…'); }
    out
}
