mod history;
mod persistence;
mod selection;
mod text_buffer;

pub use history::Edit;
pub use persistence::{SaveError, atomic_save, atomic_save_if_unchanged};
pub use selection::{CharOffset, Selection};
pub use text_buffer::{
    BufferError, DiskState, ExternalChangeOutcome, LineEnding, SaveSnapshot, TextBuffer,
};
