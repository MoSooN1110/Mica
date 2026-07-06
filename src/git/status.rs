use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitFileKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Unmerged,
    Untracked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitFileChange {
    pub path: PathBuf,
    pub original_path: Option<PathBuf>,
    pub kind: GitFileKind,
    pub index_status: char,
    pub worktree_status: char,
    pub staged: bool,
    pub unstaged: bool,
    pub conflicted: bool,
    pub untracked: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitStatus {
    pub branch: Option<String>,
    pub detached: bool,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub files: Vec<GitFileChange>,
}

/// Single-character status symbol for a per-file Git state indicator
/// (SPEC/03_workspace.md §2.1: "ファイルごとのGit状態表示(記号+色。例: `M`
/// `A` `D` `U` `!`)"). Pure mapping so callers (the Explorer tree renderer)
/// can pair it with a theme color without this module depending on
/// ratatui/theme types.
///
/// Conflicted files always resolve to `!` regardless of `kind`. Otherwise
/// untracked files resolve to `U`; everything else maps from
/// [`GitFileKind`], mirroring the markers already used by the Source
/// Control sidebar (`Renamed` -> `R`, `Copied` -> `C`, `TypeChanged` -> `T`).
#[must_use]
pub fn status_symbol(change: &GitFileChange) -> char {
    if change.conflicted {
        return '!';
    }
    if change.untracked {
        return 'U';
    }
    match change.kind {
        GitFileKind::Added => 'A',
        GitFileKind::Modified => 'M',
        GitFileKind::Deleted => 'D',
        GitFileKind::Renamed => 'R',
        GitFileKind::Copied => 'C',
        GitFileKind::TypeChanged => 'T',
        GitFileKind::Unmerged => '!',
        GitFileKind::Untracked => 'U',
    }
}

pub fn parse_porcelain_v2(bytes: &[u8]) -> GitStatus {
    let records = bytes
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .collect::<Vec<_>>();
    let mut status = GitStatus::default();
    let mut index = 0usize;
    while index < records.len() {
        let record = String::from_utf8_lossy(records[index]);
        if let Some(branch) = record.strip_prefix("# branch.head ") {
            status.detached = branch == "(detached)";
            status.branch = (!status.detached).then(|| branch.to_owned());
        } else if let Some(upstream) = record.strip_prefix("# branch.upstream ") {
            status.upstream = Some(upstream.to_owned());
        } else if let Some(ab) = record.strip_prefix("# branch.ab ") {
            for part in ab.split_whitespace() {
                if let Some(ahead) = part.strip_prefix('+').and_then(|value| value.parse().ok()) {
                    status.ahead = ahead;
                } else if let Some(behind) =
                    part.strip_prefix('-').and_then(|value| value.parse().ok())
                {
                    status.behind = behind;
                }
            }
        } else if let Some(change) = parse_ordinary(&record) {
            status.files.push(change);
        } else if record.starts_with("2 ") {
            let original = records
                .get(index + 1)
                .map(|path| PathBuf::from(String::from_utf8_lossy(path).into_owned()));
            if let Some(change) = parse_renamed(&record, original) {
                status.files.push(change);
            }
            index = index.saturating_add(1);
        } else if let Some(path) = record.strip_prefix("? ") {
            status.files.push(GitFileChange {
                path: PathBuf::from(path),
                original_path: None,
                kind: GitFileKind::Untracked,
                index_status: '?',
                worktree_status: '?',
                staged: false,
                unstaged: true,
                conflicted: false,
                untracked: true,
            });
        } else if record.starts_with("u ")
            && let Some(change) = parse_unmerged(&record)
        {
            status.files.push(change);
        }
        index = index.saturating_add(1);
    }
    status
        .files
        .sort_by(|left, right| left.path.cmp(&right.path));
    status
}

fn parse_ordinary(record: &str) -> Option<GitFileChange> {
    if !record.starts_with("1 ") {
        return None;
    }
    let fields = record.splitn(9, ' ').collect::<Vec<_>>();
    let xy = *fields.get(1)?;
    let path = PathBuf::from(*fields.get(8)?);
    change_from_xy(path, None, xy)
}

fn parse_renamed(record: &str, original_path: Option<PathBuf>) -> Option<GitFileChange> {
    let fields = record.splitn(10, ' ').collect::<Vec<_>>();
    let xy = *fields.get(1)?;
    let path = PathBuf::from(*fields.get(9)?);
    change_from_xy(path, original_path, xy)
}

fn parse_unmerged(record: &str) -> Option<GitFileChange> {
    let fields = record.splitn(11, ' ').collect::<Vec<_>>();
    let xy = *fields.get(1)?;
    let path = PathBuf::from(*fields.get(10)?);
    let mut change = change_from_xy(path, None, xy)?;
    change.kind = GitFileKind::Unmerged;
    change.conflicted = true;
    Some(change)
}

fn change_from_xy(
    path: PathBuf,
    original_path: Option<PathBuf>,
    xy: &str,
) -> Option<GitFileChange> {
    let mut chars = xy.chars();
    let index_status = chars.next()?;
    let worktree_status = chars.next()?;
    let conflicted = index_status == 'U'
        || worktree_status == 'U'
        || matches!((index_status, worktree_status), ('A', 'A') | ('D', 'D'));
    let kind = if conflicted {
        GitFileKind::Unmerged
    } else {
        kind_from_codes(index_status, worktree_status)
    };
    Some(GitFileChange {
        path,
        original_path,
        kind,
        index_status,
        worktree_status,
        staged: index_status != '.',
        unstaged: worktree_status != '.',
        conflicted,
        untracked: false,
    })
}

fn kind_from_codes(index: char, worktree: char) -> GitFileKind {
    let code = if worktree != '.' { worktree } else { index };
    match code {
        'A' => GitFileKind::Added,
        'D' => GitFileKind::Deleted,
        'R' => GitFileKind::Renamed,
        'C' => GitFileKind::Copied,
        'T' => GitFileKind::TypeChanged,
        _ => GitFileKind::Modified,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_branch_counts_and_mixed_file_states() {
        let input = b"# branch.head main\x00# branch.upstream origin/main\x00# branch.ab +2 -1\x001 M. N... 100644 100644 100644 aaa bbb staged.rs\x001 .M N... 100644 100644 100644 aaa aaa work.rs\x00? \xe6\x97\xa5\xe6\x9c\xac.rs\x00";
        let status = parse_porcelain_v2(input);
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!((status.ahead, status.behind), (2, 1));
        assert_eq!(status.files.len(), 3);
        assert!(status.files.iter().any(|file| file.staged));
        assert!(status.files.iter().any(|file| file.untracked));
    }

    fn change(kind: GitFileKind, conflicted: bool, untracked: bool) -> GitFileChange {
        GitFileChange {
            path: PathBuf::from("file.rs"),
            original_path: None,
            kind,
            index_status: '.',
            worktree_status: '.',
            staged: false,
            unstaged: true,
            conflicted,
            untracked,
        }
    }

    #[test]
    fn status_symbol_maps_each_kind() {
        assert_eq!(
            status_symbol(&change(GitFileKind::Added, false, false)),
            'A'
        );
        assert_eq!(
            status_symbol(&change(GitFileKind::Modified, false, false)),
            'M'
        );
        assert_eq!(
            status_symbol(&change(GitFileKind::Deleted, false, false)),
            'D'
        );
        assert_eq!(
            status_symbol(&change(GitFileKind::Renamed, false, false)),
            'R'
        );
        assert_eq!(
            status_symbol(&change(GitFileKind::Copied, false, false)),
            'C'
        );
        assert_eq!(
            status_symbol(&change(GitFileKind::TypeChanged, false, false)),
            'T'
        );
        assert_eq!(
            status_symbol(&change(GitFileKind::Untracked, false, true)),
            'U'
        );
    }

    #[test]
    fn status_symbol_conflicted_overrides_kind() {
        // Even an `Added` file that is mid-merge-conflict must show `!`, not
        // `A`: conflict resolution takes priority over the underlying kind.
        assert_eq!(status_symbol(&change(GitFileKind::Added, true, false)), '!');
        assert_eq!(
            status_symbol(&change(GitFileKind::Unmerged, true, false)),
            '!'
        );
    }
}
