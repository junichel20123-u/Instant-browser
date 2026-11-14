#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::time::{Duration, Instant};

use faultbox::{effective_seed, expected_sha256};
use idb_engine::{Config, Connections, Downloader, Outcome, Request, RetryPolicy};
use idb_store::Journal;
use idb_types::{
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

// ---- Segmented (parallel) downloads ----

fn seg_config(connections: usize) -> Config {
    Config {
        connections: Connections::Fixed(connections),
        min_segment: 64 * 1024,
        checkpoint_interval: Duration::from_millis(50),
        ..config()
    }
}

fn segment_count(env: &Env, url: &str) -> usize {
    let j = Journal::open(&env.dir.path().join("journal.sqlite")).unwrap();
    let rec = j
        .find(url, &env.dir.path().to_string_lossy())
        .unwrap()
        .unwrap();
    j.segments(rec.id).unwrap().len()
}

#[tokio::test]
async fn segmented_download_has_exact_hash() {
    let env = Env::new().await;
    let url = env.url("size=1000003&seed=21");
    let out = env.get(seg_config(4), &url, None).await;
    assert_eq!(
        out.state,
        DownloadState::Complete(VerificationLevel::SizeAndValidator)
    );
    assert_eq!(sha_of(&out.path.unwrap()), expected_sha256(21, 1_000_003));
    // 4 initial segments; work stealing may split them further.
    assert!(segment_count(&env, &url) >= 4);
    assert!(out.diagnostics.is_empty());
}

#[tokio::test]
async fn parallel_connections_beat_a_per_connection_limit() {
    let env = Env::new().await;
    let time = |c: usize, seed: u32| {
        let url = env.url(&format!("size=2000000&seed={seed}&per_conn_rate=1000000"));
        let env = &env;
        async move {
            let t = Instant::now();
            let out = env.get(seg_config(c), &url, None).await;
            assert!(matches!(out.state, DownloadState::Complete(_)));
            t.elapsed().as_secs_f64()
        }
    };
    let one = time(1, 1).await;
    let four = time(4, 2).await;
    assert!(one / four > 2.5, "1 conn {one:.2}s vs 4 conns {four:.2}s");
}

#[tokio::test]
async fn unannounced_range_support_is_detected_by_second_probe() {
    let env = Env::new().await;
    let url = env.url("size=800000&seed=22&hide_accept_ranges");
    let out = env.get(seg_config(4), &url, None).await;
    assert_eq!(sha_of(&out.path.unwrap()), expected_sha256(22, 800_000));
    // 4 initial segments; work stealing may split them further.
    assert!(segment_count(&env, &url) >= 4);
}

#[tokio::test]
async fn no_range_support_falls_back_to_one_connection_with_diagnostic() {
    let env = Env::new().await;
    let url = env.url("size=800000&seed=23&no_ranges");
    let out = env.get(seg_config(4), &url, None).await;
    assert!(matches!(out.state, DownloadState::Complete(_)));
    assert_eq!(sha_of(&out.path.unwrap()), expected_sha256(23, 800_000));
    assert_eq!(segment_count(&env, &url), 0);
    assert_eq!(
        out.diagnostics.iter().map(|d| &d.code).collect::<Vec<_>>(),
        vec![&idb_types::DiagnosticCode::RangeNotSupported]
    );
}

#[tokio::test]
async fn segments_survive_dropped_connections() {
    let env = Env::new().await;
    let url = env.url("size=1000000&seed=24&drop_conn=70000");
    let out = env.get(seg_config(4), &url, None).await;
    assert!(
        matches!(out.state, DownloadState::Complete(_)),
        "{:?}",
        out.state
    );
    assert_eq!(sha_of(&out.path.unwrap()), expected_sha256(24, 1_000_000));
}

#[tokio::test]
async fn segmented_resource_change_never_mixes_versions() {
    let env = Env::new().await;
    // Connections drop, so segments must ask again after the file changed:
    // If-Range then gets a 200 and the download stops instead of mixing.
    let url = env.url("size=1000000&seed=25&etag_change_at=30&drop_conn=100000");
    let out = env.get(seg_config(4), &url, None).await;
    assert_eq!(
        out.state,
        DownloadState::NeedsUser(NeedsUserReason::ResourceChanged)
    );
    assert!(!env.dir.path().join("file").exists());

    // Without interruptions every segment streams the old version to the end:
    // the result is complete and must be exactly the old version.
    let env = Env::new().await;
    let url = env.url("size=1000000&seed=29&etag_change_at=30");
    let out = env.get(seg_config(4), &url, None).await;
    if matches!(out.state, DownloadState::Complete(_)) {
        assert_eq!(sha_of(&out.path.unwrap()), expected_sha256(29, 1_000_000));
    }
}

#[tokio::test]
async fn segmented_backs_off_on_429_and_completes() {
    let env = Env::new().await;
    let url = env.url("size=800000&seed=26&429_above=2&retry_after=1&per_conn_rate=2000000");
    let out = env.get(seg_config(4), &url, None).await;
    assert!(
        matches!(out.state, DownloadState::Complete(_)),
        "{:?}",
        out.state
    );
    assert_eq!(sha_of(&out.path.unwrap()), expected_sha256(26, 800_000));
}

#[tokio::test]
async fn disk_full_is_detected_before_writing() {
    let env = Env::new().await;
    let mut cfg = seg_config(4);
    cfg.min_free_bytes = u64::MAX / 2;
    let out = env.get(cfg, &env.url("size=800000&seed=27"), None).await;
    assert_eq!(
        out.state,
        DownloadState::NeedsUser(NeedsUserReason::DiskFull)
    );
    assert!(!env.dir.path().join("file.part").exists());
}

#[tokio::test]
async fn segmented_download_resumes_per_segment() {
    let env = Env::new().await;
    let url = env.url("size=1000000&seed=28&drop_conn=100000");
    let mut cfg = seg_config(4);
    cfg.retry.max_attempts = 0;
    let out = env.get(cfg, &url, None).await;
    assert_eq!(
        out.state,
        DownloadState::Failed(FailReason::NetworkUnreachable)
    );
    let req = Request {
        url: url.clone(),
        dir: env.dir.path().to_owned(),
        expected_sha256: None,
    };
    let mut first_done = None;
    let out = env
        .downloader(seg_config(4))
        .run(req, |p| {
            first_done.get_or_insert(p.done);
        })
        .await
        .unwrap();
    assert!(first_done.unwrap() >= 100_000, "{first_done:?}");
    assert!(
        matches!(out.state, DownloadState::Complete(_)),
        "{:?}",
        out.state
    );
    assert_eq!(sha_of(&out.path.unwrap()), expected_sha256(28, 1_000_000));
}

// ---- Adaptive controller, watchdog (phase 1.3) ----

fn auto_config() -> Config {
    Config {
        connections: Connections::Auto { max: 8 },
        ..config()
    }
}

fn has_diag(out: &Outcome, f: impl Fn(&idb_types::DiagnosticCode) -> bool) -> bool {
    out.diagnostics.iter().any(|d| f(&d.code))
}

#[tokio::test]
async fn adaptive_finds_eight_connections_within_three_seconds() {
    let env = Env::new().await;
    // 2 MiB/s per connection, 16 MiB/s in total (spec §7.6 target scenario).
    let size = 64u64 << 20;
    let url = env.url(&format!(
        "size={size}&seed=31&per_conn_rate=2097152&total_rate=16777216"
    ));
    let out = env.get(auto_config(), &url, None).await;
    assert!(
        matches!(out.state, DownloadState::Complete(_)),
        "{:?}",
        out.state
    );
    assert!((7..=8).contains(&out.stats.connections), "{:?}", out.stats);
    let ramp = out.stats.ramp_ms.unwrap();
    assert!(ramp <= 3000, "ramp {ramp} ms; {:?}", out.diagnostics);
    assert!(has_diag(&out, |c| matches!(
        c,
        idb_types::DiagnosticCode::ServerPerConnectionLimit { .. }
    )));
    assert_eq!(sha_of(&out.path.unwrap()), expected_sha256(31, size));
}

#[tokio::test]
async fn adaptive_stays_low_when_the_link_is_the_bottleneck() {
    let env = Env::new().await;
    // No per-connection limit: only the total is capped (a saturated link).
    let size = 16u64 << 20;
    let url = env.url(&format!("size={size}&seed=32&total_rate=8388608"));
    let out = env.get(auto_config(), &url, None).await;
    assert!(matches!(out.state, DownloadState::Complete(_)));
    assert!(out.stats.connections <= 2, "{:?}", out.stats);
    assert!(has_diag(&out, |c| matches!(
        c,
        idb_types::DiagnosticCode::LinkSaturated { .. }
    )));
    assert_eq!(sha_of(&out.path.unwrap()), expected_sha256(32, size));
}

#[tokio::test]
async fn adaptive_halves_on_429_and_completes() {
    let env = Env::new().await;
    let size = 16u64 << 20;
    let url = env.url(&format!(
        "size={size}&seed=33&per_conn_rate=2097152&429_above=2&retry_after=1"
    ));
    let out = env.get(auto_config(), &url, None).await;
    assert!(
        matches!(out.state, DownloadState::Complete(_)),
        "{:?}",
        out.state
    );
    assert!(has_diag(&out, |c| matches!(
        c,
        idb_types::DiagnosticCode::ServerAskedToWait { .. }
    )));
    assert!(out.stats.connections <= 2, "{:?}", out.stats);
    assert_eq!(sha_of(&out.path.unwrap()), expected_sha256(33, size));
}

#[tokio::test]
async fn watchdog_replaces_frozen_connections() {
    let env = Env::new().await;
    let mut cfg = seg_config(4);
    cfg.stall_timeout = Duration::from_millis(800);
    // Every response freezes for 60 s (no RST) after 300 kB.
    let url = env.url("size=2000000&seed=34&stall_conn=300000&stall_ms=60000");
    let t = Instant::now();
    let out = env.get(cfg, &url, None).await;
    assert!(
        matches!(out.state, DownloadState::Complete(_)),
        "{:?}",
        out.state
    );
    assert!(t.elapsed() < Duration::from_secs(20), "{:?}", t.elapsed());
    assert!(has_diag(&out, |c| matches!(
        c,
        idb_types::DiagnosticCode::StalledConnectionReassigned
    )));
    assert_eq!(sha_of(&out.path.unwrap()), expected_sha256(34, 2_000_000));
}
