//! Undo / redo for text fields. iced's `text_editor` and `text_input` keep
//! no history, so the app snapshots the text before each edit. Consecutive
//! typed characters form one step (a word at a time); whitespace, deletes,
//! pastes and an undo end the group.

use iced_widget::text_editor::{self, Action, Edit};

use crate::ui::app::Renderer;

const DEPTH: usize = 200;

struct Snap {
    text: String,
    cursor: text_editor::Cursor,
}

/// History for a `text_editor::Content`: the file editor, the commit box,
/// the multiline dialog field.
#[derive(Default)]
pub struct ContentHistory {
    undo: Vec<Snap>,
    redo: Vec<Snap>,
    typing: bool,
}

impl ContentHistory {
    /// Call before `content.perform(action)`.
    pub fn before(&mut self, content: &text_editor::Content<Renderer>, action: &Action) {
        if !action.is_edit() {
            return;
        }
        let typed = matches!(action, Action::Edit(Edit::Insert(c)) if !c.is_whitespace());
        if !(typed && self.typing) {
            push(&mut self.undo, snap(content));
        }
        self.redo.clear();
        self.typing = typed;
    }

    pub fn undo(&mut self, content: &mut text_editor::Content<Renderer>) -> bool {
        let Some(s) = self.undo.pop() else { return false };
        self.redo.push(snap(content));
        restore(content, s);
        self.typing = false;
        true
    }

    pub fn redo(&mut self, content: &mut text_editor::Content<Renderer>) -> bool {
        let Some(s) = self.redo.pop() else { return false };
        self.undo.push(snap(content));
        restore(content, s);
        self.typing = false;
        true
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.typing = false;
    }
}

fn snap(content: &text_editor::Content<Renderer>) -> Snap {
    Snap {
        text: content.text(),
        cursor: content.cursor(),
    }
}

fn restore(content: &mut text_editor::Content<Renderer>, s: Snap) {
    *content = text_editor::Content::with_text(&s.text);
    content.move_to(s.cursor);
}

fn push<T>(stack: &mut Vec<T>, item: T) {
    stack.push(item);
    if stack.len() > DEPTH {
        stack.remove(0);
    }
}

/// History for a `text_input` value: the commit filter, the diff search,
/// the dialog fields. Records whole values; `record` groups typing.
#[derive(Default)]
pub struct TextHistory {
    undo: Vec<String>,
    redo: Vec<String>,
    typing: bool,
}

impl TextHistory {
    /// Call with the value before and after an input change.
    pub fn record(&mut self, old: &str, new: &str) {
        if old == new {
            return;
        }
        let typed = one_char_typed(old, new);
        if !(typed && self.typing) {
            push(&mut self.undo, old.to_owned());
        }
        self.redo.clear();
        self.typing = typed;
    }

    /// The value to put back, if any. `current` goes onto the redo stack.
    pub fn undo(&mut self, current: &str) -> Option<String> {
        let prev = self.undo.pop()?;
        self.redo.push(current.to_owned());
        self.typing = false;
        Some(prev)
    }

    pub fn redo(&mut self, current: &str) -> Option<String> {
        let next = self.redo.pop()?;
        self.undo.push(current.to_owned());
        self.typing = false;
        Some(next)
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.typing = false;
    }
}

/// True when `new` is `old` with a single non-whitespace character inserted.
fn one_char_typed(old: &str, new: &str) -> bool {
    if new.chars().count() != old.chars().count() + 1 {
        return false;
    }
    let common_prefix = old.chars().zip(new.chars()).take_while(|(a, b)| a == b).count();
    let inserted = new.chars().nth(common_prefix);
    let old_rest: String = old.chars().skip(common_prefix).collect();
    let new_rest: String = new.chars().skip(common_prefix + 1).collect();
    old_rest == new_rest && inserted.is_some_and(|c| !c.is_whitespace())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_history_groups_typing_and_splits_on_space() {
        let mut h = TextHistory::default();
        let steps = ["f", "fi", "fix", "fix ", "fix t", "fix ty"];
        let mut prev = "";
        for s in steps {
            h.record(prev, s);
            prev = s;
        }
        assert_eq!(h.undo("fix ty").as_deref(), Some("fix "));
        assert_eq!(h.undo("fix ").as_deref(), Some("fix"));
        assert_eq!(h.undo("fix").as_deref(), Some(""));
        assert_eq!(h.undo(""), None);
        assert_eq!(h.redo("").as_deref(), Some("fix"));
        // A new edit clears redo.
        h.record("fix", "fixe");
        assert_eq!(h.redo("fixe"), None);
    }

    #[test]
    fn content_history_groups_typed_characters() {
        let mut c = text_editor::Content::with_text("");
        let mut h = ContentHistory::default();
        for ch in ['a', 'b', ' ', 'c'] {
            let a = Action::Edit(Edit::Insert(ch));
            h.before(&c, &a);
            c.perform(a);
        }
        let text = |c: &text_editor::Content<Renderer>| c.text().trim_end_matches('\n').to_owned();
        assert_eq!(text(&c), "ab c");
        assert!(h.undo(&mut c));
        assert_eq!(text(&c), "ab ");
        assert!(h.undo(&mut c));
        assert_eq!(text(&c), "ab");
        assert!(h.undo(&mut c));
        assert_eq!(text(&c), "");
        assert!(!h.undo(&mut c));
        assert!(h.redo(&mut c));
        assert_eq!(text(&c), "ab");
    }
}
