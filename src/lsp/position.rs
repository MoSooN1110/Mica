use lsp_types::Position;

pub fn position_to_char_offset(text: &str, position: Position) -> usize {
    let target_line = position.line as usize;
    let mut line_start = 0usize;
    let mut current_line = 0usize;
    for (char_offset, character) in text.chars().enumerate() {
        if current_line == target_line {
            line_start = char_offset;
            break;
        }
        if character == '\n' {
            current_line += 1;
            line_start = char_offset + 1;
        }
    }
    if current_line < target_line {
        return text.chars().count();
    }
    let line: String = text
        .chars()
        .skip(line_start)
        .take_while(|character| *character != '\n')
        .collect();
    line_start + utf16_column_to_char(&line, position.character as usize)
}

/// Converts a UTF-16 code-unit column within a single line (no embedded
/// newline) to a Unicode scalar (char) offset within that line.
///
/// Shared by [`position_to_char_offset`] and by the definition-jump path,
/// which already has a single line's text loaded and only needs the
/// per-line half of the conversion (see `ColumnHint::Utf16` in
/// `crate::app::Effect::OpenFile`).
pub fn utf16_column_to_char(line: &str, target_utf16: usize) -> usize {
    let mut utf16 = 0usize;
    let mut offset = 0usize;
    for character in line.chars() {
        let width = character.len_utf16();
        if utf16 + width > target_utf16 {
            break;
        }
        utf16 += width;
        offset += 1;
        if utf16 == target_utf16 {
            break;
        }
    }
    offset
}

pub fn char_offset_to_position(text: &str, char_offset: usize) -> Position {
    let target = char_offset.min(text.chars().count());
    let mut line = 0u32;
    let mut utf16 = 0u32;
    for character in text.chars().take(target) {
        if character == '\n' {
            line = line.saturating_add(1);
            utf16 = 0;
        } else {
            utf16 = utf16.saturating_add(character.len_utf16() as u32);
        }
    }
    Position::new(line, utf16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_utf16_positions_at_unicode_boundaries() {
        let text = "a😀日\nβ";
        assert_eq!(char_offset_to_position(text, 0), Position::new(0, 0));
        assert_eq!(char_offset_to_position(text, 2), Position::new(0, 3));
        assert_eq!(char_offset_to_position(text, 3), Position::new(0, 4));
        assert_eq!(char_offset_to_position(text, 5), Position::new(1, 1));
        assert_eq!(position_to_char_offset(text, Position::new(0, 3)), 2);
        assert_eq!(position_to_char_offset(text, Position::new(0, 4)), 3);
        assert_eq!(position_to_char_offset(text, Position::new(1, 1)), 5);
    }

    #[test]
    fn clamps_mid_surrogate_and_out_of_range_positions() {
        let text = "😀x";
        assert_eq!(position_to_char_offset(text, Position::new(0, 1)), 0);
        assert_eq!(position_to_char_offset(text, Position::new(9, 0)), 2);
    }
}
