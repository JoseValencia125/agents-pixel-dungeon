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

/// Processes still running inside these folders (their working directory
/// is one of them or below), but the harnesses themselves: what a member
/// left in the background when its turn ended. Empty where `lsof` is not.
pub fn processes_in(dirs: &[&Path]) -> Vec<(i32, String)> {
    if cfg!(windows) { return vec![]; }
    let Ok(out) = command("lsof").args(["-n", "-P", "-a", "-d", "cwd", "-F", "pcn"]).output() else { return vec![] };
    let me = std::process::id() as i32;
    let skip = harness_pids();
    // lsof gives resolved paths (/private/var/… for /var/…).
    let dirs: Vec<PathBuf> = dirs.iter().flat_map(|d| [d.to_path_buf(), d.canonicalize().unwrap_or_else(|_| d.to_path_buf())]).collect();
    let (mut found, mut pid, mut name) = (vec![], 0, String::new());
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        match line.split_at(line.len().min(1)) {
            ("p", rest) => { pid = rest.parse().unwrap_or(0); name.clear(); }
            ("c", rest) => name = rest.to_string(),
            ("n", rest) => {
                let inside = dirs.iter().any(|d| Path::new(rest).starts_with(d));
                if inside && !is_harness_name(&name) && !skip.contains(&pid) && pid != me && pid > 0 { found.push((pid, name.clone())); }
            }
            _ => {}
        }
    }
    found
}

/// The processes an attempt started and left running (FB-070): those whose
/// environment carries its `GUILD_ATTEMPT` mark, wherever they work, and
/// those working inside `dirs`; never the harnesses. A process that cd'd
/// out of the worktree, or was started by a node tool, no longer slips by.
pub fn attempt_processes(attempt_id: &str, dirs: &[&Path]) -> Vec<(i32, String)> {
    let mut found = processes_in(dirs);
    if cfg!(windows) || attempt_id.is_empty() { return found; }
    let mark = format!("GUILD_ATTEMPT={attempt_id}");
    let me = std::process::id() as i32;
    let skip = harness_pids();
    for (pid, _, name) in process_table() {
        if is_harness_name(&name) || skip.contains(&pid) || pid == me || pid <= 0 || found.iter().any(|(p, _)| *p == pid) { continue; }
        if process_environment(pid).iter().any(|e| *e == mark) { found.push((pid, name)); }
    }
    found
}

/// (pid, parent pid, command name) of every process; empty where `ps` is not.
fn process_table() -> Vec<(i32, i32, String)> {
    let Ok(out) = command("ps").args(["-axo", "pid=,ppid=,comm="]).output() else { return vec![] };
    String::from_utf8_lossy(&out.stdout).lines().filter_map(|line| {
        let mut parts = line.split_whitespace();
        let (pid, ppid) = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
        let comm = line.trim_start().splitn(3, char::is_whitespace).nth(2).unwrap_or("").trim();
        let name = Path::new(comm).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        Some((pid, ppid, name))
    }).collect()
}

/// A harness, or a tool that is one's own (never what a member left behind):
/// by name, Claude Code too under its versioned binary (`2.1.312`).
fn is_harness_name(name: &str) -> bool {
    let versioned = name.contains('.') && name.chars().all(|c| c.is_ascii_digit() || c == '.');
    versioned || name.starts_with("kiro-cli") || ["claude", "codex", "node", "lsof", "ps"].contains(&name)
}

/// The harnesses' processes and the servers each runs for itself (MCP, LSP):
/// what a harness-named process started directly, other than a shell: the
/// member's own commands run in a shell, its servers do not (FB-076).
fn harness_pids() -> std::collections::HashSet<i32> {
    let table = process_table();
    let harness: std::collections::HashSet<i32> = table.iter().filter(|(_, _, n)| is_harness_name(n)).map(|(p, _, _)| *p).collect();
    let shells = ["sh", "bash", "zsh", "fish", "dash", "ksh", "tcsh"];
    table.iter().filter(|(p, parent, n)| harness.contains(p) || (harness.contains(parent) && !shells.contains(&n.as_str()))).map(|(p, _, _)| *p).collect()
}

