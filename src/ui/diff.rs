//! Diff viewer: a custom widget that draws only the visible rows, with hunk
//! buttons, line selection for line-level staging and search highlights.

use iced_core::mouse;
use iced_core::text;
use iced_core::Renderer as _;
use iced_core::widget::{tree, Tree};
use iced_core::{layout, renderer, Color, Element as CoreElement, Event, Font, Length, Pixels, Point, Rectangle, Shell, Size, Widget};
use iced_widget::{column, container, row, text as text_widget, text_input, Space};

use crate::git::repo::{DiffLine, DiffText, DiffTarget, FileKind};
use crate::ui::app::{App, Element, HunkAction, Message, Pane, Renderer};
use crate::ui::log::{draw_text, fill, measure};
use crate::ui::theme::alpha;
use crate::ui::widgets::{self, small_button};

pub const ROW_H: f32 = 20.0;
const BUTTON_W: f32 = 96.0;

enum Row<'a> {
    Hunk(usize, &'a str),
    Line(usize, usize, &'a DiffLine),
}

fn flatten(d: &DiffText) -> Vec<Row<'_>> {
    let mut rows = Vec::new();
    for (i, h) in d.hunks.iter().enumerate() {
        rows.push(Row::Hunk(i, h.header.as_str()));
        for (j, l) in h.lines.iter().enumerate() {
            rows.push(Row::Line(i, j, l));
        }
    }
    rows
}

fn row_text<'a>(row: &Row<'a>) -> &'a str {
    match row {
        Row::Hunk(_, h) => h,
        Row::Line(_, _, l) => l.text.as_str(),
    }
}

fn matches(rows: &[Row<'_>], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return Vec::new();
    }
    let q = query.to_lowercase();
    rows.iter()
        .enumerate()
        .filter(|(_, r)| row_text(r).to_lowercase().contains(&q))
        .map(|(i, _)| i)
        .collect()
}

pub fn match_count(app: &App) -> usize {
    match app.diff.as_ref() {
        Some(d) => matches(&flatten(d), &app.diff_search).len(),
        None => 0,
    }
}

