use std::ops::Range;

use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation;

use super::{display_column, grapheme_width, movement::char_offset_for_display_column};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisualRow {
    pub line_index: usize,
    pub line_start_char: usize,
    pub range_in_line: Range<usize>,
}

pub fn wrapped_line_ranges(
    line: &str,
    width: usize,
    tab_width: usize,
    ambiguous_width_wide: bool,
) -> Vec<Range<usize>> {
    let content = line.trim_end_matches(['\r', '\n']);
    let width = width.max(1);
    if content.is_empty() {
        return std::iter::once(0..0).collect();
    }
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut chars = 0;
    let mut column = 0;
    for grapheme in content.graphemes(true) {
        let count = grapheme.chars().count();
        let grapheme_columns = if grapheme == "\t" {
            tab_width - column % tab_width
        } else {
            grapheme_width(grapheme, ambiguous_width_wide)
        };
        if column > 0 && column + grapheme_columns > width {
            ranges.push(start..chars);
            start = chars;
            column = 0;
        }
        column += if grapheme == "\t" {
            tab_width - column % tab_width
        } else {
            grapheme_columns
        };
        chars += count;
    }
    ranges.push(start..chars);
    ranges
}

pub fn visible_visual_rows(
    text: &Rope,
    start_visual_row: usize,
    count: usize,
    width: usize,
    tab_width: usize,
    ambiguous_width_wide: bool,
) -> Vec<VisualRow> {
    let mut visual_index = 0;
    let mut rows = Vec::new();
    for line_index in 0..text.len_lines() {
        let line_start_char = text.line_to_char(line_index);
        let line = text.line(line_index).to_string();
        for range_in_line in wrapped_line_ranges(&line, width, tab_width, ambiguous_width_wide) {
            if visual_index >= start_visual_row {
                rows.push(VisualRow {
                    line_index,
                    line_start_char,
                    range_in_line,
                });
                if rows.len() >= count {
                    return rows;
                }
            }
            visual_index += 1;
        }
    }
    rows
}

pub fn visual_row_count(
    text: &Rope,
    width: usize,
    tab_width: usize,
    ambiguous_width_wide: bool,
) -> usize {
    text.lines()
        .map(|line| {
            wrapped_line_ranges(&line.to_string(), width, tab_width, ambiguous_width_wide).len()
        })
        .sum()
}

pub fn visual_index_for_offset(
    text: &Rope,
    char_offset: usize,
    width: usize,
    tab_width: usize,
    ambiguous_width_wide: bool,
) -> usize {
    let offset = char_offset.min(text.len_chars());
    let target_line = text.char_to_line(offset);
    let target_in_line = offset - text.line_to_char(target_line);
    let mut visual_index = 0;
    for line_index in 0..=target_line {
        let line = text.line(line_index).to_string();
        let ranges = wrapped_line_ranges(&line, width, tab_width, ambiguous_width_wide);
        if line_index == target_line {
            return visual_index
                + ranges
                    .iter()
                    .position(|range| target_in_line < range.end)
                    .unwrap_or_else(|| ranges.len().saturating_sub(1));
        }
        visual_index += ranges.len();
    }
    visual_index
}

pub fn move_visual_vertical(
    text: &Rope,
    char_offset: usize,
    preferred: Option<usize>,
    width: usize,
    tab_width: usize,
    ambiguous_width_wide: bool,
    down: bool,
) -> (usize, usize) {
    let current_index =
        visual_index_for_offset(text, char_offset, width, tab_width, ambiguous_width_wide);
    let current = visible_visual_rows(
        text,
        current_index,
        1,
        width,
        tab_width,
        ambiguous_width_wide,
    )
    .into_iter()
    .next();
    let Some(current) = current else {
        return (char_offset.min(text.len_chars()), preferred.unwrap_or(0));
    };
    let line = text.line(current.line_index).to_string();
    let segment = line
        .trim_end_matches(['\r', '\n'])
        .chars()
        .skip(current.range_in_line.start)
        .take(current.range_in_line.len())
        .collect::<String>();
    let relative = char_offset
        .saturating_sub(current.line_start_char + current.range_in_line.start)
        .min(segment.chars().count());
    let target_column = preferred
        .unwrap_or_else(|| display_column(&segment, relative, tab_width, ambiguous_width_wide));
    let target_index = if down {
        current_index.saturating_add(1)
    } else {
        current_index.saturating_sub(1)
    };
    if target_index == current_index {
        return (char_offset, target_column);
    }
    let target = visible_visual_rows(
        text,
        target_index,
        1,
        width,
        tab_width,
        ambiguous_width_wide,
    )
    .into_iter()
    .next();
    let Some(target) = target else {
        return (char_offset, target_column);
    };
    let target_line = text.line(target.line_index).to_string();
    let target_segment = target_line
        .trim_end_matches(['\r', '\n'])
        .chars()
        .skip(target.range_in_line.start)
        .take(target.range_in_line.len())
        .collect::<String>();
    let relative_target = char_offset_for_display_column(
        &target_segment,
        target_column,
        tab_width,
        ambiguous_width_wide,
    );
    (
        target.line_start_char + target.range_in_line.start + relative_target,
        target_column,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_without_splitting_unicode_graphemes() {
        assert_eq!(
            wrapped_line_ranges("ab日本👩‍💻z", 4, 4, false),
            vec![0..3, 3..7, 7..8]
        );
    }

    #[test]
    fn maps_offsets_and_visible_rows_consistently() {
        let text = Rope::from_str("abcdef\nxy\n");
        assert_eq!(visual_row_count(&text, 3, 4, false), 4);
        assert_eq!(visual_index_for_offset(&text, 4, 3, 4, false), 1);
        let rows = visible_visual_rows(&text, 1, 2, 3, 4, false);
        assert_eq!(rows[0].range_in_line, 3..6);
        assert_eq!(rows[1].line_index, 1);
    }

    #[test]
    fn vertical_movement_uses_wrapped_visual_rows() {
        let text = Rope::from_str("abcdef\n");
        assert_eq!(move_visual_vertical(&text, 1, None, 3, 4, false, true).0, 4);
        assert_eq!(
            move_visual_vertical(&text, 4, None, 3, 4, false, false).0,
            1
        );
    }
}
