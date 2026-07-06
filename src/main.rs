use std::{
    collections::HashMap,
    error::Error,
    io::{self, Read, stdout},
    path::Path,
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
        AppEvent, AppState, BottomPanelView, Effect, ExternalFileRead, FileOperationRequest,
        FileOperationResult, GitOperation, SaveAsInspection, SaveAsPlan,
    },
    buffer::{BufferError, SaveSnapshot, TextBuffer, atomic_save_if_unchanged, disk_content_hash},
    cli::{Cli, CliLocale},
    command::Command,
    config::{ConfigLoad, Keymap, Locale},
    editor::highlight,
    git::{GitBackend, GitCliBackend},
    lsp::{LspClient, LspClientConfig},
    search::{find_matches, fuzzy_files, search_workspace},
    session::{SessionJournal, SessionStore, discard_recovery},
    terminal::{TerminalConfig, TerminalEvent, TerminalSession},
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
    let mut runtime = Runtime::new(sender.clone());
    execute_effects(
        &state,
        vec![Effect::ScanWorkspace, Effect::RefreshGit],
        &sender,
        &mut runtime,
    );
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
            &mut runtime,
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
            &mut runtime,
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
        &mut runtime,
    )?;
    runtime.shutdown();
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
    runtime: &mut Runtime,
) -> io::Result<()> {
    let mut regions = Regions::default();
    while !state.should_quit {
        for _ in 0..256 {
            let Ok(event) = receiver.try_recv() else {
                break;
            };
            let effects = state.update(event);
            execute_effects(state, effects, &sender, runtime);
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
            Event::Paste(text)
                if state.focus == mica::app::Focus::BottomPanel
                    && state.bottom_panel_view == BottomPanelView::Terminal =>
            {
                Some(Command::TerminalPaste(text))
            }
            Event::Paste(text) => Some(Command::InsertText(text)),
            _ => None,
        };
        if let Some(command) = command {
            let effects = state.update(AppEvent::Command(command));
            execute_effects(state, effects, &sender, runtime);
            if let Some(journal) = session_journal {
                journal.submit(state.session_snapshot());
            }
        }
    }
    Ok(())
}

struct Runtime {
    terminal: Option<TerminalRuntime>,
    lsp: HashMap<String, LspClient>,
}

impl Runtime {
    fn new(events: Sender<AppEvent>) -> Self {
        Self {
            terminal: Some(TerminalRuntime::spawn(events)),
            lsp: HashMap::new(),
        }
    }

    fn send_terminal(&self, command: TerminalRuntimeCommand) {
        if let Some(terminal) = &self.terminal {
            terminal.send(command);
        }
    }

    fn shutdown(&mut self) {
        self.lsp.clear();
        self.terminal.take();
    }
}

enum TerminalRuntimeCommand {
    Start {
        generation: u64,
        config: TerminalConfig,
    },
    Input(Vec<u8>),
    Resize {
        rows: u16,
        cols: u16,
    },
    Stop,
    Shutdown,
}

struct TerminalRuntime {
    commands: Sender<TerminalRuntimeCommand>,
    join: Option<thread::JoinHandle<()>>,
}

