#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::time::{Duration, Instant};

use faultbox::{effective_seed, expected_sha256};
use rajada_engine::{Config, Downloader, Outcome, Request, RetryPolicy};
use rajada_store::Journal;
use rajada_types::{
    DownloadState, FailReason, HashAlgo, HashSource, NeedsUserReason, VerificationLevel,
};

fn config() -> Config {
    Config {
        no_proxy: true,
        checkpoint_bytes: 64 * 1024,
        retry: RetryPolicy {
            max_attempts: 5,
            base_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(50),
            max_retry_after: Duration::from_secs(5),
        },
        ..Config::default()
    }
}

struct Env {
    dir: tempfile::TempDir,
    addr: std::net::SocketAddr,
}

impl Env {
    async fn new() -> Env {
        Env {
            dir: tempfile::tempdir().unwrap(),
            addr: faultbox::spawn().await.unwrap(),
        }
    }

    fn url(&self, q: &str) -> String {
        format!("http://{}/file?{q}", self.addr)
    }

    fn downloader(&self, cfg: Config) -> Downloader {
        let journal = Journal::open(&self.dir.path().join("journal.sqlite")).unwrap();
        Downloader::new(cfg, journal).unwrap()
    }

    async fn get(&self, cfg: Config, url: &str, sha: Option<String>) -> Outcome {
        let req = Request {
            url: url.to_owned(),
            dir: self.dir.path().to_owned(),
            expected_sha256: sha,
        };
        self.downloader(cfg).run(req, |_| {}).await.unwrap()
    }
}

