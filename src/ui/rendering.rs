use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Margin, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Tabs},
};

use crate::{
    app::{AppState, Focus, Overlay, PathAction, SidebarView},
    buffer::LineEnding,
    workspace::TreeEntryKind,
};

use super::Theme;

#[derive(Debug, Clone, Copy, Default)]
pub struct Regions {
    pub activity: Rect,
    pub sidebar: Rect,
    pub editor: Rect,
    pub tabs: Rect,
    pub bottom: Rect,
    pub status: Rect,
}

pub fn render(frame: &mut Frame, state: &AppState, theme: &Theme) -> Regions {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.background)),
        area,
    );
    if area.width < 40 || area.height < 10 {
        frame.render_widget(
            Paragraph::new("Mica needs at least 40×10").style(
                Style::default()
                    .fg(theme.diagnostic_warning)
                    .bg(theme.background),
            ),
            area,
        );
        return Regions::default();
    }
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);
    let body = vertical[0];
    let status = vertical[1];
    let panel_height = if state.bottom_panel_visible {
        state.settings.ui.bottom_panel_height.min(body.height / 2)
    } else {
        0
    };
    let body_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(panel_height)])
        .split(body);
    let sidebar_width = if state.sidebar_visible {
        state
            .settings
            .ui
            .sidebar_width
            .min(area.width.saturating_sub(24))
    } else {
        0
    };
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(sidebar_width),
            Constraint::Min(20),
        ])
        .split(body_rows[0]);
    let regions = Regions {
        activity: columns[0],
        sidebar: columns[1],
        editor: columns[2],
        tabs: Rect::new(columns[2].x, columns[2].y, columns[2].width, 1),
        bottom: body_rows[1],
        status,
    };
    render_activity(frame, regions.activity, state, theme);
    if state.sidebar_visible {
        render_sidebar(frame, regions.sidebar, state, theme);
    }
    render_editor(frame, regions.editor, state, theme);
    if state.bottom_panel_visible {
        render_bottom(frame, regions.bottom, theme);
    }
    render_status(frame, regions.status, state, theme);
    if let Some(overlay) = &state.overlay {
        match overlay {
            Overlay::CommandPalette => render_palette(frame, area, state, theme),
            Overlay::FilePicker => render_file_picker(frame, area, state, theme),
            Overlay::BufferSearch { tab } => {
                render_buffer_search(frame, area, state, *tab, theme);
            }
            Overlay::PathInput(action) => render_path_input(frame, area, state, action, theme),
            Overlay::ConfirmDelete {
                plan,
                dirty_buffers,
            } => render_delete_confirmation(frame, area, plan, *dirty_buffers, theme),
            Overlay::ConfirmClose { tab } => {
                render_close_confirmation(frame, area, state, *tab, theme);
            }
            Overlay::RecoveryPrompt => render_recovery_prompt(frame, area, state, theme),
        }
    }
    regions
}

fn render_activity(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let entries = [
        ("E", SidebarView::Explorer),
        ("G", SidebarView::SourceControl),
        ("S", SidebarView::Search),
    ];
    let lines = entries
        .into_iter()
        .map(|(label, view)| {
            let selected = state.sidebar_view == view;
            Line::from(Span::styled(
                format!(" {label} "),
                Style::default()
                    .fg(if selected {
                        theme.accent
                    } else {
                        theme.text_muted
                    })
                    .bg(theme.surface)
                    .add_modifier(if selected {
                        Modifier::BOLD | Modifier::UNDERLINED
                    } else {
                        Modifier::empty()
                    }),
            ))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.surface)),
        area,
    );
}

