use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Margin, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Tabs},
};

use crate::{
    app::{
        AppState, BottomPanelView, DiagnosticRow, Focus, GitSection, Overlay, PathAction,
        SidebarView, WorkspaceSearchRow,
    },
    buffer::LineEnding,
    workspace::{TreeEntry, TreeEntryKind},
};

use super::{
    Theme,
    icons::{IconSet, icons},
};

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
        render_bottom(frame, regions.bottom, state, theme);
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
            Overlay::ConfirmSaveAs { plan } => {
                render_save_as_confirmation(frame, area, plan, theme);
            }
            Overlay::RecoveryPrompt => render_recovery_prompt(frame, area, state, theme),
            Overlay::GitCommitInput => {
                render_text_prompt(frame, area, " COMMIT MESSAGE ", state, theme)
            }
            Overlay::ConfirmGitRestore { path } => {
                render_git_restore_confirmation(frame, area, path, theme)
            }
            Overlay::ConfirmGitHunkRestore { .. } => {
                render_hunk_restore_confirmation(frame, area, theme)
            }
            Overlay::GitBranchPicker => render_branch_picker(frame, area, state, theme),
            Overlay::GitBranchCreate => {
                render_text_prompt(frame, area, " CREATE BRANCH ", state, theme)
            }
            Overlay::SearchIncludeGlobs => {
                render_text_prompt(frame, area, " SEARCH INCLUDE GLOBS ", state, theme)
            }
            Overlay::SearchExcludeGlobs => {
                render_text_prompt(frame, area, " SEARCH EXCLUDE GLOBS ", state, theme)
            }
            Overlay::ConfirmQuitTerminal => render_terminal_quit_confirmation(frame, area, theme),
            Overlay::LspHover => render_lsp_hover(frame, area, state, theme),
            Overlay::LspCompletion => render_lsp_completion(frame, area, state, theme),
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
    if state.sidebar_view == SidebarView::SourceControl {
        let content = git_sidebar_lines(state, theme);
        frame.render_widget(
            Paragraph::new(content)
                .block(block)
                .style(Style::default().fg(theme.text_muted).bg(theme.surface)),
            area,
        );
        return;
    }
    if state.sidebar_view != SidebarView::Explorer {
        frame.render_widget(
            Paragraph::new(search_sidebar_lines(state, theme, usize::from(area.height)))
                .block(block)
                .style(Style::default().fg(theme.text_muted).bg(theme.surface)),
            area,
        );
        return;
    }
    let max = usize::from(area.height.saturating_sub(1));
    let icon_set = icons(state.settings.ui.icon_mode);
    let items = state
        .tree
        .visible_entries()
        .take(max)
        .enumerate()
        .map(|(index, entry)| {
            let icon = match entry.kind {
                TreeEntryKind::Directory if state.tree.is_expanded(&entry.relative_path) => {
                    icon_set.dir_expanded
                }
                TreeEntryKind::Directory => icon_set.dir_collapsed,
                TreeEntryKind::File => icon_set.file,
                TreeEntryKind::Symlink => icon_set.symlink,
            };
            let name = entry.relative_path.file_name().map_or_else(
                || entry.relative_path.to_string_lossy(),
                |name| name.to_string_lossy(),
            );
            let prefix = "  ".repeat(entry.depth);
            let diagnostic_root = state.workspace.as_path().join(&entry.relative_path);
            let (errors, warnings) = state
                .diagnostics
                .diagnostics()
                .iter()
                .filter(|diagnostic| {
                    if entry.kind == TreeEntryKind::Directory {
                        diagnostic.file.starts_with(&diagnostic_root)
                    } else {
                        diagnostic.file == diagnostic_root
                    }
                })
                .fold(
                    (0usize, 0usize),
                    |(errors, warnings), diagnostic| match diagnostic.severity {
                        crate::diagnostics::DiagnosticSeverity::Error => (errors + 1, warnings),
                        crate::diagnostics::DiagnosticSeverity::Warning => (errors, warnings + 1),
                        _ => (errors, warnings),
                    },
                );
            let badge = if errors > 0 {
                format!("  E{errors}")
            } else if warnings > 0 {
                format!("  W{warnings}")
            } else {
                String::new()
            };
            let style = if index == state.tree_selected {
                Style::default()
                    .fg(theme.text)
                    .bg(theme.selection)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text_muted).bg(theme.surface)
            };
            let mut spans = vec![Span::styled(format!("{prefix}{icon} {name}"), style)];
            if let Some(marker) = tree_git_marker(entry, state.git_status.as_ref()) {
                let (symbol, color) = tree_git_marker_glyph(marker, icon_set, theme);
                spans.push(Span::styled(format!(" {symbol}"), style.fg(color)));
            }
            if !badge.is_empty() {
                spans.push(Span::styled(badge, style));
            }
            ListItem::new(Line::from(spans))
        })
        .collect::<Vec<_>>();
    frame.render_widget(List::new(items).block(block), area);
}

