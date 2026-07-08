//! Shared constants and filesystem paths for Cliccy.

use std::path::PathBuf;

/// GApplication ID. Drives single-instance behaviour: the first launch becomes
/// the resident daemon; later `cliccy toggle` invocations forward to it.
pub const APP_ID: &str = "com.cliccy.Cliccy";

/// Maximum number of unpinned history entries kept; pinned entries are never trimmed.
pub const MAX_UNPINNED: usize = 20;

/// Per-user data directory, e.g. ~/.local/share/cliccy.
pub fn data_dir() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("cliccy")
}

/// Path to the SQLite history database, creating the parent directory if needed.
pub fn db_path() -> PathBuf {
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    dir.join("history.db")
}

/// Path to the daemon's control socket. Lives in `$XDG_RUNTIME_DIR` (a tmpfs the
/// session manager cleans up on logout) so a stale socket never outlives the
/// daemon across reboots. Falls back to the data dir when the runtime dir is
/// unset (e.g. a non-session shell), where a leftover socket file is harmless —
/// `bind` unlinks it first.
pub fn socket_path() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) => PathBuf::from(dir).join("cliccy.sock"),
        None => data_dir().join("cliccy.sock"),
    }
}
