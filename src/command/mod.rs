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
    MoveLeft { extend: bool },
    MoveRight { extend: bool },
    MoveUp { extend: bool },
    MoveDown { extend: bool },
    OpenFile(PathBuf),
    SelectTree(usize),
    ToggleTree(usize),
    SelectTab(usize),
    CloseTab(usize),
    SetCursor { char_offset: usize, extend: bool },
    SearchNext,
    SearchPrevious,
    RecoveryRecover,
    RecoveryDiscard,
    RecoveryLater,
    PaletteInput(char),
    PaletteBackspace,
    PaletteAccept,
    Cancel,
    Resize(u16, u16),
}
