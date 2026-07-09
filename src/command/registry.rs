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
                    id: EDITOR_SAVE_AS,
                    title: "Save As",
                },
                CommandMeta {
                    id: EDITOR_SAVE_ALL,
                    title: "Save All",
                },
                CommandMeta {
                    id: EDITOR_CLOSE,
                    title: "Close Editor",
                },
                CommandMeta {
                    id: EDITOR_TOGGLE_PIN,
                    title: "Editor: Toggle Pin Active Tab",
                },
                CommandMeta {
                    id: EDITOR_MOVE_TAB_LEFT,
                    title: "Editor: Move Tab Left",
                },
                CommandMeta {
                    id: EDITOR_MOVE_TAB_RIGHT,
                    title: "Editor: Move Tab Right",
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
                    id: EDITOR_REPLACE,
                    title: "Replace in Buffer",
                },
                CommandMeta {
                    id: EDITOR_REPLACE_NEXT,
                    title: "Replace Current Match",
                },
                CommandMeta {
                    id: EDITOR_REPLACE_ALL,
                    title: "Replace All Matches",
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
                    id: EDITOR_INDENT,
                    title: "Indent Selected Lines",
                },
                CommandMeta {
                    id: EDITOR_OUTDENT,
                    title: "Outdent Selected Lines",
                },
                CommandMeta {
                    id: EDITOR_TOGGLE_LINE_COMMENT,
                    title: "Toggle Line Comment",
                },
                CommandMeta {
                    id: EDITOR_TOGGLE_LINE_NUMBERS,
                    title: "Editor: Toggle Line Numbers",
                },
                CommandMeta {
                    id: EDITOR_TOGGLE_WORD_WRAP,
                    title: "Editor: Toggle Word Wrap",
                },
                CommandMeta {
                    id: EDITOR_SELECT_ALL,
                    title: "Select All",
                },
                CommandMeta {
                    id: EDITOR_DUPLICATE_LINE,
                    title: "Duplicate Selected Lines",
                },
                CommandMeta {
                    id: EDITOR_MOVE_LINE_UP,
                    title: "Move Selected Lines Up",
                },
                CommandMeta {
                    id: EDITOR_MOVE_LINE_DOWN,
                    title: "Move Selected Lines Down",
                },
                CommandMeta {
                    id: EDITOR_DELETE_LINE,
                    title: "Delete Selected Lines",
                },
                CommandMeta {
                    id: EDITOR_GOTO_LINE,
                    title: "Go to Line",
                },
                CommandMeta {
                    id: EDITOR_NAVIGATE_BACK,
                    title: "Navigate Back",
                },
                CommandMeta {
                    id: EDITOR_NAVIGATE_FORWARD,
                    title: "Navigate Forward",
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
                    id: CONFIG_RELOAD,
                    title: "Configuration: Reload",
                },
                CommandMeta {
                    id: CONFIG_OPEN,
                    title: "Configuration: Open Workspace Settings",
                },
                CommandMeta {
                    id: HELP_KEYBINDINGS,
                    title: "Help: Active Keyboard Shortcuts",
                },
                CommandMeta {
                    id: NOTIFICATIONS_HISTORY,
                    title: "Notifications: Show History",
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
                    id: GIT_REFRESH,
                    title: "Git: Refresh",
                },
                CommandMeta {
                    id: GIT_STAGE,
                    title: "Git: Stage Selected File",
                },
                CommandMeta {
                    id: GIT_UNSTAGE,
                    title: "Git: Unstage Selected File",
                },
                CommandMeta {
                    id: GIT_RESTORE,
                    title: "Git: Restore Selected File",
                },
                CommandMeta {
                    id: GIT_COMMIT,
                    title: "Git: Commit Staged Changes",
                },
                CommandMeta {
                    id: GIT_BRANCH_SWITCH,
                    title: "Git: Switch Branch",
                },
                CommandMeta {
                    id: GIT_BRANCH_CREATE,
                    title: "Git: Create Branch",
                },
                CommandMeta {
                    id: GIT_FETCH,
                    title: "Git: Fetch",
                },
                CommandMeta {
                    id: GIT_PULL,
                    title: "Git: Pull (Fast-forward Only)",
                },
                CommandMeta {
                    id: GIT_PUSH,
                    title: "Git: Push",
                },
                CommandMeta {
                    id: GIT_DIFF_PREVIOUS,
                    title: "Git Diff: Previous Change",
                },
                CommandMeta {
                    id: GIT_DIFF_NEXT,
                    title: "Git Diff: Next Change",
                },
                CommandMeta {
                    id: EDITOR_SPLIT,
                    title: "Editor: Split Right",
                },
                CommandMeta {
                    id: EDITOR_FOCUS_NEXT_GROUP,
                    title: "Editor: Focus Next Group",
                },
                CommandMeta {
                    id: EDITOR_CLOSE_SPLIT,
                    title: "Editor: Close Split",
                },
                CommandMeta {
                    id: EDITOR_SCROLL_UP,
                    title: "Editor: Scroll Up",
                },
                CommandMeta {
                    id: EDITOR_SCROLL_DOWN,
                    title: "Editor: Scroll Down",
                },
                CommandMeta {
                    id: SEARCH_INCLUDE_GLOBS,
                    title: "Search: Set Include Globs",
                },
                CommandMeta {
                    id: SEARCH_EXCLUDE_GLOBS,
                    title: "Search: Set Exclude Globs",
                },
                CommandMeta {
                    id: TERMINAL_TOGGLE,
                    title: "Toggle Integrated Terminal",
                },
                CommandMeta {
                    id: TERMINAL_NEW_SESSION,
                    title: "Terminal: Restart Session",
                },
                CommandMeta {
                    id: TERMINAL_OPEN_REFERENCE,
                    title: "Terminal: Open File Reference at Cursor",
                },
                CommandMeta {
                    id: TERMINAL_SEARCH,
                    title: "Terminal: Find in Scrollback",
                },
                CommandMeta {
                    id: VIEW_OUTPUT,
                    title: "Show Output Panel",
                },
                CommandMeta {
                    id: VIEW_TERMINAL,
                    title: "Show Terminal Panel",
                },
                CommandMeta {
                    id: DIAGNOSTICS_OPEN_PROBLEMS,
                    title: "Show Problems Panel",
                },
                CommandMeta {
                    id: DIAGNOSTICS_REFRESH,
                    title: "Diagnostics: Run Cargo Check",
                },
                CommandMeta {
                    id: DIAGNOSTICS_NEXT,
                    title: "Diagnostics: Go to Next Problem",
                },
                CommandMeta {
                    id: DIAGNOSTICS_PREVIOUS,
                    title: "Diagnostics: Go to Previous Problem",
                },
                CommandMeta {
                    id: DIAGNOSTICS_FILTER_SEVERITY,
                    title: "Diagnostics: Cycle Severity Filter",
                },
                CommandMeta {
                    id: DIAGNOSTICS_FILTER_SOURCE,
                    title: "Diagnostics: Cycle Source Filter",
                },
                CommandMeta {
                    id: DIAGNOSTICS_FILTER_CURRENT_FILE,
                    title: "Diagnostics: Toggle Current File Filter",
                },
                CommandMeta {
                    id: LSP_HOVER,
                    title: "LSP: Hover",
                },
                CommandMeta {
                    id: LSP_DEFINITION,
                    title: "LSP: Go to Definition",
                },
                CommandMeta {
                    id: LSP_COMPLETION,
                    title: "LSP: Completion",
                },
                CommandMeta {
                    id: LSP_REFERENCES,
                    title: "LSP: Find References",
                },
                CommandMeta {
                    id: LSP_FORMAT,
                    title: "LSP: Format Document",
                },
                CommandMeta {
                    id: LSP_SIGNATURE_HELP,
                    title: "LSP: Signature Help",
                },
                CommandMeta {
                    id: LSP_CODE_ACTION,
                    title: "LSP: Code Action",
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

    pub fn title(&self, id: &str) -> Option<&'static str> {
        self.commands
            .iter()
            .find(|command| command.id == id)
            .map(|command| command.title)
    }
}
