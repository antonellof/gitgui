//! Commit message suggestions from an AI command line tool.
//!
//! gitgui never talks to a model itself. It pipes a prompt (the staged diff,
//! the branch, a few recent subjects) into a shell command that prints the
//! message on stdout: `claude -p`, `codex exec`, `gemini`, `ollama run`,
//! `llm`, or anything the user configures. Keys, sessions and models stay in
//! that tool.
//!
//! The command is `$GITGUI_AI_COMMAND`, else `git config gitgui.ai-command`,
//! else the first known tool found in `$PATH`. The prompt template is
//! `$GITGUI_AI_PROMPT`, else `git config gitgui.ai-prompt`, else the default
//! below; `{diff}`, `{branch}` and `{recent}` are replaced, a literal `\n`
//! becomes a newline.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const ENV_COMMAND: &str = "GITGUI_AI_COMMAND";
pub const ENV_PROMPT: &str = "GITGUI_AI_PROMPT";
pub const CONFIG_COMMAND: &str = "gitgui.ai-command";
pub const CONFIG_PROMPT: &str = "gitgui.ai-prompt";

/// Largest diff handed to the tool, in bytes. Beyond it files are cut.
pub const MAX_DIFF: usize = 24_000;
/// How long the tool may take before it is killed.
pub const TIMEOUT: Duration = Duration::from_secs(90);

pub const DEFAULT_PROMPT: &str = "Write a git commit message for the staged changes below.
Rules:
- First line: a summary in the imperative mood, at most 72 characters, no trailing period.
- Optional body after one blank line: why the change was made, wrapped at 72 columns.
- Plain text only: no markdown, no code fences, no preamble, nothing but the message.
Branch: {branch}
Recent commit subjects, match their style:
{recent}
Staged diff:
{diff}
";

/// Known tools in preference order: executable name and the command line
/// that reads the prompt on stdin and prints the answer. `ollama` is special
/// (needs a model).
const CANDIDATES: &[(&str, &str)] = &[
    ("claude", "claude -p"),
    ("codex", "codex exec -"),
    ("gemini", "gemini -p 'Reply with the commit message only.'"),
    ("ollama", "ollama run"),
    ("llm", "llm"),
];

/// Where the command came from, for `--probe` and the help dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    pub command: String,
    pub source: &'static str,
}

/// Pick the command: environment, git config, then autodetect.
pub fn resolve(env: Option<&str>, config: Option<&str>) -> Option<Tool> {
    pick(env, config).or_else(autodetect)
}

pub fn pick(env: Option<&str>, config: Option<&str>) -> Option<Tool> {
    let clean = |v: Option<&str>| v.map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned);
    if let Some(command) = clean(env) {
        return Some(Tool { command, source: ENV_COMMAND });
    }
    if let Some(command) = clean(config) {
        return Some(Tool { command, source: CONFIG_COMMAND });
    }
    None
}

/// The first known tool on `$PATH`.
pub fn autodetect() -> Option<Tool> {
    let path = std::env::var_os("PATH")?;
    let dirs: Vec<_> = std::env::split_paths(&path).collect();
    autodetect_in(&dirs, ollama_model)
}

fn autodetect_in(dirs: &[std::path::PathBuf], ollama_model: impl Fn(&Path) -> Option<String>) -> Option<Tool> {
    for (exe, command) in CANDIDATES {
        let Some(found) = dirs.iter().map(|d| d.join(exe)).find(|p| is_executable(p)) else {
            continue;
        };
        let command = if *exe == "ollama" {
            let Some(model) = ollama_model(&found) else { continue };
            format!("{command} {model}")
        } else {
            (*command).to_owned()
        };
        return Some(Tool { command, source: "autodetect" });
    }
    None
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// The first model `ollama list` knows, if any.
fn ollama_model(exe: &Path) -> Option<String> {
    let out = Command::new(exe).arg("list").stdin(Stdio::null()).output().ok()?;
    parse_ollama_list(&String::from_utf8_lossy(&out.stdout))
}

pub fn parse_ollama_list(text: &str) -> Option<String> {
    text.lines()
        .skip(1)
        .filter_map(|l| l.split_whitespace().next())
        .find(|m| !m.is_empty())
        .map(str::to_owned)
}

/// The template with `{diff}`, `{branch}` and `{recent}` filled in.
pub fn build_prompt(template: Option<&str>, diff: &str, branch: &str, recent: &[String]) -> String {
    let template = template
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| t.replace("\\n", "\n"))
        .unwrap_or_else(|| DEFAULT_PROMPT.to_owned());
    let recent = if recent.is_empty() {
        "(none)".to_owned()
    } else {
        recent.iter().map(|s| format!("- {s}")).collect::<Vec<_>>().join("\n")
    };
    let branch = if branch.is_empty() { "(detached HEAD)" } else { branch };
    template
        .replace("{branch}", branch)
        .replace("{recent}", &recent)
        .replace("{diff}", diff)
}

