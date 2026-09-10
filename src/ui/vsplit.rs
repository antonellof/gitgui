//! A vertical stack whose sections are resized by dragging the bars between
//! them. iced has no splitter widget and a nested `pane_grid` would bring
//! title bars and its own drag handling, so this is a small custom widget: it
//! lays its children out at fractions of its own height, which is the only
//! place where that height is known, and reports the new fractions when a
//! drag ends.

use iced_core::widget::{tree, Operation, Tree};
use iced_core::{
    layout, mouse, overlay, renderer, Clipboard, Element as CoreElement, Event, Layout, Length, Point, Rectangle,
    Shell, Size, Vector, Widget,
};

use crate::ui::app::{Element, Message, Renderer};

/// Height of a drag bar, in points.
pub const HANDLE_H: f32 = 6.0;
/// How far from the middle of a bar a press still grabs it.
const GRAB: f32 = 4.0;

pub struct VSplit<'a> {
    children: Vec<Element<'a>>,
    /// One fraction of the free height per child, summing to 1.
    ratios: Vec<f32>,
    /// Smallest height per child, in points.
    mins: Vec<f32>,
    line: iced_core::Color,
    hover: iced_core::Color,
    on_resize: Box<dyn Fn(Vec<f32>) -> Message + 'a>,
}

/// A column of `children` split at `ratios`, each never smaller than its
/// `mins` entry. `on_resize` fires when a drag ends, with the new ratios.
pub fn vsplit<'a>(
    children: Vec<Element<'a>>,
    ratios: Vec<f32>,
    mins: Vec<f32>,
    line: iced_core::Color,
    hover: iced_core::Color,
    on_resize: impl Fn(Vec<f32>) -> Message + 'a,
) -> Element<'a> {
    CoreElement::new(VSplit {
        children,
        ratios,
        mins,
        line,
        hover,
        on_resize: Box::new(on_resize),
    })
}

#[derive(Default)]
struct State {
    /// The ratios in use: the app's, unless a drag is changing them.
    ratios: Vec<f32>,
    /// (bar index, pointer y at press, ratios at press)
    drag: Option<(usize, f32, Vec<f32>)>,
}

/// Ratios that are finite, positive and sum to 1. A missing, short or broken
/// list becomes an even split, so a hand-edited state file cannot collapse a
/// section to nothing.
pub fn normalize(ratios: &[f32], n: usize) -> Vec<f32> {
    let even = vec![1.0 / n as f32; n];
    if n == 0 || ratios.len() != n {
        return even;
    }
    let sum: f32 = ratios.iter().sum();
    if !sum.is_finite() || sum <= 0.0 || ratios.iter().any(|r| !r.is_finite() || *r <= 0.0) {
        return even;
    }
    ratios.iter().map(|r| r / sum).collect()
}

/// Heights for `n` sections in `avail` points: the ratios, then pushed up to
/// the minimums, then trimmed from the tallest sections when they no longer
/// fit. The remainder goes to the last section so the sections always add up
/// to the full height.
pub fn heights(ratios: &[f32], mins: &[f32], avail: f32) -> Vec<f32> {
    let n = ratios.len();
    let mut h: Vec<f32> = ratios.iter().map(|r| r * avail).collect();
    if avail <= 0.0 {
        return vec![0.0; n];
    }
    // Not even the minimums fit: hand out the space in proportion instead.
    let min_sum: f32 = mins.iter().sum();
    if min_sum >= avail {
        return mins.iter().map(|m| m / min_sum * avail).collect();
    }
    for i in 0..n {
        h[i] = h[i].max(mins[i]);
    }
    // Give the overshoot back, taking from the sections with the most slack.
    let mut over: f32 = h.iter().sum::<f32>() - avail;
    while over > 0.01 {
        let slack: f32 = h.iter().zip(mins).map(|(h, m)| (h - m).max(0.0)).sum();
        if slack <= 0.01 {
            break;
        }
        let take = over.min(slack);
        for i in 0..n {
            let s = (h[i] - mins[i]).max(0.0);
            h[i] -= take * s / slack;
        }
        over = h.iter().sum::<f32>() - avail;
    }
    let used: f32 = h.iter().take(n - 1).sum();
    h[n - 1] = (avail - used).max(0.0);
    h
}

impl State {
    /// Take the app's ratios unless a drag is changing them right now.
    fn sync(&mut self, ratios: &[f32]) {
        if self.drag.is_none() || self.ratios.len() != ratios.len() {
            self.ratios = normalize(ratios, ratios.len());
        }
    }

