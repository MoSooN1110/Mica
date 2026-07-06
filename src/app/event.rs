use std::ops::Range;
use std::path::PathBuf;

use crate::{
    buffer::{SaveSnapshot, TextBuffer},
    command::Command,
    search::FileMatch,
    workspace::{DeletePlan, FileTree},
};

#[derive(Debug)]
pub enum AppEvent {
    Command(Command),
    FileOpened {
        path: PathBuf,
        line: Option<usize>,
        column: Option<usize>,
        result: Result<TextBuffer, String>,
    },
    TreeLoaded(Result<FileTree, String>),
    SaveCompleted {
        tab: usize,
        snapshot: SaveSnapshot,
        result: Result<(), String>,
    },
    SaveAsInspected(Result<SaveAsInspection, String>),
    SaveAsCompleted {
        plan: SaveAsPlan,
        result: Result<(), String>,
    },
    WorkspaceChanged(Vec<PathBuf>),
    ExternalFilesRead(Vec<ExternalFileRead>),
    DeleteInspected(Result<DeletePlan, String>),
    FileOperationCompleted(Result<FileOperationResult, String>),
    FileSearchCompleted {
        generation: u64,
        matches: Vec<FileMatch>,
    },
    FilePreviewLoaded {
        generation: u64,
        path: PathBuf,
        result: Result<Vec<String>, String>,
    },
    BufferSearchCompleted {
        tab: usize,
        buffer_generation: u64,
        search_generation: u64,
        query: String,
        matches: Vec<Range<usize>>,
    },
    SyntaxHighlighted {
        tab: usize,
        buffer_generation: u64,
        syntax_generation: u64,
        result: Result<Vec<crate::editor::HighlightSpan>, String>,
    },
    RecoveryDiscarded(Result<(), String>),
    GitStatusLoaded(Result<crate::git::GitStatus, String>),
    GitDiffLoaded(Result<crate::git::FileDiff, String>),
    GitOperationCompleted {
        result: Result<String, String>,
        workspace_changed: bool,
    },
    GitBranchesLoaded(Result<Vec<crate::git::GitBranch>, String>),
    WorkspaceSearchBatch {
        generation: u64,
        matches: Vec<crate::search::WorkspaceMatch>,
        done: bool,
        error: Option<String>,
    },
    TerminalStarted {
        generation: u64,
        result: Result<(), String>,
    },
    TerminalOutput {
        generation: u64,
        bytes: Vec<u8>,
    },
    TerminalExited {
        generation: u64,
        code: u32,
        success: bool,
    },
    TerminalError {
        generation: u64,
        error: String,
    },
    DiagnosticsReplaced {
        source: crate::diagnostics::DiagnosticSource,
        generation: u64,
        diagnostics: Vec<crate::diagnostics::Diagnostic>,
    },
    DiagnosticsFailed {
        source: crate::diagnostics::DiagnosticSource,
        generation: u64,
        error: String,
    },
    OutputMessage {
        source: String,
        message: String,
    },
    LspClient {
        language: String,
        event: crate::lsp::LspClientEvent,
    },
}

#[derive(Debug)]
pub struct ExternalFileRead {
    pub tab: usize,
    pub path: PathBuf,
    pub result: Result<Option<TextBuffer>, String>,
}

#[derive(Debug, Clone)]
pub enum Effect {
    OpenFile {
        path: PathBuf,
        read_only: bool,
        line: Option<usize>,
        column: Option<usize>,
    },
    ScanWorkspace,
    Save {
        tab: usize,
        snapshot: SaveSnapshot,
    },
    InspectSaveAs {
        tab: usize,
        destination: PathBuf,
        snapshot: SaveSnapshot,
    },
    SaveAs(SaveAsPlan),
    RefreshOpenFiles {
        files: Vec<(usize, PathBuf)>,
        read_only: bool,
    },
    InspectDelete(PathBuf),
    FileOperation(FileOperationRequest),
    FuzzyFiles {
        generation: u64,
        query: String,
        paths: Vec<PathBuf>,
        cancellation: std::sync::Arc<std::sync::atomic::AtomicU64>,
    },
    PreviewFile {
        generation: u64,
        path: PathBuf,
    },
    CopyToClipboard(String),
    SearchBuffer {
        tab: usize,
        buffer_generation: u64,
        search_generation: u64,
        query: String,
        source: ropey::Rope,
        cancellation: std::sync::Arc<std::sync::atomic::AtomicU64>,
    },
    HighlightSyntax {
        tab: usize,
        buffer_generation: u64,
        syntax_generation: u64,
        language: crate::editor::SyntaxLanguage,
        source: ropey::Rope,
        cancellation: std::sync::Arc<std::sync::atomic::AtomicU64>,
    },
    DiscardRecovery(Vec<PathBuf>),
    RefreshGit,
    LoadGitDiff {
        path: PathBuf,
        target: crate::git::DiffTarget,
    },
    GitOperation(GitOperation),
    LoadGitBranches,
    SearchWorkspace {
        generation: u64,
        options: crate::search::WorkspaceSearchOptions,
        open_buffers: std::collections::HashMap<PathBuf, String>,
        cancellation: std::sync::Arc<std::sync::atomic::AtomicU64>,
    },
    StartTerminal {
        generation: u64,
        shell: Option<PathBuf>,
        cwd: PathBuf,
        rows: u16,
        cols: u16,
    },
    TerminalInput(Vec<u8>),
    ResizeTerminal {
        rows: u16,
        cols: u16,
    },
    StopTerminal,
    RunCargoDiagnostics {
        generation: u64,
    },
    StartLsp {
        language: String,
        settings: crate::config::LanguageSettings,
        workspace: PathBuf,
    },
    SendLsp {
        language: String,
        message: serde_json::Value,
    },
    StopLsp {
        language: String,
    },
}

#[derive(Debug, Clone)]
pub enum GitOperation {
    Stage(PathBuf),
    Unstage(PathBuf),
    Restore(PathBuf),
    StageHunk(String),
    UnstageHunk(String),
    RestoreHunk(String),
    Commit(String),
    SwitchBranch(String),
    CreateBranch(String),
    Fetch,
    Pull,
    Push,
}

#[derive(Debug, Clone)]
pub struct SaveAsPlan {
    pub tab: usize,
    pub snapshot: SaveSnapshot,
}

#[derive(Debug)]
pub enum SaveAsInspection {
    Ready(SaveAsPlan),
    ConfirmOverwrite(SaveAsPlan),
}

#[derive(Debug, Clone)]
pub enum FileOperationRequest {
    CreateFile(PathBuf),
    CreateDirectory(PathBuf),
    Move {
        source: PathBuf,
        destination: PathBuf,
    },
    Delete(DeletePlan),
}

#[derive(Debug)]
pub enum FileOperationResult {
    Created {
        path: PathBuf,
        directory: bool,
    },
    Moved {
        source: PathBuf,
        destination: PathBuf,
    },
    Deleted(PathBuf),
}
