use serde::{Deserialize, Serialize};

use crate::{ByteCount, BytesPerSec, RecoveryAction};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Severity {
    Info,
    Warning,
    Problem,
}

/// Structured doctor findings (spec §7.15). Clients translate them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", content = "params")]
pub enum DiagnosticCode {
    ServerPerConnectionLimit {
        per_conn: BytesPerSec,
        conns: u32,
        total: BytesPerSec,
    },
    LinkSaturated {
        link_est: BytesPerSec,
    },
    ServerAskedToWait {
        secs: u64,
        reduced_to: u32,
    },
    HtmlInsteadOfFile {
        size: ByteCount,
    },
    RangeNotSupported,
    BdpExceedsRmem {
        bdp: ByteCount,
        rmem_max: ByteCount,
    },
    MirrorQuarantined {
        url: String,
        why: String,
    },
    StalledConnectionReassigned,
}

impl DiagnosticCode {
    pub fn severity(&self) -> Severity {
        match self {
            DiagnosticCode::ServerPerConnectionLimit { .. }
            | DiagnosticCode::LinkSaturated { .. }
            | DiagnosticCode::StalledConnectionReassigned => Severity::Info,
            DiagnosticCode::ServerAskedToWait { .. }
            | DiagnosticCode::RangeNotSupported
            | DiagnosticCode::BdpExceedsRmem { .. }
            | DiagnosticCode::MirrorQuarantined { .. } => Severity::Warning,
            DiagnosticCode::HtmlInsteadOfFile { .. } => Severity::Problem,
        }
    }

    /// At most one recommendation per diagnostic.
    pub fn recommendation(&self) -> Option<RecoveryAction> {
        match self {
            DiagnosticCode::HtmlInsteadOfFile { .. } => Some(RecoveryAction::ResendFromBrowser),
            _ => None,
        }
    }
}

/// A diagnostic ready to be sent to clients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    #[serde(flatten)]
    pub code: DiagnosticCode,
    pub severity: Severity,
    pub recommendation: Option<RecoveryAction>,
}

impl From<DiagnosticCode> for Diagnostic {
    fn from(code: DiagnosticCode) -> Self {
        Diagnostic {
            severity: code.severity(),
            recommendation: code.recommendation(),
            code,
        }
    }
}