/// The environment variables of a process of ours ("NAME=value"); empty
/// where they cannot be read (another user's, or gone).
#[cfg(target_os = "linux")]
fn process_environment(pid: i32) -> Vec<String> {
    std::fs::read(format!("/proc/{pid}/environ")).map(|b| b.split(|c| *c == 0).map(|v| String::from_utf8_lossy(v).to_string()).collect()).unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn process_environment(pid: i32) -> Vec<String> {
    // KERN_PROCARGS2: argc, the executable's path, padding NULs, argv, then the environment.
    let mut size: libc::size_t = 0;
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let buf = unsafe {
        if libc::sysctl(mib.as_mut_ptr(), 3, std::ptr::null_mut(), &mut size, std::ptr::null_mut(), 0) != 0 || size < 4 { return vec![]; }
        let mut buf = vec![0u8; size];
        if libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr() as *mut libc::c_void, &mut size, std::ptr::null_mut(), 0) != 0 { return vec![]; }
        buf.truncate(size);
        buf
    };
    let argc = i32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]).max(0) as usize;
    let mut parts = buf[4..].split(|c| *c == 0);
    // The path, then the empty strings that pad it, then argc arguments.
    let _ = parts.next();
    let mut rest = parts.skip_while(|p| p.is_empty());
    for _ in 0..argc { if rest.next().is_none() { return vec![]; } }
    rest.take_while(|p| !p.is_empty()).map(|v| String::from_utf8_lossy(v).to_string()).collect()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn process_environment(_pid: i32) -> Vec<String> { vec![] }

/// Stop what an attempt left running (FB-075): SIGTERM, and SIGKILL for
/// whatever is still there after `grace`. Returns how many it signalled.
pub fn reap_attempt_processes(attempt_id: &str, dirs: &[&Path], grace: std::time::Duration) -> usize {
    #[cfg(unix)]
    {
        let found = attempt_processes(attempt_id, dirs);
        for (pid, _) in &found { unsafe { libc::kill(*pid, libc::SIGTERM); } }
        if found.is_empty() { return 0; }
        let deadline = std::time::Instant::now() + grace;
        while std::time::Instant::now() < deadline && found.iter().any(|(pid, _)| unsafe { libc::kill(*pid, 0) } == 0) {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        for (pid, _) in &found { if unsafe { libc::kill(*pid, 0) } == 0 { unsafe { libc::kill(*pid, libc::SIGKILL); } } }
        found.len()
    }
    #[cfg(not(unix))]
    { let _ = (attempt_id, dirs, grace); 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    #[test]
    #[cfg(unix)]
    fn what_runs_in_a_folder_is_found_and_the_harness_is_not() {
        if which("lsof").is_none() { return; }
        let dir = std::env::temp_dir().join(format!("guild-bg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut child = Command::new("sleep").arg("30").current_dir(&dir).stdout(Stdio::null()).spawn().unwrap();
        let found = processes_in(&[dir.as_path()]);
        let _ = child.kill();
        let _ = child.wait();
        assert!(found.iter().any(|(pid, name)| *pid == child.id() as i32 && name == "sleep"), "{found:?}");
        assert!(processes_in(&[Path::new("/nonexistent/guild")]).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What the test below starts, to have something to find.
    #[test]
    #[ignore]
    fn a_sleeper_for_the_mark_test() { std::thread::sleep(Duration::from_secs(30)); }

    #[test]
    #[cfg(unix)]
    fn what_an_attempt_left_running_is_found_by_its_mark_and_reaped() {
        let dir = std::env::temp_dir().join(format!("guild-mark-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // It works elsewhere, so only its environment says whose it is. The test binary
        // sleeps in its own right: macOS hides the environment of its system binaries.
        let mut child = Command::new(std::env::current_exe().unwrap()).args(["--ignored", "--exact", "platform::tests::a_sleeper_for_the_mark_test"]).current_dir(std::env::temp_dir()).env("GUILD_ATTEMPT", "att-mark-1").stdout(Stdio::null()).spawn().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let found = attempt_processes("att-mark-1", &[dir.as_path()]);
        assert!(found.iter().any(|(pid, name)| *pid == child.id() as i32 && name.contains("guild")), "{found:?}");
        assert!(attempt_processes("att-other", &[dir.as_path()]).iter().all(|(pid, _)| *pid != child.id() as i32));
        assert!(reap_attempt_processes("att-mark-1", &[dir.as_path()], Duration::from_secs(2)) >= 1);
        assert!(child.wait().is_ok(), "it was stopped");
        assert!(attempt_processes("att-mark-1", &[dir.as_path()]).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_harness_is_known_by_name_also_under_its_versioned_binary() {
        // FB-076: lsof names Claude Code by its versioned binary, and it counted as what the member left running.
        for harness in ["claude", "2.1.312", "node", "codex", "kiro-cli-chat", "lsof"] { assert!(is_harness_name(harness), "{harness}"); }
        for other in ["blender", "godot", "python3", "sleep", "Godot", "run_tests.sh"] { assert!(!is_harness_name(other), "{other}"); }
    }

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
