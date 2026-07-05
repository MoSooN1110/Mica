use std::{path::PathBuf, sync::atomic::Ordering};

use crate::{
    app::{
        AppEvent, AppState, BufferTab, Effect, FileOperationRequest, FileOperationResult, Focus,
        Overlay, PathAction, SidebarView,
    },
    buffer::{CharOffset, ExternalChangeOutcome, Selection},
    command::{self, Command},
    editor,
    workspace::TreeEntryKind,
};

impl AppState {
    pub fn update(&mut self, event: AppEvent) -> Vec<Effect> {
        match event {
            AppEvent::Command(command) => self.execute(command),
            AppEvent::FileOpened {
                path,
                line,
                column,
                result,
            } => {
                let restored_cursor = self.restore_cursors.remove(&path);
                match result {
                    Ok(buffer) => {
                        if let Some(existing) = self
                            .tabs
                            .iter()
                            .position(|tab| tab.buffer.path() == Some(path.as_path()))
                        {
                            self.active_tab = Some(existing);
                        } else {
                            self.tabs.push(BufferTab {
                                buffer,
                                view: Default::default(),
                                highlights: Vec::new(),
                                syntax_generation: 0,
                            });
                            self.active_tab = Some(self.tabs.len() - 1);
                        }
                        if let Some(tab) =
                            self.active_tab.and_then(|index| self.tabs.get_mut(index))
                        {
                            let offset = restored_cursor.unwrap_or_else(|| {
                                let target_line = line
                                    .unwrap_or(1)
                                    .saturating_sub(1)
                                    .min(tab.buffer.text().len_lines().saturating_sub(1));
                                let line_start = tab.buffer.text().line_to_char(target_line);
                                let line_len = tab.buffer.text().line(target_line).len_chars();
                                let target_column =
                                    column.unwrap_or(1).saturating_sub(1).min(line_len);
                                line_start + target_column
                            });
                            tab.buffer
                                .set_selection(Selection::caret(CharOffset(offset)));
                        }
                        if let Some(preferred) = &self.preferred_active_path
                            && let Some(index) = self.tabs.iter().position(|tab| {
                                tab.buffer.path().is_some_and(|path| path == preferred)
                            })
                        {
                            self.active_tab = Some(index);
                        }
                        if self.overlay.is_none() {
                            self.focus = Focus::Editor;
                        }
                        self.notification = None;
                    }
                    Err(error) => self.notification = Some(error),
                }
                self.active_syntax_effect().into_iter().collect()
            }
            AppEvent::TreeLoaded(result) => {
                match result {
                    Ok(tree) => {
                        let selected = self
                            .tree
                            .visible_entry(self.tree_selected)
                            .map(|entry| entry.relative_path.clone());
                        let mut tree = tree;
                        tree.preserve_expansion_from(&self.tree);
                        self.tree = tree;
                        self.tree_selected = selected
                            .and_then(|path| {
                                self.tree
                                    .visible_entries()
                                    .position(|entry| entry.relative_path == path)
                            })
                            .unwrap_or(0);
                    }
                    Err(error) => self.notification = Some(error),
                }
                Vec::new()
            }
            AppEvent::SaveCompleted {
                tab,
                snapshot,
                result,
            } => {
                self.saving_tabs.remove(&tab);
                let saved = match result {
                    Ok(()) => {
                        if let Some(tab) = self.tabs.get_mut(tab) {
                            tab.buffer.complete_save(&snapshot);
                            self.notification = Some(format!("Saved {}", snapshot.path.display()));
                        }
                        true
                    }
                    Err(error) => {
                        self.notification = Some(error);
                        self.pending_saves.remove(&tab);
                        false
                    }
                };
                if saved && let Some(mut next) = self.pending_saves.remove(&tab) {
                    next.expected_disk_hash = Some(snapshot.disk_hash);
                    self.saving_tabs.insert(tab);
                    vec![Effect::Save {
                        tab,
                        snapshot: next,
                    }]
                } else {
                    Vec::new()
                }
            }
            AppEvent::WorkspaceChanged(_paths) => {
                let files = self
                    .tabs
                    .iter()
                    .enumerate()
                    .filter_map(|(index, tab)| {
                        tab.buffer.path().map(|path| (index, path.to_path_buf()))
                    })
                    .collect();
                vec![
                    Effect::ScanWorkspace,
                    Effect::RefreshOpenFiles {
                        files,
                        read_only: self.force_read_only,
                    },
                ]
            }
            AppEvent::ExternalFilesRead(reads) => {
                let mut active_reloaded = false;
                for read in reads {
                    let Some(tab) = self.tabs.get_mut(read.tab) else {
                        continue;
                    };
                    if tab.buffer.path() != Some(read.path.as_path()) {
                        continue;
                    }
                    match read.result {
                        Ok(fresh) => {
                            let outcome = tab.buffer.reconcile_external(
                                fresh,
                                self.settings.editor.auto_reload_unmodified,
                            );
                            active_reloaded |= outcome == ExternalChangeOutcome::Reloaded
                                && self.active_tab == Some(read.tab);
                            let name = read.path.file_name().map_or_else(
                                || read.path.display().to_string(),
                                |name| name.to_string_lossy().into_owned(),
                            );
                            self.notification = match outcome {
                                ExternalChangeOutcome::Unchanged => self.notification.take(),
                                ExternalChangeOutcome::Reloaded => {
                                    Some(format!("Reloaded externally changed {name}"))
                                }
                                ExternalChangeOutcome::Conflict => Some(format!(
                                    "External conflict in {name}; local text was preserved"
                                )),
                                ExternalChangeOutcome::Deleted => Some(format!(
                                    "{name} was deleted externally; buffer was preserved"
                                )),
                            };
                        }
                        Err(error) => self.notification = Some(error),
                    }
                }
                if active_reloaded {
                    self.active_syntax_effect().into_iter().collect()
                } else {
                    Vec::new()
                }
            }
            AppEvent::DeleteInspected(result) => {
                match result {
                    Ok(plan) => {
                        let absolute = plan.path().absolute();
                        let dirty_buffers = self
                            .tabs
                            .iter()
                            .filter(|tab| {
                                tab.buffer.is_dirty()
                                    && tab
                                        .buffer
                                        .path()
                                        .is_some_and(|path| path.starts_with(&absolute))
                            })
                            .count();
                        self.overlay = Some(Overlay::ConfirmDelete {
                            plan,
                            dirty_buffers,
                        });
                        self.focus = Focus::Overlay;
                    }
                    Err(error) => self.notification = Some(error),
                }
                Vec::new()
            }
            AppEvent::FileOperationCompleted(result) => {
                let mut effects = vec![Effect::ScanWorkspace];
                match result {
                    Ok(FileOperationResult::Created { path, directory }) => {
                        self.notification = Some(format!("Created {}", path.display()));
                        if !directory {
                            effects.push(Effect::OpenFile {
                                path,
                                read_only: self.force_read_only,
                                line: None,
                                column: None,
                            });
                        }
                    }
                    Ok(FileOperationResult::Moved {
                        source,
                        destination,
                    }) => {
                        for tab in &mut self.tabs {
                            let Some(path) = tab.buffer.path().map(ToOwned::to_owned) else {
                                continue;
                            };
                            if let Ok(suffix) = path.strip_prefix(&source) {
                                tab.buffer.set_path(destination.join(suffix));
                            }
                        }
                        self.notification = Some(format!(
                            "Moved {} to {}",
                            source.display(),
                            destination.display()
                        ));
                    }
                    Ok(FileOperationResult::Deleted(path)) => {
                        for tab in &mut self.tabs {
                            if tab
                                .buffer
                                .path()
                                .is_some_and(|buffer_path| buffer_path.starts_with(&path))
                            {
                                tab.buffer.reconcile_external(None, false);
                            }
                        }
                        self.notification = Some(format!(
                            "Deleted {}; open buffers were preserved",
                            path.display()
                        ));
                    }
                    Err(error) => self.notification = Some(error),
                }
                effects
            }
            AppEvent::FileSearchCompleted {
                generation,
                matches,
            } => {
                if generation == self.file_search_generation
                    && matches!(self.overlay, Some(Overlay::FilePicker))
                {
                    self.file_matches = matches;
                    self.file_picker_selected = self
                        .file_picker_selected
                        .min(self.file_matches.len().saturating_sub(1));
                    self.file_preview_path = None;
                    self.file_preview_lines.clear();
                    return self.selected_preview_effect().into_iter().collect();
                }
                Vec::new()
            }
            AppEvent::FilePreviewLoaded {
                generation,
                path,
                result,
            } => {
                if generation == self.file_search_generation
                    && self
                        .file_matches
                        .get(self.file_picker_selected)
                        .is_some_and(|matched| self.requested_file(&matched.path) == path)
                {
                    self.file_preview_path = Some(path);
                    self.file_preview_lines = result.unwrap_or_else(|error| vec![error]);
                }
                Vec::new()
            }
            AppEvent::BufferSearchCompleted {
                tab,
                buffer_generation,
                search_generation,
                query,
                matches,
            } => {
                let Some(tab_state) = self.tabs.get_mut(tab) else {
                    return Vec::new();
                };
                if tab_state.buffer.generation() != buffer_generation
                    || tab_state.view.search.generation != search_generation
                    || !matches!(self.overlay, Some(Overlay::BufferSearch { tab: open }) if open == tab)
                {
                    return Vec::new();
                }
                let cursor = tab_state.buffer.selection().head.0;
                let current = matches
                    .iter()
                    .position(|range| range.start >= cursor)
                    .unwrap_or(0);
                tab_state.view.search.query = query;
                tab_state.view.search.matches = matches;
                tab_state.view.search.current = current;
                self.select_search_match(tab, 0);
                Vec::new()
            }
            AppEvent::SyntaxHighlighted {
                tab,
                buffer_generation,
                syntax_generation,
                result,
            } => {
                let Some(tab) = self.tabs.get_mut(tab) else {
                    return Vec::new();
                };
                if tab.buffer.generation() != buffer_generation
                    || tab.syntax_generation != syntax_generation
                {
                    return Vec::new();
                }
                match result {
                    Ok(highlights) => tab.highlights = highlights,
                    Err(error) => {
                        tab.highlights.clear();
                        self.notification = Some(error);
                    }
                }
                Vec::new()
            }
            AppEvent::RecoveryDiscarded(result) => {
                if let Err(error) = result {
                    self.notification = Some(error);
                }
                Vec::new()
            }
        }
    }

