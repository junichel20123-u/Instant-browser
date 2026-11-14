//! Download loop: probing, single-connection streaming and the hand-off
//! to the segmented runner (see `segmented.rs`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use idb_store::{Journal, Record, ResourceMeta, SegmentRecord};
use idb_types::{
    Diagnostic, DiagnosticCode, DownloadId, DownloadState, FailReason, HashAlgo, HashSource,
    NeedsUserReason, VerificationLevel,
};
use reqwest::{StatusCode, header};
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use crate::controller::{Controller, Decision, Mode};
use crate::segmented::{self, End, Event, Shared};
use crate::segments;
use crate::workmap::WorkMap;
use crate::{EngineError, disk, headers, verify};

/// Retry behaviour for transient failures.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Consecutive failures without new progress before giving up.
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
    /// Upper bound for honouring `Retry-After`.
    pub max_retry_after: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy {
            max_attempts: 8,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(60),
            max_retry_after: Duration::from_secs(3600),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub retry: RetryPolicy,
    /// Durable checkpoint (fdatasync + journal) every this many bytes.
    pub checkpoint_bytes: u64,
    /// ...or after this much time, whichever comes first (slow links).
    pub checkpoint_interval: Duration,
    pub connect_timeout: Duration,
    /// Time without receiving any byte before the connection counts as stalled.
    pub read_timeout: Duration,
    pub max_redirects: usize,
    /// Ignore proxy environment variables (used by tests on loopback).
    pub no_proxy: bool,
    pub user_agent: String,
    /// How many parallel connections to use when the server allows ranges.
    pub connections: Mode,
    /// A connection that receives nothing for this long is replaced.
    pub stall_timeout: Duration,
    /// Smallest segment worth its own connection.
    pub min_segment: u64,
    /// Free space to keep on the destination filesystem.
    pub min_free_bytes: u64,
}

/// Polite ceiling of connections per host (spec §3.3).
pub const MAX_CONNECTIONS: usize = 8;

impl Default for Config {
    fn default() -> Self {
        Config {
            retry: RetryPolicy::default(),
            checkpoint_bytes: 4 << 20,
            checkpoint_interval: Duration::from_secs(1),
            connect_timeout: Duration::from_secs(15),
            read_timeout: Duration::from_secs(30),
            max_redirects: 10,
            no_proxy: false,
            user_agent: format!("idb/{}", env!("CARGO_PKG_VERSION")),
            connections: Mode::Auto {
                max: MAX_CONNECTIONS,
            },
            stall_timeout: Duration::from_secs(20),
            min_segment: 1 << 20,
            min_free_bytes: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Request {
    pub url: String,
    /// Absolute destination directory (validated by the caller).
    pub dir: PathBuf,
    /// Expected SHA-256 (hex) given by the user.
    pub expected_sha256: Option<String>,
}

/// Progress reported to the caller (CLI, daemon).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub id: DownloadId,
    pub state: DownloadState,
    pub done: u64,
    pub total: Option<u64>,
}

/// Terminal result of [`Downloader::run`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub id: DownloadId,
    pub state: DownloadState,
    /// Final file (when complete) or the kept `.part` file.
    pub path: Option<PathBuf>,
    /// Doctor findings gathered while downloading.
    pub diagnostics: Vec<Diagnostic>,
    pub stats: Stats,
}

/// Measurements of a segmented run (for `--json` and benchmarks).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    /// Time until the rate first reached 90% of the best measured rate.
    pub ramp_ms: Option<u64>,
    /// Connection target when the download ended.
    pub connections: u32,
    /// Best smoothed rate seen, in bytes/s.
    pub peak_rate: u64,
}

/// Measurement window of the adaptive controller (spec §7.6).
const WINDOW: Duration = Duration::from_millis(500);

pub struct Downloader {
    client: reqwest::Client,
    config: Config,
    journal: Arc<Mutex<Journal>>,
}

/// Why one attempt stopped.
enum Stop {
    Done,
    /// Transient problem: retry after the given delay (or backoff).
    Retry(Option<Duration>),
    Final(DownloadState),
    /// Ranges confirmed: continue in parallel; the response becomes segment 0.
    Segment(reqwest::Response),
}

struct Job {
    id: DownloadId,
    url: String,
    dir: PathBuf,
    meta: Option<ResourceMeta>,
    /// Bytes present (and in order) at the start of `.part`.
    offset: u64,
    /// Offset recorded as durable in the journal.
    durable: u64,
    /// Destination recorded before the rename, if we got that far.
    final_path: Option<PathBuf>,
    diagnostics: Vec<Diagnostic>,
    stats: Stats,
}

