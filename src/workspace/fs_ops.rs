use std::{
    fs::{self, OpenOptions},
    io,
    path::{Path, PathBuf},
};

use thiserror::Error;

use super::{PathError, WorkspacePath, WorkspaceRoot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteKind {
    File,
    EmptyDirectory,
    NonEmptyDirectory,
}

#[derive(Debug, Clone)]
pub struct DeletePlan {
    path: WorkspacePath,
    kind: DeleteKind,
    entry_count: usize,
}

impl DeletePlan {
    pub fn path(&self) -> &WorkspacePath {
        &self.path
    }

    pub fn kind(&self) -> DeleteKind {
        self.kind
    }

    pub fn entry_count(&self) -> usize {
        self.entry_count
    }
}

#[derive(Debug, Error)]
pub enum FsOperationError {
    #[error(transparent)]
    UnsafePath(#[from] PathError),
    #[error("operation on workspace root is forbidden")]
    WorkspaceRoot,
    #[error("path already exists: {0}")]
    AlreadyExists(PathBuf),
    #[error("path does not exist: {0}")]
    Missing(PathBuf),
    #[error("symlink operation requires explicit target confirmation: {0}")]
    SymlinkRequiresConfirmation(PathBuf),
    #[error("delete target changed after confirmation: {0}")]
    DeletePlanStale(PathBuf),
    #[error("file operation failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

#[derive(Debug, Clone)]
pub struct FileOperations {
    root: WorkspaceRoot,
}

impl FileOperations {
    pub fn new(root: WorkspaceRoot) -> Self {
        Self { root }
    }

    pub fn create_file(
        &self,
        relative: impl AsRef<Path>,
    ) -> Result<WorkspacePath, FsOperationError> {
        let path = self.checked_destination(relative)?;
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path.absolute())
            .map_err(|source| FsOperationError::Io {
                path: path.absolute(),
                source,
            })?;
        Ok(path)
    }

    pub fn create_directory(
        &self,
        relative: impl AsRef<Path>,
    ) -> Result<WorkspacePath, FsOperationError> {
        let path = self.checked_destination(relative)?;
        fs::create_dir(path.absolute()).map_err(|source| FsOperationError::Io {
            path: path.absolute(),
            source,
        })?;
        Ok(path)
    }

    /// Moves or renames without overwriting an existing destination.
    pub fn move_path(
        &self,
        source: impl AsRef<Path>,
        destination: impl AsRef<Path>,
    ) -> Result<(WorkspacePath, WorkspacePath), FsOperationError> {
        let source = self.checked_existing(source)?;
        let destination = self.checked_destination(destination)?;
        fs::rename(source.absolute(), destination.absolute()).map_err(|error| {
            FsOperationError::Io {
                path: source.absolute(),
                source: error,
            }
        })?;
        Ok((source, destination))
    }

    /// Creates the immutable description that a confirmation dialog must show
    /// before deletion. Execution revalidates the entry count.
    pub fn inspect_delete(
        &self,
        relative: impl AsRef<Path>,
    ) -> Result<DeletePlan, FsOperationError> {
        let path = self.checked_existing(relative)?;
        let absolute = path.absolute();
        let metadata = fs::metadata(&absolute).map_err(|source| FsOperationError::Io {
            path: absolute.clone(),
            source,
        })?;
        let (kind, entry_count) = if metadata.is_dir() {
            let count = count_directory_entries(&absolute)?;
            let kind = if count == 0 {
                DeleteKind::EmptyDirectory
            } else {
                DeleteKind::NonEmptyDirectory
            };
            (kind, count)
        } else {
            (DeleteKind::File, 0)
        };
        Ok(DeletePlan {
            path,
            kind,
            entry_count,
        })
    }

    /// Executes a previously presented and confirmed plan.
    pub fn execute_delete(&self, plan: DeletePlan) -> Result<(), FsOperationError> {
        let current = self.inspect_delete(plan.path.relative())?;
        if current.kind != plan.kind || current.entry_count != plan.entry_count {
            return Err(FsOperationError::DeletePlanStale(plan.path.absolute()));
        }
        let absolute = plan.path.absolute();
        let result = match plan.kind {
            DeleteKind::File => fs::remove_file(&absolute),
            DeleteKind::EmptyDirectory => fs::remove_dir(&absolute),
            DeleteKind::NonEmptyDirectory => fs::remove_dir_all(&absolute),
        };
        result.map_err(|source| FsOperationError::Io {
            path: absolute,
            source,
        })
    }

    fn checked_existing(
        &self,
        relative: impl AsRef<Path>,
    ) -> Result<WorkspacePath, FsOperationError> {
        let lexical = self.root.resolve_lexical(relative.as_ref())?;
        self.reject_root(&lexical)?;
        match fs::symlink_metadata(&lexical) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(FsOperationError::SymlinkRequiresConfirmation(lexical));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(FsOperationError::Missing(lexical));
            }
            Err(source) => {
                return Err(FsOperationError::Io {
                    path: lexical,
                    source,
                });
            }
        }
        Ok(self.root.resolve(relative)?)
    }

    fn checked_destination(
        &self,
        relative: impl AsRef<Path>,
    ) -> Result<WorkspacePath, FsOperationError> {
        let lexical = self.root.resolve_lexical(relative.as_ref())?;
        self.reject_root(&lexical)?;
        if lexical.exists() || lexical.is_symlink() {
            return Err(FsOperationError::AlreadyExists(lexical));
        }
        let path = self.root.resolve(relative)?;
        let Some(parent) = path.absolute().parent().map(Path::to_path_buf) else {
            return Err(FsOperationError::WorkspaceRoot);
        };
        if !parent.is_dir() {
            return Err(FsOperationError::Missing(parent));
        }
        Ok(path)
    }

    fn reject_root(&self, path: &Path) -> Result<(), FsOperationError> {
        if path == self.root.as_path() {
            Err(FsOperationError::WorkspaceRoot)
        } else {
            Ok(())
        }
    }
}

