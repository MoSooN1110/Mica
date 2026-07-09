//! Snapshot (rendering) tests for the stable UI components required by
//! `SPEC/10_testing.md` §3.
//!
//! These tests build deterministic `AppState` fixtures directly (struct
//! construction plus the same `DiagnosticStore`/`FileTree`/diff-parsing
//! entry points the real update path uses) and render them onto a
//! `ratatui::backend::TestBackend`. No git/PTY/LSP process is ever spawned.
//!
//! Snapshots capture either the plain text grid (layout-oriented cases) or a
//! compact per-cell style summary (color/focus-oriented cases), as suggested
//! by the task: full styled buffers are not reviewable.
//!
//! Every fixture workspace lives under a *fixed* (non-pid, non-timestamp)
//! temporary directory name so repeated runs are byte-identical. Any
//! absolute path derived from that workspace is redacted to `<WORKSPACE>`
//! before the snapshot assertion, so the snapshot itself never encodes a
//! machine-specific path.

use std::{collections::VecDeque, fs, path::PathBuf};

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use mica::{
    app::{
        AppState, BottomPanelView, BufferTab, Focus, NotificationEntry, NotificationLevel, Overlay,
        SidebarView,
    },
    buffer::TextBuffer,
    command::Command,
    config::{IconMode, Keymap, Settings},
    diagnostics::{Diagnostic, DiagnosticSeverity, DiagnosticSource, TextPosition, TextRange},
    editor::EditorView,
    git::{DiffTarget, FileDiff, GitFileChange, GitFileKind, GitStatus, parse_unified_diff},
    search::WorkspaceMatch,
    ui::{ColorMode, Regions, Theme, command_for_mouse, render},
    workspace::{FileOperations, FileTree, WorkspaceRoot},
};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, layout::Rect};

const MAIN_RS: &str =
    "fn main() {\n    let unused = 1;\n    println!(\"value\");\n    let missing_semi = 2\n}\n";

