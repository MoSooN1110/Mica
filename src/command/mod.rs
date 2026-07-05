mod id;
mod registry;

use std::path::PathBuf;

pub use id::*;
pub use registry::{CommandMeta, CommandRegistry};

/// UI-independent command payload dispatched by keyboard and mouse paths.
#[derive(Debug, Clone)]
pub enum Command {
    Invoke(String),
    InsertText(String),
    InsertNewline,
    DeleteBackward,
    MoveLeft {
        extend: bool,
    },
    MoveRight {
        extend: bool,
    },
    MoveUp {
        extend: bool,
    },
    MoveDown {
        extend: bool,
    },
    OpenFile(PathBuf),
    SelectTree(usize),
    ToggleTree(usize),
    SelectGit(usize),
    SelectGitAndOpen(usize),
    OpenGitDiff,
    SelectGitHunk(usize),
    GitHunkPrevious,
    GitHunkNext,
    GitHunkStageToggle,
    GitHunkRestore,
    GitHunkOpenFile,
    WorkspaceSearchInput(char),
    WorkspaceSearchBackspace,
    WorkspaceSearchSelect(usize),
    WorkspaceSearchSelectAndOpen(usize),
    WorkspaceSearchOpen,
    WorkspaceSearchToggleCase,
    WorkspaceSearchToggleWord,
    WorkspaceSearchToggleRegex,
    WorkspaceSearchToggleHidden,
    WorkspaceSearchToggleBinary,
    WorkspaceSearchToggleFile(PathBuf),
    TerminalInput(Vec<u8>),
    TerminalPaste(String),
    TerminalScroll(i32),
    FocusTerminal,
    TerminalSetSelection {
        row: usize,
        column: usize,
        extend: bool,
    },
    TerminalCopy,
    DiagnosticSelect(usize),
    DiagnosticOpen,
    DiagnosticCycleFilter,
    SelectTab(usize),
    CloseTab(usize),
    SetCursor {
        char_offset: usize,
        extend: bool,
    },
    SearchNext,
    SearchPrevious,
    RecoveryRecover,
    RecoveryDiscard,
    RecoveryLater,
    PaletteInput(char),
    PaletteBackspace,
    PaletteNewline,
    PaletteAccept,
    Cancel,
    Resize(u16, u16),
}
