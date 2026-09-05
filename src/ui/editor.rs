//! Built-in file editor shown in place of the diff pane. Plain egui
//! `TextEdit` with a line-number gutter and the highlighter from
//! `highlight.rs`. Saves straight to the working tree; git never sees an
//! editor, the next status refresh picks the change up like any other write.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{Color32, FontId, Margin};

use crate::git::ops::Command;
use crate::ui::app::App;
use crate::ui::highlight::{self, Lang, Palette};

/// Files above this are opened read-only-ish: still shown, but no editing,
/// because layout of a huge single galley is too slow per frame.
pub const MAX_EDIT_BYTES: u64 = 1024 * 1024;

pub struct Editor {
    /// Repo-relative path, as shown in the lists.
    pub path: String,
    pub full: PathBuf,
    pub text: String,
    saved: String,
    pub lang: Lang,
    crlf: bool,
    /// Take keyboard focus on the next frame.
    focus_requested: bool,
    /// The text field had focus in the last frame (single-key bindings are
    /// off while it does).
    pub has_focus: bool,
    /// (text hash, font size bits) of the cached galley.
    cache_key: (u64, u32),
    cache: Option<Arc<egui::Galley>>,
    gutter_lines: usize,
    gutter: String,
    /// Line to scroll into view on the next frame (1-based).
    pub goto_line: Option<usize>,
}

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
            saved: text.clone(),
            text,
            crlf,
            focus_requested: true,
            has_focus: false,
            cache_key: (0, 0),
            cache: None,
            gutter_lines: 0,
            gutter: String::new(),
            goto_line: None,
        })
    }

    /// Do not steal keyboard focus on the first frame.
    pub fn quiet(&mut self) {
        self.focus_requested = false;
    }

    pub fn dirty(&self) -> bool {
        self.text != self.saved
    }

    /// Write the buffer back with the file's original line endings.
    pub fn save(&mut self) -> Result<(), String> {
        let out = if self.crlf { self.text.replace('\n', "\r\n") } else { self.text.clone() };
        std::fs::write(&self.full, out).map_err(|e| format!("{}: {e}", self.path))?;
        self.saved = self.text.clone();
        Ok(())
    }

    pub fn line_count(&self) -> usize {
        self.text.split('\n').count()
    }

    fn gutter_text(&mut self) -> &str {
        let n = self.line_count();
        if n != self.gutter_lines {
            let width = n.max(1).to_string().len();
            let mut s = String::with_capacity(n * (width + 1));
            for i in 1..=n {
                if i > 1 {
                    s.push('\n');
                }
                s.push_str(&format!("{i:>width$}"));
            }
            self.gutter = s;
            self.gutter_lines = n;
        }
        &self.gutter
    }
}