/// Lock files carry no intent and blow the budget.
fn is_lockfile(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    matches!(
        name,
        "Cargo.lock" | "package-lock.json" | "yarn.lock" | "pnpm-lock.yaml" | "go.sum" | "Gemfile.lock" | "poetry.lock"
            | "composer.lock" | "flake.lock" | "Podfile.lock" | "packages.lock.json" | "bun.lockb" | "uv.lock"
    ) || name.ends_with(".lock")
}

/// Cut a unified diff down to about `max` bytes. Lock files are reduced to
/// their header; once the budget is spent the remaining files keep only
/// their `diff --git` line, so the model still sees what was touched.
pub fn truncate_diff(diff: &str, max: usize) -> String {
    let mut files: Vec<Vec<&str>> = Vec::new();
    for line in diff.lines() {
        if line.starts_with("diff --git ") || files.is_empty() {
            files.push(Vec::new());
        }
        files.last_mut().expect("pushed").push(line);
    }
    let mut out = String::new();
    let mut spent = false;
    for lines in files {
        let header = lines[0];
        let path = header.strip_prefix("diff --git a/").and_then(|r| r.split(" b/").next()).unwrap_or("");
        if !path.is_empty() && is_lockfile(path) {
            out.push_str(header);
            out.push_str("\n(lock file, content omitted)\n");
            continue;
        }
        if spent {
            out.push_str(header);
            out.push_str("\n(content omitted)\n");
            continue;
        }
        for line in lines {
            if out.len() + line.len() + 1 > max {
                out.push_str("(... truncated)\n");
                spent = true;
                break;
            }
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Turn whatever the tool printed into a message: no fences, no "Commit
/// message:" preamble, no wrapping quotes, one blank line after the subject.
pub fn clean_output(raw: &str) -> String {
    let mut lines: Vec<&str> = raw
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim_start().starts_with("```"))
        .collect();
    while lines.first().is_some_and(|l| l.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    if let Some(first) = lines.first() {
        let low = first.trim().to_ascii_lowercase();
        if low.starts_with("commit message:") || low.starts_with("here is") || low.starts_with("here's") {
            let rest = first.split_once(':').map(|(_, r)| r.trim()).unwrap_or("");
            if rest.is_empty() {
                lines.remove(0);
                while lines.first().is_some_and(|l| l.trim().is_empty()) {
                    lines.remove(0);
                }
            } else {
                lines[0] = rest;
            }
        }
    }
    let mut text = lines.join("\n");
    let quoted = |q: char| text.starts_with(q) && text.ends_with(q) && text.len() > 1;
    if quoted('"') || quoted('\'') || quoted('`') {
        text = text[1..text.len() - 1].trim().to_owned();
    }
    // A subject directly followed by body text: insert the blank line.
    if let Some((subject, rest)) = text.split_once('\n') {
        if !rest.starts_with('\n') && !rest.trim().is_empty() {
            text = format!("{subject}\n\n{rest}");
        }
    }
    while text.contains("\n\n\n") {
        text = text.replace("\n\n\n", "\n\n");
    }
    text
}

/// Run `sh -c command` in `workdir` with `prompt` on stdin. Stdout is the
/// answer; on failure the last stderr line is the error.
pub fn run(workdir: &Path, command: &str, prompt: &str, timeout: Duration) -> Result<String, String> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {command}: {e}"))?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let prompt = prompt.to_owned();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(prompt.as_bytes());
    });
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let out_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let err_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{} gave no answer within {} s", first_word(command), timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let _ = writer.join();
    let out = String::from_utf8_lossy(&out_reader.join().unwrap_or_default()).into_owned();
    let err = String::from_utf8_lossy(&err_reader.join().unwrap_or_default()).into_owned();
    let message = clean_output(&out);
    if status.success() && !message.is_empty() {
        return Ok(message);
    }
    let last_err = err.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("");
    Err(if !last_err.is_empty() {
        format!("{}: {last_err}", first_word(command))
    } else if !status.success() {
        format!("{} failed ({status})", first_word(command))
    } else {
        format!("{} printed nothing", first_word(command))
    })
}

