//! Commit list with the graph column. A custom widget: only the visible rows
//! are laid out and drawn, so a 2000 commit log costs nothing off screen.

use iced_core::mouse;
use iced_core::text::{self, Paragraph as _};
use iced_core::widget::{tree, Tree};
use iced_core::{
    alignment, event, layout, renderer, Color, Element as CoreElement, Event, Font, Length, Pixels, Point, Rectangle,
    Shell, Size, Widget,
};
use iced_widget::canvas::{Frame, Path, Stroke};
use iced_widget::{column, container, row, text as text_widget, text_input, Space};
use iced_core::Renderer as _;

use crate::git::graph::{EdgeKind, RowLayout};
use crate::ui::app::{age, App, Element, Message, MenuKind, Pane, Renderer, Selection};
use crate::ui::widgets::{self, small_button};

pub const ROW_H: f32 = 24.0;
/// Column header row with the draggable dividers.
const HEADER_H: f32 = 22.0;
const DIVIDER_GRAB: f32 = 5.0;
const LANE_W: f32 = 14.0;
const MAX_LANES: usize = 12;
const NODE_R: f32 = 3.5;

pub fn view(app: &App) -> Element<'_> {
    let t = &app.theme;
    let filtering = app.filter_active || !app.filter.is_empty();
    let mut header = row![].spacing(6).align_y(iced_core::Alignment::Center).padding([4, 6]);
    if filtering {
        header = header.push(
            text_input("filter summary, author, hash", &app.filter)
                .id(widgets::FILTER_ID.clone())
                .on_input(Message::FilterChanged)
                .size(12)
                .padding([3, 8])
                .style(widgets::text_input_style)
                .width(Length::Fill),
        );
        header = header.push(small_button("x", Some(Message::FilterClear)));
    } else {
        if app.snapshot.truncated {
            header = header.push(text_widget(format!("first {}", app.snapshot.commits.len())).size(12).color(t.weak));
            let more = app.snapshot.commits.len() + 2000;
            header = header.push(small_button(
                "load more",
                Some(Message::Run(crate::git::ops::Command::LoadMore(more))),
            ));
        }
        header = header.push(Space::new().width(Length::Fill));
        header = header.push(small_button("filter  /", Some(Message::FilterOpen)));
    }
    let list: Element<'_> = CoreElement::new(LogView {
        app,
        rows: app.log_rows(),
    });
    column![header, container(list).width(Length::Fill).height(Length::Fill)].into()
}

struct LogView<'a> {
    app: &'a App,
    rows: Vec<Selection>,
}

struct State {
    scroll: f32,
    /// Truncated strings by (text, width): measuring is the costly part of a row.
    fit: std::cell::RefCell<std::collections::HashMap<(String, u32), String>>,
    /// Column widths, dragged from the header dividers.
    author_w: f32,
    age_w: f32,
    /// (divider index, pointer x at press, width at press)
    drag: Option<(u8, f32, f32)>,
}

impl Default for State {
    fn default() -> Self {
        State {
            scroll: 0.0,
            fit: Default::default(),
            author_w: 110.0,
            age_w: 44.0,
            drag: None,
        }
    }
}

/// Column edges: (author_x, age_x). Narrow lists drop the author column.
struct Columns {
    author_x: f32,
    age_x: f32,
    show_author: bool,
}

impl State {
    fn columns(&self, bounds: Rectangle) -> Columns {
        let right = bounds.x + bounds.width;
        let age_x = right - self.age_w;
        let show_author = bounds.width > 420.0;
        let author_x = if show_author { age_x - self.author_w } else { age_x };
        Columns {
            author_x,
            age_x,
            show_author,
        }
    }

    /// Divider under `p` in the header: 0 between summary and author, 1
    /// between author and date.
    fn divider_at(&self, bounds: Rectangle, p: Point) -> Option<u8> {
        if p.y < bounds.y || p.y > bounds.y + HEADER_H {
            return None;
        }
        let c = self.columns(bounds);
        if c.show_author && (p.x - c.author_x).abs() <= DIVIDER_GRAB {
            Some(0)
        } else if (p.x - c.age_x).abs() <= DIVIDER_GRAB {
            Some(1)
        } else {
            None
        }
    }
    fn fit(&self, s: &str, width: f32, font: Font, size: Pixels) -> String {
        let key = (s.to_owned(), width.to_bits());
        if let Some(v) = self.fit.borrow().get(&key) {
            return v.clone();
        }
        let v = fit(s, width, font, size);
        let mut cache = self.fit.borrow_mut();
        if cache.len() > 4096 {
            cache.clear();
        }
        cache.insert(key, v.clone());
        v
    }
}

