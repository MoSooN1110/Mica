mod movement;
mod syntax;
mod view;

pub use movement::{display_column, move_down, move_left, move_right, move_up};
pub use syntax::{HighlightKind, HighlightSpan, SyntaxError, SyntaxLanguage, highlight};
pub use view::{BufferSearchState, EditorView};
