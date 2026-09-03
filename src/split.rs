//! Open gitgui in a new terminal split when the host supports it.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            _ => None,
        }
    }

    fn cmux(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Up => "up",
            Self::Down => "down",
        }
    }

    fn kitty(self) -> &'static str {
        match self {
            Self::Left | Self::Right => "vsplit",
            Self::Up | Self::Down => "hsplit",
        }
    }

    fn ghostty(self) -> &'static str {
        self.cmux()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Host {
    Cmux,
    Ghostty,
    Kitty,
}

fn detect_host() -> Option<Host> {
    if std::env::var_os("CMUX_SURFACE_ID").is_some()
        || std::env::var("TERM_PROGRAM").ok().as_deref() == Some("cmux")
    {
        return Some(Host::Cmux);
    }
    match std::env::var("TERM_PROGRAM").ok().as_deref() {
        Some("ghostty") => Some(Host::Ghostty),
        Some("kitty") => Some(Host::Kitty),
        _ => None,
    }
}

/// Shell-quote a single argument for `/bin/sh -c`.
pub fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    if s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"/._-:@%+,=".contains(&b))
    {
        return s.into();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Build argv for the child process: drop `--split`, `--size`, and `action`/`ls`.
pub fn child_argv(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip_next = false;
    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        match arg.as_str() {
            "--split" | "--size" => skip_next = true,
            "action" | "ls" => {}
            _ => out.push(arg.clone()),
        }
    }
    out
}

/// Try to open gitgui in a new split. Returns `Ok(true)` when the caller should
/// exit (the new pane owns the session), `Ok(false)` to run in the current pane.
pub fn try_launch(direction: Direction, exe: &Path, args: &[String]) -> Result<bool> {
    let Some(host) = detect_host() else {
        eprintln!("gitgui: no split integration for this terminal; running here");
        return Ok(false);
    };
    let child_args = child_argv(args);
    let cmdline = build_cmdline(exe, &child_args);
    match host {
        Host::Cmux => launch_cmux(direction, &cmdline),
        Host::Ghostty => launch_ghostty(direction, &cmdline),
        Host::Kitty => launch_kitty(direction, exe, &child_args),
    }
}

fn build_cmdline(exe: &Path, args: &[String]) -> String {
    let mut parts = vec![shell_quote(&exe.to_string_lossy())];
    parts.extend(args.iter().map(|a| shell_quote(a)));
    parts.join(" ")
}

fn cmux_bin() -> String {
    std::env::var("CMUX_BUNDLED_CLI_PATH").unwrap_or_else(|_| "cmux".into())
}

fn launch_cmux(direction: Direction, cmdline: &str) -> Result<bool> {
    let binary = cmux_bin();
    let out = Command::new(&binary)
        .args([
            "new-split",
            direction.cmux(),
            "--focus",
            "true",
            "--json",
            "--id-format",
            "both",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("running {binary} new-split"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("cmux new-split failed: {err}");
    }
    let reply: serde_json::Value =
        serde_json::from_slice(&out.stdout).context("parse cmux new-split json")?;
    let surface = reply
        .get("surface_id")
        .or_else(|| reply.get("surface_ref"))
        .and_then(|v| v.as_str())
        .context("cmux new-split did not return a surface id")?;
    let send = Command::new(&binary)
        .arg("send")
        .arg("--surface")
        .arg(surface)
        .arg("--")
        .arg(format!("{cmdline}\n"))
        .status()
        .with_context(|| format!("running {binary} send"))?;
    if !send.success() {
        anyhow::bail!("cmux send failed");
    }
    Ok(true)
}

fn launch_ghostty(direction: Direction, cmdline: &str) -> Result<bool> {
    let action = format!("+new_split:{}", direction.ghostty());
    if Command::new("ghostty")
        .arg(&action)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()
        .is_some_and(|s| s.success())
    {
        return Ok(true);
    }
    eprintln!(
        "gitgui: Ghostty has no stable split CLI from a child process; \
         create a split with your new_split:{} keybind, then run: {cmdline}",
        direction.ghostty()
    );
    Ok(false)
}

fn launch_kitty(direction: Direction, exe: &Path, args: &[String]) -> Result<bool> {
    let mut cmd = Command::new("kitty");
    cmd.arg("@").arg("launch");
    cmd.arg(format!("--location={}", direction.kitty()));
    cmd.arg("--cwd=current");
    cmd.arg(exe);
    cmd.args(args);
    let status = cmd
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .status()
        .context("running kitty @ launch")?;
    if status.success() {
        Ok(true)
    } else {
        eprintln!(
            "gitgui: kitty remote control failed (enable it in kitty.conf); running here"
        );
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_literals() {
        assert_eq!(shell_quote("hello"), "hello");
        assert_eq!(shell_quote("/tmp/a b"), "'/tmp/a b'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn child_argv_strips_split_flags() {
        let args = vec![
            "gitgui".into(),
            "--split".into(),
            "right".into(),
            "--size".into(),
            "0.4".into(),
            "--repo".into(),
            "/tmp/r".into(),
        ];
        let child = child_argv(&args);
        assert_eq!(child, vec!["gitgui", "--repo", "/tmp/r"]);
    }

    #[test]
    fn direction_parse() {
        assert_eq!(Direction::parse("right"), Some(Direction::Right));
        assert_eq!(Direction::parse("bogus"), None);
    }
}
