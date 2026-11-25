//! A small download manager: start, pause, resume and list downloads that
//! run concurrently. Shared by the mobile facade (`idb-mobile`) and, later,
//! the daemon (`idbd`).
//!
//! Pausing aborts the download task. That is safe by construction: the
//! journal only claims bytes that are on disk (the same guarantee the
//! SIGKILL crash tests check), and resuming simply runs the download again,
//! which continues from the journal.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use idb_types::api::DownloadSummary;
use idb_types::{ByteCount, BytesPerSec, DownloadId, DownloadState};

use crate::{Downloader, Progress, Request};

/// Local handle of a download inside one manager (stable while it lives).
pub type Handle = u64;

struct Entry {
    url: String,
    dir: PathBuf,
    summary: DownloadSummary,
    task: Option<tokio::task::JoinHandle<()>>,
}

/// Called (from a background task) whenever a download changes.
pub type Listener = Arc<dyn Fn(Handle, &DownloadSummary) + Send + Sync>;

pub struct Manager {
    downloader: Arc<Downloader>,
    runtime: tokio::runtime::Handle,
    entries: Arc<Mutex<HashMap<Handle, Entry>>>,
    next: Mutex<Handle>,
    listener: Arc<Mutex<Option<Listener>>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Manager {
    pub fn new(downloader: Downloader, runtime: tokio::runtime::Handle) -> Manager {
        Manager {
            downloader: Arc::new(downloader),
            runtime,
            entries: Arc::new(Mutex::new(HashMap::new())),
            next: Mutex::new(1),
            listener: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set_listener(&self, l: Option<Listener>) {
        *lock(&self.listener) = l;
    }

    /// Starts downloading `url` into `dir`; returns its handle.
    pub fn start(&self, url: String, dir: PathBuf) -> Handle {
        let handle = {
            let mut n = lock(&self.next);
            let h = *n;
            *n += 1;
            h
        };
        let summary = DownloadSummary {
            id: DownloadId(0),
            url: url.clone(),
            filename: None,
            state: DownloadState::Queued,
            done: ByteCount(0),
            total: None,
            speed: BytesPerSec(0),
            eta_secs: None,
        };
        lock(&self.entries).insert(
            handle,
            Entry {
                url,
                dir,
                summary,
                task: None,
            },
        );
        self.spawn(handle);
        handle
    }

    fn spawn(&self, handle: Handle) {
        let Some((url, dir)) = lock(&self.entries)
            .get(&handle)
            .map(|e| (e.url.clone(), e.dir.clone()))
        else {
            return;
        };
        let (dl, entries, listener) = (
            self.downloader.clone(),
            self.entries.clone(),
            self.listener.clone(),
        );
        let task = self.runtime.spawn(async move {
            let update = |f: &dyn Fn(&mut DownloadSummary)| {
                let snapshot = {
                    let mut map = lock(&entries);
                    let Some(e) = map.get_mut(&handle) else {
                        return;
                    };
                    f(&mut e.summary);
                    e.summary.clone()
                };
                if let Some(l) = lock(&listener).clone() {
                    l(handle, &snapshot);
                }
            };
            let req = Request {
                url,
                dir,
                expected_sha256: None,
            };
            let result = dl
                .run(req, |p: Progress| {
                    update(&|s| {
                        s.id = p.id;
                        s.state = p.state.clone();
                        s.done = ByteCount(p.done);
                        s.total = p.total.map(ByteCount);
                    })
                })
                .await;
            match result {
                Ok(out) => update(&|s| {
                    s.id = out.id;
                    s.state = out.state.clone();
                    s.filename = out
                        .path
                        .as_ref()
                        .and_then(|p| p.file_name())
                        .map(|n| n.to_string_lossy().into_owned());
                    if matches!(out.state, DownloadState::Complete(_)) {
                        s.done = s.total.unwrap_or(s.done);
                    }
                }),
                Err(e) => {
                    tracing::error!("download failed: {e}");
                    update(&|s| {
                        s.state = DownloadState::Failed(idb_types::FailReason::ServerError);
                    });
                }
            }
        });
        if let Some(e) = lock(&self.entries).get_mut(&handle) {
            e.task = Some(task);
        }
    }

    /// Stops a running download; its progress stays in the journal.
    pub fn pause(&self, handle: Handle) -> bool {
        let snapshot = {
            let mut map = lock(&self.entries);
            let Some(e) = map.get_mut(&handle) else {
                return false;
            };
            if e.summary.state.is_terminal() {
                return false;
            }
            if let Some(t) = e.task.take() {
                t.abort();
            }
            e.summary.state = DownloadState::Paused;
            e.summary.speed = BytesPerSec(0);
            e.summary.clone()
        };
        if let Some(l) = lock(&self.listener).clone() {
            l(handle, &snapshot);
        }
        true
    }

    /// Continues a paused (or failed) download from the journal.
    pub fn resume(&self, handle: Handle) -> bool {
        let ok = lock(&self.entries).get(&handle).is_some_and(|e| {
            e.task.as_ref().is_none_or(|t| t.is_finished())
                && !matches!(e.summary.state, DownloadState::Complete(_))
        });
        if ok {
            self.spawn(handle);
        }
        ok
    }

    pub fn list(&self) -> Vec<(Handle, DownloadSummary)> {
        let mut v: Vec<_> = lock(&self.entries)
            .iter()
            .map(|(h, e)| (*h, e.summary.clone()))
            .collect();
        v.sort_by_key(|(h, _)| *h);
        v
    }
}
