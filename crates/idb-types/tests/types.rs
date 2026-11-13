use idb_types::*;
use proptest::prelude::*;

fn arb_level() -> impl Strategy<Value = VerificationLevel> {
    prop_oneof![
        Just(VerificationLevel::SizeAndValidator),
        Just(VerificationLevel::SizeOnly),
        Just(VerificationLevel::Unverified),
        Just(VerificationLevel::HashMatched {
            algo: HashAlgo::Sha256,
            source: HashSource::User
        }),
    ]
}

fn arb_state() -> impl Strategy<Value = DownloadState> {
    prop_oneof![
        Just(DownloadState::Queued),
        Just(DownloadState::Probing),
        Just(DownloadState::Downloading),
        Just(DownloadState::Paused),
        any::<u64>().prop_map(|until_unix_secs| DownloadState::PoliteWait { until_unix_secs }),
        prop::sample::select(NeedsUserReason::ALL.to_vec()).prop_map(DownloadState::NeedsUser),
        (0u16..=1000).prop_map(|permille| DownloadState::Verifying { permille }),
        arb_level().prop_map(DownloadState::Complete),
        prop::sample::select(FailReason::ALL.to_vec()).prop_map(DownloadState::Failed),
    ]
}

proptest! {
    #[test]
    fn state_serde_round_trip(state in arb_state()) {
        let json = serde_json::to_string(&state).unwrap();
        let back: DownloadState = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(back, state);
    }

    #[test]
    fn row_event_round_trip(id: u64, done: u64, speed: u64, eta in proptest::option::of(any::<u64>())) {
        let ev = Event::Rows(vec![RowUpdate {
            id: DownloadId(id), done: ByteCount(done), total: None,
            speed: BytesPerSec(speed), eta_secs: eta,
        }]);
        let back: Event = serde_json::from_str(&serde_json::to_string(&ev).unwrap()).unwrap();
        prop_assert_eq!(back, ev);
    }
}

#[test]
fn every_needs_user_and_failure_has_recovery() {
    for r in NeedsUserReason::ALL {
        assert!(DownloadState::NeedsUser(r).recovery().is_some());
    }
    for r in FailReason::ALL {
        assert!(DownloadState::Failed(r).recovery().is_some());
    }
}

#[test]
fn json_schema_snapshots() {
    let samples = vec![
        serde_json::to_value(DownloadState::PoliteWait {
            until_unix_secs: 30,
        })
        .unwrap(),
        serde_json::to_value(DownloadState::NeedsUser(NeedsUserReason::UrlExpired)).unwrap(),
        serde_json::to_value(DownloadState::Complete(VerificationLevel::HashMatched {
            algo: HashAlgo::Sha256,
            source: HashSource::Server,
        }))
        .unwrap(),
        serde_json::to_value(Event::Diagnostic {
            id: DownloadId(7),
            diagnostic: DiagnosticCode::ServerPerConnectionLimit {
                per_conn: BytesPerSec(2 << 20),
                conns: 8,
                total: BytesPerSec(16_567_500),
            }
            .into(),
        })
        .unwrap(),
        serde_json::to_value(EventLevel::Detail(DownloadId(3))).unwrap(),
    ];
    insta::assert_json_snapshot!(samples);
}

#[test]
fn event_level_rates() {
    assert_eq!(EventLevel::Summary.max_hz(), 1);
    assert_eq!(EventLevel::Rows.max_hz(), 4);
    assert_eq!(API_VERSION, 1);
}