pub fn view(app: &App) -> Element<'_> {
    let t = &app.theme;
    let busy = app.busy > 0;
    let selected = app.selected_file.clone();
    let mut header = row![].spacing(6).align_y(iced_core::Alignment::Center).padding([4, 6]);
    match &selected {
        Some(target) => {
            header = header.push(text_widget(target.path().to_owned()).size(13).font(Font::MONOSPACE).color(t.strong));
            let what = match target {
                DiffTarget::WorkdirUnstaged(_) => "unstaged",
                DiffTarget::Staged(_) => "staged",
                DiffTarget::Commit(..) => "commit",
            };
            header = header.push(text_widget(what).size(12).color(t.weak));
        }
        None => {
            header = header.push(text_widget("no file selected").size(12).color(t.weak));
        }
    }
    header = header.push(Space::new().width(Length::Fill));
    if app.has_line_selection() {
        let n = app
            .line_sel
            .map(|s| {
                let (a, b) = s.range();
                b - a + 1
            })
            .unwrap_or(0);
        match app.line_selection_side() {
            Some(true) => {
                header = header.push(small_button(format!("Stage {n} lines"), (!busy).then_some(Message::LinesStage)));
                header = header.push(small_button(format!("Discard {n} lines"), (!busy).then_some(Message::LinesDiscard)));
            }
            Some(false) => {
                header = header.push(small_button(format!("Unstage {n} lines"), (!busy).then_some(Message::LinesUnstage)));
            }
            None => {}
        }
        header = header.push(small_button("clear", Some(Message::ClearLineSel)));
    }
    let conflicted = selected
        .as_ref()
        .is_some_and(|t| app.snapshot.conflicted.iter().any(|f| f.path == t.path()));
    if conflicted {
        if let Some(p) = selected.as_ref().map(|t| t.path().to_owned()) {
            header = header.push(small_button(
                "ours",
                (!busy).then_some(Message::Resolve(p.clone(), crate::git::actions::ConflictSide::Ours)),
            ));
            header = header.push(small_button(
                "theirs",
                (!busy).then_some(Message::Resolve(p.clone(), crate::git::actions::ConflictSide::Theirs)),
            ));
            header = header.push(small_button(
                "resolved",
                (!busy).then_some(Message::Run(crate::git::ops::Command::Stage(vec![p]))),
            ));
        }
    }
    header = header.push(small_button("find", Some(Message::DiffSearchOpen)));
    header = header.push(small_button("-", (app.diff_opts.context > 0).then_some(Message::DiffContext(-1))));
    header = header.push(text_widget(app.diff_opts.context.to_string()).size(12).color(t.weak));
    header = header.push(small_button("+", Some(Message::DiffContext(1))));
    header = header.push(small_button(
        if app.diff_opts.ignore_whitespace { "ws off" } else { "ws" },
        Some(Message::DiffWhitespace),
    ));

    let mut col = column![header].spacing(0);
    if app.diff_search_active {
        let total = match_count(app);
        let mut bar = row![
            text_input("search in the diff", &app.diff_search)
                .id(widgets::DIFF_SEARCH_ID.clone())
                .on_input(Message::DiffSearch)
                .on_submit(Message::DiffNext(1))
                .size(12)
                .padding([3, 8])
                .style(widgets::text_input_style)
                .width(Length::Fill),
        ]
        .spacing(6)
        .align_y(iced_core::Alignment::Center)
        .padding([2, 6]);
        let pos = if total == 0 {
            "no matches".to_owned()
        } else {
            format!("{} / {total}", app.diff_match.min(total.saturating_sub(1)) + 1)
        };
        bar = bar.push(text_widget(pos).size(12).color(t.weak));
        bar = bar.push(small_button("prev", (total > 0).then_some(Message::DiffNext(-1))));
        bar = bar.push(small_button("next", (total > 0).then_some(Message::DiffNext(1))));
        bar = bar.push(small_button("x", Some(Message::DiffSearchClose)));
        col = col.push(bar);
    }
    let body: Element<'_> = match &app.diff {
        None => {
            let msg = if app.diff_loading { "loading diff" } else { "" };
            container(text_widget(msg).size(12).color(t.weak)).padding(8).into()
        }
        Some(d) => {
            if d.binary {
                container(text_widget("binary file").size(12).color(t.weak)).padding(8).into()
            } else if d.too_large {
                container(text_widget("file too large to diff (over 2 MB)").size(12).color(t.error))
                    .padding(8)
                    .into()
            } else if d.hunks.is_empty() {
                let label = match d.status {
                    FileKind::Untracked | FileKind::Added => "empty file",
                    _ => "no changes",
                };
                container(text_widget(label).size(12).color(t.weak)).padding(8).into()
            } else {
                CoreElement::new(DiffView { app, diff: d })
            }
        }
    };
    col.push(container(body).width(Length::Fill).height(Length::Fill)).into()
}

struct DiffView<'a> {
    app: &'a App,
    diff: &'a DiffText,
}

#[derive(Default)]
struct State {
    scroll: f32,
    scroll_x: f32,
    dragging: bool,
}

/// Which hunk buttons apply: (unstaged?, path) for working tree diffs.
fn hunk_actions(d: &DiffText) -> Option<bool> {
    match &d.target {
        DiffTarget::WorkdirUnstaged(_) => Some(true),
        DiffTarget::Staged(_) => Some(false),
        DiffTarget::Commit(..) => None,
    }
}

struct Geometry {
    rows: usize,
    char_w: f32,
    gutter: f32,
}

impl DiffView<'_> {
    fn geometry(&self, renderer: &Renderer) -> Geometry {
        let size = Pixels(text::Renderer::default_size(renderer).0 - 0.5);
        let char_w = measure("0", Font::MONOSPACE, size).max(1.0);
        let max_no = self
            .diff
            .hunks
            .iter()
            .flat_map(|h| h.lines.iter())
            .flat_map(|l| [l.old_no.unwrap_or(0), l.new_no.unwrap_or(0)])
            .max()
            .unwrap_or(1);
        let digits = max_no.max(1).to_string().len().max(2) as f32;
        let rows = self.diff.hunks.iter().map(|h| h.lines.len() + 1).sum();
        Geometry {
            rows,
            char_w,
            gutter: (digits * 2.0 + 3.0) * char_w + 12.0,
        }
    }

    fn row_at(&self, state: &State, bounds: Rectangle, p: Point) -> Option<usize> {
        let i = ((p.y + state.scroll) / ROW_H).floor();
        if i < 0.0 {
            return None;
        }
        let i = i as usize;
        let _ = bounds;
        Some(i)
    }

    /// Button rects on a hunk header row, right-aligned: (rect, action).
    fn hunk_buttons(&self, bounds: Rectangle, y: f32) -> Vec<(Rectangle, HunkAction)> {
        let Some(unstaged) = hunk_actions(self.diff) else { return Vec::new() };
        let busy = self.app.busy > 0;
        if busy {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut x = bounds.x + bounds.width - 6.0;
        let mut push = |x: &mut f32, action: HunkAction| {
            *x -= BUTTON_W;
            out.push((Rectangle::new(Point::new(*x, y + 2.0), Size::new(BUTTON_W, ROW_H - 4.0)), action));
            *x -= 6.0;
        };
        if unstaged {
            push(&mut x, HunkAction::Stage);
            push(&mut x, HunkAction::Discard);
        } else {
            push(&mut x, HunkAction::Unstage);
        }
        out
    }
}

