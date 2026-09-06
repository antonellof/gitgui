//! Three-way conflict resolver: ours | result | theirs, one shared scroll,
//! per-conflict accept buttons in the gutters, accept-all and apply in the
//! header. The result is built from the choices; Apply writes it to the
//! working tree and stages the file (git's "resolved").

use std::path::{Path, PathBuf};

use iced_core::mouse;
use iced_core::text;
use iced_core::widget::{tree, Tree};
use iced_core::Renderer as _;
use iced_core::{layout, renderer, Color, Element as CoreElement, Event, Font, Length, Pixels, Point, Rectangle, Shell, Size, Widget};
use iced_widget::{column, container, row, text as text_widget, Space};

use crate::ui::app::{App, Element, Message, Renderer};
use crate::ui::log::{draw_text, fill, measure};
use crate::ui::theme::alpha;
use crate::ui::widgets::{self, primary_button, small_button};

pub const ROW_H: f32 = 20.0;
const GUTTER_W: f32 = 58.0;
const HEADER_H: f32 = 24.0;
const OURS: Color = Color::from_rgb(0.29, 0.47, 0.78);
const THEIRS: Color = Color::from_rgb(0.62, 0.42, 0.78);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Ours,
    Theirs,
    /// Ours then theirs.
    Both,
    /// Drop both sides.
    Neither,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Common(Vec<String>),
    Conflict {
        ours: Vec<String>,
        base: Option<Vec<String>>,
        theirs: Vec<String>,
        resolution: Option<Resolution>,
    },
}

#[derive(Debug, Clone)]
pub struct MergeState {
    pub path: String,
    pub full: PathBuf,
    pub ours_label: String,
    pub theirs_label: String,
    pub segments: Vec<Segment>,
    trailing_newline: bool,
}

impl MergeState {
    pub fn open(workdir: &Path, path: &str, head: &str) -> Result<MergeState, String> {
        let full = workdir.join(path);
        let bytes = std::fs::read(&full).map_err(|e| format!("{path}: {e}"))?;
        if bytes.contains(&0) {
            return Err(format!("{path} is a binary file, pick a side from the file menu"));
        }
        let text = String::from_utf8_lossy(&bytes);
        let (segments, ours_label, theirs_label) = parse(&text);
        if !segments.iter().any(|s| matches!(s, Segment::Conflict { .. })) {
            return Err(format!("{path} has no conflict markers left, mark it resolved"));
        }
        let ours_label = if ours_label.is_empty() || ours_label == "HEAD" {
            head.to_owned()
        } else {
            ours_label
        };
        Ok(MergeState {
            path: path.to_owned(),
            full,
            ours_label,
            theirs_label: if theirs_label.is_empty() { "incoming".into() } else { theirs_label },
            segments,
            trailing_newline: text.ends_with('\n'),
        })
    }

    pub fn conflicts(&self) -> usize {
        self.segments.iter().filter(|s| matches!(s, Segment::Conflict { .. })).count()
    }

    pub fn resolved(&self) -> usize {
        self.segments
            .iter()
            .filter(|s| matches!(s, Segment::Conflict { resolution: Some(_), .. }))
            .count()
    }

    pub fn set(&mut self, conflict: usize, resolution: Option<Resolution>) {
        let mut i = 0;
        for s in &mut self.segments {
            if let Segment::Conflict { resolution: r, .. } = s {
                if i == conflict {
                    *r = resolution;
                    return;
                }
                i += 1;
            }
        }
    }

    pub fn set_all(&mut self, resolution: Resolution) {
        for s in &mut self.segments {
            if let Segment::Conflict { resolution: r, .. } = s {
                *r = Some(resolution);
            }
        }
    }

