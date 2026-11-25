//! `idb doctor URL`: a short probe that explains what limits the speed
//! **before** downloading (spec §7.15). It never writes to disk.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use idb_types::{Diagnostic, DiagnosticCode, DownloadState, FailReason, NeedsUserReason};
use reqwest::{StatusCode, header};

use crate::controller::{Controller, Mode};
use crate::download::{Downloader, looks_like_unexpected_html, probe_mid, settled_diagnostic};
use crate::headers;

const WINDOW: Duration = Duration::from_millis(500);

/// What the doctor found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DoctorReport {
    pub final_url: String,
    pub size: Option<u64>,
    /// `None` when it could not be checked (unknown size or early failure).
    pub ranges: Option<bool>,
    /// Strong ETag or Last-Modified available (resume and verification).
    pub validator: bool,
    /// Connections the measurement settled on.
    pub recommended_connections: u32,
    /// Best smoothed rate measured, in bytes/s.
    pub peak_rate: u64,
    pub diagnostics: Vec<Diagnostic>,
    /// Set when the link cannot be downloaded as is (login, HTML, 404...).
    pub blocking: Option<DownloadState>,
}

enum Reader {
    Ended,
    Throttled(Option<Duration>),
}

/// Reads and discards a response body, counting bytes.
async fn drain(mut resp: reqwest::Response, counter: Arc<AtomicU64>) -> Reader {
    while let Ok(Some(chunk)) = resp.chunk().await {
        counter.fetch_add(chunk.len() as u64, Ordering::Relaxed);
    }
    Reader::Ended
}

