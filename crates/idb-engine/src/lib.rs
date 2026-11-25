//! IDB download engine.
//!
//! Probing, adaptive segmented downloads with work stealing, resume from a
//! persistent journal, verification before an atomic rename, and a doctor
//! that explains what limits the speed.

mod controller;
mod disk;
mod doctor;
mod download;
pub mod headers;
pub mod manager;
pub mod pairing;
pub mod ratelimit;
mod segmented;
pub mod segments;
mod verify;
mod workmap;

pub use controller::Mode as Connections;
pub use doctor::DoctorReport;
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
