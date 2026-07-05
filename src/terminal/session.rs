use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct TerminalConfig {
    pub shell: PathBuf,
    pub cwd: PathBuf,
    pub rows: u16,
    pub cols: u16,
}

impl TerminalConfig {
    pub fn for_workspace(cwd: PathBuf, rows: u16, cols: u16) -> Self {
        let shell = std::env::var_os("SHELL")
            .map(PathBuf::from)
            .filter(|path| path.is_file())
            .unwrap_or_else(|| PathBuf::from("/bin/bash"));
        Self {
            shell,
            cwd,
            rows: rows.max(1),
            cols: cols.max(2),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalEvent {
    Output(Vec<u8>),
    Exited { code: u32, success: bool },
    Error(String),
}

#[derive(Debug)]
pub enum TerminalCommand {
    Input(Vec<u8>),
    Resize { rows: u16, cols: u16 },
    Shutdown,
}

#[derive(Debug, Error)]
pub enum TerminalSpawnError {
    #[error("failed to create PTY: {0}")]
    Open(String),
    #[error("failed to prepare PTY reader: {0}")]
    Reader(String),
    #[error("failed to prepare PTY writer: {0}")]
    Writer(String),
    #[error("failed to start shell `{shell}`: {detail}")]
    Shell { shell: PathBuf, detail: String },
}

pub struct TerminalSession {
    commands: Sender<TerminalCommand>,
    joins: Vec<JoinHandle<()>>,
}

impl TerminalSession {
    pub fn spawn(
        config: TerminalConfig,
        emit: impl Fn(TerminalEvent) + Send + Sync + 'static,
    ) -> Result<Self, TerminalSpawnError> {
        let emit = Arc::new(emit);
        let pair = native_pty_system()
            .openpty(pty_size(config.rows, config.cols))
            .map_err(|error| TerminalSpawnError::Open(error.to_string()))?;
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| TerminalSpawnError::Reader(error.to_string()))?;
        let mut writer = pair
            .master
            .take_writer()
            .map_err(|error| TerminalSpawnError::Writer(error.to_string()))?;
        let mut command = CommandBuilder::new(&config.shell);
        command.cwd(&config.cwd);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        let mut child =
            pair.slave
                .spawn_command(command)
                .map_err(|error| TerminalSpawnError::Shell {
                    shell: config.shell.clone(),
                    detail: error.to_string(),
                })?;
        drop(pair.slave);

        let killer = child.clone_killer();
        let exited = Arc::new(AtomicBool::new(false));
        let (commands, receiver) = mpsc::channel();
        let master = pair.master;
        let command_emit = emit.clone();
        let command_exited = exited.clone();
        let command_thread = thread::spawn(move || {
            let mut killer = killer;
            while let Ok(command) = receiver.recv() {
                let result = match command {
                    TerminalCommand::Input(bytes) => {
                        writer.write_all(&bytes).and_then(|()| writer.flush())
                    }
                    TerminalCommand::Resize { rows, cols } => master
                        .resize(pty_size(rows, cols))
                        .map_err(std::io::Error::other),
                    TerminalCommand::Shutdown => {
                        if !command_exited.load(Ordering::Acquire)
                            && let Err(error) = killer.kill()
                        {
                            command_emit(TerminalEvent::Error(error.to_string()));
                        }
                        break;
                    }
                };
                if let Err(error) = result {
                    command_emit(TerminalEvent::Error(error.to_string()));
                }
            }
        });

        let read_emit = emit.clone();
        let read_thread = thread::spawn(move || {
            let mut buffer = vec![0u8; 64 * 1024];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(length) => read_emit(TerminalEvent::Output(buffer[..length].to_vec())),
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        read_emit(TerminalEvent::Error(error.to_string()));
                        break;
                    }
                }
            }
        });

        let wait_emit = emit;
        let wait_exited = exited;
        let wait_thread = thread::spawn(move || match child.wait() {
            Ok(status) => {
                wait_exited.store(true, Ordering::Release);
                wait_emit(TerminalEvent::Exited {
                    code: status.exit_code(),
                    success: status.success(),
                });
            }
            Err(error) => {
                wait_exited.store(true, Ordering::Release);
                wait_emit(TerminalEvent::Error(error.to_string()));
            }
        });

        Ok(Self {
            commands,
            joins: vec![command_thread, read_thread, wait_thread],
        })
    }

    pub fn send(&self, command: TerminalCommand) -> Result<(), String> {
        self.commands
            .send(command)
            .map_err(|error| error.to_string())
    }

    pub fn input(&self, bytes: impl Into<Vec<u8>>) -> Result<(), String> {
        self.send(TerminalCommand::Input(bytes.into()))
    }

    pub fn resize(&self, rows: u16, cols: u16) -> Result<(), String> {
        self.send(TerminalCommand::Resize { rows, cols })
    }

    fn shutdown(&mut self) {
        let _ = self.commands.send(TerminalCommand::Shutdown);
        for join in self.joins.drain(..) {
            let _ = join.join();
        }
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn pty_size(rows: u16, cols: u16) -> PtySize {
    PtySize {
        rows: rows.max(1),
        cols: cols.max(2),
        pixel_width: 0,
        pixel_height: 0,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Condvar, Mutex};
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn real_pty_runs_interactive_shell_resizes_and_reaps() {
        let events = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
        let sink = events.clone();
        let config = TerminalConfig {
            shell: PathBuf::from("/bin/bash"),
            cwd: std::env::temp_dir(),
            rows: 10,
            cols: 40,
        };
        let session = TerminalSession::spawn(config, move |event| {
            let (events, ready) = &*sink;
            events.lock().unwrap().push(event);
            ready.notify_all();
        })
        .unwrap();
        session.resize(12, 60).unwrap();
        session
            .input(b"printf 'MICA_PTY_OK\\n'\r".to_vec())
            .unwrap();
        session.input(b"exit 7\r".to_vec()).unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        let (recorded, ready) = &*events;
        let mut recorded = recorded.lock().unwrap();
        while !recorded
            .iter()
            .any(|event| matches!(event, TerminalEvent::Exited { .. }))
            && Instant::now() < deadline
        {
            let timeout = deadline.saturating_duration_since(Instant::now());
            recorded = ready.wait_timeout(recorded, timeout).unwrap().0;
        }
        let output = recorded
            .iter()
            .filter_map(|event| match event {
                TerminalEvent::Output(bytes) => Some(String::from_utf8_lossy(bytes)),
                _ => None,
            })
            .collect::<String>();
        assert!(output.contains("MICA_PTY_OK"));
        assert!(recorded.iter().any(|event| matches!(
            event,
            TerminalEvent::Exited {
                code: 7,
                success: false
            }
        )));
        drop(recorded);
        drop(session);
    }
}
