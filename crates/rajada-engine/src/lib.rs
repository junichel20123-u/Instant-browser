//! Rajada download engine.
//!
//! Phase 1.1: single connection, `.part` file, persistent journal, resume
//! with `If-Range`, retries with backoff and `Retry-After`, and final
//! verification before an atomic rename.

mod download;
pub mod headers;
mod verify;

pub use download::{Config, Downloader, Outcome, Progress, Request, RetryPolicy};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("journal: {0}")]
    Store(#[from] rajada_store::StoreError),
    #[error("http client setup: {0}")]
    Client(#[from] reqwest::Error),
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("background task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
}
