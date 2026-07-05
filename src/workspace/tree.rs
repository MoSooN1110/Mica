use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use ignore::WalkBuilder;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeEntryKind {
    Directory,
    File,
    Symlink,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    pub relative_path: PathBuf,
    pub kind: TreeEntryKind,
    pub depth: usize,
}

#[derive(Debug, Default, Clone)]
pub struct FileTree {
    pub entries: Vec<TreeEntry>,
    visible: Vec<usize>,
    expanded: BTreeSet<PathBuf>,
}

#[derive(Debug, Error)]
pub enum TreeError {
    #[error("workspace traversal failed: {0}")]
    Walk(#[from] ignore::Error),
}

impl FileTree {
    /// Builds a flat, sorted tree model. Callers must run this function on a
    /// worker because walking a workspace is blocking I/O.
    pub fn scan(
        root: &Path,
        show_hidden: bool,
        follow_symlinks: bool,
        respect_gitignore: bool,
    ) -> Result<Self, TreeError> {
        let mut builder = WalkBuilder::new(root);
        builder
            .hidden(!show_hidden)
            .follow_links(follow_symlinks)
            .git_ignore(respect_gitignore)
            .git_global(respect_gitignore)
            .git_exclude(respect_gitignore);
        let mut entries = Vec::new();
        for result in builder.build() {
            let entry = result?;
            if entry.path() == root {
                continue;
            }
            let relative_path = match entry.path().strip_prefix(root) {
                Ok(path) => path.to_path_buf(),
                Err(_) => continue,
            };
            let file_type = entry.file_type();
            let kind = if file_type.is_some_and(|kind| kind.is_symlink()) {
                TreeEntryKind::Symlink
            } else if file_type.is_some_and(|kind| kind.is_dir()) {
                TreeEntryKind::Directory
            } else {
                TreeEntryKind::File
            };
            let depth = relative_path.components().count().saturating_sub(1);
            entries.push(TreeEntry {
                relative_path,
                kind,
                depth,
            });
        }
        entries = hierarchical_order(entries);
        let mut tree = Self {
            entries,
            visible: Vec::new(),
            expanded: BTreeSet::new(),
        };
        tree.rebuild_visible();
        Ok(tree)
    }

    pub fn visible_len(&self) -> usize {
        self.visible.len()
    }

    pub fn visible_entry(&self, index: usize) -> Option<&TreeEntry> {
        self.visible
            .get(index)
            .and_then(|entry| self.entries.get(*entry))
    }

    pub fn visible_entries(&self) -> impl Iterator<Item = &TreeEntry> {
        self.visible
            .iter()
            .filter_map(|index| self.entries.get(*index))
    }

    pub fn is_expanded(&self, path: &Path) -> bool {
        self.expanded.contains(path)
    }

    pub fn toggle_visible_directory(&mut self, index: usize) -> bool {
        let Some(path) = self.visible_entry(index).and_then(|entry| {
            (entry.kind == TreeEntryKind::Directory).then(|| entry.relative_path.clone())
        }) else {
            return false;
        };
        if !self.expanded.remove(&path) {
            self.expanded.insert(path);
        }
        self.rebuild_visible();
        true
    }

    pub fn preserve_expansion_from(&mut self, previous: &Self) {
        self.expanded = previous
            .expanded
            .iter()
            .filter(|path| {
                self.entries.iter().any(|entry| {
                    entry.kind == TreeEntryKind::Directory && entry.relative_path == **path
                })
            })
            .cloned()
            .collect();
        self.rebuild_visible();
    }

    fn rebuild_visible(&mut self) {
        self.visible = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                ancestors_expanded(&entry.relative_path, &self.expanded).then_some(index)
            })
            .collect();
    }
}

fn ancestors_expanded(path: &Path, expanded: &BTreeSet<PathBuf>) -> bool {
    let mut ancestor = path.parent();
    while let Some(parent) = ancestor {
        if parent.as_os_str().is_empty() {
            break;
        }
        if !expanded.contains(parent) {
            return false;
        }
        ancestor = parent.parent();
    }
    true
}

fn hierarchical_order(entries: Vec<TreeEntry>) -> Vec<TreeEntry> {
    let mut children: BTreeMap<PathBuf, Vec<TreeEntry>> = BTreeMap::new();
    for entry in entries {
        let parent = entry
            .relative_path
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .to_path_buf();
        children.entry(parent).or_default().push(entry);
    }
    for siblings in children.values_mut() {
        siblings.sort_by(|left, right| {
            kind_rank(left.kind)
                .cmp(&kind_rank(right.kind))
                .then_with(|| {
                    left.relative_path
                        .file_name()
                        .map(|name| name.to_string_lossy().to_ascii_lowercase())
                        .cmp(
                            &right
                                .relative_path
                                .file_name()
                                .map(|name| name.to_string_lossy().to_ascii_lowercase()),
                        )
                })
                .then_with(|| left.relative_path.cmp(&right.relative_path))
        });
    }
    let mut ordered = Vec::new();
    append_children(Path::new(""), &children, &mut ordered);
    ordered
}

fn append_children(
    parent: &Path,
    children: &BTreeMap<PathBuf, Vec<TreeEntry>>,
    ordered: &mut Vec<TreeEntry>,
) {
    let Some(entries) = children.get(parent) else {
        return;
    };
    for entry in entries {
        ordered.push(entry.clone());
        if entry.kind == TreeEntryKind::Directory {
            append_children(&entry.relative_path, children, ordered);
        }
    }
}

fn kind_rank(kind: TreeEntryKind) -> u8 {
    match kind {
        TreeEntryKind::Directory => 0,
        TreeEntryKind::File => 1,
        TreeEntryKind::Symlink => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn directories_are_collapsed_and_sorted_before_files() {
        let root = std::env::temp_dir().join(format!("mica-tree-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src/nested")).unwrap();
        fs::write(root.join("README.md"), b"").unwrap();
        fs::write(root.join("src/main.rs"), b"").unwrap();
        let mut tree = FileTree::scan(&root, false, false, false).unwrap();
        let visible = tree
            .visible_entries()
            .map(|entry| entry.relative_path.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            visible,
            vec![PathBuf::from("src"), PathBuf::from("README.md")]
        );
        assert!(tree.toggle_visible_directory(0));
        let visible = tree
            .visible_entries()
            .map(|entry| entry.relative_path.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            visible,
            vec![
                PathBuf::from("src"),
                PathBuf::from("src/nested"),
                PathBuf::from("src/main.rs"),
                PathBuf::from("README.md"),
            ]
        );
        fs::remove_dir_all(root).unwrap();
    }
}
