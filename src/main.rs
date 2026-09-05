//! gitgui: a git GUI rendered as pixels inside kitty-graphics terminals.
//! This file only parses the CLI and dispatches to a mode.

mod agent;
mod cli;
mod git;
mod render;
mod runtime;
mod split;
mod term;
mod ui;

use std::env;
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
    match cli.mode {
        cli::Mode::List => match agent::run_ls() {
            Ok(code) => return ExitCode::from(code as u8),
            Err(e) => {
                eprintln!("gitgui: {e:#}");
                return ExitCode::from(1);
            }
        },
        cli::Mode::Action { json, pid } => match agent::run_action(&json, pid) {
            Ok(code) => return ExitCode::from(code as u8),
            Err(e) => {
                eprintln!("gitgui: {e:#}");
                return ExitCode::from(1);
            }
        },
        cli::Mode::SequenceEditor(file) => {
            return match git::rebase::run_sequence_editor(&file) {
                Ok(code) => ExitCode::from(code as u8),
                Err(e) => {
                    eprintln!("gitgui: {e}");
                    ExitCode::from(1)
                }
            }
        }
        cli::Mode::CommitEditor(file) => {
            return match git::rebase::run_commit_editor(&file) {
                Ok(code) => ExitCode::from(code as u8),
                Err(e) => {
                    eprintln!("gitgui: {e}");
                    ExitCode::from(1)
                }
            }
        }
        cli::Mode::Run => {}
    }
    let repo = cli
        .path
        .clone()
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| ".".into()));
    if let Some(dir) = &cli.split {
        let direction = match split::Direction::parse(dir) {
            Some(d) => d,
            None => {
                eprintln!("gitgui: bad split direction {dir:?}; use left, right, up, or down");
                return ExitCode::from(1);
            }
        };
        let exe = env::current_exe().unwrap_or_else(|_| "gitgui".into());
        let args: Vec<String> = env::args().collect();
        match split::try_launch(direction, &exe, &args) {
            Ok(true) => return ExitCode::SUCCESS,
            Ok(false) => {}
            Err(e) => {
                eprintln!("gitgui: split failed: {e:#}; running in this pane");
            }
        }
    }
    let opts = runtime::Options {
        no_shm: cli.no_shm,
        crash: cli.crash,
        scale: cli.scale,
        font_size: cli.font_size,
        editor: cli.editor.clone(),
        open: cli.open.clone(),
        path: repo,
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
