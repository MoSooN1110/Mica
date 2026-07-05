use std::{
    ops::Range,
    sync::atomic::{AtomicU64, Ordering},
};

/// Finds non-overlapping literal matches and returns Unicode scalar offsets.
/// The generation token makes superseded searches stop early.
pub fn find_matches(
    source: &str,
    query: &str,
    cancellation: &AtomicU64,
    generation: u64,
    limit: usize,
) -> Vec<Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    let query_chars = query.chars().count();
    let mut matches = Vec::new();
    let mut previous_byte = 0usize;
    let mut previous_char = 0usize;
    for (byte, _) in source.match_indices(query) {
        if cancellation.load(Ordering::Relaxed) != generation {
            return Vec::new();
        }
        previous_char += source[previous_byte..byte].chars().count();
        matches.push(previous_char..previous_char + query_chars);
        if matches.len() == limit {
            break;
        }
        previous_byte = byte;
    }
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_unicode_scalar_ranges() {
        let token = AtomicU64::new(1);
        assert_eq!(
            find_matches("日a日a", "日a", &token, 1, 100),
            vec![0..2, 2..4]
        );
    }

    #[test]
    fn cancellation_discards_partial_results() {
        let token = AtomicU64::new(2);
        assert!(find_matches("abc abc", "abc", &token, 1, 100).is_empty());
    }
}