impl TerminalRuntime {
    fn spawn(events: Sender<AppEvent>) -> Self {
        let (commands, receiver) = mpsc::channel();
        let join = thread::spawn(move || {
            let mut session: Option<TerminalSession> = None;
            let mut current_generation = 0u64;
            while let Ok(command) = receiver.recv() {
                match command {
                    TerminalRuntimeCommand::Start { generation, config } => {
                        current_generation = generation;
                        session.take();
                        let event_sender = events.clone();
                        match TerminalSession::spawn(config, move |event| {
                            let event = match event {
                                TerminalEvent::Output(bytes) => {
                                    AppEvent::TerminalOutput { generation, bytes }
                                }
                                TerminalEvent::Exited { code, success } => {
                                    AppEvent::TerminalExited {
                                        generation,
                                        code,
                                        success,
                                    }
                                }
                                TerminalEvent::Error(error) => {
                                    AppEvent::TerminalError { generation, error }
                                }
                            };
                            let _ = event_sender.send(event);
                        }) {
                            Ok(next) => {
                                session = Some(next);
                                let _ = events.send(AppEvent::TerminalStarted {
                                    generation,
                                    result: Ok(()),
                                });
                            }
                            Err(error) => {
                                let _ = events.send(AppEvent::TerminalStarted {
                                    generation,
                                    result: Err(error.to_string()),
                                });
                            }
                        }
                    }
                    TerminalRuntimeCommand::Input(bytes) => {
                        if let Some(session) = &session
                            && let Err(error) = session.input(bytes)
                        {
                            let _ = events.send(AppEvent::TerminalError {
                                generation: current_generation,
                                error,
                            });
                        }
                    }
                    TerminalRuntimeCommand::Resize { rows, cols } => {
                        if let Some(session) = &session
                            && let Err(error) = session.resize(rows, cols)
                        {
                            let _ = events.send(AppEvent::TerminalError {
                                generation: current_generation,
                                error,
                            });
                        }
                    }
                    TerminalRuntimeCommand::Stop => {
                        session.take();
                    }
                    TerminalRuntimeCommand::Shutdown => break,
                }
            }
            session.take();
        });
        Self {
            commands,
            join: Some(join),
        }
    }

    fn send(&self, command: TerminalRuntimeCommand) {
        let _ = self.commands.send(command);
    }
}