    fn execute(&mut self, command: Command) -> Vec<Effect> {
        if self.overlay.is_some() {
            return self.execute_overlay(command);
        }
        match command {
            Command::Invoke(id) => self.invoke(&id),
            Command::InsertText(text) => {
                self.edit(|tab| tab.buffer.insert(&text));
                self.active_syntax_effect().into_iter().collect()
            }
            Command::InsertNewline => {
                self.edit(|tab| {
                    let newline = if tab.buffer.line_ending() == crate::buffer::LineEnding::CrLf {
                        "\r\n"
                    } else {
                        "\n"
                    };
                    tab.buffer.insert(newline)
                });
                self.active_syntax_effect().into_iter().collect()
            }
            Command::DeleteBackward => {
                self.edit(|tab| tab.buffer.delete_backward());
                self.active_syntax_effect().into_iter().collect()
            }
            Command::MoveLeft { extend } => {
                self.move_horizontal(false, extend);
                Vec::new()
            }
            Command::MoveRight { extend } => {
                self.move_horizontal(true, extend);
                Vec::new()
            }
            Command::MoveUp { extend } => {
                self.move_vertical(false, extend);
                Vec::new()
            }
            Command::MoveDown { extend } => {
                self.move_vertical(true, extend);
                Vec::new()
            }
            Command::OpenFile(path) => vec![Effect::OpenFile {
                path,
                read_only: self.force_read_only,
                line: None,
                column: None,
            }],
            Command::SelectTree(index) => {
                self.tree_selected = index.min(self.tree.visible_len().saturating_sub(1));
                Vec::new()
            }
            Command::ToggleTree(index) => {
                self.tree.toggle_visible_directory(index);
                self.tree_selected = index.min(self.tree.visible_len().saturating_sub(1));
                Vec::new()
            }
            Command::SelectTab(index) => {
                if index < self.tabs.len() {
                    self.active_tab = Some(index);
                    self.focus = Focus::Editor;
                }
                self.active_syntax_effect().into_iter().collect()
            }
            Command::CloseTab(index) => {
                self.request_close_tab(index);
                Vec::new()
            }
            Command::SetCursor {
                char_offset,
                extend,
            } => {
                if let Some(tab) = self.active_tab.and_then(|index| self.tabs.get_mut(index)) {
                    let selection = tab.buffer.selection();
                    tab.buffer.set_selection(Selection {
                        anchor: if extend {
                            selection.anchor
                        } else {
                            CharOffset(char_offset)
                        },
                        head: CharOffset(char_offset),
                    });
                    tab.view.preferred_display_column = None;
                    self.focus = Focus::Editor;
                }
                if let Some(index) = self.active_tab {
                    self.reveal_cursor(index);
                }
                Vec::new()
            }
            Command::Cancel => Vec::new(),
            Command::Resize(width, height) => {
                self.terminal_size = (width, height);
                Vec::new()
            }
            Command::PaletteInput(_)
            | Command::PaletteBackspace
            | Command::PaletteAccept
            | Command::SearchNext
            | Command::SearchPrevious
            | Command::RecoveryRecover
            | Command::RecoveryDiscard
            | Command::RecoveryLater => Vec::new(),
        }
    }

