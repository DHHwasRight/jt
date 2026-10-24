//! Where rendered JSON goes.

use std::io;

use crate::pool;

/// A destination for rendered JSON.
///
/// Every implementation appends to a byte buffer it owns or borrows. That
/// keeps the hot path free of virtual dispatch and error checks: writing a
/// static fragment is a `memcpy` and nothing else. A streaming destination
/// hands the buffer to an [`io::Write`] at [`Out::checkpoint`], which
/// generated code calls after every array element and loop iteration, so
/// memory stays bounded by the size of one element.
///
/// The buffer must hold valid UTF-8, which is why [`Out::buf_mut`] is
/// `unsafe`. Code outside this crate writes through [`Out::raw`] and the
/// [`ToJson`](crate::ToJson) implementations instead.
pub trait Out {
    /// The buffer being written to.
    ///
    /// # Safety
    ///
    /// The caller must leave the buffer holding valid UTF-8.
    #[doc(hidden)]
    unsafe fn buf_mut(&mut self) -> &mut Vec<u8>;

    /// Lets a streaming destination flush. A no-op for in-memory ones.
    #[inline(always)]
    fn checkpoint(&mut self) {}

    /// Appends text verbatim. The caller asserts it is valid JSON in context.
    #[inline(always)]
    fn raw(&mut self, s: &str) {
        // SAFETY: `s` is UTF-8.
        unsafe { self.buf_mut() }.extend_from_slice(s.as_bytes());
    }
}

impl Out for Vec<u8> {
    #[inline(always)]
    unsafe fn buf_mut(&mut self) -> &mut Vec<u8> {
        self
    }
}

impl Out for String {
    #[inline(always)]
    unsafe fn buf_mut(&mut self) -> &mut Vec<u8> {
        // SAFETY: forwarded to the caller of `buf_mut`.
        unsafe { self.as_mut_vec() }
    }
}

impl Out for crate::PooledBuf {
    #[inline(always)]
    unsafe fn buf_mut(&mut self) -> &mut Vec<u8> {
        self.vec_mut()
    }
}

impl<T: Out + ?Sized> Out for &mut T {
    #[inline(always)]
    unsafe fn buf_mut(&mut self) -> &mut Vec<u8> {
        // SAFETY: forwarded to the caller of `buf_mut`.
        unsafe { (**self).buf_mut() }
    }

    #[inline(always)]
    fn checkpoint(&mut self) {
        (**self).checkpoint()
    }
}

/// Default bytes buffered before a [`Stream`] writes to its sink.
pub const DEFAULT_CHUNK: usize = 8 * 1024;

/// Streams rendered JSON into an [`io::Write`] in chunks.
///
/// The chunk buffer comes from the [buffer pool](crate::pool) and goes back
/// when the stream is dropped. Write errors are sticky: the first one is kept,
/// later output is discarded, and [`Stream::finish`] reports it. That keeps
/// every generated write call infallible.
pub struct Stream<W: io::Write> {
    buf: Vec<u8>,
    sink: W,
    chunk: usize,
    error: Option<io::Error>,
}

impl<W: io::Write> Stream<W> {
    /// Streams into `sink` with the default chunk size.
    #[inline]
    pub fn new(sink: W) -> Self {
        Stream::with_chunk(sink, DEFAULT_CHUNK)
    }

    /// Streams into `sink`, writing once `chunk` bytes are buffered.
    #[inline]
    pub fn with_chunk(sink: W, chunk: usize) -> Self {
        Stream {
            buf: pool::take(),
            sink,
            chunk: chunk.max(1),
            error: None,
        }
    }

    #[inline(never)]
    fn drain(&mut self) {
        if self.error.is_none() && !self.buf.is_empty() {
            if let Err(e) = self.sink.write_all(&self.buf) {
                self.error = Some(e);
            }
        }
        self.buf.clear();
    }

    /// Writes what is buffered, flushes the sink and reports the first error.
    pub fn finish(mut self) -> io::Result<()> {
        self.drain();
        if let Some(e) = self.error.take() {
            return Err(e);
        }
        self.sink.flush()
    }
}

impl<W: io::Write> Out for Stream<W> {
    #[inline(always)]
    unsafe fn buf_mut(&mut self) -> &mut Vec<u8> {
        &mut self.buf
    }

    #[inline(always)]
    fn checkpoint(&mut self) {
        if self.buf.len() >= self.chunk {
            self.drain();
        }
    }
}

impl<W: io::Write> Drop for Stream<W> {
    fn drop(&mut self) {
        pool::give(std::mem::take(&mut self.buf));
    }
}