impl Drop for TerminalRuntime {
    fn drop(&mut self) {
        let _ = self.commands.send(TerminalRuntimeCommand::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn execute_effects(
    state: &AppState,
    effects: Vec<Effect>,
    sender: &Sender<AppEvent>,
    runtime: &mut Runtime,
) {
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
            Effect::InspectSaveAs {
                tab,
                destination,
                mut snapshot,
            } => {
                let root = state.workspace.clone();
                thread::spawn(move || {
                    let result = inspect_save_as(&root, destination, tab, &mut snapshot);
                    let _ = sender.send(AppEvent::SaveAsInspected(result));
                });
            }
            Effect::SaveAs(plan) => {
                thread::spawn(move || {
                    let result = atomic_save_if_unchanged(
                        &plan.snapshot.path,
                        &plan.snapshot.bytes,
                        plan.snapshot.expected_disk_hash,
                    )
                    .map_err(|error| error.to_string());
                    let _ = sender.send(AppEvent::SaveAsCompleted { plan, result });
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
            Effect::HighlightSyntax {
                tab,
                buffer_generation,
                syntax_generation,
                language,
                source,
                cancellation,
            } => {
                thread::spawn(move || {
                    let source = source.to_string();
                    let result = highlight(language, &source, &cancellation, syntax_generation)
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
            Effect::RefreshGit => {
                let root = state.workspace.as_path().to_path_buf();
                thread::spawn(move || {
                    let result = GitCliBackend::new(root)
                        .status()
                        .map_err(|error| error.to_string());
                    let _ = sender.send(AppEvent::GitStatusLoaded(result));
                });
            }
            Effect::LoadGitDiff { path, target } => {
                let root = state.workspace.as_path().to_path_buf();
                thread::spawn(move || {
                    let result = GitCliBackend::new(root)
                        .diff(&path, target)
                        .map_err(|error| error.to_string());
                    let _ = sender.send(AppEvent::GitDiffLoaded(result));
                });
            }
            Effect::GitOperation(operation) => {
                let root = state.workspace.as_path().to_path_buf();
                thread::spawn(move || {
                    let backend = GitCliBackend::new(root);
                    let workspace_changed = matches!(
                        operation,
                        GitOperation::SwitchBranch(_) | GitOperation::CreateBranch(_)
                    );
                    let result = match operation {
                        GitOperation::Stage(path) => backend
                            .stage_file(&path)
                            .map(|()| format!("Staged {}", path.display())),
                        GitOperation::Unstage(path) => backend
                            .unstage_file(&path)
                            .map(|()| format!("Unstaged {}", path.display())),
                        GitOperation::Restore(path) => backend
                            .restore_file(&path)
                            .map(|()| format!("Restored {}", path.display())),
                        GitOperation::StageHunk(patch) => backend
                            .stage_hunk(&patch)
                            .map(|()| "Staged hunk".to_owned()),
                        GitOperation::UnstageHunk(patch) => backend
                            .unstage_hunk(&patch)
                            .map(|()| "Unstaged hunk".to_owned()),
                        GitOperation::RestoreHunk(patch) => backend
                            .restore_hunk(&patch)
                            .map(|()| "Restored hunk".to_owned()),
                        GitOperation::Commit(message) => backend
                            .commit(&message)
                            .map(|()| "Commit created".to_owned()),
                        GitOperation::SwitchBranch(branch) => backend
                            .switch_branch(&branch)
                            .map(|()| format!("Switched to {branch}")),
                        GitOperation::CreateBranch(branch) => backend
                            .create_branch(&branch)
                            .map(|()| format!("Created and switched to {branch}")),
                        GitOperation::Fetch => {
                            backend.fetch().map(|()| "Fetch completed".to_owned())
                        }
                        GitOperation::Pull => backend.pull().map(|()| "Pull completed".to_owned()),
                        GitOperation::Push => backend.push().map(|()| "Push completed".to_owned()),
                    }
                    .map_err(|error| error.to_string());
                    let _ = sender.send(AppEvent::GitOperationCompleted {
                        result,
                        workspace_changed,
                    });
                });
            }
            Effect::LoadGitBranches => {
                let root = state.workspace.as_path().to_path_buf();
                thread::spawn(move || {
                    let result = GitCliBackend::new(root)
                        .branches()
                        .map_err(|error| error.to_string());
                    let _ = sender.send(AppEvent::GitBranchesLoaded(result));
                });
            }
            Effect::SearchWorkspace {
                generation,
                options,
                open_buffers,
                cancellation,
            } => {
                let root = state.workspace.as_path().to_path_buf();
                thread::spawn(move || {
                    let batch_sender = sender.clone();
                    let result = search_workspace(
                        &root,
                        &options,
                        &open_buffers,
                        &cancellation,
                        generation,
                        |matches| {
                            let _ = batch_sender.send(AppEvent::WorkspaceSearchBatch {
                                generation,
                                matches,
                                done: false,
                                error: None,
                            });
                        },
                    );
                    let _ = sender.send(AppEvent::WorkspaceSearchBatch {
                        generation,
                        matches: Vec::new(),
                        done: true,
                        error: result.err().map(|error| error.to_string()),
                    });
                });
            }
            Effect::StartTerminal {
                generation,
                shell,
                cwd,
                rows,
                cols,
            } => {
                let mut config = TerminalConfig::for_workspace(cwd, rows, cols);
                if let Some(shell) = shell {
                    config.shell = shell;
                }
                runtime.send_terminal(TerminalRuntimeCommand::Start { generation, config });
            }
            Effect::TerminalInput(bytes) => {
                runtime.send_terminal(TerminalRuntimeCommand::Input(bytes));
            }
            Effect::ResizeTerminal { rows, cols } => {
                runtime.send_terminal(TerminalRuntimeCommand::Resize { rows, cols });
            }
            Effect::StopTerminal => {
                runtime.send_terminal(TerminalRuntimeCommand::Stop);
            }
            Effect::RunCargoDiagnostics { generation } => {
                let root = state.workspace.as_path().to_path_buf();
                thread::spawn(move || {
                    run_cargo_diagnostics(&root, generation, &sender);
                });
            }
            Effect::StartLsp {
                language,
                settings,
                workspace,
            } => {
                if runtime.lsp.contains_key(&language) {
                    continue;
                }
                let event_sender = sender.clone();
                let event_language = language.clone();
                let client = LspClient::spawn(
                    LspClientConfig {
                        command: settings.command,
                        args: settings.args,
                        root: workspace,
                        root_markers: settings.root_markers,
                        language_id: language.clone(),
                    },
                    move |event| {
                        let _ = event_sender.send(AppEvent::LspClient {
                            language: event_language.clone(),
                            event,
                        });
                    },
                );
                runtime.lsp.insert(language, client);
            }
            Effect::SendLsp { language, message } => {
                if let Some(client) = runtime.lsp.get(&language)
                    && let Err(error) = client.send(message)
                {
                    let _ = sender.send(AppEvent::LspClient {
                        language,
                        event: mica::lsp::LspClientEvent::Error(error),
                    });
                }
            }
            Effect::StopLsp { language } => {
                if let Some(client) = runtime.lsp.remove(&language) {
                    thread::spawn(move || drop(client));
                }
            }
        }
    }
}

/// Runs `cargo` with structured JSON diagnostics output, off the UI thread.
fn spawn_cargo_json(root: &Path, args: &[&str]) -> io::Result<std::process::Output> {
    std::process::Command::new("cargo")
        .current_dir(root)
        .args(args)
        .env("CARGO_TERM_COLOR", "never")
        .output()
}

/// Detects cargo's error text for a missing `clippy` component, so callers
/// can fall back to plain `cargo check` instead of surfacing a spurious
/// diagnostics failure.
fn clippy_unavailable(stderr: &[u8]) -> bool {
    let text = String::from_utf8_lossy(stderr).to_lowercase();
    text.contains("no such command") || text.contains("is not installed")
}

/// Runs `cargo clippy --message-format=json` (SPEC/06 §1.1: compiler + linter
/// diagnostic sources), falling back to `cargo check` when clippy is not
/// installed. Clippy output is a superset of check output, so a single run
/// covers both sources; the parsed diagnostics are split by
/// `DiagnosticSource` and replaced independently so each source keeps its
/// own generation.
fn run_cargo_diagnostics(root: &Path, generation: u64, sender: &Sender<AppEvent>) {
    let output = match spawn_cargo_json(root, &["clippy", "--message-format=json"]) {
        Ok(output) if !clippy_unavailable(&output.stderr) => Ok(output),
        _ => spawn_cargo_json(root, &["check", "--message-format=json"]),
    };
    match output {
        Ok(output) => {
            let diagnostics = mica::diagnostics::parse_cargo_messages(root, &output.stdout);
            if !output.stderr.is_empty() {
                let _ = sender.send(AppEvent::OutputMessage {
                    source: "cargo".to_owned(),
                    message: String::from_utf8_lossy(&output.stderr).into_owned(),
                });
            }
            if output.status.success() || !diagnostics.is_empty() {
                let (linter, compiler): (Vec<_>, Vec<_>) =
                    diagnostics.into_iter().partition(|diagnostic| {
                        diagnostic.source == mica::diagnostics::DiagnosticSource::Linter
                    });
                let _ = sender.send(AppEvent::DiagnosticsReplaced {
                    source: mica::diagnostics::DiagnosticSource::Compiler,
                    generation,
                    diagnostics: compiler,
                });
                let _ = sender.send(AppEvent::DiagnosticsReplaced {
                    source: mica::diagnostics::DiagnosticSource::Linter,
                    generation,
                    diagnostics: linter,
                });
            } else {
                let error = format!("cargo diagnostics failed: {}", output.status);
                let _ = sender.send(AppEvent::DiagnosticsFailed {
                    source: mica::diagnostics::DiagnosticSource::Compiler,
                    generation,
                    error: error.clone(),
                });
                let _ = sender.send(AppEvent::DiagnosticsFailed {
                    source: mica::diagnostics::DiagnosticSource::Linter,
                    generation,
                    error,
                });
            }
        }
        Err(error) => {
            let error = format!("cannot run cargo diagnostics: {error}");
            let _ = sender.send(AppEvent::DiagnosticsFailed {
                source: mica::diagnostics::DiagnosticSource::Compiler,
                generation,
                error: error.clone(),
            });
            let _ = sender.send(AppEvent::DiagnosticsFailed {
                source: mica::diagnostics::DiagnosticSource::Linter,
                generation,
                error,
            });
        }
    }
}

fn inspect_save_as(
    root: &WorkspaceRoot,
    destination: std::path::PathBuf,
    tab: usize,
    snapshot: &mut SaveSnapshot,
) -> Result<SaveAsInspection, String> {
    let lexical = root
        .resolve_lexical(&destination)
        .map_err(|error| error.to_string())?;
    if lexical == root.as_path() {
        return Err("cannot save over the workspace root".to_owned());
    }
    if lexical.is_symlink() {
        return Err(format!(
            "refusing Save As through symlink: {}",
            lexical.display()
        ));
    }
    let resolved = root
        .resolve(&destination)
        .map_err(|error| error.to_string())?
        .absolute();
    let parent = resolved
        .parent()
        .ok_or_else(|| "save destination has no parent".to_owned())?;
    if !parent.is_dir() {
        return Err(format!(
            "save destination directory does not exist: {}",
            parent.display()
        ));
    }
    if resolved.is_dir() {
        return Err(format!(
            "save destination is a directory: {}",
            resolved.display()
        ));
    }
    let original = snapshot.path.clone();
    snapshot.path = resolved.clone();
    if resolved.exists() {
        let bytes = std::fs::read(&resolved)
            .map_err(|error| format!("cannot inspect destination: {error}"))?;
        snapshot.expected_disk_hash = Some(disk_content_hash(&bytes));
        let plan = SaveAsPlan {
            tab,
            snapshot: snapshot.clone(),
        };
        if original == resolved {
            Ok(SaveAsInspection::Ready(plan))
        } else {
            Ok(SaveAsInspection::ConfirmOverwrite(plan))
        }
    } else {
        snapshot.expected_disk_hash = None;
        Ok(SaveAsInspection::Ready(SaveAsPlan {
            tab,
            snapshot: snapshot.clone(),
        }))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn save_snapshot(root: &std::path::Path) -> SaveSnapshot {
        let mut buffer = TextBuffer::empty(Some(root.join("original.rs")), false);
        buffer.insert("fn main() {}").unwrap();
        buffer.prepare_save().unwrap()
    }

    #[test]
    fn save_as_inspection_distinguishes_new_and_existing_targets() {
        let directory = std::env::temp_dir().join(format!("mica-save-as-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let root = WorkspaceRoot::new(&directory).unwrap();

        let mut snapshot = save_snapshot(&directory);
        let new = inspect_save_as(&root, "new.rs".into(), 0, &mut snapshot).unwrap();
        assert!(matches!(new, SaveAsInspection::Ready(_)));

        std::fs::write(directory.join("existing.rs"), b"existing").unwrap();
        let mut snapshot = save_snapshot(&directory);
        let existing = inspect_save_as(&root, "existing.rs".into(), 0, &mut snapshot).unwrap();
        let SaveAsInspection::ConfirmOverwrite(plan) = existing else {
            panic!("existing target must require confirmation");
        };
        assert_eq!(
            plan.snapshot.expected_disk_hash,
            Some(disk_content_hash(b"existing"))
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn save_as_rejects_symlink_destination() {
        use std::os::unix::fs::symlink;

        let directory =
            std::env::temp_dir().join(format!("mica-save-as-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("target"), b"keep").unwrap();
        symlink(directory.join("target"), directory.join("link")).unwrap();
        let root = WorkspaceRoot::new(&directory).unwrap();
        let mut snapshot = save_snapshot(&directory);
        assert!(inspect_save_as(&root, "link".into(), 0, &mut snapshot).is_err());
        assert_eq!(std::fs::read(directory.join("target")).unwrap(), b"keep");
        std::fs::remove_dir_all(directory).unwrap();
    }
}