    /// The bar under `p`: index `i` sits below child `i`.
    fn bar_at(&self, bounds: Rectangle, mins: &[f32], p: Point) -> Option<usize> {
        if p.x < bounds.x || p.x > bounds.x + bounds.width {
            return None;
        }
        let n = self.ratios.len();
        let avail = (bounds.height - HANDLE_H * (n - 1) as f32).max(0.0);
        let h = heights(&self.ratios, mins, avail);
        let mut y = bounds.y;
        for (i, hi) in h.iter().take(n - 1).enumerate() {
            y += hi;
            let mid = y + HANDLE_H / 2.0;
            if (p.y - mid).abs() <= GRAB + HANDLE_H / 2.0 {
                return Some(i);
            }
            y += HANDLE_H;
        }
        None
    }

    /// Move bar `i` by `dy` points, starting from the ratios `start`. The two
    /// sections around the bar trade height, the others keep theirs.
    fn drag_to(&mut self, i: usize, dy: f32, start: &[f32], mins: &[f32], avail: f32) {
        if avail <= 0.0 {
            return;
        }
        let mut h = heights(start, mins, avail);
        let pair = h[i] + h[i + 1];
        let top = (h[i] + dy).clamp(mins[i], (pair - mins[i + 1]).max(mins[i]));
        h[i] = top;
        h[i + 1] = pair - top;
        self.ratios = h.iter().map(|v| v / avail).collect();
    }
}

