//! Built-in file editor shown in the detail pane: iced `text_editor` with
//! syntax colors from `highlight.rs`. Saves straight to the working tree;
//! git never sees an editor, the next refresh picks the change up.

use std::path::{Path, PathBuf};

use iced_core::keyboard;
use iced_core::{Alignment, Font, Length};
use iced_widget::text_editor::{self, Binding};
use iced_widget::{column, row, text, Space};

use crate::ui::app::{App, Element, Message, Renderer};
use crate::ui::highlight::{self, Lang};
use crate::ui::widgets::{self, small_button};

pub const MAX_EDIT_BYTES: u64 = 1024 * 1024;

pub struct Editor {
    /// Repo-relative path, as shown in the lists.
    pub path: String,
    pub full: PathBuf,
    pub content: text_editor::Content<Renderer>,
    saved: String,
    pub lang: Lang,
    crlf: bool,
    dirty: bool,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// Last edit: when, and whether it was a typed character, so a run of
    /// typing undoes as one step.
    last_edit: Option<(std::time::Instant, bool)>,
}

/// The buffer before an edit, with the cursor to put back.
struct Snapshot {
    text: String,
    cursor: text_editor::Cursor,
}

const UNDO_DEPTH: usize = 200;
const TYPING_GROUP_MS: u128 = 800;

impl Editor {
    /// Read `path` under `workdir`. Errors are user-facing strings.
    pub fn open(workdir: &Path, path: &str) -> Result<Editor, String> {
        let full = workdir.join(path);
        let meta = std::fs::metadata(&full).map_err(|e| format!("{path}: {e}"))?;
        if meta.is_dir() {
            return Err(format!("{path} is a directory"));
        }
        if meta.len() > MAX_EDIT_BYTES {
            return Err(format!("{path} is over 1 MB, open it in $EDITOR (Shift+E)"));
        }
        let bytes = std::fs::read(&full).map_err(|e| format!("{path}: {e}"))?;
        if bytes.contains(&0) {
            return Err(format!("{path} is a binary file"));
        }
        let raw = String::from_utf8_lossy(&bytes).into_owned();
        let crlf = raw.contains("\r\n");
        let text = if crlf { raw.replace("\r\n", "\n") } else { raw };
        Ok(Editor {
            lang: Lang::from_path(path),
            path: path.to_owned(),
            full,
            content: text_editor::Content::with_text(&text),
            saved: text,
            crlf,
            dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: None,
        })
    }

    pub fn perform(&mut self, action: text_editor::Action) {
        let edit = action.is_edit();
        if edit {
            let typed = matches!(action, text_editor::Action::Edit(text_editor::Edit::Insert(c)) if !c.is_whitespace());
            let grouped = typed
                && self
                    .last_edit
                    .is_some_and(|(at, was_typed)| was_typed && at.elapsed().as_millis() < TYPING_GROUP_MS);
            if !grouped {
                self.undo.push(self.snapshot());
                if self.undo.len() > UNDO_DEPTH {
                    self.undo.remove(0);
                }
            }
            self.redo.clear();
            self.last_edit = Some((std::time::Instant::now(), typed));
        }
        self.content.perform(action);
        if edit {
            self.refresh_dirty();
        }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.content.text(),
            cursor: self.content.cursor(),
        }
    }

    fn restore(&mut self, snap: Snapshot) {
        self.content = text_editor::Content::with_text(&snap.text);
        self.content.move_to(snap.cursor);
        self.refresh_dirty();
    }

    fn refresh_dirty(&mut self) {
        self.dirty = self.content.text().trim_end_matches('\n') != self.saved.trim_end_matches('\n');
    }

    /// Ctrl+Z. False when there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        let Some(snap) = self.undo.pop() else { return false };
        self.redo.push(self.snapshot());
        self.restore(snap);
        self.last_edit = None;
        true
    }

    /// Ctrl+Y or Ctrl+Shift+Z.
    pub fn redo(&mut self) -> bool {
        let Some(snap) = self.redo.pop() else { return false };
        self.undo.push(self.snapshot());
        self.restore(snap);
        self.last_edit = None;
        true
    }

    pub fn dirty(&self) -> bool {
        self.dirty
    }

    /// Write the buffer back with the file's original line endings.
    pub fn save(&mut self) -> Result<(), String> {
        let mut text = self.content.text();
        // `Content::text` always ends with a newline; keep the file's own.
        if !self.saved.ends_with('\n') && text.ends_with('\n') {
            text.pop();
        }
        let out = if self.crlf { text.replace('\n', "\r\n") } else { text.clone() };
        std::fs::write(&self.full, out).map_err(|e| format!("{}: {e}", self.path))?;
        self.saved = text;
        self.dirty = false;
        Ok(())
    }

    pub fn line_count(&self) -> usize {
        self.content.line_count()
    }
}

