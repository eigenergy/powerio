//! Color palette. The TUI uses a single muted dark palette inspired by
//! Solarized Dark; no theme switching.

use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub bg: Color,
    pub fg: Color,
    pub fg_dim: Color,
    pub accent: Color,
    pub accent_alt: Color,
    pub good: Color,
    pub warn: Color,
    pub bad: Color,
    pub border: Color,
}

pub const T: Theme = Theme {
    bg: Color::Reset,
    fg: Color::Gray,
    fg_dim: Color::DarkGray,
    accent: Color::Cyan,
    accent_alt: Color::Magenta,
    good: Color::LightGreen,
    warn: Color::Yellow,
    bad: Color::LightRed,
    border: Color::DarkGray,
};

#[inline]
pub fn title() -> Style {
    Style::default().fg(T.accent).add_modifier(Modifier::BOLD)
}

#[inline]
pub fn highlight() -> Style {
    Style::default()
        .fg(T.bg)
        .bg(T.accent)
        .add_modifier(Modifier::BOLD)
}

#[inline]
pub fn dim() -> Style {
    Style::default().fg(T.fg_dim)
}

#[inline]
pub fn good() -> Style {
    Style::default().fg(T.good)
}

#[inline]
pub fn bad() -> Style {
    Style::default().fg(T.bad)
}

#[inline]
pub fn warn() -> Style {
    Style::default().fg(T.warn)
}

#[inline]
pub fn border() -> Style {
    Style::default().fg(T.border)
}