    fn execute_overlay(&mut self, command: Command) -> Vec<Effect> {
        match command {
            Command::RecoveryRecover if matches!(self.overlay, Some(Overlay::RecoveryPrompt)) => {
                return self.recover_pending();
            }
            Command::RecoveryDiscard if matches!(self.overlay, Some(Overlay::RecoveryPrompt)) => {
                self.pending_recovery.clear();
                self.overlay = None;
                self.focus = Focus::Editor;
                let paths = std::mem::take(&mut self.recovery_journals);
                self.notification = Some("Discarded crash recovery data".to_owned());
                return (!paths.is_empty())
                    .then_some(Effect::DiscardRecovery(paths))
                    .into_iter()
                    .collect();
            }
            Command::RecoveryLater if matches!(self.overlay, Some(Overlay::RecoveryPrompt)) => {
                self.overlay = None;
                self.focus = Focus::Editor;
                self.notification = Some("Recovery data kept for later".to_owned());
            }
            Command::PaletteInput(character) => {
                self.palette_query.push(character);
                if matches!(self.overlay, Some(Overlay::FilePicker)) {
                    return vec![self.start_file_search()];
                }
                if let Some(Overlay::BufferSearch { tab }) = self.overlay.as_ref() {
                    return self.start_buffer_search(*tab).into_iter().collect();
                }
            }
            Command::PaletteBackspace => {
                self.palette_query.pop();
                if matches!(self.overlay, Some(Overlay::FilePicker)) {
                    return vec![self.start_file_search()];
                }
                if let Some(Overlay::BufferSearch { tab }) = self.overlay.as_ref() {
                    return self.start_buffer_search(*tab).into_iter().collect();
                }
            }
            Command::SearchNext => {
                if let Some(Overlay::BufferSearch { tab }) = self.overlay.as_ref() {
                    self.select_search_match(*tab, 1);
                }
            }
            Command::SearchPrevious => {
                if let Some(Overlay::BufferSearch { tab }) = self.overlay.as_ref() {
                    self.select_search_match(*tab, -1);
                }
            }
            Command::MoveUp { .. } if matches!(self.overlay, Some(Overlay::FilePicker)) => {
                self.file_picker_selected = self.file_picker_selected.saturating_sub(1);
                return self.selected_preview_effect().into_iter().collect();
            }
            Command::MoveDown { .. } if matches!(self.overlay, Some(Overlay::FilePicker)) => {
                self.file_picker_selected =
                    (self.file_picker_selected + 1).min(self.file_matches.len().saturating_sub(1));
                return self.selected_preview_effect().into_iter().collect();
            }
            Command::PaletteAccept => {
                let overlay = self.overlay.take();
                match overlay {
                    Some(Overlay::CommandPalette) => {
                        let selected = self
                            .palette_commands()
                            .first()
                            .map(|command| command.id.to_owned());
                        self.palette_query.clear();
                        if let Some(id) = selected {
                            return self.invoke(&id);
                        }
                    }
                    Some(Overlay::FilePicker) => {
                        let selected = self
                            .file_matches
                            .get(self.file_picker_selected)
                            .map(|matched| self.requested_file(&matched.path));
                        self.palette_query.clear();
                        self.file_matches.clear();
                        self.file_preview_path = None;
                        self.file_preview_lines.clear();
                        self.focus = Focus::Editor;
                        if let Some(path) = selected {
                            return vec![Effect::OpenFile {
                                path,
                                read_only: self.force_read_only,
                                line: None,
                                column: None,
                            }];
                        }
                    }
                    Some(Overlay::BufferSearch { tab }) => {
                        self.overlay = Some(Overlay::BufferSearch { tab });
                        self.select_search_match(tab, 1);
                        return Vec::new();
                    }
                    Some(Overlay::PathInput(action)) => {
                        let path = PathBuf::from(self.palette_query.trim());
                        self.palette_query.clear();
                        if path.as_os_str().is_empty() {
                            self.notification = Some("Path cannot be empty".to_owned());
                            self.focus = Focus::Editor;
                            return Vec::new();
                        }
                        self.focus = Focus::Editor;
                        let request = match action {
                            PathAction::CreateFile => FileOperationRequest::CreateFile(path),
                            PathAction::CreateDirectory => {
                                FileOperationRequest::CreateDirectory(path)
                            }
                            PathAction::Move { source } => FileOperationRequest::Move {
                                source,
                                destination: path,
                            },
                        };
                        return vec![Effect::FileOperation(request)];
                    }
                    Some(Overlay::ConfirmDelete { plan, .. }) => {
                        self.palette_query.clear();
                        self.focus = Focus::Editor;
                        return vec![Effect::FileOperation(FileOperationRequest::Delete(plan))];
                    }
                    Some(Overlay::ConfirmClose { tab }) => {
                        self.palette_query.clear();
                        self.focus = Focus::Editor;
                        self.close_tab(tab);
                    }
                    Some(Overlay::RecoveryPrompt) => {
                        self.overlay = Some(Overlay::RecoveryPrompt);
                    }
                    None => {}
                }
            }
            Command::Cancel => {
                if let Some(Overlay::BufferSearch { tab }) = self.overlay.as_ref()
                    && let Some(tab) = self.tabs.get_mut(*tab)
                {
                    tab.view.search.matches.clear();
                    tab.view.search.query.clear();
                    tab.view.search.current = 0;
                }
                self.overlay = None;
                self.palette_query.clear();
                self.file_search_generation = self.file_search_generation.saturating_add(1);
                self.file_search_cancellation
                    .store(self.file_search_generation, Ordering::Relaxed);
                self.file_matches.clear();
                self.file_preview_path = None;
                self.file_preview_lines.clear();
                self.focus = Focus::Editor;
            }
            _ => {}
        }
        Vec::new()
    }