impl<'a> Widget<Message, iced_core::Theme, Renderer> for VSplit<'a> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn children(&self) -> Vec<Tree> {
        self.children.iter().map(Tree::new).collect()
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&self.children);
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        let state = tree.state.downcast_mut::<State>();
        state.sync(&self.ratios);
        let size = limits.max();
        let n = self.children.len();
        let avail = (size.height - HANDLE_H * (n.saturating_sub(1)) as f32).max(0.0);
        let h = heights(&state.ratios, &self.mins, avail);
        let mut nodes = Vec::with_capacity(n);
        let mut y = 0.0;
        for (i, child) in self.children.iter_mut().enumerate() {
            let limits = layout::Limits::new(Size::ZERO, Size::new(size.width, h[i]));
            let node = child
                .as_widget_mut()
                .layout(&mut tree.children[i], renderer, &limits)
                .move_to(Point::new(0.0, y));
            nodes.push(node);
            y += h[i] + HANDLE_H;
        }
        layout::Node::with_children(size, nodes)
    }

    fn operate(&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn Operation) {
        operation.container(None, layout.bounds());
        operation.traverse(&mut |operation| {
            self.children
                .iter_mut()
                .zip(&mut tree.children)
                .zip(layout.children())
                .for_each(|((child, state), layout)| {
                    child.as_widget_mut().operate(state, layout, renderer, operation);
                });
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State>();
        state.sync(&self.ratios);
        let bounds = layout.bounds();
        let n = self.children.len();
        let avail = (bounds.height - HANDLE_H * (n.saturating_sub(1)) as f32).max(0.0);
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(p) = cursor.position() {
                    if let Some(i) = state.bar_at(bounds, &self.mins, p) {
                        let start = state.ratios.clone();
                        state.drag = Some((i, p.y, start));
                        shell.capture_event();
                        return;
                    }
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if let Some((i, start_y, start)) = state.drag.clone() {
                    if let Some(p) = cursor.position() {
                        state.drag_to(i, p.y - start_y, &start, &self.mins, avail);
                        shell.capture_event();
                        shell.invalidate_layout();
                        shell.request_redraw();
                        return;
                    }
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if state.drag.take().is_some() => {
                shell.publish((self.on_resize)(state.ratios.clone()));
                shell.capture_event();
                return;
            }
            _ => {}
        }
        for ((child, tree), layout) in self.children.iter_mut().zip(&mut tree.children).zip(layout.children()) {
            child
                .as_widget_mut()
                .update(tree, event, layout, cursor, renderer, clipboard, shell, viewport);
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let state = tree.state.downcast_ref::<State>();
        if state.drag.is_some() {
            return mouse::Interaction::ResizingVertically;
        }
        if let Some(p) = cursor.position() {
            if state.bar_at(layout.bounds(), &self.mins, p).is_some() {
                return mouse::Interaction::ResizingVertically;
            }
        }
        self.children
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .map(|((child, tree), layout)| child.as_widget().mouse_interaction(tree, layout, cursor, viewport, renderer))
            .max()
            .unwrap_or_default()
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &iced_core::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State>();
        let bounds = layout.bounds();
        let Some(clipped) = bounds.intersection(viewport) else {
            return;
        };
        for ((child, tree), layout) in self
            .children
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .filter(|(_, layout)| layout.bounds().intersects(&clipped))
        {
            child.as_widget().draw(tree, renderer, theme, style, layout, cursor, &clipped);
        }
        // The bars: a hairline in the gap between two sections, brighter
        // while the pointer is on it or dragging it.
        let hot = state
            .drag
            .as_ref()
            .map(|(i, _, _)| *i)
            .or_else(|| cursor.position().and_then(|p| state.bar_at(bounds, &self.mins, p)));
        let kids: Vec<Rectangle> = layout.children().map(|l| l.bounds()).collect();
        for (i, k) in kids.iter().take(kids.len().saturating_sub(1)).enumerate() {
            let on = hot == Some(i);
            let thickness = if on { 2.0 } else { 1.0 };
            let y = k.y + k.height + (HANDLE_H - thickness) / 2.0;
            let color = if on { self.hover } else { self.line };
            bar(renderer, bounds.x + 4.0, y, (bounds.width - 8.0).max(0.0), thickness, color);
            // A short grip in the middle: the hairline alone reads as a
            // border, the grip says the bar can be dragged.
            let grip_w = 26.0_f32.min(bounds.width * 0.3);
            bar(
                renderer,
                bounds.x + (bounds.width - grip_w) / 2.0,
                k.y + k.height + (HANDLE_H - 2.0) / 2.0,
                grip_w,
                2.0,
                color,
            );
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, iced_core::Theme, Renderer>> {
        overlay::from_children(&mut self.children, tree, layout, renderer, viewport, translation)
    }
}

fn bar(renderer: &mut Renderer, x: f32, y: f32, width: f32, height: f32, color: iced_core::Color) {
    iced_core::Renderer::fill_quad(
        renderer,
        renderer::Quad {
            bounds: Rectangle { x, y, width, height },
            border: iced_core::Border {
                radius: 1.0.into(),
                ..Default::default()
            },
            ..Default::default()
        },
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratios_are_normalized() {
        assert_eq!(normalize(&[1.0, 1.0], 2), vec![0.5, 0.5]);
        assert_eq!(normalize(&[2.0, 2.0, 4.0], 3), vec![0.25, 0.25, 0.5]);
        // Broken input falls back to an even split.
        assert_eq!(normalize(&[0.0, 1.0], 2), vec![0.5, 0.5]);
        assert_eq!(normalize(&[f32::NAN, 1.0], 2), vec![0.5, 0.5]);
        assert_eq!(normalize(&[1.0], 2), vec![0.5, 0.5]);
    }

    #[test]
    fn heights_follow_the_ratios() {
        let h = heights(&[0.4, 0.4, 0.2], &[40.0, 40.0, 90.0], 500.0);
        assert!((h[0] - 200.0).abs() < 0.5, "{h:?}");
        assert!((h[1] - 200.0).abs() < 0.5, "{h:?}");
        assert!((h[2] - 100.0).abs() < 0.5, "{h:?}");
        assert!((h.iter().sum::<f32>() - 500.0).abs() < 0.01);
    }

    #[test]
    fn minimums_win_and_the_total_still_fits() {
        // The commit box cannot go below 90 pt, the lists give way.
        let h = heights(&[0.48, 0.48, 0.04], &[40.0, 40.0, 90.0], 300.0);
        assert!(h[2] >= 89.9, "{h:?}");
        assert!(h[0] >= 40.0 && h[1] >= 40.0, "{h:?}");
        assert!((h.iter().sum::<f32>() - 300.0).abs() < 0.01, "{h:?}");
    }

    #[test]
    fn a_pane_too_short_for_the_minimums_splits_in_proportion() {
        let h = heights(&[0.4, 0.4, 0.2], &[40.0, 40.0, 90.0], 85.0);
        assert!((h.iter().sum::<f32>() - 85.0).abs() < 0.01, "{h:?}");
        assert!(h.iter().all(|v| *v > 0.0), "{h:?}");
    }

    #[test]
    fn dragging_trades_height_between_the_two_sections() {
        let mins = [40.0, 40.0, 90.0];
        let mut s = State {
            ratios: vec![0.4, 0.4, 0.2],
            drag: None,
        };
        let start = s.ratios.clone();
        s.drag_to(0, 50.0, &start, &mins, 500.0);
        let h = heights(&s.ratios, &mins, 500.0);
        assert!((h[0] - 250.0).abs() < 0.5, "{h:?}");
        assert!((h[1] - 150.0).abs() < 0.5, "{h:?}");
        assert!((h[2] - 100.0).abs() < 0.5, "{h:?}");
    }

    #[test]
    fn a_drag_stops_at_the_minimum() {
        let mins = [40.0, 40.0, 90.0];
        let mut s = State {
            ratios: vec![0.4, 0.4, 0.2],
            drag: None,
        };
        let start = s.ratios.clone();
        s.drag_to(0, 1000.0, &start, &mins, 500.0);
        let h = heights(&s.ratios, &mins, 500.0);
        assert!((h[1] - 40.0).abs() < 0.5, "{h:?}");
        s.ratios = start.clone();
        s.drag_to(0, -1000.0, &start, &mins, 500.0);
        let h = heights(&s.ratios, &mins, 500.0);
        assert!((h[0] - 40.0).abs() < 0.5, "{h:?}");
    }
}
