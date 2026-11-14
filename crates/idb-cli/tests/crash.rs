//! Crash injection: SIGKILL `idb get` at random moments, rerun until it
//! completes, and require the final file to have the exact expected hash.
//! Iterations: `IDB_CRASH_ITERS` (default 25; acceptance run uses 1000).

#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use sha2::{Digest, Sha256};

fn idb(dir: &Path, url: &str) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_idb"));
    c.args(["get", url, "-d"])
        .arg(dir)
        .arg("--state-dir")
        .arg(dir.join(".state"))
        .args([
            "--checkpoint-ms",
            "30",
            "-c",
            "4",
            "--min-segment-kib",
            "256",
        ])
        .env("NO_PROXY", "127.0.0.1")
        .env_remove("HTTP_PROXY")
        .env_remove("http_proxy")
        .env_remove("ALL_PROXY")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    c
}

/// Tiny xorshift so the test needs no RNG crate; seed is printed on failure.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[test]
fn sigkill_never_yields_a_wrong_file() {
    let iters: u64 = std::env::var("IDB_CRASH_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(25);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let addr = rt.block_on(faultbox::spawn()).unwrap();
    let seed0 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
        | 1;
    let mut rng = Rng(seed0);

    let (mut kills, mut files) = (0u64, 0u64);
    while kills < iters {
        let dir = tempfile::tempdir().unwrap();
        let (size, seed) = (1_500_000 + rng.next() % 500_000, rng.next() % 1000);
        // ~0.3 s per full download at 6 MB/s with a checkpoint every 30 ms:
        // kills land before, between and after checkpoints.
        let url = format!("http://{addr}/file?size={size}&seed={seed}&per_conn_rate=6000000");
        loop {
            let mut child = idb(dir.path(), &url).spawn().unwrap();
            let wait = Duration::from_millis(rng.next() % 250);
            std::thread::sleep(wait);
            match child.try_wait().unwrap() {
                Some(status) => {
                    assert!(status.success(), "seed0={seed0}: exit {status:?}");
                    break;
                }
                None => {
                    child.kill().unwrap(); // SIGKILL
                    child.wait().unwrap();
                    kills += 1;
                }
            }
        }
        // Exactly one final file, with the exact hash; no stray copies.
        let finals: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with('.') && !n.ends_with(".part"))
            .collect();
        assert_eq!(finals, vec!["file".to_owned()], "seed0={seed0}");
        let data = std::fs::read(dir.path().join("file")).unwrap();
        let got: String = Sha256::digest(&data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(got, faultbox::expected_sha256(seed, size), "seed0={seed0}");
        files += 1;
    }
    eprintln!("crash injection: {kills} kills over {files} downloads, all hashes correct");
}