    fn invoke(&mut self, id: &str) -> Vec<Effect> {
        match id {
            command::APP_QUIT => {
                if self.tabs.iter().any(|tab| tab.buffer.is_dirty()) {
                    self.notification =
                        Some("Unsaved changes: save or close them before quitting".to_owned());
                } else {
                    self.should_quit = true;
                }
                Vec::new()
            }
            command::EDITOR_SAVE => {
                let Some(index) = self.active_tab else {
                    return Vec::new();
                };
                match self.tabs[index].buffer.prepare_save() {
                    Ok(snapshot) => {
                        if self.saving_tabs.contains(&index) {
                            self.pending_saves.insert(index, snapshot);
                            self.notification = Some("Save queued".to_owned());
                            Vec::new()
                        } else {
                            self.saving_tabs.insert(index);
                            vec![Effect::Save {
                                tab: index,
                                snapshot,
                            }]
                        }
                    }
                    Err(error) => {
                        self.notification = Some(error.to_string());
                        Vec::new()
                    }
                }
            }
            command::EDITOR_CLOSE => {
                if let Some(index) = self.active_tab {
                    self.request_close_tab(index);
                }
                Vec::new()
            }
            command::EDITOR_COPY => {
                let Some(text) = self.active_tab().and_then(|tab| tab.buffer.selected_text())
                else {
                    self.notification = Some("Select text to copy".to_owned());
                    return Vec::new();
                };
                self.internal_clipboard = text.clone();
                self.notification = Some("Copied selection".to_owned());
                vec![Effect::CopyToClipboard(text)]
            }
            command::EDITOR_CUT => {
                let Some(index) = self.active_tab else {
                    return Vec::new();
                };
                let Some(text) = self.tabs[index].buffer.selected_text() else {
                    self.notification = Some("Select text to cut".to_owned());
                    return Vec::new();
                };
                match self.tabs[index].buffer.delete_selection() {
                    Ok(true) => {
                        self.internal_clipboard = text.clone();
                        self.notification = Some("Cut selection".to_owned());
                        let mut effects = vec![Effect::CopyToClipboard(text)];
                        effects.extend(self.active_syntax_effect());
                        effects
                    }
                    Ok(false) => Vec::new(),
                    Err(error) => {
                        self.notification = Some(error.to_string());
                        Vec::new()
                    }
                }
            }
            command::EDITOR_PASTE => {
                if self.internal_clipboard.is_empty() {
                    self.notification = Some("Internal clipboard is empty".to_owned());
                } else {
                    let text = self.internal_clipboard.clone();
                    self.edit(|tab| tab.buffer.insert(&text));
                }
                self.active_syntax_effect().into_iter().collect()
            }
            command::EDITOR_FIND => {
                let Some(tab) = self.active_tab else {
                    return Vec::new();
                };
                self.overlay = Some(Overlay::BufferSearch { tab });
                self.palette_query.clear();
                self.tabs[tab].view.search = Default::default();
                self.focus = Focus::Overlay;
                Vec::new()
            }
            command::EDITOR_UNDO => {
                self.edit(|tab| tab.buffer.undo().map(|_| ()));
                self.active_syntax_effect().into_iter().collect()
            }
            command::EDITOR_REDO => {
                self.edit(|tab| tab.buffer.redo().map(|_| ()));
                self.active_syntax_effect().into_iter().collect()
            }
            command::COMMAND_PALETTE_OPEN => {
                self.overlay = Some(Overlay::CommandPalette);
                self.palette_query.clear();
                self.focus = Focus::Overlay;
                Vec::new()
            }
            command::FILE_NEW => {
                self.open_path_input(PathAction::CreateFile, String::new());
                Vec::new()
            }
            command::FILE_NEW_DIRECTORY => {
                self.open_path_input(PathAction::CreateDirectory, String::new());
                Vec::new()
            }
            command::FILE_RENAME | command::FILE_MOVE => {
                let Some(source) = self.selected_tree_path() else {
                    self.notification = Some("Select a file or directory first".to_owned());
                    return Vec::new();
                };
                let initial = source.to_string_lossy().into_owned();
                self.open_path_input(PathAction::Move { source }, initial);
                Vec::new()
            }
            command::FILE_DELETE => {
                let Some(path) = self.selected_tree_path() else {
                    self.notification = Some("Select a file or directory first".to_owned());
                    return Vec::new();
                };
                vec![Effect::InspectDelete(path)]
            }
            command::VIEW_TOGGLE_SIDEBAR => {
                self.sidebar_visible = !self.sidebar_visible;
                Vec::new()
            }
            command::VIEW_TOGGLE_BOTTOM_PANEL => {
                self.bottom_panel_visible = !self.bottom_panel_visible;
                Vec::new()
            }
            command::VIEW_EXPLORER => {
                self.sidebar_visible = true;
                self.sidebar_view = SidebarView::Explorer;
                self.focus = Focus::Sidebar;
                Vec::new()
            }
            command::VIEW_SOURCE_CONTROL => {
                self.sidebar_visible = true;
                self.sidebar_view = SidebarView::SourceControl;
                self.focus = Focus::Sidebar;
                Vec::new()
            }
            command::VIEW_SEARCH => {
                self.sidebar_visible = true;
                self.sidebar_view = SidebarView::Search;
                self.focus = Focus::Sidebar;
                Vec::new()
            }
            command::WORKSPACE_REFRESH => vec![Effect::ScanWorkspace],
            command::WORKSPACE_OPEN_FILE => {
                self.overlay = Some(Overlay::FilePicker);
                self.palette_query.clear();
                self.file_matches.clear();
                self.file_preview_path = None;
                self.file_preview_lines.clear();
                self.file_picker_selected = 0;
                self.focus = Focus::Overlay;
                vec![self.start_file_search()]
            }
            _ => {
                self.notification = Some(format!("Unknown command: {id}"));
                Vec::new()
            }
        }
    }

