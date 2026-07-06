use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicU64},
};

use crate::{
    buffer::TextBuffer,
    command::CommandRegistry,
    config::{Keymap, Settings},
    editor::EditorView,
    editor::HighlightSpan,
    search::FileMatch,
    session::{
        RecoveryBuffer, RecoverySet, RestoredSession, SessionBufferSnapshot, SessionSnapshot,
    },
    workspace::{DeletePlan, FileTree, WorkspaceRoot},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Editor,
    BottomPanel,
    Overlay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BottomPanelView {
    Problems,
    Output,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarView {
    Explorer,
    SourceControl,
    Search,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitSection {
    Staged,
    Conflicted,
    Untracked,
    Working,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceSearchRow {
    File(PathBuf),
    Match(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticRow {
    File(PathBuf),
    Item(usize),
}

impl GitSection {
    pub const ALL: [Self; 4] = [
        Self::Staged,
        Self::Conflicted,
        Self::Untracked,
        Self::Working,
    ];

    pub fn contains(self, file: &crate::git::GitFileChange) -> bool {
        match self {
            Self::Staged => file.staged && !file.conflicted,
            Self::Conflicted => file.conflicted,
            Self::Untracked => file.untracked,
            Self::Working => file.unstaged && !file.conflicted && !file.untracked,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Staged => "STAGED CHANGES",
            Self::Conflicted => "CONFLICTED",
            Self::Untracked => "UNTRACKED",
            Self::Working => "WORKING TREE CHANGES",
        }
    }
}

#[derive(Debug, Clone)]
pub enum Overlay {
    CommandPalette,
    FilePicker,
    BufferSearch {
        tab: usize,
    },
    PathInput(PathAction),
    ConfirmDelete {
        plan: DeletePlan,
        dirty_buffers: usize,
    },
    ConfirmClose {
        tab: usize,
    },
    ConfirmSaveAs {
        plan: crate::app::SaveAsPlan,
    },
    RecoveryPrompt,
    GitCommitInput,
    ConfirmGitRestore {
        path: PathBuf,
    },
    ConfirmGitHunkRestore {
        patch: String,
    },
    GitBranchPicker,
    GitBranchCreate,
    SearchIncludeGlobs,
    SearchExcludeGlobs,
    ConfirmQuitTerminal,
    LspHover,
    LspCompletion,
}

#[derive(Debug, Clone)]
pub struct CompletionCandidate {
    pub label: String,
    pub insert_text: String,
    pub detail: Option<String>,
    /// The range `insert_text` should replace, taken from the completion
    /// item's `textEdit.range` (or `textEdit.insert` for an
    /// `InsertReplaceEdit`). `None` means the server only gave `insertText` /
    /// a plain label, so acceptance falls back to inserting at the cursor.
    /// Positions are LSP (UTF-16) positions; convert with
    /// `crate::lsp::position_to_char_offset` against the *current* buffer
    /// text at accept time, since the buffer may have changed since the
    /// request was sent.
    pub replace_range: Option<(lsp_types::Position, lsp_types::Position)>,
}

/// A request sent to the LSP server, tagged with the file it was made
/// against. `handle_lsp_response` uses this to detect and drop stale
/// responses — ones that arrive after the user has switched to a different
/// tab — for request kinds where applying them to whatever tab happens to
/// be active would be wrong (`Hover`, `Completion`). `Definition` is exempt:
/// it jumps by explicitly opening the target file from the response, so it
/// is safe regardless of which tab is active when the response arrives.
#[derive(Debug, Clone)]
pub enum PendingLspRequest {
    Hover(PathBuf),
    Definition(PathBuf),
    Completion(PathBuf),
}

#[derive(Debug, Clone)]
pub enum PathAction {
    CreateFile,
    CreateDirectory,
    Move { source: PathBuf },
    SaveAs { tab: usize },
}

#[derive(Debug)]
pub struct BufferTab {
    pub buffer: TextBuffer,
    pub view: EditorView,
    pub highlights: Vec<HighlightSpan>,
    pub syntax_generation: u64,
}

impl BufferTab {
    /// Builds a tab around a freshly loaded/created buffer, with an empty
    /// view, no highlights, and syntax generation `0` — the state every new
    /// tab starts in before its first syntax pass runs.
    pub fn new(buffer: TextBuffer) -> Self {
        Self {
            buffer,
            view: Default::default(),
            highlights: Vec::new(),
            syntax_generation: 0,
        }
    }

    pub fn title(&self) -> String {
        self.buffer.path().and_then(Path::file_name).map_or_else(
            || "Untitled".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        )
    }
}

#[derive(Debug)]
pub struct AppState {
    pub workspace: WorkspaceRoot,
    pub settings: Settings,
    pub keymap: Keymap,
    pub commands: CommandRegistry,
    pub tree: FileTree,
    pub tree_selected: usize,
    pub tabs: Vec<BufferTab>,
    pub active_tab: Option<usize>,
    /// The inactive editor group's tab. `None` means the editor is not split.
    pub split_tab: Option<usize>,
    /// The active group is on the right. The active group's tab remains in
    /// `active_tab`; switching groups swaps it with `split_tab`.
    pub split_focus_right: bool,
    pub focus: Focus,
    pub sidebar_view: SidebarView,
    pub sidebar_visible: bool,
    pub bottom_panel_visible: bool,
    pub overlay: Option<Overlay>,
    pub palette_query: String,
    pub notification: Option<String>,
    pub config_warnings: Vec<String>,
    pub should_quit: bool,
    pub terminal_size: (u16, u16),
    pub force_read_only: bool,
    pub saving_tabs: HashSet<usize>,
    pub save_as_tabs: HashSet<usize>,
    pub pending_saves: HashMap<usize, crate::buffer::SaveSnapshot>,
    pub file_matches: Vec<FileMatch>,
    pub file_picker_selected: usize,
    pub file_search_generation: u64,
    pub file_search_cancellation: Arc<AtomicU64>,
    pub file_preview_path: Option<PathBuf>,
    pub file_preview_lines: Vec<String>,
    pub internal_clipboard: String,
    pub buffer_search_cancellation: Arc<AtomicU64>,
    pub syntax_cancellation: Arc<AtomicU64>,
    pub pending_recovery: Vec<RecoveryBuffer>,
    pub recovery_journals: Vec<PathBuf>,
    pub restore_cursors: HashMap<PathBuf, usize>,
    pub preferred_active_path: Option<PathBuf>,
    pub git_status: Option<crate::git::GitStatus>,
    pub git_loading: bool,
    pub git_error: Option<String>,
    pub git_selected: usize,
    pub git_diff: Option<crate::git::FileDiff>,
    /// Whether the virtual, read-only Git diff tab is the active editor tab.
    pub git_diff_active: bool,
    pub git_hunk_selected: usize,
    pub git_diff_scroll: usize,
    pub git_branches: Vec<crate::git::GitBranch>,
    pub git_branch_selected: usize,
    pub workspace_search: crate::search::WorkspaceSearchOptions,
    pub workspace_matches: Vec<crate::search::WorkspaceMatch>,
    pub workspace_search_selected: usize,
    pub workspace_search_generation: u64,
    pub workspace_search_cancellation: Arc<AtomicU64>,
    pub workspace_search_running: bool,
    pub workspace_search_collapsed: HashSet<PathBuf>,
    pub bottom_panel_view: BottomPanelView,
    pub terminal: crate::terminal::TerminalEmulator,
    pub terminal_started: bool,
    pub terminal_running: bool,
    pub terminal_exit: Option<(u32, bool)>,
    pub terminal_scroll_offset: usize,
    pub terminal_generation: u64,
    pub terminal_selection: Option<((usize, usize), (usize, usize))>,
    pub output_lines: VecDeque<String>,
    pub diagnostics: crate::diagnostics::DiagnosticStore,
    pub diagnostic_selected: usize,
    pub diagnostic_filter: Option<crate::diagnostics::DiagnosticSeverity>,
    pub compiler_diagnostic_generation: u64,
    pub lsp_starting: HashSet<String>,
    pub lsp_started: HashSet<String>,
    pub lsp_warned: HashSet<String>,
    pub lsp_versions: HashMap<PathBuf, i32>,
    pub lsp_diagnostic_generation: u64,
    pub lsp_next_request_id: u64,
    pub lsp_pending: HashMap<u64, PendingLspRequest>,
    pub lsp_hover: Vec<String>,
    pub lsp_completions: Vec<CompletionCandidate>,
    pub lsp_completion_selected: usize,
    pub lsp_restarts: HashMap<String, u8>,
}

impl AppState {
    pub fn new(
        workspace: WorkspaceRoot,
        settings: Settings,
        keymap: Keymap,
        config_warnings: Vec<String>,
        force_read_only: bool,
    ) -> Self {
        let terminal_scrollback = settings.terminal.scrollback_lines;
        Self {
            workspace,
            settings,
            keymap,
            commands: CommandRegistry::built_in(),
            tree: FileTree::default(),
            tree_selected: 0,
            tabs: Vec::new(),
            active_tab: None,
            split_tab: None,
            split_focus_right: false,
            focus: Focus::Editor,
            sidebar_view: SidebarView::Explorer,
            sidebar_visible: true,
            bottom_panel_visible: false,
            overlay: None,
            palette_query: String::new(),
            notification: config_warnings.first().cloned(),
            config_warnings,
            should_quit: false,
            terminal_size: (0, 0),
            force_read_only,
            saving_tabs: HashSet::new(),
            save_as_tabs: HashSet::new(),
            pending_saves: HashMap::new(),
            file_matches: Vec::new(),
            file_picker_selected: 0,
            file_search_generation: 0,
            file_search_cancellation: Arc::new(AtomicU64::new(0)),
            file_preview_path: None,
            file_preview_lines: Vec::new(),
            internal_clipboard: String::new(),
            buffer_search_cancellation: Arc::new(AtomicU64::new(0)),
            syntax_cancellation: Arc::new(AtomicU64::new(0)),
            pending_recovery: Vec::new(),
            recovery_journals: Vec::new(),
            restore_cursors: HashMap::new(),
            preferred_active_path: None,
            git_status: None,
            git_loading: false,
            git_error: None,
            git_selected: 0,
            git_diff: None,
            git_diff_active: false,
            git_hunk_selected: 0,
            git_diff_scroll: 0,
            git_branches: Vec::new(),
            git_branch_selected: 0,
            workspace_search: Default::default(),
            workspace_matches: Vec::new(),
            workspace_search_selected: 0,
            workspace_search_generation: 0,
            workspace_search_cancellation: Arc::new(AtomicU64::new(0)),
            workspace_search_running: false,
            workspace_search_collapsed: HashSet::new(),
            bottom_panel_view: BottomPanelView::Output,
            terminal: crate::terminal::TerminalEmulator::new(12, 80, terminal_scrollback),
            terminal_started: false,
            terminal_running: false,
            terminal_exit: None,
            terminal_scroll_offset: 0,
            terminal_generation: 0,
            terminal_selection: None,
            output_lines: VecDeque::new(),
            diagnostics: Default::default(),
            diagnostic_selected: 0,
            diagnostic_filter: None,
            compiler_diagnostic_generation: 0,
            lsp_starting: HashSet::new(),
            lsp_started: HashSet::new(),
            lsp_warned: HashSet::new(),
            lsp_versions: HashMap::new(),
            lsp_diagnostic_generation: 0,
            lsp_next_request_id: 1,
            lsp_pending: HashMap::new(),
            lsp_hover: Vec::new(),
            lsp_completions: Vec::new(),
            lsp_completion_selected: 0,
            lsp_restarts: HashMap::new(),
        }
    }

    pub fn git_entries(&self) -> Vec<(PathBuf, crate::git::DiffTarget)> {
        let Some(status) = &self.git_status else {
            return Vec::new();
        };
        let sections = [
            (GitSection::Staged, crate::git::DiffTarget::Staged),
            (GitSection::Conflicted, crate::git::DiffTarget::WorkingTree),
            (GitSection::Untracked, crate::git::DiffTarget::WorkingTree),
            (GitSection::Working, crate::git::DiffTarget::WorkingTree),
        ];
        sections
            .into_iter()
            .flat_map(|(section, target)| {
                status
                    .files
                    .iter()
                    .filter(move |file| section.contains(file))
                    .map(move |file| (file.path.clone(), target))
            })
            .collect()
    }

    pub fn workspace_search_rows(&self) -> Vec<WorkspaceSearchRow> {
        let mut rows = Vec::new();
        let mut previous: Option<&Path> = None;
        for (index, matched) in self.workspace_matches.iter().enumerate() {
            if previous != Some(matched.path.as_path()) {
                rows.push(WorkspaceSearchRow::File(matched.path.clone()));
                previous = Some(&matched.path);
            }
            if !self.workspace_search_collapsed.contains(&matched.path) {
                rows.push(WorkspaceSearchRow::Match(index));
            }
        }
        rows
    }

    pub fn git_index_at_row(&self, row: usize) -> Option<usize> {
        let status = self.git_status.as_ref()?;
        let mut visual_row = 1usize;
        let mut selection = 0usize;
        for section in GitSection::ALL {
            let count = status
                .files
                .iter()
                .filter(|file| section.contains(file))
                .count();
            if count == 0 {
                continue;
            }
            visual_row = visual_row.saturating_add(2);
            if (visual_row..visual_row + count).contains(&row) {
                return Some(selection + row - visual_row);
            }
            visual_row = visual_row.saturating_add(count);
            selection = selection.saturating_add(count);
        }
        None
    }

    pub fn visible_git_branches(&self) -> Vec<&crate::git::GitBranch> {
        let query = self.palette_query.to_ascii_lowercase();
        self.git_branches
            .iter()
            .filter(|branch| query.is_empty() || branch.name.to_ascii_lowercase().contains(&query))
            .collect()
    }

    pub fn git_hunk_at_diff_line(&self, line: usize) -> Option<usize> {
        let diff = self.git_diff.as_ref()?;
        let mut current: Option<usize> = None;
        for (raw_line, text) in diff.raw.lines().enumerate() {
            if text.starts_with("@@ ") {
                current = Some(current.map_or(0usize, |index| index.saturating_add(1)));
            }
            if raw_line == line {
                return current;
            }
        }
        None
    }

    pub fn active_tab(&self) -> Option<&BufferTab> {
        if self.git_diff_active {
            None
        } else {
            self.active_tab.and_then(|index| self.tabs.get(index))
        }
    }

    pub fn append_output(&mut self, source: &str, message: impl AsRef<str>) {
        for line in message.as_ref().lines() {
            self.output_lines.push_back(format!("[{source}] {line}"));
        }
        while self.output_lines.len() > 2_000 {
            self.output_lines.pop_front();
        }
    }

    pub fn visible_diagnostics(&self) -> Vec<&crate::diagnostics::Diagnostic> {
        self.diagnostics
            .diagnostics()
            .iter()
            .filter(|diagnostic| {
                self.diagnostic_filter
                    .is_none_or(|severity| diagnostic.severity == severity)
            })
            .collect()
    }

    pub fn language_for_path(
        &self,
        path: &Path,
    ) -> Option<(String, crate::config::LanguageSettings)> {
        let extension = path.extension()?.to_str()?;
        self.settings.languages.iter().find_map(|(name, settings)| {
            settings
                .extensions
                .iter()
                .any(|candidate| candidate == extension)
                .then(|| (name.clone(), settings.clone()))
        })
    }

    pub fn diagnostic_rows(&self) -> Vec<DiagnosticRow> {
        let diagnostics = self.visible_diagnostics();
        let mut rows = Vec::new();
        let mut previous: Option<&Path> = None;
        for (index, diagnostic) in diagnostics.iter().enumerate() {
            if previous != Some(diagnostic.file.as_path()) {
                rows.push(DiagnosticRow::File(diagnostic.file.clone()));
                previous = Some(&diagnostic.file);
            }
            rows.push(DiagnosticRow::Item(index));
        }
        rows
    }

    pub fn active_path(&self) -> Option<&Path> {
        self.active_tab().and_then(|tab| tab.buffer.path())
    }

    /// Whether `path` is the active tab's file. Used to discard LSP
    /// responses (hover, completion) that arrive after the user has
    /// switched away from the tab the request was made against.
    pub fn is_active_path(&self, path: &Path) -> bool {
        self.active_path() == Some(path)
    }

    pub fn palette_commands(&self) -> Vec<crate::command::CommandMeta> {
        let query = self.palette_query.to_ascii_lowercase();
        self.commands
            .all()
            .iter()
            .copied()
            .filter(|command| {
                query.is_empty()
                    || command.title.to_ascii_lowercase().contains(&query)
                    || command.id.contains(&query)
            })
            .collect()
    }

    pub fn requested_file(&self, relative: &Path) -> PathBuf {
        self.workspace.as_path().join(relative)
    }

    pub fn offer_recovery(&mut self, recovery: RecoverySet) {
        self.pending_recovery = recovery.buffers;
        self.recovery_journals = recovery.journal_paths;
        if !self.pending_recovery.is_empty() {
            self.overlay = Some(Overlay::RecoveryPrompt);
            self.focus = Focus::Overlay;
        }
    }

    pub fn session_snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            workspace: self.workspace.as_path().to_path_buf(),
            buffers: self
                .tabs
                .iter()
                .map(|tab| SessionBufferSnapshot {
                    path: tab.buffer.path().map(Path::to_path_buf),
                    text: tab.buffer.text().clone(),
                    dirty: tab.buffer.is_dirty(),
                    cursor_char: tab.buffer.selection().head.0,
                    expected_disk_hash: tab.buffer.disk_hash(),
                    has_bom: tab.buffer.has_bom(),
                    line_ending: tab.buffer.line_ending(),
                })
                .collect(),
            active_tab: self.active_tab,
            sidebar_visible: self.sidebar_visible,
            sidebar_view: self.sidebar_view,
            bottom_panel_visible: self.bottom_panel_visible,
            sidebar_width: self.settings.ui.sidebar_width,
            bottom_panel_height: self.settings.ui.bottom_panel_height,
        }
    }

    pub fn apply_restored_session(&mut self, restored: &RestoredSession) {
        self.sidebar_visible = restored.sidebar_visible;
        self.sidebar_view = restored.sidebar_view;
        self.bottom_panel_visible = restored.bottom_panel_visible;
        self.settings.ui.sidebar_width = restored.sidebar_width;
        self.settings.ui.bottom_panel_height = restored.bottom_panel_height;
        self.restore_cursors = restored.buffers.iter().cloned().collect();
        self.preferred_active_path = restored.active_path.clone();
    }
}
