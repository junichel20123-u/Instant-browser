use rajada_i18n::*;
use rajada_types::*;

#[test]
fn catalogs_parse_and_share_msgids() {
    for l in Locale::ALL {
        let src = match l {
            Locale::PtBr => include_str!("../po/pt_BR.po"),
            Locale::En => include_str!("../po/en.po"),
        };
        assert!(parse_po(src).is_ok(), "{l:?} catalog must parse");
    }
    assert_eq!(msgids(Locale::PtBr), msgids(Locale::En));
}

fn all_states() -> Vec<DownloadState> {
    let mut v = vec![
        DownloadState::Queued,
        DownloadState::Probing,
        DownloadState::Downloading,
        DownloadState::Paused,
        DownloadState::PoliteWait { until_unix_secs: 0 },
        DownloadState::Verifying { permille: 430 },
        DownloadState::Complete(VerificationLevel::HashMatched {
            algo: HashAlgo::Sha256,
            source: HashSource::User,
        }),
        DownloadState::Complete(VerificationLevel::SizeAndValidator),
        DownloadState::Complete(VerificationLevel::SizeOnly),
        DownloadState::Complete(VerificationLevel::Unverified),
    ];
    v.extend(NeedsUserReason::ALL.map(DownloadState::NeedsUser));
    v.extend(FailReason::ALL.map(DownloadState::Failed));
    v
}

fn all_diagnostics() -> Vec<Diagnostic> {
    [
        DiagnosticCode::ServerPerConnectionLimit {
            per_conn: BytesPerSec(2 << 20),
            conns: 8,
            total: BytesPerSec(16_567_500),
        },
        DiagnosticCode::LinkSaturated {
            link_est: BytesPerSec(12_500_000),
        },
        DiagnosticCode::ServerAskedToWait {
            secs: 30,
            reduced_to: 4,
        },
        DiagnosticCode::HtmlInsteadOfFile {
            size: ByteCount(38 * 1024),
        },
        DiagnosticCode::RangeNotSupported,
        DiagnosticCode::BdpExceedsRmem {
            bdp: ByteCount(18 << 20),
            rmem_max: ByteCount(6 << 20),
        },
        DiagnosticCode::MirrorQuarantined {
            url: "https://mirror.example/f.iso".into(),
            why: "hash".into(),
        },
        DiagnosticCode::StalledConnectionReassigned,
    ]
    .into_iter()
    .map(Diagnostic::from)
    .collect()
}

fn render_all(l: Locale) -> String {
    let mut out = String::new();
    for k in StateKind::ALL {
        out += &format!("label {k:?}: {}\n", state_label(l, k));
    }
    for s in all_states() {
        out += &format!("{:?}: {}", s.kind(), describe_state(l, &s));
        if let Some(a) = s.recovery() {
            out += &format!(" [{}]", action_label(l, a));
        }
        out.push('\n');
    }
    for d in all_diagnostics() {
        out += &format!("{:?}: {}\n", d.severity, render_diagnostic(l, &d));
    }
    out
}

#[test]
fn no_untranslated_or_unfilled_text() {
    for l in Locale::ALL {
        let text = render_all(l);
        for marker in [
            "state.", "needs.", "fail.", "action.", "verify.", "diag.", "{",
        ] {
            assert!(!text.contains(marker), "{l:?} leaks {marker:?}:\n{text}");
        }
    }
}

#[test]
fn snapshot_pt_br() {
    insta::assert_snapshot!(render_all(Locale::PtBr));
}

#[test]
fn snapshot_en() {
    insta::assert_snapshot!(render_all(Locale::En));
}