impl LogView<'_> {
    fn max_scroll(&self, height: f32) -> f32 {
        (self.rows.len() as f32 * ROW_H - (height - HEADER_H)).max(0.0)
    }
}

impl Widget<Message, iced_core::Theme, Renderer> for LogView<'_> {
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
        _renderer: &Renderer,
        _clipboard: &mut dyn iced_core::Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State>();
        let bounds = layout.bounds();
        let max = self.max_scroll(bounds.height);
        match event {
            Event::Window(iced_core::window::Event::RedrawRequested(_)) => {
                if self.app.scroll_to_selection.get() {
                    if let Some(i) = self.rows.iter().position(|r| *r == self.app.selection) {
                        let top = i as f32 * ROW_H;
                        let body_h = bounds.height - HEADER_H;
                        if top < state.scroll {
                            state.scroll = top;
                        } else if top + ROW_H > state.scroll + body_h {
                            state.scroll = (top + ROW_H - body_h).max(0.0);
                        }
                        self.app.scroll_to_selection.set(false);
                        shell.request_redraw();
                    }
                }
                state.scroll = state.scroll.clamp(0.0, max);
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                if let Some(_p) = cursor.position_in(bounds) {
                    let dy = match delta {
                        mouse::ScrollDelta::Lines { y, .. } => -y * ROW_H * 3.0,
                        mouse::ScrollDelta::Pixels { y, .. } => -y,
                    };
                    state.scroll = (state.scroll + dy).clamp(0.0, max);
                    shell.capture_event();
                    shell.request_redraw();
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if let Some((which, start_x, start_w)) = state.drag {
                    if let Some(p) = cursor.position() {
                        let dx = p.x - start_x;
                        match which {
                            0 => state.author_w = (start_w - dx).clamp(50.0, 400.0),
                            _ => state.age_w = (start_w - dx).clamp(36.0, 140.0),
                        }
                        shell.capture_event();
                        shell.request_redraw();
                    }
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if state.drag.take().is_some() {
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::ButtonPressed(button)) => {
                if let Some(abs) = cursor.position() {
                    if *button == mouse::Button::Left {
                        if let Some(which) = state.divider_at(bounds, abs) {
                            let w = if which == 0 { state.author_w } else { state.age_w };
                            state.drag = Some((which, abs.x, w));
                            shell.capture_event();
                            return;
                        }
                    }
                }
                if let Some(p) = cursor.position_in(bounds) {
                    if p.y < HEADER_H {
                        return;
                    }
                    let i = ((p.y - HEADER_H + state.scroll) / ROW_H).floor() as usize;
                    if let Some(sel) = self.rows.get(i).copied() {
                        shell.publish(Message::SelectRow(sel));
                        if *button == mouse::Button::Right {
                            if let Selection::Commit(idx) = sel {
                                shell.publish(Message::MenuOpen(MenuKind::Commit(idx)));
                            }
                        }
                        shell.capture_event();
                    }
                }
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: layout::Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        let state = tree.state.downcast_ref::<State>();
        let bounds = layout.bounds();
        if state.drag.is_some() {
            return mouse::Interaction::ResizingHorizontally;
        }
        if let Some(p) = cursor.position() {
            if state.divider_at(bounds, p).is_some() {
                return mouse::Interaction::ResizingHorizontally;
            }
        }
        if cursor.is_over(bounds) {
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
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State>();
        let bounds = layout.bounds();
        let app = self.app;
        let t = &app.theme;
        let s = &app.snapshot;
        let focused = app.focus == Pane::Log;
        let font_size = text::Renderer::default_size(renderer);
        let small = Pixels(font_size.0 - 2.0);
        let default_font = text::Renderer::default_font(renderer);
        let lanes = s.graph.max_lanes.clamp(1, MAX_LANES);
        let graph_w = lanes as f32 * LANE_W + 6.0;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let body = Rectangle::new(
            Point::new(bounds.x, bounds.y + HEADER_H),
            Size::new(bounds.width, (bounds.height - HEADER_H).max(0.0)),
        );
        let first = (state.scroll / ROW_H).floor() as usize;
        let last = ((state.scroll + body.height) / ROW_H).ceil() as usize;
        let cols = state.columns(bounds);
        let show_author = cols.show_author;

        renderer.with_layer(bounds, |renderer| {
            // Column header with its dividers.
            let header = Rectangle::new(bounds.position(), Size::new(bounds.width, HEADER_H));
            fill(renderer, header, t.panel, 0.0);
            let hy = header.center_y();
            draw_text(renderer, "Commit".to_owned(), Point::new(bounds.x + graph_w + 6.0, hy), default_font, small, t.weak, header, false);
            if show_author {
                fill(renderer, Rectangle::new(Point::new(cols.author_x - 0.5, bounds.y + 4.0), Size::new(1.0, HEADER_H - 8.0)), t.border, 0.0);
                let hclip = Rectangle::new(Point::new(cols.author_x, bounds.y), Size::new(cols.age_x - cols.author_x, HEADER_H));
                draw_text(renderer, "Author".to_owned(), Point::new(cols.author_x + 6.0, hy), default_font, small, t.weak, hclip, true);
            }
            fill(renderer, Rectangle::new(Point::new(cols.age_x - 0.5, bounds.y + 4.0), Size::new(1.0, HEADER_H - 8.0)), t.border, 0.0);
            let dclip = Rectangle::new(Point::new(cols.age_x, bounds.y), Size::new(state.age_w, HEADER_H));
            draw_text(renderer, "Date".to_owned(), Point::new(cols.age_x + 6.0, hy), default_font, small, t.weak, dclip, true);

            // Absolute coordinates: tiny-skia applies a layer translation to
            // a geometry group's clip rect twice, which pushes it off-pane.
            let mut frame = Frame::with_bounds(renderer, body);
            for i in first..last.min(self.rows.len()) {
                let y = body.y + i as f32 * ROW_H - state.scroll;
                let full = Rectangle::new(Point::new(bounds.x, y), Size::new(bounds.width, ROW_H));
                // Nested layers do not intersect in tiny-skia: clip rows to the widget ourselves.
                let Some(rect) = full.intersection(&body) else { continue };
                let partial = rect.height < ROW_H - 0.5;
                let sel = self.rows[i];
                let selected = sel == app.selection;
                if selected {
                    fill(
                        renderer,
                        rect,
                        if focused { t.selection } else { t.selection_inactive },
                        4.0,
                    );
                }
                let text_x = bounds.x + graph_w + 6.0;
                match sel {
                    Selection::WorkingTree => {
                        let cx = bounds.x + 3.0 + LANE_W / 2.0;
                        frame.stroke(
                            &Path::circle(Point::new(cx, full.center_y()), NODE_R),
                            Stroke::default().with_color(t.accent).with_width(1.5),
                        );
                        let label = format!(
                            "Working tree: {} unstaged, {} staged{}{}",
                            s.unstaged.len(),
                            s.staged.len(),
                            if s.conflicted.is_empty() {
                                String::new()
                            } else {
                                format!(", {} conflicted", s.conflicted.len())
                            },
                            if s.state == crate::git::repo::RepoState::Clean {
                                String::new()
                            } else {
                                format!(" ({} in progress)", s.state.label())
                            }
                        );
                        draw_text(renderer, label, Point::new(text_x, full.center_y()), default_font, font_size, t.strong, rect, true);
                    }
                    Selection::Commit(ci) => {
                        let Some(c) = s.commits.get(ci) else { continue };
                        if let Some(layout) = s.graph.rows.get(ci) {
                            let next = s.graph.rows.get(ci + 1);
                            let graph_rect = Rectangle::new(Point::new(bounds.x, y), Size::new(graph_w, ROW_H));
                            draw_graph_row(&mut frame, graph_rect, layout, next, t, lanes);
                        }
                        let mut x = text_x;
                        for r in &c.refs {
                            let w = measure(&r.name, default_font, small) + 10.0;
                            let pill = Rectangle::new(Point::new(x, full.center_y() - 8.0), Size::new(w, 16.0));
                            if let Some(visible) = pill.intersection(&bounds) {
                                fill(renderer, visible, t.pill(r.kind), 4.0);
                                draw_text(renderer, r.name.clone(), Point::new(x + 5.0, full.center_y()), default_font, small, Color::WHITE, visible, partial);
                            }
                            x += w + 4.0;
                        }
                        let summary_w = (cols.author_x - x - 8.0).max(40.0);
                        let clip = Rectangle::new(Point::new(x, rect.y), Size::new(summary_w, rect.height));
                        let summary = state.fit(&c.summary, summary_w, default_font, font_size);
                        draw_text(renderer, summary, Point::new(x, full.center_y()), default_font, font_size, t.text, clip, partial);
                        if show_author {
                            let ax = cols.author_x + 6.0;
                            let aw = (cols.age_x - ax - 6.0).max(10.0);
                            let aclip = Rectangle::new(Point::new(ax, rect.y), Size::new(aw, rect.height));
                            let author = state.fit(&c.author, aw, default_font, small);
                            draw_text(renderer, author, Point::new(ax, full.center_y()), default_font, small, t.weak, aclip, partial);
                        }
                        let age_s = state.fit(&age(now, c.time), state.age_w - 12.0, default_font, small);
                        let aw = measure(&age_s, default_font, small);
                        let dclip = Rectangle::new(Point::new(cols.age_x, rect.y), Size::new(state.age_w, rect.height));
                        draw_text(
                            renderer,
                            age_s,
                            Point::new(rect.x + rect.width - 6.0 - aw, full.center_y()),
                            default_font,
                            small,
                            t.weak,
                            dclip,
                            partial,
                        );
                    }
                }
            }
            iced_widget::graphics::geometry::Renderer::draw_geometry(renderer, frame.into_geometry());
        });
    }
}

pub fn fill(renderer: &mut Renderer, rect: Rectangle, color: Color, radius: f32) {
    iced_core::Renderer::fill_quad(
        renderer,
        renderer::Quad {
            bounds: rect,
            border: iced_core::Border {
                radius: radius.into(),
                ..Default::default()
            },
            ..Default::default()
        },
        color,
    );
}

/// Single-line text anchored at its left-center point, clipped to `clip`.
/// Single-line text anchored at its left-center point. With `may_overflow`
/// the text is drawn inside a layer of exactly `clip` (see below); pass
/// false for text known to fit, a layer per text is what costs.
#[allow(clippy::too_many_arguments)]
pub fn draw_text(renderer: &mut Renderer, content: String, at: Point, font: Font, size: Pixels, color: Color, clip: Rectangle, may_overflow: bool) {
    if clip.width <= 0.0 || clip.height <= 0.0 {
        return;
    }
    if !may_overflow {
        fill_text(renderer, content, at, font, size, color, clip);
        return;
    }
    // tiny-skia applies a clip mask only when the text's clip rect pokes
    // outside the current layer, so draw inside a layer of exactly `clip`
    // and hand the text a rect one pixel larger.
    let outer = Rectangle::new(
        Point::new(clip.x - 1.0, clip.y - 1.0),
        Size::new(clip.width + 2.0, clip.height + 2.0),
    );
    renderer.with_layer(clip, |renderer| fill_text(renderer, content, at, font, size, color, outer));
}

fn fill_text(renderer: &mut Renderer, content: String, at: Point, font: Font, size: Pixels, color: Color, clip: Rectangle) {
    text::Renderer::fill_text(
        renderer,
        text::Text {
            content,
            bounds: Size::new(f32::INFINITY, clip.height.max(ROW_H)),
            size,
            line_height: text::LineHeight::default(),
            font,
            align_x: text::Alignment::Left,
            align_y: alignment::Vertical::Center,
            shaping: text::Shaping::Advanced,
            wrapping: text::Wrapping::None,
        },
        at,
        color,
        clip,
    );
}

/// Multi-line text anchored at its top-left, wrapped per glyph inside
/// `width`, clipped to `clip`.
#[allow(clippy::too_many_arguments)]
pub fn draw_text_wrapped(renderer: &mut Renderer, content: String, at: Point, width: f32, font: Font, size: Pixels, color: Color, clip: Rectangle) {
    if clip.width <= 0.0 || clip.height <= 0.0 {
        return;
    }
    let outer = Rectangle::new(
        Point::new(clip.x - 1.0, clip.y - 1.0),
        Size::new(clip.width + 2.0, clip.height + 2.0),
    );
    renderer.with_layer(clip, |renderer| {
        text::Renderer::fill_text(
            renderer,
            text::Text {
                content,
                bounds: Size::new(width, clip.height),
                size,
                line_height: text::LineHeight::Absolute(Pixels(crate::ui::diff::ROW_H)),
                font,
                align_x: text::Alignment::Left,
                align_y: alignment::Vertical::Top,
                shaping: text::Shaping::Advanced,
                wrapping: text::Wrapping::Glyph,
            },
            at,
            color,
            outer,
        );
    });
}

/// `s` cut to `width` points with an ellipsis when it does not fit.
pub fn fit(s: &str, width: f32, font: Font, size: Pixels) -> String {
    if measure(s, font, size) <= width {
        return s.to_owned();
    }
    let chars: Vec<char> = s.chars().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let candidate: String = chars[..mid].iter().collect::<String>() + "…";
        if measure(&candidate, font, size) <= width {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    chars[..lo].iter().collect::<String>() + "…"
}

pub fn measure(s: &str, font: Font, size: Pixels) -> f32 {
    let p = <Renderer as text::Renderer>::Paragraph::with_text(text::Text {
        content: s,
        bounds: Size::new(f32::INFINITY, f32::INFINITY),
        size,
        line_height: text::LineHeight::default(),
        font,
        align_x: text::Alignment::Left,
        align_y: alignment::Vertical::Center,
        shaping: text::Shaping::Advanced,
        wrapping: text::Wrapping::None,
    });
    p.min_width()
}

fn lane_x(rect: Rectangle, lane: usize) -> f32 {
    rect.x + 3.0 + LANE_W / 2.0 + lane as f32 * LANE_W
}

fn has_parent_below(row: &RowLayout, next: Option<&RowLayout>) -> bool {
    match next {
        None => false,
        Some(n) => {
            n.lane == row.lane
                || n.through.iter().any(|(l, _)| *l == row.lane)
                || n.edges
                    .iter()
                    .any(|e| e.kind == EdgeKind::Merge && e.from_lane == row.lane)
        }
    }
}

fn curve(frame: &mut Frame, from: Point, to: Point, stroke: Stroke<'_>) {
    let ctrl = Point::new(to.x, from.y);
    let path = Path::new(|b| {
        b.move_to(from);
        b.quadratic_curve_to(ctrl, to);
    });
    frame.stroke(&path, stroke);
}

fn draw_graph_row(
    frame: &mut Frame,
    rect: Rectangle,
    row: &RowLayout,
    next: Option<&RowLayout>,
    theme: &crate::ui::theme::Theme,
    max_lanes: usize,
) {
    let top = rect.y;
    let bottom = rect.y + rect.height;
    let mid = rect.center_y();
    let visible = |lane: usize| lane < max_lanes;
    let stroke = |color: usize| Stroke::default().with_color(theme.graph_color(color)).with_width(1.5);

    for (lane, color) in &row.through {
        if visible(*lane) {
            let x = lane_x(rect, *lane);
            frame.stroke(&Path::line(Point::new(x, top), Point::new(x, bottom)), stroke(*color));
        }
    }
    let cx = lane_x(rect, row.lane);
    frame.stroke(&Path::line(Point::new(cx, top), Point::new(cx, mid)), stroke(row.color));
    let continues = row
        .edges
        .iter()
        .all(|e| e.kind != EdgeKind::Merge || e.to_lane != row.lane)
        && has_parent_below(row, next);
    if continues {
        frame.stroke(&Path::line(Point::new(cx, mid), Point::new(cx, bottom)), stroke(row.color));
    }
    for e in &row.edges {
        match e.kind {
            EdgeKind::Fork => {
                if visible(e.to_lane) {
                    let tx = lane_x(rect, e.to_lane);
                    let color = next
                        .and_then(|n| n.through.iter().find(|(l, _)| *l == e.to_lane).map(|(_, c)| *c))
                        .unwrap_or(row.color);
                    curve(frame, Point::new(cx, mid), Point::new(tx, bottom), stroke(color));
                }
            }
            EdgeKind::Merge => {
                if visible(e.from_lane) {
                    let fx = lane_x(rect, e.from_lane);
                    let color = row
                        .through
                        .iter()
                        .find(|(l, _)| *l == e.from_lane)
                        .map(|(_, c)| *c)
                        .unwrap_or(row.color);
                    curve(frame, Point::new(fx, top), Point::new(cx, mid), stroke(color));
                }
            }
        }
    }
    let node = Point::new(cx, mid);
    if row.is_merge {
        frame.fill(&Path::circle(node, NODE_R), theme.background);
        frame.stroke(&Path::circle(node, NODE_R), stroke(row.color));
    } else {
        frame.fill(&Path::circle(node, NODE_R), theme.graph_color(row.color));
    }
}

#[allow(dead_code)]
fn unused(_: event::Status) {}
