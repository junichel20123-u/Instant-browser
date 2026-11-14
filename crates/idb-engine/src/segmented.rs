//! Parallel download of byte ranges: one task (actor) per segment.
//!
//! Each worker owns its segment and reports progress through an atomic
//! counter of bytes written contiguously from the segment start. The
//! coordinator is the only one that checkpoints: it snapshots the counters,
//! `fdatasync`s the file and then records the snapshot in the journal, so
//! the journal never claims bytes that are not durable.

use std::fs::File;
use std::os::unix::fs::FileExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use idb_types::{DownloadState, FailReason, NeedsUserReason};
use reqwest::{StatusCode, header};
use tokio::sync::mpsc;

use crate::download::{RetryPolicy, backoff, io_state};
use crate::headers;
use crate::segments::Segment;

/// Write buffer per segment (spec: 256 KiB to 1 MiB).
const WRITE_BUF: usize = 256 * 1024;

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
    pub segs: Vec<Segment>,
    /// Bytes written contiguously from each segment start.
    pub done: Vec<AtomicU64>,
    pub retry: RetryPolicy,
    pub events: mpsc::Sender<Event>,
}

impl Shared {
    pub fn snapshot(&self) -> Vec<u64> {
        self.done.iter().map(|d| d.load(Ordering::SeqCst)).collect()
    }

    pub fn written(&self) -> u64 {
        self.snapshot().iter().sum()
    }
}

/// Notifications from workers to the coordinator.
pub(crate) enum Event {
    PoliteWait { until_unix_secs: u64 },
}

/// How a worker ended.
pub(crate) enum End {
    Done,
    Final(DownloadState),
    GaveUp,
}

enum Step {
    Retry(Option<Duration>),
    Final(DownloadState),
}

fn unix_now_plus(d: Duration) -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |n| (n + d).as_secs())
}

/// Requests `[pos, end)` and validates that the server answered exactly that.
async fn request(sh: &Shared, pos: u64, end: u64) -> Result<reqwest::Response, Step> {
    let mut rb = sh
        .client
        .get(&sh.url)
        .header(header::ACCEPT_ENCODING, "identity")
        .header(header::RANGE, format!("bytes={}-{}", pos, end - 1));
    if let Some(v) = &sh.if_range {
        rb = rb.header(header::IF_RANGE, v);
    }
    let resp = match rb.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(pos, "segment request failed: {e}");
            return Err(Step::Retry(None));
        }
    };
    let h = resp.headers();
    let get = |n: header::HeaderName| h.get(n).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let delay = get(header::RETRY_AFTER)
        .as_deref()
        .and_then(headers::retry_after);
    let changed = DownloadState::NeedsUser(NeedsUserReason::ResourceChanged);
    match resp.status() {
        StatusCode::PARTIAL_CONTENT => {}
        // If-Range failed: the resource changed. Never mix versions.
        StatusCode::OK => return Err(Step::Final(changed)),
        StatusCode::RANGE_NOT_SATISFIABLE => return Err(Step::Final(changed)),
        StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE => {
            return Err(Step::Retry(delay));
        }
        StatusCode::NOT_FOUND | StatusCode::GONE => {
            return Err(Step::Final(DownloadState::Failed(FailReason::NotFound)));
        }
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            return Err(Step::Final(DownloadState::NeedsUser(
                NeedsUserReason::LoginRequired,
            )));
        }
        s if s.is_server_error() || s == StatusCode::REQUEST_TIMEOUT => {
            return Err(Step::Retry(delay));
        }
        _ => {
            return Err(Step::Final(DownloadState::Failed(FailReason::ServerError)));
        }
    }
    if let (Some(old), Some(new)) = (&sh.strong_etag, get(header::ETAG))
        && *old != new
    {
        return Err(Step::Final(changed));
    }
    match get(header::CONTENT_RANGE)
        .as_deref()
        .and_then(headers::content_range)
    {
        Some((a, _, total)) if a == pos && total.is_none_or(|t| t == sh.total) => Ok(resp),
        Some((_, _, Some(t))) if t != sh.total => Err(Step::Final(changed)),
        _ => Err(Step::Final(DownloadState::Failed(FailReason::ServerError))),
    }
}

