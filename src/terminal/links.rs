use std::{ops::Range, path::PathBuf, sync::OnceLock};

use regex::Regex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalFileReference {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
    pub display_columns: Range<usize>,
}

pub fn find_file_references(text: &str) -> Vec<TerminalFileReference> {
    static PATTERN: OnceLock<Option<Regex>> = OnceLock::new();
    let Some(pattern) = PATTERN.get_or_init(|| {
        Regex::new(r"(?P<path>[^\s:]+\.[^\s:]+):(?P<line>[0-9]+)(?::(?P<column>[0-9]+))?").ok()
    }) else {
        return Vec::new();
    };
    pattern
        .captures_iter(text)
        .filter_map(|captures| {
            let matched = captures.get(0)?;
            let path = captures.name("path")?.as_str();
            let line = captures.name("line")?.as_str().parse().ok()?;
            let column = captures
                .name("column")
                .and_then(|value| value.as_str().parse().ok())
                .unwrap_or(1);
            Some(TerminalFileReference {
                path: PathBuf::from(path),
                line,
                column,
                display_columns: text[..matched.start()].chars().count()
                    ..text[..matched.end()].chars().count(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_relative_and_absolute_compiler_locations() {
        let references = find_file_references("error src/main.rs:12:5 and /tmp/lib.py:3");
        assert_eq!(references.len(), 2);
        assert_eq!(references[0].path, PathBuf::from("src/main.rs"));
        assert_eq!((references[0].line, references[0].column), (12, 5));
        assert_eq!((references[1].line, references[1].column), (3, 1));
    }
}
