mod keymap;
mod settings;
mod template;
mod watcher;

pub use keymap::{KeyChord, Keymap, KeymapError};
pub use settings::{
    ConfigLoad, DiagnosticsSettings, EditorSettings, GitSettings, IconMode, LanguageSettings,
    Locale, LspSettings, Settings, TerminalSettings, UiSettings, WorkspaceSettings,
};
pub use template::{ConfigTemplateError, config_template, prepare_config_template};
pub use watcher::{ConfigWatcher, ConfigWatcherError};