    /// Lines of the merged file; unresolved conflicts keep their markers so
    /// an edit in the editor still shows them.
    pub fn result_lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        for s in &self.segments {
            match s {
                Segment::Common(lines) => out.extend(lines.iter().cloned()),
                Segment::Conflict {
                    ours,
                    base,
                    theirs,
                    resolution,
                } => match resolution {
                    Some(Resolution::Ours) => out.extend(ours.iter().cloned()),
                    Some(Resolution::Theirs) => out.extend(theirs.iter().cloned()),
                    Some(Resolution::Both) => {
                        out.extend(ours.iter().cloned());
                        out.extend(theirs.iter().cloned());
                    }
                    Some(Resolution::Neither) => {}
                    None => {
                        out.push(format!("<<<<<<< {}", self.ours_label));
                        out.extend(ours.iter().cloned());
                        if let Some(b) = base {
                            out.push("||||||| base".to_owned());
                            out.extend(b.iter().cloned());
                        }
                        out.push("=======".to_owned());
                        out.extend(theirs.iter().cloned());
                        out.push(format!(">>>>>>> {}", self.theirs_label));
                    }
                },
            }
        }
        out
    }

    pub fn result_text(&self) -> String {
        let mut s = self.result_lines().join("\n");
        if self.trailing_newline || s.is_empty() {
            s.push('\n');
        }
        s
    }

    pub fn write(&self) -> Result<(), String> {
        std::fs::write(&self.full, self.result_text()).map_err(|e| format!("{}: {e}", self.path))
    }
}

/// Split a file with conflict markers into segments; returns the labels of
/// the first `<<<<<<<` and `>>>>>>>` markers.
pub fn parse(text: &str) -> (Vec<Segment>, String, String) {
    let mut segments = Vec::new();
    let mut common: Vec<String> = Vec::new();
    let mut ours_label = String::new();
    let mut theirs_label = String::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if let Some(label) = line.strip_prefix("<<<<<<< ") {
            if ours_label.is_empty() {
                ours_label = label.trim().to_owned();
            }
            if !common.is_empty() {
                segments.push(Segment::Common(std::mem::take(&mut common)));
            }
            let mut ours = Vec::new();
            let mut base: Option<Vec<String>> = None;
            let mut theirs = Vec::new();
            let mut side = 0; // 0 ours, 1 base, 2 theirs
            let mut closed = false;
            for l in lines.by_ref() {
                if side == 0 && l.starts_with("||||||| ") {
                    side = 1;
                    base = Some(Vec::new());
                } else if side < 2 && l.starts_with("=======") && l.trim_end() == "=======" {
                    side = 2;
                } else if side == 2 && l.starts_with(">>>>>>> ") {
                    if theirs_label.is_empty() {
                        theirs_label = l[8..].trim().to_owned();
                    }
                    closed = true;
                    break;
                } else {
                    match side {
                        0 => ours.push(l.to_owned()),
                        1 => base.get_or_insert_with(Vec::new).push(l.to_owned()),
                        _ => theirs.push(l.to_owned()),
                    }
                }
            }
            if !closed {
                // Unterminated marker block: keep the text as it is.
                common.push(format!("<<<<<<< {ours_label}"));
                common.extend(ours);
                if let Some(b) = base {
                    common.push("||||||| base".to_owned());
                    common.extend(b);
                }
                if side == 2 {
                    common.push("=======".to_owned());
                }
                common.extend(theirs);
                continue;
            }
            segments.push(Segment::Conflict {
                ours,
                base,
                theirs,
                resolution: None,
            });
        } else {
            common.push(line.to_owned());
        }
    }
    if !common.is_empty() {
        segments.push(Segment::Common(common));
    }
    (segments, ours_label, theirs_label)
}

// ---- view ----

