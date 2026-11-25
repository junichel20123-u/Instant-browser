#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use idb_engine::manager::Manager;
use idb_engine::{Config, Connections, Downloader};
use idb_store::Journal;
use idb_types::DownloadState;

fn sha_of(path: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(std::fs::read(path).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

async fn wait_for(m: &Manager, h: u64, f: impl Fn(&DownloadState) -> bool) {
    for _ in 0..400 {
        if m.list().iter().any(|(id, s)| *id == h && f(&s.state)) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timeout: {:?}", m.list());
}

#[tokio::test(flavor = "multi_thread")]
async fn start_pause_resume_and_list() {
    let addr = faultbox::spawn().await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let journal = Journal::open(&dir.path().join("j.sqlite")).unwrap();
    let cfg = Config {
        no_proxy: true,
        connections: Connections::Auto { max: 4 },
        checkpoint_interval: Duration::from_millis(50),
        ..Config::default()
    };
    let m = Manager::new(
        Downloader::new(cfg, journal).unwrap(),
        tokio::runtime::Handle::current(),
    );
    let events = Arc::new(Mutex::new(0u32));
    let ev = events.clone();
    m.set_listener(Some(Arc::new(move |_, _| *ev.lock().unwrap() += 1)));

    let size = 3_000_000u64;
    let url = format!("http://{addr}/file?size={size}&seed=40&per_conn_rate=2000000");
    let h = m.start(url, dir.path().to_owned());
    wait_for(&m, h, |s| matches!(s, DownloadState::Downloading)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(m.pause(h));
    assert_eq!(m.list()[0].1.state, DownloadState::Paused);
    let paused_at = m.list()[0].1.done.0;
    assert!(m.resume(h));
    wait_for(&m, h, |s| matches!(s, DownloadState::Complete(_))).await;

    let (_, s) = &m.list()[0];
    assert_eq!(s.filename.as_deref(), Some("file"));
    assert_eq!(s.done.0, size);
    assert!(paused_at > 0);
    assert_eq!(
        sha_of(&dir.path().join("file")),
        faultbox::expected_sha256(40, size)
    );
    assert!(*events.lock().unwrap() > 2);
    // A finished download cannot be paused or resumed.
    assert!(!m.pause(h));
    assert!(!m.resume(h));
}