fn hash_text(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// What the header asked for; applied after drawing.
#[derive(Default)]
struct Action {
    save: bool,
    close: bool,
    external: bool,
    preview: bool,
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let theme = app.theme.clone();
    let mut action = Action::default();
    let busy = app.busy > 0;
    let editor_name = app.external_editor();
    let cmux = crate::split::is_cmux();
    let mut hide = false;
    let Some(ed) = app.editor.as_mut() else { return };
    let dirty = ed.dirty();
    let path = ed.path.clone();
    let lang = ed.lang;
    let lines = ed.line_count();

    crate::ui::row::split(
        ui,
        |ui| {
            if ui
                .add(egui::Button::new("hide").small())
                .on_hover_text("Hide the changes and diff pane (3 toggles)")
                .clicked()
            {
                hide = true;
            }
            if ui
                .add(egui::Button::new("Close").small())
                .on_hover_text("Escape (asks when there are unsaved changes)")
                .clicked()
            {
                action.close = true;
            }
            if ui
                .add(egui::Button::new(editor_name.split_whitespace().next().unwrap_or("editor")).small())
                .on_hover_text(format!("Open in {editor_name} (Shift+E). Set with --editor or git config gitgui.editor"))
                .clicked()
            {
                action.external = true;
            }
            if cmux
                && ui
                    .add(egui::Button::new("cmux").small())
                    .on_hover_text("Open in a cmux file preview tab (Shift+O)")
                    .clicked()
            {
                action.preview = true;
            }
            if ui
                .add_enabled(dirty && !busy, egui::Button::new("Save").small())
                .on_hover_text("Ctrl+S")
                .clicked()
            {
                action.save = true;
            }
        },
        |ui| {
            let mut label = egui::RichText::new(&path).monospace();
            if dirty {
                label = label.strong();
            }
            ui.add(egui::Label::new(label).truncate());
            ui.weak(format!("{} · {lines} lines{}", lang.label(), if dirty { " · modified" } else { "" }));
        },
    );
    ui.separator();

    let font: FontId = egui::TextStyle::Monospace.resolve(ui.style());
    let text_color = ui.visuals().text_color();
    let palette = Palette::from_theme(&theme, text_color);
    let line_no: Color32 = theme.line_no;
    let row_h = ui.fonts_mut(|f| f.row_height(&font));
    let goto = ed.goto_line.take();

    // Galley cache: keyed on the buffer contents at layout time, because the
    // TextEdit re-lays out after every edit within the same frame.
    let font_bits = font.size.to_bits();
    let mut cache = ed.cache.take().map(|g| (ed.cache_key, g));
    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _wrap: f32| -> Arc<egui::Galley> {
        let key = (hash_text(buf.as_str()), font_bits);
        if let Some((k, g)) = &cache {
            if *k == key {
                return g.clone();
            }
        }
        let job = highlight::layout_job(buf.as_str(), lang, font.clone(), &palette);
        let g = ui.fonts_mut(|f| f.layout_job(job));
        cache = Some((key, g.clone()));
        g
    };

    let gutter = ed.gutter_text().to_owned();
    let mut request_focus = std::mem::take(&mut ed.focus_requested);
    let mut has_focus = false;
    let bg = ui.visuals().extreme_bg_color;
    egui::ScrollArea::both()
        .id_salt("editor_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Frame::new().fill(bg).inner_margin(Margin::symmetric(6, 4)).show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    ui.add(egui::Label::new(egui::RichText::new(gutter).font(font.clone()).color(line_no)).selectable(false));
                    let ed = app.editor.as_mut().expect("editor open");
                    let out = egui::TextEdit::multiline(&mut ed.text)
                        .id_salt(("editor", &ed.path))
                        .code_editor()
                        .font(font.clone())
                        .frame(egui::Frame::NONE)
                        .margin(Margin::ZERO)
                        .lock_focus(true)
                        .desired_width(f32::INFINITY)
                        .desired_rows(1)
                        .layouter(&mut layouter)
                        .show(ui);
                    if request_focus {
                        out.response.request_focus();
                        request_focus = false;
                    }
                    has_focus = out.response.has_focus();
                    if let Some(line) = goto {
                        let y = out.response.rect.min.y + row_h * (line.saturating_sub(1)) as f32;
                        let r = egui::Rect::from_min_size(egui::pos2(out.response.rect.min.x, y), egui::vec2(1.0, row_h));
                        ui.scroll_to_rect(r, Some(egui::Align::Center));
                    }
                });
            });
        });
    let ed = app.editor.as_mut().expect("editor open");
    ed.has_focus = has_focus;
    if let Some((k, g)) = cache {
        ed.cache_key = k;
        ed.cache = Some(g);
    }

    if hide {
        app.toggle_panel(crate::ui::app::Pane::Detail);
    }
    if action.save {
        app.save_editor();
    }
    if action.external {
        app.edit_selected_external();
    }
    if action.preview {
        app.preview_selected_in_cmux();
    }
    if action.close {
        app.close_editor();
    }
}

impl App {
    /// Open the selected file in the built-in editor.
    pub fn edit_selected(&mut self) {
        let Some(path) = self.current_file() else {
            self.toast("select a file first", true);
            return;
        };
        self.open_editor(path);
    }

