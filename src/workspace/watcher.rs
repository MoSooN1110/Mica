use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::mpsc::{self, RecvTimeoutError},
    thread::{self, JoinHandle},
    time::Duration,
};

use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct WatchBatch {
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Error)]
pub enum WatcherError {
    #[error("failed to initialize workspace watcher: {0}")]
    Initialize(String),
}

pub struct WorkspaceWatcher {
    stop: mpsc::Sender<()>,
    worker: Option<JoinHandle<()>>,
}

impl WorkspaceWatcher {
    /// Starts a recursive watcher on its own thread. Bursts are coalesced into
    /// one callback and common editor temporary files are suppressed.
    pub fn spawn(
        root: PathBuf,
        callback: impl Fn(WatchBatch) + Send + 'static,
    ) -> Result<Self, WatcherError> {
        let (stop_tx, stop_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            let (event_tx, event_rx) = mpsc::channel();
            let watcher = RecommendedWatcher::new(
                move |event| {
                    let _ = event_tx.send(event);
                },
                Config::default(),
            );
            let mut watcher = match watcher {
                Ok(watcher) => watcher,
                Err(error) => {
                    let _ = ready_tx.send(Err(error.to_string()));
                    return;
                }
            };
            if let Err(error) = watcher.watch(&root, RecursiveMode::Recursive) {
                let _ = ready_tx.send(Err(error.to_string()));
                return;
            }
            let _ = ready_tx.send(Ok(()));

            loop {
                if stop_rx.try_recv().is_ok() {
                    break;
                }
                let first = match event_rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(event) => event,
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => break,
                };
                let mut paths = BTreeSet::new();
                collect_paths(first, &mut paths);
                loop {
                    if stop_rx.try_recv().is_ok() {
                        return;
                    }
                    match event_rx.recv_timeout(Duration::from_millis(120)) {
                        Ok(event) => collect_paths(event, &mut paths),
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                paths.retain(|path| !is_temporary_noise(path));
                if !paths.is_empty() {
                    callback(WatchBatch {
                        paths: paths.into_iter().collect(),
                    });
                }
            }
        });
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                stop: stop_tx,
                worker: Some(worker),
            }),
            Ok(Err(error)) => {
                let _ = worker.join();
                Err(WatcherError::Initialize(error))
            }
            Err(error) => Err(WatcherError::Initialize(error.to_string())),
        }
    }
}

impl Drop for WorkspaceWatcher {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn collect_paths(event: Result<notify::Event, notify::Error>, paths: &mut BTreeSet<PathBuf>) {
    if let Ok(event) = event {
        paths.extend(event.paths);
    }
}

fn is_temporary_noise(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name.ends_with('~')
        || name.ends_with(".swp")
        || name.ends_with(".tmp")
        || (name.starts_with('.') && name.contains(".mica-") && name.ends_with(".tmp"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, time::Instant};

    #[test]
    fn temporary_file_filter_is_conservative() {
        assert!(is_temporary_noise(Path::new("file.rs.swp")));
        assert!(is_temporary_noise(Path::new("file.rs~")));
        assert!(is_temporary_noise(Path::new(".file.mica-1.tmp")));
        assert!(!is_temporary_noise(Path::new("important.tmp.rs")));
    }

    #[test]
    fn watcher_reports_create_update_rename_and_delete() {
        let root = std::env::temp_dir().join(format!("mica-watch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let (sender, receiver) = mpsc::channel();
        let _watcher = WorkspaceWatcher::spawn(root.clone(), move |batch| {
            let _ = sender.send(batch);
        })
        .unwrap();

        let original = root.join("original.rs");
        fs::write(&original, b"one").unwrap();
        assert!(wait_for_path(&receiver, &original));

        fs::write(&original, b"two").unwrap();
        assert!(wait_for_path(&receiver, &original));

        let renamed = root.join("renamed.rs");
        fs::rename(&original, &renamed).unwrap();
        assert!(wait_for_path(&receiver, &renamed));

        fs::remove_file(&renamed).unwrap();
        assert!(wait_for_path(&receiver, &renamed));
        fs::remove_dir_all(root).unwrap();
    }

    fn wait_for_path(receiver: &mpsc::Receiver<WatchBatch>, expected: &Path) -> bool {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match receiver.recv_timeout(remaining) {
                Ok(batch) if batch.paths.iter().any(|path| path == expected) => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
        false
    }
}
