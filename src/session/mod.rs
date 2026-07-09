use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::mpsc,
    thread::{self, JoinHandle},
};

use ropey::Rope;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    app::SidebarView,
    buffer::{LineEnding, atomic_save},
};

const SESSION_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct SessionBufferSnapshot {
    pub path: Option<PathBuf>,
    pub text: Rope,
    pub dirty: bool,
    pub cursor_char: usize,
    pub expected_disk_hash: Option<u64>,
    pub has_bom: bool,
    pub line_ending: LineEnding,
    pub pinned: bool,
}

#[derive(Debug, Clone)]
pub struct SessionSnapshot {
    pub workspace: PathBuf,
    pub buffers: Vec<SessionBufferSnapshot>,
    pub active_tab: Option<usize>,
    pub sidebar_visible: bool,
    pub sidebar_view: SidebarView,
    pub bottom_panel_visible: bool,
    pub sidebar_width: u16,
    pub bottom_panel_height: u16,
}

#[derive(Debug, Clone)]
pub struct RecoveryBuffer {
    pub path: Option<PathBuf>,
    pub text: String,
    pub cursor_char: usize,
    pub expected_disk_hash: Option<u64>,
    pub has_bom: bool,
    pub line_ending: LineEnding,
}

#[derive(Debug)]
pub struct RecoverySet {
    pub buffers: Vec<RecoveryBuffer>,
    pub journal_paths: Vec<PathBuf>,
}

#[derive(Debug)]
pub struct RestoredSession {
    pub buffers: Vec<RestoredBuffer>,
    pub active_path: Option<PathBuf>,
    pub sidebar_visible: bool,
    pub sidebar_view: SidebarView,
    pub bottom_panel_visible: bool,
    pub sidebar_width: u16,
    pub bottom_panel_height: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoredBuffer {
    pub path: PathBuf,
    pub cursor_char: usize,
    pub pinned: bool,
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("cannot determine XDG state directory")]
    MissingStateHome,
    #[error("session I/O failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid session data in {path}: {source}")]
    Invalid {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Debug, Clone)]
pub struct SessionStore {
    directory: PathBuf,
    current_journal: PathBuf,
    session_file: PathBuf,
    workspace: PathBuf,
}

impl SessionStore {
    pub fn for_workspace(workspace: &Path) -> Result<Self, SessionError> {
        let state_home = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
            })
            .ok_or(SessionError::MissingStateHome)?;
        Self::at_state_home(workspace, &state_home)
    }

    pub fn at_state_home(workspace: &Path, state_home: &Path) -> Result<Self, SessionError> {
        let mut hasher = DefaultHasher::new();
        workspace.hash(&mut hasher);
        let directory = state_home
            .join("mica/workspaces")
            .join(format!("{:016x}", hasher.finish()));
        fs::create_dir_all(&directory).map_err(|source| SessionError::Io {
            path: directory.clone(),
            source,
        })?;
        Ok(Self {
            current_journal: directory.join(format!("journal-{}.json", std::process::id())),
            session_file: directory.join("session.json"),
            directory,
            workspace: workspace.to_path_buf(),
        })
    }

    pub fn discover_recovery(&self) -> Result<RecoverySet, SessionError> {
        let mut buffers = Vec::new();
        let mut journal_paths = Vec::new();
        let entries = fs::read_dir(&self.directory).map_err(|source| SessionError::Io {
            path: self.directory.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| SessionError::Io {
                path: self.directory.clone(),
                source,
            })?;
            let path = entry.path();
            let Some(pid) = journal_pid(&path) else {
                continue;
            };
            if pid != std::process::id() && process_is_alive(pid) {
                continue;
            }
            let bytes = fs::read(&path).map_err(|source| SessionError::Io {
                path: path.clone(),
                source,
            })?;
            let journal: StoredSession =
                serde_json::from_slice(&bytes).map_err(|source| SessionError::Invalid {
                    path: path.clone(),
                    source,
                })?;
            if journal.version != SESSION_VERSION {
                continue;
            }
            if journal.workspace != self.workspace {
                continue;
            }
            buffers.extend(journal.buffers.into_iter().filter_map(|buffer| {
                Some(RecoveryBuffer {
                    path: buffer.path,
                    text: buffer.unsaved_text?,
                    cursor_char: buffer.cursor_char,
                    expected_disk_hash: buffer.expected_disk_hash,
                    has_bom: buffer.has_bom,
                    line_ending: buffer.line_ending.into(),
                })
            }));
            journal_paths.push(path);
        }
        Ok(RecoverySet {
            buffers,
            journal_paths,
        })
    }

    pub fn load_session(&self) -> Result<Option<RestoredSession>, SessionError> {
        let bytes = match fs::read(&self.session_file) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(SessionError::Io {
                    path: self.session_file.clone(),
                    source,
                });
            }
        };
        let stored: StoredSession =
            serde_json::from_slice(&bytes).map_err(|source| SessionError::Invalid {
                path: self.session_file.clone(),
                source,
            })?;
        if stored.version != SESSION_VERSION {
            return Ok(None);
        }
        if stored.workspace != self.workspace {
            return Ok(None);
        }
        let active_path = stored
            .active_tab
            .and_then(|index| stored.buffers.get(index))
            .and_then(|buffer| buffer.path.clone());
        Ok(Some(RestoredSession {
            buffers: stored
                .buffers
                .into_iter()
                .filter_map(|buffer| {
                    Some(RestoredBuffer {
                        path: buffer.path?,
                        cursor_char: buffer.cursor_char,
                        pinned: buffer.pinned,
                    })
                })
                .collect(),
            active_path,
            sidebar_visible: stored.sidebar_visible,
            sidebar_view: match stored.sidebar_view {
                StoredSidebar::Explorer => SidebarView::Explorer,
                StoredSidebar::SourceControl => SidebarView::SourceControl,
                StoredSidebar::Search => SidebarView::Search,
            },
            bottom_panel_visible: stored.bottom_panel_visible,
            sidebar_width: stored.sidebar_width,
            bottom_panel_height: stored.bottom_panel_height,
        }))
    }

    pub fn current_journal(&self) -> &Path {
        &self.current_journal
    }
}

