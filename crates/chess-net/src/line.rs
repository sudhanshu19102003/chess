//! Bounded text lines, the only framing the spike uses.

use std::io;

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Longest line either side accepts, newline excluded.
pub const MAX_LINE: usize = 256;

/// Reads one line. `None` at a clean end of stream; an error for a line that
/// is too long, cut off, or not UTF-8.
pub async fn read<R: AsyncBufRead + Unpin>(r: &mut R) -> io::Result<Option<String>> {
    let mut buf = Vec::new();
    let n = r
        .take(MAX_LINE as u64 + 1)
        .read_until(b'\n', &mut buf)
        .await?;
    if n == 0 {
        return Ok(None);
    }
    if buf.pop() != Some(b'\n') {
        return Err(invalid("line too long or cut off"));
    }
    String::from_utf8(buf)
        .map(Some)
        .map_err(|_| invalid("line is not UTF-8"))
}

/// Reads one line and fails at the end of the stream.
pub async fn expect<R: AsyncBufRead + Unpin>(r: &mut R) -> io::Result<String> {
    read(r)
        .await?
        .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed"))
}

pub async fn write<W: AsyncWrite + Unpin>(w: &mut W, line: &str) -> io::Result<()> {
    debug_assert!(line.len() <= MAX_LINE && !line.contains('\n'));
    w.write_all(format!("{line}\n").as_bytes()).await
}

pub fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}
