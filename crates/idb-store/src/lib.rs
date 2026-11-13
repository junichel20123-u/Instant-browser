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

    pub fn set_state(&self, id: DownloadId, state: &DownloadState) -> Result<()> {
        self.conn.execute(
            "UPDATE downloads SET state = ?2 WHERE id = ?1",
            params![id.0 as i64, serde_json::to_string(state)?],
        )?;
        Ok(())
    }
}
