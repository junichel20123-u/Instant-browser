//! Parallel download of byte ranges: one short-lived task per lease.
//!
//! A worker takes one lease from the [`WorkMap`](crate::workmap::WorkMap),
//! makes one request for it and streams the body until the lease ends (the
//! end may shrink while it runs: work stealing or parking). Retries, pauses
//! for `Retry-After`, the stall watchdog and checkpoints all belong to the
//! coordinator in `download.rs`, which is the only owner of the map.

use std::fs::File;
use std::os::unix::fs::FileExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bytes::{Bytes, BytesMut};
use idb_types::{DownloadState, FailReason, NeedsUserReason};
use reqwest::{StatusCode, header};
use tokio::sync::mpsc;

use crate::download::io_state;
use crate::headers;
use crate::workmap::Lease;

/// Write buffer per connection (spec: 256 KiB to 1 MiB).
pub(crate) const WRITE_BUF: usize = 256 * 1024;

/// Everything a worker needs; shared read-only between workers.
pub(crate) struct Shared {
    pub client: reqwest::Client,
    pub url: String,
    /// Validator sent in `If-Range` (strong ETag or Last-Modified).
    pub if_range: Option<String>,
    /// Strong ETag recorded at probe time, to detect changes on 206.
    pub strong_etag: Option<String>,
    pub total: u64,
    pub file: Arc<File>,
    /// Bytes received from sockets (speed is measured here, spec §7.14).
    pub received: AtomicU64,
    pub events: mpsc::Sender<Event>,
}

/// Notifications from workers to the coordinator.
pub(crate) enum Event {
    /// Time to first byte of a request (RTT proxy for the controller).
    Rtt(Duration),
}

/// How a worker ended.
pub(crate) enum End {
    /// The lease is complete (or was cut to what it already had).
    Done,
    /// A transient problem; the coordinator decides when to retry.
    Retry {
        delay: Option<Duration>,
        /// 429/503: the server asked us to slow down.
        throttled: bool,
    },
    Final(DownloadState),
}

/// Requests `[pos, end)` and validates that the server answered exactly that.
async fn request(sh: &Shared, pos: u64, end: u64) -> Result<reqwest::Response, End> {
    let mut rb = sh
        .client
        .get(&sh.url)
        .header(header::ACCEPT_ENCODING, "identity")
        .header(header::RANGE, format!("bytes={}-{}", pos, end - 1));
    if let Some(v) = &sh.if_range {
        rb = rb.header(header::IF_RANGE, v);
    }
    let sent = Instant::now();
    let resp = match rb.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(pos, "segment request failed: {e}");
            return Err(End::Retry {
                delay: None,
                throttled: false,
            });
        }
    };
    let _ = sh.events.try_send(Event::Rtt(sent.elapsed()));
    let h = resp.headers();
    let get = |n: header::HeaderName| h.get(n).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let delay = get(header::RETRY_AFTER)
        .as_deref()
        .and_then(headers::retry_after);
    let changed = End::Final(DownloadState::NeedsUser(NeedsUserReason::ResourceChanged));
    match resp.status() {
        StatusCode::PARTIAL_CONTENT => {}
        // If-Range failed: the resource changed. Never mix versions.
        StatusCode::OK | StatusCode::RANGE_NOT_SATISFIABLE => return Err(changed),
        StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE => {
            return Err(End::Retry {
                delay,
                throttled: true,
            });
        }
        StatusCode::NOT_FOUND | StatusCode::GONE => {
            return Err(End::Final(DownloadState::Failed(FailReason::NotFound)));
        }
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            return Err(End::Final(DownloadState::NeedsUser(
                NeedsUserReason::LoginRequired,
            )));
        }
        s if s.is_server_error() || s == StatusCode::REQUEST_TIMEOUT => {
            return Err(End::Retry {
                delay,
                throttled: false,
            });
        }
        _ => return Err(End::Final(DownloadState::Failed(FailReason::ServerError))),
    }
    if let (Some(old), Some(new)) = (&sh.strong_etag, get(header::ETAG))
        && *old != new
    {
        return Err(changed);
    }
    match get(header::CONTENT_RANGE)
        .as_deref()
        .and_then(headers::content_range)
    {
        Some((a, _, total)) if a == pos && total.is_none_or(|t| t == sh.total) => Ok(resp),
        Some((_, _, Some(t))) if t != sh.total => Err(changed),
        _ => Err(End::Final(DownloadState::Failed(FailReason::ServerError))),
    }
}

/// Positional write off the async runtime (`pwrite`, no global lock).
async fn write_at(file: &Arc<File>, data: Bytes, at: u64) -> std::io::Result<()> {
    let file = file.clone();
    tokio::task::spawn_blocking(move || file.write_all_at(&data, at))
        .await
        .map_err(std::io::Error::other)?
}

async fn flush(
    sh: &Shared,
    lease: &Lease,
    buf: &mut BytesMut,
    end_pos: u64,
) -> std::io::Result<()> {
    if buf.is_empty() {
        return Ok(());
    }
    let data = buf.split().freeze();
    let len = data.len() as u64;
    write_at(&sh.file, data, end_pos - len).await?;
    lease.wrote(len);
    Ok(())
}

/// Streams `resp` (whose body starts at `pos`) into the lease.
async fn stream(
    sh: &Shared,
    lease: &Lease,
    mut resp: reqwest::Response,
    mut pos: u64,
) -> std::io::Result<()> {
    let mut buf = BytesMut::with_capacity(WRITE_BUF);
    loop {
        let chunk = match resp.chunk().await {
            Ok(Some(c)) => c,
            Ok(None) => break,
            Err(e) => {
                tracing::debug!(pos, "segment body interrupted: {e}");
                break;
            }
        };
        sh.received.fetch_add(chunk.len() as u64, Ordering::Relaxed);
        // The lease may have been cut (stolen or parked) meanwhile.
        let ok = lease.claim(pos, chunk.len() as u64) as usize;
        buf.extend_from_slice(&chunk[..ok]);
        pos += ok as u64;
        if buf.len() >= WRITE_BUF || ok < chunk.len() {
            flush(sh, lease, &mut buf, pos).await?;
        }
        if ok < chunk.len() {
            // Reached the (possibly new) end: drop the connection.
            return Ok(());
        }
    }
    flush(sh, lease, &mut buf, pos).await
}

/// Downloads what is left of one lease with a single request. `first` is
/// the probe response when it starts exactly where the lease does.
pub(crate) async fn worker(
    sh: Arc<Shared>,
    lease: Arc<Lease>,
    first: Option<reqwest::Response>,
) -> End {
    let pos = lease.write_pos();
    if lease.is_finished() {
        return End::Done;
    }
    let resp = match first {
        Some(r) => r,
        None => match request(&sh, pos, lease.end()).await {
            Ok(r) => r,
            Err(end) => return end,
        },
    };
    if let Err(e) = stream(&sh, &lease, resp, pos).await {
        return End::Final(io_state(&e));
    }
    if lease.is_finished() {
        End::Done
    } else {
        End::Retry {
            delay: None,
            throttled: false,
        }
    }
}
