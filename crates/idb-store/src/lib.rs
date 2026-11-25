//! Persistent download journal backed by SQLite in WAL mode.
//!
//! The journal is the source of truth for resuming after a crash. Invariant
//! kept by the engine: `downloaded` is only advanced **after** those bytes
//! were flushed with `fdatasync`, so it never exceeds what is durable on disk.

use std::path::Path;

use idb_types::{DownloadId, DownloadState};
use rusqlite::{Connection, OptionalExtension, params};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("journal database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("corrupt state in journal: {0}")]
    State(#[from] serde_json::Error),
    #[error("journal lock poisoned")]
    Poisoned,
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// What the server told us about the resource on the first response.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceMeta {
    pub final_url: String,
    pub filename: String,
    pub total: Option<u64>,
    /// Raw `ETag` header (weak tags keep their `W/` prefix).
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub id: DownloadId,
    pub url: String,
    pub dir: String,
    pub meta: Option<ResourceMeta>,
    pub downloaded: u64,
    pub state: DownloadState,
    /// Destination chosen right before the atomic rename (crash recovery).
    pub final_path: Option<String>,
}

/// A byte range `[start, end)` of the file and how much of it is durable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentRecord {
    pub start: u64,
    pub end: u64,
    /// Bytes from `start` that are on disk and fsynced.
    pub done: u64,
}

/// A paired device (phone). Only the token's hash is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub id: u64,
    pub name: String,
    pub created_at: u64,
    pub last_seen: Option<u64>,
    pub revoked: bool,
}

