use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Locale {
    #[default]
    Auto,
    En,
    Ja,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IconMode {
    Ascii,
    #[default]
    Unicode,
    NerdFont,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EditorSettings {
    pub tab_width: u8,
    pub insert_spaces: bool,
    pub line_numbers: bool,
    pub active_line_highlight: bool,
    pub mouse: bool,
    pub auto_reload_unmodified: bool,
    pub large_file_threshold_mb: u64,
    pub word_wrap: bool,
    pub trim_trailing_whitespace: bool,
    pub insert_final_newline: bool,
    pub auto_pairs: bool,
    pub ambiguous_width_wide: bool,
    pub format_on_save: bool,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            tab_width: 4,
            insert_spaces: true,
            line_numbers: true,
            active_line_highlight: true,
            mouse: true,
            auto_reload_unmodified: true,
            large_file_threshold_mb: 10,
            word_wrap: false,
            trim_trailing_whitespace: false,
            insert_final_newline: false,
            auto_pairs: true,
            ambiguous_width_wide: false,
            format_on_save: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkspaceSettings {
    pub show_hidden: bool,
    pub follow_symlinks: bool,
    pub respect_gitignore: bool,
}

impl Default for WorkspaceSettings {
    fn default() -> Self {
        Self {
            show_hidden: false,
            follow_symlinks: false,
            respect_gitignore: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GitSettings {
    pub enabled: bool,
    pub auto_refresh: bool,
    pub confirm_destructive_actions: bool,
}

impl Default for GitSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_refresh: true,
            confirm_destructive_actions: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TerminalSettings {
    pub shell: String,
    pub scrollback_lines: usize,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            shell: String::new(),
            scrollback_lines: 10_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DiagnosticsSettings {
    pub enabled: bool,
    pub inline_messages: bool,
    pub check_on_save: bool,
}

impl Default for DiagnosticsSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            inline_messages: false,
            check_on_save: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LspSettings {
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LanguageSettings {
    pub extensions: Vec<String>,
    pub command: String,
    pub args: Vec<String>,
    pub root_markers: Vec<String>,
}

impl Default for LanguageSettings {
    fn default() -> Self {
        Self {
            extensions: Vec::new(),
            command: String::new(),
            args: Vec::new(),
            root_markers: vec![".git".to_owned()],
        }
    }
}

impl Default for LspSettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiSettings {
    pub theme: String,
    pub locale: Locale,
    pub icon_mode: IconMode,
    pub sidebar_width: u16,
    pub bottom_panel_height: u16,
    pub high_contrast: bool,
    pub reduced_decoration: bool,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            theme: "mica-dark".to_owned(),
            locale: Locale::Auto,
            icon_mode: IconMode::Unicode,
            sidebar_width: 30,
            bottom_panel_height: 12,
            high_contrast: false,
            reduced_decoration: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub editor: EditorSettings,
    pub workspace: WorkspaceSettings,
    pub git: GitSettings,
    pub terminal: TerminalSettings,
    pub diagnostics: DiagnosticsSettings,
    pub lsp: LspSettings,
    pub languages: HashMap<String, LanguageSettings>,
    pub ui: UiSettings,
    pub keymap: HashMap<String, String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            editor: EditorSettings::default(),
            workspace: WorkspaceSettings::default(),
            git: GitSettings::default(),
            terminal: TerminalSettings::default(),
            diagnostics: DiagnosticsSettings::default(),
            lsp: LspSettings::default(),
            languages: default_languages(),
            ui: UiSettings::default(),
            keymap: HashMap::new(),
        }
    }
}

fn default_languages() -> HashMap<String, LanguageSettings> {
    [
        (
            "c_cpp",
            vec!["c", "cc", "cpp", "cxx", "h", "hh", "hpp", "hxx"],
            "clangd",
            Vec::new(),
            vec!["compile_commands.json", "CMakeLists.txt", ".git"],
        ),
        (
            "rust",
            vec!["rs"],
            "rust-analyzer",
            Vec::new(),
            vec!["Cargo.toml", ".git"],
        ),
        (
            "python",
            vec!["py", "pyi"],
            "pyright-langserver",
            vec!["--stdio"],
            vec!["pyproject.toml", "setup.py", "requirements.txt", ".git"],
        ),
        (
            "json",
            vec!["json", "jsonc"],
            "vscode-json-language-server",
            vec!["--stdio"],
            vec![".git"],
        ),
        (
            "markdown",
            vec!["md", "markdown"],
            "marksman",
            vec!["server"],
            vec![".git"],
        ),
    ]
    .into_iter()
    .map(|(name, extensions, command, args, markers)| {
        (
            name.to_owned(),
            LanguageSettings {
                extensions: extensions.into_iter().map(str::to_owned).collect(),
                command: command.to_owned(),
                args: args.into_iter().map(str::to_owned).collect(),
                root_markers: markers.into_iter().map(str::to_owned).collect(),
            },
        )
    })
    .collect()
}

#[derive(Debug)]
pub struct ConfigLoad {
    pub settings: Settings,
    pub warnings: Vec<String>,
    pub paths: Vec<PathBuf>,
}

impl ConfigLoad {
    pub fn candidate_paths(
        workspace: &Path,
        explicit: Option<&Path>,
        safe_mode: bool,
    ) -> Vec<PathBuf> {
        if safe_mode {
            return Vec::new();
        }
        if let Some(path) = explicit {
            return vec![path.to_path_buf()];
        }
        let mut paths = Vec::new();
        if let Some(config_home) = config_home() {
            paths.push(config_home.join("mica/config.toml"));
        }
        paths.push(workspace.join(".mica/config.toml"));
        paths
    }

    pub fn load(workspace: &Path, explicit: Option<&Path>, safe_mode: bool) -> Self {
        if safe_mode {
            return Self {
                settings: Settings::default(),
                warnings: Vec::new(),
                paths: Vec::new(),
            };
        }
        let mut settings = Settings::default();
        let mut merged = toml::Value::Table(toml::map::Map::new());
        let mut warnings = Vec::new();
        let paths = Self::candidate_paths(workspace, explicit, safe_mode);
        let mut loaded = Vec::new();
        for path in paths {
            if !path.exists() {
                continue;
            }
            match fs::read_to_string(&path)
                .map_err(|error| error.to_string())
                .and_then(|source| {
                    toml::from_str::<toml::Value>(&source).map_err(|error| error.to_string())
                })
                .and_then(|next| {
                    let mut candidate = merged.clone();
                    merge_value(&mut candidate, next);
                    candidate
                        .clone()
                        .try_into::<Settings>()
                        .map(|settings| (candidate, settings))
                        .map_err(|error| error.to_string())
                }) {
                Ok((next_merged, next_settings)) => {
                    merged = next_merged;
                    settings = next_settings;
                    loaded.push(path);
                }
                Err(error) => warnings.push(format!("{}: {error}", path.display())),
            }
        }
        validate(&mut settings, &mut warnings);
        Self {
            settings,
            warnings,
            paths: loaded,
        }
    }
}

fn merge_value(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base), toml::Value::Table(overlay)) => {
            for (key, value) in overlay {
                match base.get_mut(&key) {
                    Some(existing) => merge_value(existing, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

fn validate(settings: &mut Settings, warnings: &mut Vec<String>) {
    if settings.editor.tab_width == 0 || settings.editor.tab_width > 16 {
        warnings.push("editor.tab_width must be between 1 and 16; using 4".to_owned());
        settings.editor.tab_width = 4;
    }
    settings.ui.sidebar_width = settings.ui.sidebar_width.clamp(16, 80);
    settings.ui.bottom_panel_height = settings.ui.bottom_panel_height.clamp(4, 40);
}

fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_merge_preserves_user_values_and_applies_workspace_overrides() {
        let mut merged: toml::Value =
            toml::from_str("[editor]\ntab_width = 2\n[ui]\nsidebar_width = 40").unwrap();
        let workspace: toml::Value = toml::from_str("[editor]\ninsert_spaces = false").unwrap();
        merge_value(&mut merged, workspace);
        let settings: Settings = merged.try_into().unwrap();
        assert_eq!(settings.editor.tab_width, 2);
        assert!(!settings.editor.insert_spaces);
        assert_eq!(settings.ui.sidebar_width, 40);
    }
}