impl Job {
    fn part_path(&self) -> Option<PathBuf> {
        self.meta
            .as_ref()
            .map(|m| self.dir.join(format!("{}.part", m.filename)))
    }
}

pub(crate) fn io_state(e: &std::io::Error) -> DownloadState {
    // ENOSPC = 28, EDQUOT = 122 on Linux.
    match (e.kind(), e.raw_os_error()) {
        (std::io::ErrorKind::PermissionDenied, _) => {
            DownloadState::NeedsUser(NeedsUserReason::PermissionDenied)
        }
        (_, Some(28 | 122)) | (std::io::ErrorKind::StorageFull, _) => {
            DownloadState::NeedsUser(NeedsUserReason::DiskFull)
        }
        _ => DownloadState::Failed(FailReason::ServerError),
    }
}

fn strong_etag(meta: &ResourceMeta) -> Option<&str> {
    meta.etag.as_deref().filter(|e| !e.starts_with("W/"))
}

fn looks_like_unexpected_html(content_type: Option<&str>, url: &reqwest::Url) -> bool {
    let is_html = content_type.is_some_and(|ct| {
        let ct = ct.to_ascii_lowercase();
        ct.starts_with("text/html") || ct.starts_with("application/xhtml")
    });
    let wants_html =
        url.path().rsplit('.').next().is_some_and(|ext| {
            matches!(ext.to_ascii_lowercase().as_str(), "html" | "htm" | "xhtml")
        });
    is_html && !wants_html
}

impl Downloader {
    pub fn new(config: Config, journal: Journal) -> Result<Self, EngineError> {
        let mut b = reqwest::Client::builder()
            .user_agent(config.user_agent.clone())
            .connect_timeout(config.connect_timeout)
            .read_timeout(config.read_timeout)
            .redirect(reqwest::redirect::Policy::limited(config.max_redirects));
        if config.no_proxy {
            b = b.no_proxy();
        }
        Ok(Downloader {
            client: b.build()?,
            config,
            journal: Arc::new(Mutex::new(journal)),
        })
    }