impl Widget<Message, iced_core::Theme, Renderer> for DiffView<'_> {
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
        let geo = self.geometry(renderer);
        let max = (geo.rows as f32 * ROW_H - bounds.height).max(0.0);
        let rows = flatten(self.diff);
        match event {
            Event::Window(iced_core::window::Event::RedrawRequested(_)) => {
                if self.app.diff_jump.get() {
                    let m = matches(&rows, &self.app.diff_search);
                    if let Some(i) = m.get(self.app.diff_match.min(m.len().saturating_sub(1))) {
                        let top = *i as f32 * ROW_H;
                        state.scroll = (top - bounds.height / 2.0).max(0.0);
                        shell.request_redraw();
                    }
                    self.app.diff_jump.set(false);
                }
                state.scroll = state.scroll.clamp(0.0, max);
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                if cursor.is_over(bounds) {
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
                let Some(p) = cursor.position_in(bounds) else { return };
                let Some(i) = self.row_at(state, bounds, p) else { return };
                let Some(r) = rows.get(i) else { return };
                shell.capture_event();
                match r {
                    Row::Hunk(hunk, _) => {
                        let y = bounds.y + i as f32 * ROW_H - state.scroll;
                        let abs = Point::new(p.x + bounds.x, p.y + bounds.y);
                        for (rect, action) in self.hunk_buttons(bounds, y) {
                            if rect.contains(abs) {
                                shell.publish(Message::DiffHunk(action, *hunk));
                                return;
                            }
                        }
                    }
                    Row::Line(hunk, line, _) => {
                        if hunk_actions(self.diff).is_some() {
                            shell.publish(Message::DiffLineClick {
                                hunk: *hunk,
                                line: *line,
                                shift: self.app.modifiers.shift(),
                            });
                            state.dragging = true;
                        }
                    }
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if state.dragging {
                    if let Some(p) = cursor.position_in(bounds) {
                        if let Some(i) = self.row_at(state, bounds, p) {
                            if let Some(Row::Line(hunk, line, _)) = rows.get(i) {
                                shell.publish(Message::DiffDragTo {
                                    hunk: *hunk,
                                    line: *line,
                                });
                            }
                        }
                    }
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                state.dragging = false;
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
        if cursor.is_over(layout.bounds()) && hunk_actions(self.diff).is_some() {
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
        let app = self.app;
        let t = &app.theme;
        let geo = self.geometry(renderer);
        let size = Pixels(text::Renderer::default_size(renderer).0 - 0.5);
        let mono = Font::MONOSPACE;
        let rows = flatten(self.diff);
        let match_rows = matches(&rows, &app.diff_search);
        let current_match = match_rows.get(app.diff_match.min(match_rows.len().saturating_sub(1))).copied();
        let first = (state.scroll / ROW_H).floor() as usize;
        let last = ((state.scroll + bounds.height) / ROW_H).ceil() as usize;
        let focused = app.focus == Pane::Detail;
        let digits = ((geo.gutter - 12.0) / geo.char_w - 3.0) / 2.0;
        let digits = digits.round() as usize;

        renderer.with_layer(bounds, |renderer| {
            fill(renderer, bounds, t.well, 0.0);
            for (i, row) in rows.iter().enumerate().take(last.min(rows.len())).skip(first) {
                let y = bounds.y + i as f32 * ROW_H - state.scroll;
                let full = Rectangle::new(Point::new(bounds.x, y), Size::new(bounds.width, ROW_H));
                let Some(rect) = full.intersection(&bounds) else { continue };
                let partial = rect.height < ROW_H - 0.5;
                let cy = full.center_y();
                match row {
                    Row::Hunk(hunk, header) => {
                        fill(renderer, rect, t.hunk_bg, 0.0);
                        let buttons = self.hunk_buttons(bounds, y);
                        let text_w = buttons
                            .iter()
                            .map(|(r, _)| r.x)
                            .fold(bounds.x + bounds.width, f32::min)
                            - bounds.x
                            - 8.0;
                        let clip = Rectangle::new(Point::new(bounds.x, rect.y), Size::new(text_w.max(20.0), rect.height));
                        let overflow = partial || state.scroll_x > 0.0 || header.chars().count() as f32 * geo.char_w + 8.0 > clip.width;
                        draw_text(renderer, header.to_string(), Point::new(bounds.x + 8.0 - state.scroll_x, cy), mono, size, t.hunk_fg, clip, overflow);
                        for (brect, action) in buttons {
                            let Some(brect) = brect.intersection(&bounds) else { continue };
                            let hovered = cursor.is_over(brect);
                            fill(
                                renderer,
                                brect,
                                if hovered { alpha(t.accent, 0.35) } else { alpha(t.accent, 0.18) },
                                4.0,
                            );
                            let label = match action {
                                HunkAction::Stage => "Stage hunk",
                                HunkAction::Unstage => "Unstage hunk",
                                HunkAction::Discard => "Discard hunk",
                            };
                            let w = measure(label, text::Renderer::default_font(renderer), Pixels(size.0 - 1.0));
                            draw_text(
                                renderer,
                                label.to_owned(),
                                Point::new(brect.center_x() - w / 2.0, brect.center_y()),
                                text::Renderer::default_font(renderer),
                                Pixels(size.0 - 1.0),
                                t.strong,
                                brect,
                                partial,
                            );
                        }
                        let _ = hunk;
                    }
                    Row::Line(hunk, line, l) => {
                        let (bg, fg) = match l.origin {
                            '+' => (Some(t.add_bg), t.add_fg),
                            '-' => (Some(t.del_bg), t.del_fg),
                            _ => (None, t.text),
                        };
                        if let Some(bg) = bg {
                            fill(renderer, rect, bg, 0.0);
                        }
                        if app.line_sel.is_some_and(|s| s.contains(*hunk, *line)) {
                            fill(
                                renderer,
                                rect,
                                if focused { alpha(t.selection, 0.85) } else { alpha(t.selection_inactive, 0.85) },
                                0.0,
                            );
                        }
                        if match_rows.binary_search(&i).is_ok() {
                            fill(
                                renderer,
                                rect,
                                if current_match == Some(i) { alpha(t.warn, 0.35) } else { alpha(t.warn, 0.15) },
                                0.0,
                            );
                        }
                        let gutter_rect = Rectangle::new(Point::new(bounds.x, rect.y), Size::new(geo.gutter, rect.height));
                        let old = l.old_no.map(|n| n.to_string()).unwrap_or_default();
                        let new = l.new_no.map(|n| n.to_string()).unwrap_or_default();
                        let numbers = format!("{old:>digits$} {new:>digits$} {}", l.origin);
                        draw_text(renderer, numbers, Point::new(bounds.x + 6.0, cy), mono, size, t.line_no, gutter_rect, partial);
                        let text_clip = Rectangle::new(
                            Point::new(bounds.x + geo.gutter, rect.y),
                            Size::new((bounds.width - geo.gutter).max(0.0), rect.height),
                        );
                        let content = if l.no_newline {
                            format!("{} \\ No newline at end of file", l.text)
                        } else {
                            l.text.clone()
                        };
                        let overflow = partial || state.scroll_x > 0.0 || content.chars().count() as f32 * geo.char_w > text_clip.width;
                        draw_text(renderer, content, Point::new(bounds.x + geo.gutter - state.scroll_x, cy), mono, size, fg, text_clip, overflow);
                    }
                }
            }
            // Scrollbar hint.
            if geo.rows as f32 * ROW_H > bounds.height {
                let total = geo.rows as f32 * ROW_H;
                let frac = bounds.height / total;
                let h = (bounds.height * frac).max(16.0);
                let y = bounds.y + (state.scroll / total) * bounds.height;
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
fn unused(_: Color) {}
