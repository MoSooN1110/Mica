use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::{
    app::{AppState, Focus, Overlay},
    command::Command,
    workspace::TreeEntryKind,
};

use super::Regions;

pub fn command_for_key(state: &AppState, event: KeyEvent) -> Option<Command> {
    if !matches!(event.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return None;
    }
    if state.overlay.is_some() {
        if matches!(state.overlay, Some(Overlay::RecoveryPrompt)) {
            return match event.code {
                KeyCode::Char('r' | 'R') => Some(Command::RecoveryRecover),
                KeyCode::Char('d' | 'D') => Some(Command::RecoveryDiscard),
                KeyCode::Char('l' | 'L') | KeyCode::Esc => Some(Command::RecoveryLater),
                _ => None,
            };
        }
        if matches!(
            state.overlay,
            Some(Overlay::ConfirmDelete { .. } | Overlay::ConfirmClose { .. })
        ) {
            return match event.code {
                KeyCode::Esc => Some(Command::Cancel),
                KeyCode::Enter => Some(Command::PaletteAccept),
                _ => None,
            };
        }
        if matches!(state.overlay, Some(Overlay::BufferSearch { .. })) {
            return match event.code {
                KeyCode::Esc => Some(Command::Cancel),
                KeyCode::Enter if event.modifiers.contains(KeyModifiers::SHIFT) => {
                    Some(Command::SearchPrevious)
                }
                KeyCode::Enter => Some(Command::SearchNext),
                KeyCode::Backspace => Some(Command::PaletteBackspace),
                KeyCode::Char(character) if !event.modifiers.contains(KeyModifiers::CONTROL) => {
                    Some(Command::PaletteInput(character))
                }
                _ => None,
            };
        }
        return match event.code {
            KeyCode::Esc => Some(Command::Cancel),
            KeyCode::Enter => Some(Command::PaletteAccept),
            KeyCode::Backspace => Some(Command::PaletteBackspace),
            KeyCode::Up => Some(Command::MoveUp { extend: false }),
            KeyCode::Down => Some(Command::MoveDown { extend: false }),
            KeyCode::Char(character) if !event.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Command::PaletteInput(character))
            }
            _ => None,
        };
    }
    if let Some(id) = state.keymap.resolve(event) {
        return Some(Command::Invoke(id.to_owned()));
    }
    if state.focus == Focus::Sidebar {
        return match event.code {
            KeyCode::Up => Some(Command::SelectTree(state.tree_selected.saturating_sub(1))),
            KeyCode::Down => Some(Command::SelectTree(
                (state.tree_selected + 1).min(state.tree.visible_len().saturating_sub(1)),
            )),
            KeyCode::Enter => state.tree.visible_entry(state.tree_selected).map(|entry| {
                if entry.kind == TreeEntryKind::Directory {
                    Command::ToggleTree(state.tree_selected)
                } else {
                    Command::OpenFile(state.requested_file(&entry.relative_path))
                }
            }),
            KeyCode::F(2) => Some(Command::Invoke("file.rename".to_owned())),
            KeyCode::Delete => Some(Command::Invoke("file.delete".to_owned())),
            KeyCode::Esc => Some(Command::Invoke("view.toggle_sidebar".to_owned())),
            _ => None,
        };
    }
    let extend = event.modifiers.contains(KeyModifiers::SHIFT);
    match event.code {
        KeyCode::Left => Some(Command::MoveLeft { extend }),
        KeyCode::Right => Some(Command::MoveRight { extend }),
        KeyCode::Up => Some(Command::MoveUp { extend }),
        KeyCode::Down => Some(Command::MoveDown { extend }),
        KeyCode::Backspace => Some(Command::DeleteBackward),
        KeyCode::Enter => Some(Command::InsertNewline),
        KeyCode::Tab => Some(Command::InsertText(
            if state.settings.editor.insert_spaces {
                " ".repeat(usize::from(state.settings.editor.tab_width))
            } else {
                "\t".to_owned()
            },
        )),
        KeyCode::Char(character)
            if !event
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            Some(Command::InsertText(character.to_string()))
        }
        _ => None,
    }
}

pub fn command_for_mouse(state: &AppState, regions: Regions, event: MouseEvent) -> Option<Command> {
    let left_down = event.kind == MouseEventKind::Down(MouseButton::Left);
    let left_drag = event.kind == MouseEventKind::Drag(MouseButton::Left);
    if !left_down && !left_drag {
        return None;
    }
    let point = (event.column, event.row);
    if left_down && contains(regions.activity, point) {
        let row = event.row.saturating_sub(regions.activity.y);
        let id = match row {
            0 => "view.explorer",
            1 => "view.source_control",
            2 => "view.search",
            _ => return None,
        };
        return Some(Command::Invoke(id.to_owned()));
    }
    if left_down && contains(regions.sidebar, point) {
        let index = usize::from(event.row.saturating_sub(regions.sidebar.y));
        return state.tree.visible_entry(index).map(|entry| {
            if entry.kind == TreeEntryKind::Directory {
                Command::ToggleTree(index)
            } else {
                Command::OpenFile(state.requested_file(&entry.relative_path))
            }
        });
    }
    if left_down && contains(regions.tabs, point) {
        let target = usize::from(event.column.saturating_sub(regions.tabs.x));
        let mut start = 0usize;
        for (index, tab) in state.tabs.iter().enumerate() {
            let dirty = if tab.buffer.is_dirty() { " ●" } else { "" };
            let label = format!(" {}{dirty} × ", tab.title());
            let width = UnicodeWidthStr::width(label.as_str()) + usize::from(index > 0);
            if target < start + width {
                return Some(if target + 3 >= start + width {
                    Command::CloseTab(index)
                } else {
                    Command::SelectTab(index)
                });
            }
            start += width;
        }
    }
    if contains(regions.editor, point) && event.row > regions.editor.y {
        let tab = state.active_tab()?;
        let line_index =
            tab.view.scroll_line + usize::from(event.row.saturating_sub(regions.editor.y + 1));
        if line_index >= tab.buffer.text().len_lines() {
            return None;
        }
        let gutter_width = tab.buffer.text().len_lines().to_string().len().max(2) + 1;
        let display_column =
            usize::from(event.column.saturating_sub(regions.editor.x)).saturating_sub(gutter_width);
        let line = tab.buffer.text().line(line_index).to_string();
        let content = line.trim_end_matches(['\r', '\n']);
        let char_in_line = char_offset_at_display_column(
            content,
            display_column,
            usize::from(state.settings.editor.tab_width),
        );
        return Some(Command::SetCursor {
            char_offset: tab.buffer.text().line_to_char(line_index) + char_in_line,
            extend: left_drag,
        });
    }
    None
}

fn char_offset_at_display_column(text: &str, target: usize, tab_width: usize) -> usize {
    let mut display = 0usize;
    let mut chars = 0usize;
    for grapheme in text.graphemes(true) {
        let width = if grapheme == "\t" {
            tab_width - (display % tab_width)
        } else {
            UnicodeWidthStr::width(grapheme)
        };
        if target < display + width {
            if target.saturating_sub(display) * 2 >= width {
                chars += grapheme.chars().count();
            }
            break;
        }
        display += width;
        chars += grapheme.chars().count();
    }
    chars
}

fn contains(rect: ratatui::layout::Rect, point: (u16, u16)) -> bool {
    point.0 >= rect.x
        && point.0 < rect.x + rect.width
        && point.1 >= rect.y
        && point.1 < rect.y + rect.height
}