    pub fn open_editor(&mut self, path: String) {
        if let Some(ed) = &self.editor {
            if ed.path == path {
                self.editor.as_mut().expect("checked").focus_requested = true;
                return;
            }
            if ed.dirty() {
                self.toast(format!("{} has unsaved changes: Ctrl+S or Escape first", ed.path), true);
                return;
            }
        }
        let workdir = self.snapshot.path.clone();
        match Editor::open(&workdir, &path) {
            Ok(ed) => {
                self.editor = Some(ed);
                self.focus = crate::ui::app::Pane::Detail;
            }
            Err(e) => self.toast(e, true),
        }
    }

    pub fn save_editor(&mut self) {
        let Some(ed) = self.editor.as_mut() else { return };
        match ed.save() {
            Ok(()) => {
                let p = ed.path.clone();
                self.toast(format!("saved {p}"), false);
                self.pending.push(Command::Refresh);
            }
            Err(e) => self.toast(format!("cannot save: {e}"), true),
        }
    }

    /// Escape: close the editor, or ask first when the buffer is dirty.
    pub fn close_editor(&mut self) {
        let Some(ed) = &self.editor else { return };
        if ed.dirty() {
            self.modal = Some(crate::ui::app::Modal::CloseEditor);
        } else {
            self.editor = None;
        }
    }

    pub fn editor_focused(&self) -> bool {
        self.editor.as_ref().is_some_and(|e| e.has_focus)
    }

    /// Open the selected file in `$EDITOR` in a new terminal split.
    pub fn edit_selected_external(&mut self) {
        let Some(path) = self.current_file() else {
            self.toast("select a file first", true);
            return;
        };
        let workdir = self.snapshot.path.clone();
        if !workdir.join(&path).exists() {
            self.toast(format!("{path} is not in the working tree"), true);
            return;
        }
        let editor = self.external_editor();
        match crate::split::open_editor(&workdir, &path, &editor) {
            Ok(()) => self.toast(format!("opened {path} in {editor}"), false),
            Err(e) => self.toast(format!("cannot open editor: {e:#}"), true),
        }
    }

    /// `--editor`, then `git config gitgui.editor`, then the environment.
    pub fn external_editor(&self) -> String {
        let explicit = self.editor_cmd.as_deref().or(self.snapshot.editor.as_deref());
        crate::split::editor_command(explicit)
    }

    /// Open the selected file in cmux's own file preview tab.
    pub fn preview_selected_in_cmux(&mut self) {
        let Some(path) = self.current_file() else {
            self.toast("select a file first", true);
            return;
        };
        let workdir = self.snapshot.path.clone();
        if !workdir.join(&path).exists() {
            self.toast(format!("{path} is not in the working tree"), true);
            return;
        }
        match crate::split::cmux_open(&workdir, &path) {
            Ok(()) => self.toast(format!("opened {path} in cmux"), false),
            Err(e) => self.toast(format!("{e:#}"), true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_save_round_trip_keeps_crlf() {
        let dir = tempfile_dir();
        std::fs::write(dir.join("a.txt"), "one\r\ntwo\r\n").unwrap();
        let mut ed = Editor::open(&dir, "a.txt").unwrap();
        assert_eq!(ed.text, "one\ntwo\n");
        assert!(!ed.dirty());
        ed.text.push_str("three\n");
        assert!(ed.dirty());
        ed.save().unwrap();
        assert!(!ed.dirty());
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "one\r\ntwo\r\nthree\r\n");
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

    #[test]
    fn gutter_pads_to_width() {
        let dir = tempfile_dir();
        std::fs::write(dir.join("c.rs"), "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n").unwrap();
        let mut ed = Editor::open(&dir, "c.rs").unwrap();
        assert_eq!(ed.lang, Lang::Rust);
        assert_eq!(ed.line_count(), 11);
        assert!(ed.gutter_text().starts_with(" 1\n 2\n"));
        assert!(ed.gutter_text().ends_with("\n11"));
        std::fs::remove_dir_all(&dir).ok();
    }

    fn tempfile_dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("gitgui-editor-{}-{}", std::process::id(), rand_suffix()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn rand_suffix() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64
    }
}
