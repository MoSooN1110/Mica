use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};

use streaming_iterator::StreamingIterator;
use thiserror::Error;
use tree_sitter::{Parser, Query, QueryCursor};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightKind {
    Keyword,
    Function,
    Type,
    String,
    Number,
    Comment,
    Variable,
    Constant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightSpan {
    pub start_char: usize,
    pub end_char: usize,
    pub kind: HighlightKind,
}

#[derive(Debug, Error)]
pub enum SyntaxError {
    #[error("failed to load Rust parser: {0}")]
    Language(String),
    #[error("failed to compile Rust highlight query: {0}")]
    Query(String),
    #[error("Rust parser did not return a syntax tree")]
    MissingTree,
}

/// Parses Rust off the UI thread and converts tree-sitter byte ranges to the
/// editor's Unicode scalar offsets at this boundary.
pub fn highlight_rust(
    source: &str,
    cancellation: &AtomicU64,
    generation: u64,
) -> Result<Vec<HighlightSpan>, SyntaxError> {
    if cancellation.load(Ordering::Relaxed) != generation {
        return Ok(Vec::new());
    }
    let language = tree_sitter_rust::LANGUAGE.into();
    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .map_err(|error| SyntaxError::Language(error.to_string()))?;
    let tree = parser.parse(source, None).ok_or(SyntaxError::MissingTree)?;
    if cancellation.load(Ordering::Relaxed) != generation {
        return Ok(Vec::new());
    }
    let query = Query::new(&language, tree_sitter_rust::HIGHLIGHTS_QUERY)
        .map_err(|error| SyntaxError::Query(error.to_string()))?;
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut captures = cursor.captures(&query, tree.root_node(), source.as_bytes());
    let mut byte_spans = Vec::new();
    while let Some((matched, capture_index)) = captures.next() {
        if cancellation.load(Ordering::Relaxed) != generation {
            return Ok(Vec::new());
        }
        let capture = matched.captures[*capture_index];
        let name = names[capture.index as usize];
        if let Some(kind) = classify_capture(name) {
            byte_spans.push((capture.node.start_byte(), capture.node.end_byte(), kind));
        }
    }
    Ok(convert_byte_spans(source, byte_spans))
}

fn classify_capture(name: &str) -> Option<HighlightKind> {
    let root = name.split('.').next().unwrap_or(name);
    match root {
        "keyword" | "operator" => Some(HighlightKind::Keyword),
        "function" | "method" => Some(HighlightKind::Function),
        "type" | "constructor" | "attribute" => Some(HighlightKind::Type),
        "string" | "character" => Some(HighlightKind::String),
        "number" | "float" => Some(HighlightKind::Number),
        "comment" => Some(HighlightKind::Comment),
        "variable" | "property" | "parameter" => Some(HighlightKind::Variable),
        "constant" | "boolean" => Some(HighlightKind::Constant),
        _ => None,
    }
}

fn convert_byte_spans(
    source: &str,
    byte_spans: Vec<(usize, usize, HighlightKind)>,
) -> Vec<HighlightSpan> {
    let mut offsets = byte_spans
        .iter()
        .flat_map(|(start, end, _)| [*start, *end])
        .collect::<Vec<_>>();
    offsets.sort_unstable();
    offsets.dedup();
    let mut converted = HashMap::with_capacity(offsets.len());
    let mut target = 0usize;
    for (char_offset, (byte_offset, _)) in source.char_indices().enumerate() {
        while target < offsets.len() && offsets[target] == byte_offset {
            converted.insert(byte_offset, char_offset);
            target += 1;
        }
    }
    let char_len = source.chars().count();
    converted.insert(source.len(), char_len);
    let mut spans = byte_spans
        .into_iter()
        .filter_map(|(start, end, kind)| {
            Some(HighlightSpan {
                start_char: *converted.get(&start)?,
                end_char: *converted.get(&end)?,
                kind,
            })
        })
        .collect::<Vec<_>>();
    spans.sort_by_key(|span| (span.start_char, span.end_char));
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_highlights_include_keywords_strings_and_comments() {
        let token = AtomicU64::new(1);
        let source = "// 日本語\nfn main() { let value = \"ok\"; }";
        let spans = highlight_rust(source, &token, 1).unwrap();
        assert!(spans.iter().any(|span| span.kind == HighlightKind::Comment));
        assert!(spans.iter().any(|span| span.kind == HighlightKind::Keyword));
        assert!(spans.iter().any(|span| span.kind == HighlightKind::String));
        assert!(
            spans
                .iter()
                .all(|span| span.end_char <= source.chars().count())
        );
    }
}