pub struct SessionJournal {
    sender: Option<mpsc::Sender<JournalMessage>>,
    worker: Option<JoinHandle<()>>,
}

enum JournalMessage {
    Write(SessionSnapshot),
    Shutdown(SessionSnapshot),
}

impl SessionJournal {
    pub fn start(store: SessionStore) -> Self {
        let (sender, receiver) = mpsc::channel();
        let worker = thread::spawn(move || {
            while let Ok(message) = receiver.recv() {
                match message {
                    JournalMessage::Write(mut snapshot) => {
                        for newer in receiver.try_iter() {
                            match newer {
                                JournalMessage::Write(next) => snapshot = next,
                                JournalMessage::Shutdown(final_snapshot) => {
                                    let _ = write_stored(&store.session_file, final_snapshot);
                                    let _ = fs::remove_file(&store.current_journal);
                                    return;
                                }
                            }
                        }
                        let _ = write_stored(&store.current_journal, snapshot);
                    }
                    JournalMessage::Shutdown(snapshot) => {
                        let _ = write_stored(&store.session_file, snapshot);
                        let _ = fs::remove_file(&store.current_journal);
                        return;
                    }
                }
            }
        });
        Self {
            sender: Some(sender),
            worker: Some(worker),
        }
    }

    pub fn submit(&self, snapshot: SessionSnapshot) {
        if let Some(sender) = &self.sender {
            let _ = sender.send(JournalMessage::Write(snapshot));
        }
    }