/// Positional write off the async runtime (`pwrite`, no global lock).
async fn write_at(file: &Arc<File>, data: Bytes, at: u64) -> std::io::Result<()> {
    let file = file.clone();
    tokio::task::spawn_blocking(move || file.write_all_at(&data, at))
        .await
        .map_err(std::io::Error::other)?
}

/// Streams `resp` into the segment. Returns `Ok(true)` if the body ended
/// (or broke) normally, `Err` on local I/O errors.
async fn stream(
    sh: &Shared,
    idx: usize,
    mut resp: reqwest::Response,
    body_start: u64,
) -> std::io::Result<()> {
    let seg = sh.segs[idx];
    let mut pos = body_start;
    let mut buf = BytesMut::with_capacity(WRITE_BUF);
    let flush = |buf: &mut BytesMut, pos: u64| {
        let data = buf.split().freeze();
        let at = pos - data.len() as u64;
        (data, at)
    };
    loop {
        let chunk = match resp.chunk().await {
            Ok(Some(c)) => c,
            Ok(None) => break,
            Err(e) => {
                tracing::warn!(idx, pos, "segment body interrupted: {e}");
                break;
            }
        };
        // The first response of the probe may run past this segment.
        let want = (seg.end.saturating_sub(pos)).min(chunk.len() as u64) as usize;
        buf.extend_from_slice(&chunk[..want]);
        pos += want as u64;
        if buf.len() >= WRITE_BUF || pos >= seg.end {
            let len = buf.len() as u64;
            let (data, at) = flush(&mut buf, pos);
            write_at(&sh.file, data, at).await?;
            sh.done[idx].fetch_add(len, Ordering::SeqCst);
        }
        if pos >= seg.end {
            return Ok(());
        }
    }
    if !buf.is_empty() {
        let len = buf.len() as u64;
        let (data, at) = flush(&mut buf, pos);
        write_at(&sh.file, data, at).await?;
        sh.done[idx].fetch_add(len, Ordering::SeqCst);
    }
    Ok(())
}

/// Downloads one segment to completion, with its own retries.
/// `first` is the probe response when it already covers this segment's
/// current position (it becomes segment 0, nothing is wasted).
pub(crate) async fn worker(
    sh: Arc<Shared>,
    idx: usize,
    mut first: Option<(reqwest::Response, u64)>,
) -> End {
    let seg = sh.segs[idx];
    let mut failures = 0u32;
    let mut best = sh.done[idx].load(Ordering::SeqCst);
    loop {
        let pos = seg.start + sh.done[idx].load(Ordering::SeqCst);
        if pos >= seg.end {
            return End::Done;
        }
        let attempt = match first.take() {
            Some((resp, body_start)) if body_start == pos => Ok(resp),
            _ => request(&sh, pos, seg.end).await,
        };
        let server_delay = match attempt {
            Ok(resp) => {
                if let Err(e) = stream(&sh, idx, resp, pos).await {
                    return End::Final(io_state(&e));
                }
                None
            }
            Err(Step::Final(state)) => return End::Final(state),
            Err(Step::Retry(d)) => d,
        };
        let now = sh.done[idx].load(Ordering::SeqCst);
        if seg.start + now >= seg.end {
            return End::Done;
        }
        if now > best {
            best = now;
            failures = 0;
        }
        failures += 1;
        if failures > sh.retry.max_attempts {
            return End::GaveUp;
        }
        let delay = match server_delay {
            Some(d) => {
                let d = d.min(sh.retry.max_retry_after);
                let _ = sh
                    .events
                    .send(Event::PoliteWait {
                        until_unix_secs: unix_now_plus(d),
                    })
                    .await;
                d
            }
            None => backoff(&sh.retry, failures),
        };
        tokio::time::sleep(delay).await;
    }
}