pub fn view(app: &App) -> Element<'_> {
    let t = &app.theme;
    let Some(m) = app.merge.as_ref() else {
        return text_widget("").into();
    };
    let busy = app.busy > 0;
    let total = m.conflicts();
    let done = m.resolved();
    let all = done == total;
    let header = row![
        text_widget(&m.path).size(13).font(Font::MONOSPACE).color(t.strong),
        text_widget(format!("{done} of {total} conflict{} resolved", if total == 1 { "" } else { "s" }))
            .size(12)
            .color(if all { t.ok } else { t.warn }),
        Space::new().width(Length::Fill),
        primary_button("Apply", (!busy && all).then_some(Message::MergeApply)),
        small_button("Close", Some(Message::MergeClose)),
    ]
    .spacing(6)
    .align_y(iced_core::Alignment::Center)
    .padding([4, 6]);
    let tools = row![
        small_button("All ours", (!busy).then_some(Message::MergeAll(Resolution::Ours))),
        small_button("All theirs", (!busy).then_some(Message::MergeAll(Resolution::Theirs))),
        small_button("Edit result", (!busy).then_some(Message::MergeEdit)),
        container(
            text_widget("» left side, « right side, + both, x neither, u undo. Apply writes the result and marks the file resolved.")
                .size(11)
                .color(t.weak)
                .wrapping(iced_core::text::Wrapping::None)
        )
        .width(Length::Fill)
        .clip(true),
    ]
    .spacing(6)
    .align_y(iced_core::Alignment::Center)
    .padding([0, 6]);
    let body: Element<'_> = CoreElement::new(MergeView { app, m });
    column![header, tools, container(body).width(Length::Fill).height(Length::Fill)]
        .spacing(4)
        .into()
}

struct MergeView<'a> {
    app: &'a App,
    m: &'a MergeState,
}

#[derive(Default)]
struct State {
    scroll: f32,
    scroll_x: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Common,
    /// (conflict index, first row of the block, resolved)
    Conflict(usize, bool, bool),
}

struct Row<'a> {
    ours: Option<(usize, &'a str)>,
    result: Option<(usize, &'a str)>,
    theirs: Option<(usize, &'a str)>,
    kind: Kind,
}

/// Row model: common segments line up in all three columns, a conflict
/// block spans the longest side.
fn rows(m: &MergeState) -> Vec<Row<'_>> {
    let mut out = Vec::new();
    let (mut on, mut rn, mut tn) = (1usize, 1usize, 1usize);
    let mut ci = 0;
    for seg in &m.segments {
        match seg {
            Segment::Common(lines) => {
                for l in lines {
                    out.push(Row {
                        ours: Some((on, l.as_str())),
                        result: Some((rn, l.as_str())),
                        theirs: Some((tn, l.as_str())),
                        kind: Kind::Common,
                    });
                    on += 1;
                    rn += 1;
                    tn += 1;
                }
            }
            Segment::Conflict {
                ours,
                theirs,
                resolution,
                ..
            } => {
                let result: Vec<&String> = match resolution {
                    Some(Resolution::Ours) => ours.iter().collect(),
                    Some(Resolution::Theirs) => theirs.iter().collect(),
                    Some(Resolution::Both) => ours.iter().chain(theirs.iter()).collect(),
                    Some(Resolution::Neither) | None => Vec::new(),
                };
                let h = ours.len().max(theirs.len()).max(result.len()).max(1);
                for k in 0..h {
                    out.push(Row {
                        ours: ours.get(k).map(|l| (on + k, l.as_str())),
                        result: result.get(k).map(|l| (rn + k, l.as_str())),
                        theirs: theirs.get(k).map(|l| (tn + k, l.as_str())),
                        kind: Kind::Conflict(ci, k == 0, resolution.is_some()),
                    });
                }
                on += ours.len();
                tn += theirs.len();
                rn += result.len();
                ci += 1;
            }
        }
    }
    out
}

struct Geometry {
    col_w: f32,
    char_w: f32,
    digits: usize,
}

impl MergeView<'_> {
    fn geometry(&self, renderer: &Renderer, bounds: Rectangle) -> Geometry {
        let size = Pixels(text::Renderer::default_size(renderer).0 - 0.5);
        let char_w = measure("0", Font::MONOSPACE, size).max(1.0);
        let lines = self.m.result_lines().len().max(self.m.segments.iter().map(|s| match s {
            Segment::Common(l) => l.len(),
            Segment::Conflict { ours, theirs, .. } => ours.len().max(theirs.len()),
        }).sum::<usize>());
        let digits = lines.max(1).to_string().len().max(2);
        Geometry {
            col_w: ((bounds.width - 2.0 * GUTTER_W) / 3.0).max(40.0),
            char_w,
            digits,
        }
    }

    /// Gutter buttons for a conflict block starting at row y: (rect, label, message).
    fn buttons(&self, bounds: Rectangle, geo: &Geometry, y: f32, conflict: usize, resolved: bool) -> Vec<(Rectangle, &'static str, Message)> {
        let mut out = Vec::new();
        let bw = (GUTTER_W - 9.0) / 2.0;
        let size = Size::new(bw, ROW_H - 4.0);
        let left_x = bounds.x + geo.col_w + 3.0;
        let right_x = bounds.x + 2.0 * geo.col_w + GUTTER_W + 3.0;
        if resolved {
            out.push((Rectangle::new(Point::new(left_x, y + 2.0), size), "u", Message::MergeSet(conflict, None)));
            out.push((Rectangle::new(Point::new(right_x, y + 2.0), size), "u", Message::MergeSet(conflict, None)));
            return out;
        }
        out.push((Rectangle::new(Point::new(left_x, y + 2.0), size), "»", Message::MergeSet(conflict, Some(Resolution::Ours))));
        out.push((Rectangle::new(Point::new(left_x + bw + 3.0, y + 2.0), size), "+", Message::MergeSet(conflict, Some(Resolution::Both))));
        out.push((Rectangle::new(Point::new(right_x, y + 2.0), size), "«", Message::MergeSet(conflict, Some(Resolution::Theirs))));
        out.push((Rectangle::new(Point::new(right_x + bw + 3.0, y + 2.0), size), "x", Message::MergeSet(conflict, Some(Resolution::Neither))));
        out
    }
}

