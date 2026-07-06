use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};

use streaming_iterator::StreamingIterator;
use thiserror::Error;
use tree_sitter::{Language, Parser, Query, QueryCursor};

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

/// Languages with built-in Tree-sitter syntax highlighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxLanguage {
    Rust,
    Markdown,
    Toml,
}

impl SyntaxLanguage {
    /// Maps a file extension (without the leading dot) to a supported syntax
    /// language. Returns `None` for unsupported extensions, which callers
    /// treat as plain text.
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension {
            "rs" => Some(Self::Rust),
            "md" | "markdown" => Some(Self::Markdown),
            "toml" => Some(Self::Toml),
            _ => None,
        }
    }
}

#[derive(Debug, Error)]
pub enum SyntaxError {
    #[error("failed to load parser: {0}")]
    Language(String),
    #[error("failed to compile highlight query: {0}")]
    Query(String),
    #[error("parser did not return a syntax tree")]
    MissingTree,
}

/// Parses `source` as `language` off the UI thread and converts Tree-sitter
/// byte ranges to the editor's Unicode scalar offsets at this boundary.
///
/// Markdown is parsed with both the block grammar (headings, code fences,
/// lists, ...) and the inline grammar (emphasis, links, code spans, ...) run
/// over the whole source; their captures are merged. This is a minimal slice
/// rather than a full injection system: the inline grammar is not restricted
/// to the exact ranges the block parser identifies as inline content, so a
/// few spans (for example fenced code bodies) are re-visited by both passes.
/// The renderer already resolves overlaps by preferring the narrowest span,
/// so this does not produce incorrect coloring, only occasional redundancy.
pub fn highlight(
    language: SyntaxLanguage,
    source: &str,
    cancellation: &AtomicU64,
    generation: u64,
) -> Result<Vec<HighlightSpan>, SyntaxError> {
    if cancellation.load(Ordering::Relaxed) != generation {
        return Ok(Vec::new());
    }
    let byte_spans = match language {
        SyntaxLanguage::Rust => collect_spans(
            tree_sitter_rust::LANGUAGE.into(),
            tree_sitter_rust::HIGHLIGHTS_QUERY,
            source,
            cancellation,
            generation,
        )?,
        SyntaxLanguage::Toml => collect_spans(
            tree_sitter_toml_ng::LANGUAGE.into(),
            tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
            source,
            cancellation,
            generation,
        )?,
        SyntaxLanguage::Markdown => {
            let mut spans = collect_spans(
                tree_sitter_md::LANGUAGE.into(),
                tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
                source,
                cancellation,
                generation,
            )?;
            spans.extend(collect_spans(
                tree_sitter_md::INLINE_LANGUAGE.into(),
                tree_sitter_md::HIGHLIGHT_QUERY_INLINE,
                source,
                cancellation,
                generation,
            )?);
            spans
        }
    };
    Ok(convert_byte_spans(source, byte_spans))
}

/// Parses `source` with a single Tree-sitter grammar and highlight query,
/// returning classified byte spans. Shared by every language in [`highlight`].
fn collect_spans(
    language: Language,
    highlights_query: &str,
    source: &str,
    cancellation: &AtomicU64,
    generation: u64,
) -> Result<Vec<(usize, usize, HighlightKind)>, SyntaxError> {
    if cancellation.load(Ordering::Relaxed) != generation {
        return Ok(Vec::new());
    }
    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .map_err(|error| SyntaxError::Language(error.to_string()))?;
    let tree = parser.parse(source, None).ok_or(SyntaxError::MissingTree)?;
    if cancellation.load(Ordering::Relaxed) != generation {
        return Ok(Vec::new());
    }
    let query = Query::new(&language, highlights_query)
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
    Ok(byte_spans)
}

/// Maps a Tree-sitter capture name to an editor [`HighlightKind`].
///
/// Rust and TOML captures follow the common `nvim-treesitter`-style capture
/// vocabulary (`keyword`, `type`, `string`, `property`, `boolean`, ...) and
/// are classified by their root name. Markdown's captures use a different
/// vocabulary (`text.title`, `text.literal`, `punctuation.special`, ...) and
/// are mapped explicitly:
///
/// - `text.title` (heading text) and `punctuation.special` (heading/list/
///   blockquote markers) -> `Keyword`, matching structural markup.
/// - `text.literal` (fenced/indented code blocks, code spans) -> `String`.
/// - `text.uri` / `text.reference` (link destinations, labels, images) ->
///   `Type`.
/// - `text.emphasis` / `text.strong` -> `Constant`.
/// - Everything else Markdown-specific (`punctuation.delimiter`, `none`) is
///   left unhighlighted.
fn classify_capture(name: &str) -> Option<HighlightKind> {
    match name {
        "text.title" | "punctuation.special" => return Some(HighlightKind::Keyword),
        "text.literal" => return Some(HighlightKind::String),
        "text.uri" | "text.reference" => return Some(HighlightKind::Type),
        "text.emphasis" | "text.strong" => return Some(HighlightKind::Constant),
        _ => {}
    }
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
        let spans = highlight(SyntaxLanguage::Rust, source, &token, 1).unwrap();
        assert!(spans.iter().any(|span| span.kind == HighlightKind::Comment));
        assert!(spans.iter().any(|span| span.kind == HighlightKind::Keyword));
        assert!(spans.iter().any(|span| span.kind == HighlightKind::String));
        assert!(
            spans
                .iter()
                .all(|span| span.end_char <= source.chars().count())
        );
    }

    #[test]
    fn markdown_highlights_include_heading_code_and_japanese_text() {
        let token = AtomicU64::new(1);
        let source = "# 見出し\n\n```rust\nfn main() {}\n```\n";
        let spans = highlight(SyntaxLanguage::Markdown, source, &token, 1).unwrap();
        assert!(!spans.is_empty());
        assert!(spans.iter().any(|span| span.kind == HighlightKind::Keyword));
        assert!(spans.iter().any(|span| span.kind == HighlightKind::String));
        let char_len = source.chars().count();
        assert!(spans.iter().all(|span| span.end_char <= char_len));
    }

    #[test]
    fn toml_highlights_include_table_key_string_comment_and_japanese_value() {
        let token = AtomicU64::new(1);
        let source = "# comment\n[section]\nkey = \"値\"\n";
        let spans = highlight(SyntaxLanguage::Toml, source, &token, 1).unwrap();
        assert!(spans.iter().any(|span| span.kind == HighlightKind::Type));
        assert!(spans.iter().any(|span| span.kind == HighlightKind::String));
        assert!(spans.iter().any(|span| span.kind == HighlightKind::Comment));
        let char_len = source.chars().count();
        assert!(spans.iter().all(|span| span.end_char <= char_len));
    }
}
