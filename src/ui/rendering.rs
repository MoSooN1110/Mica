use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Margin, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Tabs},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    app::{
        AppState, BottomPanelView, DiagnosticRow, Focus, GitSection, Overlay, PathAction,
        SidebarView, WorkspaceSearchRow,
    },
    buffer::LineEnding,
    config::IconMode,
    workspace::{TreeEntry, TreeEntryKind},
};

use super::{
    Theme,
    icons::{FileIcon, FileIconColor, IconSet, file_icon, folder_icon, icons},
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
            } => render_delete_confirmation(
                frame,
                area,
                plan,
                *dirty_buffers,
                theme,
                state.settings.ui.icon_mode,
            ),
            Overlay::ConfirmClose { tab } => {
                render_close_confirmation(frame, area, state, *tab, theme);
            }
            Overlay::ConfirmSaveAs { plan } => {
                render_save_as_confirmation(frame, area, plan, theme, state.settings.ui.icon_mode);
            }
            Overlay::RecoveryPrompt => render_recovery_prompt(frame, area, state, theme),
            Overlay::GitCommitInput => {
                render_text_prompt(frame, area, " COMMIT MESSAGE ", state, theme)
            }
            Overlay::ConfirmGitRestore { path } => render_git_restore_confirmation(
                frame,
                area,
                path,
                theme,
                state.settings.ui.icon_mode,
            ),
            Overlay::ConfirmGitHunkRestore { .. } => {
                render_hunk_restore_confirmation(frame, area, theme, state.settings.ui.icon_mode)
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
            Overlay::ConfirmQuitTerminal => {
                render_terminal_quit_confirmation(frame, area, theme, state.settings.ui.icon_mode)
            }
            Overlay::LspHover => render_lsp_hover(frame, area, state, theme),
            Overlay::LspCompletion => render_lsp_completion(frame, area, state, theme),
        }
    }
    regions
}

/// Maps a [`FileIconColor`] role onto the active theme (SPEC/01_ui.md §6.3:
/// never hard-code a `Color` in the render layer). Mirrors the existing
/// `git_status_color`/`syntax_color` helpers below.
fn file_icon_color(color: FileIconColor, theme: &Theme) -> Color {
    match color {
        FileIconColor::Number => theme.syntax_number,
        FileIconColor::Function => theme.syntax_function,
        FileIconColor::Type => theme.syntax_type,
        FileIconColor::Keyword => theme.syntax_keyword,
        FileIconColor::Str => theme.syntax_string,
        FileIconColor::Muted => theme.text_muted,
        FileIconColor::Constant => theme.syntax_constant,
        FileIconColor::GitModified => theme.git_modified,
        FileIconColor::Accent => theme.accent,
        FileIconColor::Faint => theme.text_faint,
    }
}

fn file_icon_span(icon: FileIcon, theme: &Theme, background: Color) -> Span<'static> {
    Span::styled(
        icon.glyph.to_owned(),
        Style::default()
            .fg(file_icon_color(icon.color, theme))
            .bg(background),
    )
}

/// Builds a consistently-styled overlay `Block` (palette, pickers, prompts,
/// hover, completion, confirmations, ...): `surface_raised` background,
/// `border` token, accent+bold title, rounded corners in Unicode/NerdFont
/// mode (Ascii mode keeps plain corners — rounded corners are drawn with
/// non-ASCII box-drawing glyphs).
fn overlay_block(title: impl Into<String>, theme: &Theme, icon_mode: IconMode) -> Block<'static> {
    overlay_block_bordered(title, theme.border, theme, icon_mode)
}

/// Like [`overlay_block`], but with an explicit border color — used by
/// destructive/warning confirmations that signal severity through the
/// border (`diagnostic_error`/`diagnostic_warning`) as well as by
/// focus-driven overlays that want an accent border.
fn overlay_block_bordered(
    title: impl Into<String>,
    border_color: Color,
    theme: &Theme,
    icon_mode: IconMode,
) -> Block<'static> {
    let border_type = if icon_mode == IconMode::Ascii {
        BorderType::Plain
    } else {
        BorderType::Rounded
    };
    Block::default()
        .title(title.into())
        .title_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_type(border_type)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(theme.surface_raised))
}

/// Splits a workspace-relative path display string at its last `/` so
/// callers can render the directory part faint and the file name bright
/// (Source Control file rows, file picker results).
fn split_dirname_basename(display: &str) -> (&str, &str) {
    display
        .rsplit_once('/')
        .map_or(("", display), |(dir, base)| (dir, base))
}

