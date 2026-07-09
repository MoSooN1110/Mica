use std::{path::PathBuf, sync::atomic::Ordering, time::Instant};

use crate::{
    app::{
        AppEvent, AppState, BottomPanelView, BufferTab, ColumnHint, Effect, FileOperationRequest,
        FileOperationResult, Focus, Overlay, PathAction, SaveAsInspection, SaveAsPlan, SidebarView,
    },
    buffer::{CharOffset, ExternalChangeOutcome, Selection},
    command::{self, Command},
    editor,
    workspace::TreeEntryKind,
};

impl AppState {
    pub fn update(&mut self, event: AppEvent) -> Vec<Effect> {
        let previous_notification = self.notification.clone();
        let effects = self.update_inner(event);
        if self.notification != previous_notification
            && let Some(message) = self.notification.clone()
        {
            self.record_notification(message);
        }
        effects
    }

    fn update_inner(&mut self, event: AppEvent) -> Vec<Effect> {
        match event {
            AppEvent::Tick => {
                self.expire_notification();
                Vec::new()
            }
            AppEvent::Command(command) => self.execute(command),
            AppEvent::FileOpened {
                path,
                line,
                column,
                result,
            } => {
                let restored_cursor = self.restore_cursors.remove(&path);
                let restored_pin = self.restore_pins.remove(&path);
                match result {
                    Ok(buffer) => {
                        let recent_path = path
                            .strip_prefix(self.workspace.as_path())
                            .unwrap_or(path.as_path())
                            .to_path_buf();
                        self.recent_files.retain(|item| item != &recent_path);
                        self.recent_files.insert(0, recent_path);
                        self.recent_files.truncate(100);
                        self.git_diff_active = false;
                        if let Some(existing) = self
                            .tabs
                            .iter()
                            .position(|tab| tab.buffer.path() == Some(path.as_path()))
                        {
                            self.active_tab = Some(existing);
                        } else {
                            self.tabs.push(BufferTab::new(buffer));
                            self.active_tab = Some(self.tabs.len() - 1);
                        }
                        if let Some(tab) =
                            self.active_tab.and_then(|index| self.tabs.get_mut(index))
                        {
                            if let Some(pinned) = restored_pin {
                                tab.pinned = pinned;
                            }
                            let offset = restored_cursor.unwrap_or_else(|| {
                                let target_line = line
                                    .unwrap_or(1)
                                    .saturating_sub(1)
                                    .min(tab.buffer.text().len_lines().saturating_sub(1));
                                let line_start = tab.buffer.text().line_to_char(target_line);
                                let line_text = tab.buffer.text().line(target_line).to_string();
                                let line_len = line_text.chars().count();
                                let target_column = match column {
                                    Some(ColumnHint::Chars(value)) => {
                                        value.saturating_sub(1).min(line_len)
                                    }
                                    Some(ColumnHint::Utf16(value)) => {
                                        crate::lsp::utf16_column_to_char(
                                            &line_text,
                                            value.saturating_sub(1),
                                        )
                                        .min(line_len)
                                    }
                                    None => 0,
                                };
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
                        self.notification = self
                            .active_tab()
                            .filter(|tab| self.is_large_buffer(&tab.buffer))
                            .map(|_| {
                                "Large-file mode: syntax highlighting, LSP, and diff decorations are disabled"
                                    .to_owned()
                            });
                    }
                    Err(error) => self.notification = Some(error),
                }
                let mut effects = self.active_syntax_effect().into_iter().collect::<Vec<_>>();
                effects.extend(self.lsp_open_effect(&path));
                effects
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
                } else if saved {
                    let mut effects = vec![Effect::RefreshGit];
                    if let Some(effect) = self.lsp_save_effect(&snapshot.path) {
                        effects.push(effect);
                    }
                    if self.settings.diagnostics.check_on_save {
                        effects.extend(self.start_cargo_diagnostics(false));
                    }
                    effects
                } else {
                    Vec::new()
                }
            }
            AppEvent::SaveAsInspected(result) => match result {
                Ok(SaveAsInspection::Ready(plan)) => self.start_save_as(plan),
                Ok(SaveAsInspection::ConfirmOverwrite(plan)) => {
                    self.overlay = Some(Overlay::ConfirmSaveAs { plan });
                    self.focus = Focus::Overlay;
                    Vec::new()
                }
                Err(error) => {
                    self.notification = Some(error);
                    Vec::new()
                }
            },
            AppEvent::SaveAsCompleted { plan, result } => {
                self.saving_tabs.remove(&plan.tab);
                self.save_as_tabs.remove(&plan.tab);
                match result {
                    Ok(()) => {
                        if let Some(tab) = self.tabs.get_mut(plan.tab) {
                            tab.buffer.set_path(plan.snapshot.path.clone());
                            tab.buffer.complete_save(&plan.snapshot);
                        }
                        self.notification =
                            Some(format!("Saved as {}", plan.snapshot.path.display()));
                        let mut effects = vec![Effect::ScanWorkspace, Effect::RefreshGit];
                        if self.active_tab == Some(plan.tab) {
                            effects.extend(self.active_syntax_effect());
                        }
                        effects
                    }
                    Err(error) => {
                        self.notification = Some(error);
                        Vec::new()
                    }
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
                    Effect::RefreshGit,
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
                let mut effects = vec![Effect::ScanWorkspace, Effect::RefreshGit];
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
            AppEvent::GitStatusLoaded(result) => {
                self.git_loading = false;
                match result {
                    Ok(status) => {
                        self.git_status = Some(status);
                        self.git_error = None;
                        self.git_selected = self
                            .git_selected
                            .min(self.git_entries().len().saturating_sub(1));
                    }
                    Err(error) => {
                        self.git_status = None;
                        self.git_error = Some(error);
                    }
                }
                Vec::new()
            }
            AppEvent::GitDiffLoaded(result) => {
                match result {
                    Ok(diff) => {
                        self.git_diff = Some(diff);
                        self.git_diff_active = true;
                        self.git_hunk_selected = 0;
                        self.git_diff_scroll = 0;
                        self.focus = Focus::Editor;
                    }
                    Err(error) => {
                        self.append_output("git", &error);
                        self.notification = Some(git_error_guidance(&error));
                    }
                }
                Vec::new()
            }
            AppEvent::GitOperationCompleted {
                result,
                workspace_changed,
            } => {
                let succeeded = result.is_ok();
                match result {
                    Ok(message) => self.notification = Some(message),
                    Err(error) => {
                        self.append_output("git", &error);
                        self.notification = Some(git_error_guidance(&error));
                    }
                }
                if succeeded {
                    self.git_diff = None;
                    self.git_diff_active = false;
                }
                self.git_loading = true;
                let mut effects = vec![Effect::RefreshGit];
                if workspace_changed {
                    let files = self
                        .tabs
                        .iter()
                        .enumerate()
                        .filter_map(|(index, tab)| {
                            tab.buffer.path().map(|path| (index, path.to_path_buf()))
                        })
                        .collect();
                    effects.push(Effect::ScanWorkspace);
                    effects.push(Effect::RefreshOpenFiles {
                        files,
                        read_only: self.force_read_only,
                    });
                }
                effects
            }
            AppEvent::GitBranchesLoaded(result) => {
                match result {
                    Ok(branches) => {
                        self.git_branches = branches;
                        self.git_branch_selected = self
                            .git_branch_selected
                            .min(self.git_branches.len().saturating_sub(1));
                    }
                    Err(error) => {
                        self.append_output("git", &error);
                        self.notification = Some(error);
                    }
                }
                Vec::new()
            }
            AppEvent::WorkspaceSearchBatch {
                generation,
                matches,
                done,
                error,
            } => {
                if generation != self.workspace_search_generation {
                    return Vec::new();
                }
                self.workspace_matches.extend(matches);
                self.workspace_matches.sort_by(|left, right| {
                    left.path
                        .cmp(&right.path)
                        .then(left.line.cmp(&right.line))
                        .then(left.column.cmp(&right.column))
                });
                self.workspace_search_running = !done;
                if let Some(error) = error {
                    self.append_output("search", &error);
                    self.notification = Some(error);
                }
                self.workspace_search_selected = self
                    .workspace_search_selected
                    .min(self.workspace_matches.len().saturating_sub(1));
                Vec::new()
            }
            AppEvent::TerminalStarted { generation, result } => {
                if generation != self.terminal_generation {
                    return Vec::new();
                }
                match result {
                    Ok(()) => {
                        self.terminal_started = true;
                        self.terminal_running = true;
                        self.terminal_exit = None;
                        self.notification = Some("Terminal started".to_owned());
                    }
                    Err(error) => {
                        self.terminal_started = false;
                        self.terminal_running = false;
                        self.notification = Some(error);
                    }
                }
                Vec::new()
            }
            AppEvent::TerminalOutput { generation, bytes } => {
                if generation != self.terminal_generation {
                    return Vec::new();
                }
                self.terminal.feed(&bytes);
                if matches!(self.overlay, Some(Overlay::TerminalSearch)) {
                    self.refresh_terminal_search();
                }
                Vec::new()
            }
            AppEvent::TerminalExited {
                generation,
                code,
                success,
            } => {
                if generation != self.terminal_generation {
                    return Vec::new();
                }
                self.terminal_running = false;
                self.terminal_exit = Some((code, success));
                self.notification = Some(format!("Terminal exited with code {code}"));
                Vec::new()
            }
            AppEvent::TerminalError { generation, error } => {
                if generation != self.terminal_generation {
                    return Vec::new();
                }
                self.append_output("terminal", &error);
                self.notification = Some(format!("Terminal: {error}"));
                Vec::new()
            }
            AppEvent::DiagnosticsReplaced {
                source,
                generation,
                diagnostics,
            } => {
                if !self.settings.diagnostics.enabled {
                    return self.finish_cargo_diagnostics(source);
                }
                self.diagnostics
                    .replace_source(source, generation, diagnostics);
                self.diagnostic_selected = self
                    .diagnostic_selected
                    .min(self.visible_diagnostics().len().saturating_sub(1));
                self.finish_cargo_diagnostics(source)
            }
            AppEvent::DiagnosticsFailed {
                source,
                generation,
                error,
            } => {
                self.diagnostics.clear_source(source, generation);
                if !self.settings.diagnostics.enabled {
                    return self.finish_cargo_diagnostics(source);
                }
                self.append_output("diagnostics", &error);
                self.notification = Some(error);
                self.finish_cargo_diagnostics(source)
            }
            AppEvent::OutputMessage { source, message } => {
                self.append_output(&source, message);
                Vec::new()
            }
            AppEvent::ConfigReloaded {
                settings,
                keymap,
                warnings,
            } => {
                self.settings = settings;
                self.keymap = keymap;
                self.config_warnings = warnings;
                self.notification = Some(if self.config_warnings.is_empty() {
                    "Configuration reloaded".to_owned()
                } else {
                    format!(
                        "Configuration reloaded with warnings: {}",
                        self.config_warnings.join("; ")
                    )
                });
                vec![Effect::ScanWorkspace]
            }
            AppEvent::ConfigFilePrepared(result) => match result {
                Ok(path) => {
                    self.notification = Some("Workspace configuration opened".to_owned());
                    vec![Effect::OpenFile {
                        path,
                        read_only: self.force_read_only,
                        line: None,
                        column: None,
                    }]
                }
                Err(error) => {
                    self.notification = Some(format!("Cannot open configuration: {error}"));
                    Vec::new()
                }
            },
            AppEvent::LspClient { language, event } => self.handle_lsp_event(language, event),
        }
    }

    fn execute(&mut self, command: Command) -> Vec<Effect> {
        if self.overlay.is_some() {
            return self.execute_overlay(command);
        }
        match command {
            Command::Invoke(id) => self.invoke(&id),
            Command::InsertText(text) => {
                let pair = self
                    .settings
                    .editor
                    .auto_pairs
                    .then_some(match text.as_str() {
                        "(" => Some(('(', ')')),
                        "[" => Some(('[', ']')),
                        "{" => Some(('{', '}')),
                        "\"" => Some(('"', '"')),
                        "'" => Some(('\'', '\'')),
                        _ => None,
                    })
                    .flatten();
                let closer = self
                    .settings
                    .editor
                    .auto_pairs
                    .then_some(match text.as_str() {
                        ")" => Some(')'),
                        "]" => Some(']'),
                        "}" => Some('}'),
                        "\"" => Some('"'),
                        "'" => Some('\''),
                        _ => None,
                    })
                    .flatten();
                let skipped = closer.is_some_and(|closing| {
                    self.active_tab
                        .and_then(|index| self.tabs.get_mut(index))
                        .is_some_and(|tab| tab.buffer.skip_matching_closer(closing))
                });
                if skipped {
                    // Moving over an auto-inserted closer is not an edit.
                } else if let Some((opening, closing)) = pair {
                    self.edit(|tab| tab.buffer.insert_auto_pair(opening, closing));
                } else if let Some(closing) = closer {
                    let _ = closing;
                    self.edit(|tab| tab.buffer.insert(&text));
                } else {
                    self.edit(|tab| tab.buffer.insert(&text));
                }
                self.active_post_edit_effects()
            }
            Command::InsertNewline => {
                self.edit(|tab| tab.buffer.insert_newline_with_indent());
                self.active_post_edit_effects()
            }
            Command::DeleteBackward => {
                self.edit(|tab| tab.buffer.delete_backward());
                self.active_post_edit_effects()
            }
            Command::DeleteWordBackward => {
                self.edit(|tab| {
                    if !tab.buffer.selection().is_caret() {
                        return tab.buffer.delete_selection().map(|_| ());
                    }
                    let cursor = tab.buffer.selection().head.0;
                    let start = editor::move_word_left(tab.buffer.text(), cursor);
                    tab.buffer.delete_range(start, cursor)
                });
                self.active_post_edit_effects()
            }
            Command::DeleteWordForward => {
                self.edit(|tab| {
                    if !tab.buffer.selection().is_caret() {
                        return tab.buffer.delete_selection().map(|_| ());
                    }
                    let cursor = tab.buffer.selection().head.0;
                    let end = editor::move_word_right(tab.buffer.text(), cursor);
                    tab.buffer.delete_range(cursor, end)
                });
                self.active_post_edit_effects()
            }
            Command::IndentSelection => self.change_selected_indent(false),
            Command::OutdentSelection => self.change_selected_indent(true),
            Command::ToggleLineComment => self.toggle_line_comment(),
            Command::MoveLeft { extend } => {
                self.move_horizontal(false, extend);
                Vec::new()
            }
            Command::MoveRight { extend } => {
                self.move_horizontal(true, extend);
                Vec::new()
            }
            Command::MoveWordLeft { extend } => {
                self.move_word_horizontal(false, extend);
                Vec::new()
            }
            Command::MoveWordRight { extend } => {
                self.move_word_horizontal(true, extend);
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
            Command::ClickTree(index) => {
                let index = index.min(self.tree.visible_len().saturating_sub(1));
                let now = Instant::now();
                let repeated = self.tree_last_click.is_some_and(|(last, at)| {
                    last == index && now.duration_since(at).as_millis() <= 500
                });
                self.tree_selected = index;
                self.tree_last_click = Some((index, now));
                if !repeated {
                    return Vec::new();
                }
                let Some(entry) = self.tree.visible_entry(index) else {
                    return Vec::new();
                };
                if entry.kind == TreeEntryKind::Directory {
                    self.tree.toggle_visible_directory(index);
                    self.tree_last_click = None;
                    Vec::new()
                } else {
                    let path = self.requested_file(&entry.relative_path);
                    self.tree_last_click = None;
                    vec![Effect::OpenFile {
                        path,
                        read_only: self.force_read_only,
                        line: None,
                        column: None,
                    }]
                }
            }
            Command::ToggleTree(index) => {
                self.tree.toggle_visible_directory(index);
                self.tree_selected = index.min(self.tree.visible_len().saturating_sub(1));
                self.tree_last_click = None;
                Vec::new()
            }
            Command::BeginSidebarResize => {
                self.sidebar_resize_active = true;
                Vec::new()
            }
            Command::ResizeSidebar(width) => {
                self.sidebar_visible = true;
                self.settings.ui.sidebar_width = width.clamp(16, 80);
                Vec::new()
            }
            Command::EndSidebarResize => {
                self.sidebar_resize_active = false;
                Vec::new()
            }
            Command::SelectGit(index) => {
                self.git_selected = index.min(self.git_entries().len().saturating_sub(1));
                Vec::new()
            }
            Command::StageGit(index) => {
                self.git_selected = index.min(self.git_entries().len().saturating_sub(1));
                self.selected_git_operation(true)
            }
            Command::UnstageGit(index) => {
                self.git_selected = index.min(self.git_entries().len().saturating_sub(1));
                self.selected_git_operation(false)
            }
            Command::SelectGitAndOpen(index) => {
                self.git_selected = index.min(self.git_entries().len().saturating_sub(1));
                let Some((path, target)) = self.git_entries().get(self.git_selected).cloned()
                else {
                    return Vec::new();
                };
                vec![Effect::LoadGitDiff { path, target }]
            }
            Command::OpenGitDiff => {
                let Some((path, target)) = self.git_entries().get(self.git_selected).cloned()
                else {
                    return Vec::new();
                };
                vec![Effect::LoadGitDiff { path, target }]
            }
            Command::SelectGitHunk(index) => {
                let count = self.git_diff.as_ref().map_or(0, |diff| diff.hunks.len());
                self.git_hunk_selected = index.min(count.saturating_sub(1));
                self.focus = Focus::Editor;
                Vec::new()
            }
            Command::GitHunkPrevious => {
                self.git_hunk_selected = self.git_hunk_selected.saturating_sub(1);
                self.reveal_selected_git_hunk();
                Vec::new()
            }
            Command::GitHunkNext => {
                let count = self.git_diff.as_ref().map_or(0, |diff| diff.hunks.len());
                self.git_hunk_selected = (self.git_hunk_selected + 1).min(count.saturating_sub(1));
                self.reveal_selected_git_hunk();
                Vec::new()
            }
            Command::EditorScroll(delta) => {
                if self.git_diff_active {
                    self.git_diff_scroll = if delta < 0 {
                        self.git_diff_scroll
                            .saturating_sub(delta.unsigned_abs() as usize)
                    } else {
                        let max = self
                            .git_diff
                            .as_ref()
                            .map_or(0, |diff| diff.raw.lines().count());
                        self.git_diff_scroll
                            .saturating_add(delta as usize)
                            .min(max.saturating_sub(1))
                    };
                } else {
                    let visible_height = self.editor_visible_height();
                    let Some(tab) = self.active_tab.and_then(|index| self.tabs.get_mut(index))
                    else {
                        return Vec::new();
                    };
                    let max = if self.settings.editor.word_wrap {
                        let width = editor_wrap_width(
                            self.terminal_size.0,
                            self.sidebar_visible,
                            self.settings.ui.sidebar_width,
                            self.split_tab.is_some(),
                            tab.buffer.text().len_lines(),
                        );
                        editor::visual_row_count(
                            tab.buffer.text(),
                            width,
                            usize::from(self.settings.editor.tab_width),
                            self.settings.editor.ambiguous_width_wide,
                        )
                        .saturating_sub(visible_height)
                    } else {
                        tab.buffer.text().len_lines().saturating_sub(visible_height)
                    };
                    tab.view.scroll_line = if delta < 0 {
                        tab.view
                            .scroll_line
                            .saturating_sub(delta.unsigned_abs() as usize)
                    } else {
                        tab.view.scroll_line.saturating_add(delta as usize).min(max)
                    };
                }
                Vec::new()
            }
            Command::GitHunkStageToggle => {
                let Some(diff) = &self.git_diff else {
                    return Vec::new();
                };
                let Some(hunk) = diff.hunks.get(self.git_hunk_selected) else {
                    return Vec::new();
                };
                let operation = match diff.target {
                    crate::git::DiffTarget::WorkingTree => {
                        crate::app::GitOperation::StageHunk(hunk.patch.clone())
                    }
                    crate::git::DiffTarget::Staged => {
                        crate::app::GitOperation::UnstageHunk(hunk.patch.clone())
                    }
                };
                vec![Effect::GitOperation(operation)]
            }
            Command::GitHunkRestore => {
                let Some(diff) = &self.git_diff else {
                    return Vec::new();
                };
                if diff.target != crate::git::DiffTarget::WorkingTree {
                    self.notification = Some("Only working-tree hunks can be restored".to_owned());
                    return Vec::new();
                }
                let Some(hunk) = diff.hunks.get(self.git_hunk_selected) else {
                    return Vec::new();
                };
                self.overlay = Some(Overlay::ConfirmGitHunkRestore {
                    patch: hunk.patch.clone(),
                });
                self.focus = Focus::Overlay;
                Vec::new()
            }
            Command::GitHunkOpenFile => {
                let Some(diff) = &self.git_diff else {
                    return Vec::new();
                };
                let Some(hunk) = diff.hunks.get(self.git_hunk_selected) else {
                    return Vec::new();
                };
                let path = self.workspace.as_path().join(&diff.path);
                self.focus = Focus::Editor;
                vec![Effect::OpenFile {
                    path,
                    read_only: self.force_read_only,
                    line: Some(hunk.new_start.max(1)),
                    column: Some(ColumnHint::Chars(1)),
                }]
            }
            Command::WorkspaceSearchInput(character) => {
                self.workspace_search.query.push(character);
                self.start_workspace_search().into_iter().collect()
            }
            Command::WorkspaceSearchBackspace => {
                self.workspace_search.query.pop();
                self.start_workspace_search().into_iter().collect()
            }
            Command::WorkspaceSearchSelect(index) => {
                self.workspace_search_selected =
                    index.min(self.workspace_matches.len().saturating_sub(1));
                Vec::new()
            }
            Command::WorkspaceSearchSelectAndOpen(index) => {
                self.workspace_search_selected =
                    index.min(self.workspace_matches.len().saturating_sub(1));
                let Some(matched) = self
                    .workspace_matches
                    .get(self.workspace_search_selected)
                    .cloned()
                else {
                    return Vec::new();
                };
                self.focus = Focus::Editor;
                vec![Effect::OpenFile {
                    path: self.workspace.as_path().join(matched.path),
                    read_only: self.force_read_only,
                    line: Some(matched.line),
                    column: Some(ColumnHint::Chars(matched.column)),
                }]
            }
            Command::WorkspaceSearchOpen => {
                let Some(matched) = self
                    .workspace_matches
                    .get(self.workspace_search_selected)
                    .cloned()
                else {
                    return Vec::new();
                };
                self.focus = Focus::Editor;
                vec![Effect::OpenFile {
                    path: self.workspace.as_path().join(matched.path),
                    read_only: self.force_read_only,
                    line: Some(matched.line),
                    column: Some(ColumnHint::Chars(matched.column)),
                }]
            }
            Command::WorkspaceSearchToggleCase => {
                self.workspace_search.case_sensitive = !self.workspace_search.case_sensitive;
                self.start_workspace_search().into_iter().collect()
            }
            Command::WorkspaceSearchToggleWord => {
                self.workspace_search.whole_word = !self.workspace_search.whole_word;
                self.start_workspace_search().into_iter().collect()
            }
            Command::WorkspaceSearchToggleRegex => {
                self.workspace_search.regex = !self.workspace_search.regex;
                self.start_workspace_search().into_iter().collect()
            }
            Command::WorkspaceSearchToggleHidden => {
                self.workspace_search.show_hidden = !self.workspace_search.show_hidden;
                self.start_workspace_search().into_iter().collect()
            }
            Command::WorkspaceSearchToggleBinary => {
                self.workspace_search.include_binary = !self.workspace_search.include_binary;
                self.start_workspace_search().into_iter().collect()
            }
            Command::WorkspaceSearchToggleFile(path) => {
                if !self.workspace_search_collapsed.remove(&path) {
                    self.workspace_search_collapsed.insert(path);
                }
                Vec::new()
            }
            Command::TerminalInput(bytes) => {
                if self.terminal_running {
                    self.terminal_scroll_offset = 0;
                    self.terminal_selection = None;
                    vec![Effect::TerminalInput(bytes)]
                } else {
                    Vec::new()
                }
            }
            Command::TerminalPaste(text) => {
                if !self.terminal_running {
                    return Vec::new();
                }
                let bracketed = self.terminal.snapshot(0).bracketed_paste;
                vec![Effect::TerminalInput(crate::terminal::encode_paste(
                    &text, bracketed,
                ))]
            }
            Command::TerminalScroll(delta) => {
                let maximum = self.terminal.scrollback_len();
                self.terminal_scroll_offset = if delta < 0 {
                    self.terminal_scroll_offset
                        .saturating_add(delta.unsigned_abs() as usize)
                        .min(maximum)
                } else {
                    self.terminal_scroll_offset.saturating_sub(delta as usize)
                };
                Vec::new()
            }
            Command::FocusTerminal => {
                if self.bottom_panel_view == BottomPanelView::Terminal {
                    self.focus = Focus::BottomPanel;
                }
                Vec::new()
            }
            Command::FocusSidebar => {
                self.sidebar_visible = true;
                self.focus = Focus::Sidebar;
                Vec::new()
            }
            Command::TerminalSetSelection {
                row,
                column,
                extend,
            } => {
                self.focus = Focus::BottomPanel;
                let point = (row, column);
                self.terminal_selection = Some(if extend {
                    self.terminal_selection
                        .map_or((point, point), |(anchor, _)| (anchor, point))
                } else {
                    (point, point)
                });
                Vec::new()
            }
            Command::TerminalCopy => {
                let Some((start, end)) = self.terminal_selection else {
                    self.notification = Some("Select terminal text to copy".to_owned());
                    return Vec::new();
                };
                let text = self
                    .terminal
                    .selected_text(self.terminal_scroll_offset, start, end);
                if text.is_empty() {
                    self.notification = Some("Terminal selection is empty".to_owned());
                    Vec::new()
                } else {
                    self.internal_clipboard = text.clone();
                    self.notification = Some("Copied terminal selection".to_owned());
                    vec![Effect::CopyToClipboard(text)]
                }
            }
            Command::TerminalOpenReference { row, column } => {
                self.open_terminal_reference(row, column)
            }
            Command::TerminalSearchNext => {
                self.select_terminal_search_match(1);
                Vec::new()
            }
            Command::TerminalSearchPrevious => {
                self.select_terminal_search_match(-1);
                Vec::new()
            }
            Command::DiagnosticSelect(index) => {
                self.diagnostic_selected =
                    index.min(self.visible_diagnostics().len().saturating_sub(1));
                Vec::new()
            }
            Command::DiagnosticOpen => self.open_selected_diagnostic(),
            Command::DiagnosticCycleFilter => {
                self.diagnostic_filter = match self.diagnostic_filter {
                    None => Some(crate::diagnostics::DiagnosticSeverity::Error),
                    Some(crate::diagnostics::DiagnosticSeverity::Error) => {
                        Some(crate::diagnostics::DiagnosticSeverity::Warning)
                    }
                    _ => None,
                };
                self.diagnostic_selected = 0;
                Vec::new()
            }
            Command::DiagnosticCycleSource => {
                self.diagnostic_source_filter = match self.diagnostic_source_filter {
                    None => Some(crate::diagnostics::DiagnosticSource::Compiler),
                    Some(crate::diagnostics::DiagnosticSource::Compiler) => {
                        Some(crate::diagnostics::DiagnosticSource::Linter)
                    }
                    Some(crate::diagnostics::DiagnosticSource::Linter) => {
                        Some(crate::diagnostics::DiagnosticSource::Lsp)
                    }
                    Some(crate::diagnostics::DiagnosticSource::Lsp) => {
                        Some(crate::diagnostics::DiagnosticSource::Task)
                    }
                    Some(crate::diagnostics::DiagnosticSource::Task) => None,
                };
                self.diagnostic_selected = 0;
                Vec::new()
            }
            Command::DiagnosticToggleCurrentFile => {
                self.diagnostic_current_file_only = !self.diagnostic_current_file_only;
                self.diagnostic_selected = 0;
                Vec::new()
            }
            Command::LspLocationSelect(index) => {
                self.lsp_location_selected = index.min(self.lsp_locations.len().saturating_sub(1));
                Vec::new()
            }
            Command::LspLocationOpen => Vec::new(),
            Command::LspCodeActionSelect(index) => {
                self.lsp_code_action_selected =
                    index.min(self.lsp_code_actions.len().saturating_sub(1));
                Vec::new()
            }
            Command::LspCodeActionApply => Vec::new(),
            Command::SelectTab(index) => {
                if index < self.tabs.len() {
                    self.active_tab = Some(index);
                    self.git_diff_active = false;
                    self.focus = Focus::Editor;
                } else if index == self.tabs.len() && self.git_diff.is_some() {
                    self.git_diff_active = true;
                    self.focus = Focus::Editor;
                }
                self.active_syntax_effect().into_iter().collect()
            }
            Command::BeginTabDrag(index) => {
                if index < self.tabs.len() {
                    self.tab_drag_source = Some(index);
                    self.active_tab = Some(index);
                    self.git_diff_active = false;
                    self.focus = Focus::Editor;
                }
                Vec::new()
            }
            Command::ReorderTab { from, to } => {
                self.reorder_tab(from, to);
                self.tab_drag_source = Some(from);
                Vec::new()
            }
            Command::TogglePinTab(index) => {
                if index < self.tabs.len() {
                    self.tabs[index].pinned = !self.tabs[index].pinned;
                    self.active_tab = Some(index);
                    self.tab_order = self.visual_tab_order();
                    self.notification = Some(if self.tabs[index].pinned {
                        format!("Pinned {}", self.tabs[index].title())
                    } else {
                        format!("Unpinned {}", self.tabs[index].title())
                    });
                }
                Vec::new()
            }
            Command::EndTabDrag => {
                self.tab_drag_source = None;
                Vec::new()
            }
            Command::SplitEditor => {
                if self.split_tab.is_none() {
                    self.split_tab = self.active_tab;
                    self.split_focus_right = false;
                }
                Vec::new()
            }
            Command::FocusNextEditorGroup => {
                if self.split_tab.is_some() {
                    std::mem::swap(&mut self.active_tab, &mut self.split_tab);
                    self.split_focus_right = !self.split_focus_right;
                    self.git_diff_active = false;
                    self.focus = Focus::Editor;
                }
                Vec::new()
            }
            Command::FocusEditorGroup(right) => {
                if self.split_tab.is_some() && self.split_focus_right != right {
                    std::mem::swap(&mut self.active_tab, &mut self.split_tab);
                    self.split_focus_right = right;
                    self.git_diff_active = false;
                }
                self.focus = Focus::Editor;
                Vec::new()
            }
            Command::CloseEditorSplit => {
                self.split_tab = None;
                self.split_focus_right = false;
                Vec::new()
            }
            Command::CloseTab(index) => {
                if index == self.tabs.len() && self.git_diff.is_some() {
                    self.git_diff = None;
                    self.git_diff_active = false;
                    self.focus = Focus::Editor;
                    Vec::new()
                } else {
                    self.request_close_tab(index)
                }
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
                if self.terminal_started {
                    let (rows, cols) = self.terminal_panel_size();
                    self.terminal.resize(usize::from(rows), usize::from(cols));
                    vec![Effect::ResizeTerminal { rows, cols }]
                } else {
                    Vec::new()
                }
            }
            Command::PaletteInput(_)
            | Command::PaletteBackspace
            | Command::PaletteNewline
            | Command::PaletteAccept
            | Command::SearchNext
            | Command::SearchPrevious
            | Command::SearchToggleReplaceField
            | Command::ReplaceNext
            | Command::ReplaceAll
            | Command::RecoveryRecover
            | Command::RecoveryDiscard
            | Command::RecoveryLater => Vec::new(),
        }
    }

    fn execute_overlay(&mut self, command: Command) -> Vec<Effect> {
        match command {
            Command::TerminalSearchNext
                if matches!(self.overlay, Some(Overlay::TerminalSearch)) =>
            {
                self.select_terminal_search_match(1);
                return Vec::new();
            }
            Command::TerminalSearchPrevious
                if matches!(self.overlay, Some(Overlay::TerminalSearch)) =>
            {
                self.select_terminal_search_match(-1);
                return Vec::new();
            }
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
                if matches!(self.overlay, Some(Overlay::BufferSearch { .. }))
                    && self.buffer_replace_focused
                {
                    self.buffer_replace_query.push(character);
                    return Vec::new();
                }
                self.palette_query.push(character);
                if matches!(self.overlay, Some(Overlay::TerminalSearch)) {
                    self.refresh_terminal_search();
                    return Vec::new();
                }
                if matches!(self.overlay, Some(Overlay::LspCompletion)) {
                    self.filter_lsp_completions();
                    return Vec::new();
                }
                if matches!(self.overlay, Some(Overlay::GitBranchPicker)) {
                    self.git_branch_selected = 0;
                }
                if matches!(self.overlay, Some(Overlay::FilePicker)) {
                    return vec![self.start_file_search()];
                }
                if let Some(Overlay::BufferSearch { tab }) = self.overlay.as_ref() {
                    return self.start_buffer_search(*tab).into_iter().collect();
                }
            }
            Command::PaletteBackspace => {
                if matches!(self.overlay, Some(Overlay::BufferSearch { .. }))
                    && self.buffer_replace_focused
                {
                    self.buffer_replace_query.pop();
                    return Vec::new();
                }
                self.palette_query.pop();
                if matches!(self.overlay, Some(Overlay::TerminalSearch)) {
                    self.refresh_terminal_search();
                    return Vec::new();
                }
                if matches!(self.overlay, Some(Overlay::LspCompletion)) {
                    self.filter_lsp_completions();
                    return Vec::new();
                }
                if matches!(self.overlay, Some(Overlay::GitBranchPicker)) {
                    self.git_branch_selected = 0;
                }
                if matches!(self.overlay, Some(Overlay::FilePicker)) {
                    return vec![self.start_file_search()];
                }
                if let Some(Overlay::BufferSearch { tab }) = self.overlay.as_ref() {
                    return self.start_buffer_search(*tab).into_iter().collect();
                }
            }
            Command::PaletteNewline if matches!(self.overlay, Some(Overlay::GitCommitInput)) => {
                self.palette_query.push('\n');
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
            Command::SearchToggleReplaceField => {
                if self.buffer_replace_visible {
                    self.buffer_replace_focused = !self.buffer_replace_focused;
                } else {
                    self.buffer_replace_visible = true;
                    self.buffer_replace_focused = true;
                }
            }
            Command::ReplaceNext => {
                if let Some(Overlay::BufferSearch { tab }) = self.overlay.as_ref() {
                    return self.replace_current_match(*tab);
                }
            }
            Command::ReplaceAll => {
                if let Some(Overlay::BufferSearch { tab }) = self.overlay.as_ref() {
                    return self.replace_all_matches(*tab);
                }
            }
            Command::LspLocationSelect(index)
                if matches!(self.overlay, Some(Overlay::LspLocations)) =>
            {
                self.lsp_location_selected = index.min(self.lsp_locations.len().saturating_sub(1));
            }
            Command::LspLocationOpen if matches!(self.overlay, Some(Overlay::LspLocations)) => {
                let Some(location) = self.lsp_locations.get(self.lsp_location_selected).cloned()
                else {
                    return Vec::new();
                };
                self.overlay = None;
                self.focus = Focus::Editor;
                self.record_navigation_origin();
                return vec![Effect::OpenFile {
                    path: location.path,
                    read_only: self.force_read_only,
                    line: Some(location.line),
                    column: Some(location.column),
                }];
            }
            Command::LspCodeActionSelect(index)
                if matches!(self.overlay, Some(Overlay::LspCodeActions)) =>
            {
                self.lsp_code_action_selected =
                    index.min(self.lsp_code_actions.len().saturating_sub(1));
            }
            Command::LspCodeActionApply
                if matches!(self.overlay, Some(Overlay::LspCodeActions)) =>
            {
                return self.apply_selected_code_action();
            }
            Command::MoveUp { .. } if matches!(self.overlay, Some(Overlay::FilePicker)) => {
                self.file_picker_selected = self.file_picker_selected.saturating_sub(1);
                return self.selected_preview_effect().into_iter().collect();
            }
            Command::MoveUp { .. } if matches!(self.overlay, Some(Overlay::GitBranchPicker)) => {
                self.git_branch_selected = self.git_branch_selected.saturating_sub(1);
            }
            Command::MoveUp { .. } if matches!(self.overlay, Some(Overlay::LspCompletion)) => {
                self.lsp_completion_selected = self.lsp_completion_selected.saturating_sub(1);
            }
            Command::MoveUp { .. } if matches!(self.overlay, Some(Overlay::KeybindingHelp)) => {
                self.help_selected = self.help_selected.saturating_sub(1);
            }
            Command::MoveUp { .. }
                if matches!(self.overlay, Some(Overlay::NotificationHistory)) =>
            {
                self.notification_selected = self.notification_selected.saturating_sub(1);
            }
            Command::MoveDown { .. } if matches!(self.overlay, Some(Overlay::FilePicker)) => {
                self.file_picker_selected =
                    (self.file_picker_selected + 1).min(self.file_matches.len().saturating_sub(1));
                return self.selected_preview_effect().into_iter().collect();
            }
            Command::MoveDown { .. } if matches!(self.overlay, Some(Overlay::GitBranchPicker)) => {
                self.git_branch_selected = (self.git_branch_selected + 1)
                    .min(self.visible_git_branches().len().saturating_sub(1));
            }
            Command::MoveDown { .. } if matches!(self.overlay, Some(Overlay::LspCompletion)) => {
                self.lsp_completion_selected = (self.lsp_completion_selected + 1)
                    .min(self.lsp_completions.len().saturating_sub(1));
            }
            Command::MoveDown { .. } if matches!(self.overlay, Some(Overlay::KeybindingHelp)) => {
                self.help_selected =
                    (self.help_selected + 1).min(self.active_keybindings().len().saturating_sub(1));
            }
            Command::MoveDown { .. }
                if matches!(self.overlay, Some(Overlay::NotificationHistory)) =>
            {
                self.notification_selected = (self.notification_selected + 1)
                    .min(self.notification_history.len().saturating_sub(1));
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
                        if let PathAction::SaveAs { tab } = &action {
                            self.format_tab_for_save(*tab);
                            let Some(tab_state) = self.tabs.get(*tab) else {
                                return Vec::new();
                            };
                            return match tab_state.buffer.prepare_save() {
                                Ok(snapshot) => vec![Effect::InspectSaveAs {
                                    tab: *tab,
                                    destination: path,
                                    snapshot,
                                }],
                                Err(error) => {
                                    self.notification = Some(error.to_string());
                                    Vec::new()
                                }
                            };
                        }
                        let request = match action {
                            PathAction::CreateFile => FileOperationRequest::CreateFile(path),
                            PathAction::CreateDirectory => {
                                FileOperationRequest::CreateDirectory(path)
                            }
                            PathAction::Move { source } => FileOperationRequest::Move {
                                source,
                                destination: path,
                            },
                            PathAction::SaveAs { .. } => return Vec::new(),
                        };
                        return vec![Effect::FileOperation(request)];
                    }
                    Some(Overlay::GotoLine) => {
                        let input = self.palette_query.trim().to_owned();
                        self.palette_query.clear();
                        self.focus = Focus::Editor;
                        let mut parts = input.split(':');
                        let line = parts.next().and_then(|value| value.parse::<usize>().ok());
                        let column = parts.next().and_then(|value| value.parse::<usize>().ok());
                        if parts.next().is_some() || line.is_none() {
                            self.notification =
                                Some("Enter a line or line:column number".to_owned());
                            return Vec::new();
                        }
                        let Some(index) = self.active_tab else {
                            return Vec::new();
                        };
                        let tab = &mut self.tabs[index];
                        let line_index = line
                            .unwrap_or(1)
                            .saturating_sub(1)
                            .min(tab.buffer.text().len_lines().saturating_sub(1));
                        let line_start = tab.buffer.text().line_to_char(line_index);
                        let line_text = tab.buffer.text().line(line_index).to_string();
                        let content_chars =
                            line_text.trim_end_matches(['\r', '\n']).chars().count();
                        let char_offset =
                            line_start + column.unwrap_or(1).saturating_sub(1).min(content_chars);
                        tab.buffer
                            .set_selection(Selection::caret(CharOffset(char_offset)));
                        self.reveal_cursor(index);
                        return Vec::new();
                    }
                    Some(Overlay::ConfirmDelete { plan, .. }) => {
                        self.palette_query.clear();
                        self.focus = Focus::Editor;
                        return vec![Effect::FileOperation(FileOperationRequest::Delete(plan))];
                    }
                    Some(Overlay::ConfirmClose { tab }) => {
                        self.palette_query.clear();
                        self.focus = Focus::Editor;
                        return self.close_tab(tab).into_iter().collect();
                    }
                    Some(Overlay::ConfirmSaveAs { plan }) => {
                        self.palette_query.clear();
                        return self.start_save_as(plan);
                    }
                    Some(Overlay::RecoveryPrompt) => {
                        self.overlay = Some(Overlay::RecoveryPrompt);
                    }
                    Some(Overlay::GitCommitInput) => {
                        let message = std::mem::take(&mut self.palette_query);
                        self.focus = Focus::Editor;
                        if message.trim().is_empty() {
                            self.notification = Some("Commit message cannot be empty".to_owned());
                            return Vec::new();
                        }
                        return vec![Effect::GitOperation(crate::app::GitOperation::Commit(
                            message,
                        ))];
                    }
                    Some(Overlay::ConfirmGitRestore { path }) => {
                        self.focus = Focus::Editor;
                        return vec![Effect::GitOperation(crate::app::GitOperation::Restore(
                            path,
                        ))];
                    }
                    Some(Overlay::ConfirmGitHunkRestore { patch }) => {
                        self.focus = Focus::Editor;
                        return vec![Effect::GitOperation(crate::app::GitOperation::RestoreHunk(
                            patch,
                        ))];
                    }
                    Some(Overlay::GitBranchPicker) => {
                        let branch = self
                            .visible_git_branches()
                            .get(self.git_branch_selected)
                            .map(|branch| branch.name.clone());
                        self.palette_query.clear();
                        self.focus = Focus::Editor;
                        if let Some(branch) = branch {
                            return vec![Effect::GitOperation(
                                crate::app::GitOperation::SwitchBranch(branch),
                            )];
                        }
                    }
                    Some(Overlay::GitBranchCreate) => {
                        let branch = std::mem::take(&mut self.palette_query);
                        self.focus = Focus::Editor;
                        if branch.trim().is_empty() {
                            self.notification = Some("Branch name cannot be empty".to_owned());
                            return Vec::new();
                        }
                        return vec![Effect::GitOperation(
                            crate::app::GitOperation::CreateBranch(branch),
                        )];
                    }
                    Some(Overlay::SearchIncludeGlobs) => {
                        self.workspace_search.include_globs = split_globs(&self.palette_query);
                        self.palette_query.clear();
                        self.focus = Focus::Sidebar;
                        return self.start_workspace_search().into_iter().collect();
                    }
                    Some(Overlay::SearchExcludeGlobs) => {
                        self.workspace_search.exclude_globs = split_globs(&self.palette_query);
                        self.palette_query.clear();
                        self.focus = Focus::Sidebar;
                        return self.start_workspace_search().into_iter().collect();
                    }
                    Some(Overlay::ConfirmQuitTerminal) => {
                        self.should_quit = true;
                        self.focus = Focus::Editor;
                        return vec![Effect::StopTerminal];
                    }
                    Some(Overlay::TerminalSearch) => {
                        self.overlay = Some(Overlay::TerminalSearch);
                        self.select_terminal_search_match(1);
                        return Vec::new();
                    }
                    Some(Overlay::KeybindingHelp) => {
                        self.focus = self.help_context;
                    }
                    Some(Overlay::NotificationHistory) => {
                        self.focus = self.notification_context;
                    }
                    Some(Overlay::LspHover) => {
                        self.focus = Focus::Editor;
                    }
                    Some(Overlay::LspSignature) => {
                        self.focus = Focus::Editor;
                    }
                    Some(Overlay::LspCompletion) => {
                        self.focus = Focus::Editor;
                        let Some(completion) = self
                            .lsp_completions
                            .get(self.lsp_completion_selected)
                            .cloned()
                        else {
                            return Vec::new();
                        };
                        self.apply_completion(&completion);
                        return self.active_post_edit_effects();
                    }
                    Some(Overlay::LspLocations) => {
                        self.focus = Focus::Editor;
                        let Some(location) =
                            self.lsp_locations.get(self.lsp_location_selected).cloned()
                        else {
                            return Vec::new();
                        };
                        self.record_navigation_origin();
                        return vec![Effect::OpenFile {
                            path: location.path,
                            read_only: self.force_read_only,
                            line: Some(location.line),
                            column: Some(location.column),
                        }];
                    }
                    Some(Overlay::LspCodeActions) => {
                        self.focus = Focus::Editor;
                        return self.apply_selected_code_action();
                    }
                    None => {}
                }
            }
            Command::Cancel => {
                let restore_help_focus = matches!(self.overlay, Some(Overlay::KeybindingHelp))
                    .then_some(self.help_context);
                let restore_notification_focus =
                    matches!(self.overlay, Some(Overlay::NotificationHistory))
                        .then_some(self.notification_context);
                if let Some(Overlay::BufferSearch { tab }) = self.overlay.as_ref()
                    && let Some(tab) = self.tabs.get_mut(*tab)
                {
                    tab.view.search.matches.clear();
                    tab.view.search.query.clear();
                    tab.view.search.current = 0;
                }
                self.overlay = None;
                self.terminal_search_matches.clear();
                self.terminal_search_selected = 0;
                self.buffer_replace_focused = false;
                self.buffer_replace_visible = false;
                self.palette_query.clear();
                self.file_search_generation = self.file_search_generation.saturating_add(1);
                self.file_search_cancellation
                    .store(self.file_search_generation, Ordering::Relaxed);
                self.file_matches.clear();
                self.file_preview_path = None;
                self.file_preview_lines.clear();
                self.focus = restore_help_focus
                    .or(restore_notification_focus)
                    .unwrap_or(Focus::Editor);
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
                } else if self.terminal_running {
                    self.overlay = Some(Overlay::ConfirmQuitTerminal);
                    self.focus = Focus::Overlay;
                } else {
                    self.should_quit = true;
                }
                Vec::new()
            }
            command::EDITOR_SAVE => {
                if self.git_diff_active {
                    self.notification = Some("Git diff tabs are read-only".to_owned());
                    return Vec::new();
                }
                let Some(index) = self.active_tab else {
                    return Vec::new();
                };
                if self.save_as_tabs.contains(&index) {
                    self.notification = Some("Save As is already in progress".to_owned());
                    return Vec::new();
                }
                if self.settings.editor.format_on_save {
                    let effects = self.lsp_request_effect(
                        "textDocument/formatting",
                        crate::app::PendingLspRequest::Formatting,
                    );
                    if !effects.is_empty() {
                        self.format_on_save_tabs.insert(index);
                        self.notification = Some("Formatting before save…".to_owned());
                        return effects;
                    }
                }
                self.format_tab_for_save(index);
                self.save_tab(index)
            }
            command::EDITOR_SAVE_ALL => {
                let candidates = self
                    .tabs
                    .iter()
                    .enumerate()
                    .filter(|(_, tab)| {
                        tab.buffer.is_dirty()
                            && !tab.buffer.is_read_only()
                            && tab.buffer.path().is_some()
                    })
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();
                let mut effects = Vec::new();
                for index in candidates {
                    self.format_tab_for_save(index);
                    effects.extend(self.save_tab(index));
                }
                if effects.is_empty() {
                    self.notification = Some("No writable files to save".to_owned());
                }
                effects
            }
            command::EDITOR_SCROLL_UP => self.execute(Command::EditorScroll(-3)),
            command::EDITOR_SCROLL_DOWN => self.execute(Command::EditorScroll(3)),
            command::EDITOR_SPLIT => self.execute(Command::SplitEditor),
            command::EDITOR_FOCUS_NEXT_GROUP => self.execute(Command::FocusNextEditorGroup),
            command::EDITOR_CLOSE_SPLIT => self.execute(Command::CloseEditorSplit),
            command::GIT_DIFF_PREVIOUS => self.execute(Command::GitHunkPrevious),
            command::GIT_DIFF_NEXT => self.execute(Command::GitHunkNext),
            command::EDITOR_SAVE_AS => {
                if self.git_diff_active {
                    self.notification = Some("Git diff tabs are read-only".to_owned());
                    return Vec::new();
                }
                let Some(tab) = self.active_tab else {
                    return Vec::new();
                };
                if self.saving_tabs.contains(&tab) {
                    self.notification = Some("Wait for the current save to finish".to_owned());
                    return Vec::new();
                }
                let initial = self.tabs[tab]
                    .buffer
                    .path()
                    .and_then(|path| path.strip_prefix(self.workspace.as_path()).ok())
                    .map_or_else(String::new, |path| path.to_string_lossy().into_owned());
                self.open_path_input(PathAction::SaveAs { tab }, initial);
                Vec::new()
            }
            command::EDITOR_CLOSE => {
                if self.git_diff_active && self.git_diff.is_some() {
                    self.git_diff = None;
                    self.git_diff_active = false;
                    return Vec::new();
                }
                if let Some(index) = self.active_tab {
                    return self.request_close_tab(index);
                }
                Vec::new()
            }
            command::EDITOR_TOGGLE_PIN => {
                let Some(index) = self.active_tab else {
                    return Vec::new();
                };
                self.execute(Command::TogglePinTab(index))
            }
            command::EDITOR_MOVE_TAB_LEFT => {
                self.move_active_tab(-1);
                Vec::new()
            }
            command::EDITOR_MOVE_TAB_RIGHT => {
                self.move_active_tab(1);
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
                if self.git_diff_active {
                    self.notification = Some("Git diff tabs are read-only".to_owned());
                    return Vec::new();
                }
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
                        effects.extend(self.active_post_edit_effects());
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
                self.active_post_edit_effects()
            }
            command::EDITOR_FIND => {
                let Some(tab) = self.active_tab else {
                    return Vec::new();
                };
                self.overlay = Some(Overlay::BufferSearch { tab });
                self.palette_query = self.tabs[tab].buffer.selected_text().unwrap_or_default();
                self.tabs[tab].view.search = Default::default();
                self.buffer_replace_query.clear();
                self.buffer_replace_visible = false;
                self.buffer_replace_focused = false;
                self.focus = Focus::Overlay;
                self.start_buffer_search(tab).into_iter().collect()
            }
            command::EDITOR_REPLACE => {
                let Some(tab) = self.active_tab else {
                    return Vec::new();
                };
                self.overlay = Some(Overlay::BufferSearch { tab });
                self.palette_query = self.tabs[tab].buffer.selected_text().unwrap_or_default();
                self.buffer_replace_query.clear();
                self.buffer_replace_visible = true;
                self.buffer_replace_focused = false;
                self.tabs[tab].view.search = Default::default();
                self.focus = Focus::Overlay;
                self.start_buffer_search(tab).into_iter().collect()
            }
            command::EDITOR_REPLACE_NEXT => {
                let Some(Overlay::BufferSearch { tab }) = self.overlay.as_ref() else {
                    self.notification = Some("Open Replace in Buffer first".to_owned());
                    return Vec::new();
                };
                self.replace_current_match(*tab)
            }
            command::EDITOR_REPLACE_ALL => {
                let Some(Overlay::BufferSearch { tab }) = self.overlay.as_ref() else {
                    self.notification = Some("Open Replace in Buffer first".to_owned());
                    return Vec::new();
                };
                self.replace_all_matches(*tab)
            }
            command::EDITOR_UNDO => {
                self.edit(|tab| tab.buffer.undo().map(|_| ()));
                self.active_post_edit_effects()
            }
            command::EDITOR_REDO => {
                self.edit(|tab| tab.buffer.redo().map(|_| ()));
                self.active_post_edit_effects()
            }
            command::EDITOR_INDENT => self.change_selected_indent(false),
            command::EDITOR_OUTDENT => self.change_selected_indent(true),
            command::EDITOR_TOGGLE_LINE_COMMENT => self.toggle_line_comment(),
            command::EDITOR_TOGGLE_LINE_NUMBERS => {
                self.settings.editor.line_numbers = !self.settings.editor.line_numbers;
                self.notification = Some(if self.settings.editor.line_numbers {
                    "Line numbers shown".to_owned()
                } else {
                    "Line numbers hidden".to_owned()
                });
                Vec::new()
            }
            command::EDITOR_TOGGLE_WORD_WRAP => {
                self.settings.editor.word_wrap = !self.settings.editor.word_wrap;
                self.notification = Some(if self.settings.editor.word_wrap {
                    "Word wrap enabled".to_owned()
                } else {
                    "Word wrap disabled".to_owned()
                });
                Vec::new()
            }
            command::EDITOR_SELECT_ALL => {
                if let Some(tab) = self.active_tab.and_then(|index| self.tabs.get_mut(index)) {
                    tab.buffer.select_all();
                }
                Vec::new()
            }
            command::EDITOR_DUPLICATE_LINE => {
                self.edit(|tab| tab.buffer.duplicate_selected_lines());
                self.active_post_edit_effects()
            }
            command::EDITOR_MOVE_LINE_UP => {
                self.edit(|tab| tab.buffer.move_selected_lines(false));
                self.active_post_edit_effects()
            }
            command::EDITOR_MOVE_LINE_DOWN => {
                self.edit(|tab| tab.buffer.move_selected_lines(true));
                self.active_post_edit_effects()
            }
            command::EDITOR_DELETE_LINE => {
                self.edit(|tab| tab.buffer.delete_selected_lines());
                self.active_post_edit_effects()
            }
            command::EDITOR_GOTO_LINE => {
                if self.active_tab.is_none() {
                    self.notification = Some("Open a file first".to_owned());
                    return Vec::new();
                }
                self.overlay = Some(Overlay::GotoLine);
                self.palette_query.clear();
                self.focus = Focus::Overlay;
                Vec::new()
            }
            command::EDITOR_NAVIGATE_BACK => self.navigate_history(true),
            command::EDITOR_NAVIGATE_FORWARD => self.navigate_history(false),
            command::DIAGNOSTICS_NEXT => {
                let count = self.visible_diagnostics().len();
                if count == 0 {
                    self.notification = Some("No diagnostics".to_owned());
                    return Vec::new();
                }
                self.diagnostic_selected = (self.diagnostic_selected + 1) % count;
                self.open_selected_diagnostic()
            }
            command::DIAGNOSTICS_PREVIOUS => {
                let count = self.visible_diagnostics().len();
                if count == 0 {
                    self.notification = Some("No diagnostics".to_owned());
                    return Vec::new();
                }
                self.diagnostic_selected =
                    self.diagnostic_selected.checked_sub(1).unwrap_or(count - 1);
                self.open_selected_diagnostic()
            }
            command::DIAGNOSTICS_FILTER_SEVERITY => self.execute(Command::DiagnosticCycleFilter),
            command::DIAGNOSTICS_FILTER_SOURCE => self.execute(Command::DiagnosticCycleSource),
            command::DIAGNOSTICS_FILTER_CURRENT_FILE => {
                self.execute(Command::DiagnosticToggleCurrentFile)
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
            command::TERMINAL_TOGGLE => {
                if self.bottom_panel_visible && self.bottom_panel_view == BottomPanelView::Terminal
                {
                    self.bottom_panel_visible = false;
                    self.focus = Focus::Editor;
                    return Vec::new();
                }
                self.bottom_panel_visible = true;
                self.bottom_panel_view = BottomPanelView::Terminal;
                self.focus = Focus::BottomPanel;
                if self.terminal_started {
                    Vec::new()
                } else {
                    vec![self.start_terminal_effect()]
                }
            }
            command::TERMINAL_NEW_SESSION => {
                self.bottom_panel_visible = true;
                self.bottom_panel_view = BottomPanelView::Terminal;
                self.focus = Focus::BottomPanel;
                self.terminal_started = false;
                self.terminal_running = false;
                vec![Effect::StopTerminal, self.start_terminal_effect()]
            }
            command::TERMINAL_OPEN_REFERENCE => {
                let snapshot = self.terminal.snapshot(self.terminal_scroll_offset);
                self.open_terminal_reference(snapshot.cursor_row, Some(snapshot.cursor_col))
            }
            command::TERMINAL_SEARCH => {
                self.bottom_panel_visible = true;
                self.bottom_panel_view = BottomPanelView::Terminal;
                self.focus = Focus::BottomPanel;
                self.palette_query.clear();
                self.terminal_search_matches.clear();
                self.terminal_search_selected = 0;
                self.overlay = Some(Overlay::TerminalSearch);
                Vec::new()
            }
            command::VIEW_OUTPUT => {
                self.bottom_panel_visible = true;
                self.bottom_panel_view = BottomPanelView::Output;
                self.focus = Focus::BottomPanel;
                Vec::new()
            }
            command::VIEW_TERMINAL => {
                self.bottom_panel_visible = true;
                self.bottom_panel_view = BottomPanelView::Terminal;
                self.focus = Focus::BottomPanel;
                if self.terminal_started {
                    Vec::new()
                } else {
                    vec![self.start_terminal_effect()]
                }
            }
            command::DIAGNOSTICS_OPEN_PROBLEMS => {
                self.bottom_panel_visible = true;
                self.bottom_panel_view = BottomPanelView::Problems;
                self.focus = Focus::BottomPanel;
                Vec::new()
            }
            command::DIAGNOSTICS_REFRESH => self.start_cargo_diagnostics(true),
            command::LSP_HOVER => {
                self.lsp_request_effect("textDocument/hover", crate::app::PendingLspRequest::Hover)
            }
            command::LSP_DEFINITION => {
                self.record_navigation_origin();
                self.lsp_request_effect(
                    "textDocument/definition",
                    crate::app::PendingLspRequest::Definition,
                )
            }
            command::LSP_COMPLETION => self.lsp_request_effect(
                "textDocument/completion",
                crate::app::PendingLspRequest::Completion,
            ),
            command::LSP_REFERENCES => self.lsp_request_effect(
                "textDocument/references",
                crate::app::PendingLspRequest::References,
            ),
            command::LSP_FORMAT => self.lsp_request_effect(
                "textDocument/formatting",
                crate::app::PendingLspRequest::Formatting,
            ),
            command::LSP_SIGNATURE_HELP => self.lsp_request_effect(
                "textDocument/signatureHelp",
                crate::app::PendingLspRequest::SignatureHelp,
            ),
            command::LSP_CODE_ACTION => self.lsp_request_effect(
                "textDocument/codeAction",
                crate::app::PendingLspRequest::CodeActions,
            ),
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
                self.git_loading = true;
                vec![Effect::RefreshGit]
            }
            command::VIEW_SEARCH => {
                self.sidebar_visible = true;
                self.sidebar_view = SidebarView::Search;
                self.focus = Focus::Sidebar;
                self.start_workspace_search().into_iter().collect()
            }
            command::GIT_REFRESH => {
                self.git_loading = true;
                vec![Effect::RefreshGit]
            }
            command::GIT_STAGE => self.selected_git_operation(true),
            command::GIT_UNSTAGE => self.selected_git_operation(false),
            command::GIT_RESTORE => {
                let Some((path, target)) = self.git_entries().get(self.git_selected).cloned()
                else {
                    return Vec::new();
                };
                if target != crate::git::DiffTarget::WorkingTree {
                    self.notification = Some("Select an unstaged change to restore".to_owned());
                    return Vec::new();
                }
                self.overlay = Some(Overlay::ConfirmGitRestore { path });
                self.focus = Focus::Overlay;
                Vec::new()
            }
            command::GIT_COMMIT => {
                let staged = self
                    .git_status
                    .as_ref()
                    .is_some_and(|status| status.files.iter().any(|file| file.staged));
                if !staged {
                    self.notification = Some("No staged changes to commit".to_owned());
                    return Vec::new();
                }
                self.overlay = Some(Overlay::GitCommitInput);
                self.palette_query.clear();
                self.focus = Focus::Overlay;
                Vec::new()
            }
            command::GIT_BRANCH_SWITCH => {
                self.overlay = Some(Overlay::GitBranchPicker);
                self.palette_query.clear();
                self.git_branch_selected = 0;
                self.focus = Focus::Overlay;
                vec![Effect::LoadGitBranches]
            }
            command::GIT_BRANCH_CREATE => {
                self.overlay = Some(Overlay::GitBranchCreate);
                self.palette_query.clear();
                self.focus = Focus::Overlay;
                Vec::new()
            }
            command::GIT_FETCH => vec![Effect::GitOperation(crate::app::GitOperation::Fetch)],
            command::GIT_PULL => vec![Effect::GitOperation(crate::app::GitOperation::Pull)],
            command::GIT_PUSH => {
                if self
                    .git_status
                    .as_ref()
                    .is_some_and(|status| status.upstream.is_none())
                {
                    let branch = self
                        .git_status
                        .as_ref()
                        .and_then(|status| status.branch.as_deref())
                        .unwrap_or("<branch>");
                    self.notification = Some(format!(
                        "No upstream; run `git push --set-upstream origin {branch}` in Terminal"
                    ));
                    Vec::new()
                } else {
                    vec![Effect::GitOperation(crate::app::GitOperation::Push)]
                }
            }
            command::SEARCH_INCLUDE_GLOBS => {
                self.palette_query = self.workspace_search.include_globs.join(", ");
                self.overlay = Some(Overlay::SearchIncludeGlobs);
                self.focus = Focus::Overlay;
                Vec::new()
            }
            command::SEARCH_EXCLUDE_GLOBS => {
                self.palette_query = self.workspace_search.exclude_globs.join(", ");
                self.overlay = Some(Overlay::SearchExcludeGlobs);
                self.focus = Focus::Overlay;
                Vec::new()
            }
            command::WORKSPACE_REFRESH => {
                self.git_loading = true;
                vec![Effect::ScanWorkspace, Effect::RefreshGit]
            }
            command::CONFIG_RELOAD => vec![Effect::ReloadConfig],
            command::CONFIG_OPEN => {
                let relative = std::path::Path::new(".mica/config.toml");
                if let Err(error) = self.workspace.resolve(relative) {
                    self.notification = Some(format!("Cannot open configuration: {error}"));
                    return Vec::new();
                }
                match self.workspace.resolve_lexical(relative) {
                    Ok(path) => vec![Effect::PrepareConfigFile(path)],
                    Err(error) => {
                        self.notification = Some(format!("Cannot open configuration: {error}"));
                        Vec::new()
                    }
                }
            }
            command::HELP_KEYBINDINGS => {
                self.help_context = self.focus;
                self.help_selected = 0;
                self.overlay = Some(Overlay::KeybindingHelp);
                self.focus = Focus::Overlay;
                Vec::new()
            }
            command::NOTIFICATIONS_HISTORY => {
                self.notification_context = self.focus;
                self.notification_selected = 0;
                self.overlay = Some(Overlay::NotificationHistory);
                self.focus = Focus::Overlay;
                Vec::new()
            }
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
        if self.git_diff_active {
            self.notification = Some("Git diff tabs are read-only".to_owned());
            return false;
        }
        let Some(tab) = self.active_tab.and_then(|index| self.tabs.get_mut(index)) else {
            return false;
        };
        if let Err(error) = operation(tab) {
            self.notification = Some(error.to_string());
            false
        } else {
            if let Some(path) = tab.buffer.path().map(ToOwned::to_owned) {
                self.diagnostics.mark_file_stale(&path);
            }
            self.reveal_cursor(self.active_tab.unwrap_or(0));
            true
        }
    }

    /// Accepts a completion, replacing `replace_range` (when the server gave
    /// one via `textEdit`) instead of always inserting at the cursor — a
    /// plain cursor insert would duplicate whatever prefix the user already
    /// typed (e.g. completing `ver` -> `version` would yield `verversion`).
    fn apply_completion(&mut self, completion: &crate::app::CompletionCandidate) {
        let insert_text = completion.insert_text.clone();
        let replace_range = completion.replace_range;
        let cursor_char_offset = completion.cursor_char_offset;
        self.edit(|tab| {
            let mut insertion_start = tab.buffer.selection().range().start;
            if let Some((start, end)) = replace_range {
                let text = tab.buffer.text().to_string();
                let len_chars = tab.buffer.text().len_chars();
                let mut start_offset =
                    crate::lsp::position_to_char_offset(&text, start).min(len_chars);
                let mut end_offset = crate::lsp::position_to_char_offset(&text, end).min(len_chars);
                if start_offset > end_offset {
                    std::mem::swap(&mut start_offset, &mut end_offset);
                }
                tab.buffer.set_selection(Selection {
                    anchor: CharOffset(start_offset),
                    head: CharOffset(end_offset),
                });
                insertion_start = start_offset;
            }
            tab.buffer.insert(&insert_text)?;
            if let Some(relative) = cursor_char_offset {
                tab.buffer.set_selection(Selection::caret(CharOffset(
                    insertion_start + relative.min(insert_text.chars().count()),
                )));
            }
            Ok(())
        });
    }

    fn filter_lsp_completions(&mut self) {
        let labels = self
            .lsp_completion_all
            .iter()
            .map(|completion| completion.label.as_str())
            .collect::<Vec<_>>();
        self.lsp_completions =
            crate::search::fuzzy_label_indices(&self.palette_query, &labels, 200)
                .into_iter()
                .filter_map(|index| self.lsp_completion_all.get(index).cloned())
                .collect();
        self.lsp_completion_selected = 0;
    }

    fn selected_git_operation(&mut self, stage: bool) -> Vec<Effect> {
        let Some((path, target)) = self.git_entries().get(self.git_selected).cloned() else {
            return Vec::new();
        };
        let valid = if stage {
            target == crate::git::DiffTarget::WorkingTree
        } else {
            target == crate::git::DiffTarget::Staged
        };
        if !valid {
            self.notification = Some(if stage {
                "Select an unstaged change".to_owned()
            } else {
                "Select a staged change".to_owned()
            });
            return Vec::new();
        }
        let operation = if stage {
            crate::app::GitOperation::Stage(path)
        } else {
            crate::app::GitOperation::Unstage(path)
        };
        vec![Effect::GitOperation(operation)]
    }

    fn start_workspace_search(&mut self) -> Option<Effect> {
        let generation = self
            .workspace_search_cancellation
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        self.workspace_search_cancellation
            .store(generation, Ordering::Relaxed);
        self.workspace_search_generation = generation;
        self.workspace_matches.clear();
        self.workspace_search_selected = 0;
        self.workspace_search_running = !self.workspace_search.query.is_empty();
        if self.workspace_search.query.is_empty() {
            return None;
        }
        let open_buffers = self
            .tabs
            .iter()
            .filter_map(|tab| {
                tab.buffer
                    .path()
                    .map(|path| (path.to_path_buf(), tab.buffer.text().to_string()))
            })
            .collect();
        Some(Effect::SearchWorkspace {
            generation,
            options: self.workspace_search.clone(),
            open_buffers,
            cancellation: self.workspace_search_cancellation.clone(),
        })
    }

    fn terminal_panel_size(&self) -> (u16, u16) {
        let (width, height) = self.terminal_size;
        let rows = self
            .settings
            .ui
            .bottom_panel_height
            .min(height.saturating_sub(2) / 2)
            .saturating_sub(1)
            .max(1);
        let sidebar = if self.sidebar_visible {
            self.settings.ui.sidebar_width
        } else {
            0
        };
        let cols = width.saturating_sub(3).saturating_sub(sidebar).max(2);
        (rows, cols)
    }

    fn start_terminal_effect(&mut self) -> Effect {
        self.terminal_generation = self.terminal_generation.saturating_add(1);
        let (rows, cols) = self.terminal_panel_size();
        self.terminal.resize(usize::from(rows), usize::from(cols));
        let configured = self.settings.terminal.shell.trim();
        Effect::StartTerminal {
            generation: self.terminal_generation,
            shell: (!configured.is_empty()).then(|| PathBuf::from(configured)),
            cwd: self.workspace.as_path().to_path_buf(),
            rows,
            cols,
        }
    }

    fn open_terminal_reference(&mut self, row: usize, column: Option<usize>) -> Vec<Effect> {
        let snapshot = self.terminal.snapshot(self.terminal_scroll_offset);
        let Some(cells) = snapshot.lines.get(row) else {
            self.notification = Some("No terminal line at that position".to_owned());
            return Vec::new();
        };
        let text = cells
            .iter()
            .filter(|cell| !cell.wide_continuation)
            .map(|cell| cell.character)
            .collect::<String>();
        let references = crate::terminal::find_file_references(text.trim_end());
        let selected = column
            .and_then(|column| {
                references
                    .iter()
                    .find(|reference| reference.display_columns.contains(&column))
            })
            .or_else(|| references.first());
        let Some(reference) = selected else {
            self.notification = Some("No file reference on terminal line".to_owned());
            return Vec::new();
        };
        let path = match self.workspace.resolve(&reference.path) {
            Ok(path) => path.absolute(),
            Err(error) => {
                self.notification = Some(format!("Cannot open terminal reference: {error}"));
                return Vec::new();
            }
        };
        self.focus = Focus::Editor;
        vec![Effect::OpenFile {
            path,
            read_only: self.force_read_only,
            line: Some(reference.line),
            column: Some(ColumnHint::Chars(reference.column)),
        }]
    }

    fn refresh_terminal_search(&mut self) {
        self.terminal_search_matches = self.terminal.search(&self.palette_query);
        self.terminal_search_selected = 0;
        self.reveal_terminal_search_match();
    }

    fn select_terminal_search_match(&mut self, direction: i32) {
        if self.terminal_search_matches.is_empty() {
            return;
        }
        let length = self.terminal_search_matches.len();
        self.terminal_search_selected = if direction < 0 {
            self.terminal_search_selected
                .checked_sub(1)
                .unwrap_or(length - 1)
        } else {
            (self.terminal_search_selected + 1) % length
        };
        self.reveal_terminal_search_match();
    }

    fn reveal_terminal_search_match(&mut self) {
        let Some(matched) = self
            .terminal_search_matches
            .get(self.terminal_search_selected)
        else {
            return;
        };
        let viewport_rows = self.terminal.snapshot(0).lines.len();
        let maximum_start = self.terminal.history_len().saturating_sub(viewport_rows);
        let desired_start = matched
            .history_row
            .saturating_sub(viewport_rows / 2)
            .min(maximum_start);
        self.terminal_scroll_offset = maximum_start
            .saturating_sub(desired_start)
            .min(self.terminal.scrollback_len());
    }

    fn lsp_open_effect(&mut self, path: &std::path::Path) -> Vec<Effect> {
        if !self.settings.lsp.enabled {
            return Vec::new();
        }
        if self
            .tabs
            .iter()
            .find(|tab| tab.buffer.path() == Some(path))
            .is_some_and(|tab| self.is_large_buffer(&tab.buffer))
        {
            return Vec::new();
        }
        let Some((language, settings)) = self.language_for_path(path) else {
            return Vec::new();
        };
        if self.lsp_started.contains(&language) {
            return self
                .lsp_did_open_effect(path, &language)
                .into_iter()
                .collect();
        }
        if self.lsp_starting.insert(language.clone()) {
            vec![Effect::StartLsp {
                language,
                settings,
                workspace: self.workspace.as_path().to_path_buf(),
            }]
        } else {
            Vec::new()
        }
    }

    fn active_post_edit_effects(&mut self) -> Vec<Effect> {
        let mut effects = self.active_syntax_effect().into_iter().collect::<Vec<_>>();
        if let Some(effect) = self.active_lsp_change_effect() {
            effects.push(effect);
        }
        effects
    }

    fn active_lsp_change_effect(&mut self) -> Option<Effect> {
        let tab = self.active_tab()?;
        if self.is_large_buffer(&tab.buffer) {
            return None;
        }
        let path = tab.buffer.path()?.to_path_buf();
        // `Rope::clone` is O(1) (structural sharing), unlike the
        // `.to_string()` this replaced, so cloning it here — ahead of the
        // mutable borrow below — is not the "heavy work on the UI thread"
        // Fix 5 is about; the actual document stringification now happens
        // on the LSP client's writer thread, right before the write.
        let text = tab.buffer.text().clone();
        let (language, _) = self.language_for_path(&path)?;
        if !self.lsp_started.contains(&language) {
            return None;
        }
        let version = self.lsp_versions.entry(path.clone()).or_insert(1);
        *version = version.saturating_add(1);
        Some(Effect::SendLspChange {
            language,
            uri: file_uri(&path),
            version: *version,
            text,
        })
    }

    fn lsp_save_effect(&self, path: &std::path::Path) -> Option<Effect> {
        let (language, _) = self.language_for_path(path)?;
        self.lsp_started
            .contains(&language)
            .then(|| Effect::SendLsp {
                language,
                message: serde_json::json!({
                    "jsonrpc":"2.0",
                    "method":"textDocument/didSave",
                    "params":{"textDocument":{"uri":file_uri(path)}}
                }),
            })
    }

    fn lsp_did_open_effect(&mut self, path: &std::path::Path, language: &str) -> Option<Effect> {
        let tab = self
            .tabs
            .iter()
            .find(|tab| tab.buffer.path() == Some(path))?;
        let version = *self.lsp_versions.entry(path.to_path_buf()).or_insert(1);
        Some(Effect::SendLsp {
            language: language.to_owned(),
            message: serde_json::json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didOpen",
                "params": {"textDocument": {
                    "uri": file_uri(path),
                    "languageId": language,
                    "version": version,
                    "text": tab.buffer.text().to_string(),
                }}
            }),
        })
    }

    fn handle_lsp_event(
        &mut self,
        language: String,
        event: crate::lsp::LspClientEvent,
    ) -> Vec<Effect> {
        match event {
            crate::lsp::LspClientEvent::Started => Vec::new(),
            crate::lsp::LspClientEvent::Initialized => {
                self.lsp_starting.remove(&language);
                self.lsp_started.insert(language.clone());
                self.lsp_restarts.remove(&language);
                let paths = self
                    .tabs
                    .iter()
                    .filter_map(|tab| tab.buffer.path().map(ToOwned::to_owned))
                    .filter(|path| {
                        self.language_for_path(path)
                            .is_some_and(|(name, _)| name == language)
                    })
                    .collect::<Vec<_>>();
                paths
                    .iter()
                    .filter_map(|path| self.lsp_did_open_effect(path, &language))
                    .collect()
            }
            crate::lsp::LspClientEvent::Message(message) => {
                self.handle_lsp_message(&language, &message)
            }
            crate::lsp::LspClientEvent::Stderr(message) => {
                self.append_output(&format!("lsp:{language}"), message);
                Vec::new()
            }
            crate::lsp::LspClientEvent::Exited(code) => {
                self.lsp_started.remove(&language);
                self.lsp_starting.remove(&language);
                self.lsp_diagnostic_generation = self.lsp_diagnostic_generation.saturating_add(1);
                self.diagnostics.clear_source(
                    crate::diagnostics::DiagnosticSource::Lsp,
                    self.lsp_diagnostic_generation,
                );
                let message = format!("LSP {language} exited ({code:?})");
                tracing::warn!(language = %language, ?code, "LSP process exited");
                self.append_output("lsp", &message);
                self.notification = Some(message);
                self.lsp_restart_effects(&language)
            }
            crate::lsp::LspClientEvent::Error(error) => {
                self.lsp_started.remove(&language);
                self.lsp_starting.remove(&language);
                tracing::warn!(language = %language, error = %error, "LSP client error");
                if self.lsp_warned.insert(language.clone()) {
                    self.notification = Some(format!("LSP {language}: {error}"));
                }
                self.append_output(&format!("lsp:{language}"), error);
                self.lsp_restart_effects(&language)
            }
        }
    }

    fn lsp_restart_effects(&mut self, language: &str) -> Vec<Effect> {
        let mut effects = vec![Effect::StopLsp {
            language: language.to_owned(),
        }];
        let restarts = self.lsp_restarts.entry(language.to_owned()).or_default();
        if *restarts >= 3 {
            tracing::warn!(language = %language, "LSP restart limit reached; giving up");
            self.append_output("lsp", format!("{language}: restart limit reached"));
            return effects;
        }
        let Some(settings) = self.settings.languages.get(language).cloned() else {
            return effects;
        };
        *restarts = restarts.saturating_add(1);
        self.lsp_starting.insert(language.to_owned());
        effects.push(Effect::StartLsp {
            language: language.to_owned(),
            settings,
            workspace: self.workspace.as_path().to_path_buf(),
        });
        effects
    }

    fn handle_lsp_message(&mut self, language: &str, message: &serde_json::Value) -> Vec<Effect> {
        if let Some(id) = message.get("id").and_then(serde_json::Value::as_u64)
            && let Some(request) = self.lsp_pending.remove(&id)
        {
            return self.handle_lsp_response(request, message.get("result"));
        }
        let method = message.get("method").and_then(serde_json::Value::as_str);
        if matches!(method, Some("window/logMessage" | "window/showMessage")) {
            if let Some(text) = message
                .get("params")
                .and_then(|params| params.get("message"))
                .and_then(serde_json::Value::as_str)
            {
                self.append_output(&format!("lsp:{language}"), text);
                if method == Some("window/showMessage") {
                    self.notification = Some(format!("LSP {language}: {text}"));
                }
            }
            return Vec::new();
        }
        if method == Some("$/progress") {
            let value = message.get("params").and_then(|params| params.get("value"));
            let kind = value
                .and_then(|value| value.get("kind"))
                .and_then(serde_json::Value::as_str);
            if kind == Some("end") {
                self.lsp_progress.remove(language);
            } else if let Some(value) = value {
                let title = value
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("working");
                let detail = value
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .map_or_else(String::new, |message| format!(" {message}"));
                let percentage = value
                    .get("percentage")
                    .and_then(serde_json::Value::as_u64)
                    .map_or_else(String::new, |value| format!(" {value}%"));
                self.lsp_progress
                    .insert(language.to_owned(), format!("{title}{detail}{percentage}"));
            }
            return Vec::new();
        }
        if method != Some("textDocument/publishDiagnostics") {
            return Vec::new();
        }
        if !self.settings.diagnostics.enabled {
            return Vec::new();
        }
        let Some(params) = message.get("params") else {
            return Vec::new();
        };
        let Some(uri) = params.get("uri").and_then(serde_json::Value::as_str) else {
            return Vec::new();
        };
        let Some(path) = file_uri_to_path(uri) else {
            return Vec::new();
        };
        let source = self
            .tabs
            .iter()
            .find(|tab| tab.buffer.path() == Some(path.as_path()))
            .map(|tab| tab.buffer.text().to_string());
        let diagnostics = params
            .get("diagnostics")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| lsp_diagnostic(&path, source.as_deref(), item))
            .collect();
        self.lsp_diagnostic_generation = self.lsp_diagnostic_generation.saturating_add(1);
        self.diagnostics.replace_source(
            crate::diagnostics::DiagnosticSource::Lsp,
            self.lsp_diagnostic_generation,
            diagnostics,
        );
        Vec::new()
    }

