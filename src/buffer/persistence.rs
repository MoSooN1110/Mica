use std::{
    collections::hash_map::DefaultHasher,
    fs::{self, OpenOptions},
    hash::{Hash, Hasher},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SaveError {
    #[error("cannot determine parent directory for {0}")]
    MissingParent(PathBuf),
    #[error("failed to save {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("refusing to overwrite externally changed file: {0}")]
    ExternalConflict(PathBuf),
}

/// Atomically saves only if the disk content still matches the snapshot the
/// buffer was opened from. `None` means the file is expected not to exist.
pub fn atomic_save_if_unchanged(
    path: &Path,
    bytes: &[u8],
    expected_disk_hash: Option<u64>,
) -> Result<(), SaveError> {
    let current = match fs::read(path) {
        Ok(bytes) => Some(hash_bytes(&bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(SaveError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if current != expected_disk_hash {
        return Err(SaveError::ExternalConflict(path.to_path_buf()));
    }
    atomic_save(path, bytes)
}

/// Replaces a regular file using write + fsync + rename. Symlinks are written
/// through intentionally so their identity is not destroyed.
pub fn atomic_save(path: &Path, bytes: &[u8]) -> Result<(), SaveError> {
    if replacement_would_break_file_identity(path) {
        return write_through(path, bytes);
    }

    let parent = path
        .parent()
        .ok_or_else(|| SaveError::MissingParent(path.to_path_buf()))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("buffer");
    let temporary = parent.join(format!(".{file_name}.mica-{nonce}.tmp"));

    let result = (|| -> io::Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        let mut file = options.open(&temporary)?;
        if let Ok(metadata) = fs::metadata(path) {
            file.set_permissions(metadata.permissions())?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    })();

    if let Err(source) = result {
        let _ = fs::remove_file(&temporary);
        return Err(SaveError::Io {
            path: path.to_path_buf(),
            source,
        });
    }
    Ok(())
}

fn replacement_would_break_file_identity(path: &Path) -> bool {
    if path.is_symlink() {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(path)
            .map(|metadata| metadata.nlink() > 1)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn write_through(path: &Path, bytes: &[u8]) -> Result<(), SaveError> {
    let mut file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
        .map_err(|source| SaveError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|source| SaveError::Io {
            path: path.to_path_buf(),
            source,
        })
}

pub(crate) fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_file(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("mica-{name}-{}", std::process::id()))
    }

    #[test]
    fn checked_save_refuses_external_change() {
        let path = temporary_file("save-conflict");
        let _ = fs::remove_file(&path);
        fs::write(&path, b"opened").unwrap();
        let expected = Some(hash_bytes(b"opened"));
        fs::write(&path, b"external agent edit").unwrap();
        assert!(matches!(
            atomic_save_if_unchanged(&path, b"local edit", expected),
            Err(SaveError::ExternalConflict(_))
        ));
        assert_eq!(fs::read(&path).unwrap(), b"external agent edit");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn checked_save_creates_expected_missing_file() {
        let path = temporary_file("save-new");
        let _ = fs::remove_file(&path);
        atomic_save_if_unchanged(&path, b"new", None).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        fs::remove_file(path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn save_preserves_hard_link_identity() {
        let path = temporary_file("hard-link");
        let link = temporary_file("hard-link-peer");
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(&link);
        fs::write(&path, b"before").unwrap();
        fs::hard_link(&path, &link).unwrap();
        atomic_save_if_unchanged(&path, b"after", Some(hash_bytes(b"before"))).unwrap();
        assert_eq!(fs::read(&link).unwrap(), b"after");
        fs::remove_file(path).unwrap();
        fs::remove_file(link).unwrap();
    }
}
