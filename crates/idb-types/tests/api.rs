#![allow(clippy::unwrap_used)]

use idb_types::api::*;
use idb_types::*;

fn calls() -> Vec<Call> {
    vec![
        Call::Hello {
            api_version: API_VERSION,
            client: "idb-android/0.1".into(),
        },
        Call::Add(AddParams {
            url: "https://example.org/f.iso".into(),
            dir: Some("/home/ana/Downloads".into()),
            connections: Some(ConnectionsParam::Auto),
            limit: Some(2 << 20),
            ..AddParams::default()
        }),
        Call::Pause { id: DownloadId(3) },
        Call::Remove {
            id: DownloadId(3),
            delete_file: true,
        },
        Call::List {
            filter: Some(StateKind::Downloading),
        },
        Call::List { filter: None },
        Call::Diagnose {
            url: "https://example.org/f.iso".into(),
        },
        Call::Subscribe {
            level: EventLevel::Summary,
        },
        Call::Subscribe {
            level: EventLevel::Detail(DownloadId(3)),
        },
    ]
}

#[test]
fn requests_round_trip_and_keep_their_wire_shape() {
    let wire: Vec<serde_json::Value> = calls()
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            let req = Request::new(i as u64 + 1, c);
            let json = serde_json::to_value(&req).unwrap();
            let back: Request = serde_json::from_value(json.clone()).unwrap();
            assert_eq!(back, req);
            json
        })
        .collect();
    insta::assert_json_snapshot!(wire);
}

#[test]
fn parses_a_hand_written_request() {
    let raw =
        r#"{"jsonrpc":"2.0","id":"a1","method":"download.add","params":{"url":"https://x/y"}}"#;
    let req: Request = serde_json::from_str(raw).unwrap();
    assert_eq!(req.id, Some(RequestId::Str("a1".into())));
    assert!(matches!(req.call, Call::Add(AddParams { ref url, .. }) if url == "https://x/y"));
}

#[test]
fn responses_and_events_keep_their_wire_shape() {
    let summary = DownloadSummary {
        id: DownloadId(3),
        url: "https://example.org/f.iso".into(),
        filename: Some("f.iso".into()),
        state: DownloadState::Downloading,
        done: ByteCount(1024),
        total: Some(ByteCount(4096)),
        speed: BytesPerSec(512),
        eta_secs: Some(6),
    };
    let wire = vec![
        serde_json::to_value(Response::ok(
            Some(RequestId::Num(1)),
            CallResult::Hello {
                api_version: API_VERSION,
                server: "idbd/0.0.1".into(),
            },
        ))
        .unwrap(),
        serde_json::to_value(Response::ok(
            Some(RequestId::Num(2)),
            CallResult::List {
                downloads: vec![summary],
            },
        ))
        .unwrap(),
        serde_json::to_value(Response::err(
            Some(RequestId::Num(3)),
            RpcError::new(ErrorKind::DirNotAllowed, "dir outside allowed_dirs"),
        ))
        .unwrap(),
        serde_json::to_value(Notification::event(Event::StateChanged {
            id: DownloadId(3),
            state: DownloadState::Paused,
        }))
        .unwrap(),
    ];
    for w in &wire {
        if w.get("method").is_none() {
            let back: Response = serde_json::from_value(w.clone()).unwrap();
            assert_eq!(serde_json::to_value(back).unwrap(), *w);
        }
    }
    insta::assert_json_snapshot!(wire);
}

#[test]
fn error_codes_are_stable() {
    assert_eq!(ErrorKind::MethodNotFound.code(), -32601);
    assert_eq!(ErrorKind::Unauthorized.code(), -32001);
    assert_eq!(ErrorKind::DirNotAllowed.code(), -32004);
}