/// Per-entry Git status marker for the Explorer tree
/// (SPEC/03_workspace.md §2.1). A file/symlink resolves to its exact Git
/// change, if any; a directory resolves to [`TreeGitMarker::Directory`] as
/// soon as *any* descendant path has a change.
enum TreeGitMarker<'a> {
    File(&'a crate::git::GitFileChange),
    Directory,
}

fn tree_git_marker<'a>(
    entry: &TreeEntry,
    status: Option<&'a crate::git::GitStatus>,
) -> Option<TreeGitMarker<'a>> {
    let status = status?;
    match entry.kind {
        TreeEntryKind::Directory => status
            .files
            .iter()
            .any(|file| file.path.starts_with(&entry.relative_path))
            .then_some(TreeGitMarker::Directory),
        TreeEntryKind::File | TreeEntryKind::Symlink => status
            .files
            .iter()
            .find(|file| file.path == entry.relative_path)
            .map(TreeGitMarker::File),
    }
}

/// Resolves a [`TreeGitMarker`] to a (symbol, color) pair. Directories
/// intentionally collapse every descendant state to a single "has changes"
/// dot in the `git_modified` color rather than prioritizing among
/// heterogeneous child states (added/deleted/conflicted/...) for one glyph;
/// the symbol still satisfies the "not color alone" accessibility rule
/// (SPEC/01_ui.md §6.6) since it is present regardless of color perception.
fn tree_git_marker_glyph(
    marker: TreeGitMarker<'_>,
    icons: &IconSet,
    theme: &Theme,
) -> (String, Color) {
    match marker {
        TreeGitMarker::File(change) => (
            crate::git::status_symbol(change).to_string(),
            git_status_color(change, theme),
        ),
        TreeGitMarker::Directory => (icons.bullet.to_owned(), theme.git_modified),
    }
}

fn git_status_color(change: &crate::git::GitFileChange, theme: &Theme) -> Color {
    if change.conflicted {
        return theme.git_conflict;
    }
    if change.untracked {
        return theme.git_added;
    }
    match change.kind {
        crate::git::GitFileKind::Added | crate::git::GitFileKind::Untracked => theme.git_added,
        crate::git::GitFileKind::Deleted => theme.git_deleted,
        crate::git::GitFileKind::Unmerged => theme.git_conflict,
        crate::git::GitFileKind::Modified
        | crate::git::GitFileKind::Renamed
        | crate::git::GitFileKind::Copied
        | crate::git::GitFileKind::TypeChanged => theme.git_modified,
    }
}

