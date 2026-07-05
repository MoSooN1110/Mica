use std::path::{Path, PathBuf};

use clap::{Parser, ValueEnum};
use thiserror::Error;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CliLocale {
    En,
    Ja,
}

#[derive(Debug, Parser)]
#[command(name = "mica", version, about = "A fast, terminal-native IDE")]
pub struct Cli {
    #[arg(value_name = "PATH", default_value = ".")]
    pub path: PathBuf,
    #[arg(long)]
    pub line: Option<usize>,
    #[arg(long)]
    pub column: Option<usize>,
    #[arg(long)]
    pub readonly: bool,
    #[arg(long)]
    pub no_mouse: bool,
    #[arg(long)]
    pub config: Option<PathBuf>,
    #[arg(long)]
    pub log_file: Option<PathBuf>,
    #[arg(long, default_value = "info")]
    pub log_level: String,
    #[arg(long, value_enum)]
    pub locale: Option<CliLocale>,
    #[arg(long)]
    pub safe_mode: bool,
}

#[derive(Debug, Clone)]
pub struct LaunchTarget {
    pub workspace: PathBuf,
    pub file: Option<PathBuf>,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

#[derive(Debug, Error)]
pub enum CliError {
    #[error("path does not exist and its parent is unavailable: {0}")]
    InvalidPath(PathBuf),
    #[error("failed to resolve path {path}: {source}")]
    Resolve {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl Cli {
    pub fn resolve_target(&self, current_dir: &Path) -> Result<LaunchTarget, CliError> {
        let supplied = if self.path.is_absolute() {
            self.path.clone()
        } else {
            current_dir.join(&self.path)
        };
        let (path, suffix_line, suffix_column) = split_position_suffix(supplied);
        if path.is_dir() {
            return Ok(LaunchTarget {
                workspace: canonicalize(&path)?,
                file: None,
                line: self.line.or(suffix_line),
                column: self.column.or(suffix_column),
            });
        }
        let parent = path
            .parent()
            .filter(|parent| parent.is_dir())
            .ok_or_else(|| CliError::InvalidPath(path.clone()))?;
        Ok(LaunchTarget {
            workspace: canonicalize(parent)?,
            file: Some(path),
            line: self.line.or(suffix_line),
            column: self.column.or(suffix_column),
        })
    }
}

fn canonicalize(path: &Path) -> Result<PathBuf, CliError> {
    path.canonicalize().map_err(|source| CliError::Resolve {
        path: path.to_path_buf(),
        source,
    })
}

fn split_position_suffix(path: PathBuf) -> (PathBuf, Option<usize>, Option<usize>) {
    if path.exists() {
        return (path, None, None);
    }
    let source = path.to_string_lossy();
    let mut parts = source.rsplitn(3, ':');
    let last = parts.next();
    let second = parts.next();
    let third = parts.next();
    match (
        third,
        second.and_then(|value| value.parse().ok()),
        last.and_then(|value| value.parse().ok()),
    ) {
        (Some(file), Some(line), Some(column)) => (PathBuf::from(file), Some(line), Some(column)),
        _ => {
            let mut parts = source.rsplitn(2, ':');
            let number = parts.next().and_then(|value| value.parse().ok());
            let file = parts.next();
            match (file, number) {
                (Some(file), Some(line)) => (PathBuf::from(file), Some(line), None),
                _ => (path, None, None),
            }
        }
    }
}