/// Case-insensitive first-occurrence substring match, returning the
/// *character* range in `text` that matches `query`. Used to highlight the
/// command palette's substring filter (see `AppState::palette_commands`)
/// without threading match indices through the command registry.
fn substring_match_range(text: &str, query: &str) -> Option<(usize, usize)> {
    if query.is_empty() {
        return None;
    }
    let lower_text = text.to_ascii_lowercase();
    let lower_query = query.to_ascii_lowercase();
    let byte_start = lower_text.find(&lower_query)?;
    let char_start = lower_text[..byte_start].chars().count();
    let char_len = lower_query.chars().count();
    Some((char_start, char_start + char_len))
}

/// Cheap case-insensitive subsequence scan approximating which characters
/// of `text` matched `query` (greedy first-match), for highlighting fuzzy
/// file-picker results. `nucleo_matcher` doesn't expose match indices
/// through `FileMatch` (`src/search/files.rs`), and re-running its scorer
/// here just for indices would duplicate a non-trivial matcher; a greedy
/// subsequence scan is O(len) and visually matches nucleo's subsequence
/// semantics closely enough for a highlight, not a correctness-critical
/// path.
fn subsequence_match_chars(text: &str, query: &str) -> Vec<bool> {
    let lower_query = query.to_ascii_lowercase().chars().collect::<Vec<_>>();
    let mut result = vec![false; text.chars().count()];
    if lower_query.is_empty() {
        return result;
    }
    let mut query_index = 0usize;
    for (index, character) in text.chars().enumerate() {
        if query_index < lower_query.len()
            && character.to_ascii_lowercase() == lower_query[query_index]
        {
            result[index] = true;
            query_index += 1;
        }
    }
    result
}

/// Renders `text` as spans with subsequence-matched characters highlighted
/// in `theme.accent` + bold, per [`subsequence_match_chars`].
fn highlighted_spans(text: &str, query: &str, theme: &Theme, base: Style) -> Vec<Span<'static>> {
    let matches = subsequence_match_chars(text, query);
    let mut spans = Vec::new();
    let mut current = String::new();
    let mut current_matched = false;
    let mut first = true;
    for (character, matched) in text.chars().zip(matches) {
        if first {
            current_matched = matched;
            first = false;
        }
        if matched != current_matched {
            spans.push(Span::styled(
                std::mem::take(&mut current),
                if current_matched {
                    base.fg(theme.accent).add_modifier(Modifier::BOLD)
                } else {
                    base
                },
            ));
            current_matched = matched;
        }
        current.push(character);
    }
    if !current.is_empty() {
        spans.push(Span::styled(
            current,
            if current_matched {
                base.fg(theme.accent).add_modifier(Modifier::BOLD)
            } else {
                base
            },
        ));
    }
    spans
}

/// Renders `text` as spans with the first case-insensitive substring match
/// of `query` highlighted in `theme.accent` + bold, per
/// [`substring_match_range`]. Used for the command palette, whose filter
/// (`AppState::palette_commands`) is a substring `.contains()` check rather
/// than a fuzzy subsequence match, so highlighting should reflect that
/// exactly rather than approximate it.
fn substring_highlighted_spans(
    text: &str,
    query: &str,
    theme: &Theme,
    base: Style,
) -> Vec<Span<'static>> {
    let Some((match_start, match_end)) = substring_match_range(text, query) else {
        return vec![Span::styled(text.to_owned(), base)];
    };
    let chars = text.chars().collect::<Vec<_>>();
    let before: String = chars[..match_start].iter().collect();
    let matched: String = chars[match_start..match_end].iter().collect();
    let after: String = chars[match_end..].iter().collect();
    let mut spans = Vec::new();
    if !before.is_empty() {
        spans.push(Span::styled(before, base));
    }
    spans.push(Span::styled(
        matched,
        base.fg(theme.accent).add_modifier(Modifier::BOLD),
    ));
    if !after.is_empty() {
        spans.push(Span::styled(after, base));
    }
    spans
}

/// Pads `spans` with trailing filler spaces styled with `background` so the
/// rendered line's background covers the full row width, not just the
/// glyphs it draws (`List`/`Paragraph` only paint the background behind the
/// characters a `Span` actually contains). Used for selected-row
/// highlighting so the selection color reads as a full-row bar rather than
/// a text-width smear (SPEC/01_ui.md §6.4).
fn pad_to_width(
    mut spans: Vec<Span<'static>>,
    width: usize,
    background: Color,
) -> Vec<Span<'static>> {
    let used: usize = spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum();
    if used < width {
        spans.push(Span::styled(
            " ".repeat(width - used),
            Style::default().bg(background),
        ));
    }
    spans
}