impl Widget<Message, iced_core::Theme, Renderer> for MergeView<'_> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn layout(&mut self, _tree: &mut Tree, _renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        layout::Node::new(limits.max())
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: layout::Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        _clipboard: &mut dyn iced_core::Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State>();
        let bounds = layout.bounds();
        let rows = rows(self.m);
        let max = (rows.len() as f32 * ROW_H + HEADER_H - bounds.height).max(0.0);
        match event {
            Event::Window(iced_core::window::Event::RedrawRequested(_)) => {
                state.scroll = state.scroll.clamp(0.0, max);
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                if cursor.is_over(bounds) {
                    let geo = self.geometry(renderer, bounds);
                    let (dx, dy) = match delta {
                        mouse::ScrollDelta::Lines { x, y } => (-x * geo.char_w * 8.0, -y * ROW_H * 3.0),
                        mouse::ScrollDelta::Pixels { x, y } => (-x, -y),
                    };
                    if self.app.modifiers.shift() {
                        state.scroll_x = (state.scroll_x + dy).max(0.0);
                    } else {
                        state.scroll = (state.scroll + dy).clamp(0.0, max);
                        state.scroll_x = (state.scroll_x + dx).max(0.0);
                    }
                    shell.capture_event();
                    shell.request_redraw();
                }
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(p) = cursor.position() else { return };
                if !bounds.contains(p) {
                    return;
                }
                let geo = self.geometry(renderer, bounds);
                for (i, r) in rows.iter().enumerate() {
                    if let Kind::Conflict(ci, true, resolved) = r.kind {
                        let y = bounds.y + HEADER_H + i as f32 * ROW_H - state.scroll;
                        for (rect, _, msg) in self.buttons(bounds, &geo, y, ci, resolved) {
                            if rect.contains(p) {
                                shell.publish(msg);
                                shell.capture_event();
                                return;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: layout::Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        if cursor.is_over(layout.bounds()) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::None
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        _theme: &iced_core::Theme,
        _style: &renderer::Style,
        layout: layout::Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State>();
        let bounds = layout.bounds();
        let t = &self.app.theme;
        let geo = self.geometry(renderer, bounds);
        let size = Pixels(text::Renderer::default_size(renderer).0 - 0.5);
        let mono = Font::MONOSPACE;
        let ui_font = text::Renderer::default_font(renderer);
        let rows = rows(self.m);
        let cols = [
            (bounds.x, format!("Ours: {}", self.m.ours_label), OURS),
            (bounds.x + geo.col_w + GUTTER_W, "Result".to_owned(), t.ok),
            (bounds.x + 2.0 * (geo.col_w + GUTTER_W), format!("Theirs: {}", self.m.theirs_label), THEIRS),
        ];
        let gutter_text_w = (geo.digits as f32 + 1.0) * geo.char_w + 6.0;
        let first = (state.scroll / ROW_H).floor() as usize;
        let body_top = bounds.y + HEADER_H;
        let body = Rectangle::new(Point::new(bounds.x, body_top), Size::new(bounds.width, (bounds.height - HEADER_H).max(0.0)));
        let last = ((state.scroll + body.height) / ROW_H).ceil() as usize;

        renderer.with_layer(bounds, |renderer| {
            fill(renderer, bounds, t.well, 0.0);
            // Column headers.
            for (x, label, color) in &cols {
                let r = Rectangle::new(Point::new(*x, bounds.y), Size::new(geo.col_w, HEADER_H));
                fill(renderer, r, alpha(*color, 0.25), 0.0);
                let label = crate::ui::log::fit(label, geo.col_w - 12.0, ui_font, Pixels(size.0 - 1.0));
                draw_text(renderer, label, Point::new(x + 6.0, r.center_y()), ui_font, Pixels(size.0 - 1.0), t.strong, r, false);
            }
            for (i, r) in rows.iter().enumerate().take(last.min(rows.len())).skip(first) {
                let y = body_top + i as f32 * ROW_H - state.scroll;
                let full = Rectangle::new(Point::new(bounds.x, y), Size::new(bounds.width, ROW_H));
                let Some(rect) = full.intersection(&body) else { continue };
                let partial = rect.height < ROW_H - 0.5;
                let cy = full.center_y();
                let (conflict, resolved) = match r.kind {
                    Kind::Common => (None, false),
                    Kind::Conflict(ci, _, res) => (Some(ci), res),
                };
                for (c, (x, _, color)) in cols.iter().enumerate() {
                    let cell = Rectangle::new(Point::new(*x, rect.y), Size::new(geo.col_w, rect.height));
                    let content = match c {
                        0 => r.ours,
                        1 => r.result,
                        _ => r.theirs,
                    };
                    if conflict.is_some() {
                        let bg = match c {
                            1 if resolved => alpha(t.ok, 0.18),
                            1 => alpha(t.error, 0.15),
                            _ if content.is_some() => alpha(*color, 0.22),
                            _ => alpha(*color, 0.08),
                        };
                        fill(renderer, cell, bg, 0.0);
                    }
                    if let Some((no, line)) = content {
                        let num_rect = Rectangle::new(cell.position(), Size::new(gutter_text_w, cell.height));
                        draw_text(
                            renderer,
                            format!("{no:>w$}", w = geo.digits),
                            Point::new(cell.x + 4.0, cy),
                            mono,
                            size,
                            t.line_no,
                            num_rect,
                            partial,
                        );
                        let text_rect = Rectangle::new(
                            Point::new(cell.x + gutter_text_w, cell.y),
                            Size::new((geo.col_w - gutter_text_w).max(0.0), cell.height),
                        );
                        let overflow = partial || state.scroll_x > 0.0 || line.chars().count() as f32 * geo.char_w > text_rect.width;
                        draw_text(
                            renderer,
                            line.to_owned(),
                            Point::new(text_rect.x - state.scroll_x, cy),
                            mono,
                            size,
                            t.text,
                            text_rect,
                            overflow,
                        );
                    }
                }
                // Column separators.
                for x in [bounds.x + geo.col_w, bounds.x + 2.0 * geo.col_w + GUTTER_W] {
                    fill(renderer, Rectangle::new(Point::new(x, rect.y), Size::new(GUTTER_W, rect.height)), t.panel, 0.0);
                }
                if let Kind::Conflict(ci, true, res) = r.kind {
                    for (brect, label, _) in self.buttons(bounds, &geo, y, ci, res) {
                        let Some(brect) = brect.intersection(&body) else { continue };
                        let hovered = cursor.is_over(brect);
                        fill(renderer, brect, if hovered { alpha(t.accent, 0.5) } else { alpha(t.accent, 0.22) }, 4.0);
                        let w = measure(label, ui_font, size);
                        draw_text(renderer, label.to_owned(), Point::new(brect.center_x() - w / 2.0, brect.center_y()), ui_font, size, t.strong, brect, false);
                    }
                }
            }
            // Scrollbar hint.
            let total = rows.len() as f32 * ROW_H;
            if total > body.height {
                let frac = body.height / total;
                let h = (body.height * frac).max(16.0);
                let y = body.y + (state.scroll / total) * body.height;
                fill(
                    renderer,
                    Rectangle::new(Point::new(bounds.x + bounds.width - 6.0, y), Size::new(4.0, h)),
                    alpha(t.weak, 0.5),
                    2.0,
                );
            }
        });
    }
}

#[allow(dead_code)]
fn unused() -> Element<'static> {
    widgets::button("", None)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "one\n<<<<<<< HEAD\nours a\nours b\n=======\ntheirs a\n>>>>>>> feature\nthree\n";

    #[test]
    fn parses_markers_into_segments() {
        let (segs, ours, theirs) = parse(FILE);
        assert_eq!(ours, "HEAD");
        assert_eq!(theirs, "feature");
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0], Segment::Common(vec!["one".into()]));
        assert!(matches!(&segs[1], Segment::Conflict { ours, theirs, base: None, resolution: None } if ours.len() == 2 && theirs.len() == 1));
        assert_eq!(segs[2], Segment::Common(vec!["three".into()]));
    }

    #[test]
    fn diff3_base_block_is_kept_aside() {
        let (segs, _, _) = parse("<<<<<<< HEAD\na\n||||||| base\nb\n=======\nc\n>>>>>>> x\n");
        assert!(matches!(&segs[0], Segment::Conflict { base: Some(b), .. } if b == &vec!["b".to_string()]));
    }

    #[test]
    fn result_follows_the_choices() {
        let dir = std::env::temp_dir().join(format!("gitgui-merge-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("c.txt"), FILE).unwrap();
        let mut m = MergeState::open(&dir, "c.txt", "main").unwrap();
        assert_eq!(m.ours_label, "main");
        assert_eq!(m.conflicts(), 1);
        assert_eq!(m.resolved(), 0);
        assert!(m.result_text().contains("<<<<<<< main"));
        m.set(0, Some(Resolution::Theirs));
        assert_eq!(m.result_text(), "one\ntheirs a\nthree\n");
        m.set(0, Some(Resolution::Both));
        assert_eq!(m.result_text(), "one\nours a\nours b\ntheirs a\nthree\n");
        m.set_all(Resolution::Neither);
        assert_eq!(m.result_text(), "one\nthree\n");
        m.write().unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("c.txt")).unwrap(), "one\nthree\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rows_line_up_across_columns() {
        let (segments, _, _) = parse(FILE);
        let m = MergeState {
            path: "c".into(),
            full: PathBuf::from("c"),
            ours_label: "a".into(),
            theirs_label: "b".into(),
            segments,
            trailing_newline: true,
        };
        let r = rows(&m);
        // one common, a block of two (longest side), one common.
        assert_eq!(r.len(), 4);
        assert_eq!(r[1].ours.map(|(n, _)| n), Some(2));
        assert_eq!(r[1].theirs.map(|(n, _)| n), Some(2));
        assert!(r[1].result.is_none(), "unresolved result is empty");
        assert!(r[2].theirs.is_none(), "theirs side is shorter");
        assert_eq!(r[3].ours.map(|(n, _)| n), Some(4));
        assert_eq!(r[3].theirs.map(|(n, _)| n), Some(3));
    }
}