    fn edit(
        &mut self,
        operation: impl FnOnce(&mut BufferTab) -> Result<(), crate::buffer::BufferError>,
    ) -> bool {
        let Some(tab) = self.active_tab.and_then(|index| self.tabs.get_mut(index)) else {
            return false;
        };
        if let Err(error) = operation(tab) {
            self.notification = Some(error.to_string());
            false
        } else {
            self.reveal_cursor(self.active_tab.unwrap_or(0));
            true
        }
    }

    fn move_horizontal(&mut self, right: bool, extend: bool) {
        let Some(tab) = self.active_tab.and_then(|index| self.tabs.get_mut(index)) else {
            return;
        };
        let selection = tab.buffer.selection();
        let next = if right {
            editor::move_right(tab.buffer.text(), selection.head.0)
        } else {
            editor::move_left(tab.buffer.text(), selection.head.0)
        };
        tab.buffer.set_selection(Selection {
            anchor: if extend {
                selection.anchor
            } else {
                CharOffset(next)
            },
            head: CharOffset(next),
        });
        tab.view.preferred_display_column = None;
        let _ = tab;
        if let Some(index) = self.active_tab {
            self.reveal_cursor(index);
        }
    }

    fn move_vertical(&mut self, down: bool, extend: bool) {
        let tab_width = usize::from(self.settings.editor.tab_width);
        let Some(tab) = self.active_tab.and_then(|index| self.tabs.get_mut(index)) else {
            return;
        };
        let selection = tab.buffer.selection();
        let (next, preferred) = if down {
            editor::move_down(
                tab.buffer.text(),
                selection.head.0,
                tab.view.preferred_display_column,
                tab_width,
            )
        } else {
            editor::move_up(
                tab.buffer.text(),
                selection.head.0,
                tab.view.preferred_display_column,
                tab_width,
            )
        };
        tab.buffer.set_selection(Selection {
            anchor: if extend {
                selection.anchor
            } else {
                CharOffset(next)
            },
            head: CharOffset(next),
        });
        tab.view.preferred_display_column = Some(preferred);
        let _ = tab;
        if let Some(index) = self.active_tab {
            self.reveal_cursor(index);
        }
    }

