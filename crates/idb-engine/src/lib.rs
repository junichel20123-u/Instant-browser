//! IDB download engine.
//!
//! Phase 1.1: single connection, `.part` file, persistent journal, resume
//! with `If-Range`, retries with backoff and `Retry-After`, and final
//! verification before an atomic rename.

mod controller;
mod disk;
mod download;
pub mod headers;
mod segmented;
pub mod segments;
mod verify;
mod workmap;

pub use controller::Mode as Connections;
pub use download::{
    Config, Downloader, MAX_CONNECTIONS, Outcome, Progress, Request, RetryPolicy, Stats,
};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("journal: {0}")]
    Store(#[from] idb_store::StoreError),
    #[error("http client setup: {0}")]
    Client(#[from] reqwest::Error),
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("background task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
}
