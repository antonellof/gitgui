//! Colors. Dark by default; light when the terminal background is light.

use egui::Color32;

use crate::git::repo::RefKind;

#[derive(Clone)]
pub struct Theme {
    pub dark: bool,
    pub background: Color32,
    pub graph: [Color32; 8],
    pub add_bg: Color32,
    pub del_bg: Color32,
    pub add_fg: Color32,
    pub del_fg: Color32,
    pub hunk_bg: Color32,
    pub hunk_fg: Color32,
    pub line_no: Color32,
    pub selection: Color32,
    pub selection_inactive: Color32,
    pub head_pill: Color32,
    pub branch_pill: Color32,
    pub remote_pill: Color32,
    pub tag_pill: Color32,
    pub error: Color32,
    pub ok: Color32,
}

impl Theme {
    pub fn dark() -> Self {
        Theme {
            dark: true,
            background: Color32::from_rgb(0x1b, 0x1b, 0x1b),
            graph: [
                Color32::from_rgb(0x89, 0xb4, 0xfa),
                Color32::from_rgb(0xa6, 0xe3, 0xa1),
                Color32::from_rgb(0xf9, 0xe2, 0xaf),
                Color32::from_rgb(0xf3, 0x8b, 0xa8),
                Color32::from_rgb(0xcb, 0xa6, 0xf7),
                Color32::from_rgb(0x94, 0xe2, 0xd5),
                Color32::from_rgb(0xfa, 0xb3, 0x87),
                Color32::from_rgb(0x74, 0xc7, 0xec),
            ],
            add_bg: Color32::from_rgb(0x1e, 0x36, 0x24),
            del_bg: Color32::from_rgb(0x3d, 0x1f, 0x22),
            add_fg: Color32::from_rgb(0xa6, 0xe3, 0xa1),
            del_fg: Color32::from_rgb(0xf3, 0x8b, 0xa8),
            hunk_bg: Color32::from_rgb(0x26, 0x2d, 0x3a),
            hunk_fg: Color32::from_rgb(0x89, 0xb4, 0xfa),
            line_no: Color32::from_rgb(0x6c, 0x70, 0x86),
            selection: Color32::from_rgb(0x2f, 0x4a, 0x6e),
            selection_inactive: Color32::from_rgb(0x2a, 0x2f, 0x3a),
            head_pill: Color32::from_rgb(0x40, 0xa0, 0x2b),
            branch_pill: Color32::from_rgb(0x2b, 0x6c, 0xb0),
            remote_pill: Color32::from_rgb(0x8a, 0x5a, 0xb5),
            tag_pill: Color32::from_rgb(0xb0, 0x8a, 0x2b),
            error: Color32::from_rgb(0xf3, 0x8b, 0xa8),
            ok: Color32::from_rgb(0xa6, 0xe3, 0xa1),
        }
    }

    pub fn light() -> Self {
        Theme {
            dark: false,
            background: Color32::from_rgb(0xf6, 0xf6, 0xf6),
            graph: [
                Color32::from_rgb(0x1e, 0x66, 0xf5),
                Color32::from_rgb(0x40, 0xa0, 0x2b),
                Color32::from_rgb(0xdf, 0x8e, 0x1d),
                Color32::from_rgb(0xd2, 0x0f, 0x39),
                Color32::from_rgb(0x88, 0x39, 0xef),
                Color32::from_rgb(0x17, 0x92, 0x99),
                Color32::from_rgb(0xfe, 0x64, 0x0b),
                Color32::from_rgb(0x04, 0xa5, 0xe5),
            ],
            add_bg: Color32::from_rgb(0xdd, 0xf4, 0xdd),
            del_bg: Color32::from_rgb(0xfb, 0xe0, 0xe0),
            add_fg: Color32::from_rgb(0x1a, 0x7f, 0x37),
            del_fg: Color32::from_rgb(0xcf, 0x22, 0x2e),
            hunk_bg: Color32::from_rgb(0xdd, 0xe8, 0xff),
            hunk_fg: Color32::from_rgb(0x1e, 0x66, 0xf5),
            line_no: Color32::from_rgb(0x8c, 0x8f, 0xa1),
            selection: Color32::from_rgb(0xc7, 0xdc, 0xf8),
            selection_inactive: Color32::from_rgb(0xe4, 0xe4, 0xe8),
            head_pill: Color32::from_rgb(0x40, 0xa0, 0x2b),
            branch_pill: Color32::from_rgb(0x2b, 0x6c, 0xb0),
            remote_pill: Color32::from_rgb(0x8a, 0x5a, 0xb5),
            tag_pill: Color32::from_rgb(0xb0, 0x8a, 0x2b),
            error: Color32::from_rgb(0xd2, 0x0f, 0x39),
            ok: Color32::from_rgb(0x40, 0xa0, 0x2b),
        }
    }

    /// Pick a theme from the terminal background, dark when unknown.
    pub fn from_background(bg: Option<[u8; 3]>) -> Self {
        match bg {
            Some([r, g, b]) => {
                let lum = 0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32;
                let mut t = if lum > 128.0 { Theme::light() } else { Theme::dark() };
                t.background = Color32::from_rgb(r, g, b);
                t
            }
            None => Theme::dark(),
        }
    }

    pub fn pill(&self, kind: RefKind) -> Color32 {
        match kind {
            RefKind::Head => self.head_pill,
            RefKind::LocalBranch => self.branch_pill,
            RefKind::RemoteBranch => self.remote_pill,
            RefKind::Tag => self.tag_pill,
        }
    }

    pub fn graph_color(&self, i: usize) -> Color32 {
        self.graph[i % self.graph.len()]
    }

    pub fn apply(&self, ctx: &egui::Context) {
        let mut visuals = if self.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
        visuals.panel_fill = self.background;
        visuals.window_fill = self.background;
        visuals.extreme_bg_color = if self.dark {
            Color32::from_rgb(0x11, 0x11, 0x11)
        } else {
            Color32::WHITE
        };
        visuals.selection.bg_fill = self.selection;
        ctx.set_visuals(visuals);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_picks_theme() {
        assert!(Theme::from_background(None).dark);
        assert!(Theme::from_background(Some([0x1e, 0x1e, 0x2e])).dark);
        assert!(!Theme::from_background(Some([0xff, 0xff, 0xff])).dark);
        assert_eq!(Theme::from_background(Some([10, 20, 30])).background, Color32::from_rgb(10, 20, 30));
    }
}
