use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use thiserror::Error;

const TEMPLATE: &str = r#"# Mica workspace configuration.
# User settings live at ~/.config/mica/config.toml.

[editor]
# Width of a tab in terminal columns (1..16).
tab_width = 4
insert_spaces = true
line_numbers = true
active_line_highlight = true
mouse = true
auto_reload_unmodified = true
large_file_threshold_mb = 10
word_wrap = false
trim_trailing_whitespace = false
insert_final_newline = false
auto_pairs = true
ambiguous_width_wide = false
format_on_save = false

[workspace]
show_hidden = false
follow_symlinks = false
respect_gitignore = true

[git]
enabled = true
auto_refresh = true
confirm_destructive_actions = true

[terminal]
# Empty uses $SHELL, then /bin/bash as fallback.
shell = ""
scrollback_lines = 10000

[diagnostics]
enabled = false
inline_messages = false
check_on_save = false

[lsp]
enabled = false

[ui]
# Built-ins: mica-dark, mica-light. A theme file name is also accepted.
theme = "mica-dark"
locale = "auto"
icon_mode = "unicode"
sidebar_width = 30
bottom_panel_height = 12
high_contrast = false
reduced_decoration = false

[keymap]
# Key chords map to registered command IDs. Examples:
# "ctrl-p" = "workspace.open_file"
# "f1" = "help.keybindings"
"#;

#[derive(Debug, Error)]
pub enum ConfigTemplateError {
    #[error("configuration path is not a regular file: {0}")]
    NotRegular(PathBuf),
    #[error("cannot determine configuration directory for {0}")]
    MissingParent(PathBuf),
    #[error("failed to prepare configuration at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

pub fn config_template() -> &'static str {
    TEMPLATE
}

/// Creates a complete template without ever replacing an existing path.
/// Returns `true` when this call created the file and `false` when a regular
/// file already existed (including a race with another creator).
pub fn prepare_config_template(path: &Path) -> Result<bool, ConfigTemplateError> {
    if path.exists() || path.is_symlink() {
        return validate_existing(path).map(|()| false);
    }
    let parent = path
        .parent()
        .ok_or_else(|| ConfigTemplateError::MissingParent(path.to_path_buf()))?;
    fs::create_dir_all(parent).map_err(|source| ConfigTemplateError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = parent.join(format!(".config.toml.mica-{nonce}.tmp"));
    let result = (|| -> io::Result<bool> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(TEMPLATE.as_bytes())?;
        file.sync_all()?;
        drop(file);
        match fs::hard_link(&temporary, path) {
            Ok(()) => {
                if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
                    let _ = directory.sync_all();
                }
                Ok(true)
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
            Err(error) => Err(error),
        }
    })();
    let _ = fs::remove_file(&temporary);
    let created = result.map_err(|source| ConfigTemplateError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !created {
        validate_existing(path)?;
    }
    Ok(created)
}

fn validate_existing(path: &Path) -> Result<(), ConfigTemplateError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| ConfigTemplateError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_file() {
        Ok(())
    } else {
        Err(ConfigTemplateError::NotRegular(path.to_path_buf()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Settings;

    #[test]
    fn template_is_valid_and_never_overwrites_existing_configuration() {
        let root =
            std::env::temp_dir().join(format!("mica-config-template-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let path = root.join(".mica/config.toml");
        assert!(prepare_config_template(&path).expect("create template"));
        let source = fs::read_to_string(&path).expect("read template");
        let parsed = toml::from_str::<Settings>(&source).expect("parse generated template");
        assert_eq!(parsed.editor.tab_width, 4);

        fs::write(&path, "[editor]\ntab_width = 2\n").expect("replace fixture");
        assert!(!prepare_config_template(&path).expect("keep existing config"));
        assert_eq!(
            fs::read_to_string(&path).expect("read existing config"),
            "[editor]\ntab_width = 2\n"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn template_refuses_symbolic_link_destination() {
        use std::os::unix::fs::symlink;

        let root =
            std::env::temp_dir().join(format!("mica-config-template-link-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".mica")).expect("create config directory");
        let outside = root.join("outside.toml");
        fs::write(&outside, "unchanged").expect("outside fixture");
        let path = root.join(".mica/config.toml");
        symlink(&outside, &path).expect("config symlink");

        assert!(matches!(
            prepare_config_template(&path),
            Err(ConfigTemplateError::NotRegular(_))
        ));
        assert_eq!(
            fs::read_to_string(outside).expect("outside content"),
            "unchanged"
        );
        let _ = fs::remove_dir_all(root);
    }
}