fn count_directory_entries(path: &Path) -> Result<usize, FsOperationError> {
    let mut count = 0usize;
    let entries = fs::read_dir(path).map_err(|source| FsOperationError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| FsOperationError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        count = count.saturating_add(1);
        let metadata =
            fs::symlink_metadata(entry.path()).map_err(|source| FsOperationError::Io {
                path: entry.path(),
                source,
            })?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            count = count.saturating_add(count_directory_entries(&entry.path())?);
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_directory(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("mica-fs-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn create_move_and_confirm_delete_round_trip() {
        let directory = temporary_directory("round-trip");
        let operations = FileOperations::new(WorkspaceRoot::new(&directory).unwrap());
        operations.create_directory("src").unwrap();
        operations.create_file("src/old.rs").unwrap();
        operations.move_path("src/old.rs", "src/new.rs").unwrap();
        assert!(directory.join("src/new.rs").is_file());
        let plan = operations.inspect_delete("src").unwrap();
        assert_eq!(plan.kind(), DeleteKind::NonEmptyDirectory);
        assert_eq!(plan.entry_count(), 1);
        operations.execute_delete(plan).unwrap();
        assert!(!directory.join("src").exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn stale_recursive_delete_plan_is_refused() {
        let directory = temporary_directory("stale-plan");
        fs::create_dir(directory.join("dir")).unwrap();
        let operations = FileOperations::new(WorkspaceRoot::new(&directory).unwrap());
        let plan = operations.inspect_delete("dir").unwrap();
        fs::write(directory.join("dir/new"), b"new").unwrap();
        assert!(matches!(
            operations.execute_delete(plan),
            Err(FsOperationError::DeletePlanStale(_))
        ));
        assert!(directory.join("dir/new").exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn root_and_parent_escape_are_refused() {
        let directory = temporary_directory("boundary");
        let operations = FileOperations::new(WorkspaceRoot::new(&directory).unwrap());
        assert!(matches!(
            operations.inspect_delete("."),
            Err(FsOperationError::WorkspaceRoot)
        ));
        assert!(matches!(
            operations.create_file("../outside"),
            Err(FsOperationError::UnsafePath(PathError::OutsideWorkspace(_)))
        ));
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_delete_requires_explicit_target_confirmation() {
        use std::os::unix::fs::symlink;

        let directory = temporary_directory("symlink");
        fs::write(directory.join("target"), b"keep").unwrap();
        symlink(directory.join("target"), directory.join("link")).unwrap();
        let operations = FileOperations::new(WorkspaceRoot::new(&directory).unwrap());
        assert!(matches!(
            operations.inspect_delete("link"),
            Err(FsOperationError::SymlinkRequiresConfirmation(_))
        ));
        assert_eq!(fs::read(directory.join("target")).unwrap(), b"keep");
        fs::remove_dir_all(directory).unwrap();
    }
}
