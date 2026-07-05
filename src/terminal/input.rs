use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub fn encode_key(event: KeyEvent, application_cursor: bool) -> Option<Vec<u8>> {
    let modifiers = event.modifiers;
    let code = match event.code {
        KeyCode::Char(character) if modifiers.contains(KeyModifiers::CONTROL) => {
            let lower = character.to_ascii_lowercase();
            if lower.is_ascii_lowercase() {
                vec![(lower as u8) - b'a' + 1]
            } else {
                match character {
                    ' ' | '@' => vec![0],
                    '[' => vec![0x1b],
                    '\\' => vec![0x1c],
                    ']' => vec![0x1d],
                    '^' => vec![0x1e],
                    '_' => vec![0x1f],
                    '?' => vec![0x7f],
                    _ => return None,
                }
            }
        }
        KeyCode::Char(character) => character.to_string().into_bytes(),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab if modifiers.contains(KeyModifiers::SHIFT) => b"\x1b[Z".to_vec(),
        KeyCode::Tab => vec![b'\t'],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => cursor_sequence(b'A', application_cursor),
        KeyCode::Down => cursor_sequence(b'B', application_cursor),
        KeyCode::Right => cursor_sequence(b'C', application_cursor),
        KeyCode::Left => cursor_sequence(b'D', application_cursor),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::F(number @ 1..=4) => vec![0x1b, b'O', b'P' + number - 1],
        KeyCode::F(number @ 5..=12) => {
            let value = [15, 17, 18, 19, 20, 21, 23, 24][usize::from(number - 5)];
            format!("\x1b[{value}~").into_bytes()
        }
        _ => return None,
    };
    if modifiers.contains(KeyModifiers::ALT)
        && !matches!(event.code, KeyCode::Char(_) if modifiers.contains(KeyModifiers::CONTROL))
    {
        let mut escaped = Vec::with_capacity(code.len() + 1);
        escaped.push(0x1b);
        escaped.extend(code);
        Some(escaped)
    } else {
        Some(code)
    }
}

pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        let mut bytes = b"\x1b[200~".to_vec();
        bytes.extend(text.replace("\x1b[201~", "").as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");
        bytes
    } else {
        text.as_bytes().to_vec()
    }
}

fn cursor_sequence(final_byte: u8, application_cursor: bool) -> Vec<u8> {
    vec![
        0x1b,
        if application_cursor { b'O' } else { b'[' },
        final_byte,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_control_alt_cursor_and_bracketed_paste() {
        assert_eq!(
            encode_key(
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                false
            ),
            Some(vec![3])
        );
        assert_eq!(
            encode_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), true),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            encode_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT), false),
            Some(b"\x1bx".to_vec())
        );
        assert_eq!(encode_paste("ok", true), b"\x1b[200~ok\x1b[201~");
    }
}
