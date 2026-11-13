mod common;

use common::{get, header, sha256};
use faultbox::{body, etag, expected_sha256};

#[tokio::test]
async fn full_file_matches_known_hash() {
    let addr = faultbox::spawn().await.unwrap();
    let r = get(addr, "/file?size=1000003&seed=42", &[]).await;
    assert_eq!(r.status, 200);
    assert_eq!(header(&r, "accept-ranges"), "bytes");
    assert_eq!(header(&r, "content-length"), "1000003");
    assert_eq!(sha256(&r.body.unwrap()), expected_sha256(42, 1_000_003));
}

#[tokio::test]
async fn ranges_reassemble_to_the_same_hash() {
    let addr = faultbox::spawn().await.unwrap();
    let size = 300_000u64;
    let mut all = Vec::new();
    for (s, e) in [(0, 99_999), (100_000, 199_999), (200_000, 299_999)] {
        let range = format!("bytes={s}-{e}");
        let r = get(addr, "/file?size=300000&seed=7", &[("range", &range)]).await;
        assert_eq!(r.status, 206);
        assert_eq!(header(&r, "content-range"), format!("bytes {s}-{e}/{size}"));
        all.extend(r.body.unwrap());
    }
    assert_eq!(sha256(&all), expected_sha256(7, size));
}

#[tokio::test]
async fn suffix_and_unsatisfiable_ranges() {
    let addr = faultbox::spawn().await.unwrap();
    let r = get(addr, "/file?size=100&seed=1", &[("range", "bytes=-10")]).await;
    assert_eq!(r.status, 206);
    assert_eq!(r.body.unwrap(), body::range(1, 90, 100));
    let r = get(addr, "/file?size=100&seed=1", &[("range", "bytes=100-")]).await;
    assert_eq!(r.status, 416);
    assert_eq!(header(&r, "content-range"), "bytes */100");
}

#[tokio::test]
async fn if_range_with_stale_etag_returns_full_200() {
    let addr = faultbox::spawn().await.unwrap();
    let path = "/file?size=1000&seed=3";
    let current = etag(3, 1000, 0);
    let r = get(
        addr,
        path,
        &[("range", "bytes=10-19"), ("if-range", &current)],
    )
    .await;
    assert_eq!(r.status, 206);
    assert_eq!(header(&r, "etag"), current);
    assert!(!header(&r, "last-modified").is_empty());
    let r = get(
        addr,
        path,
        &[("range", "bytes=10-19"), ("if-range", "\"stale\"")],
    )
    .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.body.unwrap().len(), 1000);
}
