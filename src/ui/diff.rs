//! Diff viewer: a custom widget that draws only the visible rows, with hunk
//! buttons, line selection for line-level staging and search highlights.

use iced_core::mouse;
use iced_core::text;
use iced_core::Renderer as _;
use iced_core::widget::{tree, Tree};
use iced_core::{layout, renderer, Color, Element as CoreElement, Event, Font, Length, Pixels, Point, Rectangle, Shell, Size, Widget};
use iced_widget::{column, container, row, text as text_widget, text_input};

use crate::git::repo::{DiffLine, DiffText, DiffTarget, FileKind};
use crate::ui::app::{App, DiffPos, Element, HunkAction, Message, Pane, Renderer};
use crate::ui::log::{draw_text, draw_text_wrapped, fill, measure};
use crate::ui::theme::alpha;
use crate::ui::widgets::{self, small_button};

pub const ROW_H: f32 = 20.0;
const BUTTON_W: f32 = 96.0;
/// Conflict side tints: ours blue, theirs purple.
const OURS: Color = Color::from_rgb(0.29, 0.47, 0.78);
const THEIRS: Color = Color::from_rgb(0.62, 0.42, 0.78);

enum Row<'a> {
    Hunk(usize, &'a str),
    Line(usize, usize, &'a DiffLine),
}

fn flatten(d: &DiffText) -> Vec<Row<'_>> {
    let mut rows = Vec::new();
    for (i, h) in d.hunks.iter().enumerate() {
        // A conflicted file is one hunk with the whole file; the banner
        // above the view says what the sides are, no header row needed.
        if d.status != FileKind::Conflicted {
            rows.push(Row::Hunk(i, h.header.as_str()));
        }
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
            let what = match target {
                DiffTarget::WorkdirUnstaged(_) => "unstaged",
                DiffTarget::Staged(_) => "staged",
                DiffTarget::Commit(..) => "commit",
            };
            header = header.push(
                container(
                    row![
                        text_widget(target.path().to_owned())
                            .size(13)
                            .font(Font::MONOSPACE)
                            .color(t.strong)
                            .wrapping(iced_core::text::Wrapping::None),
                        text_widget(what).size(12).color(t.weak),
                    ]
                    .spacing(6)
                    .align_y(iced_core::Alignment::Center),
                )
                .width(Length::Fill)
                .clip(true),
            );
        }
        None => {
            header = header.push(text_widget("no file selected").size(12).color(t.weak));
        }
    }
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
    header = header.push(small_button("find", Some(Message::DiffSearchOpen)));
    header = header.push(small_button("-", (app.diff_opts.context > 0).then_some(Message::DiffContext(-1))));
    header = header.push(text_widget(app.diff_opts.context.to_string()).size(12).color(t.weak));
    header = header.push(small_button("+", Some(Message::DiffContext(1))));
    header = header.push(small_button(
        if app.diff_opts.ignore_whitespace { "ws off" } else { "ws" },
        Some(Message::DiffWhitespace),
    ));
    header = header.push(small_button(if app.wrap { "wrap on" } else { "wrap" }, Some(Message::DiffWrap)));

    let mut col = column![header].spacing(0);
    if conflicted {
        if let (Some(p), Some(d)) = (selected.as_ref().map(|t| t.path().to_owned()), app.diff.as_ref()) {
            let (n, ours, theirs) = conflict_info(d);
            let head = app
                .snapshot
                .head
                .as_ref()
                .and_then(|h| h.branch_name.clone())
                .unwrap_or_else(|| "HEAD".into());
            let ours_label = if ours.is_empty() || ours == "HEAD" { head.clone() } else { ours };
            let theirs_label = if theirs.is_empty() { "incoming".to_owned() } else { theirs };
            let summary = format!(
                "{n} conflict{} in this file",
                if n == 1 { "" } else { "s" }
            );
            let sides = row![
                text_widget(summary).size(12).color(t.strong),
                text_widget("·").size(12).color(t.weak),
                container(text_widget(format!("ours: {ours_label}")).size(11).color(t.strong))
                    .padding([1, 6])
                    .style(|_| container::Style {
                        background: Some(iced_core::Background::Color(alpha(OURS, 0.6))),
                        border: iced_core::Border { radius: 4.0.into(), ..Default::default() },
                        ..Default::default()
                    }),
                container(text_widget(format!("theirs: {theirs_label}")).size(11).color(t.strong))
                    .padding([1, 6])
                    .style(|_| container::Style {
                        background: Some(iced_core::Background::Color(alpha(THEIRS, 0.6))),
                        border: iced_core::Border { radius: 4.0.into(), ..Default::default() },
                        ..Default::default()
                    }),
            ]
            .spacing(6)
            .align_y(iced_core::Alignment::Center);
            // Actions on their own row, the important one first, so a narrow
            // pane cuts the hint and never the Resolve button.
            let actions = row![
                widgets::primary_button("Resolve…", (!busy).then_some(Message::MergeOpen(p.clone()))),
                small_button("Use ours", (!busy).then_some(Message::Resolve(p.clone(), crate::git::actions::ConflictSide::Ours))),
                small_button("Use theirs", (!busy).then_some(Message::Resolve(p.clone(), crate::git::actions::ConflictSide::Theirs))),
                small_button("Mark resolved", (!busy).then_some(Message::Run(crate::git::ops::Command::Stage(vec![p.clone()])))),
                container(
                    text_widget("three-way tool, or pick a side for the whole file, or edit and mark resolved")
                        .size(11)
                        .color(t.weak)
                        .wrapping(iced_core::text::Wrapping::None)
                )
                .width(Length::Fill)
                .clip(true),
            ]
            .spacing(6)
            .align_y(iced_core::Alignment::Center);
            let bg = alpha(t.error, 0.12);
            col = col.push(
                container(column![sides, actions].spacing(6))
                    .padding([6, 8])
                    .width(Length::Fill)
                    .style(move |_| container::Style {
                        background: Some(iced_core::Background::Color(bg)),
                        ..Default::default()
                    }),
            );
        }
    }
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
    /// Dragging over the gutter: extending the line selection.
    dragging: bool,
    /// Left button down over the text: (position, where, shift held).
    /// A release without movement is a line click; movement selects text.
    press: Option<(DiffPos, Point, bool)>,
    /// Anchor of a text drag in progress.
    text_drag: Option<DiffPos>,
}