    fn selected_tree_path(&self) -> Option<PathBuf> {
        self.tree
            .visible_entry(self.tree_selected)
            .map(|entry| entry.relative_path.clone())
    }

    fn open_path_input(&mut self, action: PathAction, initial: String) {
        self.overlay = Some(Overlay::PathInput(action));
        self.palette_query = initial;
        self.focus = Focus::Overlay;
    }

    fn start_file_search(&mut self) -> Effect {
        self.file_search_generation = self.file_search_generation.saturating_add(1);
        self.file_search_cancellation
            .store(self.file_search_generation, Ordering::Relaxed);
        self.file_picker_selected = 0;
        let paths = self
            .tree
            .entries
            .iter()
            .filter(|entry| entry.kind != TreeEntryKind::Directory)
            .map(|entry| entry.relative_path.clone())
            .collect();
        Effect::FuzzyFiles {
            generation: self.file_search_generation,
            query: self.palette_query.clone(),
            paths,
            cancellation: self.file_search_cancellation.clone(),
        }
    }

    fn selected_preview_effect(&self) -> Option<Effect> {
        self.file_matches
            .get(self.file_picker_selected)
            .map(|matched| Effect::PreviewFile {
                generation: self.file_search_generation,
                path: self.requested_file(&matched.path),
            })
    }

    fn active_syntax_effect(&mut self) -> Option<Effect> {
        let tab_index = self.active_tab?;
        let tab = self.tabs.get_mut(tab_index)?;
        let is_rust = tab
            .buffer
            .path()
            .and_then(|path| path.extension())
            .is_some_and(|extension| extension == "rs");
        let threshold = self
            .settings
            .editor
            .large_file_threshold_mb
            .saturating_mul(1024 * 1024);
        if !is_rust || u64::try_from(tab.buffer.text().len_bytes()).unwrap_or(u64::MAX) > threshold
        {
            tab.highlights.clear();
            return None;
        }
        let generation = self
            .syntax_cancellation
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        self.syntax_cancellation
            .store(generation, Ordering::Relaxed);
        tab.syntax_generation = generation;
        Some(Effect::HighlightRust {
            tab: tab_index,
            buffer_generation: tab.buffer.generation(),
            syntax_generation: generation,
            source: tab.buffer.text().clone(),
            cancellation: self.syntax_cancellation.clone(),
        })
    }

