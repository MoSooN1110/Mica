mod buffer;
mod files;
mod workspace;

pub use buffer::find_matches;
pub use files::{FileMatch, fuzzy_files, fuzzy_label_indices};
pub use workspace::{
    WorkspaceMatch, WorkspaceSearchError, WorkspaceSearchOptions, search_workspace,
};
