use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::Duration,
};

use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigWatcherError {
    #[error("no configuration directories are available to watch")]
    NoDirectories,
    #[error("failed to initialize configuration watcher: {0}")]
    Initialize(#[from] notify::Error),
}

pub struct ConfigWatcher {
    _watcher: RecommendedWatcher,
}

impl ConfigWatcher {
    pub fn spawn(
        paths: Vec<PathBuf>,
        on_change: impl Fn() + Send + 'static,
    ) -> Result<Self, ConfigWatcherError> {
        let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let targets = paths
            .into_iter()
            .map(|path| {
                if path.is_absolute() {
                    path
                } else {
                    current_dir.join(path)
                }
            })
            .collect::<HashSet<_>>();
        let directories = targets
            .iter()
            .filter_map(|path| nearest_existing_parent(path))
            .collect::<HashSet<_>>();
        if directories.is_empty() {
            return Err(ConfigWatcherError::NoDirectories);
        }
        let (sender, receiver) = mpsc::channel();
        let mut watcher = RecommendedWatcher::new(
            move |event| {
                let _ = sender.send(event);
            },
            Config::default(),
        )?;
        for directory in directories {
            watcher.watch(&directory, RecursiveMode::Recursive)?;
        }
        thread::spawn(move || {
            while let Ok(first) = receiver.recv() {
                let mut changed = event_targets_config(first, &targets);
                while let Ok(next) = receiver.recv_timeout(Duration::from_millis(150)) {
                    changed |= event_targets_config(next, &targets);
                }
                if changed {
                    on_change();
                }
            }
        });
        Ok(Self { _watcher: watcher })
    }
}

fn nearest_existing_parent(path: &Path) -> Option<PathBuf> {
    path.parent()
        .into_iter()
        .flat_map(Path::ancestors)
        .find(|candidate| candidate.is_dir())
        .map(Path::to_path_buf)
}

fn event_targets_config(
    event: Result<notify::Event, notify::Error>,
    targets: &HashSet<PathBuf>,
) -> bool {
    event.is_ok_and(|event| {
        event.paths.iter().any(|changed| {
            targets.iter().any(|target| {
                changed == target
                    || target.starts_with(changed)
                    || (changed.file_name() == target.file_name()
                        && changed.parent() == target.parent())
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, sync::mpsc, time::Duration};

    #[test]
    fn watches_configuration_file_changes() {
        let root = std::env::temp_dir().join(format!("mica-config-watch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".mica")).expect("create config directory");
        let path = root.join(".mica/config.toml");
        fs::write(&path, "[editor]\ntab_width = 2\n").expect("initial config");
        let (sender, receiver) = mpsc::channel();
        let _watcher = ConfigWatcher::spawn(vec![path.clone()], move || {
            let _ = sender.send(());
        })
        .expect("start config watcher");

        fs::write(&path, "[editor]\ntab_width = 8\n").expect("update config");
        receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("configuration change notification");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn watches_configuration_file_created_after_startup() {
        let root =
            std::env::temp_dir().join(format!("mica-config-create-watch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create workspace");
        let path = root.join(".mica/config.toml");
        let (sender, receiver) = mpsc::channel();
        let _watcher = ConfigWatcher::spawn(vec![path.clone()], move || {
            let _ = sender.send(());
        })
        .expect("start config watcher");

        fs::create_dir_all(path.parent().expect("config parent")).expect("create config directory");
        fs::write(&path, "[editor]\ntab_width = 8\n").expect("create config");
        receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("configuration creation notification");
        let _ = fs::remove_dir_all(root);
    }
}
