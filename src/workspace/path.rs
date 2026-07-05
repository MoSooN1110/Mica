use std::{
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PathError {
    #[error("workspace root does not exist: {0}")]
    MissingRoot(PathBuf),
    #[error("failed to resolve {path}: {source}")]
    Resolve {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("path escapes workspace: {0}")]
    OutsideWorkspace(PathBuf),
}

#[derive(Debug, Clone)]
pub struct WorkspaceRoot(Arc<PathBuf>);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkspacePath {
    root: Arc<PathBuf>,
    relative: PathBuf,
}

impl WorkspaceRoot {
    pub fn new(path: impl AsRef<Path>) -> Result<Self, PathError> {
        let supplied = path.as_ref();
        let canonical = supplied.canonicalize().map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                PathError::MissingRoot(supplied.to_path_buf())
            } else {
                PathError::Resolve {
                    path: supplied.to_path_buf(),
                    source,
                }
            }
        })?;
        if !canonical.is_dir() {
            return Err(PathError::MissingRoot(canonical));
        }
        Ok(Self(Arc::new(canonical)))
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// Resolves existing paths through symlinks. For a path being created, the
    /// closest existing parent is resolved before the missing suffix is added.
    pub fn resolve(&self, input: impl AsRef<Path>) -> Result<WorkspacePath, PathError> {
        let lexical = self.resolve_lexical(input)?;

        let resolved =
            resolve_with_missing_suffix(&lexical).map_err(|source| PathError::Resolve {
                path: lexical.clone(),
                source,
            })?;
        if !resolved.starts_with(self.as_path()) {
            return Err(PathError::OutsideWorkspace(resolved));
        }
        let relative = resolved
            .strip_prefix(self.as_path())
            .map_err(|_| PathError::OutsideWorkspace(resolved.clone()))?
            .to_path_buf();
        Ok(WorkspacePath {
            root: Arc::clone(&self.0),
            relative,
        })
    }

    pub(crate) fn resolve_lexical(&self, input: impl AsRef<Path>) -> Result<PathBuf, PathError> {
        let input = input.as_ref();
        let joined = if input.is_absolute() {
            input.to_path_buf()
        } else {
            self.0.join(input)
        };
        let lexical = normalize_lexically(&joined);
        if !lexical.starts_with(self.as_path()) {
            return Err(PathError::OutsideWorkspace(joined));
        }
        Ok(lexical)
    }
}

impl WorkspacePath {
    pub fn absolute(&self) -> PathBuf {
        self.root.join(&self.relative)
    }

    pub fn relative(&self) -> &Path {
        &self.relative
    }
}

fn resolve_with_missing_suffix(path: &Path) -> std::io::Result<PathBuf> {
    if path.exists() || path.is_symlink() {
        return path.canonicalize();
    }
    let mut ancestor = path;
    let mut suffix = Vec::new();
    while !ancestor.exists() {
        let Some(name) = ancestor.file_name() else {
            return path.canonicalize();
        };
        suffix.push(name.to_owned());
        ancestor = ancestor.parent().unwrap_or(ancestor);
    }
    let mut resolved = ancestor.canonicalize()?;
    for component in suffix.into_iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn normalize_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temporary_directory(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "mica-{name}-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn rejects_parent_escape() {
        let directory = temporary_directory("path-escape");
        let root = WorkspaceRoot::new(&directory).unwrap();
        assert!(matches!(
            root.resolve("../outside"),
            Err(PathError::OutsideWorkspace(_))
        ));
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape() {
        use std::os::unix::fs::symlink;
        let directory = temporary_directory("symlink-escape");
        let outside = temporary_directory("outside");
        symlink(&outside, directory.join("link")).unwrap();
        let root = WorkspaceRoot::new(&directory).unwrap();
        assert!(matches!(
            root.resolve("link/file"),
            Err(PathError::OutsideWorkspace(_))
        ));
        fs::remove_dir_all(directory).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }
}
