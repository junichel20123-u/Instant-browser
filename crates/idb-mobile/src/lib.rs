//! Facade of the IDB engine for mobile apps, exported with UniFFI.
//!
//! Kotlin (Android) and Swift (iOS) see a small, synchronous API: create an
//! [`Engine`], start/pause/resume downloads, list them and receive updates
//! through a [`ProgressListener`]. Everything runs on an internal tokio
//! runtime. States and texts are the same as on the desktop (`idb-types`,
//! `idb-i18n`): the app only displays them.

use std::path::PathBuf;
use std::sync::Arc;

use idb_engine::manager::Manager;
use idb_engine::{Config, Connections, Downloader, MAX_CONNECTIONS};
use idb_i18n::{Locale, describe_state};
use idb_store::Journal;
use idb_types::api::DownloadSummary;

uniffi::setup_scaffolding!();

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum MobileError {
    #[error("storage error: {msg}")]
    Storage { msg: String },
    #[error("engine error: {msg}")]
    Engine { msg: String },
}

/// One download as the app shows it.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct MobileDownload {
    pub handle: u64,
    pub url: String,
    pub filename: Option<String>,
    /// Stable state name (`Queued`, `Downloading`, `Complete`...) for icons.
    pub state: String,
    /// Human text in the engine's language ("Baixando", "Verificado (hash)").
    pub status_text: String,
    pub done: u64,
    pub total: Option<u64>,
    /// True when nothing more will happen without the user.
    pub finished: bool,
}

/// Implemented by the app (Kotlin/Swift) to receive updates.
#[uniffi::export(with_foreign)]
pub trait ProgressListener: Send + Sync {
    fn on_update(&self, download: MobileDownload);
}

fn to_mobile(locale: Locale, handle: u64, s: &DownloadSummary) -> MobileDownload {
    MobileDownload {
        handle,
        url: s.url.clone(),
        filename: s.filename.clone(),
        state: format!("{:?}", s.state.kind()),
        status_text: describe_state(locale, &s.state),
        done: s.done.0,
        total: s.total.map(|t| t.0),
        finished: s.state.is_terminal(),
    }
}

#[derive(uniffi::Object)]
pub struct Engine {
    runtime: tokio::runtime::Runtime,
    manager: Manager,
    locale: Locale,
}

#[uniffi::export]
impl Engine {
    /// `data_dir`: app-private folder for the journal. `language`: a POSIX or
    /// BCP-47 tag such as `pt-BR` or `en`.
    #[uniffi::constructor]
    pub fn new(data_dir: String, language: String) -> Result<Arc<Engine>, MobileError> {
        let dir = PathBuf::from(data_dir);
        std::fs::create_dir_all(&dir).map_err(|e| MobileError::Storage { msg: e.to_string() })?;
        let journal = Journal::open(&dir.join("journal.sqlite"))
            .map_err(|e| MobileError::Storage { msg: e.to_string() })?;
        let config = Config {
            connections: Connections::Auto {
                max: MAX_CONNECTIONS,
            },
            ..Config::default()
        };
        let downloader = Downloader::new(config, journal)
            .map_err(|e| MobileError::Engine { msg: e.to_string() })?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("idb-engine")
            .build()
            .map_err(|e| MobileError::Engine { msg: e.to_string() })?;
        let manager = Manager::new(downloader, runtime.handle().clone());
        Ok(Arc::new(Engine {
            runtime,
            manager,
            locale: Locale::from_posix(&language.replace('-', "_")),
        }))
    }

    /// Version of the native API protocol this engine speaks.
    pub fn api_version(&self) -> u32 {
        idb_types::API_VERSION
    }

    /// Starts a download into `dir` (a folder the app may write to).
    pub fn start(&self, url: String, dir: String) -> u64 {
        let _guard = self.runtime.enter();
        self.manager.start(url, PathBuf::from(dir))
    }

    pub fn pause(&self, handle: u64) -> bool {
        self.manager.pause(handle)
    }

    pub fn resume(&self, handle: u64) -> bool {
        let _guard = self.runtime.enter();
        self.manager.resume(handle)
    }

    pub fn list(&self) -> Vec<MobileDownload> {
        self.manager
            .list()
            .iter()
            .map(|(h, s)| to_mobile(self.locale, *h, s))
            .collect()
    }

    /// Receives every change, from a background thread (post to the UI thread).
    pub fn set_listener(&self, listener: Arc<dyn ProgressListener>) {
        let locale = self.locale;
        self.manager.set_listener(Some(Arc::new(move |h, s| {
            listener.on_update(to_mobile(locale, h, s));
        })));
    }

    pub fn clear_listener(&self) {
        self.manager.set_listener(None);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    struct Collect(Mutex<Vec<MobileDownload>>);
    impl ProgressListener for Collect {
        fn on_update(&self, d: MobileDownload) {
            self.0.lock().unwrap().push(d);
        }
    }

    #[test]
    fn engine_downloads_with_translated_status() {
        let server_rt = tokio::runtime::Runtime::new().unwrap();
        let addr = server_rt.block_on(faultbox::spawn()).unwrap();
        let data = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        // Loopback only: run with NO_PROXY=127.0.0.1 where a proxy is set.
        let engine = Engine::new(data.path().display().to_string(), "pt-BR".into()).unwrap();
        assert_eq!(engine.api_version(), idb_types::API_VERSION);
        let seen = Arc::new(Collect(Mutex::new(Vec::new())));
        engine.set_listener(seen.clone());
        let h = engine.start(
            format!("http://{addr}/file?size=200000&seed=41"),
            out.path().display().to_string(),
        );
        for _ in 0..200 {
            if engine.list().iter().any(|d| d.handle == h && d.finished) {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let d = engine.list().into_iter().find(|d| d.handle == h).unwrap();
        assert_eq!(d.state, "Complete", "{d:?}");
        assert_eq!(d.status_text, "Verificado (tamanho)");
        assert_eq!(d.filename.as_deref(), Some("file"));
        assert!(!seen.0.lock().unwrap().is_empty());
    }
}
