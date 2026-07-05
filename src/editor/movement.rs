use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub fn move_left(text: &Rope, char_offset: usize) -> usize {
    if char_offset == 0 {
        return 0;
    }
    let prefix = text.slice(..char_offset).to_string();
    prefix
        .grapheme_indices(true)
        .next_back()
        .map_or(0, |(byte, _)| prefix[..byte].chars().count())
}

pub fn move_right(text: &Rope, char_offset: usize) -> usize {
    if char_offset >= text.len_chars() {
        return text.len_chars();
    }
    let suffix = text.slice(char_offset..).to_string();
    suffix
        .graphemes(true)
        .next()
        .map_or(char_offset, |grapheme| {
            char_offset + grapheme.chars().count()
        })
}

pub fn move_up(
    text: &Rope,
    char_offset: usize,
    preferred: Option<usize>,
    tab_width: usize,
) -> (usize, usize) {
    move_vertical(text, char_offset, preferred, tab_width, -1)
}

pub fn move_down(
    text: &Rope,
    char_offset: usize,
    preferred: Option<usize>,
    tab_width: usize,
) -> (usize, usize) {
    move_vertical(text, char_offset, preferred, tab_width, 1)
}

pub fn display_column(line: &str, char_in_line: usize, tab_width: usize) -> usize {
    let mut column = 0;
    for grapheme in line
        .graphemes(true)
        .take_while_accumulating_chars(char_in_line)
    {
        column += if grapheme == "\t" {
            tab_width - (column % tab_width)
        } else {
            UnicodeWidthStr::width(grapheme)
        };
    }
    column
}

fn move_vertical(
    text: &Rope,
    char_offset: usize,
    preferred: Option<usize>,
    tab_width: usize,
    delta: isize,
) -> (usize, usize) {
    let line_idx = text.char_to_line(char_offset.min(text.len_chars()));
    let line_start = text.line_to_char(line_idx);
    let current = text.line(line_idx).to_string();
    let target_column =
        preferred.unwrap_or_else(|| display_column(&current, char_offset - line_start, tab_width));
    let target_line = line_idx
        .saturating_add_signed(delta)
        .min(text.len_lines().saturating_sub(1));
    let target = text.line(target_line).to_string();
    let char_in_target = char_offset_for_display_column(&target, target_column, tab_width);
    (
        text.line_to_char(target_line) + char_in_target,
        target_column,
    )
}

fn char_offset_for_display_column(line: &str, target: usize, tab_width: usize) -> usize {
    let mut display = 0;
    let mut chars = 0;
    for grapheme in line.trim_end_matches(['\r', '\n']).graphemes(true) {
        let width = if grapheme == "\t" {
            tab_width - (display % tab_width)
        } else {
            UnicodeWidthStr::width(grapheme)
        };
        if display + width > target {
            break;
        }
        display += width;
        chars += grapheme.chars().count();
    }
    chars
}

trait GraphemeTakeExt<'a>: Iterator<Item = &'a str> + Sized {
    fn take_while_accumulating_chars(self, limit: usize) -> GraphemeTake<Self> {
        GraphemeTake {
            inner: self,
            remaining: limit,
        }
    }
}

impl<'a, T: Iterator<Item = &'a str>> GraphemeTakeExt<'a> for T {}

struct GraphemeTake<I> {
    inner: I,
    remaining: usize,
}

impl<'a, I: Iterator<Item = &'a str>> Iterator for GraphemeTake<I> {
    type Item = &'a str;
    fn next(&mut self) -> Option<Self::Item> {
        let grapheme = self.inner.next()?;
        let count = grapheme.chars().count();
        if count > self.remaining {
            return None;
        }
        self.remaining -= count;
        Some(grapheme)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grapheme_movement_treats_emoji_as_one_stop() {
        let rope = Rope::from_str("a👩‍💻日");
        assert_eq!(move_right(&rope, 1), 4);
        assert_eq!(move_left(&rope, 4), 1);
    }

    #[test]
    fn display_width_handles_cjk_and_tabs() {
        assert_eq!(display_column("a\t日本", 4, 4), 8);
    }
}
