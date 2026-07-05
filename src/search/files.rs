use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{CaseMatching, Normalization, Pattern},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMatch {
    pub path: PathBuf,
    pub score: u32,
}

/// Ranks file paths off the UI thread. A newer generation cancels this scan.
pub fn fuzzy_files(
    query: &str,
    paths: Vec<PathBuf>,
    cancellation: &AtomicU64,
    generation: u64,
    limit: usize,
) -> Vec<FileMatch> {
    if query.is_empty() {
        return paths
            .into_iter()
            .take(limit)
            .map(|path| FileMatch { path, score: 0 })
            .collect();
    }
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let mut utf32 = Vec::new();
    let mut matches = Vec::new();
    for path in paths {
        if cancellation.load(Ordering::Relaxed) != generation {
            return Vec::new();
        }
        let display = path.to_string_lossy();
        if let Some(score) = pattern.score(Utf32Str::new(&display, &mut utf32), &mut matcher) {
            matches.push(FileMatch { path, score });
        }
    }
    matches.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| {
                left.path
                    .components()
                    .count()
                    .cmp(&right.path.components().count())
            })
            .then_with(|| left.path.cmp(&right.path))
    });
    matches.truncate(limit);
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_search_ranks_matching_paths() {
        let cancellation = AtomicU64::new(1);
        let matches = fuzzy_files(
            "mnr",
            vec![
                PathBuf::from("docs/manual.md"),
                PathBuf::from("src/main.rs"),
                PathBuf::from("src/lib.rs"),
            ],
            &cancellation,
            1,
            20,
        );
        assert_eq!(matches[0].path, PathBuf::from("src/main.rs"));
    }

    #[test]
    fn newer_generation_cancels_search() {
        let cancellation = AtomicU64::new(2);
        assert!(
            fuzzy_files(
                "src",
                vec![PathBuf::from("src/main.rs")],
                &cancellation,
                1,
                20,
            )
            .is_empty()
        );
    }
}