fn first_word(command: &str) -> &str {
    command.split_whitespace().next().unwrap_or(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_beats_config_beats_autodetect() {
        let t = pick(Some(" claude -p "), Some("llm")).unwrap();
        assert_eq!(t.command, "claude -p");
        assert_eq!(t.source, ENV_COMMAND);
        let t = pick(Some("  "), Some("llm -m gpt")).unwrap();
        assert_eq!(t.command, "llm -m gpt");
        assert_eq!(t.source, CONFIG_COMMAND);
        assert!(pick(None, Some("")).is_none());
    }

    #[test]
    fn autodetect_walks_the_candidates_in_order() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("gitgui-ai-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mk = |name: &str| {
            let p = dir.join(name);
            std::fs::write(&p, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        let dirs = std::slice::from_ref(&dir);
        assert!(autodetect_in(dirs, |_| None).is_none());
        mk("llm");
        assert_eq!(autodetect_in(dirs, |_| None).unwrap().command, "llm");
        mk("ollama");
        // ollama without a model is skipped, with one it wins over llm.
        assert_eq!(autodetect_in(dirs, |_| None).unwrap().command, "llm");
        assert_eq!(
            autodetect_in(dirs, |_| Some("qwen2.5-coder".into())).unwrap().command,
            "ollama run qwen2.5-coder"
        );
        mk("claude");
        let t = autodetect_in(dirs, |_| None).unwrap();
        assert_eq!(t.command, "claude -p");
        assert_eq!(t.source, "autodetect");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ollama_list_first_model() {
        let text = "NAME                ID              SIZE      MODIFIED\nqwen2.5-coder:7b    abc123          4.7 GB    2 days ago\nllama3:8b  def  4 GB  1 week ago\n";
        assert_eq!(parse_ollama_list(text).as_deref(), Some("qwen2.5-coder:7b"));
        assert_eq!(parse_ollama_list("NAME ID SIZE MODIFIED\n"), None);
        assert_eq!(parse_ollama_list(""), None);
    }

    #[test]
    fn prompt_fills_placeholders() {
        let p = build_prompt(None, "DIFF", "main", &["one".into(), "two".into()]);
        assert!(p.contains("Branch: main\n"));
        assert!(p.contains("- one\n- two\n"));
        assert!(p.ends_with("Staged diff:\nDIFF\n"));
        let p = build_prompt(Some("b={branch}\\nr={recent}\\nd={diff}"), "D", "", &[]);
        assert_eq!(p, "b=(detached HEAD)\nr=(none)\nd=D");
        assert!(build_prompt(Some("  "), "D", "x", &[]).contains("Branch: x"));
    }

    const DIFF: &str = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-old\n+new\ndiff --git a/Cargo.lock b/Cargo.lock\n--- a/Cargo.lock\n+++ b/Cargo.lock\n@@ -1 +1 @@\n-x\n+y\ndiff --git a/src/b.rs b/src/b.rs\n--- a/src/b.rs\n+++ b/src/b.rs\n@@ -1 +1 @@\n-b\n+c\n";

    #[test]
    fn truncation_keeps_headers_and_drops_lock_files() {
        let full = truncate_diff(DIFF, 100_000);
        assert!(full.contains("+new\n"));
        assert!(full.contains("diff --git a/Cargo.lock b/Cargo.lock\n(lock file, content omitted)\n"));
        assert!(!full.contains("+y\n"));
        assert!(full.contains("+c\n"));
        let cut = truncate_diff(DIFF, 70);
        assert!(cut.contains("diff --git a/src/a.rs b/src/a.rs\n"));
        assert!(cut.contains("(... truncated)\n"));
        assert!(cut.contains("diff --git a/src/b.rs b/src/b.rs\n(content omitted)\n"));
        assert!(!cut.contains("+c\n"));
        assert_eq!(truncate_diff("", 10), "");
    }

    #[test]
    fn output_is_cleaned() {
        assert_eq!(clean_output("```\nfix: thing\n\nbody\n```\n"), "fix: thing\n\nbody");
        assert_eq!(clean_output("Commit message:\n\nAdd tests\n"), "Add tests");
        assert_eq!(clean_output("Commit message: Add tests"), "Add tests");
        assert_eq!(clean_output("\"Add tests\"\n"), "Add tests");
        assert_eq!(clean_output("Subject\nbody line\n"), "Subject\n\nbody line");
        assert_eq!(clean_output("Subject\n\n\n\nbody\n"), "Subject\n\nbody");
        assert_eq!(clean_output("  \n\n"), "");
    }

    #[test]
    fn runs_a_shell_command_with_the_prompt_on_stdin() {
        let dir = std::env::temp_dir();
        let out = run(&dir, "cat", "Add a thing\n", TIMEOUT).unwrap();
        assert_eq!(out, "Add a thing");
        let err = run(&dir, "echo boom >&2; exit 3", "", TIMEOUT).unwrap_err();
        assert_eq!(err, "echo: boom");
        let err = run(&dir, "true", "", TIMEOUT).unwrap_err();
        assert_eq!(err, "true printed nothing");
        let err = run(&dir, "sleep 5", "", Duration::from_millis(200)).unwrap_err();
        assert!(err.contains("no answer"), "{err}");
    }
}
