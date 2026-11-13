//! `rajada` command-line client. Phase 1: runs the engine directly, no daemon.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::{Parser, Subcommand};
use rajada_engine::{Config, Downloader, Request};
use rajada_i18n::{Locale, action_label, describe_state, format};
use rajada_store::Journal;
use rajada_types::{DownloadState, StateKind};

/// Exit codes are part of the CLI contract (documented in README).
mod exit {
    pub const COMPLETE: u8 = 0;
    pub const FAILED: u8 = 1;
    pub const NEEDS_USER: u8 = 2;
    pub const INTERNAL: u8 = 3;
}

#[derive(Parser)]
#[command(
    name = "rajada",
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
    Get {
        url: String,
        /// Destination directory (default: XDG download dir).
        #[arg(short = 'd', long)]
        dir: Option<PathBuf>,
        /// Expected SHA-256 in hex; the file is only completed if it matches.
        #[arg(long)]
        sha256: Option<String>,
        /// Print the final result as one JSON line on stdout.
        #[arg(long)]
        json: bool,
        /// Where the journal lives (default: $XDG_STATE_HOME/rajada).
        #[arg(long, hide = true)]
        state_dir: Option<PathBuf>,
        /// Durable checkpoint interval in milliseconds (testing aid).
        #[arg(long, hide = true)]
        checkpoint_ms: Option<u64>,
    },
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
        .join("rajada")
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

async fn get(
    url: String,
    dir: Option<PathBuf>,
    sha256: Option<String>,
    json: bool,
    state_dir: Option<PathBuf>,
    checkpoint_ms: Option<u64>,
) -> anyhow::Result<u8> {
    let l = locale();
    let dir = absolute_dir(&dir.unwrap_or_else(default_download_dir))?;
    let state_dir = state_dir.unwrap_or_else(default_state_dir);
    std::fs::create_dir_all(&state_dir)?;
    let journal = Journal::open(&state_dir.join("journal.sqlite"))?;
    let mut config = Config::default();
    if let Some(ms) = checkpoint_ms {
        config.checkpoint_interval = Duration::from_millis(ms);
    }
    let engine = Downloader::new(config, journal)?;

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
        let v = serde_json::json!({ "id": outcome.id, "state": outcome.state, "path": path });
        println!("{v}");
    }
    eprintln!("{}", describe_state(l, &outcome.state));
    if let Some(p) = &path {
        eprintln!("  {p}");
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
        .with_env_filter(tracing_subscriber::EnvFilter::from_env("RAJADA_LOG"))
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("rajada: {e}");
            return ExitCode::from(exit::INTERNAL);
        }
    };
    let result = rt.block_on(async {
        match cli.command {
            Command::Get {
                url,
                dir,
                sha256,
                json,
                state_dir,
                checkpoint_ms,
            } => get(url, dir, sha256, json, state_dir, checkpoint_ms).await,
        }
    });
    match result {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("rajada: {e:#}");
            ExitCode::from(exit::INTERNAL)
        }
    }
}
