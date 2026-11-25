//! `idb` command-line client. Phase 1: runs the engine directly, no daemon.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::{Args, Parser, Subcommand};
use idb_engine::{Config, Connections, DoctorReport, Downloader, MAX_CONNECTIONS, Request};
use idb_i18n::{Locale, action_label, describe_state, format, render_diagnostic, tr, tr_args};
use idb_store::Journal;
use idb_types::{Diagnostic, DownloadState, StateKind};

/// Exit codes are part of the CLI contract (see docs/CLI.md).
mod exit {
    pub const COMPLETE: u8 = 0;
    pub const FAILED: u8 = 1;
    pub const NEEDS_USER: u8 = 2;
    pub const INTERNAL: u8 = 3;
    pub const USAGE: u8 = 64;
}

#[derive(Parser)]
#[command(
    name = "idb",
    version,
    about = "Instant Download Browser: downloads that are fast, polite and never wrong"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Download a URL (resumes automatically if run again).
    Get(GetArgs),
    /// Explain what limits the speed of a URL, without downloading it.
    Doctor(DoctorArgs),
    /// Phones paired to control this computer (pairing arrives with the daemon).
    Devices {
        #[command(subcommand)]
        action: DevicesAction,
        /// Where the journal lives (default: $XDG_STATE_HOME/idb).
        #[arg(long, hide = true, global = true)]
        state_dir: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum DevicesAction {
    /// List paired phones.
    List,
    /// Remove a phone's access immediately.
    Revoke { id: u64 },
}

#[derive(Args)]
struct Common {
    /// Connections when the server allows ranges: `auto` (default, the
    /// program measures and decides) or a fixed number from 1 to 8.
    #[arg(short = 'c', long, default_value = "auto")]
    connections: String,
    /// Print the result as one JSON line on stdout.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct GetArgs {
    url: String,
    /// Destination directory (default: XDG download dir).
    #[arg(short = 'd', long)]
    dir: Option<PathBuf>,
    /// Expected SHA-256 in hex; the file is only completed if it matches.
    #[arg(long)]
    sha256: Option<String>,
    /// Speed limit for this download, e.g. `2M`, `500k`, `1.5MiB`.
    #[arg(long)]
    limit: Option<String>,
    /// Do not show progress (results and errors are still printed).
    #[arg(short = 'q', long)]
    quiet: bool,
    #[command(flatten)]
    common: Common,
    /// Where the journal lives (default: $XDG_STATE_HOME/idb).
    #[arg(long, hide = true)]
    state_dir: Option<PathBuf>,
    /// Durable checkpoint interval in milliseconds (testing aid).
    #[arg(long, hide = true)]
    checkpoint_ms: Option<u64>,
    /// Smallest segment in KiB (testing aid).
    #[arg(long, hide = true)]
    min_segment_kib: Option<u64>,
}

#[derive(Args)]
struct DoctorArgs {
    url: String,
    /// Maximum probing time in seconds.
    #[arg(long, default_value_t = 5)]
    seconds: u64,
    #[command(flatten)]
    common: Common,
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn default_download_dir() -> PathBuf {
    std::env::var_os("XDG_DOWNLOAD_DIR")
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join("Downloads")))
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn default_state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("idb")
}

fn locale() -> Locale {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .map_or(Locale::En, |v| Locale::from_posix(&v))
}

fn absolute_dir(dir: &Path) -> anyhow::Result<PathBuf> {
    std::fs::canonicalize(dir).with_context(|| format!("directory {}", dir.display()))
}

/// Prints a translated usage error and returns the usage exit code.
fn usage(l: Locale, msgid: &str, args: &[(&str, String)]) -> u8 {
    eprintln!("{}", tr_args(l, msgid, args));
    exit::USAGE
}

fn parse_connections(s: &str) -> Option<Connections> {
    match s.trim() {
        "auto" => Some(Connections::Auto {
            max: MAX_CONNECTIONS,
        }),
        n => n
            .trim_start_matches("fixed:")
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=MAX_CONNECTIONS).contains(n))
            .map(Connections::Fixed),
    }
}

