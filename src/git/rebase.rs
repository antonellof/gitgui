//! Non-interactive rewriting of a `git rebase -i` todo list. gitgui runs
//! `git rebase -i` with itself as `GIT_SEQUENCE_EDITOR`; git hands the todo
//! file to `gitgui --sequence-editor <file>`, which applies one action to one
//! commit and exits. `GIT_EDITOR` is pointed at `gitgui --commit-editor
//! <file>`, which writes a prepared message (reword) or leaves git's default
//! (squash) in place.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoAction {
    Drop,
    Squash,
    Fixup,
    Reword,
    Edit,
    /// Swap with the next newer commit.
    MoveUp,
    /// Swap with the next older commit.
    MoveDown,
    /// Leave the list as git wrote it (autosquash).
    Keep,
}

impl TodoAction {
    pub fn as_str(self) -> &'static str {
        match self {
            TodoAction::Drop => "drop",
            TodoAction::Squash => "squash",
            TodoAction::Fixup => "fixup",
            TodoAction::Reword => "reword",
            TodoAction::Edit => "edit",
            TodoAction::MoveUp => "move-up",
            TodoAction::MoveDown => "move-down",
            TodoAction::Keep => "keep",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "drop" => TodoAction::Drop,
            "squash" => TodoAction::Squash,
            "fixup" => TodoAction::Fixup,
            "reword" => TodoAction::Reword,
            "edit" => TodoAction::Edit,
            "move-up" => TodoAction::MoveUp,
            "move-down" => TodoAction::MoveDown,
            "keep" => TodoAction::Keep,
            _ => return None,
        })
    }
}

/// Environment variables through which the worker tells the editor
/// subprocess what to do.
pub const ENV_ACTION: &str = "GITGUI_TODO_ACTION";
pub const ENV_OID: &str = "GITGUI_TODO_OID";
pub const ENV_MESSAGE: &str = "GITGUI_COMMIT_MESSAGE";

fn is_pick(line: &str) -> bool {
    let mut it = line.split_whitespace();
    matches!(it.next(), Some("pick") | Some("p")) && it.next().is_some()
}

fn line_sha(line: &str) -> Option<&str> {
    line.split_whitespace().nth(1)
}

fn sha_matches(sha: &str, oid: &str) -> bool {
    let (a, b) = (sha.to_lowercase(), oid.to_lowercase());
    !a.is_empty() && (b.starts_with(&a) || a.starts_with(&b))
}

/// Apply `action` to the todo line for `oid`. Lines are oldest first, as git
/// writes them. Returns the new todo text.
pub fn rewrite_todo(todo: &str, oid: &str, action: TodoAction) -> Result<String, String> {
    if action == TodoAction::Keep {
        return Ok(todo.to_owned());
    }
    let mut lines: Vec<String> = todo.lines().map(|l| l.to_owned()).collect();
    let idx = lines
        .iter()
        .position(|l| is_pick(l) && line_sha(l).is_some_and(|s| sha_matches(s, oid)))
        .ok_or_else(|| format!("commit {oid} is not in the rebase todo"))?;
    let replace_verb = |line: &str, verb: &str| -> String {
        let rest = line.split_once(char::is_whitespace).map(|(_, r)| r).unwrap_or("");
        format!("{verb} {rest}")
    };
    match action {
        TodoAction::Drop => lines[idx] = replace_verb(&lines[idx], "drop"),
        TodoAction::Squash => {
            if !lines[..idx].iter().any(|l| is_pick(l)) {
                return Err("nothing below to squash into".into());
            }
            lines[idx] = replace_verb(&lines[idx], "squash");
        }
        TodoAction::Fixup => {
            if !lines[..idx].iter().any(|l| is_pick(l)) {
                return Err("nothing below to fix up into".into());
            }
            lines[idx] = replace_verb(&lines[idx], "fixup");
        }
        TodoAction::Reword => lines[idx] = replace_verb(&lines[idx], "reword"),
        TodoAction::Edit => lines[idx] = replace_verb(&lines[idx], "edit"),
        TodoAction::MoveUp => {
            // Newer commit is the next pick line below in the file.
            let next = lines[idx + 1..]
                .iter()
                .position(|l| is_pick(l))
                .map(|p| idx + 1 + p)
                .ok_or_else(|| "already the newest commit".to_owned())?;
            lines.swap(idx, next);
        }
        TodoAction::MoveDown => {
            let prev = lines[..idx]
                .iter()
                .rposition(|l| is_pick(l))
                .ok_or_else(|| "already the oldest commit".to_owned())?;
            lines.swap(idx, prev);
        }
        TodoAction::Keep => {}
    }
    let mut out = lines.join("\n");
    out.push('\n');
    Ok(out)
}

/// `gitgui --sequence-editor <file>`: rewrite the todo in place using the
/// action and commit from the environment.
pub fn run_sequence_editor(file: &Path) -> Result<i32, String> {
    let action = std::env::var(ENV_ACTION)
        .ok()
        .and_then(|a| TodoAction::parse(&a))
        .unwrap_or(TodoAction::Keep);
    if action == TodoAction::Keep {
        return Ok(0);
    }
    let oid = std::env::var(ENV_OID).map_err(|_| format!("{ENV_OID} not set"))?;
    let todo = std::fs::read_to_string(file).map_err(|e| format!("read todo: {e}"))?;
    let new = rewrite_todo(&todo, &oid, action)?;
    std::fs::write(file, new).map_err(|e| format!("write todo: {e}"))?;
    Ok(0)
}

