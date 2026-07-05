use std::io::{self, BufRead, Write};

use serde_json::Value;
use thiserror::Error;

const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum LspFrameError {
    #[error("LSP I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("LSP message is missing Content-Length")]
    MissingContentLength,
    #[error("invalid LSP Content-Length: {0}")]
    InvalidContentLength(String),
    #[error("LSP message exceeds {MAX_MESSAGE_BYTES} bytes")]
    MessageTooLarge,
    #[error("invalid LSP JSON: {0}")]
    Json(#[from] serde_json::Error),
}

pub fn read_message(reader: &mut impl BufRead) -> Result<Option<Value>, LspFrameError> {
    let mut content_length = None;
    let mut saw_header = false;
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            return if saw_header {
                Err(LspFrameError::MissingContentLength)
            } else {
                Ok(None)
            };
        }
        saw_header = true;
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            let value = value.trim();
            content_length = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| LspFrameError::InvalidContentLength(value.to_owned()))?,
            );
        }
    }
    let length = content_length.ok_or(LspFrameError::MissingContentLength)?;
    if length > MAX_MESSAGE_BYTES {
        return Err(LspFrameError::MessageTooLarge);
    }
    let mut body = vec![0u8; length];
    std::io::Read::read_exact(reader, &mut body)?;
    Ok(Some(serde_json::from_slice(&body)?))
}

pub fn write_message(writer: &mut impl Write, message: &Value) -> Result<(), LspFrameError> {
    let body = serde_json::to_vec(message)?;
    if body.len() > MAX_MESSAGE_BYTES {
        return Err(LspFrameError::MessageTooLarge);
    }
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{BufReader, Cursor};

    #[test]
    fn frames_multiple_messages_and_accepts_extra_headers() {
        let mut bytes = Vec::new();
        write_message(&mut bytes, &json!({"id": 1})).unwrap();
        bytes.extend_from_slice(b"Content-Type: application/vscode-jsonrpc; charset=utf-8\r\nContent-Length: 8\r\n\r\n{\"id\":2}");
        let mut reader = BufReader::new(Cursor::new(bytes));
        assert_eq!(read_message(&mut reader).unwrap(), Some(json!({"id": 1})));
        assert_eq!(read_message(&mut reader).unwrap(), Some(json!({"id": 2})));
        assert_eq!(read_message(&mut reader).unwrap(), None);
    }

    #[test]
    fn truncated_message_is_an_error() {
        let mut reader = BufReader::new(Cursor::new(b"Content-Length: 10\r\n\r\n{}"));
        assert!(matches!(
            read_message(&mut reader),
            Err(LspFrameError::Io(_))
        ));
    }
}