fn render_sidebar(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let title = match state.sidebar_view {
        SidebarView::Explorer => " EXPLORER ",
        SidebarView::SourceControl => " SOURCE CONTROL ",
        SidebarView::Search => " SEARCH ",
    };
    let focused = state.focus == Focus::Sidebar;
    let block = Block::default()
        .title(title)
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(if focused { theme.accent } else { theme.border }))
        .style(Style::default().bg(theme.surface).fg(theme.text));
    if state.sidebar_view != SidebarView::Explorer {
        frame.render_widget(
            Paragraph::new("Coming in the next milestone")
                .block(block)
                .style(Style::default().fg(theme.text_muted).bg(theme.surface)),
            area,
        );
        return;
    }
    let max = usize::from(area.height.saturating_sub(1));
    let items = state
        .tree
        .visible_entries()
        .take(max)
        .enumerate()
        .map(|(index, entry)| {
            let icon = match entry.kind {
                TreeEntryKind::Directory if state.tree.is_expanded(&entry.relative_path) => "▾",
                TreeEntryKind::Directory => "▸",
                TreeEntryKind::File => "·",
                TreeEntryKind::Symlink => "↗",
            };
            let name = entry.relative_path.file_name().map_or_else(
                || entry.relative_path.to_string_lossy(),
                |name| name.to_string_lossy(),
            );
            let prefix = "  ".repeat(entry.depth);
            let style = if index == state.tree_selected {
                Style::default()
                    .fg(theme.text)
                    .bg(theme.selection)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text_muted).bg(theme.surface)
            };
            ListItem::new(format!("{prefix}{icon} {name}")).style(style)
        })
        .collect::<Vec<_>>();
    frame.render_widget(List::new(items).block(block), area);
}

fn render_editor(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(area);
    let titles = state
        .tabs
        .iter()
        .map(|tab| {
            let dirty = if tab.buffer.is_dirty() { " ●" } else { "" };
            Line::from(format!(" {}{dirty} × ", tab.title()))
        })
        .collect::<Vec<_>>();
    let tabs = Tabs::new(if titles.is_empty() {
        vec![Line::from(" Welcome ")]
    } else {
        titles
    })
    .select(state.active_tab.unwrap_or(0))
    .style(
        Style::default()
            .fg(theme.text_muted)
            .bg(theme.surface_raised),
    )
    .highlight_style(
        Style::default()
            .fg(theme.accent)
            .bg(theme.background)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
    );
    frame.render_widget(tabs, rows[0]);
    let Some(tab) = state.active_tab() else {
        frame.render_widget(Paragraph::new("\n  MICA\n  Open a file from Explorer\n\n  Ctrl+Shift+P  Command Palette\n  Ctrl+Q        Quit")
            .style(Style::default().fg(theme.text_muted).bg(theme.background)), rows[1]);
        return;
    };
    let selection = tab.buffer.selection();
    let cursor_line = tab
        .buffer
        .text()
        .char_to_line(selection.head.0.min(tab.buffer.text().len_chars()));
    let line_start = tab.buffer.text().line_to_char(cursor_line);
    let gutter_width = tab.buffer.text().len_lines().to_string().len().max(2);
    let visible_height = usize::from(rows[1].height);
    let lines = tab
        .buffer
        .text()
        .lines()
        .skip(tab.view.scroll_line)
        .take(visible_height)
        .enumerate()
        .map(|(visible, line)| {
            let line_index = tab.view.scroll_line + visible;
            let content = line.to_string();
            let content = content.trim_end_matches(['\r', '\n']);
            let number = format!("{:>width$} ", line_index + 1, width = gutter_width);
            let active = line_index == cursor_line;
            let mut spans = vec![Span::styled(
                number,
                Style::default()
                    .fg(if active {
                        theme.accent
                    } else {
                        theme.text_faint
                    })
                    .bg(if active {
                        theme.active_line
                    } else {
                        theme.background
                    }),
            )];
            spans.extend(editor_line_spans(
                content,
                tab.buffer.text().line_to_char(line_index),
                selection,
                &tab.view.search,
                &tab.highlights,
                active,
                theme,
            ));
            Line::from(spans)
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.background)),
        rows[1],
    );
    if state.focus == Focus::Editor && cursor_line >= tab.view.scroll_line {
        let line = tab.buffer.text().line(cursor_line).to_string();
        let char_in_line = selection.head.0.saturating_sub(line_start);
        let x = rows[1].x
            + u16::try_from(
                gutter_width
                    + 1
                    + crate::editor::display_column(
                        &line,
                        char_in_line,
                        usize::from(state.settings.editor.tab_width),
                    ),
            )
            .unwrap_or(u16::MAX);
        let y = rows[1].y + u16::try_from(cursor_line - tab.view.scroll_line).unwrap_or(u16::MAX);
        if x < rows[1].right() && y < rows[1].bottom() {
            frame.set_cursor_position(Position::new(x, y));
        }
    }
}