    async fn journal<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Journal) -> idb_store::Result<T> + Send + 'static,
    ) -> Result<T, EngineError> {
        let j = self.journal.clone();
        let r = tokio::task::spawn_blocking(move || {
            let mut guard = j.lock().map_err(|_| idb_store::StoreError::Poisoned)?;
            f(&mut guard)
        })
        .await?;
        Ok(r?)
    }

    /// Runs a download to a terminal state, resuming from the journal when
    /// a previous attempt for the same `(url, dir)` exists.
    pub async fn run(
        &self,
        req: Request,
        mut on_progress: impl FnMut(Progress) + Send,
    ) -> Result<Outcome, EngineError> {
        let dir_key = req.dir.to_string_lossy().into_owned();
        let (url, key) = (req.url.clone(), dir_key.clone());
        let previous = self.journal(move |j| j.find(&url, &key)).await?;
        let mut job = self.prepare(&req, &dir_key, previous).await?;

        // Already renamed (finished earlier, or crashed between the rename
        // and recording Complete): the `.part` is gone and the destination
        // exists. Verify it again instead of downloading a duplicate.
        if let (Some(dest), Some(part)) = (job.final_path.clone(), job.part_path())
            && !tokio::fs::try_exists(&part).await?
            && tokio::fs::try_exists(&dest).await?
        {
            job.offset = tokio::fs::metadata(&dest).await?.len();
            let state = self
                .verify(&job, &dest, req.expected_sha256.as_deref())
                .await?;
            return self.finish(&job, state, Some(dest)).await;
        }

        // A segmented download in progress resumes segment by segment.
        let id = job.id;
        let plan = self.journal(move |j| j.segments(id)).await?;
        if !plan.is_empty() && job.meta.is_some() {
            if let Some(out) = self
                .run_segmented(&mut job, Some(plan), None, &mut on_progress)
                .await?
            {
                return Ok(out);
            }
            return self.complete(job, &req, &mut on_progress).await;
        }

        let mut failures = 0u32;
        let mut best = job.offset;
        loop {
            on_progress(self.progress(&job, DownloadState::Downloading));
            let stop = self.attempt(&mut job, &mut on_progress).await;
            match stop {
                Ok(Stop::Done) => break,
                Ok(Stop::Final(state)) => return self.finish(&job, state, None).await,
                Ok(Stop::Segment(resp)) => {
                    if let Some(out) = self
                        .run_segmented(&mut job, None, Some(resp), &mut on_progress)
                        .await?
                    {
                        return Ok(out);
                    }
                    return self.complete(job, &req, &mut on_progress).await;
                }
                Ok(Stop::Retry(server_delay)) => {
                    self.checkpoint(&mut job).await?;
                    if job.offset > best {
                        best = job.offset;
                        failures = 0;
                    }
                    failures += 1;
                    if failures > self.config.retry.max_attempts {
                        let st = DownloadState::Failed(FailReason::NetworkUnreachable);
                        return self.finish(&job, st, job.part_path()).await;
                    }
                    let delay = match server_delay {
                        Some(d) => {
                            let d = d.min(self.config.retry.max_retry_after);
                            let until = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map_or(0, |n| (n + d).as_secs());
                            on_progress(self.progress(
                                &job,
                                DownloadState::PoliteWait {
                                    until_unix_secs: until,
                                },
                            ));
                            d
                        }
                        None => self.backoff(failures),
                    };
                    tokio::time::sleep(delay).await;
                }
                Err(e) => {
                    let st = io_state(&e);
                    return self.finish(&job, st, job.part_path()).await;
                }
            }
        }

        self.complete(job, &req, &mut on_progress).await
    }

    /// Verification and atomic publication, shared by every download mode.
    async fn complete(
        &self,
        mut job: Job,
        req: &Request,
        on_progress: &mut (impl FnMut(Progress) + Send),
    ) -> Result<Outcome, EngineError> {
        on_progress(self.progress(&job, DownloadState::Verifying { permille: 0 }));
        self.checkpoint(&mut job).await?;
        let Some(part) = job.part_path() else {
            return self
                .finish(&job, DownloadState::Failed(FailReason::ServerError), None)
                .await;
        };
        let state = self
            .verify(&job, &part, req.expected_sha256.as_deref())
            .await?;
        if !matches!(state, DownloadState::Complete(_)) {
            return self.finish(&job, state, Some(part)).await;
        }
        let name = job
            .meta
            .as_ref()
            .map_or("download", |m| m.filename.as_str());
        let dest = verify::unique_destination(&job.dir, name);
        let (id, d) = (job.id, dest.to_string_lossy().into_owned());
        self.journal(move |j| j.set_final_path(id, &d)).await?;
        tokio::fs::rename(&part, &dest).await?;
        self.finish(&job, state, Some(dest)).await
    }

    async fn prepare(
        &self,
        req: &Request,
        dir_key: &str,
        previous: Option<Record>,
    ) -> Result<Job, EngineError> {
        if let Some(rec) = previous.filter(|r| match &r.state {
            // Already done: reuse only while the final file still exists
            // (makes `get` idempotent and covers a crash after Complete).
            DownloadState::Complete(_) => r
                .final_path
                .as_deref()
                .is_some_and(|p| Path::new(p).exists()),
            // The resource changed or the bytes failed verification:
            // re-running means "start over".
            DownloadState::NeedsUser(NeedsUserReason::ResourceChanged)
            | DownloadState::Failed(FailReason::VerificationFailed) => false,
            _ => true,
        }) {
            let mut job = Job {
                id: rec.id,
                url: rec.url,
                dir: req.dir.clone(),
                meta: rec.meta,
                offset: 0,
                durable: 0,
                final_path: rec.final_path.map(PathBuf::from),
                diagnostics: Vec::new(),
                stats: Stats::default(),
            };
            if let Some(part) = job.part_path() {
                let len = tokio::fs::metadata(&part).await.map_or(0, |m| m.len());
                // Bytes past the durable mark may be garbage after a crash.
                job.offset = len.min(rec.downloaded);
                job.durable = job.offset;
            }
            tracing::info!(id = job.id.0, offset = job.offset, "resuming");
            return Ok(job);
        }
        let (url, key) = (req.url.clone(), dir_key.to_owned());
        let id = self.journal(move |j| j.create(&url, &key)).await?;
        Ok(Job {
            id,
            url: req.url.clone(),
            dir: req.dir.clone(),
            meta: None,
            offset: 0,
            durable: 0,
            final_path: None,
            diagnostics: Vec::new(),
            stats: Stats::default(),
        })
    }

    fn progress(&self, job: &Job, state: DownloadState) -> Progress {
        Progress {
            id: job.id,
            state,
            done: job.offset,
            total: job.meta.as_ref().and_then(|m| m.total),
        }
    }

    fn backoff(&self, failures: u32) -> Duration {
        backoff(&self.config.retry, failures)
    }

    /// Makes progress durable: fdatasync the part file, then record it.
    async fn checkpoint(&self, job: &mut Job) -> Result<(), EngineError> {
        if job.offset == job.durable {
            return Ok(());
        }
        if let Some(part) = job.part_path()
            && let Ok(f) = OpenOptions::new().write(true).open(&part).await
        {
            f.sync_data().await?;
        }
        let (id, off) = (job.id, job.offset);
        self.journal(move |j| j.set_downloaded(id, off)).await?;
        job.durable = off;
        Ok(())
    }

    async fn finish(
        &self,
        job: &Job,
        state: DownloadState,
        path: Option<PathBuf>,
    ) -> Result<Outcome, EngineError> {
        let (id, st) = (job.id, state.clone());
        self.journal(move |j| j.set_state(id, &st)).await?;
        Ok(Outcome {
            id: job.id,
            state,
            path,
            diagnostics: job.diagnostics.clone(),
            stats: job.stats.clone(),
        })
    }

    /// One HTTP request; streams the body into the part file.
    async fn attempt(
        &self,
        job: &mut Job,
        on_progress: &mut (impl FnMut(Progress) + Send),
    ) -> std::io::Result<Stop> {
        let mut rb = self
            .client
            .get(&job.url)
            .header(header::ACCEPT_ENCODING, "identity")
            .header(header::RANGE, format!("bytes={}-", job.offset));
        let resuming = job.offset > 0;
        if resuming && let Some(meta) = &job.meta {
            if let Some(tag) = strong_etag(meta) {
                rb = rb.header(header::IF_RANGE, tag);
            } else if let Some(lm) = &meta.last_modified {
                rb = rb.header(header::IF_RANGE, lm);
            }
        }
        let resp = match rb.send().await {
            Ok(r) => r,
            Err(e) if e.is_builder() || e.is_redirect() => {
                let reason = if e.is_redirect() {
                    FailReason::TooManyRedirects
                } else {
                    FailReason::InvalidUrl
                };
                return Ok(Stop::Final(DownloadState::Failed(reason)));
            }
            Err(e) => {
                tracing::warn!("request failed: {e}");
                return Ok(Stop::Retry(None));
            }
        };

        let status = resp.status();
        let h = resp.headers();
        let get = |n: header::HeaderName| h.get(n).and_then(|v| v.to_str().ok()).map(str::to_owned);
        let server_delay = get(header::RETRY_AFTER)
            .as_deref()
            .and_then(headers::retry_after);
        match status {
            StatusCode::OK | StatusCode::PARTIAL_CONTENT => {}
            StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE => {
                return Ok(Stop::Retry(server_delay));
            }
            StatusCode::NOT_FOUND | StatusCode::GONE => {
                return Ok(Stop::Final(DownloadState::Failed(FailReason::NotFound)));
            }
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                return Ok(Stop::Final(DownloadState::NeedsUser(
                    NeedsUserReason::LoginRequired,
                )));
            }
            StatusCode::RANGE_NOT_SATISFIABLE if resuming => {
                // Usually means the file shrank or changed.
                return Ok(Stop::Final(DownloadState::NeedsUser(
                    NeedsUserReason::ResourceChanged,
                )));
            }
            s if s.is_server_error() || s == StatusCode::REQUEST_TIMEOUT => {
                return Ok(Stop::Retry(server_delay));
            }
            _ => return Ok(Stop::Final(DownloadState::Failed(FailReason::ServerError))),
        }

        let content_length = resp.content_length();
        let range = get(header::CONTENT_RANGE)
            .as_deref()
            .and_then(headers::content_range);
        let etag = get(header::ETAG);
        let last_modified = get(header::LAST_MODIFIED);

        // Work out where this body starts and the total size.
        let (start, total) = if status == StatusCode::PARTIAL_CONTENT {
            match range {
                Some((a, _, t)) if a == job.offset => (a, t),
                _ => return Ok(Stop::Final(DownloadState::Failed(FailReason::ServerError))),
            }
        } else {
            if resuming && let Some(meta) = &job.meta {
                // 200 to a ranged request: either the resource changed
                // (If-Range failed) or the server ignores ranges. Only an
                // identical validator proves it is the same resource.
                let same = match (
                    strong_etag(meta),
                    &etag,
                    &meta.last_modified,
                    &last_modified,
                ) {
                    (Some(old), Some(new), _, _) => old == new,
                    (None, _, Some(old), Some(new)) => old == new,
                    _ => false,
                };
                let had_validator = strong_etag(meta).is_some() || meta.last_modified.is_some();
                if had_validator && !same {
                    return Ok(Stop::Final(DownloadState::NeedsUser(
                        NeedsUserReason::ResourceChanged,
                    )));
                }
                tracing::info!("server ignored range; restarting from zero with this body");
            }
            (0, content_length)
        };

        if let Some(meta) = &job.meta {
            // Extra identity checks on resume.
            let changed = (meta.total.is_some() && total.is_some() && meta.total != total)
                || (strong_etag(meta).is_some() && etag.is_some() && meta.etag != etag);
            if changed {
                return Ok(Stop::Final(DownloadState::NeedsUser(
                    NeedsUserReason::ResourceChanged,
                )));
            }
        } else {
            if looks_like_unexpected_html(get(header::CONTENT_TYPE).as_deref(), resp.url()) {
                return Ok(Stop::Final(DownloadState::NeedsUser(
                    NeedsUserReason::HtmlInsteadOfFile,
                )));
            }
            let raw = get(header::CONTENT_DISPOSITION)
                .as_deref()
                .and_then(headers::content_disposition_filename)
                .or_else(|| headers::filename_from_url(resp.url()))
                .unwrap_or_default();
            let meta = ResourceMeta {
                final_url: resp.url().to_string(),
                filename: headers::sanitize_filename(&raw),
                total,
                etag,
                last_modified,
            };
            let (id, m) = (job.id, meta.clone());
            self.journal(move |j| j.set_meta(id, &m))
                .await
                .map_err(std::io::Error::other)?;
            job.meta = Some(meta);

            if let Some(t) = total
                && !disk::has_room(&job.dir, t, self.config.min_free_bytes)?
            {
                return Ok(Stop::Final(DownloadState::NeedsUser(
                    NeedsUserReason::DiskFull,
                )));
            }
            if let Some(stop) = self.decide_segmentation(job, status, total).await {
                return Ok(match stop {
                    true => Stop::Segment(resp),
                    false => {
                        job.diagnostics
                            .push(DiagnosticCode::RangeNotSupported.into());
                        self.stream_single(job, resp, start, on_progress).await?
                    }
                });
            }
        }

        self.stream_single(job, resp, start, on_progress).await
    }

    /// Whether to go parallel. `None`: not applicable (one connection asked,
    /// unknown or small size). `Some(true)`: ranges confirmed. `Some(false)`:
    /// the server does not support ranges.
    async fn decide_segmentation(
        &self,
        job: &Job,
        status: StatusCode,
        total: Option<u64>,
    ) -> Option<bool> {
        let total = total?;
        if total == 0 {
            return None;
        }
        // A 206 proves range support: even one connection then uses the
        // segmented writer (resumable, 256 KiB pwrite batches).
        if status == StatusCode::PARTIAL_CONTENT {
            return Some(true);
        }
        let parallel = !matches!(self.config.connections, Mode::Fixed(1));
        if !parallel || total < 2 * self.config.min_segment {
            return None;
        }
        // 200 without proof of range support: ask for one byte in the middle
        // (servers that support ranges without announcing them, Gopeed #1652).
        let mid = total / 2;
        let probe = self
            .client
            .get(&job.url)
            .header(header::ACCEPT_ENCODING, "identity")
            .header(header::RANGE, format!("bytes={mid}-{mid}"))
            .send()
            .await
            .ok()?;
        let exact = probe.status() == StatusCode::PARTIAL_CONTENT
            && probe
                .headers()
                .get(header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(headers::content_range)
                == Some((mid, mid, Some(total)));
        Some(exact)
    }

    async fn stream_single(
        &self,
        job: &mut Job,
        resp: reqwest::Response,
        start: u64,
        on_progress: &mut (impl FnMut(Progress) + Send),
    ) -> std::io::Result<Stop> {
        let Some(part) = job.part_path() else {
            return Ok(Stop::Final(DownloadState::Failed(FailReason::ServerError)));
        };
        let file = open_part(&part, start).await?;
        // Batch small network chunks into 256 KiB writes (CPU, spec §7.4).
        let mut file = tokio::io::BufWriter::with_capacity(segmented::WRITE_BUF, file);
        job.offset = start;
        if start < job.durable {
            let id = job.id;
            self.journal(move |j| j.set_downloaded(id, start))
                .await
                .map_err(std::io::Error::other)?;
            job.durable = start;
        }
        self.stream_body(resp, &mut file, job, on_progress).await
    }

    async fn stream_body(
        &self,
        mut resp: reqwest::Response,
        file: &mut tokio::io::BufWriter<File>,
        job: &mut Job,
        on_progress: &mut (impl FnMut(Progress) + Send),
    ) -> std::io::Result<Stop> {
        let mut since_checkpoint = 0u64;
        let mut last_checkpoint = tokio::time::Instant::now();
        loop {
            match resp.chunk().await {
                Ok(Some(bytes)) => {
                    file.write_all(&bytes).await?;
                    job.offset += bytes.len() as u64;
                    since_checkpoint += bytes.len() as u64;
                    if since_checkpoint >= self.config.checkpoint_bytes
                        || last_checkpoint.elapsed() >= self.config.checkpoint_interval
                    {
                        last_checkpoint = tokio::time::Instant::now();
                        file.flush().await?;
                        self.checkpoint(job).await.map_err(std::io::Error::other)?;
                        since_checkpoint = 0;
                        on_progress(self.progress(job, DownloadState::Downloading));
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!(offset = job.offset, "body interrupted: {e}");
                    file.flush().await?;
                    return Ok(Stop::Retry(None));
                }
            }
        }
        file.flush().await?;
        let total = job.meta.as_ref().and_then(|m| m.total);
        if total.is_some_and(|t| job.offset < t) {
            tracing::warn!(offset = job.offset, "premature end of body");
            return Ok(Stop::Retry(None));
        }
        Ok(Stop::Done)
    }

    /// Runs the segmented download. `resume` holds the journal's segment map,
    /// `first` the probe response (fresh start). Returns `Some(outcome)` when
    /// the download stopped in a final state, `None` when every byte is on disk.
    async fn run_segmented(
        &self,
        job: &mut Job,
        resume: Option<Vec<SegmentRecord>>,
        first: Option<reqwest::Response>,
        on_progress: &mut (impl FnMut(Progress) + Send),
    ) -> Result<Option<Outcome>, EngineError> {
        let (Some(meta), Some(part)) = (job.meta.clone(), job.part_path()) else {
            return Ok(Some(
                self.finish(job, DownloadState::Failed(FailReason::ServerError), None)
                    .await?,
            ));
        };
        let total = meta.total.unwrap_or(0);
        let fresh = resume.is_none();

        // Open the part file; on resume, never trust bytes the file lacks.
        let path = part.clone();
        let opened =
            tokio::task::spawn_blocking(move || -> std::io::Result<(std::fs::File, u64)> {
                let f = std::fs::OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .read(true)
                    .write(true)
                    .open(&path)?;
                if fresh {
                    f.set_len(0)?;
                }
                let len = f.metadata()?.len();
                Ok((f, len))
            })
            .await?;
        let (file, len) = match opened {
            Ok(v) => v,
            Err(e) => return Ok(Some(self.finish(job, io_state(&e), None).await?)),
        };
        let min = self.config.min_segment;
        let mut map = match resume {
            Some(mut recs) => {
                for r in &mut recs {
                    r.done = r.done.min(len.saturating_sub(r.start)).min(r.end - r.start);
                }
                WorkMap::from_records(&recs, min)
            }
            None => match self.config.connections {
                Mode::Fixed(n) => WorkMap::with_plan(&segments::plan(total, n, min), min),
                Mode::Auto { .. } => WorkMap::new(total, min),
            },
        };
        let file = Arc::new(file);
        if fresh {
            let (id, p) = (job.id, map.snapshot());
            self.journal(move |j| j.set_segments(id, &p)).await?;
            // Reserve space in the background; writes may start meanwhile.
            let f = file.clone();
            tokio::task::spawn_blocking(move || {
                if let Err(e) = disk::preallocate(&f, total) {
                    tracing::warn!("preallocation failed: {e}");
                }
            });
        }

        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        let shared = Arc::new(Shared {
            client: self.client.clone(),
            url: job.url.clone(),
            if_range: strong_etag(&meta)
                .map(str::to_owned)
                .or_else(|| meta.last_modified.clone()),
            strong_etag: strong_etag(&meta).map(str::to_owned),
            total,
            file,
            received: std::sync::atomic::AtomicU64::new(0),
            events: tx,
        });

        let mut ctl = Controller::new(self.config.connections);
        let mut workers = tokio::task::JoinSet::new();
        type Task = (Arc<crate::workmap::Lease>, tokio::task::AbortHandle);
        let mut tasks: std::collections::HashMap<tokio::task::Id, Task> =
            std::collections::HashMap::new();
        let mut first = first;
        // The probe response continues as the worker of the first lease.
        if let Some(resp) = first.take()
            && let Some(lease) = map.assign()
        {
            let h = workers.spawn(segmented::worker(shared.clone(), lease.clone(), Some(resp)));
            tasks.insert(h.id(), (lease, h));
        }

        let started = tokio::time::Instant::now();
        let mut paused_until = started;
        let mut failures = 0u32;
        let mut best_written = map.written();
        let mut last_received = 0u64;
        let mut rates: Vec<(Duration, f64)> = Vec::new();
        let mut durable = map.snapshot();
        let mut window = tokio::time::interval(WINDOW);
        let mut tick = tokio::time::interval(self.config.checkpoint_interval);
        window.tick().await;

        let end = loop {
            // Match the number of busy connections to the controller's target.
            if tokio::time::Instant::now() >= paused_until {
                while busy(&map) < ctl.target() {
                    let Some(lease) = map.assign() else { break };
                    let h = workers.spawn(segmented::worker(shared.clone(), lease.clone(), None));
                    tasks.insert(h.id(), (lease, h));
                }
            }
            while busy(&map) > ctl.target() {
                let Some(lease) = map.largest_unreceived() else {
                    break;
                };
                if !map.park(&lease) {
                    break;
                }
            }
            // With nothing running and work left we are paused: the select
            // below wakes up when the pause ends (or on the next tick).
            if workers.is_empty() && map.is_complete() {
                break None;
            }

            tokio::select! {
                joined = workers.join_next_with_id(), if !workers.is_empty() => {
                    let (id, ended) = match joined {
                        Some(Ok((id, ended))) => (id, ended),
                        Some(Err(e)) if e.is_cancelled() => continue,
                        Some(Err(e)) => {
                            workers.abort_all();
                            return Err(e.into());
                        }
                        None => continue,
                    };
                    if let Some((lease, _)) = tasks.remove(&id) {
                        map.release(&lease);
                    }
                    match ended {
                        End::Done => {}
                        End::Final(state) => break Some(state),
                        End::Retry { delay, throttled } => {
                            let written = map.written();
                            if written > best_written {
                                best_written = written;
                                failures = 0;
                            }
                            failures += 1;
                            if failures > self.config.retry.max_attempts {
                                break Some(DownloadState::Failed(FailReason::NetworkUnreachable));
                            }
                            let wait = if throttled {
                                let d = delay.unwrap_or_else(|| backoff(&self.config.retry, failures))
                                    .min(self.config.retry.max_retry_after);
                                // Explained by ServerAskedToWait below.
                                let _ = ctl.on_throttle();
                                job.diagnostics.push(DiagnosticCode::ServerAskedToWait {
                                    secs: d.as_secs(),
                                    reduced_to: ctl.target() as u32,
                                }.into());
                                job.offset = map.written();
                                on_progress(self.progress(job, DownloadState::PoliteWait {
                                    until_unix_secs: unix_now_plus(d),
                                }));
                                d
                            } else {
                                backoff(&self.config.retry, failures)
                            };
                            paused_until = paused_until.max(tokio::time::Instant::now() + wait);
                        }
                    }
                }
                _ = window.tick() => {
                    let received = shared.received.load(std::sync::atomic::Ordering::Relaxed);
                    let bytes = received - last_received;
                    last_received = received;
                    if let Some(dec) = ctl.on_window(bytes, WINDOW) {
                        push_decision(job, dec);
                        if dec.settled && matches!(self.config.connections, Mode::Auto { .. }) {
                            job.diagnostics.push(settled_diagnostic(dec.to, dec.rate));
                        }
                    }
                    rates.push((started.elapsed(), bytes as f64 / WINDOW.as_secs_f64()));
                    // Watchdog: a connection that receives nothing is replaced.
                    let stalled: Vec<_> = tasks
                        .iter()
                        .filter(|(_, (l, _))| !l.is_finished() && l.idle_for() > self.config.stall_timeout)
                        .map(|(id, _)| *id)
                        .collect();
                    for id in stalled {
                        if let Some((lease, handle)) = tasks.remove(&id) {
                            handle.abort();
                            tracing::warn!(start = lease.start, "stalled connection reassigned");
                            map.release(&lease);
                            job.diagnostics
                                .push(DiagnosticCode::StalledConnectionReassigned.into());
                        }
                    }
                }
                _ = tick.tick() => {
                    durable = self.checkpoint_map(job, &shared, &map, durable).await?;
                    job.offset = map.written();
                    on_progress(self.progress(job, DownloadState::Downloading));
                }
                Some(Event::Rtt(d)) = rx.recv() => ctl.rtt_sample(d),
                _ = tokio::time::sleep_until(paused_until), if tokio::time::Instant::now() < paused_until => {}
            }
        };
        workers.abort_all();
        while workers.join_next().await.is_some() {}
        for (lease, _) in tasks.values() {
            map.release(lease);
        }
        self.checkpoint_map(job, &shared, &map, durable).await?;
        job.offset = map.written();
        job.durable = job.offset;
        job.stats = stats(&rates, &ctl);
        match end {
            None => Ok(None),
            Some(state) => Ok(Some(self.finish(job, state, Some(part)).await?)),
        }
    }

    /// Snapshot, fdatasync, then record the map (never the reverse).
    async fn checkpoint_map(
        &self,
        job: &Job,
        shared: &Arc<Shared>,
        map: &WorkMap,
        durable: Vec<SegmentRecord>,
    ) -> Result<Vec<SegmentRecord>, EngineError> {
        let snap = map.snapshot();
        if snap == durable {
            return Ok(durable);
        }
        let f = shared.file.clone();
        tokio::task::spawn_blocking(move || f.sync_data()).await??;
        let (id, s) = (job.id, snap.clone());
        self.journal(move |j| j.checkpoint_map(id, &s)).await?;
        Ok(snap)
    }

    async fn verify(
        &self,
        job: &Job,
        part: &Path,
        expected: Option<&str>,
    ) -> Result<DownloadState, EngineError> {
        let Some(meta) = &job.meta else {
            return Ok(DownloadState::Failed(FailReason::ServerError));
        };
        let len = tokio::fs::metadata(part).await?.len();
        if meta.total.is_some_and(|t| t != len) || len != job.offset {
            return Ok(DownloadState::Failed(FailReason::VerificationFailed));
        }
        if let Some(want) = expected {
            let got = verify::sha256_file(part.to_owned()).await?;
            return Ok(if got.eq_ignore_ascii_case(want.trim()) {
                DownloadState::Complete(VerificationLevel::HashMatched {
                    algo: HashAlgo::Sha256,
                    source: HashSource::User,
                })
            } else {
                DownloadState::Failed(FailReason::VerificationFailed)
            });
        }
        let level = match (
            meta.total,
            strong_etag(meta).is_some() || meta.last_modified.is_some(),
        ) {
            (Some(_), true) => VerificationLevel::SizeAndValidator,
            (Some(_), false) => VerificationLevel::SizeOnly,
            (None, _) => VerificationLevel::Unverified,
        };
        Ok(DownloadState::Complete(level))
    }
}

/// Opens the part file positioned at `start`, dropping anything after it.
/// Starting from zero always truncates (avoids stale tails, axel #303).
async fn open_part(path: &Path, start: u64) -> std::io::Result<File> {
    let mut f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)
        .await?;
    f.set_len(start).await?;
    f.seek(std::io::SeekFrom::Start(start)).await?;
    Ok(f)
}

