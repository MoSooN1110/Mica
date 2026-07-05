use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};

use ropey::Rope;
use thiserror::Error;
use unicode_segmentation::UnicodeSegmentation;

use super::{
    CharOffset, Edit, Selection, atomic_save_if_unchanged,
    history::History,
    persistence::{SaveError, hash_bytes},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
    Mixed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskState {
    Present,
    ModifiedExternally,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalChangeOutcome {
    Unchanged,
    Reloaded,
    Conflict,
    Deleted,
}

#[derive(Debug, Error)]
pub enum BufferError {
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("buffer has no file path")]
    NoPath,
    #[error("buffer is read-only")]
    ReadOnly,
    #[error(transparent)]
    Save(#[from] SaveError),
}

#[derive(Debug, Clone)]
pub struct SaveSnapshot {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    pub generation: u64,
    pub content_hash: u64,
    pub disk_hash: u64,
    pub expected_disk_hash: Option<u64>,
}

/// Text and edit history shared by every view of one file.
#[derive(Debug)]
pub struct TextBuffer {
    path: Option<PathBuf>,
    text: Rope,
    selections: Vec<Selection>,
    history: History,
    generation: u64,
    saved_hash: u64,
    disk_hash: Option<u64>,
    has_bom: bool,
    line_ending: LineEnding,
    read_only: bool,
    invalid_utf8: bool,
    disk_state: DiskState,
}

impl TextBuffer {
    pub fn empty(path: Option<PathBuf>, read_only: bool) -> Self {
        let text = Rope::new();
        let hash = hash_text(&text);
        Self {
            path,
            text,
            selections: vec![Selection::caret(CharOffset(0))],
            history: History::default(),
            generation: 0,
            saved_hash: hash,
            disk_hash: None,
            has_bom: false,
            line_ending: LineEnding::Lf,
            read_only,
            invalid_utf8: false,
            disk_state: DiskState::Present,
        }
    }

    pub fn recovered(
        path: Option<PathBuf>,
        content: &str,
        expected_disk_hash: Option<u64>,
        has_bom: bool,
        line_ending: LineEnding,
        read_only: bool,
    ) -> Self {
        let text = Rope::from_str(content);
        let content_hash = hash_text(&text);
        Self {
            path,
            text,
            selections: vec![Selection::caret(CharOffset(0))],
            history: History::default(),
            generation: 1,
            saved_hash: content_hash ^ u64::MAX,
            disk_hash: expected_disk_hash,
            has_bom,
            line_ending,
            read_only,
            invalid_utf8: false,
            disk_state: DiskState::Present,
        }
    }

    pub fn open(path: &Path, force_read_only: bool) -> Result<Self, BufferError> {
        let bytes = fs::read(path).map_err(|source| BufferError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let disk_hash = hash_bytes(&bytes);
        let (has_bom, content) = bytes
            .strip_prefix(&[0xEF, 0xBB, 0xBF])
            .map_or((false, bytes.as_slice()), |rest| (true, rest));
        let (decoded, invalid_utf8) = match std::str::from_utf8(content) {
            Ok(text) => (text.to_owned(), false),
            Err(_) => (String::from_utf8_lossy(content).into_owned(), true),
        };
        let line_ending = detect_line_ending(&decoded);
        let text = Rope::from_str(&decoded);
        let saved_hash = hash_text(&text);
        let metadata_read_only = fs::metadata(path)
            .map(|metadata| metadata.permissions().readonly())
            .unwrap_or(true);
        Ok(Self {
            path: Some(path.to_path_buf()),
            text,
            selections: vec![Selection::caret(CharOffset(0))],
            history: History::default(),
            generation: 0,
            saved_hash,
            disk_hash: Some(disk_hash),
            has_bom,
            line_ending,
            read_only: force_read_only || metadata_read_only || invalid_utf8,
            invalid_utf8,
            disk_state: DiskState::Present,
        })
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn text(&self) -> &Rope {
        &self.text
    }

    pub fn text_string(&self) -> String {
        self.text.to_string()
    }

    pub fn selection(&self) -> Selection {
        self.selections[0]
    }

    pub fn selected_text(&self) -> Option<String> {
        let range = self.selection().range();
        (range.start != range.end).then(|| self.text.slice(range).to_string())
    }

    pub fn delete_selection(&mut self) -> Result<bool, BufferError> {
        if self.selection().is_caret() {
            return Ok(false);
        }
        self.insert("")?;
        Ok(true)
    }

    pub fn set_selection(&mut self, selection: Selection) {
        let max = self.text.len_chars();
        self.selections[0] = Selection {
            anchor: CharOffset(selection.anchor.0.min(max)),
            head: CharOffset(selection.head.0.min(max)),
        };
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn is_dirty(&self) -> bool {
        hash_text(&self.text) != self.saved_hash
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    pub fn has_invalid_utf8(&self) -> bool {
        self.invalid_utf8
    }

    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    pub fn has_bom(&self) -> bool {
        self.has_bom
    }

    pub fn disk_state(&self) -> DiskState {
        self.disk_state
    }

    pub fn disk_hash(&self) -> Option<u64> {
        self.disk_hash
    }

    pub fn set_path(&mut self, path: PathBuf) {
        self.path = Some(path);
    }

    /// Reconciles a background disk read with the in-memory buffer. Dirty text
    /// is never replaced. Deleted files remain open and can be saved again.
    pub fn reconcile_external(
        &mut self,
        fresh: Option<TextBuffer>,
        auto_reload_unmodified: bool,
    ) -> ExternalChangeOutcome {
        let Some(mut fresh) = fresh else {
            self.disk_state = DiskState::Deleted;
            return ExternalChangeOutcome::Deleted;
        };
        if fresh.disk_hash == self.disk_hash {
            self.disk_state = DiskState::Present;
            return ExternalChangeOutcome::Unchanged;
        }
        if self.is_dirty() || !auto_reload_unmodified {
            self.disk_state = DiskState::ModifiedExternally;
            return ExternalChangeOutcome::Conflict;
        }

        let selections = self.selections.clone();
        let next_generation = self.generation.saturating_add(1);
        fresh.selections = selections;
        for selection in &mut fresh.selections {
            let max = fresh.text.len_chars();
            selection.anchor.0 = selection.anchor.0.min(max);
            selection.head.0 = selection.head.0.min(max);
        }
        fresh.generation = next_generation;
        *self = fresh;
        ExternalChangeOutcome::Reloaded
    }

    pub fn insert(&mut self, value: &str) -> Result<(), BufferError> {
        self.ensure_writable()?;
        let selection = self.selection();
        let range = selection.range();
        let deleted = self.text.slice(range.clone()).to_string();
        let edit = Edit {
            start_char: range.start,
            deleted,
            inserted: value.to_owned(),
        };
        self.apply_forward(&edit);
        let head = edit.start_char + value.chars().count();
        self.selections[0] = Selection::caret(CharOffset(head));
        self.history.record(edit);
        Ok(())
    }

    pub fn delete_backward(&mut self) -> Result<(), BufferError> {
        self.ensure_writable()?;
        let selection = self.selection();
        if !selection.is_caret() {
            return self.insert("");
        }
        if selection.head.0 == 0 {
            return Ok(());
        }

        let before = self.text.slice(..selection.head.0).to_string();
        let deleted = before.graphemes(true).next_back().unwrap_or_default();
        let deleted_chars = deleted.chars().count();
        let edit = Edit {
            start_char: selection.head.0 - deleted_chars,
            deleted: deleted.to_owned(),
            inserted: String::new(),
        };
        self.apply_forward(&edit);
        self.selections[0] = Selection::caret(CharOffset(edit.start_char));
        self.history.record(edit);
        Ok(())
    }

    pub fn undo(&mut self) -> Result<bool, BufferError> {
        self.ensure_writable()?;
        let Some(edit) = self.history.pop_undo() else {
            return Ok(false);
        };
        self.apply_reverse(&edit);
        self.selections[0] =
            Selection::caret(CharOffset(edit.start_char + edit.deleted.chars().count()));
        self.history.push_redo(edit);
        Ok(true)
    }

    pub fn redo(&mut self) -> Result<bool, BufferError> {
        self.ensure_writable()?;
        let Some(edit) = self.history.pop_redo() else {
            return Ok(false);
        };
        self.apply_forward(&edit);
        self.selections[0] =
            Selection::caret(CharOffset(edit.start_char + edit.inserted.chars().count()));
        self.history.push_undo(edit);
        Ok(true)
    }

    pub fn save(&mut self) -> Result<(), BufferError> {
        let snapshot = self.prepare_save()?;
        atomic_save_if_unchanged(&snapshot.path, &snapshot.bytes, snapshot.expected_disk_hash)?;
        self.complete_save(&snapshot);
        Ok(())
    }

    pub fn prepare_save(&self) -> Result<SaveSnapshot, BufferError> {
        self.ensure_writable()?;
        let path = self.path.clone().ok_or(BufferError::NoPath)?;
        let mut bytes = Vec::with_capacity(self.text.len_bytes() + usize::from(self.has_bom) * 3);
        if self.has_bom {
            bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        }
        for chunk in self.text.chunks() {
            bytes.extend_from_slice(chunk.as_bytes());
        }
        Ok(SaveSnapshot {
            path,
            content_hash: hash_text(&self.text),
            disk_hash: hash_bytes(&bytes),
            expected_disk_hash: self.disk_hash,
            generation: self.generation,
            bytes,
        })
    }

    pub fn complete_save(&mut self, snapshot: &SaveSnapshot) {
        self.saved_hash = snapshot.content_hash;
        self.disk_hash = Some(snapshot.disk_hash);
        self.disk_state = DiskState::Present;
    }

    /// Re-checks disk content without ever replacing dirty text.
    pub fn refresh_disk_state(&mut self) -> Result<DiskState, BufferError> {
        let Some(path) = self.path.as_deref() else {
            return Ok(self.disk_state);
        };
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.disk_state = DiskState::Deleted;
                return Ok(self.disk_state);
            }
            Err(source) => {
                return Err(BufferError::Read {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        self.disk_state = if Some(hash_bytes(&bytes)) == self.disk_hash {
            DiskState::Present
        } else {
            DiskState::ModifiedExternally
        };
        Ok(self.disk_state)
    }

    fn ensure_writable(&self) -> Result<(), BufferError> {
        if self.read_only {
            Err(BufferError::ReadOnly)
        } else {
            Ok(())
        }
    }

    fn apply_forward(&mut self, edit: &Edit) {
        let old_end = edit.start_char + edit.deleted.chars().count();
        self.text.remove(edit.start_char..old_end);
        self.text.insert(edit.start_char, &edit.inserted);
        self.generation = self.generation.saturating_add(1);
    }

    fn apply_reverse(&mut self, edit: &Edit) {
        let new_end = edit.start_char + edit.inserted.chars().count();
        self.text.remove(edit.start_char..new_end);
        self.text.insert(edit.start_char, &edit.deleted);
        self.generation = self.generation.saturating_add(1);
    }
}

fn detect_line_ending(text: &str) -> LineEnding {
    let crlf = text.match_indices("\r\n").count();
    let all_lf = text.bytes().filter(|byte| *byte == b'\n').count();
    match (crlf, all_lf.saturating_sub(crlf)) {
        (0, _) => LineEnding::Lf,
        (_, 0) => LineEnding::CrLf,
        _ => LineEnding::Mixed,
    }
}

fn hash_text(text: &Rope) -> u64 {
    let mut hasher = DefaultHasher::new();
    for chunk in text.chunks() {
        chunk.hash(&mut hasher);
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deleting_moves_by_grapheme_cluster() {
        let mut buffer = TextBuffer::empty(None, false);
        buffer.insert("A👩‍💻e\u{301}日").unwrap();
        buffer.delete_backward().unwrap();
        assert_eq!(buffer.text_string(), "A👩‍💻e\u{301}");
        buffer.delete_backward().unwrap();
        assert_eq!(buffer.text_string(), "A👩‍💻");
        buffer.delete_backward().unwrap();
        assert_eq!(buffer.text_string(), "A");
    }

    #[test]
    fn undo_redo_and_dirty_state_track_saved_content() {
        let mut buffer = TextBuffer::empty(None, false);
        buffer.insert("日本語").unwrap();
        assert!(buffer.is_dirty());
        assert!(buffer.undo().unwrap());
        assert!(!buffer.is_dirty());
        assert!(buffer.redo().unwrap());
        assert_eq!(buffer.text_string(), "日本語");
    }

    #[test]
    fn selection_replacement_is_one_edit() {
        let mut buffer = TextBuffer::empty(None, false);
        buffer.insert("abcdef").unwrap();
        buffer.set_selection(Selection {
            anchor: CharOffset(2),
            head: CharOffset(5),
        });
        buffer.insert("X").unwrap();
        assert_eq!(buffer.text_string(), "abXf");
        buffer.undo().unwrap();
        assert_eq!(buffer.text_string(), "abcdef");
    }

    #[test]
    fn selected_text_and_cut_preserve_unicode_boundaries() {
        let mut buffer = TextBuffer::empty(None, false);
        buffer.insert("A日本👩‍💻Z").unwrap();
        buffer.set_selection(Selection {
            anchor: CharOffset(1),
            head: CharOffset(3),
        });
        assert_eq!(buffer.selected_text().as_deref(), Some("日本"));
        assert!(buffer.delete_selection().unwrap());
        assert_eq!(buffer.text_string(), "A👩‍💻Z");
    }

    #[test]
    fn save_preserves_bom_crlf_and_trailing_newline() {
        let path = std::env::temp_dir().join(format!("mica-format-{}", std::process::id()));
        let _ = fs::remove_file(&path);
        fs::write(&path, b"\xEF\xBB\xBFone\r\ntwo\r\n").unwrap();
        let mut buffer = TextBuffer::open(&path, false).unwrap();
        assert!(buffer.has_bom());
        assert_eq!(buffer.line_ending(), LineEnding::CrLf);
        buffer.set_selection(Selection::caret(CharOffset(buffer.text().len_chars())));
        buffer.insert("three").unwrap();
        buffer.save().unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"\xEF\xBB\xBFone\r\ntwo\r\nthree");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn external_change_never_replaces_dirty_text() {
        let path = std::env::temp_dir().join(format!("mica-conflict-{}", std::process::id()));
        let _ = fs::remove_file(&path);
        fs::write(&path, b"base").unwrap();
        let mut local = TextBuffer::open(&path, false).unwrap();
        local.set_selection(Selection::caret(CharOffset(local.text().len_chars())));
        local.insert(" local").unwrap();
        fs::write(&path, b"external").unwrap();
        let fresh = TextBuffer::open(&path, false).unwrap();
        assert_eq!(
            local.reconcile_external(Some(fresh), true),
            ExternalChangeOutcome::Conflict
        );
        assert_eq!(local.text_string(), "base local");
        assert_eq!(local.disk_state(), DiskState::ModifiedExternally);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn clean_buffer_auto_reloads_and_preserves_cursor() {
        let path = std::env::temp_dir().join(format!("mica-reload-{}", std::process::id()));
        let _ = fs::remove_file(&path);
        fs::write(&path, b"old").unwrap();
        let mut local = TextBuffer::open(&path, false).unwrap();
        local.set_selection(Selection::caret(CharOffset(2)));
        fs::write(&path, b"new content").unwrap();
        let fresh = TextBuffer::open(&path, false).unwrap();
        assert_eq!(
            local.reconcile_external(Some(fresh), true),
            ExternalChangeOutcome::Reloaded
        );
        assert_eq!(local.text_string(), "new content");
        assert_eq!(local.selection().head, CharOffset(2));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn external_deletion_keeps_buffer_content() {
        let mut buffer = TextBuffer::empty(Some(PathBuf::from("missing")), false);
        buffer.insert("keep me").unwrap();
        assert_eq!(
            buffer.reconcile_external(None, true),
            ExternalChangeOutcome::Deleted
        );
        assert_eq!(buffer.text_string(), "keep me");
        assert_eq!(buffer.disk_state(), DiskState::Deleted);
    }
}
