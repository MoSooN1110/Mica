use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use super::{Diagnostic, DiagnosticSeverity, DiagnosticSource, TextRange};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DiagnosticKey {
    file: PathBuf,
    range: TextRange,
    severity: DiagnosticSeverity,
    message: String,
}

#[derive(Debug, Default)]
pub struct DiagnosticStore {
    generations: HashMap<DiagnosticSource, u64>,
    by_source: HashMap<DiagnosticSource, Vec<Diagnostic>>,
    merged: Vec<Diagnostic>,
}

impl DiagnosticStore {
    pub fn replace_source(
        &mut self,
        source: DiagnosticSource,
        generation: u64,
        mut diagnostics: Vec<Diagnostic>,
    ) -> bool {
        if self
            .generations
            .get(&source)
            .is_some_and(|current| generation < *current)
        {
            return false;
        }
        for diagnostic in &mut diagnostics {
            diagnostic.source = source;
            diagnostic.file = normalize_path(&diagnostic.file);
            diagnostic.message = normalize_display_message(&diagnostic.message);
        }
        self.generations.insert(source, generation);
        self.by_source.insert(source, diagnostics);
        self.rebuild();
        true
    }

    pub fn clear_source(&mut self, source: DiagnosticSource, generation: u64) -> bool {
        self.replace_source(source, generation, Vec::new())
    }

    pub fn mark_file_stale(&mut self, file: &Path) {
        let file = normalize_path(file);
        for diagnostics in self.by_source.values_mut() {
            for diagnostic in diagnostics {
                if diagnostic.file == file {
                    diagnostic.stale = true;
                }
            }
        }
        self.rebuild();
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.merged
    }

    pub fn for_file(&self, file: &Path) -> impl Iterator<Item = &Diagnostic> {
        let file = normalize_path(file);
        self.merged.iter().filter(move |item| item.file == file)
    }

    pub fn counts(&self) -> (usize, usize) {
        self.merged
            .iter()
            .fold((0, 0), |(errors, warnings), item| match item.severity {
                DiagnosticSeverity::Error => (errors + 1, warnings),
                DiagnosticSeverity::Warning => (errors, warnings + 1),
                _ => (errors, warnings),
            })
    }

    fn rebuild(&mut self) {
        let mut sources = self.by_source.keys().copied().collect::<Vec<_>>();
        sources.sort_by_key(|source| source.priority());
        let mut seen = HashSet::new();
        let mut merged = Vec::new();
        for source in sources {
            let Some(diagnostics) = self.by_source.get(&source) else {
                continue;
            };
            for diagnostic in diagnostics {
                let key = DiagnosticKey {
                    file: diagnostic.file.clone(),
                    range: diagnostic.range,
                    severity: diagnostic.severity,
                    message: normalize_key_message(&diagnostic.message),
                };
                if seen.insert(key) {
                    merged.push(diagnostic.clone());
                }
            }
        }
        merged.sort_by(|left, right| {
            left.file
                .cmp(&right.file)
                .then(left.range.start.cmp(&right.range.start))
                .then(left.severity.cmp(&right.severity))
        });
        self.merged = merged;
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir if normalized.file_name().is_some() => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn normalize_display_message(message: &str) -> String {
    message.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize_key_message(message: &str) -> String {
    normalize_display_message(message).to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::TextPosition;

    fn diagnostic(source: DiagnosticSource, message: &str) -> Diagnostic {
        Diagnostic {
            file: PathBuf::from("src/./main.rs"),
            range: TextRange {
                start: TextPosition {
                    line: 2,
                    column: 3,
                    char_offset: Some(5),
                },
                end: TextPosition {
                    line: 2,
                    column: 4,
                    char_offset: Some(6),
                },
            },
            severity: DiagnosticSeverity::Error,
            message: message.to_owned(),
            source,
            code: None,
            stale: false,
        }
    }

    #[test]
    fn deduplicates_by_normalized_key_with_source_priority() {
        let mut store = DiagnosticStore::default();
        store.replace_source(
            DiagnosticSource::Lsp,
            1,
            vec![diagnostic(DiagnosticSource::Lsp, "type   mismatch")],
        );
        store.replace_source(
            DiagnosticSource::Compiler,
            1,
            vec![diagnostic(DiagnosticSource::Compiler, "Type mismatch")],
        );
        assert_eq!(store.diagnostics().len(), 1);
        assert_eq!(store.diagnostics()[0].source, DiagnosticSource::Compiler);
    }

    #[test]
    fn ignores_old_generations_and_marks_files_stale() {
        let mut store = DiagnosticStore::default();
        assert!(store.replace_source(
            DiagnosticSource::Lsp,
            2,
            vec![diagnostic(DiagnosticSource::Lsp, "new")],
        ));
        assert!(!store.replace_source(
            DiagnosticSource::Lsp,
            1,
            vec![diagnostic(DiagnosticSource::Lsp, "old")],
        ));
        store.mark_file_stale(Path::new("src/main.rs"));
        assert_eq!(store.diagnostics()[0].message, "new");
        assert!(store.diagnostics()[0].stale);
    }
}
