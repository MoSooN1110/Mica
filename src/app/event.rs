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
    HighlightRust {
        tab: usize,
        buffer_generation: u64,
        syntax_generation: u64,
        source: ropey::Rope,
        cancellation: std::sync::Arc<std::sync::atomic::AtomicU64>,
    },
    DiscardRecovery(Vec<PathBuf>),
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
