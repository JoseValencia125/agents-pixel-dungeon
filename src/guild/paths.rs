//! Where the guild keeps its database, worktrees, artifacts and socket:
//! the app's data directory, or `HPD_GUILD_HOME` for tests and scratch
//! instances.

use std::path::PathBuf;

pub fn home() -> PathBuf {
    if let Ok(p) = std::env::var("HPD_GUILD_HOME") {
        if !p.is_empty() { return PathBuf::from(p); }
    }
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("agents-pixel-dungeon").join("guild")
}

pub fn db_path() -> PathBuf { home().join("guild.sqlite") }
pub fn socket_path() -> PathBuf {
    if let Ok(p) = std::env::var("HPD_GUILD_SOCKET") {
        if !p.is_empty() { return PathBuf::from(p); }
    }
    let preferred = home().join("guild.sock");
    // Unix socket paths are limited to ~100 bytes; a deep data folder
    // gets a short name in the temp dir keyed by its own path.
    if preferred.as_os_str().len() < 96 {
        return preferred;
    }
    let digest = <sha2::Sha256 as sha2::Digest>::digest(home().as_os_str().as_encoded_bytes());
    let key = digest[..6].iter().fold(0u64, |acc, b| (acc << 8) | *b as u64);
    std::env::temp_dir().join(format!("hpd-guild-{key:x}.sock"))
}
pub fn lock_path() -> PathBuf { home().join("guild.lock") }
pub fn log_path() -> PathBuf { home().join("guild.log") }
pub fn worktrees_dir() -> PathBuf { home().join("worktrees") }
pub fn artifacts_dir() -> PathBuf { home().join("artifacts") }

/// Tests that point `HPD_GUILD_HOME` at a scratch folder hold this, so
/// they do not pull the folder from under each other.
#[doc(hidden)]
pub fn test_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    &LOCK
}

pub fn ensure_home() -> std::io::Result<PathBuf> {
    let h = home();
    std::fs::create_dir_all(&h)?;
    std::fs::create_dir_all(worktrees_dir())?;
    std::fs::create_dir_all(artifacts_dir())?;
    Ok(h)
}

/// An attempt's own folder for scratch files (scripts, logs, data), in the
/// system's temporary folder so the permission policy lets members use it;
/// the guild deletes it when the attempt ends.
pub fn scratch_dir(attempt_id: &str) -> PathBuf { std::env::temp_dir().join(format!("guild-{attempt_id}")) }
