mod backend;
mod branch;
mod diff;
mod status;

pub use backend::{GitBackend, GitCliBackend, GitError};
pub use branch::GitBranch;
pub use diff::{DiffHunk, DiffLine, DiffLineKind, DiffTarget, FileDiff, parse_unified_diff};
pub use status::{GitFileChange, GitFileKind, GitStatus, parse_porcelain_v2, status_symbol};