    fn lsp_request_effect(
        &mut self,
        method: &str,
        make_request: impl FnOnce(PathBuf) -> crate::app::PendingLspRequest,
    ) -> Vec<Effect> {
        let Some(tab) = self.active_tab() else {
            return Vec::new();
        };
        if self.is_large_buffer(&tab.buffer) {
            self.notification = Some("LSP is disabled in large-file mode".to_owned());
            return Vec::new();
        }
        let Some(path) = tab.buffer.path().map(ToOwned::to_owned) else {
            return Vec::new();
        };
        let text = tab.buffer.text().to_string();
        let cursor = tab.buffer.selection().head.0;
        let Some((language, _)) = self.language_for_path(&path) else {
            return Vec::new();
        };
        if !self.lsp_started.contains(&language) {
            self.notification = Some(format!("LSP {language} is not ready"));
            return Vec::new();
        }
        let id = self.lsp_next_request_id;
        self.lsp_next_request_id = self.lsp_next_request_id.saturating_add(1);
        // Tag the pending request with the file it was made against, so a
        // response that arrives after the user has switched tabs can be
        // recognized as stale (Fix 7; see `PendingLspRequest`).
        self.lsp_pending.insert(id, make_request(path.clone()));
        let position = crate::lsp::char_offset_to_position(&text, cursor);
        let params = if method == "textDocument/references" {
            serde_json::json!({
                "textDocument":{"uri":file_uri(&path)},
                "position":position,
                "context":{"includeDeclaration":true}
            })
        } else if method == "textDocument/codeAction" {
            serde_json::json!({
                "textDocument":{"uri":file_uri(&path)},
                "range":{"start":position,"end":position},
                "context":{"diagnostics":[]}
            })
        } else if method == "textDocument/formatting" {
            serde_json::json!({
                "textDocument":{"uri":file_uri(&path)},
                "options":{
                    "tabSize":self.settings.editor.tab_width,
                    "insertSpaces":self.settings.editor.insert_spaces
                }
            })
        } else {
            serde_json::json!({"textDocument":{"uri":file_uri(&path)},"position":position})
        };
        vec![Effect::SendLsp {
            language,
            message: serde_json::json!({
                "jsonrpc":"2.0",
                "id":id,
                "method":method,
                "params":params
            }),
        }]
    }