pub struct Journal {
    conn: Connection,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS downloads (
    id            INTEGER PRIMARY KEY,
    url           TEXT NOT NULL,
    dir           TEXT NOT NULL,
    final_url     TEXT,
    filename      TEXT,
    total         INTEGER,
    etag          TEXT,
    last_modified TEXT,
    downloaded    INTEGER NOT NULL DEFAULT 0,
    state         TEXT NOT NULL,
    final_path    TEXT,
    UNIQUE (url, dir)
);
CREATE TABLE IF NOT EXISTS segments (
    download_id INTEGER NOT NULL,
    idx         INTEGER NOT NULL,
    start       INTEGER NOT NULL,
    end         INTEGER NOT NULL,
    done        INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (download_id, idx)
);
CREATE TABLE IF NOT EXISTS pairing_codes (
    code_hash  TEXT PRIMARY KEY,
    expires_at INTEGER NOT NULL,
    used       INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS devices (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL,
    last_seen  INTEGER,
    revoked    INTEGER NOT NULL DEFAULT 0
);";

impl Journal {
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(SCHEMA)?;
        Ok(Journal { conn })
    }

    /// Creates a fresh record, replacing any previous one for `(url, dir)`.
    pub fn create(&self, url: &str, dir: &str) -> Result<DownloadId> {
        let state = serde_json::to_string(&DownloadState::Probing)?;
        self.conn.execute(
            "DELETE FROM segments WHERE download_id IN
             (SELECT id FROM downloads WHERE url = ?1 AND dir = ?2)",
            params![url, dir],
        )?;
        self.conn.execute(
            "INSERT OR REPLACE INTO downloads (url, dir, state) VALUES (?1, ?2, ?3)",
            params![url, dir, state],
        )?;
        Ok(DownloadId(self.conn.last_insert_rowid() as u64))
    }

    pub fn find(&self, url: &str, dir: &str) -> Result<Option<Record>> {
        self.query_one("WHERE url = ?1 AND dir = ?2", params![url, dir])
    }

    pub fn get(&self, id: DownloadId) -> Result<Option<Record>> {
        self.query_one("WHERE id = ?1", params![id.0 as i64])
    }

    fn query_one(&self, filter: &str, p: impl rusqlite::Params) -> Result<Option<Record>> {
        let sql = format!(
            "SELECT id, url, dir, final_url, filename, total, etag, last_modified, downloaded, state, final_path
             FROM downloads {filter}"
        );
        let row = self
            .conn
            .query_row(&sql, p, |r| {
                let final_url: Option<String> = r.get(3)?;
                let filename: Option<String> = r.get(4)?;
                let meta = match (final_url, filename) {
                    (Some(final_url), Some(filename)) => Some(ResourceMeta {
                        final_url,
                        filename,
                        total: r.get::<_, Option<i64>>(5)?.map(|t| t as u64),
                        etag: r.get(6)?,
                        last_modified: r.get(7)?,
                    }),
                    _ => None,
                };
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    meta,
                    r.get::<_, i64>(8)?,
                    r.get::<_, String>(9)?,
                    r.get::<_, Option<String>>(10)?,
                ))
            })
            .optional()?;
        row.map(|(id, url, dir, meta, downloaded, state, final_path)| {
            Ok(Record {
                id: DownloadId(id as u64),
                url,
                dir,
                meta,
                downloaded: downloaded as u64,
                state: serde_json::from_str(&state)?,
                final_path,
            })
        })
        .transpose()
    }

    pub fn set_meta(&self, id: DownloadId, m: &ResourceMeta) -> Result<()> {
        self.conn.execute(
            "UPDATE downloads SET final_url = ?2, filename = ?3, total = ?4, etag = ?5,
             last_modified = ?6 WHERE id = ?1",
            params![
                id.0 as i64,
                m.final_url,
                m.filename,
                m.total.map(|t| t as i64),
                m.etag,
                m.last_modified
            ],
        )?;
        Ok(())
    }

    /// Records durable progress. Call only after `fdatasync` of the data.
    pub fn set_downloaded(&self, id: DownloadId, downloaded: u64) -> Result<()> {
        self.conn.execute(
            "UPDATE downloads SET downloaded = ?2 WHERE id = ?1",
            params![id.0 as i64, downloaded as i64],
        )?;
        Ok(())
    }

    /// Records the destination before renaming `.part` onto it.
    pub fn set_final_path(&self, id: DownloadId, path: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE downloads SET final_path = ?2 WHERE id = ?1",
            params![id.0 as i64, path],
        )?;
        Ok(())
    }

    /// Replaces the segment plan of a download.
    pub fn set_segments(&mut self, id: DownloadId, segs: &[SegmentRecord]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM segments WHERE download_id = ?1",
            params![id.0 as i64],
        )?;
        for (i, s) in segs.iter().enumerate() {
            tx.execute(
                "INSERT INTO segments (download_id, idx, start, end, done) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id.0 as i64, i as i64, s.start as i64, s.end as i64, s.done as i64],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn segments(&self, id: DownloadId) -> Result<Vec<SegmentRecord>> {
        let mut st = self
            .conn
            .prepare("SELECT start, end, done FROM segments WHERE download_id = ?1 ORDER BY idx")?;
        let rows = st.query_map(params![id.0 as i64], |r| {
            Ok(SegmentRecord {
                start: r.get::<_, i64>(0)? as u64,
                end: r.get::<_, i64>(1)? as u64,
                done: r.get::<_, i64>(2)? as u64,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Replaces the whole segment map (ranges change with work stealing) and
    /// the total in one transaction. Call only after `fdatasync` of the data.
    pub fn checkpoint_map(&mut self, id: DownloadId, segs: &[SegmentRecord]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM segments WHERE download_id = ?1",
            params![id.0 as i64],
        )?;
        for (i, s) in segs.iter().enumerate() {
            tx.execute(
                "INSERT INTO segments (download_id, idx, start, end, done) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id.0 as i64, i as i64, s.start as i64, s.end as i64, s.done as i64],
            )?;
        }
        let total: u64 = segs.iter().map(|s| s.done).sum();
        tx.execute(
            "UPDATE downloads SET downloaded = ?2 WHERE id = ?1",
            params![id.0 as i64, total as i64],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Records durable progress of every segment (and the total) atomically.
    /// Call only after `fdatasync` of the data.
    pub fn checkpoint(&mut self, id: DownloadId, done: &[u64]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for (i, d) in done.iter().enumerate() {
            tx.execute(
                "UPDATE segments SET done = ?3 WHERE download_id = ?1 AND idx = ?2",
                params![id.0 as i64, i as i64, *d as i64],
            )?;
        }
        let total: u64 = done.iter().sum();
        tx.execute(
            "UPDATE downloads SET downloaded = ?2 WHERE id = ?1",
            params![id.0 as i64, total as i64],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn set_state(&self, id: DownloadId, state: &DownloadState) -> Result<()> {
        self.conn.execute(
            "UPDATE downloads SET state = ?2 WHERE id = ?1",
            params![id.0 as i64, serde_json::to_string(state)?],
        )?;
        Ok(())
    }

    /// Stores a pairing code (by hash) valid until `expires_at` (unix secs).
    pub fn add_pairing_code(&self, code_hash: &str, expires_at: u64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO pairing_codes (code_hash, expires_at) VALUES (?1, ?2)",
            params![code_hash, expires_at as i64],
        )?;
        Ok(())
    }

    /// Marks a code as used if it exists, is unused and not expired.
    /// Returns whether it was accepted (single use: a second call fails).
    pub fn consume_pairing_code(&self, code_hash: &str, now: u64) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE pairing_codes SET used = 1
             WHERE code_hash = ?1 AND used = 0 AND expires_at >= ?2",
            params![code_hash, now as i64],
        )?;
        Ok(changed == 1)
    }

    pub fn add_device(&self, name: &str, token_hash: &str, now: u64) -> Result<u64> {
        self.conn.execute(
            "INSERT INTO devices (name, token_hash, created_at) VALUES (?1, ?2, ?3)",
            params![name, token_hash, now as i64],
        )?;
        Ok(self.conn.last_insert_rowid() as u64)
    }

    /// Finds a non-revoked device by token hash and records the access.
    pub fn touch_device(&self, token_hash: &str, now: u64) -> Result<Option<u64>> {
        let id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM devices WHERE token_hash = ?1 AND revoked = 0",
                params![token_hash],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = id {
            self.conn.execute(
                "UPDATE devices SET last_seen = ?2 WHERE id = ?1",
                params![id, now as i64],
            )?;
        }
        Ok(id.map(|i| i as u64))
    }

    pub fn devices(&self) -> Result<Vec<Device>> {
        let mut st = self
            .conn
            .prepare("SELECT id, name, created_at, last_seen, revoked FROM devices ORDER BY id")?;
        let rows = st.query_map([], |r| {
            Ok(Device {
                id: r.get::<_, i64>(0)? as u64,
                name: r.get(1)?,
                created_at: r.get::<_, i64>(2)? as u64,
                last_seen: r.get::<_, Option<i64>>(3)?.map(|v| v as u64),
                revoked: r.get::<_, i64>(4)? != 0,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Revokes a device; returns false if it did not exist.
    pub fn revoke_device(&self, id: u64) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE devices SET revoked = 1 WHERE id = ?1",
            params![id as i64],
        )? == 1)
    }
}
