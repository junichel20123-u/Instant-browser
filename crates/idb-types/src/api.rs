//! Native API messages (JSON-RPC 2.0), version [`API_VERSION`](crate::API_VERSION).
//!
//! One protocol for every transport: the local daemon socket (Unix, phase 2),
//! a paired phone over the LAN (WebSocket + TLS, phase 2) and direct calls
//! when the engine is embedded in the mobile app. Transports only move these
//! messages; they never invent their own.
//!
//! Wire shape: `{"jsonrpc":"2.0","id":7,"method":"download.add","params":{...}}`.

use serde::{Deserialize, Serialize};

use crate::{ByteCount, BytesPerSec, DownloadId, DownloadState, Event, EventLevel, StateKind};

pub const JSONRPC: &str = "2.0";

/// Request id chosen by the client (number or string, per JSON-RPC).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Num(u64),
    Str(String),
}

/// A call from a client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    /// `None` for notifications (no response expected).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<RequestId>,
    #[serde(flatten)]
    pub call: Call,
}

impl Request {
    pub fn new(id: u64, call: Call) -> Request {
        Request {
            jsonrpc: JSONRPC.to_owned(),
            id: Some(RequestId::Num(id)),
            call,
        }
    }
}

/// Connection policy requested by a client (mirrors `idb get -c`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionsParam {
    Auto,
    Fixed(u32),
}

/// Methods of the native API (spec §8.2) and their parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum Call {
    /// First call of every session: version negotiation.
    #[serde(rename = "session.hello")]
    Hello { api_version: u32, client: String },
    #[serde(rename = "download.add")]
    Add(AddParams),
    #[serde(rename = "download.pause")]
    Pause { id: DownloadId },
    #[serde(rename = "download.resume")]
    Resume { id: DownloadId },
    #[serde(rename = "download.remove")]
    Remove {
        id: DownloadId,
        #[serde(default)]
        delete_file: bool,
    },
    #[serde(rename = "download.list")]
    List {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<StateKind>,
    },
    #[serde(rename = "download.get")]
    Get { id: DownloadId },
    #[serde(rename = "download.changeSource")]
    ChangeSource { id: DownloadId, url: String },
    #[serde(rename = "doctor.diagnose")]
    Diagnose { url: String },
    #[serde(rename = "events.subscribe")]
    Subscribe { level: EventLevel },
    #[serde(rename = "events.unsubscribe")]
    Unsubscribe { level: EventLevel },
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AddParams {
    pub url: String,
    /// Destination directory; must be inside the daemon's `allowed_dirs`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_sha256: Option<String>,
    /// Speed limit in bytes/s.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connections: Option<ConnectionsParam>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referer: Option<String>,
}

/// One row of the download list: everything a list UI (desktop or phone)
/// needs, already computed by the engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadSummary {
    pub id: DownloadId,
    pub url: String,
    pub filename: Option<String>,
    pub state: DownloadState,
    pub done: ByteCount,
    pub total: Option<ByteCount>,
    pub speed: BytesPerSec,
    pub eta_secs: Option<u64>,
}

/// Result payloads, one per method family.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CallResult {
    Hello { api_version: u32, server: String },
    Added { id: DownloadId },
    List { downloads: Vec<DownloadSummary> },
    Download(Box<DownloadSummary>),
    Ok {},
}

/// Error categories with stable numeric codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorKind {
    ParseError,
    InvalidRequest,
    MethodNotFound,
    InvalidParams,
    Internal,
    /// Missing, wrong or revoked token (TCP/LAN transports).
    Unauthorized,
    /// The client speaks a different `api_version`.
    UnsupportedVersion,
    NotFound,
    /// Destination outside `allowed_dirs` (spec §8.3).
    DirNotAllowed,
}

impl ErrorKind {
    /// JSON-RPC 2.0 reserved codes, then application codes from -32000 down.
    pub fn code(self) -> i32 {
        match self {
            ErrorKind::ParseError => -32700,
            ErrorKind::InvalidRequest => -32600,
            ErrorKind::MethodNotFound => -32601,
            ErrorKind::InvalidParams => -32602,
            ErrorKind::Internal => -32603,
            ErrorKind::Unauthorized => -32001,
            ErrorKind::UnsupportedVersion => -32002,
            ErrorKind::NotFound => -32003,
            ErrorKind::DirNotAllowed => -32004,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    /// Developer-facing English text; user-facing text comes from `idb-i18n`.
    pub message: String,
    pub data: ErrorData,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorData {
    pub kind: ErrorKind,
}

impl RpcError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> RpcError {
        RpcError {
            code: kind.code(),
            message: message.into(),
            data: ErrorData { kind },
        }
    }
}

/// Reply to a [`Request`] with an id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: Option<RequestId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<CallResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    pub fn ok(id: Option<RequestId>, result: CallResult) -> Response {
        Response {
            jsonrpc: JSONRPC.to_owned(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: Option<RequestId>, error: RpcError) -> Response {
        Response {
            jsonrpc: JSONRPC.to_owned(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// Server push (`{"jsonrpc":"2.0","method":"event","params":{...}}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    pub params: Event,
}

impl Notification {
    pub fn event(params: Event) -> Notification {
        Notification {
            jsonrpc: JSONRPC.to_owned(),
            method: "event".to_owned(),
            params,
        }
    }
}
