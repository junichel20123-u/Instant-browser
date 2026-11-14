mod common;

use std::time::{Duration, Instant};

use common::{get, header, sha256};
use faultbox::{effective_seed, etag, expected_sha256};

#[tokio::test]
async fn no_ranges_ignores_range_header() {
    let addr = faultbox::spawn().await.unwrap();
    let r = get(
        addr,
        "/file?size=5000&seed=9&no_ranges",
        &[("range", "bytes=0-9")],
    )
    .await;
    assert_eq!(r.status, 200);
    assert_eq!(header(&r, "accept-ranges"), "");
    assert_eq!(sha256(&r.body.unwrap()), expected_sha256(9, 5000));
}

#[tokio::test]
async fn per_conn_rate_limits_throughput() {
    let addr = faultbox::spawn().await.unwrap();
    let rate = 4u64 << 20; // 4 MiB/s
    let size = 4u64 << 20; // ~1 s
    let t = Instant::now();
    let r = get(
        addr,
        &format!("/file?size={size}&per_conn_rate={rate}"),
        &[],
    )
    .await;
    let secs = t.elapsed().as_secs_f64();
    assert_eq!(r.body.unwrap().len() as u64, size);
    let measured = size as f64 / secs;
    assert!(
        (0.8..=1.2).contains(&(measured / rate as f64)),
        "measured {measured:.0} B/s vs limit {rate}"
    );
}

#[tokio::test]
async fn latency_delays_response() {
    let addr = faultbox::spawn().await.unwrap();
    let t = Instant::now();
    let r = get(addr, "/file?size=10&latency_ms=200", &[]).await;
    assert_eq!(r.status, 200);
    assert!(t.elapsed() >= Duration::from_millis(200));
}

#[tokio::test]
async fn drop_conn_cuts_body_after_n_bytes() {
    let addr = faultbox::spawn().await.unwrap();
    let r = get(addr, "/file?size=1000000&drop_conn=300000", &[]).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.body, Err(300_000));
}

#[tokio::test]
async fn too_many_connections_get_429_with_retry_after() {
    let addr = faultbox::spawn().await.unwrap();
    // Two slow responses stay in flight while a third arrives.
    let path = "/file?size=200000&per_conn_rate=200000&429_above=2&retry_after=30";
    let slow = [
        tokio::spawn(get(addr, path, &[])),
        tokio::spawn(get(addr, path, &[])),
    ];
    tokio::time::sleep(Duration::from_millis(200)).await;
    let r = get(addr, path, &[]).await;
    assert_eq!(r.status, 429);
    assert_eq!(header(&r, "retry-after"), "30");
    for h in slow {
        assert_eq!(h.await.unwrap().status, 200);
    }
    // Once they finish, requests are accepted again.
    assert_eq!(get(addr, path, &[]).await.status, 200);
}

#[tokio::test]
async fn etag_changes_mid_download() {
    let addr = faultbox::spawn().await.unwrap();
    let path = "/file?size=100000&seed=5&etag_change_at=50";
    let old = etag(5, 100_000, 0);
    let first = get(addr, path, &[("range", "bytes=0-59999")]).await;
    assert_eq!(header(&first, "etag"), old);
    // Resuming with the old validator must not mix versions.
    let resume = get(addr, path, &[("range", "bytes=60000-"), ("if-range", &old)]).await;
    assert_eq!(resume.status, 200);
    assert_eq!(header(&resume, "etag"), etag(5, 100_000, 1));
    assert_eq!(
        sha256(&resume.body.unwrap()),
        expected_sha256(effective_seed(5, 1), 100_000)
    );
}

#[tokio::test]
async fn fail_first_then_serves() {
    let addr = faultbox::spawn().await.unwrap();
    let path = "/file?size=100&fail_first=2&status=503&retry_after=7";
    for _ in 0..2 {
        let r = get(addr, path, &[]).await;
        assert_eq!(r.status, 503);
        assert_eq!(header(&r, "retry-after"), "7");
    }
    assert_eq!(get(addr, path, &[]).await.status, 200);
}

#[tokio::test]
async fn html_interstitial() {
    let addr = faultbox::spawn().await.unwrap();
    let r = get(addr, "/file?size=100&html", &[]).await;
    assert!(header(&r, "content-type").starts_with("text/html"));
    assert!(r.body.unwrap().starts_with(b"<!DOCTYPE html"));
}

#[tokio::test]
async fn hide_accept_ranges_still_serves_ranges() {
    let addr = faultbox::spawn().await.unwrap();
    let path = "/file?size=1000&seed=4&hide_accept_ranges";
    let r = get(addr, path, &[]).await;
    assert_eq!(header(&r, "accept-ranges"), "");
    let r = get(addr, path, &[("range", "bytes=500-500")]).await;
    assert_eq!(r.status, 206);
    assert_eq!(header(&r, "content-range"), "bytes 500-500/1000");
    assert_eq!(r.body.unwrap(), faultbox::body::range(4, 500, 501));
}

#[tokio::test]
async fn total_rate_is_shared_between_connections() {
    let addr = faultbox::spawn().await.unwrap();
    // 4 connections, each capped at 4 MB/s, all together at 4 MB/s:
    // 4 x 1 MB must take about 1 s, not 0.25 s.
    let path = "/file?size=1000000&per_conn_rate=4000000&total_rate=4000000";
    let t = Instant::now();
    let tasks: Vec<_> = (0..4).map(|_| tokio::spawn(get(addr, path, &[]))).collect();
    for t in tasks {
        assert_eq!(t.await.unwrap().body.unwrap().len(), 1_000_000);
    }
    let secs = t.elapsed().as_secs_f64();
    assert!((0.8..1.4).contains(&secs), "took {secs:.2}s");
}

#[tokio::test]
async fn stall_conn_freezes_without_closing() {
    let addr = faultbox::spawn().await.unwrap();
    let t = Instant::now();
    let r = get(addr, "/file?size=100000&seed=1&stall_conn=50000&stall_ms=400", &[]).await;
    assert!(t.elapsed() >= Duration::from_millis(400));
    assert_eq!(sha256(&r.body.unwrap()), expected_sha256(1, 100_000));
}