/// Creates a fixed-name (not pid/time based) workspace directory containing
/// a small, deterministic fixture: a Rust source tree, a nested directory,
/// and a Japanese filename, per the Unicode test-data requirement in
/// SPEC/10_testing.md §4.
fn fixture_workspace(name: &str) -> WorkspaceRoot {
    let root = std::env::temp_dir().join(format!("mica-snapshot-fixture-{name}"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src")).expect("create src dir");
    fs::write(root.join("README.md"), "# Mica\n").expect("write README.md");
    fs::write(root.join("src/main.rs"), MAIN_RS).expect("write src/main.rs");
    fs::write(root.join("src/utils.rs"), "pub fn helper() {}\n").expect("write src/utils.rs");
    fs::write(root.join("設計.md"), "# 設計メモ\n").expect("write Japanese filename fixture");
    WorkspaceRoot::new(&root).expect("canonicalize workspace root")
}

/// Builds an `AppState` over a scanned, partially expanded file tree. Tests
/// that only need a plain state can use this directly; others add tabs,
/// diagnostics, or git state on top.
fn base_state(fixture: &str) -> AppState {
    let workspace = fixture_workspace(fixture);
    let mut tree =
        FileTree::scan(workspace.as_path(), false, false, true).expect("scan workspace tree");
    // Expand "src" (first visible entry: directories sort before files).
    tree.toggle_visible_directory(0);
    let mut state = AppState::new(
        workspace,
        Settings::default(),
        Keymap::default(),
        Vec::new(),
        false,
    );
    state.tree = tree;
    state
}

/// Opens `src/main.rs` as a real (disk-backed) buffer so gutter rendering,
/// diagnostics, and path-based git-diff matching all exercise the same
/// absolute path.
fn open_main_tab(workspace: &WorkspaceRoot) -> BufferTab {
    let path = workspace.as_path().join("src/main.rs");
    let buffer = TextBuffer::open(&path, false).expect("open src/main.rs");
    BufferTab {
        buffer,
        view: EditorView::default(),
        highlights: Vec::new(),
        syntax_generation: 0,
        pinned: false,
    }
}

/// One warning (unused variable) and one error (missing semicolon) on
/// `src/main.rs`, with character offsets computed from the real buffer
/// rope so the underline/gutter math is exact rather than guessed.
fn main_rs_diagnostics(tab: &BufferTab) -> Vec<Diagnostic> {
    let path = tab.buffer.path().expect("tab has a path").to_path_buf();
    let rope = tab.buffer.text();

    let warning_line = 1usize;
    let warning_line_text = rope.line(warning_line).to_string();
    let warning_column = warning_line_text.find("unused").expect("token `unused`");
    let warning_start = rope.line_to_char(warning_line) + warning_column;
    let warning_end = warning_start + "unused".chars().count();

    let error_line = 3usize;
    let error_line_text = rope.line(error_line).to_string();
    let error_column = error_line_text
        .find("missing_semi")
        .expect("token `missing_semi`");
    let error_start = rope.line_to_char(error_line) + error_column;
    let error_end = error_start + "missing_semi".chars().count();

    vec![
        Diagnostic {
            file: path.clone(),
            range: TextRange {
                start: TextPosition {
                    line: warning_line,
                    column: warning_column,
                    char_offset: Some(warning_start),
                },
                end: TextPosition {
                    line: warning_line,
                    column: warning_column + 6,
                    char_offset: Some(warning_end),
                },
            },
            severity: DiagnosticSeverity::Warning,
            message: "unused variable: `unused`".to_owned(),
            source: DiagnosticSource::Compiler,
            code: Some("unused_variables".to_owned()),
            stale: false,
        },
        Diagnostic {
            file: path,
            range: TextRange {
                start: TextPosition {
                    line: error_line,
                    column: error_column,
                    char_offset: Some(error_start),
                },
                end: TextPosition {
                    line: error_line,
                    column: error_column + 12,
                    char_offset: Some(error_end),
                },
            },
            severity: DiagnosticSeverity::Error,
            message: "expected `;`, found `}`".to_owned(),
            source: DiagnosticSource::Compiler,
            code: Some("E0308".to_owned()),
            stale: false,
        },
    ]
}

fn utils_rs_warning(workspace: &WorkspaceRoot) -> Diagnostic {
    Diagnostic {
        file: workspace.as_path().join("src/utils.rs"),
        range: TextRange {
            start: TextPosition {
                line: 0,
                column: 0,
                char_offset: Some(0),
            },
            end: TextPosition {
                line: 0,
                column: 3,
                char_offset: Some(3),
            },
        },
        severity: DiagnosticSeverity::Warning,
        message: "missing documentation".to_owned(),
        source: DiagnosticSource::Linter,
        code: None,
        stale: false,
    }
}

/// A hand-built (not just parsed) unified diff for `src/main.rs` covering
/// all three gutter-marker shapes: a pure addition, a modified line
/// (removal + addition on the same new-line), and a pure removal.
fn main_rs_diff() -> FileDiff {
    let raw = "diff --git a/src/main.rs b/src/main.rs\n\
         --- a/src/main.rs\n\
         +++ b/src/main.rs\n\
         @@ -1,0 +1,1 @@\n\
         +    // banner\n\
         @@ -3,1 +3,1 @@\n\
         -    println!(\"old\");\n\
         +    println!(\"value\");\n\
         @@ -5,1 +5,0 @@\n\
         -    let obsolete = 0;\n"
        .to_owned();
    parse_unified_diff(PathBuf::from("src/main.rs"), DiffTarget::WorkingTree, raw)
}

fn sample_git_status() -> GitStatus {
    GitStatus {
        branch: Some("feature/snapshots".to_owned()),
        detached: false,
        upstream: Some("origin/feature/snapshots".to_owned()),
        ahead: 2,
        behind: 1,
        files: vec![
            GitFileChange {
                path: PathBuf::from("src/utils.rs"),
                original_path: None,
                kind: GitFileKind::Modified,
                index_status: 'M',
                worktree_status: '.',
                staged: true,
                unstaged: false,
                conflicted: false,
                untracked: false,
            },
            GitFileChange {
                path: PathBuf::from("src/main.rs"),
                original_path: None,
                kind: GitFileKind::Modified,
                index_status: '.',
                worktree_status: 'M',
                staged: false,
                unstaged: true,
                conflicted: false,
                untracked: false,
            },
            GitFileChange {
                path: PathBuf::from("src/legacy.rs"),
                original_path: None,
                kind: GitFileKind::Unmerged,
                index_status: 'U',
                worktree_status: 'U',
                staged: false,
                unstaged: true,
                conflicted: true,
                untracked: false,
            },
            GitFileChange {
                path: PathBuf::from("設計.md"),
                original_path: None,
                kind: GitFileKind::Untracked,
                index_status: '?',
                worktree_status: '?',
                staged: false,
                unstaged: true,
                conflicted: false,
                untracked: true,
            },
        ],
    }
}

fn row_text(buffer: &Buffer, area: Rect, row: u16) -> String {
    (area.x..area.right())
        .map(|x| cell_at(buffer, x, area.y + row).symbol())
        .collect()
}

fn cell_at(buffer: &Buffer, x: u16, y: u16) -> &ratatui::buffer::Cell {
    buffer
        .cell((x, y))
        .unwrap_or_else(|| panic!("cell ({x},{y}) out of bounds for {:?}", buffer.area))
}

fn region_text(buffer: &Buffer, area: Rect) -> String {
    (0..area.height)
        .map(|row| row_text(buffer, area, row))
        .collect::<Vec<_>>()
        .join("\n")
}

fn full_text(buffer: &Buffer) -> String {
    region_text(buffer, buffer.area)
}

/// Replaces the workspace's absolute temp-dir prefix with a fixed token so
/// snapshots never encode a machine-specific path.
fn redact(text: &str, workspace: &WorkspaceRoot) -> String {
    text.replace(&workspace.as_path().display().to_string(), "<WORKSPACE>")
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

fn style_line(buffer: &Buffer, x: u16, y: u16) -> String {
    let cell = cell_at(buffer, x, y);
    format!(
        "({x},{y}) symbol={:?} fg={:?} bg={:?} modifier={:?}",
        cell.symbol(),
        cell.fg,
        cell.bg,
        cell.modifier
    )
}

fn draw(
    state: &AppState,
    theme: &Theme,
    width: u16,
    height: u16,
) -> (Regions, Terminal<TestBackend>) {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("build terminal");
    let mut regions = Regions::default();
    terminal
        .draw(|frame| {
            regions = render(frame, state, theme);
        })
        .expect("draw frame");
    (regions, terminal)
}

/// Bullet: ファイルツリー(Git状態・診断バッジ付き) + ステータスバー +
/// 小さい端末サイズ(80x24). The Explorer tree now renders a per-file Git
/// status marker alongside the diagnostic badge (SPEC/03_workspace.md
/// §2.1): `main.rs`/`utils.rs` pin `M` (modified), `設計.md` pins `U`
/// (untracked), and the `src` directory pins `•` because at least one
/// descendant (`main.rs`) has a change (directories do not try to
/// prioritize among heterogeneous descendant states for a single glyph;
/// see `tree_git_marker_glyph` in `src/ui/rendering.rs`).
#[test]
fn file_tree_status_bar_and_editor_at_small_terminal() {
    let mut state = base_state("tree");
    let tab = open_main_tab(&state.workspace);
    let diagnostics = main_rs_diagnostics(&tab);
    state
        .diagnostics
        .replace_source(DiagnosticSource::Compiler, 1, diagnostics);
    state.tabs.push(tab);
    state.active_tab = Some(0);
    state.notification = Some("Saved src/main.rs".to_owned());
    // `src/main.rs` and `src/utils.rs` are both `Modified` (`M`), and
    // `設計.md` is `Untracked` (`U`): confirms the tree row for a changed
    // file shows both the Git status marker and the diagnostic badge.
    state.git_status = Some(sample_git_status());

    let theme = Theme::mica_dark(ColorMode::Ansi256);
    let (_, terminal) = draw(&state, &theme, 80, 24);
    let text = full_text(terminal.backend().buffer());
    insta::assert_snapshot!(redact(&text, &state.workspace));
}

/// Bullet: アイコンモード(ascii / unicode) (SPEC/01_ui.md §6.5,
/// SPEC/10_testing.md §3). Same tree/Git fixture as the Unicode snapshot
/// above, but with `UiSettings::icon_mode` set to `Ascii`: the directory
/// expand marker, file marker, and directory Git-aggregate dot must fall
/// back to plain ASCII (`v`/`-`/`-`), while the per-file Git status
/// letters (`M`/`U`/...) are unchanged, since those are semantic codes
/// rather than decorative icons.
#[test]
fn file_tree_renders_ascii_icon_mode() {
    let mut state = base_state("tree-ascii");
    state.settings.ui.icon_mode = IconMode::Ascii;
    state.git_status = Some(sample_git_status());

    let theme = Theme::mica_dark(ColorMode::Ansi256);
    let (regions, terminal) = draw(&state, &theme, 80, 24);
    let text = region_text(terminal.backend().buffer(), regions.sidebar);
    insta::assert_snapshot!(redact(&text, &state.workspace));
}

/// Bullet: Source Controlビュー、差分表示 (sidebar half of the pair).
#[test]
fn source_control_sidebar_shows_git_sections() {
    let mut state = base_state("git-sidebar");
    state.sidebar_view = SidebarView::SourceControl;
    state.git_status = Some(sample_git_status());
    state.git_selected = 1;

    let theme = Theme::mica_dark(ColorMode::Ansi256);
    let (regions, terminal) = draw(&state, &theme, 100, 30);
    let text = region_text(terminal.backend().buffer(), regions.sidebar);
    insta::assert_snapshot!(redact(&text, &state.workspace));
}

#[test]
fn source_control_mouse_buttons_stage_and_open_commit_input() {
    let mut state = base_state("git-sidebar-mouse");
    state.sidebar_view = SidebarView::SourceControl;
    state.git_status = Some(sample_git_status());
    state.focus = Focus::Sidebar;

    let theme = Theme::mica_dark(ColorMode::Ansi256);
    let (regions, _) = draw(&state, &theme, 100, 30);
    let unstage = command_for_mouse(
        &state,
        regions,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: regions.sidebar.x + 2,
            row: regions.sidebar.y + 5,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(matches!(unstage, Some(Command::UnstageGit(0))));

    let working_index = state
        .git_entries()
        .iter()
        .position(|(path, target)| {
            path == &PathBuf::from("src/main.rs") && *target == DiffTarget::WorkingTree
        })
        .expect("working tree entry");
    let working_row = (0..usize::from(regions.sidebar.height))
        .find(|row| state.git_index_at_row(*row) == Some(working_index))
        .expect("visible working tree row");

    let stage = command_for_mouse(
        &state,
        regions,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: regions.sidebar.x + 2,
            row: regions.sidebar.y + u16::try_from(working_row).expect("row fits"),
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(matches!(stage, Some(Command::StageGit(index)) if index == working_index));

    let commit = command_for_mouse(
        &state,
        regions,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: regions.sidebar.right().saturating_sub(2),
            row: regions.sidebar.y + 2,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(matches!(commit, Some(Command::Invoke(id)) if id == "git.commit"));
}

#[test]
fn search_sidebar_renders_clickable_box_toggles_and_focus_shortcuts() {
    let mut state = base_state("search-sidebar");
    state.sidebar_view = SidebarView::Search;
    state.focus = Focus::Sidebar;
    state.workspace_search.query = "value".to_owned();
    state.workspace_search.regex = true;
    state.workspace_matches = vec![WorkspaceMatch {
        path: PathBuf::from("src/main.rs"),
        line: 3,
        column: 14,
        line_text: "    println!(\"value\");".to_owned(),
        match_start: 14,
        match_end: 19,
    }];

    let theme = Theme::mica_dark(ColorMode::Ansi256);
    let (regions, terminal) = draw(&state, &theme, 100, 30);
    let text = region_text(terminal.backend().buffer(), regions.sidebar);
    insta::assert_snapshot!(redact(&text, &state.workspace));
    let status = region_text(terminal.backend().buffer(), regions.status);
    assert!(status.contains("type ↑↓ Enter"));

    let focus = command_for_mouse(
        &state,
        regions,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: regions.sidebar.x + 5,
            row: regions.sidebar.y + 1,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(matches!(focus, Some(Command::FocusSidebar)));

    let toggle = command_for_mouse(
        &state,
        regions,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: regions.sidebar.x + 18,
            row: regions.sidebar.y + 5,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(matches!(toggle, Some(Command::WorkspaceSearchToggleHidden)));
}

#[test]
fn explorer_mouse_click_moves_selection_and_second_click_opens_file() {
    let mut state = base_state("explorer-mouse");
    state.sidebar_view = SidebarView::Explorer;
    state.focus = Focus::Sidebar;

    let theme = Theme::mica_dark(ColorMode::Ansi256);
    let (regions, _) = draw(&state, &theme, 100, 30);
    let file_row = 2u16;
    let click = command_for_mouse(
        &state,
        regions,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: regions.sidebar.x + 8,
            row: regions.sidebar.y + file_row,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(matches!(click, Some(Command::ClickTree(1))));
    let effects = state.update(mica::app::AppEvent::Command(click.expect("tree click")));
    assert!(effects.is_empty());
    assert_eq!(state.tree_selected, 1);

    let second = Command::ClickTree(1);
    let effects = state.update(mica::app::AppEvent::Command(second));
    assert!(matches!(
        effects.as_slice(),
        [mica::app::Effect::OpenFile { path, .. }]
            if path.ends_with("src/main.rs")
    ));
}

#[test]
fn sidebar_border_drag_resizes_sidebar_width() {
    let mut state = base_state("sidebar-resize");
    let theme = Theme::mica_dark(ColorMode::Ansi256);
    let (regions, _) = draw(&state, &theme, 100, 30);

    let down = command_for_mouse(
        &state,
        regions,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: regions.sidebar.right().saturating_sub(1),
            row: regions.sidebar.y + 4,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(matches!(down, Some(Command::BeginSidebarResize)));
    state.update(mica::app::AppEvent::Command(down.expect("resize begin")));

    let drag = command_for_mouse(
        &state,
        regions,
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: regions.sidebar.x + 39,
            row: regions.sidebar.y + 4,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(matches!(drag, Some(Command::ResizeSidebar(40))));
    state.update(mica::app::AppEvent::Command(drag.expect("resize drag")));
    assert_eq!(state.settings.ui.sidebar_width, 40);

    let up = command_for_mouse(
        &state,
        regions,
        MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: regions.sidebar.x + 39,
            row: regions.sidebar.y + 4,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(matches!(up, Some(Command::EndSidebarResize)));
}

/// Bullet: Source Controlビュー、差分表示 (read-only editor tab half).
#[test]
fn git_diff_panel_renders_unified_hunks() {
    let mut state = base_state("git-diff");
    state.git_diff = Some(main_rs_diff());
    state.git_diff_active = true;
    state.git_hunk_selected = 1;
    state.focus = Focus::Editor;

    let theme = Theme::mica_dark(ColorMode::Ansi256);
    let (regions, terminal) = draw(&state, &theme, 100, 30);
    let text = region_text(terminal.backend().buffer(), regions.editor);
    insta::assert_snapshot!(redact(&text, &state.workspace));
}

#[test]
fn selected_git_hunk_uses_green_background_and_other_hunks_do_not() {
    let mut state = base_state("git-diff-focus");
    state.git_diff = Some(main_rs_diff());
    state.git_diff_active = true;
    state.git_hunk_selected = 1;
    state.focus = Focus::Editor;

    let theme = Theme::mica_dark(ColorMode::TrueColor);
    let (regions, terminal) = draw(&state, &theme, 100, 30);
    let buffer = terminal.backend().buffer();
    let selected_row = (0..regions.editor.height)
        .find(|row| row_text(buffer, regions.editor, *row).contains("println!(\"value\")"))
        .expect("selected hunk row");
    let other_row = (0..regions.editor.height)
        .find(|row| row_text(buffer, regions.editor, *row).contains("// banner"))
        .expect("non-selected hunk row");

    assert_eq!(
        cell_at(buffer, regions.editor.x, regions.editor.y + selected_row).bg,
        theme.diff_add_bg
    );
    assert_eq!(
        cell_at(buffer, regions.editor.x, regions.editor.y + other_row).bg,
        theme.surface
    );
}

#[test]
fn split_editor_groups_render_side_by_side() {
    let mut state = base_state("editor-split");
    state.sidebar_visible = false;
    let main = open_main_tab(&state.workspace);
    let utils_path = state.workspace.as_path().join("src/utils.rs");
    let utils = BufferTab::new(TextBuffer::open(&utils_path, false).expect("open utils.rs"));
    state.tabs.push(main);
    state.tabs.push(utils);
    state.active_tab = Some(0);
    state.split_tab = Some(1);
    state.focus = Focus::Editor;

    let theme = Theme::mica_dark(ColorMode::Ansi256);
    let (regions, terminal) = draw(&state, &theme, 100, 20);
    let text = region_text(terminal.backend().buffer(), regions.editor);
    insta::assert_snapshot!(redact(&text, &state.workspace));
}

#[test]
fn tab_bar_renders_pins_and_diagnostic_badges() {
    let mut state = base_state("tab-pin-badge");
    let mut main = open_main_tab(&state.workspace);
    let diagnostics = main_rs_diagnostics(&main);
    main.pinned = true;
    state.tabs.push(main);
    state.active_tab = Some(0);
    state
        .diagnostics
        .replace_source(DiagnosticSource::Compiler, 1, diagnostics);

    let theme = Theme::mica_dark(ColorMode::Ansi256);
    let (regions, terminal) = draw(&state, &theme, 80, 12);
    let text = region_text(terminal.backend().buffer(), regions.tabs);
    insta::assert_snapshot!(redact(&text, &state.workspace));
}

#[test]
fn editor_mouse_wheel_dispatches_scroll_command() {
    let mut state = base_state("editor-scroll-input");
    state.tabs.push(open_main_tab(&state.workspace));
    state.active_tab = Some(0);
    let theme = Theme::mica_dark(ColorMode::Ansi256);
    let (regions, _) = draw(&state, &theme, 100, 24);
    let command = command_for_mouse(
        &state,
        regions,
        MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: regions.editor.x + 10,
            row: regions.editor.y + 5,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(matches!(
        command,
        Some(mica::command::Command::EditorScroll(3))
    ));
}

#[test]
fn tab_mouse_drag_reorders_and_control_click_pins() {
    let mut state = base_state("tab-drag");
    state.tabs.push(open_main_tab(&state.workspace));
    let utils_path = state.workspace.as_path().join("src/utils.rs");
    state.tabs.push(BufferTab::new(
        TextBuffer::open(&utils_path, false).expect("open utils.rs"),
    ));
    state.active_tab = Some(0);
    let theme = Theme::mica_dark(ColorMode::Ansi256);
    let (regions, _) = draw(&state, &theme, 100, 24);

    let down = command_for_mouse(
        &state,
        regions,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: regions.tabs.x + 2,
            row: regions.tabs.y,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(matches!(down, Some(Command::BeginTabDrag(0))));
    state.update(mica::app::AppEvent::Command(down.expect("tab down")));
    let drag = command_for_mouse(
        &state,
        regions,
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: regions.tabs.x + 16,
            row: regions.tabs.y,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(matches!(drag, Some(Command::ReorderTab { from: 0, to: 1 })));
    state.update(mica::app::AppEvent::Command(drag.expect("tab drag")));
    assert_eq!(state.visual_tab_order(), vec![1, 0]);

    let pin = command_for_mouse(
        &state,
        regions,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: regions.tabs.x + 2,
            row: regions.tabs.y,
            modifiers: KeyModifiers::CONTROL,
        },
    );
    assert!(matches!(pin, Some(Command::TogglePinTab(1))));
}

/// Bullet: Problemsパネル.
#[test]
fn problems_panel_groups_diagnostics_by_file() {
    let mut state = base_state("problems");
    let tab = open_main_tab(&state.workspace);
    let mut diagnostics = main_rs_diagnostics(&tab);
    diagnostics.push(utils_rs_warning(&state.workspace));
    state
        .diagnostics
        .replace_source(DiagnosticSource::Compiler, 1, diagnostics);
    state.bottom_panel_visible = true;
    state.bottom_panel_view = BottomPanelView::Problems;
    state.focus = Focus::BottomPanel;
    state.diagnostic_selected = 1;

    let theme = Theme::mica_dark(ColorMode::Ansi256);
    let (regions, terminal) = draw(&state, &theme, 100, 30);
    let text = region_text(terminal.backend().buffer(), regions.bottom);
    insta::assert_snapshot!(redact(&text, &state.workspace));
}

/// Bullet: エディタガター(行番号・差分・診断マーカー). Exercises every
/// marker shape in one pass: `+` (pure addition), `~` (modified line),
/// `-` (pure removal), `E` / `W` (diagnostics, which take priority over
/// diff markers on their own line).
#[test]
fn editor_gutter_shows_diff_and_diagnostic_markers() {
    let mut state = base_state("editor-gutter");
    state.sidebar_visible = false;
    let tab = open_main_tab(&state.workspace);
    let diagnostics = main_rs_diagnostics(&tab);
    state
        .diagnostics
        .replace_source(DiagnosticSource::Compiler, 1, diagnostics);
    state.git_diff = Some(main_rs_diff());
    state.tabs.push(tab);
    state.active_tab = Some(0);
    state.focus = Focus::Editor;

    let theme = Theme::mica_dark(ColorMode::Ansi256);
    let (regions, terminal) = draw(&state, &theme, 60, 12);
    let text = region_text(terminal.backend().buffer(), regions.editor);
    insta::assert_snapshot!(redact(&text, &state.workspace));
}

#[test]
fn editor_line_numbers_and_word_wrap_can_be_toggled() {
    let mut state = base_state("editor-display-toggles");
    state.settings.editor.line_numbers = false;
    let path = state.workspace.as_path().join("wrap.rs");
    let mut buffer = TextBuffer::empty(Some(path), false);
    buffer
        .insert("long-line-with-enough-characters-to-wrap-when-word-wrap-is-enabled")
        .unwrap();
    state.tabs.push(BufferTab::new(buffer));
    state.active_tab = Some(0);
    state.focus = Focus::Editor;

    let theme = Theme::mica_dark(ColorMode::Ansi256);
    let (regions, terminal) = draw(&state, &theme, 40, 12);
    let text = region_text(terminal.backend().buffer(), regions.editor);
    assert!(!text.contains("  1 long-line"));

    state.update(mica::app::AppEvent::Command(Command::Invoke(
        mica::command::EDITOR_TOGGLE_LINE_NUMBERS.to_owned(),
    )));
    assert!(state.settings.editor.line_numbers);

    state.update(mica::app::AppEvent::Command(Command::Invoke(
        mica::command::EDITOR_TOGGLE_WORD_WRAP.to_owned(),
    )));
    assert!(state.settings.editor.word_wrap);
}

/// Bullet: コマンドパレット、ダイアログ (palette half).
#[test]
fn command_palette_overlay_filters_commands() {
    let mut state = base_state("palette");
    state.overlay = Some(Overlay::CommandPalette);
    state.palette_query = "save".to_owned();
    state.focus = Focus::Overlay;

    let theme = Theme::mica_dark(ColorMode::Ansi256);
    let (_, terminal) = draw(&state, &theme, 80, 24);
    let text = full_text(terminal.backend().buffer());
    insta::assert_snapshot!(redact(&text, &state.workspace));
}

#[test]
fn keybinding_help_lists_active_shortcuts_and_status_opens_it() {
    let mut state = base_state("keybinding-help");
    state.tabs.push(open_main_tab(&state.workspace));
    state.active_tab = Some(0);
    state.focus = Focus::Editor;
    state.help_context = Focus::Editor;
    state.overlay = Some(Overlay::KeybindingHelp);

    let theme = Theme::mica_dark(ColorMode::Ansi256);
    let (regions, terminal) = draw(&state, &theme, 100, 30);
    let text = full_text(terminal.backend().buffer());
    insta::assert_snapshot!(redact(&text, &state.workspace));

    state.overlay = None;
    let command = command_for_mouse(
        &state,
        regions,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: regions.status.right().saturating_sub(2),
            row: regions.status.y,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(matches!(
        command,
        Some(Command::Invoke(id)) if id == "help.keybindings"
    ));
}

#[test]
fn notification_toast_history_and_status_mouse_path_render() {
    let mut state = base_state("notification-history");
    state.notification_history = VecDeque::from([
        NotificationEntry {
            message: "Saved src/main.rs".to_owned(),
            level: NotificationLevel::Info,
            unix_seconds: 3_661,
        },
        NotificationEntry {
            message: "Git push failed".to_owned(),
            level: NotificationLevel::Error,
            unix_seconds: 7_322,
        },
    ]);
    state.notification = Some("Git push failed".to_owned());
    state.notification_context = Focus::Editor;
    state.overlay = Some(Overlay::NotificationHistory);
    state.focus = Focus::Overlay;

    let theme = Theme::mica_dark(ColorMode::Ansi256);
    let (regions, terminal) = draw(&state, &theme, 100, 24);
    let text = full_text(terminal.backend().buffer());
    insta::assert_snapshot!(redact(&text, &state.workspace));

    state.overlay = None;
    state.focus = Focus::Editor;
    let (_, terminal) = draw(&state, &theme, 100, 24);
    assert!(full_text(terminal.backend().buffer()).contains("Git push failed"));
    let command = command_for_mouse(
        &state,
        regions,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: regions.status.right().saturating_sub(15),
            row: regions.status.y,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(matches!(
        command,
        Some(Command::Invoke(id)) if id == "notifications.history"
    ));
}

/// Bullet: コマンドパレット、ダイアログ (confirmation dialog half).
#[test]
fn confirm_delete_dialog_shows_target_and_warning() {
    let mut state = base_state("delete-dialog");
    let operations = FileOperations::new(state.workspace.clone());
    let plan = operations
        .inspect_delete("src/utils.rs")
        .expect("inspect_delete src/utils.rs");
    state.overlay = Some(Overlay::ConfirmDelete {
        plan,
        dirty_buffers: 1,
    });
    state.focus = Focus::Overlay;

    let theme = Theme::mica_dark(ColorMode::Ansi256);
    let (_, terminal) = draw(&state, &theme, 80, 24);
    let text = full_text(terminal.backend().buffer());
    insta::assert_snapshot!(redact(&text, &state.workspace));
}

/// Bullet: フォーカスのアクティブ / 非アクティブ状態. Focus is purely a
/// style change (the sidebar's right border switches between the accent
/// color and the plain border color), so this snapshots per-cell style
/// instead of text, which would be identical either way.
#[test]
fn sidebar_focus_style_toggles_between_active_and_inactive() {
    let state_fixture = "focus";
    let mut state = base_state(state_fixture);
    state.sidebar_view = SidebarView::Explorer;
    let theme = Theme::mica_dark(ColorMode::Ansi256);

    state.focus = Focus::Sidebar;
    let (regions, terminal) = draw(&state, &theme, 80, 24);
    let border_x = regions.sidebar.right() - 1;
    let active = style_line(terminal.backend().buffer(), border_x, regions.sidebar.y + 2);
    insta::assert_snapshot!("sidebar_focus_active", active);

    state.focus = Focus::Editor;
    let (regions, terminal) = draw(&state, &theme, 80, 24);
    let border_x = regions.sidebar.right() - 1;
    let inactive = style_line(terminal.backend().buffer(), border_x, regions.sidebar.y + 2);
    insta::assert_snapshot!("sidebar_focus_inactive", inactive);
}

/// Bullet: 256色フォールバック. Same state, two themes: `TrueColor` must
/// produce `Color::Rgb`, `Ansi256` must produce `Color::Indexed` (the
/// approximation tested numerically in `src/ui/theme.rs`); this snapshot
/// pins down what that looks like on an actual rendered widget (the status
/// bar), which doubles as coverage for the status-bar bullet.
#[test]
fn status_bar_color_mode_fallback_uses_indexed_colors() {
    let state = base_state("color-mode");

    let true_color = Theme::mica_dark(ColorMode::TrueColor);
    let (regions, terminal) = draw(&state, &true_color, 80, 24);
    let rgb = style_line(
        terminal.backend().buffer(),
        regions.status.x + 1,
        regions.status.y,
    );
    insta::assert_snapshot!("status_bar_truecolor", rgb);

    let ansi256 = Theme::mica_dark(ColorMode::Ansi256);
    let (regions, terminal) = draw(&state, &ansi256, 80, 24);
    let indexed = style_line(
        terminal.backend().buffer(),
        regions.status.x + 1,
        regions.status.y,
    );
    insta::assert_snapshot!("status_bar_ansi256", indexed);
}

/// Bullet: ファイルツリー(Nerd Fontアイコンモード) (SPEC/01_ui.md §6.5,
/// visual-refresh brief §1). Same tree/Git fixture as the ASCII snapshot
/// above, but with `UiSettings::icon_mode` set to `NerdFont`: file rows use
/// the Devicons-family codepoints from `icons::file_icon` (`.rs` ->
/// nf-dev-rust, `.md` -> nf-dev-markdown) and directories use the Font
/// Awesome folder glyphs, rather than the Unicode shape glyphs.
#[test]
fn file_tree_renders_nerd_font_icon_mode() {
    let mut state = base_state("tree-nerd-font");
    state.settings.ui.icon_mode = IconMode::NerdFont;
    state.git_status = Some(sample_git_status());

    let theme = Theme::mica_dark(ColorMode::Ansi256);
    let (regions, terminal) = draw(&state, &theme, 80, 24);
    let text = region_text(terminal.backend().buffer(), regions.sidebar);
    insta::assert_snapshot!(redact(&text, &state.workspace));
}

/// Bullet: ステータスバー(診断+Gitセグメント) (visual-refresh brief §5).
/// A wide terminal (120 cols) so every segment — pane label, branch with
/// ahead/behind/changed-count, the notification, the diagnostics summary,
/// language, encoding, and cursor position — fits without the narrow-
/// terminal truncation exercised by
/// `file_tree_status_bar_and_editor_at_small_terminal`.
#[test]
fn status_bar_shows_diagnostics_and_git_segments() {
    let mut state = base_state("status-full");
    let tab = open_main_tab(&state.workspace);
    let diagnostics = main_rs_diagnostics(&tab);
    state
        .diagnostics
        .replace_source(DiagnosticSource::Compiler, 1, diagnostics);
    state.tabs.push(tab);
    state.active_tab = Some(0);
    state.focus = Focus::Editor;
    state.lsp_started.insert("rust".to_owned());
    state.notification = Some("Saved src/main.rs".to_owned());
    state.git_status = Some(sample_git_status());

    let theme = Theme::mica_dark(ColorMode::Ansi256);
    let (regions, terminal) = draw(&state, &theme, 120, 24);
    let text = region_text(terminal.backend().buffer(), regions.status);
    insta::assert_snapshot!(redact(&text, &state.workspace));
}

#[test]
fn word_wrap_renders_continuations_and_mouse_maps_to_visual_row() {
    let mut state = base_state("word-wrap");
    state.sidebar_visible = false;
    state.settings.editor.word_wrap = true;
    let path = state.workspace.as_path().join("wrapped.md");
    let mut buffer = TextBuffer::empty(Some(path), false);
    buffer
        .insert("長い日本語の行とemoji👩‍💻を含むword-wrap-content-that-continues")
        .unwrap();
    state.tabs.push(BufferTab::new(buffer));
    state.active_tab = Some(0);
    state.focus = Focus::Editor;

    let theme = Theme::mica_dark(ColorMode::Ansi256);
    let (regions, terminal) = draw(&state, &theme, 50, 12);
    let text = region_text(terminal.backend().buffer(), regions.editor);
    assert!(text.lines().filter(|line| line.contains('1')).count() >= 1);
    assert!(text.lines().any(|line| line.contains("continues")));

    let command = command_for_mouse(
        &state,
        regions,
        MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: regions.editor.x + 8,
            row: regions.editor.y + 3,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(matches!(
        command,
        Some(Command::SetCursor { char_offset, .. }) if char_offset > 0
    ));
}
