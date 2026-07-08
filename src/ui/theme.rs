use std::{fs, path::PathBuf};

use ratatui::style::Color;
use serde::Deserialize;
use thiserror::Error;

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
    pub fn load(name: &str, mode: ColorMode) -> Result<Self, ThemeError> {
        match name {
            "mica-dark" => return Ok(Self::mica_dark(mode)),
            "mica-light" => return Ok(Self::mica_light(mode)),
            _ => {}
        }
        if name
            != PathBuf::from(name)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("")
        {
            return Err(ThemeError::InvalidName(name.to_owned()));
        }
        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .ok_or(ThemeError::NoConfigHome)?;
        let filename = if name.ends_with(".toml") {
            name.to_owned()
        } else {
            format!("{name}.toml")
        };
        let path = config_home.join("mica/themes").join(filename);
        let source = fs::read_to_string(&path).map_err(|source| ThemeError::Read {
            path: path.clone(),
            source,
        })?;
        let file: ThemeFile = toml::from_str(&source).map_err(|source| ThemeError::Parse {
            path: path.clone(),
            source,
        })?;
        let mut theme = Self::mica_dark(mode);
        theme.apply_colors(file.colors, mode)?;
        Ok(theme)
    }

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

    pub fn mica_light(mode: ColorMode) -> Self {
        let color = |hex| theme_color(hex, mode);
        Self {
            background: color(0xFAFAFC),
            surface: color(0xF1F2F6),
            surface_raised: color(0xFFFFFF),
            border: color(0xD4D7DE),
            text: color(0x242833),
            text_muted: color(0x626B7A),
            text_faint: color(0x8B93A1),
            accent: color(0x315EAF),
            selection: color(0xDCE7FA),
            active_line: color(0xF0F4FA),
            cursor: color(0x172033),
            syntax_keyword: color(0x7C3E9D),
            syntax_function: color(0x245AA8),
            syntax_type: color(0x8A5B00),
            syntax_string: color(0x357A38),
            syntax_number: color(0xB24728),
            syntax_comment: color(0x747D8C),
            syntax_variable: color(0x242833),
            syntax_constant: color(0x087F8C),
            git_added: color(0x2E7D32),
            git_modified: color(0xA06400),
            git_deleted: color(0xC62828),
            git_conflict: color(0xC45100),
            diagnostic_error: color(0xC62828),
            diagnostic_warning: color(0x966000),
            diagnostic_info: color(0x245AA8),
            diagnostic_hint: color(0x087F8C),
            diff_add_bg: color(0xDDF2DF),
            diff_delete_bg: color(0xF8DFE1),
        }
    }

    fn apply_colors(&mut self, colors: ThemeColors, mode: ColorMode) -> Result<(), ThemeError> {
        macro_rules! apply {
            ($field:ident) => {
                if let Some(value) = colors.$field {
                    self.$field = parse_color(stringify!($field), &value, mode)?;
                }
            };
        }
        apply!(background);
        apply!(surface);
        apply!(surface_raised);
        apply!(border);
        apply!(text);
        apply!(text_muted);
        apply!(text_faint);
        apply!(accent);
        apply!(selection);
        apply!(active_line);
        apply!(cursor);
        apply!(syntax_keyword);
        apply!(syntax_function);
        apply!(syntax_type);
        apply!(syntax_string);
        apply!(syntax_number);
        apply!(syntax_comment);
        apply!(syntax_variable);
        apply!(syntax_constant);
        apply!(git_added);
        apply!(git_modified);
        apply!(git_deleted);
        apply!(git_conflict);
        apply!(diagnostic_error);
        apply!(diagnostic_warning);
        apply!(diagnostic_info);
        apply!(diagnostic_hint);
        apply!(diff_add_bg);
        apply!(diff_delete_bg);
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum ThemeError {
    #[error("invalid theme name: {0}")]
    InvalidName(String),
    #[error("cannot locate the configuration directory")]
    NoConfigHome,
    #[error("failed to read theme {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse theme {path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("invalid color for {token}: {value}; expected #RRGGBB")]
    InvalidColor { token: &'static str, value: String },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFile {
    colors: ThemeColors,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ThemeColors {
    background: Option<String>,
    surface: Option<String>,
    surface_raised: Option<String>,
    border: Option<String>,
    text: Option<String>,
    text_muted: Option<String>,
    text_faint: Option<String>,
    accent: Option<String>,
    selection: Option<String>,
    active_line: Option<String>,
    cursor: Option<String>,
    syntax_keyword: Option<String>,
    syntax_function: Option<String>,
    syntax_type: Option<String>,
    syntax_string: Option<String>,
    syntax_number: Option<String>,
    syntax_comment: Option<String>,
    syntax_variable: Option<String>,
    syntax_constant: Option<String>,
    git_added: Option<String>,
    git_modified: Option<String>,
    git_deleted: Option<String>,
    git_conflict: Option<String>,
    diagnostic_error: Option<String>,
    diagnostic_warning: Option<String>,
    diagnostic_info: Option<String>,
    diagnostic_hint: Option<String>,
    diff_add_bg: Option<String>,
    diff_delete_bg: Option<String>,
}

fn parse_color(token: &'static str, value: &str, mode: ColorMode) -> Result<Color, ThemeError> {
    let hex = value
        .strip_prefix('#')
        .filter(|value| value.len() == 6)
        .ok_or_else(|| ThemeError::InvalidColor {
            token,
            value: value.to_owned(),
        })?;
    let value = u32::from_str_radix(hex, 16).map_err(|_| ThemeError::InvalidColor {
        token,
        value: value.to_owned(),
    })?;
    Ok(theme_color(value, mode))
}

fn theme_color(hex: u32, mode: ColorMode) -> Color {
    let red = ((hex >> 16) & 0xff) as u8;
    let green = ((hex >> 8) & 0xff) as u8;
    let blue = (hex & 0xff) as u8;
    match mode {
        ColorMode::TrueColor => Color::Rgb(red, green, blue),
        ColorMode::Ansi256 => Color::Indexed(nearest_xterm(red, green, blue)),
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

    #[test]
    fn light_theme_is_distinct_and_supports_ansi256() {
        let dark = Theme::mica_dark(ColorMode::TrueColor);
        let light = Theme::mica_light(ColorMode::TrueColor);
        assert_ne!(light.background, dark.background);
        assert!(matches!(
            Theme::mica_light(ColorMode::Ansi256).background,
            Color::Indexed(_)
        ));
    }

    #[test]
    fn external_theme_colors_overlay_defaults_and_validate_hex() {
        let mut theme = Theme::mica_dark(ColorMode::TrueColor);
        theme
            .apply_colors(
                ThemeColors {
                    accent: Some("#123456".to_owned()),
                    ..ThemeColors::default()
                },
                ColorMode::TrueColor,
            )
            .unwrap();
        assert_eq!(theme.accent, Color::Rgb(0x12, 0x34, 0x56));
        assert!(matches!(
            parse_color("accent", "blue", ColorMode::TrueColor),
            Err(ThemeError::InvalidColor { .. })
        ));
    }
}
