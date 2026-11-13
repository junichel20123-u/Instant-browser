//! `faultbox`: deterministic HTTP file server with fault injection.
//!
//! `GET /file?size=N&seed=S[&scenario...]` serves `N` deterministic bytes.
//! Scenarios (query string):
//! - `no_ranges`: ignore `Range`, always 200, no `Accept-Ranges`.
//! - `per_conn_rate=B`: limit each response to `B` bytes/s.
//! - `latency_ms=M`: delay each response by `M` ms.
//! - `drop_conn=N`: abort the connection after `N` body bytes.
//! - `429_above=N&retry_after=S`: answer 429 when more than `N` responses
//!   are in flight.
//! - `etag_change_at=P`: after P% of the file was served (all responses
//!   together), the file changes: new content, new ETag.

pub mod body;
mod range;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use bytes::Bytes;

pub use body::expected_sha256;

const LAST_MODIFIED: &str = "Wed, 01 Jan 2025 00:00:00 GMT";
const CHUNK: u64 = 64 * 1024;

#[derive(Default)]
struct Shared {
    in_flight: AtomicUsize,
    served: AtomicU64,
    version: AtomicU64,
}

/// Decrements the in-flight counter when the response body is dropped.
struct InFlight(Arc<Shared>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, Default)]
struct Scenario {
    size: u64,
    seed: u64,
    no_ranges: bool,
    per_conn_rate: Option<u64>,
    latency_ms: Option<u64>,
    drop_conn: Option<u64>,
    limit_429_above: Option<usize>,
    retry_after: u64,
    etag_change_at: Option<u64>,
}

impl Scenario {
    fn from_query(q: &HashMap<String, String>) -> Result<Self, String> {
        let num = |k: &str| -> Result<Option<u64>, String> {
            q.get(k)
                .map(|v| v.parse::<u64>().map_err(|_| format!("invalid {k}")))
                .transpose()
        };
        Ok(Scenario {
            size: num("size")?.unwrap_or(1 << 20),
            seed: num("seed")?.unwrap_or(0),
            no_ranges: q.contains_key("no_ranges"),
            per_conn_rate: num("per_conn_rate")?.filter(|r| *r > 0),
            latency_ms: num("latency_ms")?,
            drop_conn: num("drop_conn")?,
            limit_429_above: num("429_above")?.map(|n| n as usize),
            retry_after: num("retry_after")?.unwrap_or(1),
            etag_change_at: num("etag_change_at")?,
        })
    }
}

/// Seed actually used for content and ETag, given the file version.
pub fn effective_seed(seed: u64, version: u64) -> u64 {
    seed ^ version.wrapping_mul(0x5851_F42D_4C95_7F2D)
}

/// Strong ETag of the file for a given seed, size and version.
pub fn etag(seed: u64, size: u64, version: u64) -> String {
    format!("\"{seed:x}-{size:x}-v{version}\"")
}

pub fn router() -> Router {
    Router::new()
        .route("/file", get(serve_file))
        .with_state(Arc::new(Shared::default()))
}

/// Binds to `127.0.0.1:0` and serves in the background. Returns the address.
pub async fn spawn() -> std::io::Result<SocketAddr> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router()).await {
            tracing::error!("faultbox stopped: {e}");
        }
    });
    Ok(addr)
}

fn plain(status: StatusCode, msg: &str) -> Response {
    (status, msg.to_owned()).into_response()
}

async fn serve_file(
    State(shared): State<Arc<Shared>>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let sc = match Scenario::from_query(&q) {
        Ok(sc) => sc,
        Err(e) => return plain(StatusCode::BAD_REQUEST, &e),
    };
    if let Some(ms) = sc.latency_ms {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }

    let guard = InFlight(shared.clone());
    let now = shared.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
    if sc.limit_429_above.is_some_and(|limit| now > limit) {
        let mut r = plain(StatusCode::TOO_MANY_REQUESTS, "slow down");
        r.headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from(sc.retry_after));
        return r;
    }

    let version = shared.version.load(Ordering::SeqCst);
    let tag = etag(sc.seed, sc.size, version);
    let seed = effective_seed(sc.seed, version);

    let if_range_ok = headers
        .get(header::IF_RANGE)
        .and_then(|v| v.to_str().ok())
        .is_none_or(|v| v == tag || v == LAST_MODIFIED);
    let requested = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .filter(|_| !sc.no_ranges && if_range_ok);

    let (status, start, end) = match requested.map(|h| range::parse(h, sc.size)) {
        Some(Err(())) => {
            let mut r = plain(StatusCode::RANGE_NOT_SATISFIABLE, "unsatisfiable");
            if let Ok(v) = HeaderValue::from_str(&format!("bytes */{}", sc.size)) {
                r.headers_mut().insert(header::CONTENT_RANGE, v);
            }
            return r;
        }
        Some(Ok(Some((s, e)))) => (StatusCode::PARTIAL_CONTENT, s, e),
        Some(Ok(None)) | None => (StatusCode::OK, 0, sc.size),
    };

    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, end - start)
        .header(header::ETAG, &tag)
        .header(header::LAST_MODIFIED, LAST_MODIFIED);
    if !sc.no_ranges {
        builder = builder.header(header::ACCEPT_RANGES, "bytes");
    }
    if status == StatusCode::PARTIAL_CONTENT {
        builder = builder.header(
            header::CONTENT_RANGE,
            format!("bytes {}-{}/{}", start, end - 1, sc.size),
        );
    }

    let stream = body_stream(shared, sc, seed, start, end, guard);
    builder
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "build error"))
}

fn body_stream(
    shared: Arc<Shared>,
    sc: Scenario,
    seed: u64,
    start: u64,
    end: u64,
    guard: InFlight,
) -> impl futures_util::Stream<Item = std::io::Result<Bytes>> {
    // With a rate limit, send ~10 chunks per second for smooth pacing.
    let chunk = sc.per_conn_rate.map_or(CHUNK, |r| (r / 10).clamp(1, CHUNK));
    let state = (start, 0u64, guard);
    futures_util::stream::unfold(state, move |(pos, sent, guard)| {
        let shared = shared.clone();
        let sc = sc.clone();
        async move {
            if pos >= end {
                return None;
            }
            if sc.drop_conn.is_some_and(|limit| sent >= limit) {
                // Let hyper flush what was already written before aborting.
                tokio::time::sleep(Duration::from_millis(50)).await;
                let err = std::io::Error::new(std::io::ErrorKind::ConnectionReset, "drop_conn");
                return Some((Err(err), (end, sent, guard)));
            }
            let mut n = chunk.min(end - pos);
            if let Some(limit) = sc.drop_conn {
                n = n.min(limit - sent).max(1);
            }
            if let Some(rate) = sc.per_conn_rate {
                tokio::time::sleep(Duration::from_secs_f64(n as f64 / rate as f64)).await;
            }
            let data = body::range(seed, pos, pos + n);
            let total = shared.served.fetch_add(n, Ordering::SeqCst) + n;
            if let Some(pct) = sc.etag_change_at
                && sc.size > 0
                && total * 100 >= sc.size * pct
            {
                // Bump once: only the first crossing changes the version.
                let _ = shared
                    .version
                    .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst);
            }
            Some((Ok(Bytes::from(data)), (pos + n, sent + n, guard)))
        }
    })
}
