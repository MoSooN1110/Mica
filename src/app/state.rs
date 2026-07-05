use std::{
    collections::{HashMap, HashSet},
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
pub enum SidebarView {
    Explorer,
    SourceControl,
    Search,
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
    RecoveryPrompt,
}

#[derive(Debug, Clone)]
pub enum PathAction {
    CreateFile,
    CreateDirectory,
    Move { source: PathBuf },
}

#[derive(Debug)]
pub struct BufferTab {
    pub buffer: TextBuffer,
    pub view: EditorView,
    pub highlights: Vec<HighlightSpan>,
    pub syntax_generation: u64,
}

impl BufferTab {
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
}

impl AppState {
    pub fn new(
        workspace: WorkspaceRoot,
        settings: Settings,
        keymap: Keymap,
        config_warnings: Vec<String>,
        force_read_only: bool,
    ) -> Self {
        Self {
            workspace,
            settings,
            keymap,
            commands: CommandRegistry::built_in(),
            tree: FileTree::default(),
            tree_selected: 0,
            tabs: Vec::new(),
            active_tab: None,
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
        }
    }

    pub fn active_tab(&self) -> Option<&BufferTab> {
        self.active_tab.and_then(|index| self.tabs.get(index))
    }

    pub fn active_path(&self) -> Option<&Path> {
        self.active_tab().and_then(|tab| tab.buffer.path())
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