/// Exponential backoff with up to 25% jitter, without an RNG dependency.
pub(crate) fn backoff(p: &RetryPolicy, failures: u32) -> Duration {
    let exp = p
        .base_delay
        .saturating_mul(1u32 << failures.min(16).saturating_sub(1));
    let capped = exp.min(p.max_delay);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    capped + capped.mul_f64(f64::from(nanos % 1000) / 4000.0)
}

/// Leases that still expect bytes from the network (parked ones do not).
fn busy(map: &WorkMap) -> usize {
    map.owned().filter(|l| l.unreceived() > 0).count()
}

fn unix_now_plus(d: Duration) -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |n| (n + d).as_secs())
}

fn push_decision(job: &mut Job, d: Decision) {
    // The first step is a trial, not a measured decision: nothing to explain.
    if !d.measured {
        return;
    }
    job.diagnostics.push(
        DiagnosticCode::ConnectionsAdjusted {
            from: d.from as u32,
            to: d.to as u32,
            gain_pct: d.gain_pct,
        }
        .into(),
    );
}

/// Explains where the controller settled (spec §7.15).
fn settled_diagnostic(conns: usize, rate: f64) -> Diagnostic {
    let total = idb_types::BytesPerSec(rate as u64);
    if conns > 2 {
        DiagnosticCode::ServerPerConnectionLimit {
            per_conn: idb_types::BytesPerSec((rate / conns as f64) as u64),
            conns: conns as u32,
            total,
        }
        .into()
    } else {
        DiagnosticCode::LinkSaturated { link_est: total }.into()
    }
}

fn stats(rates: &[(Duration, f64)], ctl: &Controller) -> Stats {
    // Ignore the last window: it is usually cut short by the end of the file.
    let full = &rates[..rates.len().saturating_sub(1)];
    let best = full.iter().map(|(_, r)| *r).fold(0.0, f64::max);
    let ramp = full
        .iter()
        .find(|(_, r)| best > 0.0 && *r >= 0.9 * best)
        .map(|(t, _)| t.as_millis() as u64);
    Stats {
        ramp_ms: ramp,
        connections: ctl.target() as u32,
        peak_rate: ctl.best_rate() as u64,
    }
}
