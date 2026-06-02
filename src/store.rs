//! SQLite-backed clipboard history.
//!
//! Newest entries sort first; pinned entries always float to the top and are
//! exempt from trimming. Content is unique — re-copying an existing item simply
//! bumps its timestamp instead of creating a duplicate.

use rusqlite::{params, Connection};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::MAX_UNPINNED;

/// One stored clipboard item.
#[derive(Clone)]
pub struct Entry {
    pub id: i64,
    pub content: String,
    pub pinned: bool,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (creating if absent) the history database at `path`.
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS history (
                id        INTEGER PRIMARY KEY AUTOINCREMENT,
                content   TEXT    NOT NULL UNIQUE,
                pinned    INTEGER NOT NULL DEFAULT 0,
                copied_at INTEGER NOT NULL
            );",
        )?;
        Ok(Self { conn })
    }

    /// Record a freshly copied value. Empty/whitespace-only content is ignored.
    pub fn record(&self, content: &str) -> rusqlite::Result<()> {
        if content.trim().is_empty() {
            return Ok(());
        }
        self.conn.execute(
            "INSERT INTO history (content, pinned, copied_at) VALUES (?1, 0, ?2)
             ON CONFLICT(content) DO UPDATE SET copied_at = ?2",
            params![content, now_millis()],
        )?;
        self.trim()
    }

    /// All entries, pinned first, then most-recently-copied first.
    pub fn list(&self) -> rusqlite::Result<Vec<Entry>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, content, pinned FROM history ORDER BY pinned DESC, copied_at DESC")?;
        let rows = stmt.query_map([], |r| {
            Ok(Entry {
                id: r.get(0)?,
                content: r.get(1)?,
                pinned: r.get::<_, i64>(2)? != 0,
            })
        })?;
        rows.collect()
    }

    pub fn toggle_pin(&self, id: i64) -> rusqlite::Result<()> {
        self.conn
            .execute("UPDATE history SET pinned = 1 - pinned WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn delete(&self, id: i64) -> rusqlite::Result<()> {
        self.conn
            .execute("DELETE FROM history WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Remove all unpinned entries.
    pub fn clear(&self) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM history WHERE pinned = 0", [])?;
        Ok(())
    }

    /// Drop the oldest unpinned rows beyond the configured cap.
    fn trim(&self) -> rusqlite::Result<()> {
        self.conn.execute(
            "DELETE FROM history WHERE pinned = 0 AND id NOT IN (
                SELECT id FROM history WHERE pinned = 0 ORDER BY copied_at DESC LIMIT ?1
            )",
            params![MAX_UNPINNED as i64],
        )?;
        Ok(())
    }
}

/// Wall-clock milliseconds since the epoch. Millisecond precision keeps rapid
/// successive copies in their true order (second-level granularity would tie).
fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
