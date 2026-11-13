//! One message catalog for every IDB client (CLI, TUI, GUI, extension).
//!
//! Catalogs are gettext `.po` files embedded at compile time and parsed in
//! pure Rust (see ADR-0002). Messages use `{name}` placeholders.

pub mod format;
mod po;

use std::collections::HashMap;
use std::sync::OnceLock;

use idb_types::{
    Diagnostic, DiagnosticCode, DownloadState, RecoveryAction, StateKind, VerificationLevel,
};

pub use po::{PoError, parse as parse_po};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Locale {
    PtBr,
    En,
}

impl Locale {
    pub const ALL: [Locale; 2] = [Locale::PtBr, Locale::En];

    /// Picks a locale from a POSIX value like `pt_BR.UTF-8`; defaults to English.
    pub fn from_posix(value: &str) -> Locale {
        if value.starts_with("pt") {
            Locale::PtBr
        } else {
            Locale::En
        }
    }

    fn source(self) -> &'static str {
        match self {
            Locale::PtBr => include_str!("../po/pt_BR.po"),
            Locale::En => include_str!("../po/en.po"),
        }
    }
}

fn catalog(locale: Locale) -> &'static HashMap<String, String> {
    static PT: OnceLock<HashMap<String, String>> = OnceLock::new();
    static EN: OnceLock<HashMap<String, String>> = OnceLock::new();
    let cell = match locale {
        Locale::PtBr => &PT,
        Locale::En => &EN,
    };
    // Catalogs are embedded and validated by tests; a parse error yields an
    // empty catalog so lookups fall back to the msgid instead of panicking.
    cell.get_or_init(|| po::parse(locale.source()).unwrap_or_default())
}

/// Every msgid in the given catalog (used by consistency tests).
pub fn msgids(locale: Locale) -> Vec<&'static str> {
    let mut v: Vec<_> = catalog(locale).keys().map(String::as_str).collect();
    v.sort_unstable();
    v
}

/// Looks up a message; falls back to the msgid itself when missing.
pub fn tr(locale: Locale, msgid: &str) -> String {
    catalog(locale)
        .get(msgid)
        .cloned()
        .unwrap_or_else(|| msgid.to_owned())
}

/// Looks up a message and substitutes `{name}` placeholders.
pub fn tr_args(locale: Locale, msgid: &str, args: &[(&str, String)]) -> String {
    let mut s = tr(locale, msgid);
    for (k, v) in args {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

pub fn state_label(locale: Locale, kind: StateKind) -> String {
    tr(locale, &format!("state.{kind:?}"))
}

pub fn verification_label(locale: Locale, level: &VerificationLevel) -> String {
    let id = match level {
        VerificationLevel::HashMatched { .. } => "verify.Hash",
        VerificationLevel::SizeAndValidator | VerificationLevel::SizeOnly => "verify.Size",
        VerificationLevel::Unverified => "verify.Unverified",
    };
    tr(locale, id)
}

pub fn action_label(locale: Locale, action: RecoveryAction) -> String {
    tr(locale, &format!("action.{action:?}"))
}

/// Full human description of a state, identical in every client.
pub fn describe_state(locale: Locale, state: &DownloadState) -> String {
    match state {
        DownloadState::Verifying { permille } => tr_args(
            locale,
            "verify.progress",
            &[("pct", (permille / 10).to_string())],
        ),
        DownloadState::Complete(level) => verification_label(locale, level),
        DownloadState::NeedsUser(r) => tr(locale, &format!("needs.{r:?}")),
        DownloadState::Failed(r) => format!(
            "{}: {}",
            state_label(locale, state.kind()),
            tr(locale, &format!("fail.{r:?}"))
        ),
        DownloadState::PoliteWait { .. }
        | DownloadState::Queued
        | DownloadState::Probing
        | DownloadState::Downloading
        | DownloadState::Paused => state_label(locale, state.kind()),
    }
}

/// Renders a doctor diagnostic as one human sentence.
pub fn render_diagnostic(locale: Locale, diag: &Diagnostic) -> String {
    use format::{bytes, duration, rate};
    let (id, args): (&str, Vec<(&str, String)>) = match &diag.code {
        DiagnosticCode::ServerPerConnectionLimit {
            per_conn,
            conns,
            total,
        } => (
            "diag.ServerPerConnectionLimit",
            vec![
                ("per_conn", rate(per_conn.0, locale)),
                ("conns", conns.to_string()),
                ("total", rate(total.0, locale)),
            ],
        ),
        DiagnosticCode::LinkSaturated { link_est } => (
            "diag.LinkSaturated",
            vec![("link_est", rate(link_est.0, locale))],
        ),
        DiagnosticCode::ServerAskedToWait { secs, reduced_to } => (
            "diag.ServerAskedToWait",
            vec![
                ("secs", duration(*secs)),
                ("reduced_to", reduced_to.to_string()),
            ],
        ),
        DiagnosticCode::HtmlInsteadOfFile { size } => (
            "diag.HtmlInsteadOfFile",
            vec![("size", bytes(size.0, locale))],
        ),
        DiagnosticCode::RangeNotSupported => ("diag.RangeNotSupported", vec![]),
        DiagnosticCode::BdpExceedsRmem { bdp, rmem_max } => (
            "diag.BdpExceedsRmem",
            vec![
                ("bdp", bytes(bdp.0, locale)),
                ("rmem_max", bytes(rmem_max.0, locale)),
            ],
        ),
        DiagnosticCode::MirrorQuarantined { url, why } => (
            "diag.MirrorQuarantined",
            vec![("url", url.clone()), ("why", why.clone())],
        ),
        DiagnosticCode::StalledConnectionReassigned => ("diag.StalledConnectionReassigned", vec![]),
    };
    tr_args(locale, id, &args)
}
