use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffTarget {
    WorkingTree,
    Staged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
    Meta,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub text: String,
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
    pub heading: String,
    pub lines: Vec<DiffLine>,
    /// A standalone patch containing file headers and this hunk.
    pub patch: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub path: PathBuf,
    pub target: DiffTarget,
    pub binary: bool,
    pub hunks: Vec<DiffHunk>,
    pub raw: String,
}

pub fn parse_unified_diff(path: PathBuf, target: DiffTarget, raw: String) -> FileDiff {
    let binary = raw.lines().any(|line| {
        line.starts_with("Binary files ")
            || line == "GIT binary patch"
            || line == "Binary files differ"
    });
    let lines = raw.lines().collect::<Vec<_>>();
    let first_hunk = lines
        .iter()
        .position(|line| line.starts_with("@@ "))
        .unwrap_or(lines.len());
    let header = if first_hunk == 0 {
        String::new()
    } else {
        format!("{}\n", lines[..first_hunk].join("\n"))
    };
    let mut hunks = Vec::new();
    let mut index = first_hunk;
    while index < lines.len() {
        if !lines[index].starts_with("@@ ") {
            index += 1;
            continue;
        }
        let hunk_start = index;
        index += 1;
        while index < lines.len() && !lines[index].starts_with("@@ ") {
            index += 1;
        }
        if let Some(hunk) = parse_hunk(&lines[hunk_start..index], &header) {
            hunks.push(hunk);
        }
    }
    FileDiff {
        path,
        target,
        binary,
        hunks,
        raw,
    }
}

fn parse_hunk(lines: &[&str], file_header: &str) -> Option<DiffHunk> {
    let header = *lines.first()?;
    let (old_start, old_count, new_start, new_count, heading) = parse_hunk_header(header)?;
    let mut old_line = old_start;
    let mut new_line = new_start;
    let mut parsed_lines = Vec::new();
    for line in lines.iter().skip(1) {
        let (kind, old, new) = if line.starts_with('+') && !line.starts_with("+++") {
            let current = new_line;
            new_line = new_line.saturating_add(1);
            (DiffLineKind::Added, None, Some(current))
        } else if line.starts_with('-') && !line.starts_with("---") {
            let current = old_line;
            old_line = old_line.saturating_add(1);
            (DiffLineKind::Removed, Some(current), None)
        } else if line.starts_with(' ') {
            let old_current = old_line;
            let new_current = new_line;
            old_line = old_line.saturating_add(1);
            new_line = new_line.saturating_add(1);
            (DiffLineKind::Context, Some(old_current), Some(new_current))
        } else {
            (DiffLineKind::Meta, None, None)
        };
        parsed_lines.push(DiffLine {
            kind,
            text: (*line).to_owned(),
            old_line: old,
            new_line: new,
        });
    }
    let patch = format!("{file_header}{}\n", lines.join("\n"));
    Some(DiffHunk {
        old_start,
        old_count,
        new_start,
        new_count,
        heading,
        lines: parsed_lines,
        patch,
    })
}

fn parse_hunk_header(header: &str) -> Option<(usize, usize, usize, usize, String)> {
    let remainder = header.strip_prefix("@@ -")?;
    let (old, remainder) = remainder.split_once(" +")?;
    let (new, heading) = remainder.split_once(" @@")?;
    let (old_start, old_count) = parse_range(old)?;
    let (new_start, new_count) = parse_range(new)?;
    Some((
        old_start,
        old_count,
        new_start,
        new_count,
        heading.trim().to_owned(),
    ))
}

fn parse_range(range: &str) -> Option<(usize, usize)> {
    match range.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((range.parse().ok()?, 1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hunks_and_line_numbers() {
        let raw = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,2 +1,3 @@ fn main\n old\n-removed\n+added\n+extra\n".to_owned();
        let diff = parse_unified_diff(PathBuf::from("a.rs"), DiffTarget::WorkingTree, raw);
        assert_eq!(diff.hunks.len(), 1);
        let hunk = &diff.hunks[0];
        assert_eq!((hunk.old_start, hunk.new_start), (1, 1));
        assert_eq!(hunk.lines[1].old_line, Some(2));
        assert_eq!(hunk.lines[2].new_line, Some(2));
        assert!(hunk.patch.starts_with("diff --git"));
    }

    #[test]
    fn identifies_binary_diff() {
        let diff = parse_unified_diff(
            PathBuf::from("image.png"),
            DiffTarget::WorkingTree,
            "Binary files a/image.png and b/image.png differ\n".to_owned(),
        );
        assert!(diff.binary);
        assert!(diff.hunks.is_empty());
    }
}