/// Pointer travel before a press over the text becomes a text drag.
const DRAG_SLOP: f32 = 3.0;

/// Conflict summary from the markers: (count, ours label, theirs label).
fn conflict_info(d: &DiffText) -> (usize, String, String) {
    let mut n = 0;
    let mut ours = String::new();
    let mut theirs = String::new();
    for l in d.hunks.iter().flat_map(|h| h.lines.iter()) {
        if let Some(rest) = l.text.strip_prefix("<<<<<<< ") {
            n += 1;
            if ours.is_empty() {
                ours = rest.trim().to_owned();
            }
        } else if let Some(rest) = l.text.strip_prefix(">>>>>>> ") {
            if theirs.is_empty() {
                theirs = rest.trim().to_owned();
            }
        }
    }
    (n, ours, theirs)
}

fn is_marker(text: &str) -> bool {
    text.starts_with("<<<<<<< ") || text.starts_with("=======") || text.starts_with(">>>>>>> ") || text.starts_with("||||||| ")
}

/// Height of a row: wrapped lines take several visual lines.
fn row_height(r: &Row<'_>, geo: &Geometry) -> f32 {
    match r {
        Row::Hunk(..) => ROW_H,
        Row::Line(_, _, l) if geo.cpl > 0 => {
            let n = l.text.chars().count().max(1);
            n.div_ceil(geo.cpl) as f32 * ROW_H
        }
        Row::Line(..) => ROW_H,
    }
}

/// Which hunk buttons apply: (unstaged?, path) for working tree diffs.
fn hunk_actions(d: &DiffText) -> Option<bool> {
    if d.status == FileKind::Conflicted {
        return None;
    }
    match &d.target {
        DiffTarget::WorkdirUnstaged(_) => Some(true),
        DiffTarget::Staged(_) => Some(false),
        DiffTarget::Commit(..) => None,
    }
}

struct Geometry {
    char_w: f32,
    gutter: f32,
    /// Characters that fit on one visual line when wrapping (0 = no wrap).
    cpl: usize,
}