    fn recover_pending(&mut self) -> Vec<Effect> {
        for recovered in std::mem::take(&mut self.pending_recovery) {
            let mut buffer = crate::buffer::TextBuffer::recovered(
                recovered.path.clone(),
                &recovered.text,
                recovered.expected_disk_hash,
                recovered.has_bom,
                recovered.line_ending,
                self.force_read_only,
            );
            buffer.set_selection(Selection::caret(CharOffset(recovered.cursor_char)));
            let recovered_tab = BufferTab {
                buffer,
                view: Default::default(),
                highlights: Vec::new(),
                syntax_generation: 0,
            };
            if let Some(index) = recovered.path.as_deref().and_then(|path| {
                self.tabs
                    .iter()
                    .position(|tab| tab.buffer.path() == Some(path))
            }) {
                self.tabs[index] = recovered_tab;
                self.active_tab = Some(index);
            } else {
                self.tabs.push(recovered_tab);
                self.active_tab = Some(self.tabs.len() - 1);
            }
        }
        self.overlay = None;
        self.focus = Focus::Editor;
        self.notification = Some("Recovered unsaved buffers after abnormal exit".to_owned());
        let paths = std::mem::take(&mut self.recovery_journals);
        let mut effects = Vec::new();
        if !paths.is_empty() {
            effects.push(Effect::DiscardRecovery(paths));
        }
        effects.extend(self.active_syntax_effect());
        effects
    }

    fn start_buffer_search(&mut self, tab_index: usize) -> Option<Effect> {
        let tab = self.tabs.get_mut(tab_index)?;
        let generation = self
            .buffer_search_cancellation
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        self.buffer_search_cancellation
            .store(generation, Ordering::Relaxed);
        tab.view.search.generation = generation;
        tab.view.search.query = self.palette_query.clone();
        tab.view.search.matches.clear();
        tab.view.search.current = 0;
        if self.palette_query.is_empty() {
            return None;
        }
        Some(Effect::SearchBuffer {
            tab: tab_index,
            buffer_generation: tab.buffer.generation(),
            search_generation: generation,
            query: self.palette_query.clone(),
            source: tab.buffer.text().clone(),
            cancellation: self.buffer_search_cancellation.clone(),
        })
    }

    fn select_search_match(&mut self, tab_index: usize, direction: isize) {
        let Some(tab) = self.tabs.get_mut(tab_index) else {
            return;
        };
        let count = tab.view.search.matches.len();
        if count == 0 {
            return;
        }
        tab.view.search.current = if direction > 0 {
            (tab.view.search.current + 1) % count
        } else if direction < 0 {
            tab.view.search.current.checked_sub(1).unwrap_or(count - 1)
        } else {
            tab.view.search.current.min(count - 1)
        };
        let range = tab.view.search.matches[tab.view.search.current].clone();
        tab.buffer.set_selection(Selection {
            anchor: CharOffset(range.start),
            head: CharOffset(range.end),
        });
        self.reveal_cursor(tab_index);
    }