/// Builds a query-input row: `❯ {query}` (ascii: `> {query}`), prompt glyph
/// in `theme.accent`. Shared by the palette, file picker, path input, text
/// prompt, branch picker, and buffer search overlays (SPEC brief §6:
/// "prompt row starts with accent ❯"). The prompt-plus-space is always 2
/// columns wide in every mode, matching the previous hard-coded `"> "` so
/// existing cursor-column math (`+2`) stays correct.
fn prompt_line(query: &str, theme: &Theme, icon_set: &IconSet, background: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{} ", icon_set.prompt),
            Style::default().fg(theme.accent).bg(background),
        ),
        Span::styled(
            query.to_owned(),
            Style::default().fg(theme.text).bg(background),
        ),
    ])
}

fn render_activity(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let icon_set = icons(state.settings.ui.icon_mode);
    let changed_files = state
        .git_status
        .as_ref()
        .map_or(0, |status| status.files.len());
    let entries = [
        ("E", SidebarView::Explorer, 0usize),
        ("G", SidebarView::SourceControl, changed_files),
        ("S", SidebarView::Search, 0usize),
    ];
    let lines = entries
        .into_iter()
        .map(|(label, view, badge)| {
            let selected = state.sidebar_view == view;
            let bar = if selected && !state.settings.ui.reduced_decoration {
                icon_set.accent_bar
            } else {
                " "
            };
            let badge_text = if badge > 0 {
                format!(" {badge}")
            } else {
                String::new()
            };
            Line::from(vec![
                Span::styled(bar, Style::default().fg(theme.accent).bg(theme.surface)),
                Span::styled(
                    format!("{label} "),
                    Style::default()
                        .fg(if selected {
                            theme.accent
                        } else {
                            theme.text_faint
                        })
                        .bg(theme.surface)
                        .add_modifier(if selected {
                            Modifier::BOLD | Modifier::UNDERLINED
                        } else {
                            Modifier::empty()
                        }),
                ),
                Span::styled(
                    badge_text,
                    Style::default().fg(theme.git_modified).bg(theme.surface),
                ),
            ])
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
        let content = git_sidebar_lines(
            state,
            theme,
            usize::from(area.width.saturating_sub(1)),
            focused,
        );
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
    let icon_mode = state.settings.ui.icon_mode;
    let icon_set = icons(icon_mode);
    // Row width inside the sidebar, minus the 1-column right border drawn
    // by `block`. Used to pad selection backgrounds to a full-row bar.
    let row_width = usize::from(area.width.saturating_sub(1));
    let items = state
        .tree
        .visible_entries()
        .take(max)
        .enumerate()
        .map(|(index, entry)| {
            let expanded = state.tree.is_expanded(&entry.relative_path);
            let name = entry.relative_path.file_name().map_or_else(
                || entry.relative_path.to_string_lossy(),
                |name| name.to_string_lossy(),
            );
            let icon = match entry.kind {
                TreeEntryKind::Directory => folder_icon(expanded, icon_mode),
                TreeEntryKind::File => file_icon(&name, icon_mode),
                TreeEntryKind::Symlink => FileIcon {
                    glyph: icon_set.symlink,
                    color: FileIconColor::Accent,
                },
            };
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
            let selected = index == state.tree_selected;
            let background = if selected {
                if focused {
                    theme.selection
                } else {
                    theme.surface
                }
            } else {
                theme.surface
            };
            let name_color = if entry.kind == TreeEntryKind::Directory || selected {
                theme.text
            } else {
                theme.text_muted
            };
            let base = Style::default().fg(name_color).bg(background).add_modifier(
                if selected && focused {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                },
            );
            let mut spans = Vec::new();
            let bar = if selected { icon_set.accent_bar } else { " " };
            let bar_color = if focused {
                theme.accent
            } else {
                theme.text_faint
            };
            spans.push(Span::styled(
                bar,
                Style::default().fg(bar_color).bg(background),
            ));
            if !state.settings.ui.reduced_decoration && !icon_set.indent_guide.is_empty() {
                for _ in 0..entry.depth {
                    spans.push(Span::styled(
                        format!("{} ", icon_set.indent_guide),
                        Style::default().fg(theme.text_faint).bg(background),
                    ));
                }
            } else {
                spans.push(Span::styled("  ".repeat(entry.depth), base));
            }
            spans.push(file_icon_span(icon, theme, background));
            spans.push(Span::styled(format!(" {name}"), base));
            if let Some(marker) = tree_git_marker(entry, state.git_status.as_ref()) {
                let (symbol, color) = tree_git_marker_glyph(marker, icon_set, theme);
                spans.push(Span::styled(
                    format!(" {symbol}"),
                    Style::default().fg(color).bg(background),
                ));
            }
            if errors > 0 {
                spans.push(Span::styled(
                    format!("  E{errors}"),
                    Style::default().fg(theme.diagnostic_error).bg(background),
                ));
            } else if warnings > 0 {
                spans.push(Span::styled(
                    format!("  W{warnings}"),
                    Style::default().fg(theme.diagnostic_warning).bg(background),
                ));
            }
            ListItem::new(Line::from(pad_to_width(spans, row_width, background)))
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

fn git_sidebar_lines(
    state: &AppState,
    theme: &Theme,
    width: usize,
    focused: bool,
) -> Vec<Line<'static>> {
    let icon_mode = state.settings.ui.icon_mode;
    let icon_set = icons(icon_mode);
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
    let mut branch_spans = vec![
        Span::styled(
            format!(" {} ", icon_set.branch),
            Style::default().fg(theme.accent).bg(theme.surface),
        ),
        Span::styled(
            branch.to_owned(),
            Style::default()
                .fg(theme.accent)
                .bg(theme.surface)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if status.ahead > 0 {
        branch_spans.push(Span::styled(
            format!("  {}{}", icon_set.arrow_up, status.ahead),
            Style::default().fg(theme.git_added).bg(theme.surface),
        ));
    }
    if status.behind > 0 {
        branch_spans.push(Span::styled(
            format!("  {}{}", icon_set.arrow_down, status.behind),
            Style::default().fg(theme.git_modified).bg(theme.surface),
        ));
    }
    let mut lines = vec![Line::from(branch_spans)];
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
        lines.push(section_rule_line(
            section.title(),
            files.len(),
            width,
            theme,
            icon_set,
        ));
        for file in files {
            let marker = crate::git::status_symbol(file);
            let marker_color = git_status_color(file, theme);
            let selected = selection_index == state.git_selected;
            let background = if selected {
                if focused {
                    theme.selection
                } else {
                    theme.surface
                }
            } else {
                theme.surface
            };
            let display = file.path.to_string_lossy();
            let (dirname, basename) = split_dirname_basename(&display);
            let dir_prefix = if dirname.is_empty() {
                String::new()
            } else {
                format!("{dirname}/")
            };
            let icon = file_icon(basename, icon_mode);
            let mut spans = vec![
                Span::styled(
                    if selected { icon_set.accent_bar } else { " " },
                    Style::default()
                        .fg(if focused {
                            theme.accent
                        } else {
                            theme.text_faint
                        })
                        .bg(background),
                ),
                Span::styled(
                    format!(" {marker} "),
                    Style::default().fg(marker_color).bg(background),
                ),
                file_icon_span(icon, theme, background),
                Span::styled(
                    format!(" {dir_prefix}"),
                    Style::default().fg(theme.text_faint).bg(background),
                ),
                Span::styled(
                    basename.to_owned(),
                    Style::default()
                        .fg(if selected {
                            theme.text
                        } else {
                            theme.text_muted
                        })
                        .bg(background)
                        .add_modifier(if selected {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                ),
            ];
            spans = pad_to_width(spans, width, background);
            lines.push(Line::from(spans));
            selection_index = selection_index.saturating_add(1);
        }
    }
    if status.files.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(" No changes"));
    }
    lines
}

/// Builds the `─ STAGED (3) ─────` Source Control section-header pattern:
/// a leading rule glyph, the uppercase title, the count in accent, then
/// rule fill to `width` in `border`/`text_faint`.
fn section_rule_line(
    title: &str,
    count: usize,
    width: usize,
    theme: &Theme,
    icon_set: &IconSet,
) -> Line<'static> {
    let prefix = format!("{} {title} (", icon_set.rule);
    let count_text = count.to_string();
    let used =
        UnicodeWidthStr::width(prefix.as_str()) + UnicodeWidthStr::width(count_text.as_str()) + 2; // ") " suffix before the rule fill
    let fill_width = width.saturating_sub(used).max(1);
    let fill = icon_set.rule.repeat(fill_width);
    Line::from(vec![
        Span::styled(
            prefix,
            Style::default().fg(theme.text_faint).bg(theme.surface),
        ),
        Span::styled(
            count_text,
            Style::default().fg(theme.accent).bg(theme.surface),
        ),
        Span::styled(
            ") ",
            Style::default().fg(theme.text_faint).bg(theme.surface),
        ),
        Span::styled(fill, Style::default().fg(theme.border).bg(theme.surface)),
    ])
}

fn render_editor(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let icon_set = icons(state.settings.ui.icon_mode);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(area);
    let mut titles = state
        .tabs
        .iter()
        .map(|tab| {
            let dirty_style = Style::default().fg(theme.git_modified).bg(theme.surface);
            let text_style = Style::default().fg(theme.text_muted).bg(theme.surface);
            let mut spans = vec![Span::styled(format!(" {}", tab.title()), text_style)];
            if tab.buffer.is_dirty() {
                spans.push(Span::styled(format!(" {}", icon_set.dirty), dirty_style));
            }
            spans.push(Span::styled(format!(" {} ", icon_set.close), text_style));
            Line::from(spans)
        })
        .collect::<Vec<_>>();
    if let Some(diff) = &state.git_diff {
        let name = diff.path.file_name().map_or_else(
            || diff.path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        titles.push(Line::from(vec![
            Span::styled(
                format!(" {name} (Diff) "),
                Style::default().fg(theme.text_muted).bg(theme.surface),
            ),
            Span::styled(
                format!("{} ", icon_set.close),
                Style::default().fg(theme.text_muted).bg(theme.surface),
            ),
        ]));
    }
    let tabs = Tabs::new(if titles.is_empty() {
        vec![Line::from(" Welcome ")]
    } else {
        titles
    })
    .select(if state.git_diff_active {
        state.tabs.len()
    } else {
        state.active_tab.unwrap_or(0)
    })
    .style(Style::default().fg(theme.text_muted).bg(theme.surface))
    .highlight_style(
        Style::default()
            .fg(theme.accent)
            .bg(theme.surface_raised)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
    )
    .divider(Span::styled(
        icon_set.separator,
        Style::default().fg(theme.text_faint),
    ));
    frame.render_widget(tabs, rows[0]);
    if state.git_diff_active {
        render_git_diff_editor(frame, rows[1], state, theme);
        return;
    }
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
                        theme.text
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

fn render_git_diff_editor(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let Some(diff) = &state.git_diff else {
        return;
    };
    let target = match diff.target {
        crate::git::DiffTarget::WorkingTree => "Working Tree ↔ Index",
        crate::git::DiffTarget::Staged => "Index ↔ HEAD",
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!(" {} ", diff.path.display()),
            Style::default()
                .fg(theme.text)
                .bg(theme.surface_raised)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{target}  READ-ONLY"),
            Style::default()
                .fg(theme.text_faint)
                .bg(theme.surface_raised),
        ),
    ])];
    lines.extend(git_diff_lines(
        state,
        theme,
        usize::from(area.height.saturating_sub(1)),
    ));
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().fg(theme.text_muted).bg(theme.background)),
        area,
    );
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
                        .title(" PROBLEMS  OUTPUT  TERMINAL   [Output] ")
                        .borders(Borders::TOP)
                        .border_style(Style::default().fg(theme.border)),
                )
                .style(Style::default().fg(theme.text_muted).bg(theme.surface)),
            area,
        );
    }
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
                    " PROBLEMS  OUTPUT  TERMINAL   [Problems: {filter}] F filter "
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
        crate::diagnostics::DiagnosticSeverity::Information => ("I", theme.diagnostic_info),
        crate::diagnostics::DiagnosticSeverity::Hint => ("H", theme.diagnostic_hint),
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
                        " PROBLEMS  OUTPUT  TERMINAL   [Terminal] {title} [{status}]{scroll} "
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
            let is_hunk_header = line.starts_with("@@ ");
            if is_hunk_header {
                hunk_index = Some(hunk_index.map_or(0usize, |index| index.saturating_add(1)));
            }
            let selected = hunk_index == Some(state.git_hunk_selected);
            let is_added = line.starts_with('+') && !line.starts_with("+++");
            let is_deleted = line.starts_with('-') && !line.starts_with("---");
            let (color, base_background) = if is_hunk_header {
                (theme.accent, theme.surface_raised)
            } else if is_added {
                (theme.git_added, theme.diff_add_bg)
            } else if is_deleted {
                (theme.git_deleted, theme.diff_delete_bg)
            } else {
                (theme.text_muted, theme.surface)
            };
            Line::from(Span::styled(
                line.to_owned(),
                Style::default()
                    .fg(color)
                    .bg(if selected {
                        theme.active_line
                    } else {
                        base_background
                    })
                    .add_modifier(if is_hunk_header {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ))
        })
        .collect()
}

