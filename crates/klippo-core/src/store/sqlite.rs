//! SQLite-backed history store (bundled SQLite, WAL mode).
//!
//! The content hash is the primary key, so inserting identical content is an
//! upsert that just bumps the timestamp (MRU promotion). After each insert the
//! store prunes to `max_items` and reports the removed entries so the caller
//! can garbage-collect any orphaned image files.

use std::path::Path;

use rusqlite::{params, Connection, Row};

use crate::error::Result;
use crate::model::{Entry, EntryKind};

const SCHEMA_VERSION: i64 = 1;

const SELECT_COLS: &str = "id, kind, text, image_path, thumb_path, preview, timestamp_ms, pinned";

/// Handle to the on-disk (or in-memory) history database.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (creating if needed) the database at `path`.
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;",
        )?;
        let store = Store { conn };
        store.migrate()?;
        Ok(store)
    }

    /// Open an ephemeral in-memory database (used by tests).
    pub fn open_in_memory() -> Result<Store> {
        let store = Store {
            conn: Connection::open_in_memory()?,
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        let version: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 1 {
            self.conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS entries (
                    id           TEXT PRIMARY KEY,
                    kind         INTEGER NOT NULL,
                    text         TEXT,
                    image_path   TEXT,
                    thumb_path   TEXT,
                    preview      TEXT NOT NULL,
                    timestamp_ms INTEGER NOT NULL,
                    pinned       INTEGER NOT NULL DEFAULT 0
                 );
                 CREATE INDEX IF NOT EXISTS idx_entries_order
                     ON entries(pinned DESC, timestamp_ms DESC);",
            )?;
        }
        self.conn
            .pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(())
    }

    /// Insert `entry`, or bump the timestamp of existing identical content, then
    /// prune to `max_items`. Returns entries removed by pruning (for image GC).
    pub fn upsert(&self, entry: &Entry, max_items: u32) -> Result<Vec<Entry>> {
        self.conn.execute(
            "INSERT INTO entries (id, kind, text, image_path, thumb_path, preview, timestamp_ms, pinned)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
                 timestamp_ms = excluded.timestamp_ms,
                 preview      = excluded.preview",
            params![
                entry.id,
                kind_to_i64(entry.kind),
                entry.text,
                entry.image_path.as_ref().map(|p| path_to_string(p)),
                entry.thumb_path.as_ref().map(|p| path_to_string(p)),
                entry.preview,
                entry.timestamp_ms,
                entry.pinned as i64,
            ],
        )?;
        self.prune(max_items)
    }

    /// Promote an existing entry to the top of the MRU order.
    pub fn touch(&self, id: &str, now_ms: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE entries SET timestamp_ms = ?2 WHERE id = ?1",
            params![id, now_ms],
        )?;
        Ok(())
    }

    /// The most recent `limit` entries, newest first (pinned above all).
    pub fn list(&self, limit: u32) -> Result<Vec<Entry>> {
        let sql = format!(
            "SELECT {SELECT_COLS} FROM entries
             ORDER BY pinned DESC, timestamp_ms DESC LIMIT ?1"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([limit], row_to_entry)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Fetch a single entry by id.
    pub fn get(&self, id: &str) -> Result<Option<Entry>> {
        let sql = format!("SELECT {SELECT_COLS} FROM entries WHERE id = ?1");
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query_map([id], row_to_entry)?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    /// Remove one entry, returning it (so its image files can be cleaned up).
    pub fn remove(&self, id: &str) -> Result<Option<Entry>> {
        let existing = self.get(id)?;
        if existing.is_some() {
            self.conn
                .execute("DELETE FROM entries WHERE id = ?1", [id])?;
        }
        Ok(existing)
    }

    /// Remove every entry, returning them all (for image GC).
    pub fn clear(&self) -> Result<Vec<Entry>> {
        let all = self.list(u32::MAX)?;
        self.conn.execute("DELETE FROM entries", [])?;
        Ok(all)
    }

    /// Number of stored entries.
    pub fn count(&self) -> Result<u32> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0))?;
        Ok(n as u32)
    }

    /// Flush the WAL into the main database file (called on hide / shutdown).
    pub fn checkpoint(&self) -> Result<()> {
        self.conn
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    fn prune(&self, max_items: u32) -> Result<Vec<Entry>> {
        let sql = format!(
            "SELECT {SELECT_COLS} FROM entries
             ORDER BY pinned DESC, timestamp_ms DESC LIMIT -1 OFFSET ?1"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let pruned = stmt
            .query_map([max_items], row_to_entry)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for e in &pruned {
            self.conn
                .execute("DELETE FROM entries WHERE id = ?1", [&e.id])?;
        }
        Ok(pruned)
    }
}

fn kind_to_i64(kind: EntryKind) -> i64 {
    match kind {
        EntryKind::Text => 0,
        EntryKind::Image => 1,
    }
}

fn kind_from_i64(v: i64) -> EntryKind {
    if v == 1 {
        EntryKind::Image
    } else {
        EntryKind::Text
    }
}

fn path_to_string(p: &std::path::Path) -> String {
    p.to_string_lossy().into_owned()
}

fn row_to_entry(row: &Row) -> rusqlite::Result<Entry> {
    let image_path: Option<String> = row.get(3)?;
    let thumb_path: Option<String> = row.get(4)?;
    Ok(Entry {
        id: row.get(0)?,
        kind: kind_from_i64(row.get(1)?),
        text: row.get(2)?,
        image_path: image_path.map(Into::into),
        thumb_path: thumb_path.map(Into::into),
        preview: row.get(5)?,
        timestamp_ms: row.get(6)?,
        pinned: row.get::<_, i64>(7)? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &str, ts: i64) -> Entry {
        Entry::new_text(t, ts)
    }

    #[test]
    fn dedup_promotes_existing_to_top() {
        let s = Store::open_in_memory().unwrap();
        s.upsert(&text("a", 1), 25).unwrap();
        s.upsert(&text("b", 2), 25).unwrap();
        s.upsert(&text("a", 3), 25).unwrap(); // same content as first, newer

        let list = s.list(25).unwrap();
        assert_eq!(list.len(), 2, "duplicate must not create a new row");
        assert_eq!(list[0].text.as_deref(), Some("a"), "promoted to top");
        assert_eq!(list[1].text.as_deref(), Some("b"));
    }

    #[test]
    fn prune_enforces_max_items() {
        let s = Store::open_in_memory().unwrap();
        for i in 0..30 {
            s.upsert(&text(&format!("item{i}"), i), 25).unwrap();
        }
        assert_eq!(s.count().unwrap(), 25);
        let list = s.list(100).unwrap();
        assert_eq!(list[0].text.as_deref(), Some("item29"), "newest on top");
        assert_eq!(list.last().unwrap().text.as_deref(), Some("item5"));
    }

    #[test]
    fn remove_and_clear() {
        let s = Store::open_in_memory().unwrap();
        let a = text("x", 1);
        s.upsert(&a, 25).unwrap();
        s.upsert(&text("y", 2), 25).unwrap();

        assert!(s.remove(&a.id).unwrap().is_some());
        assert!(
            s.remove(&a.id).unwrap().is_none(),
            "second remove is a no-op"
        );
        assert_eq!(s.count().unwrap(), 1);

        let cleared = s.clear().unwrap();
        assert_eq!(cleared.len(), 1);
        assert_eq!(s.count().unwrap(), 0);
    }
}