fn search_sidebar_lines(state: &AppState, theme: &Theme, height: usize) -> Vec<Line<'static>> {
    let icon_set = icons(state.settings.ui.icon_mode);
    let running = if state.workspace_search_running {
        format!(" {}", icon_set.ellipsis)
    } else {
        String::new()
    };
    let mut lines = vec![
        Line::from(Span::styled(
            format!("> {}{running}", state.workspace_search.query),
            Style::default().fg(theme.text).bg(theme.selection),
        )),
        Line::from(format!(
            "[{}]Case [{}]Word [{}]Regex [{}]Hidden [{}]Binary",
            if state.workspace_search.case_sensitive {
                "x"
            } else {
                " "
            },
            if state.workspace_search.whole_word {
                "x"
            } else {
                " "
            },
            if state.workspace_search.regex {
                "x"
            } else {
                " "
            },
            if state.workspace_search.show_hidden {
                "x"
            } else {
                " "
            },
            if state.workspace_search.include_binary {
                "x"
            } else {
                " "
            }
        )),
        Line::from(format!(" {} results", state.workspace_matches.len())),
    ];
    for row in state
        .workspace_search_rows()
        .into_iter()
        .take(height.saturating_sub(lines.len()))
    {
        let WorkspaceSearchRow::Match(index) = row else {
            let WorkspaceSearchRow::File(path) = row else {
                unreachable!();
            };
            let collapsed = state.workspace_search_collapsed.contains(&path);
            let count = state
                .workspace_matches
                .iter()
                .filter(|matched| matched.path == path)
                .count();
            lines.push(Line::from(Span::styled(
                format!(
                    "{} {} ({count})",
                    if collapsed {
                        icon_set.dir_collapsed
                    } else {
                        icon_set.dir_expanded
                    },
                    path.display()
                ),
                Style::default()
                    .fg(theme.text)
                    .bg(theme.surface)
                    .add_modifier(Modifier::BOLD),
            )));
            continue;
        };
        let Some(matched) = state.workspace_matches.get(index) else {
            continue;
        };
        let selected = index == state.workspace_search_selected;
        let background = if selected {
            theme.selection
        } else {
            theme.surface
        };
        let prefix = format!("  {}:{}  ", matched.line, matched.column);
        let before = &matched.line_text[..matched.match_start];
        let hit = &matched.line_text[matched.match_start..matched.match_end];
        let after = &matched.line_text[matched.match_end..];
        lines.push(Line::from(vec![
            Span::styled(prefix, Style::default().fg(theme.text_faint).bg(background)),
            Span::styled(
                before.to_owned(),
                Style::default().fg(theme.text_muted).bg(background),
            ),
            Span::styled(
                hit.to_owned(),
                Style::default()
                    .fg(theme.accent)
                    .bg(background)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                after.to_owned(),
                Style::default().fg(theme.text_muted).bg(background),
            ),
        ]));
    }
    lines
}

fn git_sidebar_lines(state: &AppState, theme: &Theme) -> Vec<Line<'static>> {
    let icon_set = icons(state.settings.ui.icon_mode);
    if state.git_loading {
        return vec![Line::from(format!(" Refreshing{}", icon_set.ellipsis))];
    }
    let Some(status) = &state.git_status else {
        return vec![Line::from(
            state
                .git_error
                .as_deref()
                .unwrap_or("No Git repository")
                .to_owned(),
        )];
    };
    let branch = status.branch.as_deref().unwrap_or("detached HEAD");
    let tracking = match (status.ahead, status.behind) {
        (0, 0) => String::new(),
        (ahead, 0) => format!(" {}{ahead}", icon_set.arrow_up),
        (0, behind) => format!(" {}{behind}", icon_set.arrow_down),
        (ahead, behind) => format!(
            " {}{ahead} {}{behind}",
            icon_set.arrow_up, icon_set.arrow_down
        ),
    };
    let mut lines = vec![Line::from(format!(
        " {} {branch}{tracking}",
        icon_set.branch
    ))];
    let mut selection_index = 0usize;
    for section in GitSection::ALL {
        let files = status
            .files
            .iter()
            .filter(|file| section.contains(file))
            .collect::<Vec<_>>();
        if files.is_empty() {
            continue;
        }
        lines.push(Line::from(""));
        lines.push(Line::from(format!(
            " {} ({})",
            section.title(),
            files.len()
        )));
        for file in files {
            let marker = if file.conflicted {
                "!"
            } else if file.untracked {
                "?"
            } else {
                match file.kind {
                    crate::git::GitFileKind::Added => "A",
                    crate::git::GitFileKind::Deleted => "D",
                    crate::git::GitFileKind::Renamed => "R",
                    crate::git::GitFileKind::Copied => "C",
                    crate::git::GitFileKind::TypeChanged => "T",
                    crate::git::GitFileKind::Unmerged => "!",
                    crate::git::GitFileKind::Untracked => "?",
                    crate::git::GitFileKind::Modified => "M",
                }
            };
            let selected = selection_index == state.git_selected;
            lines.push(Line::from(Span::styled(
                format!("  {marker} {}", file.path.display()),
                Style::default()
                    .fg(if selected {
                        theme.text
                    } else {
                        theme.text_muted
                    })
                    .bg(if selected {
                        theme.selection
                    } else {
                        theme.surface
                    })
                    .add_modifier(if selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            )));
            selection_index = selection_index.saturating_add(1);
        }
    }
    if status.files.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(" No changes"));
    }
    lines
}