fn valid_sha256(s: &str) -> bool {
    let s = s.trim();
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Diagnostics for `--json`: the structured code plus the translated text.
fn diagnostics_json(l: Locale, ds: &[Diagnostic]) -> serde_json::Value {
    ds.iter()
        .map(|d| {
            serde_json::json!({
                "code": d.code,
                "severity": d.severity,
                "text": render_diagnostic(l, d),
            })
        })
        .collect()
}

fn exit_for(state: &DownloadState) -> u8 {
    match state.kind() {
        StateKind::Complete => exit::COMPLETE,
        StateKind::NeedsUser => exit::NEEDS_USER,
        _ if matches!(state, DownloadState::Failed(_)) => exit::FAILED,
        _ => exit::INTERNAL,
    }
}

async fn get(a: GetArgs) -> anyhow::Result<u8> {
    let l = locale();
    let max = MAX_CONNECTIONS.to_string();
    let Some(connections) = parse_connections(&a.common.connections) else {
        return Ok(usage(l, "cli.connections_limit", &[("max", max)]));
    };
    if let Some(h) = &a.sha256
        && !valid_sha256(h)
    {
        return Ok(usage(l, "cli.invalid_sha256", &[]));
    }
    let limit = match a.limit.as_deref().map(idb_engine::ratelimit::parse_rate) {
        Some(None) => return Ok(usage(l, "cli.invalid_limit", &[])),
        Some(Some(r)) => Some(r),
        None => None,
    };
    let dir = absolute_dir(&a.dir.unwrap_or_else(default_download_dir))?;
    let state_dir = a.state_dir.unwrap_or_else(default_state_dir);
    std::fs::create_dir_all(&state_dir)?;
    let journal = Journal::open(&state_dir.join("journal.sqlite"))?;
    let mut config = Config {
        connections,
        limit,
        ..Config::default()
    };
    if let Some(ms) = a.checkpoint_ms {
        config.checkpoint_interval = Duration::from_millis(ms);
    }
    if let Some(kib) = a.min_segment_kib {
        config.min_segment = kib.max(1) * 1024;
    }
    let engine = Downloader::new(config, journal)?;

    let quiet = a.quiet;
    let mut last_print = Instant::now() - Duration::from_secs(1);
    let mut last_kind = None;
    let req = Request {
        url: a.url,
        dir,
        expected_sha256: a.sha256.map(|h| h.trim().to_owned()),
    };
    let outcome = engine
        .run(req, |p| {
            if quiet {
                return;
            }
            let kind = p.state.kind();
            if Some(kind) == last_kind && last_print.elapsed() < Duration::from_millis(500) {
                return;
            }
            last_kind = Some(kind);
            last_print = Instant::now();
            let size = match p.total {
                Some(t) => format!("{} / {}", format::bytes(p.done, l), format::bytes(t, l)),
                None => format::bytes(p.done, l),
            };
            let mut err = std::io::stderr().lock();
            let _ = write!(err, "\r\x1b[2K{} — {size}", describe_state(l, &p.state));
            let _ = err.flush();
        })
        .await?;
    if !quiet {
        eprintln!();
    }

    let path = outcome.path.as_ref().map(|p| p.display().to_string());
    if a.common.json {
        let v = serde_json::json!({
            "id": outcome.id,
            "state": outcome.state,
            "path": path,
            "connections": outcome.stats.connections,
            "ramp_ms": outcome.stats.ramp_ms,
            "peak_rate": outcome.stats.peak_rate,
            "diagnostics": diagnostics_json(l, &outcome.diagnostics),
        });
        println!("{v}");
    }
    eprintln!("{}", describe_state(l, &outcome.state));
    if let Some(p) = &path {
        eprintln!("  {p}");
    }
    for d in &outcome.diagnostics {
        eprintln!("  ⓘ {}", render_diagnostic(l, d));
    }
    if let Some(a) = outcome.state.recovery() {
        eprintln!("  → {}", action_label(l, a));
    }
    Ok(exit_for(&outcome.state))
}

fn print_doctor(l: Locale, r: &DoctorReport) {
    let size = r
        .size
        .map_or_else(|| tr(l, "doctor.unknown"), |s| format::bytes(s, l));
    println!("{}", tr_args(l, "doctor.size", &[("size", size)]));
    let ranges = match r.ranges {
        Some(true) => "doctor.ranges.yes",
        Some(false) => "doctor.ranges.no",
        None => "doctor.unknown",
    };
    println!(
        "{}",
        tr_args(l, "doctor.ranges", &[("value", tr(l, ranges))])
    );
    let validator = if r.validator {
        "doctor.validator.yes"
    } else {
        "doctor.validator.no"
    };
    println!("{}", tr(l, validator));
    if r.peak_rate > 0 {
        println!(
            "{}",
            tr_args(
                l,
                "doctor.recommend",
                &[
                    ("n", r.recommended_connections.to_string()),
                    ("rate", format::rate(r.peak_rate, l)),
                ],
            )
        );
    }
    for d in &r.diagnostics {
        println!("  ⓘ {}", render_diagnostic(l, d));
    }
    match &r.blocking {
        Some(state) => {
            println!("{}", describe_state(l, state));
            if let Some(a) = state.recovery() {
                println!("  → {}", action_label(l, a));
            }
        }
        None => println!("{}", tr(l, "doctor.ok")),
    }
}

async fn doctor(a: DoctorArgs) -> anyhow::Result<u8> {
    let l = locale();
    let Some(connections) = parse_connections(&a.common.connections) else {
        let max = MAX_CONNECTIONS.to_string();
        return Ok(usage(l, "cli.connections_limit", &[("max", max)]));
    };
    let config = Config {
        connections,
        ..Config::default()
    };
    // The doctor never writes; an in-memory journal keeps it that way.
    let engine = Downloader::new(config, Journal::open_in_memory()?)?;
    let r = engine
        .doctor(&a.url, Duration::from_secs(a.seconds.clamp(1, 60)))
        .await;
    if a.common.json {
        let v = serde_json::json!({
            "final_url": r.final_url,
            "size": r.size,
            "ranges": r.ranges,
            "validator": r.validator,
            "recommended_connections": r.recommended_connections,
            "peak_rate": r.peak_rate,
            "blocking": r.blocking,
            "diagnostics": diagnostics_json(l, &r.diagnostics),
        });
        println!("{v}");
    } else {
        print_doctor(l, &r);
    }
    Ok(r.blocking.as_ref().map_or(exit::COMPLETE, exit_for))
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn devices(action: DevicesAction, state_dir: Option<PathBuf>) -> anyhow::Result<u8> {
    let l = locale();
    let state_dir = state_dir.unwrap_or_else(default_state_dir);
    std::fs::create_dir_all(&state_dir)?;
    let journal = Journal::open(&state_dir.join("journal.sqlite"))?;
    let now = unix_now();
    match action {
        DevicesAction::List => {
            let list = journal.devices()?;
            if list.is_empty() {
                println!("{}", tr(l, "devices.none"));
            }
            for d in list {
                let seen = d.last_seen.map_or_else(
                    || tr(l, "devices.never"),
                    |t| format::duration(now.saturating_sub(t)),
                );
                let key = if d.revoked {
                    "devices.row.revoked"
                } else {
                    "devices.row"
                };
                println!(
                    "{}",
                    tr_args(
                        l,
                        key,
                        &[("id", d.id.to_string()), ("name", d.name), ("seen", seen),],
                    )
                );
            }
            Ok(exit::COMPLETE)
        }
        DevicesAction::Revoke { id } => {
            if journal.revoke_device(id)? {
                println!(
                    "{}",
                    tr_args(l, "devices.revoked", &[("id", id.to_string())])
                );
                Ok(exit::COMPLETE)
            } else {
                Ok(usage(l, "devices.not_found", &[("id", id.to_string())]))
            }
        }
    }
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_env("IDB_LOG"))
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("idb: {e}");
            return ExitCode::from(exit::INTERNAL);
        }
    };
    let result = rt.block_on(async {
        match cli.command {
            Command::Get(args) => get(args).await,
            Command::Doctor(args) => doctor(args).await,
            Command::Devices { action, state_dir } => devices(action, state_dir),
        }
    });
    match result {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("idb: {e:#}");
            ExitCode::from(exit::INTERNAL)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_connection_modes() {
        assert_eq!(
            parse_connections("auto"),
            Some(Connections::Auto { max: 8 })
        );
        assert_eq!(parse_connections("3"), Some(Connections::Fixed(3)));
        assert_eq!(parse_connections("fixed:2"), Some(Connections::Fixed(2)));
        assert_eq!(parse_connections("9"), None);
        assert_eq!(parse_connections("0"), None);
    }

    #[test]
    fn validates_sha256() {
        assert!(valid_sha256(&"a".repeat(64)));
        assert!(!valid_sha256(&"a".repeat(63)));
        assert!(!valid_sha256(&"g".repeat(64)));
    }
}
