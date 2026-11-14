//! Project automation.
//! - `cargo xtask ci`: fmt, clippy and tests.
//! - `cargo xtask bench`: idb vs wget/curl/aria2c against a local faultbox.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

fn cargo(args: &[&str]) -> bool {
    println!("$ cargo {}", args.join(" "));
    Command::new(env!("CARGO"))
        .args(args)
        .status()
        .is_ok_and(|s| s.success())
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

const PROXY_VARS: [&str; 6] = [
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
];

/// Benchmark subjects: (label, shell command; `$D` is the output dir, `$U` the URL).
fn tools(idb: &Path) -> Vec<(String, String)> {
    let idb = idb.display();
    let run = |c: u32| format!("{idb} get -c {c} -d $D --state-dir $D/.s $U");
    vec![
        ("idb -c1".into(), run(1)),
        ("idb -c4".into(), run(4)),
        ("idb -c8".into(), run(8)),
        ("wget".into(), "wget -q -O $D/f $U".into()),
        ("curl".into(), "curl -s -o $D/f $U".into()),
        (
            "aria2c -x1".into(),
            "aria2c -q -x1 -s1 -d $D -o f $U".into(),
        ),
        (
            "aria2c -x8 -s8".into(),
            "aria2c -q -x8 -s8 -k1M -d $D -o f $U".into(),
        ),
    ]
}

fn version(cmd: &str, arg: &str) -> String {
    Command::new(cmd)
        .arg(arg)
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.lines().next().map(str::to_owned))
        .unwrap_or_else(|| "?".into())
}

/// Medians measured by hyperfine (JSON schema 2).
struct Measure {
    wall: f64,
    wall_sd: f64,
    cpu: f64,
    rss_mib: f64,
}

fn hyperfine(cmd: &str, url: &str, dir: &Path, runs: u32) -> Option<Measure> {
    let json = dir.join("hf.json");
    let d = dir.join("out");
    let shell_cmd = cmd
        .replace("$D", &d.display().to_string())
        .replace("$U", &format!("'{url}'"));
    let mut c = Command::new("hyperfine");
    c.args(["--style", "none", "--warmup", "1", "--runs"])
        .arg(runs.to_string())
        .arg("--prepare")
        .arg(format!("rm -rf {0} && mkdir -p {0}", d.display()))
        .arg("--export-json")
        .arg(&json)
        .arg(&shell_cmd)
        .stdout(Stdio::null());
    for v in PROXY_VARS {
        c.env_remove(v);
    }
    if !c.status().is_ok_and(|s| s.success()) {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&json).ok()?).ok()?;
    let s = &v["results"][0]["summary"];
    Some(Measure {
        wall: s["time_wall_clock"]["median"].as_f64()?,
        wall_sd: s["time_wall_clock"]["stddev"].as_f64().unwrap_or(0.0),
        cpu: s["time_cpu"]["median"].as_f64().unwrap_or(f64::NAN),
        rss_mib: s["memory_peak_resident"]["median"]
            .as_f64()
            .unwrap_or(f64::NAN)
            / (1 << 20) as f64,
    })
}

fn bench() -> ExitCode {
    if !cargo(&["build", "--release", "-p", "idb-cli", "-p", "faultbox"]) {
        return ExitCode::FAILURE;
    }
    let target = root().join("target/release");
    let port = 18_200;
    let Ok(mut server) = Command::new(target.join("faultbox"))
        .args(["--port", &port.to_string()])
        .stdout(Stdio::null())
        .spawn()
    else {
        eprintln!("cannot start faultbox");
        return ExitCode::FAILURE;
    };
    std::thread::sleep(Duration::from_millis(300));
    let work = std::env::temp_dir().join("idb-bench");
    let _ = std::fs::create_dir_all(&work);

    let size: u64 = 32 << 20;
    let base = format!("http://127.0.0.1:{port}/file?size={size}&seed=7");
    let scenarios = [
        (
            "A — servidor limita 2 MiB/s por conexão (32 MiB)",
            format!("{base}&per_conn_rate=2097152"),
            3,
        ),
        ("B — sem limite, loopback (32 MiB)", base.clone(), 5),
    ];
    let mut out = String::new();
    out += &format!(
        "Máquina: {} CPUs, {}\n\nVersões: {} · {} · {} · hyperfine {}\n\n",
        std::thread::available_parallelism().map_or(0, usize::from),
        version("uname", "-sr"),
        version(&target.join("idb").display().to_string(), "--version"),
        version("wget", "--version"),
        version("aria2c", "--version"),
        version("hyperfine", "--version")
            .trim_start_matches("hyperfine ")
            .to_owned(),
    );
    out += &format!("curl: {}\n\n", version("curl", "--version"));
    let started = Instant::now();
    for (title, url, runs) in &scenarios {
        out += &format!(
            "### {title}\n\n| Ferramenta | Tempo, mediana (s) | Desvio (s) | Vazão (MiB/s) | CPU (ms/MiB) | RSS máx. (MiB) |\n|---|---:|---:|---:|---:|---:|\n"
        );
        for (label, cmd) in tools(&target.join("idb")) {
            eprintln!("bench: {title} / {label}");
            match hyperfine(&cmd, url, &work, *runs) {
                Some(m) => {
                    let mib = size as f64 / (1 << 20) as f64;
                    out += &format!(
                        "| `{label}` | {:.2} | {:.2} | {:.1} | {:.2} | {:.1} |\n",
                        m.wall,
                        m.wall_sd,
                        mib / m.wall,
                        m.cpu * 1000.0 / mib,
                        m.rss_mib
                    );
                }
                None => out += &format!("| `{label}` | falhou | | | | |\n"),
            }
        }
        out += &format!("\n{runs} execuções por ferramenta (+1 de aquecimento), mediana.\n\n");
    }
    let _ = server.kill();
    let _ = server.wait();
    let report = root().join("target/bench.md");
    let _ = std::fs::write(&report, &out);
    println!("{out}");
    eprintln!(
        "bench: done in {:.0}s; report in {}",
        started.elapsed().as_secs_f64(),
        report.display()
    );
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("ci") => {
            let ok = cargo(&["fmt", "--all", "--check"])
                && cargo(&["clippy", "--all-targets", "--", "-D", "warnings"])
                && cargo(&["test"]);
            if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Some("bench") => bench(),
        _ => {
            eprintln!("usage: cargo xtask <ci|bench>");
            ExitCode::FAILURE
        }
    }
}
