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
    C,
    Cpp,
    Python,
    Json,
    Markdown,
    Toml,
    Yaml,
    Bash,
    JavaScript,
    TypeScript,
    Tsx,
    Html,
    Css,
}

impl SyntaxLanguage {
    pub fn comment_tokens(self) -> Option<(&'static str, Option<&'static str>)> {
        match self {
            Self::Rust | Self::C | Self::Cpp | Self::JavaScript | Self::TypeScript | Self::Tsx => {
                Some(("//", None))
            }
            Self::Python | Self::Yaml | Self::Bash | Self::Toml => Some(("#", None)),
            Self::Html | Self::Markdown => Some(("<!--", Some("-->"))),
            Self::Css => Some(("/*", Some("*/"))),
            Self::Json => None,
        }
    }

    /// Maps a file extension (without the leading dot) to a supported syntax
    /// language. Returns `None` for unsupported extensions, which callers
    /// treat as plain text.
    ///
    /// This is the fallback path for files with no matching entry in
    /// `settings.languages` (`SPEC/06_diagnostics_lsp.md`'s LSP language
    /// table) — see [`Self::from_language_name`], which most callers should
    /// try first so syntax highlighting and LSP language detection agree.
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension {
            "rs" => Some(Self::Rust),
            "c" | "h" => Some(Self::C),
            "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" => Some(Self::Cpp),
            "py" | "pyi" => Some(Self::Python),
            "json" | "jsonc" => Some(Self::Json),
            "md" | "markdown" => Some(Self::Markdown),
            "toml" => Some(Self::Toml),
            "yaml" | "yml" => Some(Self::Yaml),
            "sh" | "bash" => Some(Self::Bash),
            "js" | "jsx" | "mjs" | "cjs" => Some(Self::JavaScript),
            "ts" | "mts" | "cts" => Some(Self::TypeScript),
            "tsx" => Some(Self::Tsx),
            "html" | "htm" => Some(Self::Html),
            "css" => Some(Self::Css),
            _ => None,
        }
    }

    /// Maps a `settings.languages` language name (e.g. `"rust"`,
    /// `"markdown"`) to a supported syntax language. This is the
    /// settings-driven counterpart to [`Self::from_extension`]: callers that
    /// already resolved a language name for LSP purposes should use this
    /// first, so a file's syntax highlighting and its LSP language always
    /// agree, falling back to `from_extension` only when there is no
    /// matching language configured in settings (e.g. TOML, which has no
    /// default LSP entry but is still highlighted by extension).
    pub fn from_language_name(name: &str) -> Option<Self> {
        match name {
            "rust" => Some(Self::Rust),
            "c_cpp" | "cpp" => Some(Self::Cpp),
            "c" => Some(Self::C),
            "python" => Some(Self::Python),
            "json" | "jsonc" => Some(Self::Json),
            "markdown" => Some(Self::Markdown),
            "toml" => Some(Self::Toml),
            "yaml" => Some(Self::Yaml),
            "bash" | "shell" => Some(Self::Bash),
            "javascript" => Some(Self::JavaScript),
            "typescript" => Some(Self::TypeScript),
            "tsx" => Some(Self::Tsx),
            "html" => Some(Self::Html),
            "css" => Some(Self::Css),
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
        SyntaxLanguage::C => collect_spans(
            tree_sitter_c::LANGUAGE.into(),
            tree_sitter_c::HIGHLIGHT_QUERY,
            source,
            cancellation,
            generation,
        )?,
        SyntaxLanguage::Cpp => collect_spans(
            tree_sitter_cpp::LANGUAGE.into(),
            tree_sitter_cpp::HIGHLIGHT_QUERY,
            source,
            cancellation,
            generation,
        )?,
        SyntaxLanguage::Python => collect_spans(
            tree_sitter_python::LANGUAGE.into(),
            tree_sitter_python::HIGHLIGHTS_QUERY,
            source,
            cancellation,
            generation,
        )?,
        SyntaxLanguage::Json => collect_spans(
            tree_sitter_json::LANGUAGE.into(),
            tree_sitter_json::HIGHLIGHTS_QUERY,
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
        SyntaxLanguage::Yaml => collect_spans(
            tree_sitter_yaml::LANGUAGE.into(),
            tree_sitter_yaml::HIGHLIGHTS_QUERY,
            source,
            cancellation,
            generation,
        )?,
        SyntaxLanguage::Bash => collect_spans(
            tree_sitter_bash::LANGUAGE.into(),
            tree_sitter_bash::HIGHLIGHT_QUERY,
            source,
            cancellation,
            generation,
        )?,
        SyntaxLanguage::JavaScript => collect_spans(
            tree_sitter_javascript::LANGUAGE.into(),
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            source,
            cancellation,
            generation,
        )?,
        SyntaxLanguage::TypeScript => collect_typescript_spans(
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            source,
            cancellation,
            generation,
        )?,
        SyntaxLanguage::Tsx => collect_typescript_spans(
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            source,
            cancellation,
            generation,
        )?,
        SyntaxLanguage::Html => collect_spans(
            tree_sitter_html::LANGUAGE.into(),
            tree_sitter_html::HIGHLIGHTS_QUERY,
            source,
            cancellation,
            generation,
        )?,
        SyntaxLanguage::Css => collect_spans(
            tree_sitter_css::LANGUAGE.into(),
            tree_sitter_css::HIGHLIGHTS_QUERY,
            source,
            cancellation,
            generation,
        )?,
    };
    Ok(convert_byte_spans(source, byte_spans))
}

fn collect_typescript_spans(
    language: Language,
    source: &str,
    cancellation: &AtomicU64,
    generation: u64,
) -> Result<Vec<(usize, usize, HighlightKind)>, SyntaxError> {
    // The TypeScript grammar publishes only TypeScript-specific additions;
    // JavaScript's base captures are required for identifiers, literals,
    // functions, JSX, and comments.
    let query = format!(
        "{}\n{}",
        tree_sitter_javascript::HIGHLIGHT_QUERY,
        tree_sitter_typescript::HIGHLIGHTS_QUERY
    );
    collect_spans(language, &query, source, cancellation, generation)
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

    #[test]
    fn every_spec_language_produces_classified_highlights() {
        let token = AtomicU64::new(1);
        let samples = [
            (SyntaxLanguage::C, "int main(void) { return 0; }"),
            (SyntaxLanguage::Cpp, "class Value { public: int n; };"),
            (
                SyntaxLanguage::Python,
                "def greet(name):\n    return f'hi {name}'\n",
            ),
            (SyntaxLanguage::Json, r#"{"name": true, "count": 2}"#),
            (SyntaxLanguage::Yaml, "name: value\nenabled: true\n"),
            (SyntaxLanguage::Bash, "#!/bin/bash\necho \"hello\"\n"),
            (
                SyntaxLanguage::JavaScript,
                "const greet = (name) => `hi ${name}`;",
            ),
            (SyntaxLanguage::TypeScript, "const count: number = 2;"),
            (SyntaxLanguage::Tsx, "const view = <div>Hello</div>;"),
            (SyntaxLanguage::Html, "<main class=\"content\">Hello</main>"),
            (SyntaxLanguage::Css, ".content { color: red; }"),
        ];
        for (language, source) in samples {
            let spans = highlight(language, source, &token, 1).unwrap();
            assert!(!spans.is_empty(), "no highlights for {language:?}");
            assert!(
                spans
                    .iter()
                    .all(|span| span.end_char <= source.chars().count()),
                "invalid Unicode offset for {language:?}"
            );
        }
    }
}