/// `gitgui --commit-editor <file>`: replace the commit message with the one
/// from the environment, or leave git's default when none was given.
pub fn run_commit_editor(file: &Path) -> Result<i32, String> {
    if let Ok(msg) = std::env::var(ENV_MESSAGE) {
        let mut text = msg.trim_end().to_owned();
        text.push('\n');
        std::fs::write(file, text).map_err(|e| format!("write message: {e}"))?;
    }
    Ok(0)
}

/// Quote a path for `sh -c`, which is how git invokes editors.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TODO: &str = "pick aaaaaaa first\npick bbbbbbb second\npick ccccccc third\n\n# Rebase 1234..ccccccc onto 1234 (3 commands)\n#\n# Commands:\n# p, pick <commit> = use commit\n";

    #[test]
    fn drop_squash_fixup_reword() {
        let out = rewrite_todo(TODO, "bbbbbbb1234", TodoAction::Drop).unwrap();
        assert!(out.starts_with("pick aaaaaaa first\ndrop bbbbbbb second\npick ccccccc third\n"));
        assert!(out.contains("# Commands:"), "comments kept");
        let out = rewrite_todo(TODO, "ccc", TodoAction::Squash).unwrap();
        assert!(out.contains("\nsquash ccccccc third\n"));
        let out = rewrite_todo(TODO, "ccccccc", TodoAction::Fixup).unwrap();
        assert!(out.contains("\nfixup ccccccc third\n"));
        let out = rewrite_todo(TODO, "aaaaaaa", TodoAction::Reword).unwrap();
        assert!(out.starts_with("reword aaaaaaa first\n"));
        let out = rewrite_todo(TODO, "aaaaaaa", TodoAction::Edit).unwrap();
        assert!(out.starts_with("edit aaaaaaa first\n"));
    }

    #[test]
    fn squash_needs_an_older_commit() {
        assert!(rewrite_todo(TODO, "aaaaaaa", TodoAction::Squash).is_err());
        assert!(rewrite_todo(TODO, "aaaaaaa", TodoAction::Fixup).is_err());
        assert!(rewrite_todo(TODO, "ddddddd", TodoAction::Drop).is_err());
    }

    #[test]
    fn move_up_and_down() {
        let out = rewrite_todo(TODO, "bbbbbbb", TodoAction::MoveUp).unwrap();
        assert!(out.starts_with("pick aaaaaaa first\npick ccccccc third\npick bbbbbbb second\n"));
        let out = rewrite_todo(TODO, "bbbbbbb", TodoAction::MoveDown).unwrap();
        assert!(out.starts_with("pick bbbbbbb second\npick aaaaaaa first\npick ccccccc third\n"));
        assert!(rewrite_todo(TODO, "ccccccc", TodoAction::MoveUp).is_err());
        assert!(rewrite_todo(TODO, "aaaaaaa", TodoAction::MoveDown).is_err());
    }

    #[test]
    fn keep_leaves_todo_alone() {
        assert_eq!(rewrite_todo(TODO, "zzz", TodoAction::Keep).unwrap(), TODO);
    }

    #[test]
    fn short_verbs_and_full_oids_match() {
        let todo = "p 0123456 msg\np 89abcde other\n";
        let out = rewrite_todo(
            todo,
            "0123456789abcdef0123456789abcdef01234567",
            TodoAction::Drop,
        )
        .unwrap();
        assert_eq!(out, "drop 0123456 msg\np 89abcde other\n");
    }

    #[test]
    fn editors_write_files() {
        let dir = std::env::temp_dir().join(format!("gitgui-rebase-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let todo = dir.join("todo");
        std::fs::write(&todo, TODO).unwrap();
        std::env::set_var(ENV_ACTION, "drop");
        std::env::set_var(ENV_OID, "bbbbbbb");
        run_sequence_editor(&todo).unwrap();
        assert!(std::fs::read_to_string(&todo)
            .unwrap()
            .contains("drop bbbbbbb second"));
        std::env::remove_var(ENV_ACTION);
        std::env::remove_var(ENV_OID);
        let msg = dir.join("msg");
        std::fs::write(&msg, "old\n").unwrap();
        std::env::set_var(ENV_MESSAGE, "new message\n\nbody");
        run_commit_editor(&msg).unwrap();
        assert_eq!(std::fs::read_to_string(&msg).unwrap(), "new message\n\nbody\n");
        std::env::remove_var(ENV_MESSAGE);
        run_commit_editor(&msg).unwrap();
        assert_eq!(std::fs::read_to_string(&msg).unwrap(), "new message\n\nbody\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quoting() {
        assert_eq!(shell_quote("/usr/bin/gitgui"), "'/usr/bin/gitgui'");
        assert_eq!(shell_quote("/a b/it's"), "'/a b/it'\\''s'");
        assert_eq!(TodoAction::parse("move-up"), Some(TodoAction::MoveUp));
        assert_eq!(TodoAction::Squash.as_str(), "squash");
    }
}
