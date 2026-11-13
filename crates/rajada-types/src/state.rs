use serde::{Deserialize, Serialize};

/// State of a download (spec §7.1). The same names are used by every client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "data")]
pub enum DownloadState {
    Queued,
    Probing,
    Downloading,
    Paused,
    /// The server asked us to wait; `until_unix_secs` drives a countdown.
    PoliteWait {
        until_unix_secs: u64,
    },
    NeedsUser(NeedsUserReason),
    /// Final verification; `permille` is 0..=1000 for determinate progress.
    Verifying {
        permille: u16,
    },
    Complete(VerificationLevel),
    Failed(FailReason),
}

/// Data-free discriminant of [`DownloadState`], used for labels and colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StateKind {
    Queued,
    Probing,
    Downloading,
    Paused,
    PoliteWait,
    NeedsUser,
    Verifying,
    Complete,
    Failed,
}

impl StateKind {
    pub const ALL: [StateKind; 9] = [
        StateKind::Queued,
        StateKind::Probing,
        StateKind::Downloading,
        StateKind::Paused,
        StateKind::PoliteWait,
        StateKind::NeedsUser,
        StateKind::Verifying,
        StateKind::Complete,
        StateKind::Failed,
    ];
}

impl DownloadState {
    pub fn kind(&self) -> StateKind {
        match self {
            DownloadState::Queued => StateKind::Queued,
            DownloadState::Probing => StateKind::Probing,
            DownloadState::Downloading => StateKind::Downloading,
            DownloadState::Paused => StateKind::Paused,
            DownloadState::PoliteWait { .. } => StateKind::PoliteWait,
            DownloadState::NeedsUser(_) => StateKind::NeedsUser,
            DownloadState::Verifying { .. } => StateKind::Verifying,
            DownloadState::Complete(_) => StateKind::Complete,
            DownloadState::Failed(_) => StateKind::Failed,
        }
    }

    /// The single recovery action offered to the user, if any.
    pub fn recovery(&self) -> Option<RecoveryAction> {
        match self {
            DownloadState::NeedsUser(r) => Some(r.recovery()),
            DownloadState::Failed(r) => Some(r.recovery()),
            _ => None,
        }
    }

    /// Terminal states never change without a user action.
    pub fn is_terminal(&self) -> bool {
        matches!(self, DownloadState::Complete(_) | DownloadState::Failed(_))
    }
}

/// Why a download is waiting for the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NeedsUserReason {
    UrlExpired,
    LoginRequired,
    HtmlInsteadOfFile,
    ResourceChanged,
    DiskFull,
    PermissionDenied,
}

impl NeedsUserReason {
    pub const ALL: [NeedsUserReason; 6] = [
        NeedsUserReason::UrlExpired,
        NeedsUserReason::LoginRequired,
        NeedsUserReason::HtmlInsteadOfFile,
        NeedsUserReason::ResourceChanged,
        NeedsUserReason::DiskFull,
        NeedsUserReason::PermissionDenied,
    ];

    pub fn recovery(self) -> RecoveryAction {
        match self {
            NeedsUserReason::UrlExpired => RecoveryAction::RenewLink,
            NeedsUserReason::LoginRequired | NeedsUserReason::HtmlInsteadOfFile => {
                RecoveryAction::ResendFromBrowser
            }
            NeedsUserReason::ResourceChanged => RecoveryAction::RestartFromScratch,
            NeedsUserReason::DiskFull => RecoveryAction::FreeDiskSpace,
            NeedsUserReason::PermissionDenied => RecoveryAction::ChooseAnotherFolder,
        }
    }
}

/// Definitive failures. Every one has a suggested recovery action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FailReason {
    VerificationFailed,
    NotFound,
    ServerError,
    TooManyRedirects,
    NetworkUnreachable,
    InvalidUrl,
    Cancelled,
}

impl FailReason {
    pub const ALL: [FailReason; 7] = [
        FailReason::VerificationFailed,
        FailReason::NotFound,
        FailReason::ServerError,
        FailReason::TooManyRedirects,
        FailReason::NetworkUnreachable,
        FailReason::InvalidUrl,
        FailReason::Cancelled,
    ];

    pub fn recovery(self) -> RecoveryAction {
        match self {
            FailReason::VerificationFailed => RecoveryAction::RedownloadCorruptPart,
            FailReason::NotFound | FailReason::InvalidUrl => RecoveryAction::ChangeSource,
            FailReason::ServerError
            | FailReason::TooManyRedirects
            | FailReason::NetworkUnreachable
            | FailReason::Cancelled => RecoveryAction::Retry,
        }
    }
}

/// A concrete next step: a button in the GUI, a command in the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RecoveryAction {
    Retry,
    RenewLink,
    ResendFromBrowser,
    RestartFromScratch,
    FreeDiskSpace,
    ChooseAnotherFolder,
    ChangeSource,
    RedownloadCorruptPart,
}

/// How strongly a completed file was verified (spec §7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "level")]
pub enum VerificationLevel {
    HashMatched { algo: HashAlgo, source: HashSource },
    SizeAndValidator,
    SizeOnly,
    Unverified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HashAlgo {
    Sha256,
    Sha512,
    Blake3,
    Sha1,
    Md5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HashSource {
    /// `Repr-Digest`/`Content-Digest` (RFC 9530).
    Server,
    Metalink,
    User,
}
