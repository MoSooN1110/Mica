use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TextPosition {
    pub line: usize,
    pub column: usize,
    pub char_offset: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TextRange {
    pub start: TextPosition,
    pub end: TextPosition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DiagnosticSource {
    Compiler,
    Lsp,
    Linter,
    Task,
}

impl DiagnosticSource {
    pub fn priority(self) -> u8 {
        match self {
            Self::Compiler => 0,
            Self::Lsp => 1,
            Self::Linter => 2,
            Self::Task => 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub file: PathBuf,
    pub range: TextRange,
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub source: DiagnosticSource,
    pub code: Option<String>,
    pub stale: bool,
}
