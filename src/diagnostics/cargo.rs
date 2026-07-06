use std::path::Path;

use serde::Deserialize;

use super::{Diagnostic, DiagnosticSeverity, DiagnosticSource, TextPosition, TextRange};

#[derive(Debug, Deserialize)]
struct CargoMessage {
    reason: String,
    message: Option<CompilerMessage>,
}

#[derive(Debug, Deserialize)]
struct CompilerMessage {
    message: String,
    code: Option<CompilerCode>,
    level: String,
    spans: Vec<CompilerSpan>,
}

#[derive(Debug, Deserialize)]
struct CompilerCode {
    code: String,
}

#[derive(Debug, Deserialize)]
struct CompilerSpan {
    file_name: String,
    line_start: usize,
    line_end: usize,
    column_start: usize,
    column_end: usize,
    is_primary: bool,
}

pub fn parse_cargo_messages(root: &Path, output: &[u8]) -> Vec<Diagnostic> {
    String::from_utf8_lossy(output)
        .lines()
        .filter_map(|line| serde_json::from_str::<CargoMessage>(line).ok())
        .filter(|message| message.reason == "compiler-message")
        .filter_map(|message| message.message)
        .flat_map(|message| {
            let severity = match message.level.as_str() {
                "error" | "failure-note" => DiagnosticSeverity::Error,
                "warning" => DiagnosticSeverity::Warning,
                "note" => DiagnosticSeverity::Information,
                _ => DiagnosticSeverity::Hint,
            };
            let code = message.code.map(|code| code.code);
            let source = if code
                .as_deref()
                .is_some_and(|code| code.starts_with("clippy::"))
            {
                DiagnosticSource::Linter
            } else {
                DiagnosticSource::Compiler
            };
            message
                .spans
                .into_iter()
                .filter(|span| span.is_primary)
                .map(move |span| Diagnostic {
                    file: root.join(span.file_name),
                    range: TextRange {
                        start: TextPosition {
                            line: span.line_start.saturating_sub(1),
                            column: span.column_start.saturating_sub(1),
                            char_offset: None,
                        },
                        end: TextPosition {
                            line: span.line_end.saturating_sub(1),
                            column: span.column_end.saturating_sub(1),
                            char_offset: None,
                        },
                    },
                    severity,
                    message: message.message.clone(),
                    source,
                    code: code.clone(),
                    stale: false,
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_structured_primary_rustc_span() {
        let output = br#"{"reason":"compiler-message","message":{"message":"mismatched types","code":{"code":"E0308"},"level":"error","spans":[{"file_name":"src/main.rs","line_start":3,"line_end":3,"column_start":5,"column_end":8,"is_primary":true},{"file_name":"src/main.rs","line_start":1,"line_end":1,"column_start":1,"column_end":2,"is_primary":false}]}}
"#;
        let diagnostics = parse_cargo_messages(Path::new("/workspace"), output);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].file, Path::new("/workspace/src/main.rs"));
        assert_eq!(diagnostics[0].range.start.line, 2);
        assert_eq!(diagnostics[0].code.as_deref(), Some("E0308"));
        assert_eq!(diagnostics[0].source, DiagnosticSource::Compiler);
    }

    #[test]
    fn tags_clippy_lints_as_linter_source() {
        let output = br#"{"reason":"compiler-message","message":{"message":"needless return statement","code":{"code":"clippy::needless_return"},"level":"warning","spans":[{"file_name":"src/lib.rs","line_start":10,"line_end":10,"column_start":5,"column_end":20,"is_primary":true}]}}
"#;
        let diagnostics = parse_cargo_messages(Path::new("/workspace"), output);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].source, DiagnosticSource::Linter);
        assert_eq!(
            diagnostics[0].code.as_deref(),
            Some("clippy::needless_return")
        );
    }
}