/// Joins pre-built segment spans with a single-cell gap and a `text_faint`
/// separator glyph (SPEC brief §5: "single-cell gaps and text_faint │
/// separators, no powerline triangles"). Empty segments are dropped rather
/// than leaving a dangling separator.
fn join_segments(
    segments: Vec<Vec<Span<'static>>>,
    separator: &str,
    theme: &Theme,
    background: Color,
) -> Vec<Span<'static>> {
    let separator_style = Style::default().fg(theme.text_faint).bg(background);
    let mut spans = Vec::new();
    for segment in segments {
        if segment.is_empty() {
            continue;
        }
        if !spans.is_empty() {
            spans.push(Span::styled(format!(" {separator} "), separator_style));
        }
        spans.extend(segment);
    }
    spans
}

fn render_status(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let background = theme.surface_raised;
    let icon_set = icons(state.settings.ui.icon_mode);
    let base = Style::default().fg(theme.text).bg(background);
    let faint = Style::default().fg(theme.text_faint).bg(background);

    // Left: focus/pane indicator.
    let pane_label = match state.focus {
        Focus::Sidebar => match state.sidebar_view {
            SidebarView::Explorer => "EXPLORER",
            SidebarView::SourceControl => "SOURCE CONTROL",
            SidebarView::Search => "SEARCH",
        },
        Focus::Editor => "EDITOR",
        Focus::BottomPanel => match state.bottom_panel_view {
            BottomPanelView::Problems => "PROBLEMS",
            BottomPanelView::Output => "OUTPUT",
            BottomPanelView::Terminal => "TERMINAL",
        },
        Focus::Overlay => "OVERLAY",
    };
    let pane_segment = vec![Span::styled(
        format!(" {pane_label}"),
        Style::default()
            .fg(theme.accent)
            .bg(background)
            .add_modifier(Modifier::BOLD),
    )];

    // Left: Git branch segment.
    let git_segment = state.git_status.as_ref().map_or_else(Vec::new, |git| {
        let branch = git.branch.as_deref().unwrap_or("detached");
        let mut spans = vec![
            Span::styled(format!("{} ", icon_set.branch), base),
            Span::styled(
                branch.to_owned(),
                Style::default().fg(theme.accent).bg(background),
            ),
        ];
        if git.ahead > 0 {
            spans.push(Span::styled(
                format!(" {}{}", icon_set.arrow_up, git.ahead),
                Style::default().fg(theme.git_added).bg(background),
            ));
        }
        if git.behind > 0 {
            spans.push(Span::styled(
                format!(" {}{}", icon_set.arrow_down, git.behind),
                Style::default().fg(theme.git_modified).bg(background),
            ));
        }
        if !git.files.is_empty() {
            spans.push(Span::styled(
                format!(" {}{}", icon_set.delta, git.files.len()),
                Style::default().fg(theme.git_modified).bg(background),
            ));
        }
        spans
    });

    // Right: diagnostics summary.
    let (errors, warnings) = state.diagnostics.counts();
    let mut diagnostics_segment = Vec::new();
    if errors > 0 {
        diagnostics_segment.push(Span::styled(
            format!("{} {errors}", icon_set.error_icon),
            Style::default().fg(theme.diagnostic_error).bg(background),
        ));
    }
    if warnings > 0 {
        if !diagnostics_segment.is_empty() {
            diagnostics_segment.push(Span::styled("  ", base));
        }
        diagnostics_segment.push(Span::styled(
            format!("{} {warnings}", icon_set.warning_icon),
            Style::default().fg(theme.diagnostic_warning).bg(background),
        ));
    }

    // Right: cursor position, language, encoding, RO marker.
    let (position, language, encoding, read_only) = state.active_tab().map_or_else(
        || (String::new(), String::new(), String::new(), false),
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
            let language = tab
                .buffer
                .path()
                .and_then(|path| state.language_for_path(path))
                .map_or_else(String::new, |(name, _)| name);
            (
                format!("Ln {}, Col {}", line + 1, column + 1),
                language,
                format!("UTF-8 {ending}"),
                tab.buffer.is_read_only(),
            )
        },
    );
    let position_segment = if position.is_empty() {
        Vec::new()
    } else {
        vec![Span::styled(position, faint)]
    };
    let language_segment = if language.is_empty() {
        Vec::new()
    } else {
        vec![Span::styled(language, faint)]
    };
    let encoding_segment = if encoding.is_empty() {
        Vec::new()
    } else {
        vec![Span::styled(encoding, faint)]
    };
    let ro_segment = if read_only {
        vec![Span::styled(
            "RO",
            Style::default()
                .fg(theme.diagnostic_warning)
                .bg(background)
                .add_modifier(Modifier::BOLD),
        )]
    } else {
        Vec::new()
    };

    let left = join_segments(
        vec![pane_segment, git_segment],
        icon_set.separator,
        theme,
        background,
    );
    let right = join_segments(
        vec![
            diagnostics_segment,
            ro_segment,
            language_segment,
            encoding_segment,
            position_segment,
        ],
        icon_set.separator,
        theme,
        background,
    );
    let left_width = left
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum::<usize>();
    let right_width = right
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum::<usize>()
        + 1; // trailing gap before the right edge
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(u16::try_from(left_width).unwrap_or(u16::MAX)),
            Constraint::Min(0),
            Constraint::Length(u16::try_from(right_width).unwrap_or(u16::MAX)),
        ])
        .split(area);
    frame.render_widget(Paragraph::new(Line::from(left)).style(base), columns[0]);
    let message = state.notification.as_deref().unwrap_or("");
    frame.render_widget(
        Paragraph::new(format!(" {message}"))
            .style(Style::default().fg(theme.text_muted).bg(background)),
        columns[1],
    );
    frame.render_widget(Paragraph::new(Line::from(right)).style(base), columns[2]);
}

