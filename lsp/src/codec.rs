//! JSON-RPC framing codec for LSP communication.
//!
//! LSP uses `Content-Length: N\r\n\r\n{json}` framing over stdin/stdout.
//! This module provides [`FrameReader`] and [`FrameWriter`] for async
//! reading and writing of framed JSON-RPC messages.

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

/// Maximum frame size (4 MiB) to prevent unbounded memory allocation.
const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

/// Reads JSON-RPC frames from an async reader.
///
/// Parses `Content-Length` headers and reads exactly that many bytes,
/// then deserializes the body as JSON.
pub struct FrameReader<R> {
    reader: BufReader<R>,
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader: BufReader::new(reader),
        }
    }

    /// Read the next JSON-RPC frame.
    ///
    /// Returns `Ok(None)` on EOF (clean shutdown).
    /// Returns `Err` on malformed headers or oversized frames.
    pub async fn read_frame(&mut self) -> Result<Option<serde_json::Value>> {
        let content_length = match self.read_headers().await? {
            Some(len) => len,
            None => return Ok(None), // EOF
        };

        if content_length > MAX_FRAME_BYTES {
            bail!("Content-Length {content_length} exceeds maximum {MAX_FRAME_BYTES}");
        }

        let mut body = vec![0u8; content_length];
        self.reader
            .read_exact(&mut body)
            .await
            .context("reading frame body")?;

        let value = serde_json::from_slice(&body).context("parsing JSON-RPC frame")?;
        Ok(Some(value))
    }

    /// Parse headers until the empty line separator.
    ///
    /// Returns the `Content-Length` value, or `None` on EOF.
    async fn read_headers(&mut self) -> Result<Option<usize>> {
        let mut content_length: Option<usize> = None;
        let mut line = String::new();
        let mut saw_any_header_bytes = false;

        loop {
            line.clear();
            let bytes_read = self
                .reader
                .read_line(&mut line)
                .await
                .context("reading header line")?;

            if bytes_read == 0 {
                // EOF — only valid if we haven't started reading headers at all.
                //
                // Note: `content_length == None` doesn't imply "no headers read"
                // (e.g. EOF after reading only Content-Type should be an error).
                if !saw_any_header_bytes {
                    return Ok(None);
                }
                bail!("unexpected EOF while reading headers");
            }
            saw_any_header_bytes = true;

            let trimmed = line.trim();
            if trimmed.is_empty() {
                // Empty line = end of headers
                break;
            }

            // LSP spec uses "Content-Length" but parse case-insensitively for robustness.
            if let Some(colon_pos) = trimmed.find(':') {
                let key = &trimmed[..colon_pos];
                if key.eq_ignore_ascii_case("Content-Length") {
                    let len: usize = trimmed[colon_pos + 1..]
                        .trim()
                        .parse()
                        .context("invalid Content-Length value")?;
                    content_length = Some(len);
                }
            }
            // Ignore other headers (e.g. Content-Type)
        }

        match content_length {
            Some(len) => Ok(Some(len)),
            None => bail!("missing Content-Length header"),
        }
    }
}

/// Writes JSON-RPC frames to an async writer.
///
/// Serializes JSON and prepends the `Content-Length` header.
pub struct FrameWriter<W> {
    writer: W,
}

impl<W: AsyncWrite + Unpin> FrameWriter<W> {
    pub fn new(writer: W) -> Self {
        Self { writer }
    }

    /// Write a JSON-RPC frame with `Content-Length` header.
    pub async fn write_frame(&mut self, msg: &serde_json::Value) -> Result<()> {
        let body = serde_json::to_string(msg).context("serializing JSON-RPC frame")?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());

