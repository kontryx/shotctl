use ratatui::{buffer::Buffer, style::Color};

use crate::{
    config::Theme,
    ui::{BG, BORDER, CYAN, GREEN, MUTED, PANEL, RED, SELECTED, TEXT, YELLOW},
};

/// Each dark theme colour and the light theme colour in the same role.
const LIGHT: [(Color, Color); 10] = [
    (BG, Color::Rgb(240, 243, 247)),
    (PANEL, Color::Rgb(255, 255, 255)),
    (TEXT, Color::Rgb(30, 41, 56)),
    (MUTED, Color::Rgb(92, 107, 128)),
    (CYAN, Color::Rgb(0, 118, 145)),
    (GREEN, Color::Rgb(22, 128, 72)),
    (YELLOW, Color::Rgb(161, 98, 0)),
    (RED, Color::Rgb(200, 40, 40)),
    (SELECTED, Color::Rgb(214, 232, 244)),
    (BORDER, Color::Rgb(178, 192, 208)),
];

fn in_theme(color: Color, theme: Theme) -> Color {
    match theme {
        Theme::Dark => color,
        Theme::Light => LIGHT
            .iter()
            .find(|(dark, _)| *dark == color)
            .map_or(color, |(_, light)| *light),
    }
}

/// How colours are written to the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    /// 24-bit RGB, exactly as designed.
    TrueColor,
    /// Nearest entries 16–255 of the xterm palette. Unlike entries 0–15, these are
    /// not part of terminal colour themes, so the UI keeps its own look.
    Palette256,
}

impl ColorMode {
    pub fn detect() -> Self {
        Self::from_env(|name| std::env::var(name).ok())
    }

    /// `SHOTCTL_COLOR=truecolor|256` overrides detection. Unknown terminals get the
    /// 256-colour palette: terminals without 24-bit support misread RGB codes as
    /// unrelated attributes and theme colours.
    pub fn from_env(var: impl Fn(&str) -> Option<String>) -> Self {
        let value = |name| var(name).unwrap_or_default().to_ascii_lowercase();
        match value("SHOTCTL_COLOR").as_str() {
            "truecolor" | "24bit" => return Self::TrueColor,
            "256" => return Self::Palette256,
            _ => {}
        }
        if matches!(value("COLORTERM").as_str(), "truecolor" | "24bit") {
            return Self::TrueColor;
        }
        let truecolor = match var("TERM_PROGRAM").unwrap_or_default().as_str() {
            // Terminal.app gained 24-bit colour in macOS 26 (build 466); macOS 15 ships 455.
            "Apple_Terminal" => var("TERM_PROGRAM_VERSION")
                .and_then(|version| version.split('.').next()?.parse::<u32>().ok())
                .is_some_and(|build| build >= 460),
            "iTerm.app" | "WezTerm" | "ghostty" | "vscode" | "WarpTerminal" | "Hyper" | "Tabby" => {
                true
            }
            _ => false,
        };
        let term = value("TERM");
        if truecolor
            || var("WT_SESSION").is_some()
            || [
                "truecolor",
                "24bit",
                "direct",
                "kitty",
                "alacritty",
                "ghostty",
                "wezterm",
            ]
            .iter()
            .any(|name| term.contains(name))
        {
            Self::TrueColor
        } else {
            Self::Palette256
        }
    }
}

/// Recolours a frame drawn in the dark theme, and makes every cell independent of the
/// user's terminal theme.
pub fn apply(buffer: &mut Buffer, theme: Theme, mode: ColorMode) {
    for cell in &mut buffer.content {
        if cell.fg == Color::Reset {
            cell.fg = TEXT;
        }
        if cell.bg == Color::Reset {
            cell.bg = BG;
        }
        cell.fg = in_theme(cell.fg, theme);
        cell.bg = in_theme(cell.bg, theme);
        if mode == ColorMode::Palette256 {
            cell.fg = to_palette(cell.fg);
            cell.bg = to_palette(cell.bg);
        }
    }
}

const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