fn render_palette(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let icon_mode = state.settings.ui.icon_mode;
    let icon_set = icons(icon_mode);
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
    frame.render_widget(overlay_block(" COMMAND PALETTE ", theme, icon_mode), popup);
    frame.render_widget(
        Paragraph::new(prompt_line(
            &state.palette_query,
            theme,
            icon_set,
            theme.selection,
        ))
        .style(Style::default().bg(theme.selection)),
        rows[0],
    );
    let row_width = usize::from(rows[1].width);
    let commands = state
        .palette_commands()
        .into_iter()
        .take(usize::from(rows[1].height))
        .enumerate()
        .map(|(index, command)| {
            let selected = index == 0;
            let background = if selected {
                theme.selection
            } else {
                theme.surface_raised
            };
            let base = Style::default()
                .fg(if selected {
                    theme.text
                } else {
                    theme.text_muted
                })
                .bg(background);
            let bar = if selected { icon_set.accent_bar } else { " " };
            let mut spans = vec![Span::styled(
                bar,
                Style::default().fg(theme.accent).bg(background),
            )];
            spans.extend(substring_highlighted_spans(
                command.title,
                &state.palette_query,
                theme,
                base,
            ));
            spans.push(Span::styled(
                format!("  {}", command.id),
                Style::default().fg(theme.text_faint).bg(background),
            ));
            let spans = pad_to_width(spans, row_width, background);
            ListItem::new(Line::from(spans)).style(base)
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
    let icon_mode = state.settings.ui.icon_mode;
    frame.render_widget(
        Paragraph::new(prompt_line(
            &state.palette_query,
            theme,
            icons(icon_mode),
            theme.surface_raised,
        ))
        .block(overlay_block(title.to_owned(), theme, icon_mode))
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
    let icon_mode = state.settings.ui.icon_mode;
    frame.render_widget(
        Paragraph::new(prompt_line(
            &state.palette_query,
            theme,
            icons(icon_mode),
            theme.surface_raised,
        ))
        .block(overlay_block(title.to_owned(), theme, icon_mode))
        .style(Style::default().fg(theme.text).bg(theme.surface_raised)),
        popup,
    );
}

fn render_git_restore_confirmation(
    frame: &mut Frame,
    area: Rect,
    path: &std::path::Path,
    theme: &Theme,
    icon_mode: IconMode,
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
            .block(overlay_block_bordered(
                " CONFIRM RESTORE ",
                theme.diagnostic_error,
                theme,
                icon_mode,
            ))
            .style(Style::default().fg(theme.text).bg(theme.surface_raised)),
        popup,
    );
}

fn render_hunk_restore_confirmation(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    icon_mode: IconMode,
) {
    let width = area.width.saturating_sub(8).min(68);
    let popup = Rect::new(area.x + (area.width - width) / 2, area.y + 3, width, 6);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(
            "Discard the selected working-tree hunk?\n\nEnter: restore permanently   Esc: cancel",
        )
        .block(overlay_block_bordered(
            " CONFIRM HUNK RESTORE ",
            theme.diagnostic_error,
            theme,
            icon_mode,
        ))
        .style(Style::default().fg(theme.text).bg(theme.surface_raised)),
        popup,
    );
}

fn render_terminal_quit_confirmation(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    icon_mode: IconMode,
) {
    let width = area.width.saturating_sub(8).min(68);
    let popup = Rect::new(area.x + (area.width - width) / 2, area.y + 3, width, 6);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(
            "A process is still running in the integrated terminal.\n\nEnter: terminate and quit   Esc: cancel",
        )
        .block(overlay_block_bordered(
            " TERMINATE TERMINAL? ",
            theme.diagnostic_warning,
            theme,
            icon_mode,
        ))
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
            .block(overlay_block(
                " HOVER  Enter/Esc close ",
                theme,
                state.settings.ui.icon_mode,
            ))
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
    let icon_mode = state.settings.ui.icon_mode;
    let icon_set = icons(icon_mode);
    let row_width = usize::from(popup.width.saturating_sub(2));
    let items = state
        .lsp_completions
        .iter()
        .take(usize::from(height.saturating_sub(2)))
        .enumerate()
        .map(|(index, completion)| {
            let selected = index == state.lsp_completion_selected;
            let background = if selected {
                theme.selection
            } else {
                theme.surface_raised
            };
            let bar = if selected { icon_set.accent_bar } else { " " };
            let mut spans = vec![
                Span::styled(bar, Style::default().fg(theme.accent).bg(background)),
                Span::styled(
                    completion.label.clone(),
                    Style::default()
                        .fg(if selected {
                            theme.text
                        } else {
                            theme.text_muted
                        })
                        .bg(background),
                ),
            ];
            if let Some(detail) = completion.detail.as_deref() {
                spans.push(Span::styled(
                    format!("  {detail}"),
                    Style::default().fg(theme.text_faint).bg(background),
                ));
            }
            ListItem::new(Line::from(pad_to_width(spans, row_width, background)))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(items).block(overlay_block(" COMPLETION ", theme, icon_mode)),
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
    let icon_mode = state.settings.ui.icon_mode;
    let icon_set = icons(icon_mode);
    frame.render_widget(overlay_block(" SWITCH BRANCH ", theme, icon_mode), popup);
    frame.render_widget(
        Paragraph::new(prompt_line(
            &state.palette_query,
            theme,
            icon_set,
            theme.selection,
        ))
        .style(Style::default().bg(theme.selection)),
        rows[0],
    );
    let row_width = usize::from(rows[1].width);
    let branches = state
        .visible_git_branches()
        .into_iter()
        .take(usize::from(rows[1].height))
        .enumerate()
        .map(|(index, branch)| {
            let selected = index == state.git_branch_selected;
            let background = if selected {
                theme.selection
            } else {
                theme.surface_raised
            };
            let bar = if selected { icon_set.accent_bar } else { " " };
            let marker = if branch.current { icon_set.dirty } else { " " };
            let remote = if branch.remote { "  remote" } else { "" };
            let base = Style::default()
                .fg(if selected {
                    theme.text
                } else {
                    theme.text_muted
                })
                .bg(background);
            let spans = vec![
                Span::styled(bar, Style::default().fg(theme.accent).bg(background)),
                Span::styled(
                    format!("{marker} "),
                    Style::default().fg(theme.accent).bg(background),
                ),
                Span::styled(format!("{}{remote}", branch.name), base),
            ];
            ListItem::new(Line::from(pad_to_width(spans, row_width, background))).style(base)
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
    let icon_mode = state.settings.ui.icon_mode;
    let block = overlay_block(format!(" FIND  {current}/{total} "), theme, icon_mode);
    frame.render_widget(
        Paragraph::new(prompt_line(
            &state.palette_query,
            theme,
            icons(icon_mode),
            theme.surface_raised,
        ))
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
    let icon_mode = state.settings.ui.icon_mode;
    let icon_set = icons(icon_mode);
    frame.render_widget(overlay_block(" OPEN FILE ", theme, icon_mode), popup);
    frame.render_widget(
        Paragraph::new(prompt_line(
            &state.palette_query,
            theme,
            icon_set,
            theme.selection,
        ))
        .style(Style::default().bg(theme.selection)),
        rows[0],
    );
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(rows[1]);
    let visible_height = usize::from(columns[0].height);
    let row_width = usize::from(columns[0].width);
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
            let background = if selected {
                theme.selection
            } else {
                theme.surface_raised
            };
            let base = Style::default()
                .fg(if selected {
                    theme.text
                } else {
                    theme.text_muted
                })
                .bg(background);
            let display = matched.path.to_string_lossy().into_owned();
            let (dirname, basename) = split_dirname_basename(&display);
            let icon = file_icon(basename, icon_mode);
            let bar = if selected { icon_set.accent_bar } else { " " };
            let mut spans = vec![Span::styled(
                bar,
                Style::default().fg(theme.accent).bg(background),
            )];
            spans.push(file_icon_span(icon, theme, background));
            spans.push(Span::styled(" ", Style::default().bg(background)));
            if !dirname.is_empty() {
                spans.push(Span::styled(
                    format!("{dirname}/"),
                    Style::default().fg(theme.text_faint).bg(background),
                ));
            }
            spans.extend(highlighted_spans(
                basename,
                &state.palette_query,
                theme,
                base,
            ));
            let spans = pad_to_width(spans, row_width, background);
            ListItem::new(Line::from(spans)).style(base)
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
    icon_mode: IconMode,
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
    let block =
        overlay_block_bordered(" CONFIRM DELETE ", theme.diagnostic_error, theme, icon_mode);
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
    let block = overlay_block_bordered(
        " UNSAVED CHANGES ",
        theme.diagnostic_warning,
        theme,
        state.settings.ui.icon_mode,
    );
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
    icon_mode: IconMode,
) {
    let width = area.width.saturating_sub(8).min(72);
    let popup = Rect::new(area.x + (area.width - width) / 2, area.y + 3, width, 6);
    frame.render_widget(Clear, popup);
    let text = format!(
        "  {} already exists.\n  Replace it with the current buffer?\n\n  Enter: replace    Esc: cancel",
        plan.snapshot.path.display()
    );
    let block = overlay_block_bordered(
        " CONFIRM SAVE AS ",
        theme.diagnostic_warning,
        theme,
        icon_mode,
    );
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
    let block = overlay_block_bordered(
        " CRASH RECOVERY ",
        theme.diagnostic_warning,
        theme,
        state.settings.ui.icon_mode,
    );
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
