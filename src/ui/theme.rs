//! Colors derived from the terminal palette (OSC 10/11 query, dark fallback)
//! and the iced theme built from them.

use iced_core::theme::Palette;
use iced_core::Color;

use crate::git::repo::RefKind;

fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgb8(r, g, b)
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub background: Color,
    /// Slightly raised surface: pane title bars, footer, dialogs.
    pub panel: Color,
    /// Sunken surface: text fields, code.
    pub well: Color,
    pub border: Color,
    pub text: Color,
    pub weak: Color,
    pub strong: Color,
    pub accent: Color,
    pub graph: [Color; 8],
    pub add_bg: Color,
    pub del_bg: Color,
    pub add_fg: Color,
    pub del_fg: Color,
    pub hunk_bg: Color,
    pub hunk_fg: Color,
    pub line_no: Color,
    pub selection: Color,
    pub selection_inactive: Color,
    pub head_pill: Color,
    pub branch_pill: Color,
    pub remote_pill: Color,
    pub tag_pill: Color,
    pub error: Color,
    pub ok: Color,
    pub warn: Color,
}

impl Theme {
    pub fn dark() -> Self {
        Theme {
            background: rgb(0x1b, 0x1b, 0x1b),
            panel: rgb(0x24, 0x24, 0x26),
            well: rgb(0x15, 0x15, 0x16),
            border: rgb(0x33, 0x34, 0x3a),
            text: rgb(0xd6, 0xd6, 0xd6),
            weak: rgb(0x8b, 0x8d, 0x96),
            strong: rgb(0xf4, 0xf4, 0xf4),
            accent: rgb(0x89, 0xb4, 0xfa),
            graph: [
                rgb(0x89, 0xb4, 0xfa),
                rgb(0xa6, 0xe3, 0xa1),
                rgb(0xf9, 0xe2, 0xaf),
                rgb(0xf3, 0x8b, 0xa8),
                rgb(0xcb, 0xa6, 0xf7),
                rgb(0x94, 0xe2, 0xd5),
                rgb(0xfa, 0xb3, 0x87),
                rgb(0x74, 0xc7, 0xec),
            ],
            add_bg: rgb(0x1e, 0x36, 0x24),
            del_bg: rgb(0x3d, 0x1f, 0x22),
            add_fg: rgb(0xa6, 0xe3, 0xa1),
            del_fg: rgb(0xf3, 0x8b, 0xa8),
            hunk_bg: rgb(0x26, 0x2d, 0x3a),
            hunk_fg: rgb(0x89, 0xb4, 0xfa),
            line_no: rgb(0x6c, 0x70, 0x86),
            selection: rgb(0x2f, 0x4a, 0x6e),
            selection_inactive: rgb(0x2a, 0x2f, 0x3a),
            head_pill: rgb(0x40, 0xa0, 0x2b),
            branch_pill: rgb(0x2b, 0x6c, 0xb0),
            remote_pill: rgb(0x8a, 0x5a, 0xb5),
            tag_pill: rgb(0xb0, 0x8a, 0x2b),
            error: rgb(0xf3, 0x8b, 0xa8),
            ok: rgb(0xa6, 0xe3, 0xa1),
            warn: rgb(0xf9, 0xe2, 0xaf),
        }
    }

    pub fn light() -> Self {
        Theme {
            background: rgb(0xf6, 0xf6, 0xf6),
            panel: rgb(0xec, 0xec, 0xee),
            well: rgb(0xff, 0xff, 0xff),
            border: rgb(0xd4, 0xd4, 0xda),
            text: rgb(0x2a, 0x2a, 0x2e),
            weak: rgb(0x7c, 0x7f, 0x91),
            strong: rgb(0x10, 0x10, 0x12),
            accent: rgb(0x1e, 0x66, 0xf5),
            graph: [
                rgb(0x1e, 0x66, 0xf5),
                rgb(0x40, 0xa0, 0x2b),
                rgb(0xdf, 0x8e, 0x1d),
                rgb(0xd2, 0x0f, 0x39),
                rgb(0x88, 0x39, 0xef),
                rgb(0x17, 0x92, 0x99),
                rgb(0xfe, 0x64, 0x0b),
                rgb(0x04, 0xa5, 0xe5),
            ],
            add_bg: rgb(0xdd, 0xf4, 0xdd),
            del_bg: rgb(0xfb, 0xe0, 0xe0),
            add_fg: rgb(0x1a, 0x7f, 0x37),
            del_fg: rgb(0xcf, 0x22, 0x2e),
            hunk_bg: rgb(0xdd, 0xe8, 0xff),
            hunk_fg: rgb(0x1e, 0x66, 0xf5),
            line_no: rgb(0x8c, 0x8f, 0xa1),
            selection: rgb(0xc7, 0xdc, 0xf8),
            selection_inactive: rgb(0xe4, 0xe4, 0xe8),
            head_pill: rgb(0x40, 0xa0, 0x2b),
            branch_pill: rgb(0x2b, 0x6c, 0xb0),
            remote_pill: rgb(0x8a, 0x5a, 0xb5),
            tag_pill: rgb(0xb0, 0x8a, 0x2b),
            error: rgb(0xd2, 0x0f, 0x39),
            ok: rgb(0x40, 0xa0, 0x2b),
            warn: rgb(0xdf, 0x8e, 0x1d),
        }
    }

    /// Pick a theme from the terminal background, dark when unknown.
    pub fn from_background(bg: Option<[u8; 3]>) -> Self {
        match bg {
            Some([r, g, b]) => {
                let lum = 0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32;
                let mut t = if lum > 128.0 { Theme::light() } else { Theme::dark() };
                t.background = rgb(r, g, b);
                t
            }
            None => Theme::dark(),
        }
    }

    pub fn pill(&self, kind: RefKind) -> Color {
        match kind {
            RefKind::Head => self.head_pill,
            RefKind::LocalBranch => self.branch_pill,
            RefKind::RemoteBranch => self.remote_pill,
            RefKind::Tag => self.tag_pill,
        }
    }

    pub fn graph_color(&self, i: usize) -> Color {
        self.graph[i % self.graph.len()]
    }

    /// The iced theme every widget styles itself from.
    pub fn iced(&self) -> iced_core::Theme {
        iced_core::Theme::custom(
            "gitgui".to_owned(),
            Palette {
                background: self.background,
                text: self.text,
                primary: self.accent,
                success: self.ok,
                warning: self.warn,
                danger: self.error,
            },
        )
    }
}

/// `color` with the given alpha.
pub fn alpha(color: Color, a: f32) -> Color {
    Color { a, ..color }
}
