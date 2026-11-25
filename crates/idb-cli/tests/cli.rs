//! End-to-end CLI contract: exit codes and the `--json` shape.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::path::Path;
use std::process::{Command, Output};

fn idb(args: &[&str], dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_idb"))
        .args(args)
        .env("NO_PROXY", "127.0.0.1")
        .env("LANG", "pt_BR.UTF-8")
        .env_remove("HTTP_PROXY")
        .env_remove("http_proxy")
        .env_remove("ALL_PROXY")
        .current_dir(dir)
        .output()
        .unwrap()
}

fn server() -> (tokio::runtime::Runtime, std::net::SocketAddr) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let addr = rt.block_on(faultbox::spawn()).unwrap();
    (rt, addr)
}

fn get(url: &str, extra: &[&str], dir: &Path) -> Output {
    let d = dir.to_str().unwrap();
    let state = dir.join(".s");
    let mut args = vec![
        "get",
        url,
        "-d",
        d,
        "--state-dir",
        state.to_str().unwrap(),
        "--json",
        "-q",
    ];
    args.extend_from_slice(extra);
    idb(&args, dir)
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn complete_download_exits_0_with_stable_json() {
    let (_rt, addr) = server();
    let dir = tempfile::tempdir().unwrap();
    let sha = faultbox::expected_sha256(5, 300_000);
    let out = get(
        &format!("http://{addr}/file?size=300000&seed=5"),
        &["--sha256", &sha],
        dir.path(),
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json(&out);
    for key in [
        "id",
        "state",
        "path",
        "connections",
        "ramp_ms",
        "peak_rate",
        "diagnostics",
    ] {
        assert!(v.get(key).is_some(), "missing {key}: {v}");
    }
    assert_eq!(v["state"]["state"], "Complete");
    assert_eq!(v["state"]["data"]["level"], "HashMatched");
}

#[test]
fn html_needs_user_exits_2_with_translated_text() {
    let (_rt, addr) = server();
    let dir = tempfile::tempdir().unwrap();
    let out = get(&format!("http://{addr}/file?size=10&html"), &[], dir.path());
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("Este link abre uma página"), "{err}");
}

#[test]
fn not_found_exits_1() {
    let (_rt, addr) = server();
    let dir = tempfile::tempdir().unwrap();
    let out = get(&format!("http://{addr}/missing"), &[], dir.path());
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn bad_arguments_exit_64_before_any_network() {
    let dir = tempfile::tempdir().unwrap();
    let url = "http://127.0.0.1:9/never";
    for extra in [
        &["--sha256", "abc"][..],
        &["--limit", "fast"][..],
        &["-c", "12"][..],
    ] {
        let out = get(url, extra, dir.path());
        assert_eq!(out.status.code(), Some(64), "{extra:?}");
    }
}

#[test]
fn doctor_reports_json_and_exit_codes() {
    let (_rt, addr) = server();
    let dir = tempfile::tempdir().unwrap();
    let url = format!("http://{addr}/file?size=268435456&total_rate=8388608");
    let out = idb(&["doctor", &url, "--json", "--seconds", "4"], dir.path());
    assert_eq!(out.status.code(), Some(0));
    let v = json(&out);
    assert_eq!(v["ranges"], true);
    let codes: Vec<String> = v["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"]["code"].as_str().unwrap_or("").to_owned())
        .collect();
    assert!(codes.contains(&"LinkSaturated".to_owned()), "{v}");
    // Doctor never writes files.
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);

    let html = format!("http://{addr}/file?size=10&html");
    let out = idb(&["doctor", &html], dir.path());
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stdout).contains("Este link abre uma página"));
}

#[test]
fn devices_list_and_revoke() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join(".s");
    let s = state.to_str().unwrap();
    let out = idb(&["devices", "list", "--state-dir", s], dir.path());
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("Nenhum celular pareado"));
    // Pair one device through the engine API, then manage it from the CLI.
    let j = idb_store::Journal::open(&state.join("journal.sqlite")).unwrap();
    let offer = idb_engine::pairing::new_offer(&j, "h", 1, &"0".repeat(64), 0).unwrap();
    let (id, _token) = idb_engine::pairing::redeem(&j, &offer.code, "Pixel", 1).unwrap();
    drop(j);
    let out = idb(&["devices", "list", "--state-dir", s], dir.path());
    assert!(String::from_utf8_lossy(&out.stdout).contains("Pixel"));
    let id = id.to_string();
    let out = idb(&["devices", "revoke", &id, "--state-dir", s], dir.path());
    assert_eq!(out.status.code(), Some(0));
    let out = idb(&["devices", "revoke", "999", "--state-dir", s], dir.path());
    assert_eq!(out.status.code(), Some(64));
}
