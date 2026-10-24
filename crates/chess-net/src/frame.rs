//! Length-prefixed frames: a two-byte big-endian length, then that many
//! bytes. A zero length is an abort, which the handshake sends to say the
//! codes did not match.

use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Largest frame: the most a two-byte length can say, and Noise's own limit.
pub const MAX_FRAME: usize = u16::MAX as usize;

pub enum Frame {
    Data(Vec<u8>),
    Abort,
}

/// A stream read in frames. Reading is cancel-safe: a `recv` dropped part
/// way (in a `select!`) keeps what it read for the next call.
pub struct Framed<S> {
    stream: S,
    buf: Vec<u8>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Framed<S> {
    pub fn new(stream: S) -> Framed<S> {
        Framed {
            stream,
            buf: Vec::new(),
        }
    }

    pub async fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        assert!(!bytes.is_empty() && bytes.len() <= MAX_FRAME);
        let mut frame = Vec::with_capacity(2 + bytes.len());
        frame.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
        frame.extend_from_slice(bytes);
        self.stream.write_all(&frame).await?;
        self.stream.flush().await
    }

    pub async fn send_abort(&mut self) -> io::Result<()> {
        self.stream.write_all(&[0, 0]).await?;
        self.stream.flush().await
    }

    /// The next frame, refusing any longer than `max` bytes.
    pub async fn recv(&mut self, max: usize) -> io::Result<Frame> {
        loop {
            if self.buf.len() >= 2 {
                let len = u16::from_be_bytes([self.buf[0], self.buf[1]]) as usize;
                if len == 0 {
                    self.buf.drain(..2);
                    return Ok(Frame::Abort);
                }
                if len > max {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too long"));
                }
                if self.buf.len() >= 2 + len {
                    let frame = self.buf[2..2 + len].to_vec();
                    self.buf.drain(..2 + len);
                    return Ok(Frame::Data(frame));
                }
            }
            let mut chunk = [0u8; 4096];
            let n = self.stream.read(&mut chunk).await?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "connection closed",
                ));
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn a_cancelled_recv_loses_nothing() {
        let (a, mut b) = tokio::io::duplex(64);
        let mut framed = Framed::new(a);
        b.write_all(&[0, 5, b'h', b'e']).await.unwrap();
        let cut = tokio::time::timeout(Duration::from_millis(20), framed.recv(100)).await;
        assert!(cut.is_err());
        b.write_all(b"llo").await.unwrap();
        let Frame::Data(bytes) = framed.recv(100).await.unwrap() else {
            panic!()
        };
        assert_eq!(bytes, b"hello");
    }

    #[tokio::test]
    async fn long_frames_and_aborts() {
        let (a, mut b) = tokio::io::duplex(64);
        let mut framed = Framed::new(a);
        b.write_all(&[0, 0, 0, 200]).await.unwrap();
        assert!(matches!(framed.recv(100).await.unwrap(), Frame::Abort));
        assert_eq!(
            framed.recv(100).await.err().unwrap().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
