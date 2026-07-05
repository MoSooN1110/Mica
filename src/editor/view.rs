use std::ops::Range;

#[derive(Debug, Clone, Default)]
pub struct BufferSearchState {
    pub query: String,
    pub matches: Vec<Range<usize>>,
    pub current: usize,
    pub generation: u64,
}

#[derive(Debug, Clone, Default)]
pub struct EditorView {
    pub scroll_line: usize,
    pub scroll_column: usize,
    pub preferred_display_column: Option<usize>,
    pub search: BufferSearchState,
}
