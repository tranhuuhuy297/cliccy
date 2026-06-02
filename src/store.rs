//! SQLite-backed clipboard history (text and PNG images).
//!
//! Newest entries sort first; pinned entries always float to the top and are
//! exempt from trimming. Each entry has a `dedup` key (text by value, image by
//! hash) — re-copying the same content bumps its timestamp instead of adding a
//! duplicate.

use rusqlite::{params, Connection};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::clipboard_backend::image_key;
use crate::config::MAX_UNPINNED;

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Text,
    Image,
}

/// One stored clipboard item. Exactly one of `text`/`image` is set per `kind`.
#[derive(Clone)]
pub struct Entry {
    pub id: i64,
    pub kind: Kind,
    pub text: Option<String>,
    pub image: Option<Vec<u8>>,
    pub pinned: bool,
    /// Wall-clock milliseconds since the epoch of the last copy — drives the
    /// relative "time ago" shown on each row.
    pub copied_at: i64,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (creating/migrating if needed) the history database at `path`.
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        migrate(&conn)?;
        Ok(Self { conn })
    }

    /// Record copied text. Empty/whitespace-only content is ignored.
    pub fn record_text(&self, text: &str) -> rusqlite::Result<()> {
        if text.trim().is_empty() {
            return Ok(());
        }
        self.upsert("text", Some(text), None, text)
    }

    /// Record a copied PNG image.
    pub fn record_image(&self, bytes: &[u8]) -> rusqlite::Result<()> {
        let key = image_key(bytes);
        self.upsert("image", None, Some(bytes), &key)
    }

    fn upsert(
        &self,
        kind: &str,
        content: Option<&str>,
        data: Option<&[u8]>,
        dedup: &str,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO history (kind, content, data, dedup, pinned, copied_at)
             VALUES (?1, ?2, ?3, ?4, 0, ?5)
             ON CONFLICT(dedup) DO UPDATE SET copied_at = ?5",
            params![kind, content, data, dedup, now_millis()],
        )?;
        self.trim()
    }

    /// All entries, pinned first, then most-recently-copied first.
    pub fn list(&self) -> rusqlite::Result<Vec<Entry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, content, data, pinned, copied_at FROM history ORDER BY pinned DESC, copied_at DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            let kind = if r.get::<_, String>(1)? == "image" {
                Kind::Image
            } else {
                Kind::Text
            };
            Ok(Entry {
                id: r.get(0)?,
                kind,
                text: r.get(2)?,
                image: r.get(3)?,
                pinned: r.get::<_, i64>(4)? != 0,
                copied_at: r.get(5)?,
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

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS history (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    kind      TEXT    NOT NULL DEFAULT 'text',
    content   TEXT,
    data      BLOB,
    dedup     TEXT    NOT NULL UNIQUE,
    pinned    INTEGER NOT NULL DEFAULT 0,
    copied_at INTEGER NOT NULL
);";

/// Create the table, upgrading the pre-image schema (text-only `content UNIQUE`)
/// in place while preserving existing rows and pins.
fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    if table_exists(conn, "history")? && !column_exists(conn, "history", "kind")? {
        conn.execute_batch(
            "BEGIN;
             ALTER TABLE history RENAME TO history_old;
             CREATE TABLE history (
                id        INTEGER PRIMARY KEY AUTOINCREMENT,
                kind      TEXT    NOT NULL DEFAULT 'text',
                content   TEXT,
                data      BLOB,
                dedup     TEXT    NOT NULL UNIQUE,
                pinned    INTEGER NOT NULL DEFAULT 0,
                copied_at INTEGER NOT NULL
             );
             INSERT INTO history (kind, content, data, dedup, pinned, copied_at)
                SELECT 'text', content, NULL, content, pinned, copied_at FROM history_old;
             DROP TABLE history_old;
             COMMIT;",
        )
    } else {
        conn.execute_batch(SCHEMA)
    }
}

fn table_exists(conn: &Connection, name: &str) -> rusqlite::Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
        params![name],
        |r| r.get(0),
    )?;
    Ok(count > 0)
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = stmt.query_map([], |r| r.get::<_, String>(1))?;
    for name in names {
        if name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Wall-clock milliseconds since the epoch. Millisecond precision keeps rapid
/// successive copies in their true order (second-level granularity would tie).
fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
