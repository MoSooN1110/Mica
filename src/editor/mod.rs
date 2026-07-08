mod movement;
mod syntax;
mod view;
mod wrap;

pub use movement::{
    display_column, grapheme_width, matching_brackets, move_down, move_left, move_right, move_up,
    move_word_left, move_word_right,
};
pub use syntax::{HighlightKind, HighlightSpan, SyntaxError, SyntaxLanguage, highlight};
pub use view::{BufferSearchState, EditorView};
pub use wrap::{
    VisualRow, move_visual_vertical, visible_visual_rows, visual_index_for_offset,
    visual_row_count, wrapped_line_ranges,
};