async fn extra_reader(
    client: reqwest::Client,
    url: String,
    offset: u64,
    counter: Arc<AtomicU64>,
) -> Reader {
    let resp = client
        .get(&url)
        .header(header::ACCEPT_ENCODING, "identity")
        .header(header::RANGE, format!("bytes={offset}-"))
        .send()
        .await;
    match resp {
        Ok(r) if r.status() == StatusCode::PARTIAL_CONTENT => drain(r, counter).await,
        Ok(r)
            if r.status() == StatusCode::TOO_MANY_REQUESTS
                || r.status() == StatusCode::SERVICE_UNAVAILABLE =>
        {
            let delay = r
                .headers()
                .get(header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(headers::retry_after);
            Reader::Throttled(delay)
        }
        _ => Reader::Ended,
    }
}

impl Downloader {
    /// Probes `url` for at most `budget` and explains what limits the speed.
    pub async fn doctor(&self, url: &str, budget: Duration) -> DoctorReport {
        let mut report = DoctorReport::default();
        let first = self
            .client
            .get(url)
            .header(header::ACCEPT_ENCODING, "identity")
            .header(header::RANGE, "bytes=0-")
            .send()
            .await;
        let resp = match first {
            Ok(r) => r,
            Err(_) => {
                report.blocking = Some(DownloadState::Failed(FailReason::NetworkUnreachable));
                return report;
            }
        };
        report.final_url = resp.url().to_string();
        let status = resp.status();
        let h = resp.headers();
        let get = |n: header::HeaderName| h.get(n).and_then(|v| v.to_str().ok()).map(str::to_owned);
        match status {
            StatusCode::OK | StatusCode::PARTIAL_CONTENT => {}
            StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE => {
                let secs = get(header::RETRY_AFTER)
                    .as_deref()
                    .and_then(headers::retry_after)
                    .map_or(0, |d| d.as_secs());
                report.diagnostics.push(
                    DiagnosticCode::ServerAskedToWait {
                        secs,
                        reduced_to: 1,
                    }
                    .into(),
                );
                return report;
            }
            StatusCode::NOT_FOUND | StatusCode::GONE => {
                report.blocking = Some(DownloadState::Failed(FailReason::NotFound));
                return report;
            }
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                report.blocking = Some(DownloadState::NeedsUser(NeedsUserReason::LoginRequired));
                return report;
            }
            _ => {
                report.blocking = Some(DownloadState::Failed(FailReason::ServerError));
                return report;
            }
        }

        let range = get(header::CONTENT_RANGE)
            .as_deref()
            .and_then(headers::content_range);
        let total = match (status, range) {
            (StatusCode::PARTIAL_CONTENT, Some((_, _, t))) => t,
            _ => resp.content_length(),
        };
        report.size = total;
        report.validator = get(header::ETAG).is_some_and(|e| !e.starts_with("W/"))
            || get(header::LAST_MODIFIED).is_some();

        if looks_like_unexpected_html(get(header::CONTENT_TYPE).as_deref(), resp.url()) {
            let size = resp.content_length().unwrap_or(0);
            report.diagnostics.push(
                DiagnosticCode::HtmlInsteadOfFile {
                    size: idb_types::ByteCount(size),
                }
                .into(),
            );
            report.blocking = Some(DownloadState::NeedsUser(NeedsUserReason::HtmlInsteadOfFile));
            return report;
        }

        report.ranges = match (status, total) {
            (StatusCode::PARTIAL_CONTENT, _) => Some(true),
            (_, Some(t)) if t > 1 => Some(probe_mid(&self.client, url, t).await),
            _ => None,
        };
        if report.ranges == Some(false) {
            report
                .diagnostics
                .push(DiagnosticCode::RangeNotSupported.into());
        }

        self.measure(url, resp, total, &mut report, budget).await;
        report
    }

    /// Lets the adaptive controller ramp up on throwaway readers.
    async fn measure(
        &self,
        url: &str,
        first: reqwest::Response,
        total: Option<u64>,
        report: &mut DoctorReport,
        budget: Duration,
    ) {
        let parallel = report.ranges == Some(true) && total.is_some_and(|t| t > 0);
        let max = match self.config.connections {
            Mode::Auto { max } => max,
            Mode::Fixed(n) => n,
        };
        let mut ctl = Controller::new(if parallel {
            Mode::Auto { max }
        } else {
            Mode::Fixed(1)
        });
        let counter = Arc::new(AtomicU64::new(0));
        let mut readers = tokio::task::JoinSet::new();
        readers.spawn(drain(first, counter.clone()));
        let mut opened = 1u64;
        let deadline = tokio::time::Instant::now() + budget;
        let mut window = tokio::time::interval(WINDOW);
        window.tick().await;
        let mut last = 0u64;
        let mut throttled = false;
        let mut settled = false;
        loop {
            tokio::select! {
                _ = window.tick() => {
                    let now = counter.load(Ordering::Relaxed);
                    let bytes = now - last;
                    last = now;
                    if let Some(d) = ctl.on_window(bytes, WINDOW) {
                        if d.measured && d.from != d.to {
                            report.diagnostics.push(DiagnosticCode::ConnectionsAdjusted {
                                from: d.from as u32,
                                to: d.to as u32,
                                gain_pct: d.gain_pct,
                            }.into());
                        }
                        if d.settled {
                            settled = true;
                            if parallel && !throttled {
                                report.diagnostics.push(settled_diagnostic(d.to, d.rate));
                            }
                            break;
                        }
                    }
                    // Open readers spread over the file to reach the target.
                    while parallel && (readers.len() as u64) < ctl.target() as u64 {
                        let t = total.unwrap_or(1);
                        let offset = t * opened / (max as u64 + 1);
                        opened += 1;
                        readers.spawn(extra_reader(self.client.clone(), url.to_owned(), offset, counter.clone()));
                    }
                }
                ended = readers.join_next(), if !readers.is_empty() => {
                    if let Some(Ok(Reader::Throttled(delay))) = ended
                        && !throttled
                    {
                        throttled = true;
                        let _ = ctl.on_throttle();
                        report.diagnostics.push(DiagnosticCode::ServerAskedToWait {
                            secs: delay.map_or(0, |d| d.as_secs()),
                            reduced_to: ctl.target() as u32,
                        }.into());
                    }
                    if readers.is_empty() {
                        break; // the file is shorter than the measurement
                    }
                }
                _ = tokio::time::sleep_until(deadline) => break,
            }
        }
        readers.abort_all();
        if parallel && !settled && !throttled && ctl.best_rate() > 0.0 {
            // Out of time before settling: explain what was measured so far.
            report
                .diagnostics
                .push(settled_diagnostic(ctl.target(), ctl.best_rate()));
        }
        report.recommended_connections = ctl.target() as u32;
        report.peak_rate = ctl.best_rate() as u64;
    }
}
