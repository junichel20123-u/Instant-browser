//! Types shared by every Rajada component: download states, reasons,
//! verification levels, diagnostics and the event schema of the API.
//!
//! Clients never compute anything important: the daemon sends these
//! structured values and clients only translate and display them.

mod diagnostic;
mod event;
mod ids;
mod state;

pub use diagnostic::{Diagnostic, DiagnosticCode, Severity};
pub use event::{Event, EventLevel, RowUpdate, Summary};
pub use ids::{ByteCount, ByteOffset, BytesPerSec, DownloadId};
pub use state::{
    DownloadState, FailReason, HashAlgo, HashSource, NeedsUserReason, RecoveryAction, StateKind,
    VerificationLevel,
};

/// Version of the native JSON-RPC API. Bumped on breaking changes.
pub const API_VERSION: u32 = 1;