pub fn view(app: &App) -> Element<'_> {
    let t = &app.theme;
    let Some(ed) = app.editor.as_ref() else {
        return text("").into();
    };
    let dirty = ed.dirty();
    let busy = app.busy > 0;
    let pos = ed.content.cursor().position;
    let (line, col) = (pos.line, pos.column);
    let editor_name = app.external_editor();
    let editor_label = editor_name.split_whitespace().next().unwrap_or("editor").to_owned();
    let mut header = row![
        text(&ed.path).size(13).font(Font::MONOSPACE).color(if dirty { t.strong } else { t.text }),
        text(format!(
            "{} · {} lines · Ln {}, Col {}{}",
            ed.lang.label(),
            ed.line_count(),
            line + 1,
            col + 1,
            if dirty { " · modified" } else { "" }
        ))
        .size(12)
        .color(t.weak),
        Space::new().width(Length::Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .padding([4, 6]);
    header = header.push(small_button(if app.editor_wrap { "wrap on" } else { "wrap" }, Some(Message::EditorWrap)));
    header = header.push(small_button("Save", (dirty && !busy).then_some(Message::EditorSave)));
    if crate::split::is_cmux() {
        header = header.push(small_button("open/preview", Some(Message::EditorPreview)));
    }
    header = header.push(small_button(editor_label, Some(Message::EditorExternal)));
    header = header.push(small_button("Close", Some(Message::EditorClose)));

    let editor = iced_widget::TextEditor::new(&ed.content)
        .id(widgets::EDITOR_ID.clone())
        .on_action(Message::EditorAction)
        .font(Font::MONOSPACE)
        .size(13)
        .padding(8)
        .height(Length::Fill)
        .wrapping(if app.editor_wrap {
            iced_core::text::Wrapping::Word
        } else {
            iced_core::text::Wrapping::None
        })
        .key_binding(|press| {
            // iced consults the binding even when the editor is not focused.
            if !matches!(press.status, text_editor::Status::Focused { .. }) {
                return None;
            }
            let mods = press.modifiers;
            match &press.key {
                keyboard::Key::Character(c) if mods.control() && c.as_str() == "s" => {
                    Some(Binding::Custom(Message::EditorSave))
                }
                keyboard::Key::Character(c) if mods.control() && c.as_str() == "z" => Some(Binding::Custom(if mods.shift() {
                    Message::EditorRedo
                } else {
                    Message::EditorUndo
                })),
                keyboard::Key::Character(c) if mods.control() && c.as_str() == "y" => {
                    Some(Binding::Custom(Message::EditorRedo))
                }
                keyboard::Key::Named(keyboard::key::Named::Escape) => Some(Binding::Custom(Message::EditorClose)),
                keyboard::Key::Named(keyboard::key::Named::Tab) if !mods.control() => {
                    Some(Binding::Sequence(vec![Binding::Insert(' '), Binding::Insert(' '), Binding::Insert(' '), Binding::Insert(' ')]))
                }
                _ => Binding::from_key_press(press),
            }
        })
        .highlight_with::<highlight::Highlighter>(ed.lang, highlight::format)
        .style(widgets::text_editor_style);
    column![header, editor].into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempfile_dir() -> PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let d = std::env::temp_dir().join(format!("gitgui-editor-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn open_save_round_trip_keeps_crlf() {
        let dir = tempfile_dir();
        std::fs::write(dir.join("a.txt"), "one\r\ntwo\r\n").unwrap();
        let mut ed = Editor::open(&dir, "a.txt").unwrap();
        assert!(!ed.dirty());
        ed.perform(text_editor::Action::Move(text_editor::Motion::DocumentEnd));
        ed.perform(text_editor::Action::Edit(text_editor::Edit::Insert('x')));
        assert!(ed.dirty());
        ed.save().unwrap();
        assert!(!ed.dirty());
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "one\r\ntwo\r\nx");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rejects_binary_and_missing() {
        let dir = tempfile_dir();
        std::fs::write(dir.join("b.bin"), [0u8, 1, 2]).unwrap();
        let err = Editor::open(&dir, "b.bin").err().expect("binary rejected");
        assert!(err.contains("binary"));
        assert!(Editor::open(&dir, "nope").is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
