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
    cmux_run_in_split(direction, cmdline)?;
    Ok(true)
}

/// Open a new cmux split and type `cmdline` into its shell.
fn cmux_run_in_split(direction: Direction, cmdline: &str) -> Result<()> {
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
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("running {binary} send"))?;
    if !send.success() {
        anyhow::bail!("cmux send failed");
    }
    Ok(())
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

/// Whether the current terminal is cmux (its CLI can open file previews).
pub fn is_cmux() -> bool {
    detect_host() == Some(Host::Cmux)
}

/// The editor command for `Shift+E`. `explicit` is `--editor` or
/// `git config gitgui.editor`; then `$GITGUI_EDITOR`, `$VISUAL`, `$EDITOR`,
/// and finally `vi`. May contain arguments ("code -w"), so it is spliced into
/// the command line unquoted.
pub fn editor_command(explicit: Option<&str>) -> String {
    resolve_editor(
        explicit,
        std::env::var("GITGUI_EDITOR").ok().as_deref(),
        std::env::var("VISUAL").ok().as_deref(),
        std::env::var("EDITOR").ok().as_deref(),
    )
}

pub fn resolve_editor(explicit: Option<&str>, gitgui: Option<&str>, visual: Option<&str>, editor: Option<&str>) -> String {
    [explicit, gitgui, visual, editor]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|v| !v.is_empty())
        .unwrap_or("vi")
        .to_owned()
}

/// GUI editors open their own window; they run detached instead of in a
/// terminal split.
pub fn is_gui_editor(editor: &str) -> bool {
    let first = editor.split_whitespace().next().unwrap_or("");
    let name = first.rsplit('/').next().unwrap_or(first);
    matches!(
        name,
        "code" | "code-insiders" | "codium" | "cursor" | "windsurf" | "subl" | "sublime_text" | "zed" | "mate"
            | "atom" | "idea" | "pycharm" | "clion" | "rustrover" | "webstorm" | "goland" | "bbedit" | "nova"
            | "gedit" | "kate" | "gvim" | "mvim" | "open"
    )
}

/// Shell command that opens `path` (relative to `workdir`) in `editor`.
pub fn editor_cmdline(editor: &str, workdir: &Path, path: &str) -> String {
    format!(
        "cd {} && {editor} {}",
        shell_quote(&workdir.to_string_lossy()),
        shell_quote(path)
    )
}

/// Open `path` in `editor`: a GUI editor is spawned detached, a terminal
/// editor gets a new split next to gitgui. The UI keeps running either way.
/// Errors name the reason so the caller can show it in a toast.
pub fn open_editor(workdir: &Path, path: &str, editor: &str) -> Result<()> {
    let cmdline = editor_cmdline(editor, workdir, path);
    if is_gui_editor(editor) {
        Command::new("/bin/sh")
            .arg("-c")
            .arg(&cmdline)
            .current_dir(workdir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("running {editor}"))?;
        return Ok(());
    }
    match detect_host() {
        Some(Host::Cmux) => cmux_run_in_split(Direction::Right, &cmdline),
        Some(Host::Kitty) => {
            let status = Command::new("kitty")
                .args(["@", "launch", "--location=vsplit"])
                .arg(format!("--cwd={}", workdir.display()))
                .args(["sh", "-c", &cmdline])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .context("running kitty @ launch")?;
            if status.success() {
                Ok(())
            } else {
                anyhow::bail!("kitty remote control failed (enable it in kitty.conf)")
            }
        }
        Some(Host::Ghostty) => anyhow::bail!("Ghostty has no split CLI; run: {cmdline}"),
        None => anyhow::bail!("no split integration for this terminal"),
    }
}

/// `cmux open <file>`: cmux's own file preview tab (markdown rendered,
/// other files with syntax colors) in the pane gitgui runs in.
pub fn cmux_open(workdir: &Path, path: &str) -> Result<()> {
    if !is_cmux() {
        anyhow::bail!("file preview needs cmux");
    }
    let binary = cmux_bin();
    let full = workdir.join(path);
    let out = Command::new(&binary)
        .arg("open")
        .arg(&full)
        .args(["--focus", "true"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("running {binary} open"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("cmux open failed: {}", err.trim());
    }
    Ok(())
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

    #[test]
    fn editor_resolution_order() {
        assert_eq!(resolve_editor(Some("nano"), Some("vim"), Some("code"), Some("vi")), "nano");
        assert_eq!(resolve_editor(None, Some(" vim "), Some("code"), None), "vim");
        assert_eq!(resolve_editor(None, Some(""), None, Some("emacs -nw")), "emacs -nw");
        assert_eq!(resolve_editor(None, None, None, None), "vi");
    }

    #[test]
    fn gui_editors_detected_by_basename() {
        assert!(is_gui_editor("code -w"));
        assert!(is_gui_editor("/usr/local/bin/subl"));
        assert!(!is_gui_editor("vim"));
        assert!(!is_gui_editor("emacs -nw"));
    }

    #[test]
    fn editor_cmdline_quotes_paths_not_editor() {
        let line = editor_cmdline("code -w", Path::new("/tmp/my repo"), "src/a b.rs");
        assert_eq!(line, "cd '/tmp/my repo' && code -w 'src/a b.rs'");
    }
}
