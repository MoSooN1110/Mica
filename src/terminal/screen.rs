use std::collections::VecDeque;

use unicode_width::UnicodeWidthChar;
use vte::{Params, Parser, Perform};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TerminalColor {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CellStyle {
    pub foreground: TerminalColor,
    pub background: TerminalColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalCell {
    pub character: char,
    pub style: CellStyle,
    pub wide_continuation: bool,
}

impl Default for TerminalCell {
    fn default() -> Self {
        Self {
            character: ' ',
            style: CellStyle::default(),
            wide_continuation: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSnapshot {
    pub lines: Vec<Vec<TerminalCell>>,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub cursor_visible: bool,
    pub application_cursor: bool,
    pub bracketed_paste: bool,
    pub alternate_screen: bool,
    pub title: Option<String>,
}

pub struct TerminalEmulator {
    parser: Parser,
    screen: Screen,
}

impl std::fmt::Debug for TerminalEmulator {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalEmulator")
            .field("rows", &self.screen.rows)
            .field("cols", &self.screen.cols)
            .field("scrollback", &self.screen.scrollback.len())
            .finish()
    }
}

impl TerminalEmulator {
    pub fn new(rows: usize, cols: usize, scrollback_limit: usize) -> Self {
        Self {
            parser: Parser::new(),
            screen: Screen::new(rows, cols, scrollback_limit),
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.parser.advance(&mut self.screen, *byte);
        }
    }

    pub fn resize(&mut self, rows: usize, cols: usize) {
        self.screen.resize(rows, cols);
    }

    pub fn snapshot(&self, scroll_offset: usize) -> TerminalSnapshot {
        self.screen.snapshot(scroll_offset)
    }

    pub fn scrollback_len(&self) -> usize {
        self.screen.scrollback.len()
    }

    pub fn selected_text(
        &self,
        scroll_offset: usize,
        start: (usize, usize),
        end: (usize, usize),
    ) -> String {
        let snapshot = self.snapshot(scroll_offset);
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        let mut output = String::new();
        for row in start.0..=end.0.min(snapshot.lines.len().saturating_sub(1)) {
            let line = &snapshot.lines[row];
            let from = if row == start.0 { start.1 } else { 0 };
            let to = if row == end.0 {
                end.1.saturating_add(1).min(line.len())
            } else {
                line.len()
            };
            let text = line[from.min(line.len())..to]
                .iter()
                .filter(|cell| !cell.wide_continuation)
                .map(|cell| cell.character)
                .collect::<String>();
            output.push_str(text.trim_end());
            if row < end.0 {
                output.push('\n');
            }
        }
        output
    }
}

struct Screen {
    rows: usize,
    cols: usize,
    grid: Vec<Vec<TerminalCell>>,
    primary: Option<PrimaryScreen>,
    scrollback: VecDeque<Vec<TerminalCell>>,
    scrollback_limit: usize,
    cursor_row: usize,
    cursor_col: usize,
    saved_cursor: (usize, usize),
    scroll_top: usize,
    scroll_bottom: usize,
    style: CellStyle,
    cursor_visible: bool,
    application_cursor: bool,
    bracketed_paste: bool,
    alternate_screen: bool,
    title: Option<String>,
}

struct PrimaryScreen {
    grid: Vec<Vec<TerminalCell>>,
    cursor: (usize, usize),
    saved_cursor: (usize, usize),
}

impl Screen {
    fn new(rows: usize, cols: usize, scrollback_limit: usize) -> Self {
        let rows = rows.max(1);
        let cols = cols.max(2);
        Self {
            rows,
            cols,
            grid: blank_grid(rows, cols),
            primary: None,
            scrollback: VecDeque::new(),
            scrollback_limit,
            cursor_row: 0,
            cursor_col: 0,
            saved_cursor: (0, 0),
            scroll_top: 0,
            scroll_bottom: rows - 1,
            style: CellStyle::default(),
            cursor_visible: true,
            application_cursor: false,
            bracketed_paste: false,
            alternate_screen: false,
            title: None,
        }
    }

    fn snapshot(&self, scroll_offset: usize) -> TerminalSnapshot {
        let lines = if self.alternate_screen || scroll_offset == 0 {
            self.grid.clone()
        } else {
            let mut history = self.scrollback.iter().cloned().collect::<Vec<_>>();
            history.extend(self.grid.iter().cloned());
            let end = history
                .len()
                .saturating_sub(scroll_offset.min(self.scrollback.len()));
            let start = end.saturating_sub(self.rows);
            history[start..end].to_vec()
        };
        TerminalSnapshot {
            lines,
            cursor_row: self.cursor_row,
            cursor_col: self.cursor_col,
            cursor_visible: self.cursor_visible && scroll_offset == 0,
            application_cursor: self.application_cursor,
            bracketed_paste: self.bracketed_paste,
            alternate_screen: self.alternate_screen,
            title: self.title.clone(),
        }
    }

    fn resize(&mut self, rows: usize, cols: usize) {
        let rows = rows.max(1);
        let cols = cols.max(2);
        for line in &mut self.grid {
            line.resize(cols, TerminalCell::default());
        }
        if rows > self.grid.len() {
            self.grid.extend(blank_grid(rows - self.grid.len(), cols));
        } else {
            while self.grid.len() > rows {
                let line = self.grid.remove(0);
                if !self.alternate_screen {
                    self.push_scrollback(line);
                }
            }
        }
        self.rows = rows;
        self.cols = cols;
        self.cursor_row = self.cursor_row.min(rows - 1);
        self.cursor_col = self.cursor_col.min(cols - 1);
        self.scroll_top = 0;
        self.scroll_bottom = rows - 1;
    }

    fn put_char(&mut self, character: char) {
        let width = UnicodeWidthChar::width(character).unwrap_or(0);
        if width == 0 {
            return;
        }
        if self.cursor_col + width > self.cols {
            self.cursor_col = 0;
            self.line_feed();
        }
        self.grid[self.cursor_row][self.cursor_col] = TerminalCell {
            character,
            style: self.style,
            wide_continuation: false,
        };
        if width == 2 && self.cursor_col + 1 < self.cols {
            self.grid[self.cursor_row][self.cursor_col + 1] = TerminalCell {
                character: ' ',
                style: self.style,
                wide_continuation: true,
            };
        }
        self.cursor_col += width;
        if self.cursor_col >= self.cols {
            self.cursor_col = 0;
            self.line_feed();
        }
    }

    fn line_feed(&mut self) {
        if self.cursor_row == self.scroll_bottom {
            self.scroll_up(1);
        } else {
            self.cursor_row = (self.cursor_row + 1).min(self.rows - 1);
        }
    }

    fn reverse_index(&mut self) {
        if self.cursor_row == self.scroll_top {
            self.scroll_down(1);
        } else {
            self.cursor_row = self.cursor_row.saturating_sub(1);
        }
    }

    fn scroll_up(&mut self, count: usize) {
        for _ in 0..count.min(self.scroll_bottom - self.scroll_top + 1) {
            let removed = self.grid.remove(self.scroll_top);
            self.grid
                .insert(self.scroll_bottom, vec![TerminalCell::default(); self.cols]);
            if self.scroll_top == 0 && !self.alternate_screen {
                self.push_scrollback(removed);
            }
        }
    }

    fn scroll_down(&mut self, count: usize) {
        for _ in 0..count.min(self.scroll_bottom - self.scroll_top + 1) {
            self.grid.remove(self.scroll_bottom);
            self.grid
                .insert(self.scroll_top, vec![TerminalCell::default(); self.cols]);
        }
    }

    fn push_scrollback(&mut self, line: Vec<TerminalCell>) {
        if self.scrollback_limit == 0 {
            return;
        }
        self.scrollback.push_back(line);
        while self.scrollback.len() > self.scrollback_limit {
            self.scrollback.pop_front();
        }
    }

    fn erase_display(&mut self, mode: u16) {
        match mode {
            0 => {
                self.erase_line(0);
                for row in self.cursor_row + 1..self.rows {
                    self.grid[row].fill(TerminalCell::default());
                }
            }
            1 => {
                self.erase_line(1);
                for row in 0..self.cursor_row {
                    self.grid[row].fill(TerminalCell::default());
                }
            }
            2 | 3 => {
                for line in &mut self.grid {
                    line.fill(TerminalCell::default());
                }
                if mode == 3 {
                    self.scrollback.clear();
                }
            }
            _ => {}
        }
    }

    fn erase_line(&mut self, mode: u16) {
        match mode {
            0 => self.grid[self.cursor_row][self.cursor_col..].fill(TerminalCell::default()),
            1 => self.grid[self.cursor_row][..=self.cursor_col].fill(TerminalCell::default()),
            2 => self.grid[self.cursor_row].fill(TerminalCell::default()),
            _ => {}
        }
    }

    fn set_private_mode(&mut self, mode: u16, enabled: bool) {
        match mode {
            1 => self.application_cursor = enabled,
            25 => self.cursor_visible = enabled,
            1047 | 1049 => self.set_alternate_screen(enabled),
            2004 => self.bracketed_paste = enabled,
            _ => {}
        }
    }

    fn set_alternate_screen(&mut self, enabled: bool) {
        if enabled && !self.alternate_screen {
            self.primary = Some(PrimaryScreen {
                grid: std::mem::replace(&mut self.grid, blank_grid(self.rows, self.cols)),
                cursor: (self.cursor_row, self.cursor_col),
                saved_cursor: self.saved_cursor,
            });
            self.cursor_row = 0;
            self.cursor_col = 0;
            self.alternate_screen = true;
        } else if !enabled
            && self.alternate_screen
            && let Some(primary) = self.primary.take()
        {
            self.grid = primary.grid;
            (self.cursor_row, self.cursor_col) = primary.cursor;
            self.saved_cursor = primary.saved_cursor;
            self.alternate_screen = false;
        }
    }

    fn sgr(&mut self, params: &Params) {
        let values = params
            .iter()
            .map(|parameter| parameter.first().copied().unwrap_or(0))
            .collect::<Vec<_>>();
        let values = if values.is_empty() { vec![0] } else { values };
        let mut index = 0usize;
        while index < values.len() {
            match values[index] {
                0 => self.style = CellStyle::default(),
                1 => self.style.bold = true,
                3 => self.style.italic = true,
                4 => self.style.underline = true,
                7 => self.style.inverse = true,
                22 => self.style.bold = false,
                23 => self.style.italic = false,
                24 => self.style.underline = false,
                27 => self.style.inverse = false,
                30..=37 => {
                    self.style.foreground = TerminalColor::Indexed((values[index] - 30) as u8)
                }
                38 => {
                    index += parse_extended_color(&values[index + 1..], &mut self.style.foreground)
                }
                39 => self.style.foreground = TerminalColor::Default,
                40..=47 => {
                    self.style.background = TerminalColor::Indexed((values[index] - 40) as u8)
                }
                48 => {
                    index += parse_extended_color(&values[index + 1..], &mut self.style.background)
                }
                49 => self.style.background = TerminalColor::Default,
                90..=97 => {
                    self.style.foreground = TerminalColor::Indexed((values[index] - 90 + 8) as u8)
                }
                100..=107 => {
                    self.style.background = TerminalColor::Indexed((values[index] - 100 + 8) as u8)
                }
                _ => {}
            }
            index += 1;
        }
    }

    fn reset(&mut self) {
        let limit = self.scrollback_limit;
        *self = Self::new(self.rows, self.cols, limit);
    }
}

impl Perform for Screen {
    fn print(&mut self, character: char) {
        self.put_char(character);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x08 => self.cursor_col = self.cursor_col.saturating_sub(1),
            b'\t' => self.cursor_col = ((self.cursor_col / 8 + 1) * 8).min(self.cols - 1),
            b'\n' | 0x0b | 0x0c => self.line_feed(),
            b'\r' => self.cursor_col = 0,
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        if ignore {
            return;
        }
        let values = params
            .iter()
            .map(|parameter| parameter.first().copied().unwrap_or(0))
            .collect::<Vec<_>>();
        let value = |index: usize, default: usize| {
            usize::from(values.get(index).copied().unwrap_or(default as u16)).max(default)
        };
        let private = intermediates.contains(&b'?');
        match action {
            'A' => self.cursor_row = self.cursor_row.saturating_sub(value(0, 1)),
            'B' => self.cursor_row = (self.cursor_row + value(0, 1)).min(self.rows - 1),
            'C' => self.cursor_col = (self.cursor_col + value(0, 1)).min(self.cols - 1),
            'D' => self.cursor_col = self.cursor_col.saturating_sub(value(0, 1)),
            'E' => {
                self.cursor_row = (self.cursor_row + value(0, 1)).min(self.rows - 1);
                self.cursor_col = 0;
            }
            'F' => {
                self.cursor_row = self.cursor_row.saturating_sub(value(0, 1));
                self.cursor_col = 0;
            }
            'G' => self.cursor_col = value(0, 1).saturating_sub(1).min(self.cols - 1),
            'H' | 'f' => {
                self.cursor_row = value(0, 1).saturating_sub(1).min(self.rows - 1);
                self.cursor_col = value(1, 1).saturating_sub(1).min(self.cols - 1);
            }
            'J' => self.erase_display(values.first().copied().unwrap_or(0)),
            'K' => self.erase_line(values.first().copied().unwrap_or(0)),
            'L' => {
                for _ in 0..value(0, 1).min(self.scroll_bottom - self.cursor_row + 1) {
                    self.grid.remove(self.scroll_bottom);
                    self.grid
                        .insert(self.cursor_row, vec![TerminalCell::default(); self.cols]);
                }
            }
            'M' => {
                for _ in 0..value(0, 1).min(self.scroll_bottom - self.cursor_row + 1) {
                    self.grid.remove(self.cursor_row);
                    self.grid
                        .insert(self.scroll_bottom, vec![TerminalCell::default(); self.cols]);
                }
            }
            '@' => {
                let count = value(0, 1).min(self.cols - self.cursor_col);
                let line = &mut self.grid[self.cursor_row];
                line.splice(
                    self.cursor_col..self.cursor_col,
                    std::iter::repeat_n(TerminalCell::default(), count),
                );
                line.truncate(self.cols);
            }
            'P' => {
                let count = value(0, 1).min(self.cols - self.cursor_col);
                let line = &mut self.grid[self.cursor_row];
                line.drain(self.cursor_col..self.cursor_col + count);
                line.resize(self.cols, TerminalCell::default());
            }
            'S' => self.scroll_up(value(0, 1)),
            'T' => self.scroll_down(value(0, 1)),
            'm' => self.sgr(params),
            'r' => {
                self.scroll_top = value(0, 1).saturating_sub(1).min(self.rows - 1);
                self.scroll_bottom = value(1, self.rows).saturating_sub(1).min(self.rows - 1);
                if self.scroll_top >= self.scroll_bottom {
                    self.scroll_top = 0;
                    self.scroll_bottom = self.rows - 1;
                }
                self.cursor_row = 0;
                self.cursor_col = 0;
            }
            's' => self.saved_cursor = (self.cursor_row, self.cursor_col),
            'u' => (self.cursor_row, self.cursor_col) = self.saved_cursor,
            'h' | 'l' if private => {
                for mode in values {
                    self.set_private_mode(mode, action == 'h');
                }
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], ignore: bool, byte: u8) {
        if ignore {
            return;
        }
        match byte {
            b'7' => self.saved_cursor = (self.cursor_row, self.cursor_col),
            b'8' => (self.cursor_row, self.cursor_col) = self.saved_cursor,
            b'D' => self.line_feed(),
            b'M' => self.reverse_index(),
            b'c' => self.reset(),
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if matches!(params.first().copied(), Some(b"0" | b"2"))
            && let Some(title) = params.get(1)
        {
            self.title = Some(String::from_utf8_lossy(title).into_owned());
        }
    }
}

fn blank_grid(rows: usize, cols: usize) -> Vec<Vec<TerminalCell>> {
    vec![vec![TerminalCell::default(); cols]; rows]
}

fn parse_extended_color(values: &[u16], color: &mut TerminalColor) -> usize {
    match values {
        [5, index, ..] => {
            *color = TerminalColor::Indexed((*index).min(255) as u8);
            2
        }
        [2, red, green, blue, ..] => {
            *color = TerminalColor::Rgb(
                (*red).min(255) as u8,
                (*green).min(255) as u8,
                (*blue).min(255) as u8,
            );
            4
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(snapshot: &TerminalSnapshot, row: usize) -> String {
        snapshot.lines[row]
            .iter()
            .filter(|cell| !cell.wide_continuation)
            .map(|cell| cell.character)
            .collect::<String>()
            .trim_end()
            .to_owned()
    }

    #[test]
    fn parses_cursor_color_unicode_and_scrollback() {
        let mut terminal = TerminalEmulator::new(2, 8, 10);
        terminal.feed(b"one\r\ntwo\r\n\x1b[38;2;1;2;3mX");
        terminal.feed("日".as_bytes());
        let current = terminal.snapshot(0);
        assert_eq!(text(&current, 0), "two");
        assert_eq!(text(&current, 1), "X日");
        assert_eq!(
            current.lines[1][0].style.foreground,
            TerminalColor::Rgb(1, 2, 3)
        );
        assert_eq!(terminal.scrollback_len(), 1);
        assert_eq!(text(&terminal.snapshot(1), 0), "one");
    }

    #[test]
    fn tracks_private_modes_and_restores_primary_screen() {
        let mut terminal = TerminalEmulator::new(3, 10, 10);
        terminal.feed(b"primary\x1b[?1h\x1b[?2004h\x1b[?1049halt");
        let alternate = terminal.snapshot(0);
        assert!(alternate.application_cursor);
        assert!(alternate.bracketed_paste);
        assert!(alternate.alternate_screen);
        assert_eq!(text(&alternate, 0), "alt");
        terminal.feed(b"\x1b[?1049l");
        assert_eq!(text(&terminal.snapshot(0), 0), "primary");
    }

    #[test]
    fn resizes_without_losing_visible_content() {
        let mut terminal = TerminalEmulator::new(2, 4, 10);
        terminal.feed(b"abc");
        terminal.resize(3, 8);
        let snapshot = terminal.snapshot(0);
        assert_eq!(snapshot.lines.len(), 3);
        assert_eq!(snapshot.lines[0].len(), 8);
        assert_eq!(text(&snapshot, 0), "abc");
    }

    #[test]
    fn extracts_multiline_selection_without_padding() {
        let mut terminal = TerminalEmulator::new(2, 8, 10);
        terminal.feed(b"hello\r\nworld");
        assert_eq!(terminal.selected_text(0, (0, 1), (1, 2)), "ello\nwor");
    }
}
