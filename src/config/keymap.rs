use std::{collections::HashMap, str::FromStr};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

#[derive(Debug, Error)]
pub enum KeymapError {
    #[error("invalid key chord: {0}")]
    InvalidChord(String),
}

impl FromStr for KeyChord {
    type Err = KeymapError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let mut modifiers = KeyModifiers::empty();
        let mut code = None;
        for part in value.to_ascii_lowercase().split('-') {
            match part {
                "ctrl" | "control" => modifiers.insert(KeyModifiers::CONTROL),
                "shift" => modifiers.insert(KeyModifiers::SHIFT),
                "alt" => modifiers.insert(KeyModifiers::ALT),
                "enter" => code = Some(KeyCode::Enter),
                "esc" | "escape" => code = Some(KeyCode::Esc),
                "tab" => code = Some(KeyCode::Tab),
                "backspace" => code = Some(KeyCode::Backspace),
                "delete" => code = Some(KeyCode::Delete),
                "up" => code = Some(KeyCode::Up),
                "down" => code = Some(KeyCode::Down),
                "left" => code = Some(KeyCode::Left),
                "right" => code = Some(KeyCode::Right),
                "space" => code = Some(KeyCode::Char(' ')),
                value if value.chars().count() == 1 => {
                    code = value.chars().next().map(KeyCode::Char)
                }
                _ => return Err(KeymapError::InvalidChord(value.to_owned())),
            }
        }
        code.map(|code| Self { code, modifiers })
            .ok_or_else(|| KeymapError::InvalidChord(value.to_owned()))
    }
}

#[derive(Debug, Clone)]
pub struct Keymap {
    bindings: HashMap<KeyChord, String>,
}

impl Default for Keymap {
    fn default() -> Self {
        let defaults = [
            ("ctrl-s", "editor.save"),
            ("ctrl-shift-s", "editor.save_as"),
            ("ctrl-w", "editor.close"),
            ("ctrl-c", "editor.copy"),
            ("ctrl-x", "editor.cut"),
            ("ctrl-v", "editor.paste"),
            ("ctrl-f", "editor.find"),
            ("ctrl-z", "editor.undo"),
            ("ctrl-y", "editor.redo"),
            ("ctrl-p", "workspace.open_file"),
            ("ctrl-shift-p", "command_palette.open"),
            ("ctrl-b", "view.toggle_sidebar"),
            ("ctrl-j", "view.toggle_bottom_panel"),
            ("ctrl-`", "terminal.toggle"),
            ("alt-1", "view.explorer"),
            ("alt-2", "view.source_control"),
            ("alt-3", "view.search"),
            ("ctrl-q", "app.quit"),
        ];
        let bindings = defaults
            .into_iter()
            .filter_map(|(key, command)| key.parse().ok().map(|chord| (chord, command.to_owned())))
            .collect();
        Self { bindings }
    }
}

impl Keymap {
    pub fn from_overrides(overrides: &HashMap<String, String>) -> (Self, Vec<String>) {
        let mut keymap = Self::default();
        let mut warnings = Vec::new();
        for (chord, command) in overrides {
            match chord.parse() {
                Ok(chord) => {
                    keymap.bindings.insert(chord, command.clone());
                }
                Err(error) => warnings.push(error.to_string()),
            }
        }
        (keymap, warnings)
    }

    pub fn resolve(&self, event: KeyEvent) -> Option<&str> {
        let chord = KeyChord {
            code: event.code,
            modifiers: event.modifiers,
        };
        self.bindings.get(&chord).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_default_binding() {
        let keymap = Keymap::default();
        let event = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(keymap.resolve(event), Some("editor.save"));
    }
}