pub fn to_palette(color: Color) -> Color {
    let Color::Rgb(r, g, b) = color else {
        return color;
    };
    let distance = |(x, y, z): (u8, u8, u8)| {
        let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).pow(2);
        d(r, x) + d(g, y) + d(b, z)
    };
    let level = |value: u8| {
        (0..CUBE.len())
            .min_by_key(|&i| (i32::from(CUBE[i]) - i32::from(value)).abs())
            .unwrap()
    };
    let average = (u32::from(r) + u32::from(g) + u32::from(b)) / 3;
    // Near-neutral colours would pick up a cast from the coarse cube (slate turns
    // lavender), so they only consider the cube's own greys.
    let neutral = r.max(g).max(b) - r.min(g).min(b) < 50;
    let (ri, gi, bi) = if neutral {
        let level = level(average as u8);
        (level, level, level)
    } else {
        (level(r), level(g), level(b))
    };
    let cube = (CUBE[ri], CUBE[gi], CUBE[bi]);
    let cube_index = 16 + 36 * ri + 6 * gi + bi;

    let step = (average.saturating_sub(3) / 10).min(23);
    let grey = (8 + 10 * step) as u8;
    let grey_index = 232 + step as usize;

    if distance((grey, grey, grey)) < distance(cube) {
        Color::Indexed(grey_index as u8)
    } else {
        Color::Indexed(cube_index as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn mode(vars: &[(&str, &str)]) -> ColorMode {
        let vars: HashMap<_, _> = vars.iter().copied().collect();
        ColorMode::from_env(|name| vars.get(name).map(|value| value.to_string()))
    }

    #[test]
    fn detects_terminals_that_support_24_bit_colour() {
        assert_eq!(mode(&[("COLORTERM", "truecolor")]), ColorMode::TrueColor);
        assert_eq!(mode(&[("TERM_PROGRAM", "iTerm.app")]), ColorMode::TrueColor);
        assert_eq!(mode(&[("TERM", "xterm-kitty")]), ColorMode::TrueColor);
        assert_eq!(
            mode(&[
                ("TERM_PROGRAM", "Apple_Terminal"),
                ("TERM_PROGRAM_VERSION", "466")
            ]),
            ColorMode::TrueColor
        );
    }

    #[test]
    fn older_terminal_app_and_unknown_terminals_use_the_256_colour_palette() {
        assert_eq!(
            mode(&[
                ("TERM_PROGRAM", "Apple_Terminal"),
                ("TERM_PROGRAM_VERSION", "455.1")
            ]),
            ColorMode::Palette256
        );
        assert_eq!(mode(&[("TERM", "xterm-256color")]), ColorMode::Palette256);
        assert_eq!(mode(&[]), ColorMode::Palette256);
    }

    #[test]
    fn shotctl_color_overrides_detection() {
        assert_eq!(
            mode(&[("SHOTCTL_COLOR", "256"), ("COLORTERM", "truecolor")]),
            ColorMode::Palette256
        );
        assert_eq!(
            mode(&[("SHOTCTL_COLOR", "truecolor"), ("TERM", "xterm")]),
            ColorMode::TrueColor
        );
    }

    #[test]
    fn palette_colours_avoid_the_themeable_first_sixteen() {
        for r in (0..=255).step_by(17) {
            for g in (0..=255).step_by(17) {
                for b in (0..=255).step_by(17) {
                    let Color::Indexed(index) = to_palette(Color::Rgb(r, g, b)) else {
                        panic!("expected an indexed colour");
                    };
                    assert!(index >= 16);
                }
            }
        }
        assert_eq!(to_palette(Color::Rgb(91, 214, 231)), Color::Indexed(80));
    }

    fn is_grey(index: u8) -> bool {
        index >= 232 || [16, 59, 102, 145, 188, 231].contains(&index)
    }

    #[test]
    fn near_neutral_theme_colours_become_greys_without_a_tint() {
        for theme in [Theme::Dark, Theme::Light] {
            for color in [BG, PANEL, TEXT, MUTED, SELECTED, BORDER] {
                let Color::Indexed(index) = to_palette(in_theme(color, theme)) else {
                    panic!("expected an indexed colour");
                };
                assert!(is_grey(index), "{theme:?} {color:?} became tinted {index}");
            }
        }
    }

    #[test]
    fn backgrounds_stay_distinct_in_the_256_colour_palette() {
        for theme in [Theme::Dark, Theme::Light] {
            let [bg, panel, selected] =
                [BG, PANEL, SELECTED].map(|color| to_palette(in_theme(color, theme)));
            assert!(
                bg != panel && panel != selected && bg != selected,
                "{theme:?}"
            );
        }
    }

    #[test]
    fn the_light_theme_replaces_every_dark_colour() {
        for (dark, light) in LIGHT {
            assert_eq!(in_theme(dark, Theme::Dark), dark);
            assert_eq!(in_theme(dark, Theme::Light), light);
            assert!(LIGHT.iter().all(|(other, _)| *other != light));
        }
    }

    #[test]
    fn unstyled_cells_take_the_theme_instead_of_the_terminal_defaults() {
        let area = ratatui::layout::Rect::new(0, 0, 2, 1);
        for (theme, fg, bg) in [
            (Theme::Dark, TEXT, BG),
            (Theme::Light, LIGHT[2].1, LIGHT[0].1),
        ] {
            let mut buffer = Buffer::empty(area);
            apply(&mut buffer, theme, ColorMode::TrueColor);
            assert!(
                buffer
                    .content
                    .iter()
                    .all(|cell| cell.fg == fg && cell.bg == bg)
            );
        }
    }
}
