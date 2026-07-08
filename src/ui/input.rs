use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::{
    app::{AppState, BottomPanelView, Focus, Overlay},
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
            Some(
                Overlay::ConfirmDelete { .. }
                    | Overlay::ConfirmClose { .. }
                    | Overlay::ConfirmSaveAs { .. }
                    | Overlay::ConfirmGitRestore { .. }
                    | Overlay::ConfirmGitHunkRestore { .. }
                    | Overlay::ConfirmQuitTerminal
            )
        ) {
            return match event.code {
                KeyCode::Esc => Some(Command::Cancel),
                KeyCode::Enter => Some(Command::PaletteAccept),
                _ => None,
            };
        }
        if matches!(
            state.overlay,
            Some(Overlay::LspHover | Overlay::LspSignature)
        ) {
            return match event.code {
                KeyCode::Enter => Some(Command::PaletteAccept),
                KeyCode::Esc => Some(Command::Cancel),
                _ => None,
            };
        }
        if matches!(state.overlay, Some(Overlay::LspCompletion)) {
            return match event.code {
                KeyCode::Up => Some(Command::MoveUp { extend: false }),
                KeyCode::Down => Some(Command::MoveDown { extend: false }),
                KeyCode::Enter => Some(Command::PaletteAccept),
                KeyCode::Backspace => Some(Command::PaletteBackspace),
                KeyCode::Char(character)
                    if !event
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    Some(Command::PaletteInput(character))
                }
                KeyCode::Esc => Some(Command::Cancel),
                _ => None,
            };
        }
        if matches!(state.overlay, Some(Overlay::LspLocations)) {
            return match event.code {
                KeyCode::Up => Some(Command::LspLocationSelect(
                    state.lsp_location_selected.saturating_sub(1),
                )),
                KeyCode::Down => Some(Command::LspLocationSelect(
                    (state.lsp_location_selected + 1)
                        .min(state.lsp_locations.len().saturating_sub(1)),
                )),
                KeyCode::Enter => Some(Command::LspLocationOpen),
                KeyCode::Esc => Some(Command::Cancel),
                _ => None,
            };
        }
        if matches!(state.overlay, Some(Overlay::LspCodeActions)) {
            return match event.code {
                KeyCode::Up => Some(Command::LspCodeActionSelect(
                    state.lsp_code_action_selected.saturating_sub(1),
                )),
                KeyCode::Down => Some(Command::LspCodeActionSelect(
                    (state.lsp_code_action_selected + 1)
                        .min(state.lsp_code_actions.len().saturating_sub(1)),
                )),
                KeyCode::Enter => Some(Command::LspCodeActionApply),
                KeyCode::Esc => Some(Command::Cancel),
                _ => None,
            };
        }
        if matches!(state.overlay, Some(Overlay::BufferSearch { .. })) {
            return match event.code {
                KeyCode::Esc => Some(Command::Cancel),
                KeyCode::Tab | KeyCode::BackTab => Some(Command::SearchToggleReplaceField),
                KeyCode::Enter if event.modifiers.contains(KeyModifiers::CONTROL) => {
                    Some(Command::ReplaceAll)
                }
                KeyCode::Enter if state.buffer_replace_focused => Some(Command::ReplaceNext),
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
        if matches!(state.overlay, Some(Overlay::GitCommitInput))
            && event.code == KeyCode::Enter
            && event.modifiers.contains(KeyModifiers::SHIFT)
        {
            return Some(Command::PaletteNewline);
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
        if state.sidebar_view == crate::app::SidebarView::SourceControl {
            return match event.code {
                KeyCode::Up => Some(Command::SelectGit(state.git_selected.saturating_sub(1))),
                KeyCode::Down => Some(Command::SelectGit(
                    (state.git_selected + 1).min(state.git_entries().len().saturating_sub(1)),
                )),
                KeyCode::Enter => Some(Command::OpenGitDiff),
                KeyCode::Char('r' | 'R') => Some(Command::Invoke("git.refresh".to_owned())),
                KeyCode::Char('s' | 'S') => Some(Command::Invoke("git.stage".to_owned())),
                KeyCode::Char('u' | 'U') => Some(Command::Invoke("git.unstage".to_owned())),
                KeyCode::Char('d' | 'D') => Some(Command::Invoke("git.restore".to_owned())),
                KeyCode::Char('c' | 'C') => Some(Command::Invoke("git.commit".to_owned())),
                KeyCode::Char('b' | 'B') => Some(Command::Invoke("git.branch_switch".to_owned())),
                KeyCode::Char('n' | 'N') => Some(Command::Invoke("git.branch_create".to_owned())),
                KeyCode::Char('f' | 'F') => Some(Command::Invoke("git.fetch".to_owned())),
                KeyCode::Char('p') => Some(Command::Invoke("git.pull".to_owned())),
                KeyCode::Char('P') => Some(Command::Invoke("git.push".to_owned())),
                KeyCode::Esc => Some(Command::Invoke("view.toggle_sidebar".to_owned())),
                _ => None,
            };
        }
        if state.sidebar_view == crate::app::SidebarView::Search {
            return match event.code {
                KeyCode::Up => Some(Command::WorkspaceSearchSelect(
                    state.workspace_search_selected.saturating_sub(1),
                )),
                KeyCode::Down => Some(Command::WorkspaceSearchSelect(
                    (state.workspace_search_selected + 1)
                        .min(state.workspace_matches.len().saturating_sub(1)),
                )),
                KeyCode::Enter => Some(Command::WorkspaceSearchOpen),
                KeyCode::Left | KeyCode::Right => state
                    .workspace_matches
                    .get(state.workspace_search_selected)
                    .map(|matched| Command::WorkspaceSearchToggleFile(matched.path.clone())),
                KeyCode::Backspace => Some(Command::WorkspaceSearchBackspace),
                KeyCode::Char('c') if event.modifiers.contains(KeyModifiers::ALT) => {
                    Some(Command::WorkspaceSearchToggleCase)
                }
                KeyCode::Char('w') if event.modifiers.contains(KeyModifiers::ALT) => {
                    Some(Command::WorkspaceSearchToggleWord)
                }
                KeyCode::Char('r') if event.modifiers.contains(KeyModifiers::ALT) => {
                    Some(Command::WorkspaceSearchToggleRegex)
                }
                KeyCode::Char('h') if event.modifiers.contains(KeyModifiers::ALT) => {
                    Some(Command::WorkspaceSearchToggleHidden)
                }
                KeyCode::Char('b') if event.modifiers.contains(KeyModifiers::ALT) => {
                    Some(Command::WorkspaceSearchToggleBinary)
                }
                KeyCode::Char(character)
                    if !event
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    Some(Command::WorkspaceSearchInput(character))
                }
                KeyCode::Esc => Some(Command::Invoke("view.toggle_sidebar".to_owned())),
                _ => None,
            };
        }
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
    if state.focus == Focus::BottomPanel {
        if state.bottom_panel_view == BottomPanelView::Problems {
            return match event.code {
                KeyCode::Up => Some(Command::DiagnosticSelect(
                    state.diagnostic_selected.saturating_sub(1),
                )),
                KeyCode::Down => Some(Command::DiagnosticSelect(
                    (state.diagnostic_selected + 1)
                        .min(state.visible_diagnostics().len().saturating_sub(1)),
                )),
                KeyCode::Enter => Some(Command::DiagnosticOpen),
                KeyCode::Char('f' | 'F') => Some(Command::DiagnosticCycleFilter),
                KeyCode::Char('s' | 'S') => Some(Command::DiagnosticCycleSource),
                KeyCode::Char('c' | 'C') => Some(Command::DiagnosticToggleCurrentFile),
                KeyCode::Esc => Some(Command::Invoke("view.toggle_bottom_panel".to_owned())),
                _ => None,
            };
        }
        if state.bottom_panel_view == BottomPanelView::Terminal {
            if event.code == KeyCode::Char('c')
                && event
                    .modifiers
                    .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT)
            {
                return Some(Command::TerminalCopy);
            }
            if event.code == KeyCode::Char('v')
                && event
                    .modifiers
                    .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT)
            {
                return Some(Command::TerminalPaste(state.internal_clipboard.clone()));
            }
            if event.code == KeyCode::Char('`')
                && event
                    .modifiers
                    .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT)
            {
                return Some(Command::TerminalInput(vec![0]));
            }
            if event.code == KeyCode::PageUp && event.modifiers.contains(KeyModifiers::SHIFT) {
                return Some(Command::TerminalScroll(-10));
            }
            if event.code == KeyCode::PageDown && event.modifiers.contains(KeyModifiers::SHIFT) {
                return Some(Command::TerminalScroll(10));
            }
            let application_cursor = state.terminal.snapshot(0).application_cursor;
            return crate::terminal::encode_key(event, application_cursor)
                .map(Command::TerminalInput);
        }
        return match event.code {
            KeyCode::Up | KeyCode::Char('k') => Some(Command::GitHunkPrevious),
            KeyCode::Down | KeyCode::Char('j') => Some(Command::GitHunkNext),
            KeyCode::Char('s' | 'S') | KeyCode::Enter => Some(Command::GitHunkStageToggle),
            KeyCode::Char('d' | 'D') | KeyCode::Delete => Some(Command::GitHunkRestore),
            KeyCode::Char('o' | 'O') => Some(Command::GitHunkOpenFile),
            KeyCode::Esc => Some(Command::Invoke("view.toggle_bottom_panel".to_owned())),
            _ => None,
        };
    }
    if state.git_diff_active {
        return match event.code {
            KeyCode::Up | KeyCode::Char('k') => Some(Command::GitHunkPrevious),
            KeyCode::Down | KeyCode::Char('j') => Some(Command::GitHunkNext),
            KeyCode::Char('s' | 'S') => Some(Command::GitHunkStageToggle),
            KeyCode::Char('d' | 'D') | KeyCode::Delete => Some(Command::GitHunkRestore),
            KeyCode::Enter | KeyCode::Char('o' | 'O') => Some(Command::GitHunkOpenFile),
            _ => None,
        };
    }
    if event.code == KeyCode::PageUp {
        return Some(Command::EditorScroll(-10));
    }
    if event.code == KeyCode::PageDown {
        return Some(Command::EditorScroll(10));
    }
    let extend = event.modifiers.contains(KeyModifiers::SHIFT);
    match event.code {
        KeyCode::Left if event.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(Command::MoveWordLeft { extend })
        }
        KeyCode::Right if event.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(Command::MoveWordRight { extend })
        }
        KeyCode::Left => Some(Command::MoveLeft { extend }),
        KeyCode::Right => Some(Command::MoveRight { extend }),
        KeyCode::Up => Some(Command::MoveUp { extend }),
        KeyCode::Down => Some(Command::MoveDown { extend }),
        KeyCode::Backspace if event.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(Command::DeleteWordBackward)
        }
        KeyCode::Delete if event.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(Command::DeleteWordForward)
        }
        KeyCode::Backspace => Some(Command::DeleteBackward),
        KeyCode::Enter => Some(Command::InsertNewline),
        KeyCode::BackTab => Some(Command::OutdentSelection),
        KeyCode::Tab
            if state
                .active_tab()
                .is_some_and(|tab| !tab.buffer.selection().is_caret()) =>
        {
            Some(Command::IndentSelection)
        }
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
    if event.kind == MouseEventKind::Down(MouseButton::Left)
        && contains(regions.bottom, (event.column, event.row))
        && event.row == regions.bottom.y
    {
        let column = event.column.saturating_sub(regions.bottom.x);
        let id = if column < 11 {
            "diagnostics.open_problems"
        } else if column < 20 {
            "view.output"
        } else {
            "view.terminal"
        };
        return Some(Command::Invoke(id.to_owned()));
    }
    if contains(regions.bottom, (event.column, event.row))
        && state.bottom_panel_view == BottomPanelView::Terminal
    {
        if event.kind == MouseEventKind::Down(MouseButton::Left)
            && event.modifiers.contains(KeyModifiers::CONTROL)
        {
            return Some(Command::TerminalOpenReference {
                row: usize::from(event.row.saturating_sub(regions.bottom.y + 1)),
                column: Some(usize::from(event.column.saturating_sub(regions.bottom.x))),
            });
        }
        return match event.kind {
            MouseEventKind::ScrollUp => Some(Command::TerminalScroll(-3)),
            MouseEventKind::ScrollDown => Some(Command::TerminalScroll(3)),
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left) => {
                let row = usize::from(event.row.saturating_sub(regions.bottom.y + 1));
                let column = usize::from(event.column.saturating_sub(regions.bottom.x));
                Some(Command::TerminalSetSelection {
                    row,
                    column,
                    extend: matches!(event.kind, MouseEventKind::Drag(MouseButton::Left)),
                })
            }
            _ => None,
        };
    }
    let point = (event.column, event.row);
    if contains(regions.editor, point)
        && matches!(
            event.kind,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        )
    {
        let right = state.split_tab.is_some()
            && event.column >= regions.editor.x + regions.editor.width / 2;
        if state.split_tab.is_some() && right != state.split_focus_right {
            return Some(Command::FocusEditorGroup(right));
        }
        return Some(Command::EditorScroll(
            if event.kind == MouseEventKind::ScrollUp {
                -3
            } else {
                3
            },
        ));
    }
    if event.kind == MouseEventKind::Down(MouseButton::Left)
        && contains(regions.bottom, (event.column, event.row))
        && state.bottom_panel_view == BottomPanelView::Problems
    {
        let row = usize::from(event.row.saturating_sub(regions.bottom.y + 1));
        return match state.diagnostic_rows().get(row) {
            Some(crate::app::DiagnosticRow::Item(index)) => Some(Command::DiagnosticSelect(*index)),
            _ => None,
        };
    }
    let left_down = event.kind == MouseEventKind::Down(MouseButton::Left);
    let left_drag = event.kind == MouseEventKind::Drag(MouseButton::Left);
    if !left_down && !left_drag {
        return None;
    }
    if state.split_tab.is_some() && contains(regions.editor, point) {
        let right = event.column >= regions.editor.x + regions.editor.width / 2;
        if right != state.split_focus_right {
            return Some(Command::FocusEditorGroup(right));
        }
    }
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
        if state.sidebar_view == crate::app::SidebarView::SourceControl {
            return state.git_index_at_row(index).map(Command::SelectGitAndOpen);
        }
        if state.sidebar_view == crate::app::SidebarView::Search {
            if index == 1 {
                let column = usize::from(event.column.saturating_sub(regions.sidebar.x));
                return match column {
                    0..=7 => Some(Command::WorkspaceSearchToggleCase),
                    8..=15 => Some(Command::WorkspaceSearchToggleWord),
                    16..=24 => Some(Command::WorkspaceSearchToggleRegex),
                    25..=34 => Some(Command::WorkspaceSearchToggleHidden),
                    _ => Some(Command::WorkspaceSearchToggleBinary),
                };
            }
            let row = index.saturating_sub(3);
            return match state.workspace_search_rows().get(row) {
                Some(crate::app::WorkspaceSearchRow::File(path)) => {
                    Some(Command::WorkspaceSearchToggleFile(path.clone()))
                }
                Some(crate::app::WorkspaceSearchRow::Match(index)) => {
                    Some(Command::WorkspaceSearchSelectAndOpen(*index))
                }
                None => None,
            };
        }
        return state.tree.visible_entry(index).map(|entry| {
            if entry.kind == TreeEntryKind::Directory {
                Command::ToggleTree(index)
            } else {
                Command::OpenFile(state.requested_file(&entry.relative_path))
            }
        });
    }
    if left_down && contains(regions.tabs, point) {
        let group_x = if state.split_tab.is_some() && state.split_focus_right {
            regions.tabs.x + regions.tabs.width / 2
        } else {
            regions.tabs.x
        };
        let target = usize::from(event.column.saturating_sub(group_x));
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
        if let Some(diff) = &state.git_diff {
            let name = diff.path.file_name().map_or_else(
                || diff.path.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            let label = format!(" {name} (Diff) × ");
            let width =
                UnicodeWidthStr::width(label.as_str()) + usize::from(!state.tabs.is_empty());
            if target < start + width {
                return Some(if target + 3 >= start + width {
                    Command::CloseTab(state.tabs.len())
                } else {
                    Command::SelectTab(state.tabs.len())
                });
            }
        }
    }
    if left_down && state.git_diff_active && contains(regions.editor, point) {
        let group_x = if state.split_tab.is_some() && state.split_focus_right {
            regions.editor.x + regions.editor.width / 2
        } else {
            regions.editor.x
        };
        if event.row == regions.editor.y + 2 {
            let column = event.column.saturating_sub(group_x);
            if column < 8 {
                return Some(Command::GitHunkPrevious);
            }
            if (10..18).contains(&column) {
                return Some(Command::GitHunkNext);
            }
        }
    }
    if state.git_diff_active && contains(regions.editor, point) && event.row > regions.editor.y + 2
    {
        let raw_line =
            state.git_diff_scroll + usize::from(event.row.saturating_sub(regions.editor.y + 3));
        return state
            .git_hunk_at_diff_line(raw_line)
            .map(Command::SelectGitHunk);
    }
    if contains(regions.editor, point) && event.row > regions.editor.y + 1 {
        let tab = state.active_tab()?;
        let gutter_width = tab.buffer.text().len_lines().to_string().len().max(2) + 3;
        let group_x = if state.split_tab.is_some() && state.split_focus_right {
            regions.editor.x + regions.editor.width / 2 + 1
        } else {
            regions.editor.x
        };
        let display_column =
            usize::from(event.column.saturating_sub(group_x)).saturating_sub(gutter_width);
        let visual_offset = usize::from(event.row.saturating_sub(regions.editor.y + 2));
        let (line_index, segment_start) = if state.settings.editor.word_wrap {
            let group_width = if state.split_tab.is_some() {
                regions.editor.width.saturating_sub(1) / 2
            } else {
                regions.editor.width
            };
            let wrap_width = usize::from(group_width).saturating_sub(gutter_width).max(1);
            let row = crate::editor::visible_visual_rows(
                tab.buffer.text(),
                tab.view.scroll_line + visual_offset,
                1,
                wrap_width,
                usize::from(state.settings.editor.tab_width),
                state.settings.editor.ambiguous_width_wide,
            )
            .into_iter()
            .next()?;
            (row.line_index, row.range_in_line.start)
        } else {
            (tab.view.scroll_line + visual_offset, 0)
        };
        if line_index >= tab.buffer.text().len_lines() {
            return None;
        }
        let line = tab.buffer.text().line(line_index).to_string();
        let content = line.trim_end_matches(['\r', '\n']);
        let segment = content.chars().skip(segment_start).collect::<String>();
        let char_in_line = char_offset_at_display_column(
            &segment,
            display_column,
            usize::from(state.settings.editor.tab_width),
            state.settings.editor.ambiguous_width_wide,
        );
        return Some(Command::SetCursor {
            char_offset: tab.buffer.text().line_to_char(line_index) + segment_start + char_in_line,
            extend: left_drag,
        });
    }
    None
}

fn char_offset_at_display_column(
    text: &str,
    target: usize,
    tab_width: usize,
    ambiguous_width_wide: bool,
) -> usize {
    let mut display = 0usize;
    let mut chars = 0usize;
    for grapheme in text.graphemes(true) {
        let width = if grapheme == "\t" {
            tab_width - (display % tab_width)
        } else {
            crate::editor::grapheme_width(grapheme, ambiguous_width_wide)
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
