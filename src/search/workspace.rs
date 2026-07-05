use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use regex::{Regex, RegexBuilder};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSearchOptions {
    pub query: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    pub include_globs: Vec<String>,
    pub exclude_globs: Vec<String>,
    pub show_hidden: bool,
    pub respect_gitignore: bool,
    pub include_binary: bool,
}

impl Default for WorkspaceSearchOptions {
    fn default() -> Self {
        Self {
            query: String::new(),
            case_sensitive: false,
            whole_word: false,
            regex: false,
            include_globs: Vec::new(),
            exclude_globs: Vec::new(),
            show_hidden: false,
            respect_gitignore: true,
            include_binary: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceMatch {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
    pub line_text: String,
    pub match_start: usize,
    pub match_end: usize,
}

#[derive(Debug, Error)]
pub enum WorkspaceSearchError {
    #[error("invalid search expression: {0}")]
    InvalidRegex(#[from] regex::Error),
    #[error("invalid glob `{glob}`: {source}")]
    InvalidGlob {
        glob: String,
        source: globset::Error,
    },
}

pub fn search_workspace(
    root: &Path,
    options: &WorkspaceSearchOptions,
    open_buffers: &HashMap<PathBuf, String>,
    cancellation: &AtomicU64,
    generation: u64,
    mut emit: impl FnMut(Vec<WorkspaceMatch>),
) -> Result<(), WorkspaceSearchError> {
    if options.query.is_empty() {
        return Ok(());
    }
    let matcher = build_matcher(options)?;
    let includes = build_globs(&options.include_globs)?;
    let excludes = build_globs(&options.exclude_globs)?;
    let mut searched = HashSet::new();
    let mut batch = Vec::new();

    for (path, text) in open_buffers {
        if cancelled(cancellation, generation) {
            return Ok(());
        }
        let relative = path.strip_prefix(root).unwrap_or(path);
        if path_allowed(relative, includes.as_ref(), excludes.as_ref()) {
            search_text(relative, text, &matcher, &mut batch);
            flush_if_ready(&mut batch, &mut emit);
        }
        searched.insert(path.clone());
    }

    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(!options.show_hidden)
        .git_ignore(options.respect_gitignore)
        .git_global(options.respect_gitignore)
        .git_exclude(options.respect_gitignore)
        .follow_links(false);
    for entry in builder.build().filter_map(Result::ok) {
        if cancelled(cancellation, generation) {
            return Ok(());
        }
        let path = entry.path();
        if !entry.file_type().is_some_and(|kind| kind.is_file()) || searched.contains(path) {
            continue;
        }
        let relative = path.strip_prefix(root).unwrap_or(path);
        if !path_allowed(relative, includes.as_ref(), excludes.as_ref()) {
            continue;
        }
        let Ok(bytes) = fs::read(path) else {
            continue;
        };
        if bytes.contains(&0) && !options.include_binary {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        search_text(relative, &text, &matcher, &mut batch);
        flush_if_ready(&mut batch, &mut emit);
    }
    if !batch.is_empty() && !cancelled(cancellation, generation) {
        emit(batch);
    }
    Ok(())
}

fn build_matcher(options: &WorkspaceSearchOptions) -> Result<Regex, regex::Error> {
    let mut pattern = if options.regex {
        options.query.clone()
    } else {
        regex::escape(&options.query)
    };
    if options.whole_word {
        pattern = format!(r"\b(?:{pattern})\b");
    }
    RegexBuilder::new(&pattern)
        .case_insensitive(!options.case_sensitive)
        .build()
}

fn build_globs(patterns: &[String]) -> Result<Option<GlobSet>, WorkspaceSearchError> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = Glob::new(pattern).map_err(|source| WorkspaceSearchError::InvalidGlob {
            glob: pattern.clone(),
            source,
        })?;
        builder.add(glob);
    }
    builder
        .build()
        .map(Some)
        .map_err(|source| WorkspaceSearchError::InvalidGlob {
            glob: patterns.join(","),
            source,
        })
}

fn path_allowed(path: &Path, includes: Option<&GlobSet>, excludes: Option<&GlobSet>) -> bool {
    includes.is_none_or(|set| set.is_match(path)) && excludes.is_none_or(|set| !set.is_match(path))
}

fn search_text(path: &Path, text: &str, matcher: &Regex, output: &mut Vec<WorkspaceMatch>) {
    for (line_index, line) in text.lines().enumerate() {
        for matched in matcher.find_iter(line) {
            output.push(WorkspaceMatch {
                path: path.to_path_buf(),
                line: line_index + 1,
                column: line[..matched.start()].chars().count() + 1,
                line_text: line.to_owned(),
                match_start: matched.start(),
                match_end: matched.end(),
            });
        }
    }
}

fn flush_if_ready(batch: &mut Vec<WorkspaceMatch>, emit: &mut impl FnMut(Vec<WorkspaceMatch>)) {
    if batch.len() >= 64 {
        emit(std::mem::take(batch));
    }
}

fn cancelled(cancellation: &AtomicU64, generation: u64) -> bool {
    cancellation.load(Ordering::Relaxed) != generation
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searches_unsaved_buffers_before_disk_and_honors_options() {
        let root = std::env::temp_dir().join(format!("mica-search-{}", std::process::id()));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.rs"), "disk only\n").unwrap();
        fs::write(root.join("src/b.rs"), "Hello hello_world HELLO\n").unwrap();
        fs::write(root.join("image.bin"), b"hello\0world").unwrap();
        let mut open = HashMap::new();
        open.insert(root.join("src/a.rs"), "Hello from unsaved\n".to_owned());
        let cancellation = AtomicU64::new(1);
        let options = WorkspaceSearchOptions {
            query: "hello".to_owned(),
            whole_word: true,
            include_globs: vec!["src/**".to_owned()],
            ..Default::default()
        };
        let mut results = Vec::new();
        search_workspace(&root, &options, &open, &cancellation, 1, |batch| {
            results.extend(batch)
        })
        .unwrap();
        assert_eq!(results.len(), 3);
        assert!(
            results
                .iter()
                .any(|item| item.path == Path::new("src/a.rs"))
        );
        assert!(
            !results
                .iter()
                .any(|item| item.line_text.contains("disk only"))
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn invalid_regex_is_reported_and_cancellation_stops_search() {
        let root = std::env::temp_dir();
        let cancellation = AtomicU64::new(2);
        let options = WorkspaceSearchOptions {
            query: "[".to_owned(),
            regex: true,
            ..Default::default()
        };
        assert!(
            search_workspace(&root, &options, &HashMap::new(), &cancellation, 2, |_| {}).is_err()
        );

        let options = WorkspaceSearchOptions {
            query: "anything".to_owned(),
            ..Default::default()
        };
        let mut emitted = false;
        search_workspace(&root, &options, &HashMap::new(), &cancellation, 1, |_| {
            emitted = true
        })
        .unwrap();
        assert!(!emitted);
    }
}