impl DiffView<'_> {
    /// The character position under `p` (relative to the widget), if it is
    /// over a diff line.
    fn pos_at(&self, p: Point, rows: &[Row<'_>], offs: &[f32], geo: &Geometry, state: &State) -> Option<DiffPos> {
        let i = self.row_at_y(offs, p.y + state.scroll)?;
        let Row::Line(hunk, line, l) = rows.get(i)? else { return None };
        let row_top = offs[i] - state.scroll;
        let x = (p.x - geo.gutter + state.scroll_x).max(0.0);
        let base = (x / geo.char_w).round() as usize;
        let n = l.text.chars().count();
        let col = if geo.cpl > 0 {
            let sub = ((p.y - row_top) / ROW_H).floor().max(0.0) as usize;
            sub * geo.cpl + base.min(geo.cpl)
        } else {
            base
        };
        Some(DiffPos {
            hunk: *hunk,
            line: *line,
            col: col.min(n),
        })
    }

    fn geometry(&self, renderer: &Renderer, bounds: Rectangle) -> Geometry {
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
        let gutter = (digits * 2.0 + 3.0) * char_w + 12.0;
        let cpl = if self.app.wrap {
            (((bounds.width - gutter - 8.0) / char_w).floor() as usize).max(8)
        } else {
            0
        };
        Geometry { char_w, gutter, cpl }
    }

    /// Row top offsets (prefix sums), `rows.len() + 1` entries.
    fn offsets(&self, rows: &[Row<'_>], geo: &Geometry) -> Vec<f32> {
        let mut offs = Vec::with_capacity(rows.len() + 1);
        let mut y = 0.0;
        for r in rows {
            offs.push(y);
            y += row_height(r, geo);
        }
        offs.push(y);
        offs
    }

    fn row_at_y(&self, offs: &[f32], y: f32) -> Option<usize> {
        if y < 0.0 || offs.len() < 2 || y >= *offs.last().unwrap() {
            return None;
        }
        let i = offs.partition_point(|o| *o <= y);
        Some(i.saturating_sub(1))
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
        let geo = self.geometry(renderer, bounds);
        let rows = flatten(self.diff);
        let offs = self.offsets(&rows, &geo);
        let total = *offs.last().unwrap_or(&0.0);
        let max = (total - bounds.height).max(0.0);
        if geo.cpl > 0 {
            state.scroll_x = 0.0;
        }
        match event {
            Event::Window(iced_core::window::Event::RedrawRequested(_)) => {
                if self.app.diff_jump.get() {
                    let m = matches(&rows, &self.app.diff_search);
                    if let Some(i) = m.get(self.app.diff_match.min(m.len().saturating_sub(1))) {
                        let top = offs[*i];
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
                let Some(i) = self.row_at_y(&offs, p.y + state.scroll) else { return };
                let Some(r) = rows.get(i) else { return };
                shell.capture_event();
                match r {
                    Row::Hunk(hunk, _) => {
                        let y = bounds.y + offs[i] - state.scroll;
                        let abs = Point::new(p.x + bounds.x, p.y + bounds.y);
                        for (rect, action) in self.hunk_buttons(bounds, y) {
                            if rect.contains(abs) {
                                shell.publish(Message::DiffHunk(action, *hunk));
                                return;
                            }
                        }
                    }
                    Row::Line(hunk, line, _) => {
                        let shift = self.app.modifiers.shift();
                        if p.x >= geo.gutter {
                            // Over the text: decide on release (click) or
                            // on movement (text selection).
                            if let Some(pos) = self.pos_at(p, &rows, &offs, &geo, state) {
                                state.press = Some((pos, p, shift));
                            }
                        } else if hunk_actions(self.diff).is_some() {
                            shell.publish(Message::DiffLineClick {
                                hunk: *hunk,
                                line: *line,
                                shift,
                            });
                            state.dragging = true;
                        }
                    }
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let p = cursor.position_in(bounds);
                if let (Some((anchor, at, _)), Some(p)) = (state.press, p) {
                    if state.text_drag.is_none() && (p.x - at.x).abs().max((p.y - at.y).abs()) > DRAG_SLOP {
                        state.text_drag = Some(anchor);
                    }
                }
                if let Some(anchor) = state.text_drag {
                    if let Some(head) = p.and_then(|p| self.pos_at(p, &rows, &offs, &geo, state)) {
                        shell.publish(Message::DiffTextDrag { anchor, head });
                        shell.capture_event();
                    }
                } else if state.dragging {
                    if let Some(p) = p {
                        if let Some(i) = self.row_at_y(&offs, p.y + state.scroll) {
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
                if let Some((pos, _, shift)) = state.press.take() {
                    if state.text_drag.take().is_none() && hunk_actions(self.diff).is_some() {
                        shell.publish(Message::DiffLineClick {
                            hunk: pos.hunk,
                            line: pos.line,
                            shift,
                        });
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
        let geo = self.geometry(renderer, bounds);
        let size = Pixels(text::Renderer::default_size(renderer).0 - 0.5);
        let mono = Font::MONOSPACE;
        let rows = flatten(self.diff);
        let offs = self.offsets(&rows, &geo);
        let match_rows = matches(&rows, &app.diff_search);
        let current_match = match_rows.get(app.diff_match.min(match_rows.len().saturating_sub(1))).copied();
        let first = self.row_at_y(&offs, state.scroll).unwrap_or(0);
        let last = self
            .row_at_y(&offs, state.scroll + bounds.height)
            .map(|i| i + 1)
            .unwrap_or(rows.len());
        let focused = app.focus == Pane::Detail;
        let digits = ((geo.gutter - 12.0) / geo.char_w - 3.0) / 2.0;
        let digits = digits.round() as usize;

        renderer.with_layer(bounds, |renderer| {
            fill(renderer, bounds, t.well, 0.0);
            for (i, row) in rows.iter().enumerate().take(last.min(rows.len())).skip(first) {
                let y = bounds.y + offs[i] - state.scroll;
                let rh = offs[i + 1] - offs[i];
                let full = Rectangle::new(Point::new(bounds.x, y), Size::new(bounds.width, rh));
                let Some(rect) = full.intersection(&bounds) else { continue };
                let partial = rect.height < rh - 0.5;
                // Text sits on the first visual line of the row.
                let cy = y + ROW_H / 2.0;
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
                        let conflict = self.diff.status == FileKind::Conflicted;
                        let (bg, fg) = if conflict {
                            if is_marker(&l.text) {
                                (Some(t.hunk_bg), t.hunk_fg)
                            } else {
                                match l.origin {
                                    '-' => (Some(alpha(OURS, 0.22)), t.text),
                                    '+' => (Some(alpha(THEIRS, 0.22)), t.text),
                                    _ => (None, t.text),
                                }
                            }
                        } else {
                            match l.origin {
                                '+' => (Some(t.add_bg), t.add_fg),
                                '-' => (Some(t.del_bg), t.del_fg),
                                _ => (None, t.text),
                            }
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
                        if let Some((a, b)) = app.diff_text_sel.map(|s| s.ordered()) {
                            let here = (*hunk, *line);
                            if (a.hunk, a.line) <= here && here <= (b.hunk, b.line) {
                                let n = l.text.chars().count();
                                let start = if here == (a.hunk, a.line) { a.col.min(n) } else { 0 };
                                let mut end = if here == (b.hunk, b.line) { b.col.min(n) } else { n };
                                if here != (b.hunk, b.line) {
                                    end = end.max(start + 1);
                                }
                                let text_x = bounds.x + geo.gutter;
                                let text_w = (bounds.width - geo.gutter).max(0.0);
                                let color = if focused { alpha(t.selection, 0.7) } else { alpha(t.selection_inactive, 0.9) };
                                let segments: Vec<(usize, usize, f32)> = if geo.cpl > 0 && rh > ROW_H {
                                    (0..)
                                        .map(|k| (k * geo.cpl, (k + 1) * geo.cpl))
                                        .take_while(|(s0, _)| *s0 < end.max(1))
                                        .enumerate()
                                        .filter_map(|(k, (s0, s1))| {
                                            let s = start.max(s0);
                                            let e = end.min(s1);
                                            (s < e).then_some((s - s0, e - s0, y + k as f32 * ROW_H))
                                        })
                                        .collect()
                                } else {
                                    vec![(start, end, y)]
                                };
                                for (s0, e0, sy) in segments {
                                    let x0 = text_x + s0 as f32 * geo.char_w - if geo.cpl > 0 { 0.0 } else { state.scroll_x };
                                    let w = (e0.saturating_sub(s0)) as f32 * geo.char_w;
                                    let hl = Rectangle::new(Point::new(x0, sy), Size::new(w, ROW_H));
                                    let clip = Rectangle::new(Point::new(text_x, rect.y), Size::new(text_w, rect.height));
                                    if let Some(r) = hl.intersection(&clip) {
                                        fill(renderer, r, color, 0.0);
                                    }
                                }
                            }
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
                        let origin = if conflict {
                            match l.origin {
                                '-' => '<',
                                '+' => '>',
                                _ => ' ',
                            }
                        } else {
                            l.origin
                        };
                        let numbers = format!("{old:>digits$} {new:>digits$} {origin}");
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
                        if geo.cpl > 0 && rh > ROW_H {
                            draw_text_wrapped(renderer, content, Point::new(bounds.x + geo.gutter, y), text_clip.width, mono, size, fg, text_clip);
                        } else {
                            let overflow = partial || state.scroll_x > 0.0 || content.chars().count() as f32 * geo.char_w > text_clip.width;
                            draw_text(renderer, content, Point::new(bounds.x + geo.gutter - state.scroll_x, cy), mono, size, fg, text_clip, overflow);
                        }
                    }
                }
            }
            // Scrollbar hint.
            let total = *offs.last().unwrap_or(&0.0);
            if total > bounds.height {
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
