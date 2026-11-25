#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::time::Duration;

use idb_engine::{Config, Connections, DoctorReport, Downloader};
use idb_store::Journal;
use idb_types::{DiagnosticCode, DownloadState, NeedsUserReason};

async fn doctor(q: &str) -> DoctorReport {
    let addr = faultbox::spawn().await.unwrap();
    let cfg = Config {
        no_proxy: true,
        connections: Connections::Auto { max: 8 },
        ..Config::default()
    };
    let d = Downloader::new(cfg, Journal::open_in_memory().unwrap()).unwrap();
    d.doctor(&format!("http://{addr}/file?{q}"), Duration::from_secs(5))
        .await
}

fn has(r: &DoctorReport, f: impl Fn(&DiagnosticCode) -> bool) -> bool {
    r.diagnostics.iter().any(|d| f(&d.code))
}

#[tokio::test]
async fn per_connection_limit_is_explained() {
    let r = doctor("size=268435456&per_conn_rate=2097152&total_rate=16777216").await;
    assert_eq!(r.ranges, Some(true));
    assert!(r.validator);
    assert!(r.recommended_connections >= 7, "{r:?}");
    assert!(
        has(&r, |c| matches!(
            c,
            DiagnosticCode::ServerPerConnectionLimit { .. }
        )),
        "{r:?}"
    );
    assert!(r.blocking.is_none());
}

#[tokio::test]
async fn saturated_link_is_explained() {
    let r = doctor("size=268435456&total_rate=8388608").await;
    assert!(r.recommended_connections <= 2, "{r:?}");
    assert!(
        has(&r, |c| matches!(c, DiagnosticCode::LinkSaturated { .. })),
        "{r:?}"
    );
}

#[tokio::test]
async fn no_range_support_is_explained() {
    let r = doctor("size=8000000&no_ranges&per_conn_rate=4000000").await;
    assert_eq!(r.ranges, Some(false));
    assert!(has(&r, |c| matches!(c, DiagnosticCode::RangeNotSupported)));
    assert_eq!(r.recommended_connections, 1);
}

#[tokio::test]
async fn html_page_is_blocking() {
    let r = doctor("size=10&html").await;
    assert_eq!(
        r.blocking,
        Some(DownloadState::NeedsUser(NeedsUserReason::HtmlInsteadOfFile))
    );
    assert!(has(&r, |c| matches!(
        c,
        DiagnosticCode::HtmlInsteadOfFile { .. }
    )));
}

#[tokio::test]
async fn rate_limited_server_is_explained() {
    let r = doctor("size=268435456&per_conn_rate=2097152&429_above=2&retry_after=7").await;
    assert!(
        has(&r, |c| matches!(
            c,
            DiagnosticCode::ServerAskedToWait { secs: 7, .. }
        )),
        "{r:?}"
    );
}