    fn request_close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        if !self.saving_tabs.is_empty() {
            self.notification = Some("Wait for pending saves before closing a tab".to_owned());
            return;
        }
        if self.tabs[index].buffer.is_dirty() {
            self.overlay = Some(Overlay::ConfirmClose { tab: index });
            self.focus = Focus::Overlay;
        } else {
            self.close_tab(index);
        }
    }

    fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() || !self.saving_tabs.is_empty() {
            return;
        }
        self.tabs.remove(index);
        self.active_tab = match self.active_tab {
            None => None,
            Some(_) if self.tabs.is_empty() => None,
            Some(active) if active > index => Some(active - 1),
            Some(active) if active == index => Some(index.min(self.tabs.len() - 1)),
            Some(active) => Some(active),
        };
    }

    fn reveal_cursor(&mut self, tab_index: usize) {
        let (_, terminal_height) = self.terminal_size;
        let panel_height = if self.bottom_panel_visible {
            self.settings
                .ui
                .bottom_panel_height
                .min(terminal_height.saturating_sub(2) / 2)
        } else {
            0
        };
        let visible_height = usize::from(
            terminal_height
                .saturating_sub(2)
                .saturating_sub(panel_height)
                .max(1),
        );
        let Some(tab) = self.tabs.get_mut(tab_index) else {
            return;
        };
        let cursor_line = tab.buffer.text().char_to_line(
            tab.buffer
                .selection()
                .head
                .0
                .min(tab.buffer.text().len_chars()),
        );
        if cursor_line < tab.view.scroll_line {
            tab.view.scroll_line = cursor_line;
        } else if cursor_line >= tab.view.scroll_line + visible_height {
            tab.view.scroll_line = cursor_line.saturating_sub(visible_height - 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::session::{RecoveryBuffer, RecoverySet};
    use crate::{buffer::TextBuffer, config::Keymap, workspace::WorkspaceRoot};

    fn state() -> AppState {
        let root = std::env::temp_dir().join(format!("mica-app-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        AppState::new(
            WorkspaceRoot::new(root).unwrap(),
            Default::default(),
            Keymap::default(),
            Vec::new(),
            false,
        )
    }

    #[test]
    fn dirty_tab_requires_explicit_close_confirmation() {
        let mut state = state();
        let mut buffer = TextBuffer::empty(None, false);
        buffer.insert("unsaved").unwrap();
        state.tabs.push(BufferTab {
            buffer,
            view: Default::default(),
            highlights: Vec::new(),
            syntax_generation: 0,
        });
        state.active_tab = Some(0);

        state.update(AppEvent::Command(Command::CloseTab(0)));
        assert!(matches!(
            state.overlay,
            Some(Overlay::ConfirmClose { tab: 0 })
        ));
        assert_eq!(state.tabs.len(), 1);

        state.update(AppEvent::Command(Command::PaletteAccept));
        assert!(state.tabs.is_empty());
        assert_eq!(state.active_tab, None);
    }

    #[test]
    fn cut_and_paste_use_internal_clipboard() {
        let mut state = state();
        let mut buffer = TextBuffer::empty(None, false);
        buffer.insert("日本語").unwrap();
        buffer.set_selection(Selection {
            anchor: CharOffset(0),
            head: CharOffset(2),
        });
        state.tabs.push(BufferTab {
            buffer,
            view: Default::default(),
            highlights: Vec::new(),
            syntax_generation: 0,
        });
        state.active_tab = Some(0);

        let effects = state.update(AppEvent::Command(Command::Invoke(
            command::EDITOR_CUT.to_owned(),
        )));
        assert!(matches!(effects.as_slice(), [Effect::CopyToClipboard(_)]));
        assert_eq!(state.internal_clipboard, "日本");
        assert_eq!(state.tabs[0].buffer.text_string(), "語");

        state.update(AppEvent::Command(Command::Invoke(
            command::EDITOR_PASTE.to_owned(),
        )));
        assert_eq!(state.tabs[0].buffer.text_string(), "日本語");
    }

    #[test]
    fn incremental_buffer_search_selects_and_cycles_matches() {
        let mut state = state();
        let mut buffer = TextBuffer::empty(None, false);
        buffer.insert("a日 a日").unwrap();
        buffer.set_selection(Selection::caret(CharOffset(0)));
        state.tabs.push(BufferTab {
            buffer,
            view: Default::default(),
            highlights: Vec::new(),
            syntax_generation: 0,
        });
        state.active_tab = Some(0);
        state.update(AppEvent::Command(Command::Invoke(
            command::EDITOR_FIND.to_owned(),
        )));
        let effects = state.update(AppEvent::Command(Command::PaletteInput('日')));
        let [
            Effect::SearchBuffer {
                tab,
                buffer_generation,
                search_generation,
                query,
                source,
                cancellation,
            },
        ] = effects.as_slice()
        else {
            panic!("expected one buffer search effect");
        };
        let matches = crate::search::find_matches(
            &source.to_string(),
            query,
            cancellation,
            *search_generation,
            100,
        );
        state.update(AppEvent::BufferSearchCompleted {
            tab: *tab,
            buffer_generation: *buffer_generation,
            search_generation: *search_generation,
            query: query.clone(),
            matches,
        });
        assert_eq!(state.tabs[0].buffer.selection().range(), 1..2);

        state.update(AppEvent::Command(Command::SearchNext));
        assert_eq!(state.tabs[0].buffer.selection().range(), 4..5);
    }

    #[test]
    fn crash_recovery_restores_dirty_buffer_and_removes_stale_journal() {
        let mut state = state();
        let journal = std::env::temp_dir().join("stale-mica-journal.json");
        state.offer_recovery(RecoverySet {
            buffers: vec![RecoveryBuffer {
                path: Some(state.workspace.as_path().join("draft.rs")),
                text: "fn 復旧() {}".to_owned(),
                cursor_char: 3,
                expected_disk_hash: None,
                has_bom: false,
                line_ending: crate::buffer::LineEnding::Lf,
            }],
            journal_paths: vec![journal.clone()],
        });
        let effects = state.update(AppEvent::Command(Command::RecoveryRecover));
        assert_eq!(state.tabs.len(), 1);
        assert_eq!(state.tabs[0].buffer.text_string(), "fn 復旧() {}");
        assert!(state.tabs[0].buffer.is_dirty());
        assert_eq!(state.tabs[0].buffer.selection().head, CharOffset(3));
        assert!(effects.iter().any(
            |effect| matches!(effect, Effect::DiscardRecovery(paths) if paths == &vec![journal.clone()])
        ));
    }
}
