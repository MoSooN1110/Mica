use std::{path::PathBuf, sync::atomic::Ordering};

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
                        self.notification = None;
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
                        self.focus = Focus::Editor;
                    }
                    Err(error) => {
                        self.append_output("git", &error);
                        self.notification = Some(error);
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
                        self.notification = Some(error);
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
                self.diagnostics
                    .replace_source(source, generation, diagnostics);
                self.diagnostic_selected = self
                    .diagnostic_selected
                    .min(self.visible_diagnostics().len().saturating_sub(1));
                Vec::new()
            }
            AppEvent::DiagnosticsFailed {
                source,
                generation,
                error,
            } => {
                self.diagnostics.clear_source(source, generation);
                self.append_output("diagnostics", &error);
                self.notification = Some(error);
                Vec::new()
            }
            AppEvent::OutputMessage { source, message } => {
                self.append_output(&source, message);
                Vec::new()
            }
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
                self.edit(|tab| tab.buffer.insert(&text));
                self.active_post_edit_effects()
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
                self.active_post_edit_effects()
            }
            Command::DeleteBackward => {
                self.edit(|tab| tab.buffer.delete_backward());
                self.active_post_edit_effects()
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
            Command::SelectGit(index) => {
                self.git_selected = index.min(self.git_entries().len().saturating_sub(1));
                Vec::new()
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
                Vec::new()
            }
            Command::GitHunkNext => {
                let count = self.git_diff.as_ref().map_or(0, |diff| diff.hunks.len());
                self.git_hunk_selected = (self.git_hunk_selected + 1).min(count.saturating_sub(1));
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
            Command::DiagnosticSelect(index) => {
                self.diagnostic_selected =
                    index.min(self.visible_diagnostics().len().saturating_sub(1));
                Vec::new()
            }
            Command::DiagnosticOpen => {
                let Some(diagnostic) = self
                    .visible_diagnostics()
                    .get(self.diagnostic_selected)
                    .cloned()
                    .cloned()
                else {
                    return Vec::new();
                };
                self.focus = Focus::Editor;
                vec![Effect::OpenFile {
                    path: diagnostic.file,
                    read_only: self.force_read_only,
                    line: Some(diagnostic.range.start.line + 1),
                    // Note: `range.start.column` here is itself UTF-16-derived
                    // (see `lsp_diagnostic`'s `position` closure), so this
                    // inherits the same latent non-BMP-column bug that Fix 1
                    // addresses for definition jumps. Left as `Chars` to keep
                    // this change scoped to `PendingLspRequest::Definition`.
                    column: Some(ColumnHint::Chars(diagnostic.range.start.column + 1)),
                }]
            }
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
                self.palette_query.pop();
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
                    Some(Overlay::LspHover) => {
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
                self.palette_query.clear();
                self.tabs[tab].view.search = Default::default();
                self.focus = Focus::Overlay;
                Vec::new()
            }
            command::EDITOR_UNDO => {
                self.edit(|tab| tab.buffer.undo().map(|_| ()));
                self.active_post_edit_effects()
            }
            command::EDITOR_REDO => {
                self.edit(|tab| tab.buffer.redo().map(|_| ()));
                self.active_post_edit_effects()
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
            command::DIAGNOSTICS_REFRESH => {
                if !self.workspace.as_path().join("Cargo.toml").is_file() {
                    self.notification =
                        Some("Cargo diagnostics require Cargo.toml in the workspace".to_owned());
                    return Vec::new();
                }
                self.compiler_diagnostic_generation =
                    self.compiler_diagnostic_generation.saturating_add(1);
                self.notification = Some("Running cargo clippy…".to_owned());
                vec![Effect::RunCargoDiagnostics {
                    generation: self.compiler_diagnostic_generation,
                }]
            }
            command::LSP_HOVER => {
                self.lsp_request_effect("textDocument/hover", crate::app::PendingLspRequest::Hover)
            }
            command::LSP_DEFINITION => self.lsp_request_effect(
                "textDocument/definition",
                crate::app::PendingLspRequest::Definition,
            ),
            command::LSP_COMPLETION => self.lsp_request_effect(
                "textDocument/completion",
                crate::app::PendingLspRequest::Completion,
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
        self.edit(|tab| {
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
            }
            tab.buffer.insert(&insert_text)
        });
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

    fn lsp_open_effect(&mut self, path: &std::path::Path) -> Vec<Effect> {
        if !self.settings.lsp.enabled {
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

    fn handle_lsp_message(&mut self, _language: &str, message: &serde_json::Value) -> Vec<Effect> {
        if let Some(id) = message.get("id").and_then(serde_json::Value::as_u64)
            && let Some(request) = self.lsp_pending.remove(&id)
        {
            return self.handle_lsp_response(request, message.get("result"));
        }
        if message.get("method").and_then(serde_json::Value::as_str)
            != Some("textDocument/publishDiagnostics")
        {
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
        vec![Effect::SendLsp {
            language,
            message: serde_json::json!({
                "jsonrpc":"2.0",
                "id":id,
                "method":method,
                "params":{"textDocument":{"uri":file_uri(&path)},"position":position}
            }),
        }]
    }

    fn handle_lsp_response(
        &mut self,
        request: crate::app::PendingLspRequest,
        result: Option<&serde_json::Value>,
    ) -> Vec<Effect> {
        let Some(result) = result.filter(|result| !result.is_null()) else {
            self.notification = Some("LSP returned no result".to_owned());
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
                self.lsp_completions = items
                    .into_iter()
                    .flatten()
                    .filter_map(completion_candidate)
                    .take(200)
                    .collect();
                self.lsp_completion_selected = 0;
                if self.lsp_completions.is_empty() {
                    self.notification = Some("No completions".to_owned());
                } else {
                    self.overlay = Some(Overlay::LspCompletion);
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

    fn request_close_tab(&mut self, index: usize) -> Vec<Effect> {
        if index >= self.tabs.len() {
            return Vec::new();
        }
        if !self.saving_tabs.is_empty() {
            self.notification = Some("Wait for pending saves before closing a tab".to_owned());
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
        self.active_tab = match self.active_tab {
            None => None,
            Some(_) if self.tabs.is_empty() => None,
            Some(active) if active > index => Some(active - 1),
            Some(active) if active == index => Some(index.min(self.tabs.len() - 1)),
            Some(active) => Some(active),
        };
        lsp_close
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

fn completion_candidate(value: &serde_json::Value) -> Option<crate::app::CompletionCandidate> {
    let label = value.get("label")?.as_str()?.to_owned();
    let is_snippet = value
        .get("insertTextFormat")
        .and_then(serde_json::Value::as_u64)
        == Some(2);
    let insert_text = if is_snippet {
        label.clone()
    } else {
        value
            .get("textEdit")
            .and_then(|edit| edit.get("newText"))
            .and_then(serde_json::Value::as_str)
            .or_else(|| value.get("insertText").and_then(serde_json::Value::as_str))
            .unwrap_or(&label)
            .to_owned()
    };
    Some(crate::app::CompletionCandidate {
        label,
        insert_text,
        detail: value
            .get("detail")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        replace_range: completion_replace_range(value),
    })
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
    fn rust_file_lazily_starts_lsp_and_sends_full_document_changes() {
        let mut state = state();
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
    fn lsp_diagnostics_convert_utf16_and_hover_response_opens_overlay() {
        let mut state = state();
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