fn sha_of(path: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    let data = std::fs::read(path).unwrap();
    Sha256::digest(&data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[tokio::test]
async fn downloads_and_verifies_with_size_and_validator() {
    let env = Env::new().await;
    let out = env
        .get(config(), &env.url("size=1000003&seed=11"), None)
        .await;
    assert_eq!(
        out.state,
        DownloadState::Complete(VerificationLevel::SizeAndValidator)
    );
    let path = out.path.unwrap();
    assert_eq!(path, env.dir.path().join("file"));
    assert_eq!(sha_of(&path), expected_sha256(11, 1_000_003));
    assert!(!env.dir.path().join("file.part").exists());
}

#[tokio::test]
async fn user_hash_gives_hash_matched_or_verification_failure() {
    let env = Env::new().await;
    let url = env.url("size=50000&seed=2");
    let out = env
        .get(config(), &url, Some(expected_sha256(2, 50_000)))
        .await;
    assert_eq!(
        out.state,
        DownloadState::Complete(VerificationLevel::HashMatched {
            algo: HashAlgo::Sha256,
            source: HashSource::User
        })
    );
    let out = env.get(config(), &url, Some("00".repeat(32))).await;
    assert_eq!(
        out.state,
        DownloadState::Failed(FailReason::VerificationFailed)
    );
    // Asking for a different hash re-checked the existing file and refused it.
    assert!(!env.dir.path().join("file (1)").exists());

    let other = Env::new().await;
    let url = other.url("size=50000&seed=3");
    let out = other.get(config(), &url, Some("00".repeat(32))).await;
    assert_eq!(
        out.state,
        DownloadState::Failed(FailReason::VerificationFailed)
    );
    // The .part is kept for inspection and never gets the final name.
    assert!(out.path.unwrap().ends_with("file.part"));
    assert!(!other.dir.path().join("file").exists());
}

#[tokio::test]
async fn existing_file_is_not_overwritten() {
    let env = Env::new().await;
    std::fs::write(env.dir.path().join("file"), b"mine").unwrap();
    let out = env.get(config(), &env.url("size=100&seed=1"), None).await;
    assert_eq!(out.path.unwrap(), env.dir.path().join("file (1)"));
    assert_eq!(std::fs::read(env.dir.path().join("file")).unwrap(), b"mine");
}

#[tokio::test]
async fn dropped_connections_resume_with_correct_hash() {
    let env = Env::new().await;
    let out = env
        .get(
            config(),
            &env.url("size=1000000&seed=3&drop_conn=150000"),
            None,
        )
        .await;
    assert!(
        matches!(out.state, DownloadState::Complete(_)),
        "{:?}",
        out.state
    );
    assert_eq!(sha_of(&out.path.unwrap()), expected_sha256(3, 1_000_000));
}

#[tokio::test]
async fn etag_change_mid_download_needs_user_and_never_mixes() {
    let env = Env::new().await;
    let url = env.url("size=400000&seed=4&drop_conn=250000&etag_change_at=50");
    let out = env.get(config(), &url, None).await;
    assert_eq!(
        out.state,
        DownloadState::NeedsUser(NeedsUserReason::ResourceChanged)
    );
    assert!(!env.dir.path().join("file").exists());
    // Re-running means "start over" and yields the new version intact.
    let url = env.url("size=400000&seed=4&etag_change_at=50");
    let out = env.get(config(), &url, None).await;
    assert!(matches!(out.state, DownloadState::Complete(_)));
    let got = sha_of(&out.path.unwrap());
    // Each server instance has its own version counter: the fresh run starts at v0
    // on this server (already bumped to v1 by the first run).
    assert_eq!(got, expected_sha256(effective_seed(4, 1), 400_000));
}

#[tokio::test]
async fn honours_retry_after() {
    let env = Env::new().await;
    let t = Instant::now();
    let out = env
        .get(
            config(),
            &env.url("size=1000&fail_first=1&status=503&retry_after=1"),
            None,
        )
        .await;
    assert!(matches!(out.state, DownloadState::Complete(_)));
    assert!(t.elapsed() >= Duration::from_secs(1));
}

#[tokio::test]
async fn polite_wait_is_reported() {
    let env = Env::new().await;
    let req = Request {
        url: env.url("size=10&fail_first=1&status=429&retry_after=1"),
        dir: env.dir.path().to_owned(),
        expected_sha256: None,
    };
    let mut saw_wait = false;
    env.downloader(config())
        .run(req, |p| {
            saw_wait |= matches!(p.state, DownloadState::PoliteWait { .. })
        })
        .await
        .unwrap();
    assert!(saw_wait);
}

#[tokio::test]
async fn html_page_needs_user() {
    let env = Env::new().await;
    let out = env.get(config(), &env.url("size=10&html"), None).await;
    assert_eq!(
        out.state,
        DownloadState::NeedsUser(NeedsUserReason::HtmlInsteadOfFile)
    );
}

#[tokio::test]
async fn not_found_fails_with_recovery() {
    let env = Env::new().await;
    let url = format!("http://{}/missing", env.addr);
    let out = env.get(config(), &url, None).await;
    assert_eq!(out.state, DownloadState::Failed(FailReason::NotFound));
    assert!(out.state.recovery().is_some());
}

#[tokio::test]
async fn gives_up_when_no_progress_is_possible() {
    let env = Env::new().await;
    // Ranges ignored and every response drops: we can never get past 1000 bytes.
    let out = env
        .get(
            config(),
            &env.url("size=5000&no_ranges&drop_conn=1000"),
            None,
        )
        .await;
    assert_eq!(
        out.state,
        DownloadState::Failed(FailReason::NetworkUnreachable)
    );
}

#[tokio::test]
async fn resumes_from_journal_in_a_new_run() {
    let env = Env::new().await;
    let mut cfg = config();
    cfg.retry.max_attempts = 0;
    let out = env
        .get(cfg, &env.url("size=600000&seed=8&drop_conn=200000"), None)
        .await;
    assert_eq!(
        out.state,
        DownloadState::Failed(FailReason::NetworkUnreachable)
    );

    let j = Journal::open(&env.dir.path().join("journal.sqlite")).unwrap();
    let url = env.url("size=600000&seed=8&drop_conn=200000");
    let rec = j
        .find(&url, &env.dir.path().to_string_lossy())
        .unwrap()
        .unwrap();
    assert_eq!(rec.downloaded, 200_000);
    drop(j);

    // A transient failure is resumable: the next run continues at 200000.
    let req = Request {
        url: url.clone(),
        dir: env.dir.path().to_owned(),
        expected_sha256: None,
    };
    let mut first_done = None;
    let out = env
        .downloader(config())
        .run(req, |p| {
            first_done.get_or_insert(p.done);
        })
        .await
        .unwrap();
    assert_eq!(first_done, Some(200_000));
    assert!(matches!(out.state, DownloadState::Complete(_)));
    assert_eq!(sha_of(&out.path.unwrap()), expected_sha256(8, 600_000));
}

#[tokio::test]
async fn crash_after_rename_does_not_download_again() {
    let env = Env::new().await;
    let url = env.url("size=5000&seed=6");
    let out = env.get(config(), &url, None).await;
    let path = out.path.unwrap();
    // Simulate a crash between the rename and recording Complete.
    let j = Journal::open(&env.dir.path().join("journal.sqlite")).unwrap();
    j.set_state(out.id, &DownloadState::Verifying { permille: 0 })
        .unwrap();
    drop(j);
    let out = env.get(config(), &url, None).await;
    assert_eq!(
        out.state,
        DownloadState::Complete(VerificationLevel::SizeAndValidator)
    );
    assert_eq!(out.path.unwrap(), path);
    assert!(!env.dir.path().join("file (1)").exists());
}

#[tokio::test]
async fn get_is_idempotent_while_the_file_exists() {
    let env = Env::new().await;
    let url = env.url("size=3000&seed=12");
    let first = env.get(config(), &url, None).await.path.unwrap();
    let again = env.get(config(), &url, None).await;
    assert!(matches!(again.state, DownloadState::Complete(_)));
    assert_eq!(again.path.unwrap(), first);
    // Once the user deletes it, the same command downloads it again.
    std::fs::remove_file(&first).unwrap();
    let third = env.get(config(), &url, None).await;
    assert_eq!(third.path.unwrap(), first);
    assert_eq!(sha_of(&first), expected_sha256(12, 3000));
}
