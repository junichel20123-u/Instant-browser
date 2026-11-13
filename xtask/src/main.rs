//! Project automation. `cargo xtask ci` runs fmt, clippy and tests.
use std::process::{Command, ExitCode};

fn run(args: &[&str]) -> bool {
    println!("$ cargo {}", args.join(" "));
    Command::new(env!("CARGO"))
        .args(args)
        .status()
        .is_ok_and(|s| s.success())
}

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("ci") => {
            let ok = run(&["fmt", "--all", "--check"])
                && run(&["clippy", "--all-targets", "--", "-D", "warnings"])
                && run(&["test"]);
            if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        _ => {
            eprintln!("usage: cargo xtask ci");
            ExitCode::FAILURE
        }
    }
}