fn render_editor(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let icon_set = icons(state.settings.ui.icon_mode);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(area);
    let titles = state
        .tabs
        .iter()
        .map(|tab| {
            let dirty = if tab.buffer.is_dirty() {
                format!(" {}", icon_set.dirty)
            } else {
                String::new()
            };
            Line::from(format!(" {}{dirty} {} ", tab.title(), icon_set.close))
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
    let active_diagnostics = tab
        .buffer
        .path()
        .map(|path| state.diagnostics.for_file(path).collect::<Vec<_>>())
        .unwrap_or_default();
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
            let (marker, diagnostic_color) = editor_gutter_marker(state, line_index, theme);
            let number = format!("{marker} {:>width$} ", line_index + 1, width = gutter_width);
            let active = line_index == cursor_line;
            let mut spans = vec![Span::styled(
                number,
                Style::default()
                    .fg(diagnostic_color.unwrap_or(if active {
                        theme.accent
                    } else {
                        theme.text_faint
                    }))
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
                active,
                theme,
                &EditorLineDecorations {
                    search: &tab.view.search,
                    highlights: &tab.highlights,
                    diagnostics: &active_diagnostics,
                },
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
                    + 3
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

fn editor_gutter_marker(
    state: &AppState,
    line_index: usize,
    theme: &Theme,
) -> (&'static str, Option<Color>) {
    if let Some(path) = state.active_path()
        && let Some(diagnostic) = state
            .diagnostics
            .for_file(path)
            .filter(|diagnostic| diagnostic.range.start.line == line_index)
            .min_by_key(|diagnostic| diagnostic.severity)
    {
        let (marker, color) = diagnostic_style(diagnostic.severity, theme);
        return (marker, Some(color));
    }
    let Some(diff) = &state.git_diff else {
        return (" ", None);
    };
    let Some(active_path) = state.active_path() else {
        return (" ", None);
    };
    let matches_path = active_path == diff.path
        || active_path
            .strip_prefix(state.workspace.as_path())
            .is_ok_and(|relative| relative == diff.path);
    if !matches_path {
        return (" ", None);
    }
    let line_number = line_index + 1;
    for hunk in &diff.hunks {
        let has_removed = hunk
            .lines
            .iter()
            .any(|line| line.kind == crate::git::DiffLineKind::Removed);
        if hunk.lines.iter().any(|line| {
            line.kind == crate::git::DiffLineKind::Added && line.new_line == Some(line_number)
        }) {
            return (if has_removed { "~" } else { "+" }, None);
        }
        let only_removed = has_removed
            && !hunk
                .lines
                .iter()
                .any(|line| line.kind == crate::git::DiffLineKind::Added);
        if only_removed && hunk.new_start.max(1) == line_number {
            return ("-", None);
        }
    }
    (" ", None)
}

struct EditorLineDecorations<'a> {
    search: &'a crate::editor::BufferSearchState,
    highlights: &'a [crate::editor::HighlightSpan],
    diagnostics: &'a [&'a crate::diagnostics::Diagnostic],
}

fn editor_line_spans(
    content: &str,
    line_start_char: usize,
    selection: crate::buffer::Selection,
    active: bool,
    theme: &Theme,
    decorations: &EditorLineDecorations<'_>,
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
    for matched in &decorations.search.matches {
        add_boundaries(matched);
    }
    for highlight in decorations.highlights {
        add_boundaries(&(highlight.start_char..highlight.end_char));
    }
    for diagnostic in decorations.diagnostics {
        if let (Some(start), Some(end)) = (
            diagnostic.range.start.char_offset,
            diagnostic.range.end.char_offset,
        ) {
            add_boundaries(&(start..end.max(start + 1)));
        }
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
        let matched = decorations
            .search
            .matches
            .iter()
            .any(|range| range.contains(&global_start));
        let syntax = decorations
            .highlights
            .iter()
            .filter(|span| span.start_char <= global_start && global_start < span.end_char)
            .min_by_key(|span| span.end_char.saturating_sub(span.start_char));
        let mut style = if selected {
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
        if let Some(diagnostic) = decorations.diagnostics.iter().find(|diagnostic| {
            diagnostic
                .range
                .start
                .char_offset
                .zip(diagnostic.range.end.char_offset)
                .is_some_and(|(start, end)| {
                    start <= global_start && global_start < end.max(start + 1)
                })
        }) {
            let (_, color) = diagnostic_style(diagnostic.severity, theme);
            style = style.fg(color).add_modifier(Modifier::UNDERLINED);
        }
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

fn render_bottom(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    if state.bottom_panel_view == BottomPanelView::Problems {
        render_problems(frame, area, state, theme);
        return;
    }
    if state.bottom_panel_view == BottomPanelView::Terminal {
        render_terminal(frame, area, state, theme);
        return;
    }
    if state.bottom_panel_view == BottomPanelView::Output {
        let height = usize::from(area.height.saturating_sub(1));
        let start = state.output_lines.len().saturating_sub(height);
        let output = if state.output_lines.is_empty() {
            vec![Line::from("No output")]
        } else {
            state
                .output_lines
                .iter()
                .skip(start)
                .cloned()
                .map(Line::from)
                .collect()
        };
        frame.render_widget(
            Paragraph::new(output)
                .block(
                    Block::default()
                        .title(" PROBLEMS  DIFF  OUTPUT  TERMINAL   [Output] ")
                        .borders(Borders::TOP)
                        .border_style(Style::default().fg(theme.border)),
                )
                .style(Style::default().fg(theme.text_muted).bg(theme.surface)),
            area,
        );
        return;
    }
    let icon_set = icons(state.settings.ui.icon_mode);
    let block = Block::default()
        .title(format!(
            " PROBLEMS  DIFF  OUTPUT  TERMINAL   [Diff] {}/{} hunk ",
            icon_set.arrow_up, icon_set.arrow_down
        ))
        .borders(Borders::TOP)
        .border_style(Style::default().fg(theme.border));
    frame.render_widget(
        Paragraph::new(git_diff_lines(
            state,
            theme,
            usize::from(area.height.saturating_sub(1)),
        ))
        .block(block)
        .style(Style::default().fg(theme.text_muted).bg(theme.surface)),
        area,
    );
}

fn render_problems(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let icon_set = icons(state.settings.ui.icon_mode);
    let diagnostics = state.visible_diagnostics();
    let filter = state
        .diagnostic_filter
        .map_or("All", |severity| match severity {
            crate::diagnostics::DiagnosticSeverity::Error => "Errors",
            crate::diagnostics::DiagnosticSeverity::Warning => "Warnings",
            crate::diagnostics::DiagnosticSeverity::Information => "Information",
            crate::diagnostics::DiagnosticSeverity::Hint => "Hints",
        });
    let lines = state
        .diagnostic_rows()
        .into_iter()
        .take(usize::from(area.height.saturating_sub(1)))
        .filter_map(|row| match row {
            DiagnosticRow::File(path) => Some(Line::from(Span::styled(
                format!("{} {}", icon_set.dir_expanded, path.display()),
                Style::default()
                    .fg(theme.text)
                    .bg(theme.surface)
                    .add_modifier(Modifier::BOLD),
            ))),
            DiagnosticRow::Item(index) => {
                let diagnostic = diagnostics.get(index)?;
                let (marker, color) = diagnostic_style(diagnostic.severity, theme);
                let selected = index == state.diagnostic_selected;
                let source = format!("{:?}", diagnostic.source).to_ascii_lowercase();
                let code = diagnostic
                    .code
                    .as_deref()
                    .map_or(String::new(), |code| format!(" [{code}]"));
                let stale = if diagnostic.stale { " (stale)" } else { "" };
                Some(Line::from(vec![
                    Span::styled(
                        format!("  {marker} "),
                        Style::default().fg(color).bg(if selected {
                            theme.selection
                        } else {
                            theme.surface
                        }),
                    ),
                    Span::styled(
                        format!(
                            "{}{} — {}  {}:{}{stale}",
                            diagnostic.message,
                            code,
                            source,
                            diagnostic.range.start.line + 1,
                            diagnostic.range.start.column + 1,
                        ),
                        Style::default()
                            .fg(if selected {
                                theme.text
                            } else {
                                theme.text_muted
                            })
                            .bg(if selected {
                                theme.selection
                            } else {
                                theme.surface
                            }),
                    ),
                ]))
            }
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(if lines.is_empty() {
            vec![Line::from("No problems")]
        } else {
            lines
        })
        .block(
            Block::default()
                .title(format!(
                    " PROBLEMS  DIFF  OUTPUT  TERMINAL   [Problems: {filter}] F filter "
                ))
                .borders(Borders::TOP)
                .border_style(Style::default().fg(if state.focus == Focus::BottomPanel {
                    theme.accent
                } else {
                    theme.border
                })),
        )
        .style(Style::default().bg(theme.surface)),
        area,
    );
}

fn diagnostic_style(
    severity: crate::diagnostics::DiagnosticSeverity,
    theme: &Theme,
) -> (&'static str, Color) {
    match severity {
        crate::diagnostics::DiagnosticSeverity::Error => ("E", theme.diagnostic_error),
        crate::diagnostics::DiagnosticSeverity::Warning => ("W", theme.diagnostic_warning),
        crate::diagnostics::DiagnosticSeverity::Information => ("I", theme.accent),
        crate::diagnostics::DiagnosticSeverity::Hint => ("H", theme.text_faint),
    }
}

fn render_terminal(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let content_height = usize::from(area.height.saturating_sub(1));
    let snapshot = state.terminal.snapshot(state.terminal_scroll_offset);
    let title = snapshot.title.as_deref().unwrap_or("Terminal");
    let status = if state.terminal_running {
        "running".to_owned()
    } else if let Some((code, success)) = state.terminal_exit {
        format!("exited {code}{}", if success { "" } else { " !" })
    } else {
        "not started".to_owned()
    };
    let scroll = if state.terminal_scroll_offset > 0 {
        format!("  scroll +{}", state.terminal_scroll_offset)
    } else {
        String::new()
    };
    let lines = snapshot
        .lines
        .iter()
        .take(content_height)
        .enumerate()
        .map(|(row, line)| terminal_line(line, row, state.terminal_selection, theme))
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(format!(
                        " PROBLEMS  DIFF  OUTPUT  TERMINAL   [Terminal] {title} [{status}]{scroll} "
                    ))
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(if state.focus == Focus::BottomPanel {
                        theme.accent
                    } else {
                        theme.border
                    })),
            )
            .style(Style::default().fg(theme.text).bg(theme.background)),
        area,
    );
    if state.focus == Focus::BottomPanel
        && snapshot.cursor_visible
        && snapshot.cursor_row < content_height
    {
        let x = area
            .x
            .saturating_add(u16::try_from(snapshot.cursor_col).unwrap_or(u16::MAX));
        let y = area
            .y
            .saturating_add(1)
            .saturating_add(u16::try_from(snapshot.cursor_row).unwrap_or(u16::MAX));
        if x < area.right() && y < area.bottom() {
            frame.set_cursor_position(Position::new(x, y));
        }
    }
}

fn terminal_line(
    cells: &[crate::terminal::TerminalCell],
    row: usize,
    selection: Option<((usize, usize), (usize, usize))>,
    theme: &Theme,
) -> Line<'static> {
    let spans = cells
        .iter()
        .enumerate()
        .filter(|(_, cell)| !cell.wide_continuation)
        .map(|(column, cell)| {
            let mut foreground = terminal_color(cell.style.foreground, theme.text);
            let mut background = terminal_color(cell.style.background, theme.background);
            if cell.style.inverse {
                std::mem::swap(&mut foreground, &mut background);
            }
            if terminal_cell_selected(selection, row, column) {
                background = theme.selection;
            }
            let mut modifier = Modifier::empty();
            modifier.set(Modifier::BOLD, cell.style.bold);
            modifier.set(Modifier::ITALIC, cell.style.italic);
            modifier.set(Modifier::UNDERLINED, cell.style.underline);
            Span::styled(
                cell.character.to_string(),
                Style::default()
                    .fg(foreground)
                    .bg(background)
                    .add_modifier(modifier),
            )
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

fn terminal_cell_selected(
    selection: Option<((usize, usize), (usize, usize))>,
    row: usize,
    column: usize,
) -> bool {
    let Some((start, end)) = selection else {
        return false;
    };
    let (start, end) = if start <= end {
        (start, end)
    } else {
        (end, start)
    };
    (row, column) >= start && (row, column) <= end
}

fn terminal_color(color: crate::terminal::TerminalColor, default: Color) -> Color {
    match color {
        crate::terminal::TerminalColor::Default => default,
        crate::terminal::TerminalColor::Indexed(index) => Color::Indexed(index),
        crate::terminal::TerminalColor::Rgb(red, green, blue) => Color::Rgb(red, green, blue),
    }
}

fn git_diff_lines(state: &AppState, theme: &Theme, limit: usize) -> Vec<Line<'static>> {
    let Some(diff) = &state.git_diff else {
        return vec![Line::from("No output")];
    };
    if diff.binary {
        return vec![Line::from(format!("Binary file: {}", diff.path.display()))];
    }
    let mut hunk_index: Option<usize> = None;
    diff.raw
        .lines()
        .take(limit)
        .map(|line| {
            if line.starts_with("@@ ") {
                hunk_index = Some(hunk_index.map_or(0usize, |index| index.saturating_add(1)));
            }
            let color = if line.starts_with('+') && !line.starts_with("+++") {
                theme.git_added
            } else if line.starts_with('-') && !line.starts_with("---") {
                theme.git_deleted
            } else {
                theme.text_muted
            };
            let selected = hunk_index == Some(state.git_hunk_selected);
            Line::from(Span::styled(
                line.to_owned(),
                Style::default()
                    .fg(color)
                    .bg(if selected {
                        theme.active_line
                    } else {
                        theme.surface
                    })
                    .add_modifier(if line.starts_with("@@ ") && selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ))
        })
        .collect()
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
    let icon_set = icons(state.settings.ui.icon_mode);
    let message = state.notification.as_deref().unwrap_or("");
    let git = state.git_status.as_ref().map_or_else(String::new, |git| {
        let branch = git.branch.as_deref().unwrap_or("detached");
        let count = git.files.len();
        let tracking = match (git.ahead, git.behind) {
            (0, 0) => String::new(),
            (ahead, 0) => format!(" {}{ahead}", icon_set.arrow_up),
            (0, behind) => format!(" {}{behind}", icon_set.arrow_down),
            (ahead, behind) => format!(
                " {}{ahead}{}{behind}",
                icon_set.arrow_up, icon_set.arrow_down
            ),
        };
        format!("Git {branch}{tracking} {}{count}", icon_set.delta)
    });
    let (errors, warnings) = state.diagnostics.counts();
    let problems = format!("E {errors}  W {warnings}");
    let status = format!(" {file}   {message}   {git}   {problems}   {encoding}   {position} ");
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
        PathAction::SaveAs { .. } => " SAVE AS (workspace-relative) ",
    };
    let width = area.width.saturating_sub(8).min(72);
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + 3,
        width,
        7.min(area.height.saturating_sub(4)),
    );
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

fn render_text_prompt(frame: &mut Frame, area: Rect, title: &str, state: &AppState, theme: &Theme) {
    let width = area.width.saturating_sub(8).min(72);
    let popup = Rect::new(area.x + (area.width - width) / 2, area.y + 3, width, 3);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(format!("> {}", state.palette_query))
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.accent)),
            )
            .style(Style::default().fg(theme.text).bg(theme.surface_raised)),
        popup,
    );
}

fn render_git_restore_confirmation(
    frame: &mut Frame,
    area: Rect,
    path: &std::path::Path,
    theme: &Theme,
) {
    let width = area.width.saturating_sub(8).min(72);
    let popup = Rect::new(area.x + (area.width - width) / 2, area.y + 3, width, 6);
    frame.render_widget(Clear, popup);
    let text = format!(
        "Discard all working-tree changes in {}?\n\nEnter: restore permanently   Esc: cancel",
        path.display()
    );
    frame.render_widget(
        Paragraph::new(text)
            .block(
                Block::default()
                    .title(" CONFIRM RESTORE ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.diagnostic_error)),
            )
            .style(Style::default().fg(theme.text).bg(theme.surface_raised)),
        popup,
    );
}

fn render_hunk_restore_confirmation(frame: &mut Frame, area: Rect, theme: &Theme) {
    let width = area.width.saturating_sub(8).min(68);
    let popup = Rect::new(area.x + (area.width - width) / 2, area.y + 3, width, 6);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(
            "Discard the selected working-tree hunk?\n\nEnter: restore permanently   Esc: cancel",
        )
        .block(
            Block::default()
                .title(" CONFIRM HUNK RESTORE ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.diagnostic_error)),
        )
        .style(Style::default().fg(theme.text).bg(theme.surface_raised)),
        popup,
    );
}

fn render_terminal_quit_confirmation(frame: &mut Frame, area: Rect, theme: &Theme) {
    let width = area.width.saturating_sub(8).min(68);
    let popup = Rect::new(area.x + (area.width - width) / 2, area.y + 3, width, 6);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(
            "A process is still running in the integrated terminal.\n\nEnter: terminate and quit   Esc: cancel",
        )
        .block(
            Block::default()
                .title(" TERMINATE TERMINAL? ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.diagnostic_warning)),
        )
        .style(Style::default().fg(theme.text).bg(theme.surface_raised)),
        popup,
    );
}

fn render_lsp_hover(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let width = area.width.saturating_sub(6).min(72);
    let height = u16::try_from(state.lsp_hover.len().saturating_add(2))
        .unwrap_or(u16::MAX)
        .min(area.height.saturating_sub(4))
        .max(3);
    let popup = Rect::new(area.x + (area.width - width) / 2, area.y + 2, width, height);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(state.lsp_hover.join("\n"))
            .block(
                Block::default()
                    .title(" HOVER  Enter/Esc close ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.accent)),
            )
            .style(Style::default().fg(theme.text).bg(theme.surface_raised)),
        popup,
    );
}

fn render_lsp_completion(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let width = area.width.saturating_sub(6).min(64);
    let height = area.height.saturating_sub(4).min(14);
    let popup = Rect::new(
        area.right().saturating_sub(width + 2),
        area.y + 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let items = state
        .lsp_completions
        .iter()
        .take(usize::from(height.saturating_sub(2)))
        .enumerate()
        .map(|(index, completion)| {
            let selected = index == state.lsp_completion_selected;
            let detail = completion.detail.as_deref().unwrap_or("");
            ListItem::new(format!("{}  {detail}", completion.label)).style(
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
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .title(" COMPLETION ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.accent)),
        ),
        popup,
    );
}

fn render_branch_picker(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let width = area.width.saturating_sub(8).min(64);
    let height = area.height.saturating_sub(4).min(16);
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
    frame.render_widget(
        Block::default()
            .title(" SWITCH BRANCH ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.accent))
            .style(Style::default().bg(theme.surface_raised)),
        popup,
    );
    frame.render_widget(
        Paragraph::new(format!("> {}", state.palette_query))
            .style(Style::default().fg(theme.text).bg(theme.selection)),
        rows[0],
    );
    let icon_set = icons(state.settings.ui.icon_mode);
    let branches = state
        .visible_git_branches()
        .into_iter()
        .take(usize::from(rows[1].height))
        .enumerate()
        .map(|(index, branch)| {
            let selected = index == state.git_branch_selected;
            let marker = if branch.current { icon_set.dirty } else { " " };
            let remote = if branch.remote { "  remote" } else { "" };
            ListItem::new(format!("{marker} {}{remote}", branch.name)).style(
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
    frame.render_widget(List::new(branches), rows[1]);
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

fn render_save_as_confirmation(
    frame: &mut Frame,
    area: Rect,
    plan: &crate::app::SaveAsPlan,
    theme: &Theme,
) {
    let width = area.width.saturating_sub(8).min(72);
    let popup = Rect::new(area.x + (area.width - width) / 2, area.y + 3, width, 6);
    frame.render_widget(Clear, popup);
    let text = format!(
        "  {} already exists.\n  Replace it with the current buffer?\n\n  Enter: replace    Esc: cancel",
        plan.snapshot.path.display()
    );
    let block = Block::default()
        .title(" CONFIRM SAVE AS ")
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
    let icon_set = icons(state.settings.ui.icon_mode);
    let names = state
        .pending_recovery
        .iter()
        .take(5)
        .map(|buffer| {
            buffer.path.as_ref().map_or_else(
                || format!("  {} Untitled", icon_set.bullet),
                |path| format!("  {} {}", icon_set.bullet, path.display()),
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
