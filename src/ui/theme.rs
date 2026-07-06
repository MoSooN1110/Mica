use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    TrueColor,
    Ansi256,
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub background: Color,
    pub surface: Color,
    pub surface_raised: Color,
    pub border: Color,
    pub text: Color,
    pub text_muted: Color,
    pub text_faint: Color,
    pub accent: Color,
    pub selection: Color,
    pub active_line: Color,
    pub cursor: Color,
    pub syntax_keyword: Color,
    pub syntax_function: Color,
    pub syntax_type: Color,
    pub syntax_string: Color,
    pub syntax_number: Color,
    pub syntax_comment: Color,
    pub syntax_variable: Color,
    pub syntax_constant: Color,
    pub git_added: Color,
    pub git_modified: Color,
    pub git_deleted: Color,
    pub git_conflict: Color,
    pub diagnostic_error: Color,
    pub diagnostic_warning: Color,
    pub diagnostic_info: Color,
    pub diagnostic_hint: Color,
    /// Subtle background tint behind added diff lines (SPEC/01_ui.md §6.2
    /// extension). Dark enough to keep `syntax_*`/`text` foreground colors
    /// readable on top; distinct from `background`/`surface` in both
    /// TrueColor and the 256-color approximation.
    pub diff_add_bg: Color,
    /// Subtle background tint behind deleted diff lines. See
    /// [`Theme::diff_add_bg`].
    pub diff_delete_bg: Color,
}

impl Theme {
    pub fn mica_dark(mode: ColorMode) -> Self {
        let color = |hex: u32| {
            let red = ((hex >> 16) & 0xff) as u8;
            let green = ((hex >> 8) & 0xff) as u8;
            let blue = (hex & 0xff) as u8;
            match mode {
                ColorMode::TrueColor => Color::Rgb(red, green, blue),
                ColorMode::Ansi256 => Color::Indexed(nearest_xterm(red, green, blue)),
            }
        };
        Self {
            background: color(0x10131A),
            surface: color(0x151923),
            surface_raised: color(0x1B202C),
            border: color(0x2A3140),
            text: color(0xD8DEE9),
            text_muted: color(0x8B95A7),
            text_faint: color(0x626C7D),
            accent: color(0x82AFFF),
            selection: color(0x26344D),
            active_line: color(0x171C27),
            cursor: color(0xE6EDF7),
            syntax_keyword: color(0xC792EA),
            syntax_function: color(0x82AAFF),
            syntax_type: color(0xFFCB6B),
            syntax_string: color(0xC3E88D),
            syntax_number: color(0xF78C6C),
            syntax_comment: color(0x687487),
            syntax_variable: color(0xD8DEE9),
            syntax_constant: color(0x89DDFF),
            git_added: color(0x9ECE6A),
            git_modified: color(0xE0AF68),
            git_deleted: color(0xF7768E),
            git_conflict: color(0xFF9E64),
            diagnostic_error: color(0xFF6B81),
            diagnostic_warning: color(0xEBCB8B),
            diagnostic_info: color(0x7AA2F7),
            diagnostic_hint: color(0x73DACA),
            // Chosen (rather than the spec brief's illustrative #16211A /
            // #251A1E) because those values alias to the same 256-color
            // index as `surface`/`surface_raised` under `nearest_xterm`'s
            // Euclidean quantization: they're too close to neutral gray to
            // land on a color-cube entry instead of the grayscale ramp.
            // These stay dark (readable text on top) while landing on
            // distinct green/red cube entries in Ansi256 (see theme tests).
            diff_add_bg: color(0x124612),
            diff_delete_bg: color(0x461212),
        }
    }
}

fn nearest_xterm(red: u8, green: u8, blue: u8) -> u8 {
    let mut best_index = 16;
    let mut best_distance = u32::MAX;
    for index in 16..=255 {
        let (r, g, b) = xterm_rgb(index);
        let dr = i32::from(red) - i32::from(r);
        let dg = i32::from(green) - i32::from(g);
        let db = i32::from(blue) - i32::from(b);
        let distance = (dr * dr + dg * dg + db * db) as u32;
        if distance < best_distance {
            best_distance = distance;
            best_index = index;
        }
    }
    best_index
}

fn xterm_rgb(index: u8) -> (u8, u8, u8) {
    if index >= 232 {
        let value = 8 + (index - 232) * 10;
        return (value, value, value);
    }
    let cube = index - 16;
    let level = |value: u8| if value == 0 { 0 } else { 55 + value * 40 };
    (level(cube / 36), level((cube % 36) / 6), level(cube % 6))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_theme_contains_only_indexed_colors() {
        let theme = Theme::mica_dark(ColorMode::Ansi256);
        assert!(matches!(theme.background, Color::Indexed(_)));
        assert!(matches!(theme.accent, Color::Indexed(_)));
    }

    #[test]
    fn diff_backgrounds_stay_distinct_from_background_and_surface_in_ansi256() {
        let theme = Theme::mica_dark(ColorMode::Ansi256);
        assert_ne!(theme.diff_add_bg, theme.background);
        assert_ne!(theme.diff_add_bg, theme.surface);
        assert_ne!(theme.diff_delete_bg, theme.background);
        assert_ne!(theme.diff_delete_bg, theme.surface);
        assert_ne!(theme.diff_add_bg, theme.diff_delete_bg);
    }

    #[test]
    fn diagnostic_info_and_hint_tokens_are_present_and_distinct() {
        let theme = Theme::mica_dark(ColorMode::TrueColor);
        assert_ne!(theme.diagnostic_info, theme.diagnostic_error);
        assert_ne!(theme.diagnostic_hint, theme.diagnostic_warning);
        assert_ne!(theme.diagnostic_info, theme.diagnostic_hint);
    }
}