    pub fn shutdown(mut self, snapshot: SessionSnapshot) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(JournalMessage::Shutdown(snapshot));
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for SessionJournal {
    fn drop(&mut self) {
        // Dropping without Shutdown intentionally leaves the journal behind.
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub fn discard_recovery(paths: &[PathBuf]) -> Result<(), SessionError> {
    for path in paths {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(SessionError::Io {
                    path: path.clone(),
                    source,
                });
            }
        }
    }
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredSession {
    version: u32,
    workspace: PathBuf,
    buffers: Vec<StoredBuffer>,
    active_tab: Option<usize>,
    sidebar_visible: bool,
    sidebar_view: StoredSidebar,
    bottom_panel_visible: bool,
    sidebar_width: u16,
    bottom_panel_height: u16,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredBuffer {
    path: Option<PathBuf>,
    unsaved_text: Option<String>,
    cursor_char: usize,
    expected_disk_hash: Option<u64>,
    has_bom: bool,
    line_ending: StoredLineEnding,
    #[serde(default)]
    pinned: bool,
}

#[derive(Debug, Serialize, Deserialize)]
enum StoredSidebar {
    Explorer,
    SourceControl,
    Search,
}

#[derive(Debug, Serialize, Deserialize)]
enum StoredLineEnding {
    Lf,
    CrLf,
    Mixed,
}

impl From<LineEnding> for StoredLineEnding {
    fn from(value: LineEnding) -> Self {
        match value {
            LineEnding::Lf => Self::Lf,
            LineEnding::CrLf => Self::CrLf,
            LineEnding::Mixed => Self::Mixed,
        }
    }
}

impl From<StoredLineEnding> for LineEnding {
    fn from(value: StoredLineEnding) -> Self {
        match value {
            StoredLineEnding::Lf => Self::Lf,
            StoredLineEnding::CrLf => Self::CrLf,
            StoredLineEnding::Mixed => Self::Mixed,
        }
    }
}

fn write_stored(path: &Path, snapshot: SessionSnapshot) -> Result<(), SessionError> {
    let stored = StoredSession {
        version: SESSION_VERSION,
        workspace: snapshot.workspace,
        buffers: snapshot
            .buffers
            .into_iter()
            .map(|buffer| StoredBuffer {
                path: buffer.path,
                unsaved_text: buffer.dirty.then(|| buffer.text.to_string()),
                cursor_char: buffer.cursor_char,
                expected_disk_hash: buffer.expected_disk_hash,
                has_bom: buffer.has_bom,
                line_ending: buffer.line_ending.into(),
                pinned: buffer.pinned,
            })
            .collect(),
        active_tab: snapshot.active_tab,
        sidebar_visible: snapshot.sidebar_visible,
        sidebar_view: match snapshot.sidebar_view {
            SidebarView::Explorer => StoredSidebar::Explorer,
            SidebarView::SourceControl => StoredSidebar::SourceControl,
            SidebarView::Search => StoredSidebar::Search,
        },
        bottom_panel_visible: snapshot.bottom_panel_visible,
        sidebar_width: snapshot.sidebar_width,
        bottom_panel_height: snapshot.bottom_panel_height,
    };
    let bytes = serde_json::to_vec(&stored).map_err(|source| SessionError::Invalid {
        path: path.to_path_buf(),
        source,
    })?;
    atomic_save(path, &bytes).map_err(|error| SessionError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::other(error.to_string()),
    })
}

fn journal_pid(path: &Path) -> Option<u32> {
    let name = path.file_name()?.to_str()?;
    name.strip_prefix("journal-")?
        .strip_suffix(".json")?
        .parse()
        .ok()
}

fn process_is_alive(pid: u32) -> bool {
    Path::new("/proc").join(pid.to_string()).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(workspace: &Path, text: &str) -> SessionSnapshot {
        SessionSnapshot {
            workspace: workspace.to_path_buf(),
            buffers: vec![SessionBufferSnapshot {
                path: Some(workspace.join("src.rs")),
                text: Rope::from_str(text),
                dirty: true,
                cursor_char: 2,
                expected_disk_hash: Some(42),
                has_bom: false,
                line_ending: LineEnding::Lf,
                pinned: true,
            }],
            active_tab: Some(0),
            sidebar_visible: true,
            sidebar_view: SidebarView::Explorer,
            bottom_panel_visible: false,
            sidebar_width: 30,
            bottom_panel_height: 12,
        }
    }

    #[test]
    fn stale_journal_restores_unsaved_unicode_text() {
        let root = std::env::temp_dir().join(format!("mica-session-{}", std::process::id()));
        let state_home = root.join("state");
        let workspace = root.join("workspace");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&workspace).unwrap();
        let store = SessionStore::at_state_home(&workspace, &state_home).unwrap();
        let stale = store.directory.join("journal-4294967294.json");
        write_stored(&stale, snapshot(&workspace, "未保存👩‍💻")).unwrap();

        let recovery = store.discover_recovery().unwrap();
        assert_eq!(recovery.buffers.len(), 1);
        assert_eq!(recovery.buffers[0].text, "未保存👩‍💻");
        assert_eq!(recovery.journal_paths, vec![stale]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn normal_shutdown_removes_current_journal() {
        let root = std::env::temp_dir().join(format!("mica-session-clean-{}", std::process::id()));
        let state_home = root.join("state");
        let workspace = root.join("workspace");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&workspace).unwrap();
        let store = SessionStore::at_state_home(&workspace, &state_home).unwrap();
        let journal_path = store.current_journal().to_path_buf();
        let journal = SessionJournal::start(store);
        journal.submit(snapshot(&workspace, "draft"));
        journal.shutdown(snapshot(&workspace, "saved"));
        assert!(!journal_path.exists());
        let restored = SessionStore::at_state_home(&workspace, &state_home)
            .unwrap()
            .load_session()
            .unwrap()
            .unwrap();
        assert_eq!(
            restored.buffers,
            vec![RestoredBuffer {
                path: workspace.join("src.rs"),
                cursor_char: 2,
                pinned: true,
            }]
        );
        assert_eq!(restored.active_path, Some(workspace.join("src.rs")));
        fs::remove_dir_all(root).unwrap();
    }
}