    fn handle_lsp_response(
        &mut self,
        request: crate::app::PendingLspRequest,
        result: Option<&serde_json::Value>,
    ) -> Vec<Effect> {
        let result = result.filter(|result| !result.is_null());
        if result.is_none() {
            if let crate::app::PendingLspRequest::Formatting(path) = request {
                self.notification =
                    Some("Formatter returned no edits; saving unchanged".to_owned());
                return self.finish_format_on_save(&path);
            }
            self.notification = Some("LSP returned no result".to_owned());
            return Vec::new();
        }
        let Some(result) = result else {
            return Vec::new();
        };
        match request {
            crate::app::PendingLspRequest::Hover(path) => {
                if !self.is_active_path(&path) {
                    self.notification = Some("Stale LSP response discarded".to_owned());
                    return Vec::new();
                }
                self.lsp_hover = hover_lines(result);
                if self.lsp_hover.is_empty() {
                    self.notification = Some("No hover information".to_owned());
                } else {
                    self.overlay = Some(Overlay::LspHover);
                    self.focus = Focus::Overlay;
                }
                Vec::new()
            }
            // `Definition` is intentionally not guarded: it jumps by opening
            // the target file explicitly out of the response, which is
            // correct regardless of which tab happens to be active when the
            // response arrives.
            crate::app::PendingLspRequest::Definition(_) => {
                let location = if result.is_array() {
                    result.as_array().and_then(|items| items.first())
                } else {
                    Some(result)
                };
                let Some(location) = location else {
                    return Vec::new();
                };
                let uri = location
                    .get("uri")
                    .or_else(|| location.get("targetUri"))
                    .and_then(serde_json::Value::as_str);
                let range = location
                    .get("range")
                    .or_else(|| location.get("targetSelectionRange"));
                let Some(path) = uri.and_then(file_uri_to_path) else {
                    return Vec::new();
                };
                let start = range.and_then(|range| range.get("start"));
                let line = start
                    .and_then(|start| start.get("line"))
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0) as usize
                    + 1;
                let column = start
                    .and_then(|start| start.get("character"))
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0) as usize
                    + 1;
                vec![Effect::OpenFile {
                    path,
                    read_only: self.force_read_only,
                    line: Some(line),
                    // `character` is UTF-16 code units per the LSP spec (Fix
                    // 1); `AppEvent::FileOpened` converts it against the
                    // loaded line's real content instead of treating it as a
                    // char column.
                    column: Some(ColumnHint::Utf16(column)),
                }]
            }
            crate::app::PendingLspRequest::Completion(path) => {
                if !self.is_active_path(&path) {
                    self.notification = Some("Stale LSP response discarded".to_owned());
                    return Vec::new();
                }
                let items = result
                    .as_array()
                    .or_else(|| result.get("items").and_then(serde_json::Value::as_array));
                self.lsp_completion_all = items
                    .into_iter()
                    .flatten()
                    .filter_map(completion_candidate)
                    .take(200)
                    .collect();
                self.lsp_completions = self.lsp_completion_all.clone();
                self.palette_query.clear();
                self.lsp_completion_selected = 0;
                if self.lsp_completions.is_empty() {
                    self.notification = Some("No completions".to_owned());
                } else {
                    self.overlay = Some(Overlay::LspCompletion);
                    self.focus = Focus::Overlay;
                }
                Vec::new()
            }
            crate::app::PendingLspRequest::References(path) => {
                if !self.is_active_path(&path) {
                    self.notification = Some("Stale LSP response discarded".to_owned());
                    return Vec::new();
                }
                self.lsp_locations = result
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(lsp_location)
                    .collect();
                self.lsp_location_selected = 0;
                if self.lsp_locations.is_empty() {
                    self.notification = Some("No references found".to_owned());
                } else {
                    self.overlay = Some(Overlay::LspLocations);
                    self.focus = Focus::Overlay;
                }
                Vec::new()
            }
            crate::app::PendingLspRequest::Formatting(path) => {
                if !self.is_active_path(&path) {
                    self.notification = Some("Stale LSP response discarded".to_owned());
                    return Vec::new();
                }
                let Some(index) = self.active_tab else {
                    return Vec::new();
                };
                let source = self.tabs[index].buffer.text().to_string();
                let edits = result
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|edit| lsp_text_edit(&source, edit))
                    .collect::<Vec<_>>();
                if edits.is_empty() {
                    self.notification = Some("Document is already formatted".to_owned());
                    return self.finish_format_on_save(&path);
                }
                self.edit(|tab| tab.buffer.apply_text_edits(edits).map(|_| ()));
                self.notification = Some("Formatted document".to_owned());
                let mut effects = self.active_post_edit_effects();
                effects.extend(self.finish_format_on_save(&path));
                effects
            }
            crate::app::PendingLspRequest::SignatureHelp(path) => {
                if !self.is_active_path(&path) {
                    self.notification = Some("Stale LSP response discarded".to_owned());
                    return Vec::new();
                }
                self.lsp_signature = signature_help_lines(result);
                if self.lsp_signature.is_empty() {
                    self.notification = Some("No signature help available".to_owned());
                } else {
                    self.overlay = Some(Overlay::LspSignature);
                    self.focus = Focus::Overlay;
                }
                Vec::new()
            }
            crate::app::PendingLspRequest::CodeActions(path) => {
                if !self.is_active_path(&path) {
                    self.notification = Some("Stale LSP response discarded".to_owned());
                    return Vec::new();
                }
                self.lsp_code_actions = result
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|item| {
                        Some(crate::app::CodeActionCandidate {
                            title: item.get("title")?.as_str()?.to_owned(),
                            edit: item.get("edit").cloned(),
                            command: item.get("command").map(|command| {
                                if command.is_string() {
                                    item.clone()
                                } else {
                                    command.clone()
                                }
                            }),
                        })
                    })
                    .collect();
                self.lsp_code_action_selected = 0;
                if self.lsp_code_actions.is_empty() {
                    self.notification = Some("No code actions available".to_owned());
                } else {
                    self.overlay = Some(Overlay::LspCodeActions);
                    self.focus = Focus::Overlay;
                }
                Vec::new()
            }
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

    fn change_selected_indent(&mut self, outdent: bool) -> Vec<Effect> {
        let indent = if self.settings.editor.insert_spaces {
            " ".repeat(usize::from(self.settings.editor.tab_width))
        } else {
            "\t".to_owned()
        };
        self.edit(|tab| tab.buffer.change_selected_line_indent(&indent, outdent));
        self.active_post_edit_effects()
    }

    fn format_tab_for_save(&mut self, index: usize) {
        let trim = self.settings.editor.trim_trailing_whitespace;
        let final_newline = self.settings.editor.insert_final_newline;
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        if let Err(error) = tab.buffer.apply_save_formatting(trim, final_newline) {
            self.notification = Some(error.to_string());
        }
    }

    fn save_tab(&mut self, index: usize) -> Vec<Effect> {
        let Some(tab) = self.tabs.get(index) else {
            return Vec::new();
        };
        match tab.buffer.prepare_save() {
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

    fn finish_format_on_save(&mut self, path: &std::path::Path) -> Vec<Effect> {
        let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.buffer.path() == Some(path))
        else {
            return Vec::new();
        };
        if !self.format_on_save_tabs.remove(&index) {
            return Vec::new();
        }
        self.format_tab_for_save(index);
        self.save_tab(index)
    }

    fn apply_selected_code_action(&mut self) -> Vec<Effect> {
        let Some(action) = self
            .lsp_code_actions
            .get(self.lsp_code_action_selected)
            .cloned()
        else {
            return Vec::new();
        };
        self.overlay = None;
        self.focus = Focus::Editor;
        let mut edits_by_tab = Vec::new();
        if let Some(edit) = &action.edit {
            if edit.get("documentChanges").is_some() {
                self.notification = Some(
                    "This code action uses documentChanges, which is not yet supported".to_owned(),
                );
                return Vec::new();
            }
            if let Some(changes) = edit.get("changes").and_then(serde_json::Value::as_object) {
                for (uri, edits) in changes {
                    let Some(path) = file_uri_to_path(uri) else {
                        self.notification =
                            Some("Code action contains an invalid file URI".to_owned());
                        return Vec::new();
                    };
                    let Some(index) = self
                        .tabs
                        .iter()
                        .position(|tab| tab.buffer.path() == Some(path.as_path()))
                    else {
                        self.notification = Some(format!(
                            "Code action edits unopened file {}; open it before retrying",
                            path.display()
                        ));
                        return Vec::new();
                    };
                    let source = self.tabs[index].buffer.text().to_string();
                    let parsed = edits
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|edit| lsp_text_edit(&source, edit))
                        .collect::<Vec<_>>();
                    edits_by_tab.push((index, path, parsed));
                }
            }
        }
        for (index, path, edits) in edits_by_tab {
            if let Some(tab) = self.tabs.get_mut(index)
                && let Err(error) = tab.buffer.apply_text_edits(edits)
            {
                self.notification = Some(error.to_string());
                return Vec::new();
            }
            self.diagnostics.mark_file_stale(&path);
        }
        let mut effects = self.active_post_edit_effects();
        if let Some(command) = action.command {
            let command_name = command.get("command").and_then(serde_json::Value::as_str);
            let Some(command_name) = command_name else {
                self.notification = Some("Code action returned an invalid command".to_owned());
                return effects;
            };
            let Some(path) = self.active_tab().and_then(|tab| tab.buffer.path()) else {
                return effects;
            };
            let Some((language, _)) = self.language_for_path(path) else {
                return effects;
            };
            let id = self.lsp_next_request_id;
            self.lsp_next_request_id = self.lsp_next_request_id.saturating_add(1);
            effects.push(Effect::SendLsp {
                language,
                message: serde_json::json!({
                    "jsonrpc":"2.0",
                    "id":id,
                    "method":"workspace/executeCommand",
                    "params":{
                        "command":command_name,
                        "arguments":command.get("arguments").cloned().unwrap_or_else(|| serde_json::json!([]))
                    }
                }),
            });
        }
        self.notification = Some(format!("Applied code action: {}", action.title));
        effects
    }

    fn open_selected_diagnostic(&mut self) -> Vec<Effect> {
        let Some(diagnostic) = self
            .visible_diagnostics()
            .get(self.diagnostic_selected)
            .cloned()
            .cloned()
        else {
            return Vec::new();
        };
        self.record_navigation_origin();
        self.focus = Focus::Editor;
        vec![Effect::OpenFile {
            path: diagnostic.file,
            read_only: self.force_read_only,
            line: Some(diagnostic.range.start.line + 1),
            column: Some(ColumnHint::Chars(diagnostic.range.start.column + 1)),
        }]
    }

    fn start_cargo_diagnostics(&mut self, notify: bool) -> Vec<Effect> {
        if !self.settings.diagnostics.enabled {
            if notify {
                self.notification = Some("Diagnostics are disabled".to_owned());
            }
            return Vec::new();
        }
        if !self.workspace.as_path().join("Cargo.toml").is_file() {
            if notify {
                self.notification =
                    Some("Cargo diagnostics require Cargo.toml in the workspace".to_owned());
            }
            return Vec::new();
        }
        if self.cargo_diagnostics_running {
            self.cargo_diagnostics_pending = true;
            return Vec::new();
        }
        self.cargo_diagnostics_running = true;
        self.compiler_diagnostic_generation = self.compiler_diagnostic_generation.saturating_add(1);
        if notify {
            self.notification = Some("Running cargo clippy…".to_owned());
        }
        vec![Effect::RunCargoDiagnostics {
            generation: self.compiler_diagnostic_generation,
        }]
    }

    fn finish_cargo_diagnostics(
        &mut self,
        source: crate::diagnostics::DiagnosticSource,
    ) -> Vec<Effect> {
        if source != crate::diagnostics::DiagnosticSource::Compiler {
            return Vec::new();
        }
        self.cargo_diagnostics_running = false;
        if self.cargo_diagnostics_pending {
            self.cargo_diagnostics_pending = false;
            self.start_cargo_diagnostics(false)
        } else {
            Vec::new()
        }
    }

    fn current_navigation_location(&self) -> Option<crate::app::NavigationLocation> {
        let tab = self.active_tab()?;
        let path = tab.buffer.path()?.to_path_buf();
        let cursor = tab
            .buffer
            .selection()
            .head
            .0
            .min(tab.buffer.text().len_chars());
        let line_index = tab.buffer.text().char_to_line(cursor);
        let column = cursor - tab.buffer.text().line_to_char(line_index) + 1;
        Some(crate::app::NavigationLocation {
            path,
            line: line_index + 1,
            column: ColumnHint::Chars(column),
        })
    }

    fn record_navigation_origin(&mut self) {
        if let Some(location) = self.current_navigation_location() {
            self.navigation_back.push(location);
            if self.navigation_back.len() > 100 {
                self.navigation_back.remove(0);
            }
            self.navigation_forward.clear();
        }
    }

    fn navigate_history(&mut self, back: bool) -> Vec<Effect> {
        let target = if back {
            self.navigation_back.pop()
        } else {
            self.navigation_forward.pop()
        };
        let Some(target) = target else {
            self.notification = Some(if back {
                "No previous navigation location".to_owned()
            } else {
                "No forward navigation location".to_owned()
            });
            return Vec::new();
        };
        if let Some(current) = self.current_navigation_location() {
            if back {
                self.navigation_forward.push(current);
            } else {
                self.navigation_back.push(current);
            }
        }
        vec![Effect::OpenFile {
            path: target.path,
            read_only: self.force_read_only,
            line: Some(target.line),
            column: Some(target.column),
        }]
    }

    fn toggle_line_comment(&mut self) -> Vec<Effect> {
        let language = self
            .active_tab()
            .and_then(|tab| tab.buffer.path())
            .and_then(|path| {
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .and_then(editor::SyntaxLanguage::from_extension)
                    .or_else(|| {
                        self.language_for_path(path)
                            .and_then(|(name, _)| editor::SyntaxLanguage::from_language_name(&name))
                    })
            });
        let Some((prefix, suffix)) = language.and_then(editor::SyntaxLanguage::comment_tokens)
        else {
            self.notification = Some("No line comment syntax for this file type".to_owned());
            return Vec::new();
        };
        self.edit(|tab| tab.buffer.toggle_selected_line_comment(prefix, suffix));
        self.active_post_edit_effects()
    }

    fn move_word_horizontal(&mut self, right: bool, extend: bool) {
        let Some(tab) = self.active_tab.and_then(|index| self.tabs.get_mut(index)) else {
            return;
        };
        let selection = tab.buffer.selection();
        let next = if right {
            editor::move_word_right(tab.buffer.text(), selection.head.0)
        } else {
            editor::move_word_left(tab.buffer.text(), selection.head.0)
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
        let ambiguous_width_wide = self.settings.editor.ambiguous_width_wide;
        let wrap_width = self
            .active_tab
            .and_then(|index| self.tabs.get(index))
            .map(|tab| {
                editor_wrap_width(
                    self.terminal_size.0,
                    self.sidebar_visible,
                    self.settings.ui.sidebar_width,
                    self.split_tab.is_some(),
                    tab.buffer.text().len_lines(),
                )
            });
        let Some(tab) = self.active_tab.and_then(|index| self.tabs.get_mut(index)) else {
            return;
        };
        let selection = tab.buffer.selection();
        let (next, preferred) = if self.settings.editor.word_wrap {
            editor::move_visual_vertical(
                tab.buffer.text(),
                selection.head.0,
                tab.view.preferred_display_column,
                wrap_width.unwrap_or(1),
                tab_width,
                ambiguous_width_wide,
                down,
            )
        } else if down {
            editor::move_down(
                tab.buffer.text(),
                selection.head.0,
                tab.view.preferred_display_column,
                tab_width,
                ambiguous_width_wide,
            )
        } else {
            editor::move_up(
                tab.buffer.text(),
                selection.head.0,
                tab.view.preferred_display_column,
                tab_width,
                ambiguous_width_wide,
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
        let mut paths = self
            .tree
            .entries
            .iter()
            .filter(|entry| entry.kind != TreeEntryKind::Directory)
            .map(|entry| entry.relative_path.clone())
            .collect::<Vec<_>>();
        if self.palette_query.is_empty() {
            let mut ordered = self
                .recent_files
                .iter()
                .filter(|path| paths.contains(path))
                .cloned()
                .collect::<Vec<_>>();
            paths.retain(|path| !ordered.contains(path));
            ordered.extend(paths);
            paths = ordered;
        }
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
        // Resolve the language before taking a mutable borrow of the tab
        // below: `language_for_path` is settings-driven (the same mapping
        // LSP uses) and takes precedence, so syntax highlighting and LSP
        // language detection agree; `from_extension` is only a fallback for
        // extensions with no entry in `settings.languages` (Fix 8).
        let path = self
            .tabs
            .get(tab_index)?
            .buffer
            .path()
            .map(std::path::Path::to_path_buf);
        let language = path.as_deref().and_then(|path| {
            self.language_for_path(path)
                .and_then(|(name, _)| editor::SyntaxLanguage::from_language_name(&name))
                .or_else(|| {
                    path.extension()
                        .and_then(|extension| extension.to_str())
                        .and_then(editor::SyntaxLanguage::from_extension)
                })
        });
        let tab = self.tabs.get_mut(tab_index)?;
        let threshold = self
            .settings
            .editor
            .large_file_threshold_mb
            .saturating_mul(1024 * 1024);
        let Some(language) = language else {
            tab.highlights.clear();
            return None;
        };
        if u64::try_from(tab.buffer.text().len_bytes()).unwrap_or(u64::MAX) > threshold {
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
        Some(Effect::HighlightSyntax {
            tab: tab_index,
            buffer_generation: tab.buffer.generation(),
            syntax_generation: generation,
            language,
            source: tab.buffer.text().clone(),
            cancellation: self.syntax_cancellation.clone(),
        })
    }

    fn start_save_as(&mut self, plan: SaveAsPlan) -> Vec<Effect> {
        if plan.tab >= self.tabs.len() {
            return Vec::new();
        }
        if self.tabs.iter().enumerate().any(|(index, tab)| {
            index != plan.tab && tab.buffer.path() == Some(plan.snapshot.path.as_path())
        }) {
            self.notification = Some(format!(
                "{} is already open in another tab",
                plan.snapshot.path.display()
            ));
            self.overlay = None;
            self.focus = Focus::Editor;
            return Vec::new();
        }
        self.overlay = None;
        self.focus = Focus::Editor;
        self.saving_tabs.insert(plan.tab);
        self.save_as_tabs.insert(plan.tab);
        vec![Effect::SaveAs(plan)]
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
            let recovered_tab = BufferTab::new(buffer);
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

    fn replace_current_match(&mut self, tab_index: usize) -> Vec<Effect> {
        let range = self.tabs.get(tab_index).and_then(|tab| {
            tab.view
                .search
                .matches
                .get(tab.view.search.current)
                .cloned()
        });
        let Some(range) = range else {
            self.notification = Some("No current match to replace".to_owned());
            return Vec::new();
        };
        let replacement = self.buffer_replace_query.clone();
        self.edit(|tab| {
            tab.buffer.set_selection(Selection {
                anchor: CharOffset(range.start),
                head: CharOffset(range.end),
            });
            tab.buffer.insert(&replacement)
        });
        self.notification = Some("Replaced current match".to_owned());
        let mut effects = self.active_post_edit_effects();
        effects.extend(self.start_buffer_search(tab_index));
        effects
    }

    fn replace_all_matches(&mut self, tab_index: usize) -> Vec<Effect> {
        let ranges = self
            .tabs
            .get(tab_index)
            .map(|tab| tab.view.search.matches.clone())
            .unwrap_or_default();
        if ranges.is_empty() {
            self.notification = Some("No matches to replace".to_owned());
            return Vec::new();
        }
        let replacement = self.buffer_replace_query.clone();
        let count = ranges.len();
        self.edit(|tab| {
            tab.buffer
                .replace_all_ranges(&ranges, &replacement)
                .map(|_| ())
        });
        self.notification = Some(format!("Replaced {count} matches"));
        let mut effects = self.active_post_edit_effects();
        effects.extend(self.start_buffer_search(tab_index));
        effects
    }

    fn request_close_tab(&mut self, index: usize) -> Vec<Effect> {
        if index >= self.tabs.len() {
            return Vec::new();
        }
        if !self.saving_tabs.is_empty() {
            self.notification = Some("Wait for pending saves before closing a tab".to_owned());
            return Vec::new();
        }
        if self.tabs[index].pinned {
            self.notification = Some("Unpin the tab before closing it".to_owned());
            return Vec::new();
        }
        if self.tabs[index].buffer.is_dirty() {
            self.overlay = Some(Overlay::ConfirmClose { tab: index });
            self.focus = Focus::Overlay;
        } else {
            return self.close_tab(index).into_iter().collect();
        }
        Vec::new()
    }

    fn close_tab(&mut self, index: usize) -> Option<Effect> {
        if index >= self.tabs.len() || !self.saving_tabs.is_empty() {
            return None;
        }
        let lsp_close = self.tabs[index]
            .buffer
            .path()
            .and_then(|path| {
                self.language_for_path(path)
                    .map(|(language, _)| (path.to_path_buf(), language))
            })
            .filter(|(_, language)| self.lsp_started.contains(language))
            .map(|(path, language)| Effect::SendLsp {
                language,
                message: serde_json::json!({
                    "jsonrpc":"2.0","method":"textDocument/didClose",
                    "params":{"textDocument":{"uri":file_uri(&path)}}
                }),
            });
        self.tabs.remove(index);
        self.tab_order.retain(|tab| *tab != index);
        for tab in &mut self.tab_order {
            if *tab > index {
                *tab -= 1;
            }
        }
        self.tab_drag_source = self.tab_drag_source.and_then(|tab| match tab.cmp(&index) {
            std::cmp::Ordering::Less => Some(tab),
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Greater => Some(tab - 1),
        });
        self.split_tab = self.split_tab.and_then(|split| match split.cmp(&index) {
            std::cmp::Ordering::Less => Some(split),
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Greater => Some(split - 1),
        });
        if self.split_tab.is_none() {
            self.split_focus_right = false;
        }
        self.active_tab = match self.active_tab {
            None => None,
            Some(_) if self.tabs.is_empty() => None,
            Some(active) if active > index => Some(active - 1),
            Some(active) if active == index => Some(index.min(self.tabs.len() - 1)),
            Some(active) => Some(active),
        };
        lsp_close
    }

    fn reorder_tab(&mut self, from: usize, to: usize) {
        if from >= self.tabs.len()
            || to >= self.tabs.len()
            || self.tabs[from].pinned != self.tabs[to].pinned
        {
            return;
        }
        let mut order = self.visual_tab_order();
        let Some(from_position) = order.iter().position(|tab| *tab == from) else {
            return;
        };
        let Some(to_position) = order.iter().position(|tab| *tab == to) else {
            return;
        };
        let tab = order.remove(from_position);
        order.insert(to_position, tab);
        self.tab_order = order;
    }

    fn move_active_tab(&mut self, direction: i32) {
        let Some(active) = self.active_tab else {
            return;
        };
        let order = self.visual_tab_order();
        let Some(position) = order.iter().position(|tab| *tab == active) else {
            return;
        };
        let target = if direction < 0 {
            position.checked_sub(1)
        } else {
            (position + 1 < order.len()).then_some(position + 1)
        };
        let Some(target) = target else {
            return;
        };
        self.reorder_tab(active, order[target]);
    }

    fn reveal_cursor(&mut self, tab_index: usize) {
        let visible_height = self.editor_visible_height();
        let wrap_width = self.tabs.get(tab_index).map(|tab| {
            editor_wrap_width(
                self.terminal_size.0,
                self.sidebar_visible,
                self.settings.ui.sidebar_width,
                self.split_tab.is_some(),
                tab.buffer.text().len_lines(),
            )
        });
        let Some(tab) = self.tabs.get_mut(tab_index) else {
            return;
        };
        let cursor_row = if self.settings.editor.word_wrap {
            editor::visual_index_for_offset(
                tab.buffer.text(),
                tab.buffer.selection().head.0,
                wrap_width.unwrap_or(1),
                usize::from(self.settings.editor.tab_width),
                self.settings.editor.ambiguous_width_wide,
            )
        } else {
            tab.buffer.text().char_to_line(
                tab.buffer
                    .selection()
                    .head
                    .0
                    .min(tab.buffer.text().len_chars()),
            )
        };
        if cursor_row < tab.view.scroll_line {
            tab.view.scroll_line = cursor_row;
        } else if cursor_row >= tab.view.scroll_line + visible_height {
            tab.view.scroll_line = cursor_row.saturating_sub(visible_height - 1);
        }
    }

    fn editor_visible_height(&self) -> usize {
        let (_, terminal_height) = self.terminal_size;
        let panel_height = if self.bottom_panel_visible {
            self.settings
                .ui
                .bottom_panel_height
                .min(terminal_height.saturating_sub(2) / 2)
        } else {
            0
        };
        usize::from(
            terminal_height
                .saturating_sub(3)
                .saturating_sub(panel_height)
                .max(1),
        )
    }

    fn is_large_buffer(&self, buffer: &crate::buffer::TextBuffer) -> bool {
        let threshold = self
            .settings
            .editor
            .large_file_threshold_mb
            .saturating_mul(1024 * 1024);
        u64::try_from(buffer.text().len_bytes()).unwrap_or(u64::MAX) > threshold
    }

    fn reveal_selected_git_hunk(&mut self) {
        let Some(diff) = &self.git_diff else {
            return;
        };
        let mut hunk = 0usize;
        for (line, text) in diff.raw.lines().enumerate() {
            if text.starts_with("@@ ") {
                if hunk == self.git_hunk_selected {
                    self.git_diff_scroll = line;
                    return;
                }
                hunk = hunk.saturating_add(1);
            }
        }
    }
}

fn split_globs(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|pattern| !pattern.is_empty())
        .map(str::to_owned)
        .collect()
}

fn file_uri(path: &std::path::Path) -> String {
    let mut uri = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.' | b'~') {
            uri.push(char::from(byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let encoded = uri.strip_prefix("file://")?;
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let value = std::str::from_utf8(&bytes[index + 1..index + 3])
                .ok()
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())?;
            decoded.push(value);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    Some(PathBuf::from(String::from_utf8(decoded).ok()?))
}

fn lsp_diagnostic(
    path: &std::path::Path,
    source_text: Option<&str>,
    value: &serde_json::Value,
) -> Option<crate::diagnostics::Diagnostic> {
    let range = value.get("range")?;
    let position = |name: &str| -> Option<(usize, usize)> {
        let position = range.get(name)?;
        Some((
            position.get("line")?.as_u64()? as usize,
            position.get("character")?.as_u64()? as usize,
        ))
    };
    let (start_line, start_column) = position("start")?;
    let (end_line, end_column) = position("end")?;
    let char_offset = |line: usize, column: usize| {
        source_text.map(|text| {
            crate::lsp::position_to_char_offset(
                text,
                lsp_types::Position::new(line as u32, column as u32),
            )
        })
    };
    let severity = match value.get("severity").and_then(serde_json::Value::as_u64) {
        Some(1) => crate::diagnostics::DiagnosticSeverity::Error,
        Some(2) => crate::diagnostics::DiagnosticSeverity::Warning,
        Some(3) => crate::diagnostics::DiagnosticSeverity::Information,
        _ => crate::diagnostics::DiagnosticSeverity::Hint,
    };
    let code = value.get("code").and_then(|code| match code {
        serde_json::Value::String(code) => Some(code.clone()),
        serde_json::Value::Number(code) => Some(code.to_string()),
        _ => None,
    });
    Some(crate::diagnostics::Diagnostic {
        file: path.to_path_buf(),
        range: crate::diagnostics::TextRange {
            start: crate::diagnostics::TextPosition {
                line: start_line,
                column: start_column,
                char_offset: char_offset(start_line, start_column),
            },
            end: crate::diagnostics::TextPosition {
                line: end_line,
                column: end_column,
                char_offset: char_offset(end_line, end_column),
            },
        },
        severity,
        message: value.get("message")?.as_str()?.to_owned(),
        source: crate::diagnostics::DiagnosticSource::Lsp,
        code,
        stale: false,
    })
}

fn hover_lines(result: &serde_json::Value) -> Vec<String> {
    let Some(contents) = result.get("contents") else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    let mut append = |value: &serde_json::Value| {
        let text = value
            .as_str()
            .or_else(|| value.get("value").and_then(serde_json::Value::as_str));
        if let Some(text) = text {
            lines.extend(text.lines().map(str::to_owned));
        }
    };
    if let Some(items) = contents.as_array() {
        for item in items {
            append(item);
        }
    } else {
        append(contents);
    }
    lines
}

fn signature_help_lines(result: &serde_json::Value) -> Vec<String> {
    let signatures = result
        .get("signatures")
        .and_then(serde_json::Value::as_array);
    let active = result
        .get("activeSignature")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as usize;
    let Some(signature) = signatures.and_then(|items| items.get(active).or_else(|| items.first()))
    else {
        return Vec::new();
    };
    let mut lines = signature
        .get("label")
        .and_then(serde_json::Value::as_str)
        .map(|label| vec![label.to_owned()])
        .unwrap_or_default();
    let documentation = signature.get("documentation").and_then(|documentation| {
        documentation.as_str().or_else(|| {
            documentation
                .get("value")
                .and_then(serde_json::Value::as_str)
        })
    });
    if let Some(documentation) = documentation {
        lines.extend(documentation.lines().map(str::to_owned));
    }
    lines
}

fn completion_candidate(value: &serde_json::Value) -> Option<crate::app::CompletionCandidate> {
    let label = value.get("label")?.as_str()?.to_owned();
    let is_snippet = value
        .get("insertTextFormat")
        .and_then(serde_json::Value::as_u64)
        == Some(2);
    let raw_insert_text = value
        .get("textEdit")
        .and_then(|edit| edit.get("newText"))
        .and_then(serde_json::Value::as_str)
        .or_else(|| value.get("insertText").and_then(serde_json::Value::as_str))
        .unwrap_or(&label);
    let (insert_text, cursor_char_offset) = if is_snippet {
        expand_lsp_snippet(raw_insert_text)
    } else {
        (raw_insert_text.to_owned(), None)
    };
    Some(crate::app::CompletionCandidate {
        label,
        insert_text,
        detail: value
            .get("detail")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        replace_range: completion_replace_range(value),
        cursor_char_offset,
    })
}

fn expand_lsp_snippet(snippet: &str) -> (String, Option<usize>) {
    let chars = snippet.chars().collect::<Vec<_>>();
    let mut output = String::new();
    let mut placeholders = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '\\' && index + 1 < chars.len() {
            output.push(chars[index + 1]);
            index += 2;
            continue;
        }
        if chars[index] != '$' || index + 1 >= chars.len() {
            output.push(chars[index]);
            index += 1;
            continue;
        }
        if chars[index + 1].is_ascii_digit() {
            let mut end = index + 1;
            while end < chars.len() && chars[end].is_ascii_digit() {
                end += 1;
            }
            let number = chars[index + 1..end]
                .iter()
                .collect::<String>()
                .parse::<usize>()
                .unwrap_or(0);
            placeholders.push((number, output.chars().count()));
            index = end;
            continue;
        }
        if chars[index + 1] == '{' {
            let mut end = index + 2;
            while end < chars.len() && chars[end].is_ascii_digit() {
                end += 1;
            }
            if end == index + 2 {
                output.push('$');
                index += 1;
                continue;
            }
            let number = chars[index + 2..end]
                .iter()
                .collect::<String>()
                .parse::<usize>()
                .unwrap_or(0);
            let start_offset = output.chars().count();
            if chars.get(end) == Some(&':') {
                end += 1;
                while end < chars.len() && chars[end] != '}' {
                    output.push(chars[end]);
                    end += 1;
                }
            }
            if chars.get(end) == Some(&'}') {
                placeholders.push((number, start_offset));
                index = end + 1;
                continue;
            }
        }
        output.push('$');
        index += 1;
    }
    let cursor = placeholders
        .iter()
        .filter(|(number, _)| *number > 0)
        .min_by_key(|(number, _)| *number)
        .or_else(|| placeholders.iter().find(|(number, _)| *number == 0))
        .map(|(_, offset)| *offset);
    (output, cursor)
}

/// Parses the range `textEdit` says the completion's `newText` should
/// replace. Handles both a plain LSP `TextEdit` (`range`) and an
/// `InsertReplaceEdit` (`insert`/`replace`), preferring `insert` for the
/// latter since that is the range that covers what the user already typed.
fn completion_replace_range(
    value: &serde_json::Value,
) -> Option<(lsp_types::Position, lsp_types::Position)> {
    let edit = value.get("textEdit")?;
    let range = edit.get("range").or_else(|| edit.get("insert"))?;
    let start = completion_position(range.get("start")?)?;
    let end = completion_position(range.get("end")?)?;
    Some((start, end))
}

fn completion_position(value: &serde_json::Value) -> Option<lsp_types::Position> {
    Some(lsp_types::Position::new(
        value.get("line")?.as_u64()? as u32,
        value.get("character")?.as_u64()? as u32,
    ))
}

fn lsp_location(value: &serde_json::Value) -> Option<crate::app::NavigationLocation> {
    let path = value
        .get("uri")
        .or_else(|| value.get("targetUri"))
        .and_then(serde_json::Value::as_str)
        .and_then(file_uri_to_path)?;
    let range = value
        .get("range")
        .or_else(|| value.get("targetSelectionRange"))?;
    let start = range.get("start")?;
    Some(crate::app::NavigationLocation {
        path,
        line: start.get("line")?.as_u64()? as usize + 1,
        column: ColumnHint::Utf16(start.get("character")?.as_u64()? as usize + 1),
    })
}

fn lsp_text_edit(
    source: &str,
    value: &serde_json::Value,
) -> Option<(std::ops::Range<usize>, String)> {
    let range = value.get("range")?;
    let start = completion_position(range.get("start")?)?;
    let end = completion_position(range.get("end")?)?;
    let mut start = crate::lsp::position_to_char_offset(source, start);
    let mut end = crate::lsp::position_to_char_offset(source, end);
    if start > end {
        std::mem::swap(&mut start, &mut end);
    }
    Some((start..end, value.get("newText")?.as_str()?.to_owned()))
}

fn git_error_guidance(error: &str) -> String {
    let lower = error.to_ascii_lowercase();
    let guidance = if lower.contains("authentication failed")
        || lower.contains("could not read username")
        || lower.contains("permission denied (publickey)")
    {
        "Authentication failed. Verify the remote URL and configure HTTPS credentials or an SSH key."
    } else if lower.contains("non-fast-forward") || lower.contains("rejected") {
        "Push was rejected because the remote has newer commits. Pull or fetch and reconcile the branch before pushing again."
    } else if lower.contains("no upstream branch") || lower.contains("has no upstream") {
        "This branch has no upstream. Configure one with `git push --set-upstream <remote> <branch>`."
    } else if lower.contains("author identity unknown")
        || lower.contains("please tell me who you are")
        || lower.contains("user.email")
    {
        "Git author identity is not configured. Set `git config --global user.name` and `git config --global user.email`."
    } else if lower.contains("unmerged files") || lower.contains("resolve your current index") {
        "Git found unresolved merge conflicts. Resolve and stage conflicted files before retrying."
    } else {
        return error.to_owned();
    };
    guidance.to_owned()
}

fn editor_wrap_width(
    terminal_width: u16,
    sidebar_visible: bool,
    configured_sidebar_width: u16,
    split: bool,
    line_count: usize,
) -> usize {
    let sidebar_width = if sidebar_visible {
        configured_sidebar_width.min(terminal_width.saturating_sub(24))
    } else {
        0
    };
    let editor_width = terminal_width
        .saturating_sub(3)
        .saturating_sub(sidebar_width);
    let group_width = if split {
        editor_width.saturating_sub(1) / 2
    } else {
        editor_width
    };
    usize::from(group_width)
        .saturating_sub(line_count.to_string().len().max(2) + 3)
        .max(1)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::session::{RecoveryBuffer, RecoverySet, RestoredBuffer, RestoredSession};
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
        state.tabs.push(BufferTab::new(buffer));
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
        state.tabs.push(BufferTab::new(buffer));
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
        state.tabs.push(BufferTab::new(buffer));
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
    fn buffer_replace_all_is_command_driven_and_undoable() {
        let mut state = state();
        let mut buffer = TextBuffer::empty(None, false);
        buffer.insert("one two one").unwrap();
        state.tabs.push(BufferTab::new(buffer));
        state.active_tab = Some(0);
        state.update(AppEvent::Command(Command::Invoke(
            command::EDITOR_REPLACE.to_owned(),
        )));
        let mut effects = Vec::new();
        for character in "one".chars() {
            effects = state.update(AppEvent::Command(Command::PaletteInput(character)));
        }
        let Effect::SearchBuffer {
            tab,
            buffer_generation,
            search_generation,
            query,
            source,
            cancellation,
        } = effects.into_iter().next().expect("search effect")
        else {
            panic!("expected search effect");
        };
        let matches = crate::search::find_matches(
            &source.to_string(),
            &query,
            &cancellation,
            search_generation,
            100,
        );
        state.update(AppEvent::BufferSearchCompleted {
            tab,
            buffer_generation,
            search_generation,
            query,
            matches,
        });
        state.update(AppEvent::Command(Command::SearchToggleReplaceField));
        state.update(AppEvent::Command(Command::PaletteInput('1')));
        state.update(AppEvent::Command(Command::ReplaceAll));
        assert_eq!(state.tabs[0].buffer.text_string(), "1 two 1");
        state.tabs[0].buffer.undo().unwrap();
        assert_eq!(state.tabs[0].buffer.text_string(), "one two one");
    }

    #[test]
    fn navigation_history_moves_back_and_forward_between_files() {
        let mut state = state();
        let first = state.workspace.as_path().join("first.rs");
        let second = state.workspace.as_path().join("second.rs");
        let mut first_buffer = TextBuffer::empty(Some(first.clone()), false);
        first_buffer.insert("first\nline").unwrap();
        first_buffer.set_selection(Selection::caret(CharOffset(7)));
        state.tabs.push(BufferTab::new(first_buffer));
        state.tabs.push(BufferTab::new(TextBuffer::empty(
            Some(second.clone()),
            false,
        )));
        state.active_tab = Some(0);
        state.record_navigation_origin();
        state.active_tab = Some(1);

        let effects = state.update(AppEvent::Command(Command::Invoke(
            command::EDITOR_NAVIGATE_BACK.to_owned(),
        )));
        assert!(matches!(
            effects.as_slice(),
            [Effect::OpenFile { path, line: Some(2), .. }] if path == &first
        ));
        state.active_tab = Some(0);
        let effects = state.update(AppEvent::Command(Command::Invoke(
            command::EDITOR_NAVIGATE_FORWARD.to_owned(),
        )));
        assert!(matches!(
            effects.as_slice(),
            [Effect::OpenFile { path, .. }] if path == &second
        ));
    }

    #[test]
    fn source_control_commands_dispatch_selected_file_operations() {
        let mut state = state();
        state.update(AppEvent::GitStatusLoaded(Ok(crate::git::GitStatus {
            branch: Some("main".to_owned()),
            files: vec![crate::git::GitFileChange {
                path: PathBuf::from("src/main.rs"),
                original_path: None,
                kind: crate::git::GitFileKind::Modified,
                index_status: '.',
                worktree_status: 'M',
                staged: false,
                unstaged: true,
                conflicted: false,
                untracked: false,
            }],
            ..Default::default()
        })));

        let effects = state.update(AppEvent::Command(Command::Invoke(
            command::GIT_STAGE.to_owned(),
        )));
        assert!(matches!(
            effects.as_slice(),
            [Effect::GitOperation(crate::app::GitOperation::Stage(path))]
                if path == &PathBuf::from("src/main.rs")
        ));

        state.update(AppEvent::Command(Command::Invoke(
            command::GIT_RESTORE.to_owned(),
        )));
        assert!(matches!(
            state.overlay,
            Some(Overlay::ConfirmGitRestore { ref path })
                if path == &PathBuf::from("src/main.rs")
        ));
    }

    #[test]
    fn commit_requires_staged_changes_and_collects_message() {
        let mut state = state();
        state.update(AppEvent::Command(Command::Invoke(
            command::GIT_COMMIT.to_owned(),
        )));
        assert_eq!(
            state.notification.as_deref(),
            Some("No staged changes to commit")
        );

        state.git_status = Some(crate::git::GitStatus {
            files: vec![crate::git::GitFileChange {
                path: PathBuf::from("new.rs"),
                original_path: None,
                kind: crate::git::GitFileKind::Added,
                index_status: 'A',
                worktree_status: '.',
                staged: true,
                unstaged: false,
                conflicted: false,
                untracked: false,
            }],
            ..Default::default()
        });
        state.update(AppEvent::Command(Command::Invoke(
            command::GIT_COMMIT.to_owned(),
        )));
        state.update(AppEvent::Command(Command::PaletteInput('o')));
        state.update(AppEvent::Command(Command::PaletteInput('k')));
        state.update(AppEvent::Command(Command::PaletteNewline));
        state.update(AppEvent::Command(Command::PaletteInput('!')));
        let effects = state.update(AppEvent::Command(Command::PaletteAccept));
        assert!(matches!(
            effects.as_slice(),
            [Effect::GitOperation(crate::app::GitOperation::Commit(message))]
                if message == "ok\n!"
        ));
    }

    #[test]
    fn branch_picker_filters_and_dispatches_switch() {
        let mut state = state();
        let effects = state.update(AppEvent::Command(Command::Invoke(
            command::GIT_BRANCH_SWITCH.to_owned(),
        )));
        assert!(matches!(effects.as_slice(), [Effect::LoadGitBranches]));
        state.update(AppEvent::GitBranchesLoaded(Ok(vec![
            crate::git::GitBranch {
                name: "main".to_owned(),
                current: true,
                remote: false,
                upstream: None,
            },
            crate::git::GitBranch {
                name: "feature".to_owned(),
                current: false,
                remote: false,
                upstream: None,
            },
        ])));
        for character in "feat".chars() {
            state.update(AppEvent::Command(Command::PaletteInput(character)));
        }
        let effects = state.update(AppEvent::Command(Command::PaletteAccept));
        assert!(matches!(
            effects.as_slice(),
            [Effect::GitOperation(crate::app::GitOperation::SwitchBranch(branch))]
                if branch == "feature"
        ));
    }

    #[test]
    fn branch_completion_refreshes_tree_and_open_buffers() {
        let mut state = state();
        let path = state.workspace.as_path().join("open.rs");
        state
            .tabs
            .push(BufferTab::new(TextBuffer::empty(Some(path.clone()), false)));
        let effects = state.update(AppEvent::GitOperationCompleted {
            result: Ok("Switched".to_owned()),
            workspace_changed: true,
        });
        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, Effect::ScanWorkspace))
        );
        assert!(effects.iter().any(|effect| matches!(
            effect,
            Effect::RefreshOpenFiles { files, .. } if files == &vec![(0, path.clone())]
        )));
    }

    #[test]
    fn diff_hunk_commands_stage_restore_and_open_target_line() {
        let mut state = state();
        let raw = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -3 +3 @@\n-old\n+new\n";
        let diff = crate::git::parse_unified_diff(
            PathBuf::from("src/a.rs"),
            crate::git::DiffTarget::WorkingTree,
            raw.to_owned(),
        );
        state.update(AppEvent::GitDiffLoaded(Ok(diff)));

        assert!(state.git_diff_active);
        assert_eq!(state.focus, Focus::Editor);
        assert!(state.active_tab().is_none());

        let effects = state.update(AppEvent::Command(Command::GitHunkStageToggle));
        assert!(matches!(
            effects.as_slice(),
            [Effect::GitOperation(crate::app::GitOperation::StageHunk(patch))]
                if patch.contains("@@ -3 +3 @@")
        ));

        state.update(AppEvent::Command(Command::GitHunkRestore));
        assert!(matches!(
            state.overlay,
            Some(Overlay::ConfirmGitHunkRestore { .. })
        ));
        let effects = state.update(AppEvent::Command(Command::PaletteAccept));
        assert!(matches!(
            effects.as_slice(),
            [Effect::GitOperation(crate::app::GitOperation::RestoreHunk(
                _
            ))]
        ));

        let effects = state.update(AppEvent::Command(Command::GitHunkOpenFile));
        assert!(matches!(
            effects.as_slice(),
            [Effect::OpenFile { path, line: Some(3), .. }]
                if path == &state.workspace.as_path().join("src/a.rs")
        ));
    }

    #[test]
    fn diff_tab_is_read_only_and_closes_without_touching_file_tabs() {
        let mut state = state();
        let path = state.workspace.as_path().join("src/a.rs");
        let mut buffer = TextBuffer::empty(Some(path), false);
        buffer.insert("kept").unwrap();
        state.tabs.push(BufferTab::new(buffer));
        state.active_tab = Some(0);
        let diff = crate::git::parse_unified_diff(
            PathBuf::from("src/a.rs"),
            crate::git::DiffTarget::WorkingTree,
            "@@ -1 +1 @@\n-old\n+new\n".to_owned(),
        );
        state.update(AppEvent::GitDiffLoaded(Ok(diff)));

        state.update(AppEvent::Command(Command::InsertText("blocked".to_owned())));
        assert_eq!(state.tabs[0].buffer.text().to_string(), "kept");
        assert_eq!(
            state.notification.as_deref(),
            Some("Git diff tabs are read-only")
        );
        state.tabs[0].buffer.set_selection(Selection {
            anchor: CharOffset(0),
            head: CharOffset(4),
        });
        state.update(AppEvent::Command(Command::Invoke(
            command::EDITOR_CUT.to_owned(),
        )));
        assert_eq!(state.tabs[0].buffer.text().to_string(), "kept");

        state.update(AppEvent::Command(Command::CloseTab(1)));
        assert!(state.git_diff.is_none());
        assert!(!state.git_diff_active);
        assert_eq!(state.tabs.len(), 1);
        assert_eq!(state.active_tab, Some(0));
    }

    #[test]
    fn editor_scroll_and_split_groups_keep_independent_active_tabs() {
        let mut state = state();
        for name in ["left.rs", "right.rs"] {
            let mut buffer = TextBuffer::empty(Some(state.workspace.as_path().join(name)), false);
            buffer.insert(&format!("{name}\n").repeat(40)).unwrap();
            state.tabs.push(BufferTab::new(buffer));
        }
        state.active_tab = Some(0);
        state.update(AppEvent::Command(Command::EditorScroll(8)));
        assert_eq!(state.tabs[0].view.scroll_line, 8);

        state.update(AppEvent::Command(Command::SplitEditor));
        state.update(AppEvent::Command(Command::SelectTab(1)));
        assert_eq!(state.active_tab, Some(1));
        assert_eq!(state.split_tab, Some(0));

        state.update(AppEvent::Command(Command::FocusNextEditorGroup));
        assert_eq!(state.active_tab, Some(0));
        assert_eq!(state.split_tab, Some(1));
        assert!(state.split_focus_right);
        assert_eq!(state.tabs[0].view.scroll_line, 8);
    }

    #[test]
    fn tabs_pin_reorder_and_close_without_changing_stable_buffer_indices() {
        let mut state = state();
        for name in ["one.rs", "two.rs", "three.rs"] {
            state.tabs.push(BufferTab::new(TextBuffer::empty(
                Some(state.workspace.as_path().join(name)),
                false,
            )));
        }
        state.active_tab = Some(1);
        state.update(AppEvent::Command(Command::TogglePinTab(1)));
        assert_eq!(state.visual_tab_order(), vec![1, 0, 2]);
        assert_eq!(state.active_tab, Some(1));

        state.update(AppEvent::Command(Command::ReorderTab { from: 2, to: 0 }));
        assert_eq!(state.visual_tab_order(), vec![1, 2, 0]);
        state.update(AppEvent::Command(Command::ReorderTab { from: 1, to: 2 }));
        assert_eq!(state.visual_tab_order(), vec![1, 2, 0]);

        state.update(AppEvent::Command(Command::CloseTab(1)));
        assert_eq!(state.tabs.len(), 3, "pinned tab must be protected");
        state.update(AppEvent::Command(Command::TogglePinTab(1)));
        state.update(AppEvent::Command(Command::CloseTab(1)));
        assert_eq!(state.tabs.len(), 2);
        assert_eq!(state.visual_tab_order(), vec![1, 0]);
    }

    #[test]
    fn session_snapshot_preserves_visual_tab_order_and_pin_state() {
        let mut state = state();
        for name in ["one.rs", "two.rs"] {
            state.tabs.push(BufferTab::new(TextBuffer::empty(
                Some(state.workspace.as_path().join(name)),
                false,
            )));
        }
        state.tabs[1].pinned = true;
        state.active_tab = Some(0);
        let snapshot = state.session_snapshot();
        assert_eq!(
            snapshot.buffers[0]
                .path
                .as_ref()
                .and_then(|path| path.file_name())
                .and_then(|name| name.to_str()),
            Some("two.rs")
        );
        assert!(snapshot.buffers[0].pinned);
        assert_eq!(snapshot.active_tab, Some(1));
    }

    #[test]
    fn restored_tab_order_survives_out_of_order_file_completion() {
        let mut state = state();
        let first = state.workspace.as_path().join("first.rs");
        let second = state.workspace.as_path().join("second.rs");
        state.apply_restored_session(&RestoredSession {
            buffers: vec![
                RestoredBuffer {
                    path: first.clone(),
                    cursor_char: 0,
                    pinned: true,
                },
                RestoredBuffer {
                    path: second.clone(),
                    cursor_char: 0,
                    pinned: false,
                },
            ],
            active_path: Some(second.clone()),
            sidebar_visible: true,
            sidebar_view: SidebarView::Explorer,
            bottom_panel_visible: false,
            sidebar_width: 30,
            bottom_panel_height: 12,
        });
        for path in [second.clone(), first.clone()] {
            state.update(AppEvent::FileOpened {
                path: path.clone(),
                line: None,
                column: None,
                result: Ok(TextBuffer::empty(Some(path), false)),
            });
        }
        let paths = state
            .visual_tab_order()
            .into_iter()
            .filter_map(|index| state.tabs[index].buffer.path().map(PathBuf::from))
            .collect::<Vec<_>>();
        assert_eq!(paths, vec![first, second.clone()]);
        assert!(state.tabs[state.visual_tab_order()[0]].pinned);
        assert_eq!(state.active_path(), Some(second.as_path()));
    }

    #[test]
    fn diff_next_previous_reveal_the_selected_hunk() {
        let mut state = state();
        let diff = crate::git::parse_unified_diff(
            PathBuf::from("src/a.rs"),
            crate::git::DiffTarget::WorkingTree,
            "@@ -1 +1 @@\n-old\n+new\n@@ -20 +20 @@\n-before\n+after\n".to_owned(),
        );
        state.update(AppEvent::GitDiffLoaded(Ok(diff)));
        state.update(AppEvent::Command(Command::GitHunkNext));
        assert_eq!(state.git_hunk_selected, 1);
        assert_eq!(state.git_diff_scroll, 3);
        state.update(AppEvent::Command(Command::GitHunkPrevious));
        assert_eq!(state.git_hunk_selected, 0);
        assert_eq!(state.git_diff_scroll, 0);
    }

    #[test]
    fn workspace_search_effect_uses_open_buffer_text_and_ignores_stale_batches() {
        let mut state = state();
        let path = state.workspace.as_path().join("live.rs");
        let mut buffer = TextBuffer::empty(Some(path.clone()), false);
        buffer.insert("unsaved needle").unwrap();
        state.tabs.push(BufferTab::new(buffer));
        state.sidebar_view = SidebarView::Search;

        let effects = state.update(AppEvent::Command(Command::WorkspaceSearchInput('n')));
        let [
            Effect::SearchWorkspace {
                generation,
                open_buffers,
                ..
            },
        ] = effects.as_slice()
        else {
            panic!("expected workspace search effect");
        };
        assert_eq!(
            open_buffers.get(&path).map(String::as_str),
            Some("unsaved needle")
        );

        state.update(AppEvent::WorkspaceSearchBatch {
            generation: generation.saturating_sub(1),
            matches: vec![crate::search::WorkspaceMatch {
                path: PathBuf::from("stale.rs"),
                line: 1,
                column: 1,
                line_text: "n".to_owned(),
                match_start: 0,
                match_end: 1,
            }],
            done: true,
            error: None,
        });
        assert!(state.workspace_matches.is_empty());
    }

    #[test]
    fn workspace_search_rows_group_and_collapse_files() {
        let mut state = state();
        state.workspace_matches = vec![
            crate::search::WorkspaceMatch {
                path: PathBuf::from("a.rs"),
                line: 1,
                column: 1,
                line_text: "x".to_owned(),
                match_start: 0,
                match_end: 1,
            },
            crate::search::WorkspaceMatch {
                path: PathBuf::from("a.rs"),
                line: 2,
                column: 1,
                line_text: "x".to_owned(),
                match_start: 0,
                match_end: 1,
            },
        ];
        assert_eq!(state.workspace_search_rows().len(), 3);
        state.update(AppEvent::Command(Command::WorkspaceSearchToggleFile(
            PathBuf::from("a.rs"),
        )));
        assert_eq!(state.workspace_search_rows().len(), 1);
    }

    #[test]
    fn terminal_toggle_starts_session_and_stale_output_is_ignored() {
        let mut state = state();
        state.terminal_size = (100, 30);
        let effects = state.update(AppEvent::Command(Command::Invoke(
            command::TERMINAL_TOGGLE.to_owned(),
        )));
        let [
            Effect::StartTerminal {
                generation,
                cwd,
                rows,
                cols,
                ..
            },
        ] = effects.as_slice()
        else {
            panic!("expected terminal start");
        };
        assert_eq!(*generation, state.terminal_generation);
        assert_eq!(cwd, state.workspace.as_path());
        assert!(*rows > 0 && *cols > 0);
        assert_eq!(state.bottom_panel_view, BottomPanelView::Terminal);

        state.update(AppEvent::TerminalOutput {
            generation: generation.saturating_sub(1),
            bytes: b"stale".to_vec(),
        });
        assert!(
            state
                .terminal
                .snapshot(0)
                .lines
                .iter()
                .flatten()
                .all(|cell| cell.character != 's')
        );
        state.update(AppEvent::TerminalOutput {
            generation: *generation,
            bytes: b"live".to_vec(),
        });
        assert_eq!(state.terminal.snapshot(0).lines[0][0].character, 'l');
    }

    #[test]
    fn quitting_with_running_terminal_requires_confirmation() {
        let mut state = state();
        state.terminal_running = true;
        state.update(AppEvent::Command(Command::Invoke(
            command::APP_QUIT.to_owned(),
        )));
        assert!(!state.should_quit);
        assert!(matches!(state.overlay, Some(Overlay::ConfirmQuitTerminal)));
        let effects = state.update(AppEvent::Command(Command::PaletteAccept));
        assert!(state.should_quit);
        assert!(matches!(effects.as_slice(), [Effect::StopTerminal]));
    }

    #[test]
    fn terminal_selection_copy_uses_shared_clipboard_effect() {
        let mut state = state();
        state.terminal.feed(b"copy me");
        state.update(AppEvent::Command(Command::TerminalSetSelection {
            row: 0,
            column: 0,
            extend: false,
        }));
        state.update(AppEvent::Command(Command::TerminalSetSelection {
            row: 0,
            column: 3,
            extend: true,
        }));
        let effects = state.update(AppEvent::Command(Command::TerminalCopy));
        assert_eq!(state.internal_clipboard, "copy");
        assert!(matches!(
            effects.as_slice(),
            [Effect::CopyToClipboard(text)] if text == "copy"
        ));
    }

    #[test]
    fn terminal_file_reference_command_opens_workspace_location() {
        let mut state = state();
        fs::create_dir_all(state.workspace.as_path().join("src")).unwrap();
        fs::write(state.workspace.as_path().join("src/main.rs"), "a\nb\n").unwrap();
        state.terminal.feed(b"error src/main.rs:2:1");
        let effects = state.update(AppEvent::Command(Command::TerminalOpenReference {
            row: 0,
            column: Some(12),
        }));
        assert!(matches!(
            effects.as_slice(),
            [Effect::OpenFile {
                path,
                line: Some(2),
                column: Some(ColumnHint::Chars(1)),
                ..
            }] if path == &state.workspace.as_path().join("src/main.rs")
        ));
    }

    #[test]
    fn terminal_search_command_finds_scrollback_and_reveals_matches() {
        let mut state = state();
        for line in 0..20 {
            state
                .terminal
                .feed(format!("line {line} needle\r\n").as_bytes());
        }

        state.update(AppEvent::Command(Command::Invoke(
            command::TERMINAL_SEARCH.to_owned(),
        )));
        for character in "needle".chars() {
            state.update(AppEvent::Command(Command::PaletteInput(character)));
        }

        assert!(matches!(state.overlay, Some(Overlay::TerminalSearch)));
        assert_eq!(state.terminal_search_matches.len(), 20);
        assert!(state.terminal_scroll_offset > 0);
        let first = state.terminal_search_selected;
        state.update(AppEvent::Command(Command::TerminalSearchNext));
        assert_eq!(state.terminal_search_selected, first + 1);
        state.update(AppEvent::Command(Command::TerminalSearchPrevious));
        assert_eq!(state.terminal_search_selected, first);
    }

    #[test]
    fn configuration_reload_command_and_result_update_runtime_settings() {
        let mut state = state();
        let effects = state.update(AppEvent::Command(Command::Invoke(
            command::CONFIG_RELOAD.to_owned(),
        )));
        assert!(matches!(effects.as_slice(), [Effect::ReloadConfig]));

        let mut settings = state.settings.clone();
        settings.editor.tab_width = 8;
        settings
            .keymap
            .insert("alt-r".to_owned(), command::CONFIG_RELOAD.to_owned());
        let (keymap, warnings) = crate::config::Keymap::from_overrides(&settings.keymap);
        state.update(AppEvent::ConfigReloaded {
            settings,
            keymap,
            warnings,
        });

        assert_eq!(state.settings.editor.tab_width, 8);
        assert_eq!(
            state.notification.as_deref(),
            Some("Configuration reloaded")
        );
    }

    #[test]
    fn configuration_open_prepares_workspace_file_then_opens_it() {
        let mut state = state();
        let expected = state.workspace.as_path().join(".mica/config.toml");
        let effects = state.update(AppEvent::Command(Command::Invoke(
            command::CONFIG_OPEN.to_owned(),
        )));
        assert!(matches!(
            effects.as_slice(),
            [Effect::PrepareConfigFile(path)] if path == &expected
        ));

        let effects = state.update(AppEvent::ConfigFilePrepared(Ok(expected.clone())));
        assert!(matches!(
            effects.as_slice(),
            [Effect::OpenFile { path, line: None, column: None, .. }] if path == &expected
        ));
    }

    #[test]
    fn keybinding_help_filters_for_focus_and_restores_it_on_close() {
        let mut state = state();
        state.focus = Focus::Editor;
        state.update(AppEvent::Command(Command::Invoke(
            command::HELP_KEYBINDINGS.to_owned(),
        )));
        let editor_commands = state
            .active_keybindings()
            .into_iter()
            .map(|(_, id, _)| id.to_owned())
            .collect::<Vec<_>>();
        assert!(editor_commands.iter().any(|id| id == command::EDITOR_SAVE));
        assert!(
            !editor_commands
                .iter()
                .any(|id| id == command::TERMINAL_SEARCH)
        );
        state.update(AppEvent::Command(Command::Cancel));
        assert_eq!(state.focus, Focus::Editor);

        state.focus = Focus::BottomPanel;
        state.bottom_panel_view = BottomPanelView::Terminal;
        state.update(AppEvent::Command(Command::Invoke(
            command::HELP_KEYBINDINGS.to_owned(),
        )));
        let terminal_commands = state
            .active_keybindings()
            .into_iter()
            .map(|(_, id, _)| id.to_owned())
            .collect::<Vec<_>>();
        assert!(
            terminal_commands
                .iter()
                .any(|id| id == command::TERMINAL_SEARCH)
        );
        assert!(
            !terminal_commands
                .iter()
                .any(|id| id == command::EDITOR_SAVE)
        );
    }

    #[test]
    fn notifications_are_retained_and_expire_as_toasts() {
        let mut state = state();
        state.update(AppEvent::Command(Command::Invoke(
            "unknown.command".to_owned(),
        )));
        assert_eq!(state.notification_history.len(), 1);
        assert!(state.notification_expires_at.is_some());
        state.update(AppEvent::ConfigFilePrepared(Err(
            "permission denied".to_owned()
        )));
        assert_eq!(
            state.notification_history.back().map(|entry| entry.level),
            Some(crate::app::NotificationLevel::Error)
        );

        state.notification_expires_at = Some(std::time::Instant::now());
        state.update(AppEvent::Tick);
        assert!(state.notification.is_none());
        assert_eq!(state.notification_history.len(), 2);

        for index in 0..205 {
            state.notify(format!("message {index}"));
        }
        assert_eq!(state.notification_history.len(), 200);
        assert_eq!(
            state
                .notification_history
                .front()
                .map(|entry| entry.message.as_str()),
            Some("message 5")
        );
    }

    #[test]
    fn notification_history_opens_and_restores_focus() {
        let mut state = state();
        state.focus = Focus::BottomPanel;
        state.update(AppEvent::Command(Command::Invoke(
            command::NOTIFICATIONS_HISTORY.to_owned(),
        )));
        assert!(matches!(state.overlay, Some(Overlay::NotificationHistory)));
        state.update(AppEvent::Command(Command::Cancel));
        assert_eq!(state.focus, Focus::BottomPanel);
    }

    #[test]
    fn utf16_column_hint_lands_past_non_bmp_characters_on_the_target_line() {
        let mut state = state();
        let path = state.workspace.as_path().join("unicode_definition.rs");
        let mut buffer = TextBuffer::empty(Some(path.clone()), false);
        // Line 2 (1-based) contains two non-BMP emoji before `foo`; a naive
        // "1 UTF-16 unit == 1 char" conversion would land inside the string
        // literal instead of at `foo`.
        buffer.insert("a\nlet s = \"😀😀\"; foo();\n").unwrap();
        // `character` 16 (0-based UTF-16 units) is where `foo` starts on that
        // line; requests are carried 1-based, so the hint is 17.
        state.update(AppEvent::FileOpened {
            path: path.clone(),
            line: Some(2),
            column: Some(ColumnHint::Utf16(17)),
            result: Ok(buffer),
        });
        let tab = &state.tabs[state.active_tab.unwrap()];
        let line_start = tab.buffer.text().line_to_char(1);
        let cursor = tab.buffer.selection().head.0;
        assert_eq!(cursor - line_start, 14, "cursor should land right at `foo`");
        let rest_of_line = tab.buffer.text().slice(cursor..).to_string();
        assert!(
            rest_of_line.starts_with("foo"),
            "expected cursor at `foo`, got: {rest_of_line:?}"
        );
    }

    #[test]
    fn completion_candidate_parses_insert_replace_edit_using_the_insert_range() {
        let item = serde_json::json!({
            "label": "version",
            "textEdit": {
                "insert": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 5}},
                "replace": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 9}},
                "newText": "version"
            }
        });
        let candidate = completion_candidate(&item).expect("valid completion item");
        assert_eq!(candidate.insert_text, "version");
        assert_eq!(
            candidate.replace_range,
            Some((
                lsp_types::Position::new(1, 2),
                lsp_types::Position::new(1, 5)
            ))
        );
    }

    #[test]
    fn completion_candidate_expands_snippet_defaults_and_places_cursor() {
        let item = serde_json::json!({
            "label": "println!",
            "insertTextFormat": 2,
            "insertText": "println!(\"${1:value}\");$0"
        });
        let candidate = completion_candidate(&item).expect("valid completion item");
        assert_eq!(candidate.insert_text, "println!(\"value\");");
        assert_eq!(candidate.cursor_char_offset, Some(10));
    }

    #[test]
    fn signature_help_selects_active_signature_and_documentation() {
        let lines = signature_help_lines(&serde_json::json!({
            "activeSignature": 1,
            "signatures": [
                {"label": "first()"},
                {"label": "target(value: i32)", "documentation": {"kind": "markdown", "value": "Target docs"}}
            ]
        }));
        assert_eq!(lines, vec!["target(value: i32)", "Target docs"]);
    }

    #[test]
    fn git_failures_receive_actionable_guidance_without_losing_raw_output() {
        assert!(git_error_guidance("fatal: Authentication failed").contains("SSH key"));
        assert!(
            git_error_guidance("! [rejected] main -> main (non-fast-forward)")
                .contains("remote has newer")
        );
        assert!(
            git_error_guidance("Author identity unknown; please tell me who you are")
                .contains("user.name")
        );
        assert_eq!(git_error_guidance("unknown failure"), "unknown failure");
    }

    #[test]
    fn cargo_diagnostics_save_triggers_are_coalesced_while_running() {
        let mut state = state();
        state.settings.diagnostics.enabled = true;
        fs::write(
            state.workspace.as_path().join("Cargo.toml"),
            "[package]\nname='x'",
        )
        .unwrap();
        let first = state.start_cargo_diagnostics(false);
        assert!(matches!(
            first.as_slice(),
            [Effect::RunCargoDiagnostics { generation: 1 }]
        ));
        assert!(state.start_cargo_diagnostics(false).is_empty());
        assert!(state.cargo_diagnostics_pending);
        let next = state.update(AppEvent::DiagnosticsReplaced {
            source: crate::diagnostics::DiagnosticSource::Compiler,
            generation: 1,
            diagnostics: Vec::new(),
        });
        assert!(matches!(
            next.as_slice(),
            [Effect::RunCargoDiagnostics { generation: 2 }]
        ));
        assert!(state.cargo_diagnostics_running);
        assert!(!state.cargo_diagnostics_pending);
    }

    #[test]
    fn accepting_a_completion_replaces_the_textedit_range_instead_of_duplicating_the_prefix() {
        let mut state = state();
        let path = state.workspace.as_path().join("main.rs");
        let mut buffer = TextBuffer::empty(Some(path.clone()), false);
        buffer.insert("ver").unwrap();
        state.tabs.push(BufferTab::new(buffer));
        state.active_tab = Some(0);
        state.lsp_started.insert("rust".to_owned());

        let effects = state.update(AppEvent::Command(Command::Invoke(
            command::LSP_COMPLETION.to_owned(),
        )));
        let id = effects
            .iter()
            .find_map(|effect| match effect {
                Effect::SendLsp { message, .. } => message.get("id")?.as_u64(),
                _ => None,
            })
            .unwrap();
        state.update(AppEvent::LspClient {
            language: "rust".to_owned(),
            event: crate::lsp::LspClientEvent::Message(serde_json::json!({
                "jsonrpc":"2.0","id":id,"result":{"items":[{
                    "label":"version",
                    "textEdit": {
                        "range": {"start":{"line":0,"character":0},"end":{"line":0,"character":3}},
                        "newText": "version"
                    }
                }]}
            })),
        });
        assert!(matches!(state.overlay, Some(Overlay::LspCompletion)));

        state.update(AppEvent::Command(Command::PaletteAccept));
        assert_eq!(
            state.tabs[0].buffer.text_string(),
            "version",
            "accepting the completion must replace `ver`, not append to it"
        );
    }

    #[test]
    fn stale_hover_and_completion_responses_are_discarded_after_switching_tabs() {
        let mut state = state();
        let path_a = state.workspace.as_path().join("a.rs");
        let path_b = state.workspace.as_path().join("b.rs");
        for path in [&path_a, &path_b] {
            let mut buffer = TextBuffer::empty(Some(path.clone()), false);
            buffer.insert("fn main() {}").unwrap();
            state.tabs.push(BufferTab::new(buffer));
        }
        state.active_tab = Some(0);
        state.lsp_started.insert("rust".to_owned());

        let effects = state.update(AppEvent::Command(Command::Invoke(
            command::LSP_HOVER.to_owned(),
        )));
        let hover_id = effects
            .iter()
            .find_map(|effect| match effect {
                Effect::SendLsp { message, .. } => message.get("id")?.as_u64(),
                _ => None,
            })
            .unwrap();

        // The user switches to another tab before the response arrives.
        state.active_tab = Some(1);
        state.update(AppEvent::LspClient {
            language: "rust".to_owned(),
            event: crate::lsp::LspClientEvent::Message(serde_json::json!({
                "jsonrpc":"2.0","id":hover_id,"result":{"contents":"stale hover"}
            })),
        });
        assert!(
            !matches!(state.overlay, Some(Overlay::LspHover)),
            "a stale hover response must not open the hover overlay"
        );
        assert_eq!(
            state.notification.as_deref(),
            Some("Stale LSP response discarded")
        );
        assert!(state.lsp_hover.is_empty());

        // Same story for completion: request from tab 0, respond after
        // switching to tab 1.
        state.active_tab = Some(0);
        let effects = state.update(AppEvent::Command(Command::Invoke(
            command::LSP_COMPLETION.to_owned(),
        )));
        let completion_id = effects
            .iter()
            .find_map(|effect| match effect {
                Effect::SendLsp { message, .. } => message.get("id")?.as_u64(),
                _ => None,
            })
            .unwrap();
        state.active_tab = Some(1);
        state.update(AppEvent::LspClient {
            language: "rust".to_owned(),
            event: crate::lsp::LspClientEvent::Message(serde_json::json!({
                "jsonrpc":"2.0","id":completion_id,"result":[{"label":"stale"}]
            })),
        });
        assert!(
            !matches!(state.overlay, Some(Overlay::LspCompletion)),
            "a stale completion response must not open the completion overlay"
        );
        assert!(state.lsp_completions.is_empty());
    }

    #[test]
    fn syntax_language_resolves_via_settings_and_falls_back_to_extension() {
        let mut state = state();

        // "rust" is in `settings.languages` (used for LSP), so this must go
        // through `SyntaxLanguage::from_language_name`, not `from_extension`.
        let rust_path = state.workspace.as_path().join("main.rs");
        let mut rust_buffer = TextBuffer::empty(Some(rust_path.clone()), false);
        rust_buffer.insert("fn main() {}").unwrap();
        let effects = state.update(AppEvent::FileOpened {
            path: rust_path,
            line: None,
            column: None,
            result: Ok(rust_buffer),
        });
        assert!(effects.iter().any(|effect| matches!(
            effect,
            Effect::HighlightSyntax { language, .. } if *language == editor::SyntaxLanguage::Rust
        )));
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::StartLsp { .. })),
            "syntax highlighting must still run while code analysis is disabled"
        );

        // TOML has no default entry in `settings.languages` (no LSP server
        // configured for it), so this must fall back to `from_extension`.
        let toml_path = state.workspace.as_path().join("Cargo.toml");
        let mut toml_buffer = TextBuffer::empty(Some(toml_path.clone()), false);
        toml_buffer.insert("[package]\n").unwrap();
        let effects = state.update(AppEvent::FileOpened {
            path: toml_path,
            line: None,
            column: None,
            result: Ok(toml_buffer),
        });
        assert!(effects.iter().any(|effect| matches!(
            effect,
            Effect::HighlightSyntax { language, .. } if *language == editor::SyntaxLanguage::Toml
        )));
    }

    #[test]
    fn large_file_mode_skips_syntax_and_lsp_and_explains_degradation() {
        let mut state = state();
        state.settings.editor.large_file_threshold_mb = 0;
        let path = state.workspace.as_path().join("large.rs");
        let mut buffer = TextBuffer::empty(Some(path.clone()), false);
        buffer.insert("fn main() {}").unwrap();
        let effects = state.update(AppEvent::FileOpened {
            path,
            line: None,
            column: None,
            result: Ok(buffer),
        });
        assert!(!effects.iter().any(|effect| matches!(
            effect,
            Effect::HighlightSyntax { .. } | Effect::StartLsp { .. }
        )));
        assert!(
            state
                .notification
                .as_deref()
                .is_some_and(|message| message.contains("Large-file mode"))
        );
    }

    #[test]
    fn rust_file_lazily_starts_lsp_and_sends_full_document_changes() {
        let mut state = state();
        state.settings.lsp.enabled = true;
        let path = state.workspace.as_path().join("main.rs");
        let mut buffer = TextBuffer::empty(Some(path.clone()), false);
        buffer.insert("fn main() {}").unwrap();
        let effects = state.update(AppEvent::FileOpened {
            path: path.clone(),
            line: None,
            column: None,
            result: Ok(buffer),
        });
        assert!(effects.iter().any(|effect| matches!(
            effect,
            Effect::StartLsp { language, .. } if language == "rust"
        )));

        let effects = state.update(AppEvent::LspClient {
            language: "rust".to_owned(),
            event: crate::lsp::LspClientEvent::Initialized,
        });
        assert!(effects.iter().any(|effect| matches!(
            effect,
            Effect::SendLsp { message, .. }
                if message.get("method").and_then(serde_json::Value::as_str)
                    == Some("textDocument/didOpen")
        )));
        let effects = state.update(AppEvent::Command(Command::InsertText("x".to_owned())));
        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, Effect::SendLspChange { version: 2, .. }))
        );
    }

    #[test]
    fn lsp_log_and_progress_notifications_update_output_and_status_state() {
        let mut state = state();
        state.update(AppEvent::LspClient {
            language: "rust".to_owned(),
            event: crate::lsp::LspClientEvent::Message(serde_json::json!({
                "method": "window/logMessage",
                "params": {"type": 3, "message": "indexed crate"}
            })),
        });
        assert!(
            state
                .output_lines
                .iter()
                .any(|line| line.contains("indexed crate"))
        );

        state.update(AppEvent::LspClient {
            language: "rust".to_owned(),
            event: crate::lsp::LspClientEvent::Message(serde_json::json!({
                "method": "$/progress",
                "params": {"token": "index", "value": {
                    "kind": "report", "message": "workspace", "percentage": 60
                }}
            })),
        });
        assert_eq!(
            state.lsp_progress.get("rust").map(String::as_str),
            Some("working workspace 60%")
        );
        state.update(AppEvent::LspClient {
            language: "rust".to_owned(),
            event: crate::lsp::LspClientEvent::Message(serde_json::json!({
                "method": "$/progress",
                "params": {"token": "index", "value": {"kind": "end"}}
            })),
        });
        assert!(!state.lsp_progress.contains_key("rust"));
    }

    #[test]
    fn references_request_opens_navigable_multi_location_results() {
        let mut state = state();
        let path = state.workspace.as_path().join("main.rs");
        let mut buffer = TextBuffer::empty(Some(path.clone()), false);
        buffer.insert("fn target() {}\n").unwrap();
        state.tabs.push(BufferTab::new(buffer));
        state.active_tab = Some(0);
        state.lsp_started.insert("rust".to_owned());
        let effects = state.update(AppEvent::Command(Command::Invoke(
            command::LSP_REFERENCES.to_owned(),
        )));
        let (id, message) = effects
            .iter()
            .find_map(|effect| match effect {
                Effect::SendLsp { message, .. } => Some((message["id"].as_u64()?, message)),
                _ => None,
            })
            .expect("references request");
        assert_eq!(message["params"]["context"]["includeDeclaration"], true);
        state.update(AppEvent::LspClient {
            language: "rust".to_owned(),
            event: crate::lsp::LspClientEvent::Message(serde_json::json!({
                "id": id,
                "result": [
                    {"uri": file_uri(&path), "range": {"start": {"line": 0, "character": 3}}},
                    {"uri": file_uri(&path), "range": {"start": {"line": 1, "character": 1}}}
                ]
            })),
        });
        assert!(matches!(state.overlay, Some(Overlay::LspLocations)));
        assert_eq!(state.lsp_locations.len(), 2);
        state.update(AppEvent::Command(Command::LspLocationSelect(1)));
        let effects = state.update(AppEvent::Command(Command::LspLocationOpen));
        assert!(matches!(
            effects.as_slice(),
            [Effect::OpenFile { line: Some(2), .. }]
        ));
    }

    #[test]
    fn formatting_applies_utf16_text_edits_as_one_undoable_change() {
        let mut state = state();
        let path = state.workspace.as_path().join("main.rs");
        let mut buffer = TextBuffer::empty(Some(path), false);
        buffer.insert("fn  x(){}").unwrap();
        state.tabs.push(BufferTab::new(buffer));
        state.active_tab = Some(0);
        state.lsp_started.insert("rust".to_owned());
        let effects = state.update(AppEvent::Command(Command::Invoke(
            command::LSP_FORMAT.to_owned(),
        )));
        let message = effects
            .iter()
            .find_map(|effect| match effect {
                Effect::SendLsp { message, .. } => Some(message),
                _ => None,
            })
            .expect("format request");
        assert!(message["params"].get("position").is_none());
        assert_eq!(message["params"]["options"]["tabSize"], 4);
        let id = message["id"].as_u64().unwrap();
        state.update(AppEvent::LspClient {
            language: "rust".to_owned(),
            event: crate::lsp::LspClientEvent::Message(serde_json::json!({
                "id": id,
                "result": [{
                    "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 9}},
                    "newText": "fn x() {}"
                }]
            })),
        });
        assert_eq!(state.tabs[0].buffer.text_string(), "fn x() {}");
        state.tabs[0].buffer.undo().unwrap();
        assert_eq!(state.tabs[0].buffer.text_string(), "fn  x(){}");
    }

    #[test]
    fn format_on_save_waits_for_formatter_then_dispatches_save() {
        let mut state = state();
        state.settings.editor.format_on_save = true;
        let path = state.workspace.as_path().join("save.rs");
        let mut buffer = TextBuffer::empty(Some(path), false);
        buffer.insert("fn  x(){}").unwrap();
        state.tabs.push(BufferTab::new(buffer));
        state.active_tab = Some(0);
        state.lsp_started.insert("rust".to_owned());
        let effects = state.update(AppEvent::Command(Command::Invoke(
            command::EDITOR_SAVE.to_owned(),
        )));
        let id = effects
            .iter()
            .find_map(|effect| match effect {
                Effect::SendLsp { message, .. } => message["id"].as_u64(),
                _ => None,
            })
            .expect("format request before save");
        let effects = state.update(AppEvent::LspClient {
            language: "rust".to_owned(),
            event: crate::lsp::LspClientEvent::Message(serde_json::json!({
                "id": id,
                "result": [{
                    "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 9}},
                    "newText": "fn x() {}"
                }]
            })),
        });
        assert!(effects.iter().any(|effect| matches!(
            effect,
            Effect::Save { snapshot, .. } if snapshot.bytes == b"fn x() {}"
        )));
    }

    #[test]
    fn code_action_lists_and_applies_workspace_edit_to_open_buffer() {
        let mut state = state();
        let path = state.workspace.as_path().join("main.rs");
        let mut buffer = TextBuffer::empty(Some(path.clone()), false);
        buffer.insert("let value = 1").unwrap();
        state.tabs.push(BufferTab::new(buffer));
        state.active_tab = Some(0);
        state.lsp_started.insert("rust".to_owned());
        let effects = state.update(AppEvent::Command(Command::Invoke(
            command::LSP_CODE_ACTION.to_owned(),
        )));
        let message = effects
            .iter()
            .find_map(|effect| match effect {
                Effect::SendLsp { message, .. } => Some(message),
                _ => None,
            })
            .expect("code action request");
        assert_eq!(
            message["params"]["context"]["diagnostics"],
            serde_json::json!([])
        );
        let id = message["id"].as_u64().unwrap();
        state.update(AppEvent::LspClient {
            language: "rust".to_owned(),
            event: crate::lsp::LspClientEvent::Message(serde_json::json!({
                "id": id,
                "result": [{
                    "title": "Add semicolon",
                    "edit": {"changes": {(file_uri(&path)): [{
                        "range": {"start": {"line": 0, "character": 13}, "end": {"line": 0, "character": 13}},
                        "newText": ";"
                    }]}}
                }]
            })),
        });
        assert!(matches!(state.overlay, Some(Overlay::LspCodeActions)));
        state.update(AppEvent::Command(Command::LspCodeActionApply));
        assert_eq!(state.tabs[0].buffer.text_string(), "let value = 1;");
        state.tabs[0].buffer.undo().unwrap();
        assert_eq!(state.tabs[0].buffer.text_string(), "let value = 1");
    }

    #[test]
    fn lsp_diagnostics_convert_utf16_and_hover_response_opens_overlay() {
        let mut state = state();
        state.settings.diagnostics.enabled = true;
        let path = state.workspace.as_path().join("unicode.rs");
        let mut buffer = TextBuffer::empty(Some(path.clone()), false);
        buffer.insert("a😀value").unwrap();
        state.tabs.push(BufferTab::new(buffer));
        state.active_tab = Some(0);
        state.lsp_started.insert("rust".to_owned());
        state.update(AppEvent::LspClient {
            language: "rust".to_owned(),
            event: crate::lsp::LspClientEvent::Message(serde_json::json!({
                "jsonrpc":"2.0","method":"textDocument/publishDiagnostics",
                "params":{"uri":file_uri(&path),"diagnostics":[{
                    "range":{"start":{"line":0,"character":3},"end":{"line":0,"character":8}},
                    "severity":1,"message":"bad value","code":"E1"
                }]}
            })),
        });
        assert_eq!(
            state.diagnostics.diagnostics()[0].range.start.char_offset,
            Some(2)
        );

        let effects = state.update(AppEvent::Command(Command::Invoke(
            command::LSP_HOVER.to_owned(),
        )));
        let id = effects
            .iter()
            .find_map(|effect| match effect {
                Effect::SendLsp { message, .. } => message.get("id")?.as_u64(),
                _ => None,
            })
            .unwrap();
        state.update(AppEvent::LspClient {
            language: "rust".to_owned(),
            event: crate::lsp::LspClientEvent::Message(serde_json::json!({
                "jsonrpc":"2.0","id":id,"result":{"contents":{"kind":"markdown","value":"**type**"}}
            })),
        });
        assert!(matches!(state.overlay, Some(Overlay::LspHover)));
        assert_eq!(state.lsp_hover, vec!["**type**"]);
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
