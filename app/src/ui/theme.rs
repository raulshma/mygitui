//! Native port of the shadcn-svelte token set.
//!
//! The former web UI defined its palette as CSS custom properties in
//! `oklch()`; the values below are that palette converted once to 8-bit
//! gamma-encoded sRGB. Widgets take [`Tokens`] and build their styles from
//! it, so light/dark switching needs no stylesheet.

use iced::font::{Family, Font, Weight};
use iced::widget::button;
use iced::{Background, Border, Color, Shadow, Theme};
use serde::{Deserialize, Serialize};

/// User preference for the colour scheme.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Follow the OS.
    #[default]
    System,
    Light,
    Dark,
}

impl Mode {
    pub fn next(self) -> Self {
        match self {
            Self::System => Self::Light,
            Self::Light => Self::Dark,
            Self::Dark => Self::System,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

/// Corner radius used for controls (shadcn `--radius: 0.625rem`).
pub const RADIUS: f32 = 6.0;

const fn rgb(hex: u32) -> Color {
    Color {
        r: ((hex >> 16) & 0xff) as f32 / 255.0,
        g: ((hex >> 8) & 0xff) as f32 / 255.0,
        b: (hex & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

const fn rgba(hex: u32, a: f32) -> Color {
    let c = rgb(hex);
    Color { a, ..c }
}

/// One resolved palette. Every field maps to a token the old CSS exposed.
#[derive(Debug, Clone, Copy)]
pub struct Tokens {
    pub dark: bool,
    pub background: Color,
    pub foreground: Color,
    pub card: Color,
    pub card_foreground: Color,
    pub popover: Color,
    pub popover_foreground: Color,
    pub primary: Color,
    pub primary_foreground: Color,
    pub secondary: Color,
    pub secondary_foreground: Color,
    pub muted: Color,
    pub muted_foreground: Color,
    pub accent: Color,
    pub accent_foreground: Color,
    pub destructive: Color,
    pub border: Color,
    pub input: Color,
    pub ring: Color,
    pub sidebar: Color,
    pub sidebar_foreground: Color,
    pub sidebar_primary: Color,
    pub sidebar_primary_foreground: Color,
    pub sidebar_accent: Color,
    pub sidebar_accent_foreground: Color,
    pub sidebar_border: Color,
    pub sidebar_ring: Color,
    pub added: Color,
    pub deleted: Color,
    pub modified: Color,
    pub renamed: Color,
    pub untracked: Color,
    pub conflicted: Color,
    pub hover: Color,
    pub active: Color,
    pub disabled_bg: Color,
    pub disabled_fg: Color,
}

impl Tokens {
    /// Light scheme: `--background: oklch(1 0 0) …` from the old `:root`.
    pub const LIGHT: Self = Self {
        dark: false,
        background: rgb(0xffffff),
        foreground: rgb(0x0a0a0a),
        card: rgb(0xffffff),
        card_foreground: rgb(0x0a0a0a),
        popover: rgb(0xffffff),
        popover_foreground: rgb(0x0a0a0a),
        primary: rgb(0x171717),
        primary_foreground: rgb(0xfafafa),
        secondary: rgb(0xf5f5f5),
        secondary_foreground: rgb(0x171717),
        muted: rgb(0xf5f5f5),
        muted_foreground: rgb(0x737373),
        accent: rgb(0xf5f5f5),
        accent_foreground: rgb(0x171717),
        destructive: rgb(0xe7000b),
        border: rgb(0xe5e5e5),
        input: rgb(0xe5e5e5),
        ring: rgb(0xa1a1a1),
        sidebar: rgb(0xfafafa),
        sidebar_foreground: rgb(0x0a0a0a),
        sidebar_primary: rgb(0x171717),
        sidebar_primary_foreground: rgb(0xfafafa),
        sidebar_accent: rgb(0xf5f5f5),
        sidebar_accent_foreground: rgb(0x171717),
        sidebar_border: rgb(0xe5e5e5),
        sidebar_ring: rgb(0xa1a1a1),
        added: rgb(0x1a7f37),
        deleted: rgb(0xcf222e),
        modified: rgb(0x9a6700),
        renamed: rgb(0x8250df),
        untracked: rgb(0x1a7f37),
        conflicted: rgb(0xcf222e),
        hover: rgba(0x0a0a0a, 0.05),
        active: rgba(0x171717, 0.15),
        disabled_bg: rgba(0x0a0a0a, 0.12),
        disabled_fg: rgba(0x0a0a0a, 0.38),
    };

    /// Dark scheme: the `.dark` token block plus the dark `--app-*` hues.
    pub const DARK: Self = Self {
        dark: true,
        background: rgb(0x0a0a0a),
        foreground: rgb(0xfafafa),
        card: rgb(0x171717),
        card_foreground: rgb(0xfafafa),
        popover: rgb(0x171717),
        popover_foreground: rgb(0xfafafa),
        primary: rgb(0xe5e5e5),
        primary_foreground: rgb(0x171717),
        secondary: rgb(0x262626),
        secondary_foreground: rgb(0xfafafa),
        muted: rgb(0x262626),
        muted_foreground: rgb(0xa1a1a1),
        accent: rgb(0x262626),
        accent_foreground: rgb(0xfafafa),
        destructive: rgb(0xff6467),
        border: rgba(0xffffff, 0.10),
        input: rgba(0xffffff, 0.15),
        ring: rgb(0x737373),
        sidebar: rgb(0x171717),
        sidebar_foreground: rgb(0xfafafa),
        sidebar_primary: rgb(0x1447e6),
        sidebar_primary_foreground: rgb(0xfafafa),
        sidebar_accent: rgb(0x262626),
        sidebar_accent_foreground: rgb(0xfafafa),
        sidebar_border: rgba(0xffffff, 0.10),
        sidebar_ring: rgb(0x737373),
        added: rgb(0x3fb950),
        deleted: rgb(0xf85149),
        modified: rgb(0xd29922),
        renamed: rgb(0xa371f7),
        untracked: rgb(0x3fb950),
        conflicted: rgb(0xf85149),
        hover: rgba(0xfafafa, 0.08),
        active: rgba(0xe5e5e5, 0.15),
        disabled_bg: rgba(0xfafafa, 0.12),
        disabled_fg: rgba(0xfafafa, 0.38),
    };

    pub fn for_scheme(dark: bool) -> Self {
        if dark {
            Self::DARK
        } else {
            Self::LIGHT
        }
    }

    pub fn added_bg(&self) -> Color {
        with_alpha(self.added, 0.13)
    }

    pub fn deleted_bg(&self) -> Color {
        with_alpha(self.deleted, 0.13)
    }

    pub fn modified_bg(&self) -> Color {
        with_alpha(self.modified, 0.14)
    }

    pub fn renamed_bg(&self) -> Color {
        with_alpha(self.renamed, 0.13)
    }

    pub fn untracked_bg(&self) -> Color {
        with_alpha(self.untracked, 0.12)
    }

    pub fn conflicted_bg(&self) -> Color {
        with_alpha(self.conflicted, 0.16)
    }
}

pub fn with_alpha(color: Color, a: f32) -> Color {
    Color { a, ..color }
}

/// Linear sRGB interpolation; used for hover/pressed shades.
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

/// Base iced theme (mainly for widgets that still consult the palette).
pub fn app_theme(tokens: Tokens) -> Theme {
    Theme::custom(
        if tokens.dark {
            "mygitui dark"
        } else {
            "mygitui light"
        },
        iced::theme::Palette {
            background: tokens.background,
            text: tokens.foreground,
            primary: tokens.primary,
            success: tokens.added,
            warning: tokens.modified,
            danger: tokens.destructive,
        },
    )
}

pub fn mono() -> Font {
    Font {
        family: Family::Monospace,
        ..Font::DEFAULT
    }
}

pub fn semibold() -> Font {
    Font {
        weight: Weight::Semibold,
        ..Font::DEFAULT
    }
}

/// Visual variants shared by every button in the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

pub fn button_style(
    tokens: Tokens,
    kind: ButtonKind,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let (base_bg, base_fg, base_border) = match kind {
            ButtonKind::Primary => (
                Some(tokens.primary),
                tokens.primary_foreground,
                with_alpha(tokens.primary, 0.0),
            ),
            ButtonKind::Secondary => (
                Some(tokens.secondary),
                tokens.secondary_foreground,
                tokens.border,
            ),
            ButtonKind::Ghost => (None, tokens.foreground, with_alpha(tokens.accent, 0.0)),
            ButtonKind::Danger => (
                Some(tokens.destructive),
                tokens.primary_foreground,
                with_alpha(tokens.destructive, 0.0),
            ),
        };

        let (background, text_color) = match status {
            button::Status::Active => (base_bg, base_fg),
            button::Status::Hovered => {
                let bg = match kind {
                    ButtonKind::Primary => mix(
                        tokens.primary,
                        tokens.foreground,
                        if tokens.dark { 0.18 } else { 0.12 },
                    ),
                    ButtonKind::Secondary => tokens.accent,
                    ButtonKind::Ghost => tokens.hover,
                    ButtonKind::Danger => mix(tokens.destructive, Color::WHITE, 0.12),
                };
                (Some(bg), base_fg)
            }
            button::Status::Pressed => {
                let bg = match kind {
                    ButtonKind::Primary => mix(tokens.primary, tokens.foreground, 0.25),
                    ButtonKind::Secondary => tokens.active,
                    ButtonKind::Ghost => tokens.active,
                    ButtonKind::Danger => mix(tokens.destructive, Color::BLACK, 0.12),
                };
                (Some(bg), base_fg)
            }
            button::Status::Disabled => (Some(tokens.disabled_bg), tokens.disabled_fg),
        };

        button::Style {
            background: background.map(Background::Color),
            text_color,
            border: Border {
                color: base_border,
                width: 1.0,
                radius: RADIUS.into(),
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// Card/panel surface: card colour, hairline border.
pub fn panel_style(tokens: Tokens) -> impl Fn(&Theme) -> iced::widget::container::Style {
    move |_theme| iced::widget::container::Style {
        text_color: Some(tokens.card_foreground),
        background: Some(Background::Color(tokens.card)),
        border: Border {
            color: tokens.border,
            width: 1.0,
            radius: RADIUS.into(),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// Flat surface on the window background.
pub fn surface_style(tokens: Tokens) -> impl Fn(&Theme) -> iced::widget::container::Style {
    move |_theme| iced::widget::container::Style {
        text_color: Some(tokens.foreground),
        background: Some(Background::Color(tokens.background)),
        border: Border::default(),
        shadow: Shadow::default(),
        snap: true,
    }
}

/// Separator rule colour.
pub fn rule_color(tokens: Tokens) -> Color {
    tokens.border
}