fn editor_line_spans(
    content: &str,
    line_start_char: usize,
    selection: crate::buffer::Selection,
    search: &crate::editor::BufferSearchState,
    highlights: &[crate::editor::HighlightSpan],
    active: bool,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let base_background = if active {
        theme.active_line
    } else {
        theme.background
    };
    let base = Style::default().fg(theme.text).bg(base_background);
    let line_chars = content.chars().count();
    let line_end_char = line_start_char + line_chars;
    let selection_range = selection.range();
    let mut boundaries = vec![0usize, line_chars];
    let mut add_boundaries = |range: &std::ops::Range<usize>| {
        let start = range.start.max(line_start_char).min(line_end_char);
        let end = range.end.max(line_start_char).min(line_end_char);
        if start < end {
            boundaries.push(start - line_start_char);
            boundaries.push(end - line_start_char);
        }
    };
    add_boundaries(&selection_range);
    for matched in &search.matches {
        add_boundaries(matched);
    }
    for highlight in highlights {
        add_boundaries(&(highlight.start_char..highlight.end_char));
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    let mut spans = Vec::with_capacity(boundaries.len().saturating_sub(1));
    for pair in boundaries.windows(2) {
        let local_start = pair[0];
        let local_end = pair[1];
        if local_start == local_end {
            continue;
        }
        let global_start = line_start_char + local_start;
        let selected = selection_range.contains(&global_start);
        let matched = search
            .matches
            .iter()
            .any(|range| range.contains(&global_start));
        let syntax = highlights
            .iter()
            .filter(|span| span.start_char <= global_start && global_start < span.end_char)
            .min_by_key(|span| span.end_char.saturating_sub(span.start_char));
        let style = if selected {
            Style::default().fg(theme.text).bg(theme.selection)
        } else if matched {
            Style::default().fg(theme.accent).bg(theme.surface_raised)
        } else if let Some(syntax) = syntax {
            Style::default()
                .fg(syntax_color(syntax.kind, theme))
                .bg(base_background)
        } else {
            base
        };
        let start_byte = byte_index_at_char(content, local_start);
        let end_byte = byte_index_at_char(content, local_end);
        spans.push(Span::styled(
            content[start_byte..end_byte].to_owned(),
            style,
        ));
    }
    spans
}

fn syntax_color(kind: crate::editor::HighlightKind, theme: &Theme) -> ratatui::style::Color {
    match kind {
        crate::editor::HighlightKind::Keyword => theme.syntax_keyword,
        crate::editor::HighlightKind::Function => theme.syntax_function,
        crate::editor::HighlightKind::Type => theme.syntax_type,
        crate::editor::HighlightKind::String => theme.syntax_string,
        crate::editor::HighlightKind::Number => theme.syntax_number,
        crate::editor::HighlightKind::Comment => theme.syntax_comment,
        crate::editor::HighlightKind::Variable => theme.syntax_variable,
        crate::editor::HighlightKind::Constant => theme.syntax_constant,
    }
}

fn byte_index_at_char(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map_or(text.len(), |(byte, _)| byte)
}

fn render_bottom(frame: &mut Frame, area: Rect, theme: &Theme) {
    let block = Block::default()
        .title(" PROBLEMS  OUTPUT  TERMINAL ")
        .borders(Borders::TOP)
        .border_style(Style::default().fg(theme.border));
    frame.render_widget(
        Paragraph::new("No output")
            .block(block)
            .style(Style::default().fg(theme.text_muted).bg(theme.surface)),
        area,
    );
}

fn render_status(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let (file, position, encoding) = state.active_tab().map_or_else(
        || ("No file".to_owned(), String::new(), String::new()),
        |tab| {
            let selection = tab.buffer.selection();
            let line = tab
                .buffer
                .text()
                .char_to_line(selection.head.0.min(tab.buffer.text().len_chars()));
            let char_in_line = selection.head.0 - tab.buffer.text().line_to_char(line);
            let line_text = tab.buffer.text().line(line).to_string();
            let column = crate::editor::display_column(
                &line_text,
                char_in_line,
                usize::from(state.settings.editor.tab_width),
            );
            let ending = match tab.buffer.line_ending() {
                LineEnding::Lf => "LF",
                LineEnding::CrLf => "CRLF",
                LineEnding::Mixed => "Mixed",
            };
            (
                tab.title(),
                format!("Ln {}, Col {}", line + 1, column + 1),
                format!("UTF-8  {ending}"),
            )
        },
    );
    let message = state.notification.as_deref().unwrap_or("");
    let status = format!(" {file}   {message}   {encoding}   {position} ");
    frame.render_widget(
        Paragraph::new(status).style(Style::default().fg(theme.text).bg(theme.surface_raised)),
        area,
    );
}

fn render_palette(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let width = area.width.saturating_sub(8).min(72);
    let height = area.height.saturating_sub(4).min(14);
    let popup = Rect::new(area.x + (area.width - width) / 2, area.y + 2, width, height);
    frame.render_widget(Clear, popup);
    let inner = popup.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(inner);
    let block = Block::default()
        .title(" COMMAND PALETTE ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent))
        .style(Style::default().bg(theme.surface_raised));
    frame.render_widget(block, popup);
    frame.render_widget(
        Paragraph::new(format!("> {}", state.palette_query))
            .style(Style::default().fg(theme.text).bg(theme.selection)),
        rows[0],
    );
    let commands = state
        .palette_commands()
        .into_iter()
        .take(usize::from(rows[1].height))
        .enumerate()
        .map(|(index, command)| {
            ListItem::new(format!("{}  {}", command.title, command.id)).style(
                Style::default()
                    .fg(if index == 0 {
                        theme.text
                    } else {
                        theme.text_muted
                    })
                    .bg(if index == 0 {
                        theme.selection
                    } else {
                        theme.surface_raised
                    }),
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(List::new(commands), rows[1]);
}

fn render_path_input(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    action: &PathAction,
    theme: &Theme,
) {
    let title = match action {
        PathAction::CreateFile => " NEW FILE (workspace-relative) ",
        PathAction::CreateDirectory => " NEW DIRECTORY (workspace-relative) ",
        PathAction::Move { .. } => " RENAME / MOVE (workspace-relative) ",
    };
    let width = area.width.saturating_sub(8).min(72);
    let popup = Rect::new(area.x + (area.width - width) / 2, area.y + 3, width, 3);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent))
        .style(Style::default().bg(theme.surface_raised));
    frame.render_widget(
        Paragraph::new(format!("> {}", state.palette_query))
            .block(block)
            .style(Style::default().fg(theme.text).bg(theme.surface_raised)),
        popup,
    );
    let query_column = crate::editor::display_column(
        &state.palette_query,
        state.palette_query.chars().count(),
        usize::from(state.settings.editor.tab_width),
    );
    let cursor_x = popup
        .x
        .saturating_add(2)
        .saturating_add(u16::try_from(query_column).unwrap_or(u16::MAX));
    if cursor_x < popup.right().saturating_sub(1) {
        frame.set_cursor_position(Position::new(cursor_x, popup.y + 1));
    }
}

fn render_buffer_search(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    tab: usize,
    theme: &Theme,
) {
    let width = area.width.saturating_sub(4).min(48);
    let popup = Rect::new(area.right().saturating_sub(width + 2), area.y + 2, width, 3);
    frame.render_widget(Clear, popup);
    let (current, total) = state.tabs.get(tab).map_or((0, 0), |tab| {
        let total = tab.view.search.matches.len();
        (usize::from(total > 0) + tab.view.search.current, total)
    });
    let block = Block::default()
        .title(format!(" FIND  {current}/{total} "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent))
        .style(Style::default().bg(theme.surface_raised));
    frame.render_widget(
        Paragraph::new(format!("> {}", state.palette_query))
            .block(block)
            .style(Style::default().fg(theme.text).bg(theme.surface_raised)),
        popup,
    );
    let query_column = crate::editor::display_column(
        &state.palette_query,
        state.palette_query.chars().count(),
        usize::from(state.settings.editor.tab_width),
    );
    let cursor_x = popup
        .x
        .saturating_add(2)
        .saturating_add(u16::try_from(query_column).unwrap_or(u16::MAX));
    if cursor_x < popup.right().saturating_sub(1) {
        frame.set_cursor_position(Position::new(cursor_x, popup.y + 1));
    }
}

fn render_file_picker(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let width = area.width.saturating_sub(8).min(84);
    let height = area.height.saturating_sub(4).min(18);
    let popup = Rect::new(area.x + (area.width - width) / 2, area.y + 2, width, height);
    frame.render_widget(Clear, popup);
    let inner = popup.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(inner);
    let block = Block::default()
        .title(" OPEN FILE ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent))
        .style(Style::default().bg(theme.surface_raised));
    frame.render_widget(block, popup);
    frame.render_widget(
        Paragraph::new(format!("> {}", state.palette_query))
            .style(Style::default().fg(theme.text).bg(theme.selection)),
        rows[0],
    );
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(rows[1]);
    let visible_height = usize::from(columns[0].height);
    let offset = state
        .file_picker_selected
        .saturating_sub(visible_height.saturating_sub(1));
    let matches = state
        .file_matches
        .iter()
        .skip(offset)
        .take(visible_height)
        .enumerate()
        .map(|(index, matched)| {
            let selected = offset + index == state.file_picker_selected;
            ListItem::new(matched.path.to_string_lossy().into_owned()).style(
                Style::default()
                    .fg(if selected {
                        theme.text
                    } else {
                        theme.text_muted
                    })
                    .bg(if selected {
                        theme.selection
                    } else {
                        theme.surface_raised
                    }),
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(List::new(matches), columns[0]);
    let preview = state
        .file_preview_lines
        .iter()
        .take(usize::from(columns[1].height.saturating_sub(1)))
        .enumerate()
        .map(|(index, line)| {
            Line::from(vec![
                Span::styled(
                    format!("{:>3} ", index + 1),
                    Style::default().fg(theme.text_faint),
                ),
                Span::styled(line.clone(), Style::default().fg(theme.text)),
            ])
        })
        .collect::<Vec<_>>();
    let preview_block = Block::default()
        .title(" PREVIEW ")
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(theme.border));
    frame.render_widget(
        Paragraph::new(preview)
            .block(preview_block)
            .style(Style::default().bg(theme.surface_raised)),
        columns[1],
    );
}

fn render_delete_confirmation(
    frame: &mut Frame,
    area: Rect,
    plan: &crate::workspace::DeletePlan,
    dirty_buffers: usize,
    theme: &Theme,
) {
    let width = area.width.saturating_sub(8).min(72);
    let popup = Rect::new(area.x + (area.width - width) / 2, area.y + 3, width, 7);
    frame.render_widget(Clear, popup);
    let warning = if dirty_buffers == 0 {
        String::new()
    } else {
        format!("\n  WARNING: {dirty_buffers} unsaved buffer(s) will remain open")
    };
    let text = format!(
        "  Delete {}?\n  {} entr{}{}\n\n  Enter: confirm    Esc: cancel",
        plan.path().relative().display(),
        plan.entry_count(),
        if plan.entry_count() == 1 { "y" } else { "ies" },
        warning,
    );
    let block = Block::default()
        .title(" CONFIRM DELETE ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.diagnostic_error))
        .style(Style::default().bg(theme.surface_raised));
    frame.render_widget(
        Paragraph::new(text)
            .block(block)
            .style(Style::default().fg(theme.text).bg(theme.surface_raised)),
        popup,
    );
}

fn render_close_confirmation(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    tab: usize,
    theme: &Theme,
) {
    let width = area.width.saturating_sub(8).min(64);
    let popup = Rect::new(area.x + (area.width - width) / 2, area.y + 3, width, 6);
    frame.render_widget(Clear, popup);
    let title = state
        .tabs
        .get(tab)
        .map_or_else(|| "this editor".to_owned(), |tab| tab.title());
    let text = format!(
        "  {title} has unsaved changes.\n  Close and discard them?\n\n  Enter: discard and close    Esc: cancel"
    );
    let block = Block::default()
        .title(" UNSAVED CHANGES ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.diagnostic_warning))
        .style(Style::default().bg(theme.surface_raised));
    frame.render_widget(
        Paragraph::new(text)
            .block(block)
            .style(Style::default().fg(theme.text).bg(theme.surface_raised)),
        popup,
    );
}

fn render_recovery_prompt(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let width = area.width.saturating_sub(8).min(72);
    let height = area.height.saturating_sub(4).min(12);
    let popup = Rect::new(area.x + (area.width - width) / 2, area.y + 2, width, height);
    frame.render_widget(Clear, popup);
    let names = state
        .pending_recovery
        .iter()
        .take(5)
        .map(|buffer| {
            buffer.path.as_ref().map_or_else(
                || "  • Untitled".to_owned(),
                |path| format!("  • {}", path.display()),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let text = format!(
        "Mica found {} unsaved buffer(s) after an abnormal exit.\n\n{}\n\n[R] Recover    [D] Discard    [L] Decide later",
        state.pending_recovery.len(),
        names,
    );
    let block = Block::default()
        .title(" CRASH RECOVERY ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.diagnostic_warning))
        .style(Style::default().bg(theme.surface_raised));
    frame.render_widget(
        Paragraph::new(text)
            .block(block)
            .style(Style::default().fg(theme.text).bg(theme.surface_raised)),
        popup,
    );
}

#[cfg(test)]
mod tests {
    use std::fs;

    use ratatui::{Terminal, backend::TestBackend};

    use super::*;
    use crate::{config::Keymap, workspace::WorkspaceRoot};

    fn state() -> AppState {
        let path = std::env::temp_dir().join(format!("mica-render-{}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        AppState::new(
            WorkspaceRoot::new(path).unwrap(),
            Default::default(),
            Keymap::default(),
            Vec::new(),
            false,
        )
    }

    #[test]
    fn editor_layout_renders_at_minimum_supported_size() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = state();
        let theme = Theme::mica_dark(crate::ui::ColorMode::Ansi256);
        terminal
            .draw(|frame| {
                render(frame, &state, &theme);
            })
            .unwrap();
        let screen = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(screen.contains("EXPLORER"));
        assert!(screen.contains("MICA"));
    }

    #[test]
    fn tiny_terminal_uses_safe_fallback() {
        let backend = TestBackend::new(30, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = state();
        let theme = Theme::mica_dark(crate::ui::ColorMode::Ansi256);
        terminal
            .draw(|frame| {
                render(frame, &state, &theme);
            })
            .unwrap();
        let screen = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(screen.contains("Mica needs"));
    }
}
