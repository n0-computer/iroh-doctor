use std::{pin::Pin, task::Poll};

use tokio::{
    io::{self, AsyncWrite},
    sync::mpsc,
};

/// A writer that tries to send the total number of bytes written after each write
///
/// It sends the total number instead of just an increment so the update is self-contained
#[derive(Debug)]
pub(crate) struct ProgressWriter<W> {
    inner: TrackingWriter<W>,
    sender: mpsc::Sender<u64>,
}

impl<W> ProgressWriter<W> {
    /// Create a new `ProgressWriter` from an inner writer
    pub(crate) fn new(inner: W) -> (Self, mpsc::Receiver<u64>) {
        let (sender, receiver) = mpsc::channel(1);
        (
            Self {
                inner: TrackingWriter::new(inner),
                sender,
            },
            receiver,
        )
    }

    /// Return the inner writer
    #[allow(dead_code)]
    pub(crate) fn into_inner(self) -> W {
        self.inner.into_parts().0
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for ProgressWriter<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = &mut *self;
        let res = Pin::new(&mut this.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(_)) = res {
            this.sender.try_send(this.inner.bytes_written()).ok();
        }
        res
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// A writer that tracks the number of bytes written
#[derive(Debug)]
pub(crate) struct TrackingWriter<W> {
    inner: W,
    written: u64,
}

impl<W> TrackingWriter<W> {
    /// Wrap a writer in a tracking writer
    pub(crate) fn new(inner: W) -> Self {
        Self { inner, written: 0 }
    }

    /// Get the number of bytes written
    #[allow(dead_code)]
    pub(crate) fn bytes_written(&self) -> u64 {
        self.written
    }

    /// Get the inner writer
    pub(crate) fn into_parts(self) -> (W, u64) {
        (self.inner, self.written)
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for TrackingWriter<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = &mut *self;
        let res = Pin::new(&mut this.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(size)) = res {
            this.written = this.written.saturating_add(size as u64);
        }
        res
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
