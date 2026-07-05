use super::id::*;

#[derive(Debug, Clone, Copy)]
pub struct CommandMeta {
    pub id: &'static str,
    pub title: &'static str,
}

#[derive(Debug, Default)]
pub struct CommandRegistry {
    commands: Vec<CommandMeta>,
}

impl CommandRegistry {
    pub fn built_in() -> Self {
        Self {
            commands: vec![
                CommandMeta {
                    id: FILE_NEW,
                    title: "New File",
                },
                CommandMeta {
                    id: FILE_NEW_DIRECTORY,
                    title: "New Directory",
                },
                CommandMeta {
                    id: FILE_RENAME,
                    title: "Rename File",
                },
                CommandMeta {
                    id: FILE_MOVE,
                    title: "Move File",
                },
                CommandMeta {
                    id: FILE_DELETE,
                    title: "Delete File",
                },
                CommandMeta {
                    id: EDITOR_SAVE,
                    title: "Save File",
                },
                CommandMeta {
                    id: EDITOR_CLOSE,
                    title: "Close Editor",
                },
                CommandMeta {
                    id: EDITOR_COPY,
                    title: "Copy",
                },
                CommandMeta {
                    id: EDITOR_CUT,
                    title: "Cut",
                },
                CommandMeta {
                    id: EDITOR_PASTE,
                    title: "Paste",
                },
                CommandMeta {
                    id: EDITOR_FIND,
                    title: "Find in Buffer",
                },
                CommandMeta {
                    id: EDITOR_UNDO,
                    title: "Undo",
                },
                CommandMeta {
                    id: EDITOR_REDO,
                    title: "Redo",
                },
                CommandMeta {
                    id: WORKSPACE_OPEN_FILE,
                    title: "Open File",
                },
                CommandMeta {
                    id: WORKSPACE_REFRESH,
                    title: "Refresh Workspace",
                },
                CommandMeta {
                    id: VIEW_TOGGLE_SIDEBAR,
                    title: "Toggle Sidebar",
                },
                CommandMeta {
                    id: VIEW_TOGGLE_BOTTOM_PANEL,
                    title: "Toggle Bottom Panel",
                },
                CommandMeta {
                    id: VIEW_EXPLORER,
                    title: "Show Explorer",
                },
                CommandMeta {
                    id: VIEW_SOURCE_CONTROL,
                    title: "Show Source Control",
                },
                CommandMeta {
                    id: VIEW_SEARCH,
                    title: "Show Search",
                },
                CommandMeta {
                    id: COMMAND_PALETTE_OPEN,
                    title: "Show Command Palette",
                },
                CommandMeta {
                    id: APP_QUIT,
                    title: "Quit Mica",
                },
            ],
        }
    }

    pub fn all(&self) -> &[CommandMeta] {
        &self.commands
    }

    pub fn contains(&self, id: &str) -> bool {
        self.commands.iter().any(|command| command.id == id)
    }
}
