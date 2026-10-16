use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| in_dir(&dir, name))
}

pub fn in_dir(dir: &Path, name: &str) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        if Path::new(name).extension().is_some() {
            let p = dir.join(name);
            if is_executable(&p) { return Some(p); }
        }
        let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        pathext.split(';').filter(|e| !e.is_empty()).map(|ext| dir.join(format!("{name}{}", ext.to_ascii_lowercase()))).find(|p| is_executable(p))
    }
    #[cfg(not(windows))]
    {
        let p = dir.join(name);
        is_executable(&p).then_some(p)
    }
}

pub fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
    }
    #[cfg(windows)]
    {
        let runnable = path.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "exe" | "cmd" | "bat" | "com"));
        runnable && path.is_file()
    }
}

pub fn command<S: AsRef<std::ffi::OsStr>>(program: S) -> Command {
    let mut cmd = Command::new(program);
    no_window(&mut cmd);
    cmd
}

pub fn no_window(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

pub fn detach(cmd: &mut Command) -> &mut Command {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Not DETACHED_PROCESS: every console program the child starts would open its own window.
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
    cmd
}

/// Windows children inherit every inheritable handle: a daemon spawned from here would hold the
/// caller's stdout pipe open for its whole life, and the caller's `Command::output` would never return.
pub fn keep_stdio_private() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE_FLAG_INHERIT};
        use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
        for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let handle = GetStdHandle(which);
            if !handle.is_null() { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0); }
        }
    }
}

pub fn terminate_tree(pid: i32, force: bool) {
    // killpg(0) signals our own process group.
    if pid <= 0 { return; }
    #[cfg(unix)]
    unsafe {
        libc::killpg(pid, if force { libc::SIGKILL } else { libc::SIGTERM });
    }
    #[cfg(windows)]
    {
        let _ = force;
        let _ = command("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
    }
}

pub fn interrupt_tree(pid: i32) {
    if pid <= 0 { return; }
    #[cfg(unix)]
    unsafe {
        libc::killpg(pid, libc::SIGINT);
    }
    #[cfg(windows)]
    terminate_tree(pid, true);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    #[test]
    fn finds_a_runnable_file_by_its_bare_name() {
        let dir = std::env::temp_dir().join(format!("hpd-platform-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(if cfg!(windows) { "hpdtool.exe" } else { "hpdtool" });
        std::fs::write(&file, b"").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(in_dir(&dir, "hpdtool"), Some(file));
        assert!(in_dir(&dir, "nothing").is_none());
        let _ = std::fs::remove_dir_all(&dir);
        assert!(which("definitely-not-a-program-hpd").is_none());
    }

    #[test]
    fn a_detached_child_ends_when_terminated() {
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "ping -n 60 127.0.0.1 > nul"]);
            c
        } else {
            let mut c = Command::new("sh");
            c.args(["-c", "sleep 60 & wait"]);
            c
        };
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        detach(&mut cmd);
        let mut child = cmd.spawn().expect("a shell to start");
        terminate_tree(child.id() as i32, true);
        let started = Instant::now();
        while child.try_wait().unwrap().is_none() {
            assert!(started.elapsed() < Duration::from_secs(10), "the child outlived terminate_tree");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn a_pid_that_cannot_be_a_group_is_ignored() {
        terminate_tree(0, true);
        terminate_tree(-1, true);
        interrupt_tree(0);
    }

    #[test]
    fn a_directory_is_not_executable() {
        assert!(!is_executable(&std::env::temp_dir()));
    }
}
