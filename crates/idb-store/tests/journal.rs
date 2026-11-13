use idb_store::{Journal, ResourceMeta};
use idb_types::{DownloadState, NeedsUserReason};

#[test]
fn round_trip_and_replace() {
    let j = Journal::open_in_memory().unwrap();
    let id = j.create("http://x/f", "/tmp").unwrap();
    let rec = j.find("http://x/f", "/tmp").unwrap().unwrap();
    assert_eq!((rec.id, rec.downloaded, rec.meta), (id, 0, None));
    assert_eq!(rec.state, DownloadState::Probing);

    let meta = ResourceMeta {
        final_url: "http://y/f".into(),
        filename: "f.iso".into(),
        total: Some(10),
        etag: Some("\"a\"".into()),
        last_modified: None,
    };
    j.set_meta(id, &meta).unwrap();
    j.set_downloaded(id, 7).unwrap();
    let st = DownloadState::NeedsUser(NeedsUserReason::ResourceChanged);
    j.set_state(id, &st).unwrap();
    let rec = j.get(id).unwrap().unwrap();
    assert_eq!((rec.downloaded, rec.meta, rec.state), (7, Some(meta), st));

    let id2 = j.create("http://x/f", "/tmp").unwrap();
    assert_ne!(id, id2);
    assert!(j.get(id).unwrap().is_none());
    assert_eq!(j.get(id2).unwrap().unwrap().downloaded, 0);
}

#[test]
fn persists_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("j.sqlite");
    let id = {
        let j = Journal::open(&path).unwrap();
        let id = j.create("u", "d").unwrap();
        j.set_downloaded(id, 42).unwrap();
        id
    };
    let j = Journal::open(&path).unwrap();
    assert_eq!(j.get(id).unwrap().unwrap().downloaded, 42);
}
