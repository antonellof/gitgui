//! gitgui: a git GUI rendered as pixels inside kitty-graphics terminals.
//! This file only parses the CLI and dispatches to a mode.

mod cli;
mod render;
mod runtime;
mod term;
mod ui;

use std::process::ExitCode;
use std::time::Duration;

fn run_probe(no_shm: bool) -> anyhow::Result<i32> {
    let caps = {
        let _raw = term::RawGuard::enter()?;
        term::probe::probe(!no_shm, Duration::from_millis(1000))?
    };
    print!("{caps}");
    Ok(0)
}

fn main() -> ExitCode {
    let cli = match cli::parse(std::env::args().skip(1)) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(1);
        }
    };
    let opts = runtime::Options {
        no_shm: cli.no_shm,
        crash: cli.crash,
        scale: cli.scale,
        font_size: cli.font_size,
    };
    let result = if cli.probe {
        run_probe(cli.no_shm)
    } else if let Some(path) = &cli.headless {
        runtime::run_headless(path, cli.size, &opts)
    } else if cli.dump_input {
        term::install_handlers();
        let r = runtime::run_dump_input();
        term::restore_terminal();
        r
    } else {
        term::install_handlers();
        let r = runtime::run_interactive(&opts);
        term::restore_terminal();
        r
    };
    match result {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("gitgui: {e:#}");
            ExitCode::from(1)
        }
    }
}
