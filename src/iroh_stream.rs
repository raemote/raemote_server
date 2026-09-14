//! Adapts an iroh bidirectional stream to a single `AsyncRead + AsyncWrite`,
//! which is what hyper expects.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use iroh::endpoint::{RecvStream, SendStream};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// A bidirectional iroh stream presented as a single `AsyncRead + AsyncWrite`.
///
/// iroh hands out the two QUIC halves separately; hyper needs one duplex object.
pub struct IrohStream {
    send: SendStream,
    recv: RecvStream,
}

impl IrohStream {
    /// Wrap an iroh stream's two halves as one duplex I/O object.
    pub fn new(send: SendStream, recv: RecvStream) -> Self {
        Self { send, recv }
    }
}

impl AsyncRead for IrohStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.recv).poll_read(cx, buf)
    }
}

impl AsyncWrite for IrohStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        // UFCS disambiguates the tokio trait impl from `SendStream::poll_write`,
        // which is inherent and returns `WriteError` instead of `io::Error`.
        AsyncWrite::poll_write(Pin::new(&mut self.send), cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_flush(Pin::new(&mut self.send), cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_shutdown(Pin::new(&mut self.send), cx)
    }
}
