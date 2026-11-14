//! `idb` command-line client. Phase 1: runs the engine directly, no daemon.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::{Args, Parser, Subcommand};
use idb_engine::{Config, Connections, Downloader, MAX_CONNECTIONS, Request};
use idb_i18n::{Locale, action_label, describe_state, format, render_diagnostic, tr_args};
use idb_store::Journal;
use idb_types::{DownloadState, StateKind};

/// Exit codes are part of the CLI contract (documented in README).
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
    about = "Download manager that never delivers a wrong file"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Download a URL (resumes automatically if run again).
    Get(GetArgs),
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
    /// Connections when the server allows ranges: `auto` (default, the
    /// program measures and decides) or a fixed number from 1 to 8.
    #[arg(short = 'c', long, default_value = "auto")]
    connections: String,
    /// Print the final result as one JSON line on stdout.
    #[arg(long)]
    json: bool,
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

async fn get(a: GetArgs) -> anyhow::Result<u8> {
    let l = locale();
    let connections = match a.connections.trim() {
        "auto" => Some(Connections::Auto {
            max: MAX_CONNECTIONS,
        }),
        n => n
            .trim_start_matches("fixed:")
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=MAX_CONNECTIONS).contains(n))
            .map(Connections::Fixed),
    };
    let Some(connections) = connections else {
        eprintln!(
            "{}",
            tr_args(
                l,
                "cli.connections_limit",
                &[("max", MAX_CONNECTIONS.to_string())]
            )
        );
        return Ok(exit::USAGE);
    };
    let dir = absolute_dir(&a.dir.unwrap_or_else(default_download_dir))?;
    let state_dir = a.state_dir.unwrap_or_else(default_state_dir);
    std::fs::create_dir_all(&state_dir)?;
    let journal = Journal::open(&state_dir.join("journal.sqlite"))?;
    let mut config = Config {
        connections,
        ..Config::default()
    };
    if let Some(ms) = a.checkpoint_ms {
        config.checkpoint_interval = Duration::from_millis(ms);
    }
    if let Some(kib) = a.min_segment_kib {
        config.min_segment = kib.max(1) * 1024;
    }
    let engine = Downloader::new(config, journal)?;
    let (url, sha256, json) = (a.url, a.sha256, a.json);

    let mut last_print = Instant::now() - Duration::from_secs(1);
    let mut last_kind = None;
    let req = Request {
        url,
        dir,
        expected_sha256: sha256,
    };
    let outcome = engine
        .run(req, |p| {
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
    eprintln!();

    let path = outcome.path.as_ref().map(|p| p.display().to_string());
    if json {
        let v = serde_json::json!({
            "id": outcome.id,
            "state": outcome.state,
            "path": path,
            "connections": outcome.stats.connections,
            "ramp_ms": outcome.stats.ramp_ms,
            "peak_rate": outcome.stats.peak_rate,
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
    Ok(match outcome.state.kind() {
        StateKind::Complete => exit::COMPLETE,
        StateKind::NeedsUser => exit::NEEDS_USER,
        _ if matches!(outcome.state, DownloadState::Failed(_)) => exit::FAILED,
        _ => exit::INTERNAL,
    })
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
