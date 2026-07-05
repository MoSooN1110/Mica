use std::{
    error::Error,
    io::{self, Read, stdout},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use clap::Parser;
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use mica::{
    app::{
        AppEvent, AppState, Effect, ExternalFileRead, FileOperationRequest, FileOperationResult,
    },
    buffer::{BufferError, TextBuffer, atomic_save_if_unchanged},
    cli::{Cli, CliLocale},
    command::Command,
    config::{ConfigLoad, Keymap, Locale},
    editor::highlight_rust,
    search::{find_matches, fuzzy_files},
    session::{SessionJournal, SessionStore, discard_recovery},
    ui::{ColorMode, Regions, Theme, command_for_key, command_for_mouse, render},
    workspace::{FileOperations, FileTree, WorkspaceRoot, WorkspaceWatcher},
};
use ratatui::{Terminal, backend::CrosstermBackend};

fn main() {
    if let Err(error) = run() {
        eprintln!("mica: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    let current_dir = std::env::current_dir()?;
    let target = cli.resolve_target(&current_dir)?;
    let mut config = ConfigLoad::load(&target.workspace, cli.config.as_deref(), cli.safe_mode);
    if cli.no_mouse {
        config.settings.editor.mouse = false;
    }
    if let Some(locale) = cli.locale {
        config.settings.ui.locale = match locale {
            CliLocale::En => Locale::En,
            CliLocale::Ja => Locale::Ja,
        };
    }
    let (keymap, key_warnings) = Keymap::from_overrides(&config.settings.keymap);
    config.warnings.extend(key_warnings);
    let workspace = WorkspaceRoot::new(&target.workspace)?;
    let mut state = AppState::new(
        workspace,
        config.settings,
        keymap,
        config.warnings,
        cli.readonly,
    );
    let mut session_journal = None;
    let mut restored_paths = Vec::new();
    match SessionStore::for_workspace(state.workspace.as_path()) {
        Ok(store) => {
            match store.load_session() {
                Ok(Some(restored)) => {
                    state.apply_restored_session(&restored);
                    restored_paths = restored
                        .buffers
                        .iter()
                        .map(|(path, _)| path.clone())
                        .filter(|path| path.is_file())
                        .collect();
                }
                Ok(None) => {}
                Err(error) => state.notification = Some(error.to_string()),
            }
            match store.discover_recovery() {
                Ok(recovery) if recovery.buffers.is_empty() => {
                    if let Err(error) = discard_recovery(&recovery.journal_paths) {
                        state.notification = Some(error.to_string());
                    }
                }
                Ok(recovery) => state.offer_recovery(recovery),
                Err(error) => state.notification = Some(error.to_string()),
            }
            session_journal = Some(SessionJournal::start(store));
        }
        Err(error) => state.notification = Some(format!("Session recovery unavailable: {error}")),
    }
    let (sender, receiver) = mpsc::channel();
    execute_effects(&state, vec![Effect::ScanWorkspace], &sender);
    for path in restored_paths {
        execute_effects(
            &state,
            vec![Effect::OpenFile {
                path,
                read_only: cli.readonly,
                line: None,
                column: None,
            }],
            &sender,
        );
    }
    if let Some(path) = target.file {
        execute_effects(
            &state,
            vec![Effect::OpenFile {
                path,
                read_only: cli.readonly,
                line: target.line,
                column: target.column,
            }],
            &sender,
        );
    }
    let watch_sender = sender.clone();
    let watcher = WorkspaceWatcher::spawn(state.workspace.as_path().to_path_buf(), move |batch| {
        let _ = watch_sender.send(AppEvent::WorkspaceChanged(batch.paths));
    });
    let _watcher = match watcher {
        Ok(watcher) => Some(watcher),
        Err(error) => {
            state.notification = Some(format!("File watcher unavailable: {error}"));
            None
        }
    };

    install_panic_hook(state.settings.editor.mouse);
    let _guard = TerminalGuard::enter(state.settings.editor.mouse)?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    let size = terminal.size()?;
    state.update(AppEvent::Command(Command::Resize(size.width, size.height)));
    let color_mode = detect_color_mode();
    let theme = Theme::mica_dark(color_mode);
    if let Some(journal) = &session_journal {
        journal.submit(state.session_snapshot());
    }
    event_loop(
        &mut terminal,
        &mut state,
        &theme,
        receiver,
        sender,
        session_journal.as_ref(),
    )?;
    if let Some(journal) = session_journal {
        journal.shutdown(state.session_snapshot());
    }
    Ok(())
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut AppState,
    theme: &Theme,
    receiver: Receiver<AppEvent>,
    sender: Sender<AppEvent>,
    session_journal: Option<&SessionJournal>,
) -> io::Result<()> {
    let mut regions = Regions::default();
    while !state.should_quit {
        while let Ok(event) = receiver.try_recv() {
            let effects = state.update(event);
            execute_effects(state, effects, &sender);
            if let Some(journal) = session_journal {
                journal.submit(state.session_snapshot());
            }
        }
        terminal.draw(|frame| {
            regions = render(frame, state, theme);
        })?;
        if !event::poll(Duration::from_millis(50))? {
            continue;
        }
        let command = match event::read()? {
            Event::Key(key) => command_for_key(state, key),
            Event::Mouse(mouse) if state.settings.editor.mouse => {
                command_for_mouse(state, regions, mouse)
            }
            Event::Resize(width, height) => Some(Command::Resize(width, height)),
            Event::Paste(text) => Some(Command::InsertText(text)),
            _ => None,
        };
        if let Some(command) = command {
            let effects = state.update(AppEvent::Command(command));
            execute_effects(state, effects, &sender);
            if let Some(journal) = session_journal {
                journal.submit(state.session_snapshot());
            }
        }
    }
    Ok(())
}

fn execute_effects(state: &AppState, effects: Vec<Effect>, sender: &Sender<AppEvent>) {
    for effect in effects {
        let sender = sender.clone();
        match effect {
            Effect::OpenFile {
                path,
                read_only,
                line,
                column,
            } => {
                thread::spawn(move || {
                    let canonical = if path.exists() {
                        path.canonicalize().unwrap_or(path)
                    } else {
                        path
                    };
                    let result = if canonical.exists() {
                        TextBuffer::open(&canonical, read_only)
                    } else {
                        Ok(TextBuffer::empty(Some(canonical.clone()), read_only))
                    }
                    .map_err(|error| error.to_string());
                    let _ = sender.send(AppEvent::FileOpened {
                        path: canonical,
                        line,
                        column,
                        result,
                    });
                });
            }
            Effect::ScanWorkspace => {
                let root = state.workspace.as_path().to_path_buf();
                let settings = state.settings.workspace.clone();
                thread::spawn(move || {
                    let result = FileTree::scan(
                        &root,
                        settings.show_hidden,
                        settings.follow_symlinks,
                        settings.respect_gitignore,
                    )
                    .map_err(|error| error.to_string());
                    let _ = sender.send(AppEvent::TreeLoaded(result));
                });
            }
            Effect::Save { tab, snapshot } => {
                thread::spawn(move || {
                    let result = atomic_save_if_unchanged(
                        &snapshot.path,
                        &snapshot.bytes,
                        snapshot.expected_disk_hash,
                    )
                    .map_err(|error| error.to_string());
                    let _ = sender.send(AppEvent::SaveCompleted {
                        tab,
                        snapshot,
                        result,
                    });
                });
            }
            Effect::RefreshOpenFiles { files, read_only } => {
                thread::spawn(move || {
                    let reads = files
                        .into_iter()
                        .map(|(tab, path)| {
                            let result = match TextBuffer::open(&path, read_only) {
                                Ok(buffer) => Ok(Some(buffer)),
                                Err(BufferError::Read { source, .. })
                                    if source.kind() == io::ErrorKind::NotFound =>
                                {
                                    Ok(None)
                                }
                                Err(error) => Err(error.to_string()),
                            };
                            ExternalFileRead { tab, path, result }
                        })
                        .collect();
                    let _ = sender.send(AppEvent::ExternalFilesRead(reads));
                });
            }
            Effect::InspectDelete(path) => {
                let root = state.workspace.clone();
                thread::spawn(move || {
                    let result = FileOperations::new(root)
                        .inspect_delete(path)
                        .map_err(|error| error.to_string());
                    let _ = sender.send(AppEvent::DeleteInspected(result));
                });
            }
            Effect::FileOperation(request) => {
                let root = state.workspace.clone();
                thread::spawn(move || {
                    let operations = FileOperations::new(root);
                    let result =
                        match request {
                            FileOperationRequest::CreateFile(path) => operations
                                .create_file(path)
                                .map(|path| FileOperationResult::Created {
                                    path: path.absolute(),
                                    directory: false,
                                }),
                            FileOperationRequest::CreateDirectory(path) => operations
                                .create_directory(path)
                                .map(|path| FileOperationResult::Created {
                                    path: path.absolute(),
                                    directory: true,
                                }),
                            FileOperationRequest::Move {
                                source,
                                destination,
                            } => operations.move_path(source, destination).map(
                                |(source, destination)| FileOperationResult::Moved {
                                    source: source.absolute(),
                                    destination: destination.absolute(),
                                },
                            ),
                            FileOperationRequest::Delete(plan) => {
                                let path = plan.path().absolute();
                                operations
                                    .execute_delete(plan)
                                    .map(|()| FileOperationResult::Deleted(path))
                            }
                        }
                        .map_err(|error| error.to_string());
                    let _ = sender.send(AppEvent::FileOperationCompleted(result));
                });
            }
            Effect::FuzzyFiles {
                generation,
                query,
                paths,
                cancellation,
            } => {
                thread::spawn(move || {
                    let matches = fuzzy_files(&query, paths, &cancellation, generation, 100);
                    let _ = sender.send(AppEvent::FileSearchCompleted {
                        generation,
                        matches,
                    });
                });
            }
            Effect::PreviewFile { generation, path } => {
                thread::spawn(move || {
                    let result = (|| -> io::Result<Vec<String>> {
                        let mut bytes = Vec::new();
                        std::fs::File::open(&path)?
                            .take(64 * 1024)
                            .read_to_end(&mut bytes)?;
                        if bytes.contains(&0) {
                            return Ok(vec!["<binary file>".to_owned()]);
                        }
                        Ok(String::from_utf8_lossy(&bytes)
                            .lines()
                            .take(200)
                            .map(ToOwned::to_owned)
                            .collect())
                    })()
                    .map_err(|error| format!("Preview failed: {error}"));
                    let _ = sender.send(AppEvent::FilePreviewLoaded {
                        generation,
                        path,
                        result,
                    });
                });
            }
            Effect::CopyToClipboard(text) => {
                use std::io::Write;

                let encoded = BASE64.encode(text);
                let mut output = stdout();
                let _ = write!(output, "\x1b]52;c;{encoded}\x07");
                let _ = output.flush();
            }
            Effect::SearchBuffer {
                tab,
                buffer_generation,
                search_generation,
                query,
                source,
                cancellation,
            } => {
                thread::spawn(move || {
                    let source = source.to_string();
                    let matches =
                        find_matches(&source, &query, &cancellation, search_generation, 10_000);
                    let _ = sender.send(AppEvent::BufferSearchCompleted {
                        tab,
                        buffer_generation,
                        search_generation,
                        query,
                        matches,
                    });
                });
            }
            Effect::HighlightRust {
                tab,
                buffer_generation,
                syntax_generation,
                source,
                cancellation,
            } => {
                thread::spawn(move || {
                    let source = source.to_string();
                    let result = highlight_rust(&source, &cancellation, syntax_generation)
                        .map_err(|error| error.to_string());
                    let _ = sender.send(AppEvent::SyntaxHighlighted {
                        tab,
                        buffer_generation,
                        syntax_generation,
                        result,
                    });
                });
            }
            Effect::DiscardRecovery(paths) => {
                thread::spawn(move || {
                    let result = discard_recovery(&paths).map_err(|error| error.to_string());
                    let _ = sender.send(AppEvent::RecoveryDiscarded(result));
                });
            }
        }
    }
}

struct TerminalGuard {
    mouse: bool,
}

impl TerminalGuard {
    fn enter(mouse: bool) -> io::Result<Self> {
        enable_raw_mode()?;
        let mut output = stdout();
        execute!(output, EnterAlternateScreen, EnableBracketedPaste)?;
        if mouse {
            execute!(output, EnableMouseCapture)?;
        }
        Ok(Self { mouse })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let mut output = stdout();
        if self.mouse {
            let _ = execute!(output, DisableMouseCapture);
        }
        let _ = execute!(output, DisableBracketedPaste, LeaveAlternateScreen);
    }
}

fn install_panic_hook(mouse: bool) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let mut output = stdout();
        if mouse {
            let _ = execute!(output, DisableMouseCapture);
        }
        let _ = execute!(output, DisableBracketedPaste, LeaveAlternateScreen);
        previous(info);
    }));
}

fn detect_color_mode() -> ColorMode {
    let color_term = std::env::var("COLORTERM")
        .unwrap_or_default()
        .to_ascii_lowercase();
    if color_term.contains("truecolor") || color_term.contains("24bit") {
        ColorMode::TrueColor
    } else {
        ColorMode::Ansi256
    }
}