        self.writer
            .write_all(header.as_bytes())
            .await
            .context("writing frame header")?;
        self.writer
            .write_all(body.as_bytes())
            .await
            .context("writing frame body")?;
        self.writer.flush().await.context("flushing frame")?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_roundtrip() {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": { "uri": "file:///test.rs" }
        });

        // Write
        let mut buf = Vec::new();
        let mut writer = FrameWriter::new(&mut buf);
        writer.write_frame(&msg).await.unwrap();

        // Read back
        let mut reader = FrameReader::new(buf.as_slice());
        let result = reader.read_frame().await.unwrap().unwrap();
        assert_eq!(result, msg);
    }

    #[tokio::test]
    async fn test_multiple_frames() {
        let msg1 = serde_json::json!({"jsonrpc": "2.0", "id": 1});
        let msg2 = serde_json::json!({"jsonrpc": "2.0", "id": 2});

        let mut buf = Vec::new();
        let mut writer = FrameWriter::new(&mut buf);
        writer.write_frame(&msg1).await.unwrap();
        writer.write_frame(&msg2).await.unwrap();

        let mut reader = FrameReader::new(buf.as_slice());
        assert_eq!(reader.read_frame().await.unwrap().unwrap(), msg1);
        assert_eq!(reader.read_frame().await.unwrap().unwrap(), msg2);
    }

    #[tokio::test]
    async fn test_eof_returns_none() {
        let buf: &[u8] = b"";
        let mut reader = FrameReader::new(buf);
        assert!(reader.read_frame().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_missing_content_length() {
        let buf: &[u8] = b"Content-Type: application/json\r\n\r\n{}";
        let mut reader = FrameReader::new(buf);
        assert!(reader.read_frame().await.is_err());
    }

    #[tokio::test]
    async fn test_eof_mid_headers_is_error() {
        // EOF after reading a header line must not be treated as a clean shutdown.
        let buf: &[u8] = b"Content-Type: application/json\r\n";
        let mut reader = FrameReader::new(buf);
        assert!(reader.read_frame().await.is_err());
    }

    #[tokio::test]
    async fn test_oversized_frame_rejected() {
        let header = format!("Content-Length: {}\r\n\r\n", MAX_FRAME_BYTES + 1);
        let buf = header.as_bytes();
        let mut reader = FrameReader::new(buf);
        assert!(reader.read_frame().await.is_err());
    }

    #[tokio::test]
    async fn test_case_insensitive_content_length() {
        let body = r#"{"jsonrpc":"2.0","id":1}"#;
        let frame = format!("content-length: {}\r\n\r\n{body}", body.len());

        let mut reader = FrameReader::new(frame.as_bytes());
        let result = reader.read_frame().await.unwrap().unwrap();
        assert_eq!(result["id"], 1);
    }

    #[tokio::test]
    async fn test_ignores_extra_headers() {
        let body = r#"{"jsonrpc":"2.0","id":1}"#;
        let frame = format!(
            "Content-Type: application/vscode-jsonrpc; charset=utf-8\r\nContent-Length: {}\r\n\r\n{body}",
            body.len(),
        );

        let mut reader = FrameReader::new(frame.as_bytes());
        let result = reader.read_frame().await.unwrap().unwrap();
        assert_eq!(result["id"], 1);
    }

    #[tokio::test]
    async fn test_eof_mid_body() {
        // Content-Length says 100, but only 5 bytes follow
        let buf: &[u8] = b"Content-Length: 100\r\n\r\nhello";
        let mut reader = FrameReader::new(buf);
        assert!(reader.read_frame().await.is_err());
    }

    #[tokio::test]
    async fn test_invalid_json_body() {
        let body = b"not valid json!!!";
        let frame = format!("Content-Length: {}\r\n\r\n", body.len());
        let mut buf = frame.into_bytes();
        buf.extend_from_slice(body);

        let mut reader = FrameReader::new(buf.as_slice());
        assert!(reader.read_frame().await.is_err());
    }

    #[tokio::test]
    async fn test_multibyte_utf8_content_length_counts_bytes() {
        // Content-Length counts bytes, not characters.
        // "é" is 2 bytes in UTF-8, so {"k":"é"} is 10 bytes.
        let body = r#"{"k":"é"}"#;
        assert_eq!(body.len(), 10); // 2-byte char
        let frame = format!("Content-Length: {}\r\n\r\n{body}", body.len());

        let mut reader = FrameReader::new(frame.as_bytes());
        let result = reader.read_frame().await.unwrap().unwrap();
        assert_eq!(result["k"], "é");
    }

    #[tokio::test]
    async fn test_eof_mid_headers() {
        // Start a Content-Length header but EOF before the empty separator line
        let buf: &[u8] = b"Content-Length: 10\r\n";
        let mut reader = FrameReader::new(buf);
        assert!(reader.read_frame().await.is_err());
    }

    #[tokio::test]
    async fn test_invalid_content_length_value() {
        let buf: &[u8] = b"Content-Length: not_a_number\r\n\r\n";
        let mut reader = FrameReader::new(buf);
        assert!(reader.read_frame().await.is_err());
    }

    #[tokio::test]
    async fn test_write_content_length_is_byte_count() {
        let msg = serde_json::json!({"k": "é"});
        let mut buf = Vec::new();
        let mut writer = FrameWriter::new(&mut buf);
        writer.write_frame(&msg).await.unwrap();

        let output = String::from_utf8(buf).unwrap();
        // The serialized JSON body
        let body = serde_json::to_string(&msg).unwrap();
        // Header should contain the byte length
        assert!(output.starts_with(&format!("Content-Length: {}\r\n\r\n", body.len())));
    }
}
