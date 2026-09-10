//! Per-repository UI state that survives a restart: the pane layouts (which
//! panes are open, where, and their sizes), the maximized pane, collapsed
//! sidebar sections, the wrap toggles, the open folders of the file tree, the
//! diff options, the commit list's column widths and the changes pane's
//! section heights.
//!
//! Stored as JSON in `.git/gitgui.json` of the repository, so it follows the
//! repository without ever showing up as an untracked file.

use std::path::{Path, PathBuf};

use iced_widget::pane_grid::{self, Axis, Configuration, Node};
use serde::{Deserialize, Serialize};

use crate::ui::app::{App, Pane};

pub const FILE_NAME: &str = "gitgui.json";

/// A pane layout as a tree: `{"axis":"v","ratio":0.2,"a":..,"b":..}` splits
/// and `"files"` leaves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Layout {
    Pane(String),
    Split {
        axis: String,
        ratio: f32,
        a: Box<Layout>,
        b: Box<Layout>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Persisted {
    pub version: u32,
    pub panes: Option<Layout>,
    pub editor_panes: Option<Layout>,
    pub maximized: Option<String>,
    pub collapsed: Vec<String>,
    pub wrap: bool,
    pub editor_wrap: bool,
    pub tree_open: Vec<String>,
    pub diff_context: u32,
    pub ignore_whitespace: bool,
    pub author_width: f32,
    pub age_width: f32,
    /// Unstaged, staged and commit box shares of the changes pane.
    pub changes_split: [f32; 3],
    pub zoom: f32,
}

impl Default for Persisted {
    fn default() -> Self {
        Persisted {
            version: 1,
            panes: None,
            editor_panes: None,
            maximized: None,
            collapsed: Vec::new(),
            wrap: false,
            editor_wrap: false,
            tree_open: Vec::new(),
            diff_context: 3,
            ignore_whitespace: false,
            author_width: 110.0,
            age_width: 44.0,
            changes_split: crate::ui::app::DEFAULT_CHANGES_SPLIT,
            zoom: 1.0,
        }
    }
}

/// Where the state of the repository at `repo` lives: its git directory,
/// which also works from a subdirectory of the working tree.
pub fn path_for(repo: &Path) -> Option<PathBuf> {
    let r = git2::Repository::discover(repo).ok()?;
    Some(r.path().join(FILE_NAME))
}

pub fn load(path: &Path) -> Option<Persisted> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save(path: &Path, state: &Persisted) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(state).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

fn layout_of(state: &pane_grid::State<Pane>) -> Layout {
    fn walk(state: &pane_grid::State<Pane>, node: &Node) -> Layout {
        match node {
            Node::Pane(p) => Layout::Pane(state.get(*p).map(|k| k.id()).unwrap_or("?").to_owned()),
            Node::Split { axis, ratio, a, b, .. } => Layout::Split {
                axis: if *axis == Axis::Horizontal { "h" } else { "v" }.to_owned(),
                ratio: *ratio,
                a: Box::new(walk(state, a)),
                b: Box::new(walk(state, b)),
            },
        }
    }
    walk(state, state.layout())
}

/// A layout back into a pane grid configuration. Rejects unknown pane ids,
/// duplicates and broken ratios so a hand-edited or stale file cannot leave
/// the grid without a pane.
fn configuration(layout: &Layout, allowed: &[Pane]) -> Option<Configuration<Pane>> {
    fn walk(layout: &Layout, allowed: &[Pane], seen: &mut Vec<Pane>) -> Option<Configuration<Pane>> {
        match layout {
            Layout::Pane(id) => {
                let kind = Pane::from_id(id)?;
                if !allowed.contains(&kind) || seen.contains(&kind) {
                    return None;
                }
                seen.push(kind);
                Some(Configuration::Pane(kind))
            }
            Layout::Split { axis, ratio, a, b } => {
                let axis = match axis.as_str() {
                    "h" => Axis::Horizontal,
                    "v" => Axis::Vertical,
                    _ => return None,
                };
                if !(*ratio > 0.0 && *ratio < 1.0) {
                    return None;
                }
                Some(Configuration::Split {
                    axis,
                    ratio: *ratio,
                    a: Box::new(walk(a, allowed, seen)?),
                    b: Box::new(walk(b, allowed, seen)?),
                })
            }
        }
    }
    walk(layout, allowed, &mut Vec::new())
}

impl Persisted {
    pub fn capture(app: &App) -> Persisted {
        let mut collapsed: Vec<String> = app.sidebar_collapsed.iter().map(|s| (*s).to_owned()).collect();
        collapsed.sort();
        let mut tree_open: Vec<String> = app.tree_open.iter().cloned().collect();
        tree_open.sort();
        Persisted {
            version: 1,
            panes: Some(layout_of(&app.panes)),
            // Derived from `panes` when the editor opens; not saved.
            editor_panes: None,
            maximized: app.panes.maximized().and_then(|p| app.panes.get(p)).map(|k| k.id().to_owned()),
            collapsed,
            wrap: app.wrap,
            editor_wrap: app.editor_wrap,
            tree_open,
            diff_context: app.diff_opts.context,
            ignore_whitespace: app.diff_opts.ignore_whitespace,
            author_width: app.log_columns.0,
            age_width: app.log_columns.1,
            changes_split: app.changes_split,
            zoom: app.zoom,
        }
    }

    pub fn apply(&self, app: &mut App) {
        if let Some(cfg) = self.panes.as_ref().and_then(|l| configuration(l, Pane::ALL)) {
            app.panes = pane_grid::State::with_configuration(cfg);
            if let Some(kind) = self.maximized.as_deref().and_then(Pane::from_id) {
                let found = app.panes.iter().find(|(_, k)| **k == kind).map(|(p, _)| *p);
                if let Some(p) = found {
                    app.panes.maximize(p);
                }
            }
        }
        app.sidebar_collapsed = self
            .collapsed
            .iter()
            .filter_map(|s| crate::ui::sidebar::SECTIONS.iter().find(|t| **t == s.as_str()).copied())
            .collect();
        app.wrap = self.wrap;
        app.editor_wrap = self.editor_wrap;
        app.tree_open = self.tree_open.iter().filter(|d| !d.is_empty()).cloned().collect();
        app.diff_opts.context = self.diff_context.min(100);
        app.diff_opts.ignore_whitespace = self.ignore_whitespace;
        app.log_columns = (self.author_width.clamp(50.0, 400.0), self.age_width.clamp(36.0, 140.0));
        app.changes_split = crate::ui::app::normalize_changes_split(&self.changes_split);
        app.zoom = if self.zoom.is_finite() { self.zoom.clamp(0.5, 3.0) } else { 1.0 };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(s: &str) -> Layout {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn layout_round_trips_through_json() {
        let l = layout(r#"{"axis":"v","ratio":0.25,"a":"repository","b":{"axis":"h","ratio":0.5,"a":"commits","b":"diff"}}"#);
        let cfg = configuration(&l, Pane::ALL).expect("valid");
        let state = pane_grid::State::with_configuration(cfg);
        assert_eq!(state.len(), 3);
        assert_eq!(layout_of(&state), l);
        assert_eq!(serde_json::to_value(&l).unwrap()["a"], "repository");
    }

    #[test]
    fn broken_layouts_are_rejected() {
        assert!(configuration(&layout(r#""nope""#), Pane::ALL).is_none());
        assert!(configuration(&layout(r#"{"axis":"v","ratio":0.5,"a":"files","b":"files"}"#), Pane::ALL).is_none());
        assert!(configuration(&layout(r#"{"axis":"v","ratio":1.5,"a":"files","b":"diff"}"#), Pane::ALL).is_none());
        assert!(configuration(&layout(r#"{"axis":"x","ratio":0.5,"a":"files","b":"diff"}"#), Pane::ALL).is_none());
        assert!(configuration(&layout(r#""commits""#), Pane::ALL).is_some());
    }

    #[test]
    fn missing_fields_take_defaults() {
        let p: Persisted = serde_json::from_str(r#"{"wrap":true}"#).unwrap();
        assert!(p.wrap);
        assert_eq!(p.diff_context, 3);
        assert!(p.panes.is_none());
    }
}
