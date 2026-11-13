use serde::{Deserialize, Serialize};

use crate::{ByteCount, BytesPerSec, Diagnostic, DownloadId, DownloadState};

/// Subscription levels (spec §8.2). The daemon aggregates and rate-limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "level", content = "id")]
pub enum EventLevel {
    /// Totals for tray and mini-mode, 1 Hz.
    Summary,
    /// Changed list rows only, 4 Hz.
    Rows,
    /// One download in detail, 4 Hz.
    Detail(DownloadId),
}

impl EventLevel {
    pub fn max_hz(self) -> u32 {
        match self {
            EventLevel::Summary => 1,
            EventLevel::Rows | EventLevel::Detail(_) => 4,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub active: u32,
    pub queued: u32,
    pub needs_user: u32,
    pub speed: BytesPerSec,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowUpdate {
    pub id: DownloadId,
    pub done: ByteCount,
    pub total: Option<ByteCount>,
    pub speed: BytesPerSec,
    pub eta_secs: Option<u64>,
}

/// Events pushed from the daemon to subscribed clients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", content = "data")]
pub enum Event {
    /// Sent immediately, never waits for the next tick.
    StateChanged {
        id: DownloadId,
        state: DownloadState,
    },
    Summary(Summary),
    Rows(Vec<RowUpdate>),
    Diagnostic {
        id: DownloadId,
        diagnostic: Diagnostic,
    },
}
