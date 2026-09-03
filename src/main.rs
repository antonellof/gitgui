//! gitgui: a git GUI rendered as pixels inside kitty-graphics terminals.
//! This file only parses the CLI and dispatches to a mode.

mod demo;
mod term;

use std::process::ExitCode;
use std::time::Duration;

struct Cli {
    probe: bool,
    no_shm: bool,
    crash: bool,
}

const USAGE: &str = "usage: gitgui [--probe] [--no-shm]

  --probe     print detected terminal capabilities and exit
  --no-shm    force the direct (base64 + zlib) transport
  -h, --help  show this help";

fn parse_cli() -> Result<Cli, String> {
    let mut cli = Cli { probe: false, no_shm: false, crash: false };
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--probe" => cli.probe = true,
            "--no-shm" => cli.no_shm = true,
            // Hidden: panic one second into the session to verify restoration.
            "--crash" => cli.crash = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other:?}\n{USAGE}")),
        }
    }
    Ok(cli)
}

fn run_probe(no_shm: bool) -> anyhow::Result<i32> {
    let caps = {
        let _raw = term::RawGuard::enter()?;
        term::probe::probe(!no_shm, Duration::from_millis(1000))?
    };
    print!("{caps}");
    Ok(0)
}

fn main() -> ExitCode {
    let cli = match parse_cli() {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(1);
        }
    };
    term::install_handlers();
    let result = if cli.probe { run_probe(cli.no_shm) } else { demo::run(cli.no_shm, cli.crash) };
    term::restore_terminal();
    match result {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("gitgui: {e:#}");
            ExitCode::from(1)
        }
    }
}
