//! A per-thread pool of byte buffers.
//!
//! Rendering into a fresh `Vec` costs an allocation, plus a few reallocations
//! while it grows. A server that renders the same shape of document over and
//! over can skip all of that by reusing buffers: once the pool is warm, a
//! render allocates nothing.
//!
//! The pool is thread-local, so taking and returning a buffer is a
//! `RefCell` borrow and a `Vec::pop`/`push` — no lock, no atomic. A buffer
//! dropped on another thread simply joins that thread's pool.
//!
//! Two limits keep the pool from hoarding memory: it holds at most
//! [`MAX_POOLED`] buffers per thread, and a buffer that grew past
//! [`MAX_RETAINED_CAPACITY`] is freed instead of kept.

use std::cell::RefCell;
use std::fmt;
use std::ops::Deref;

/// Capacity of a buffer created when the pool is empty.
pub const DEFAULT_CAPACITY: usize = 1024;
/// Buffers kept per thread.
pub const MAX_POOLED: usize = 16;
/// A returned buffer with more capacity than this is freed, not pooled.
pub const MAX_RETAINED_CAPACITY: usize = 4 * 1024 * 1024;

thread_local! {
    static POOL: RefCell<Vec<Vec<u8>>> = const { RefCell::new(Vec::new()) };
}

/// Takes an empty buffer from this thread's pool, or allocates one.
#[inline]
pub fn take() -> Vec<u8> {
    POOL.try_with(|pool| pool.borrow_mut().pop())
        .ok()
        .flatten()
        .unwrap_or_else(|| Vec::with_capacity(DEFAULT_CAPACITY))
}

/// Returns a buffer to this thread's pool. Its contents are discarded.
#[inline]
pub fn give(mut buf: Vec<u8>) {
    if buf.capacity() == 0 || buf.capacity() > MAX_RETAINED_CAPACITY {
        return;
    }
    buf.clear();
    // `try_with` fails during thread teardown; the buffer is then just freed.
    let _ = POOL.try_with(|pool| {
        let mut pool = pool.borrow_mut();
        if pool.len() < MAX_POOLED {
            pool.push(buf);
        }
    });
}

/// Number of buffers currently pooled on this thread.
pub fn pooled() -> usize {
    POOL.try_with(|pool| pool.borrow().len()).unwrap_or(0)
}

/// A rendered document in a pooled buffer.
///
/// It dereferences to the bytes, and [`PooledBuf::as_str`] views them as text
/// without copying. Dropping it returns the buffer to the pool.
/// [`PooledBuf::into_vec`] keeps the bytes instead, and the pool loses that
/// buffer.
pub struct PooledBuf {
    buf: Vec<u8>,
}

impl PooledBuf {
    /// Takes an empty buffer from the pool.
    #[inline]
    pub fn new() -> Self {
        PooledBuf { buf: take() }
    }

    /// # Safety
    ///
    /// `buf` must hold valid UTF-8.
    #[inline]
    pub(crate) unsafe fn from_vec_unchecked(buf: Vec<u8>) -> Self {
        PooledBuf { buf }
    }

    /// The document as text. No copy, no validation.
    #[inline]
    pub fn as_str(&self) -> &str {
        // SAFETY: every way to fill a `PooledBuf` writes UTF-8 only.
        unsafe { std::str::from_utf8_unchecked(&self.buf) }
    }

    /// The document as bytes.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    /// Keeps the buffer. It does not go back to the pool.
    #[inline]
    pub fn into_vec(mut self) -> Vec<u8> {
        std::mem::take(&mut self.buf)
    }

    /// Keeps the buffer as a `String`. No copy.
    #[inline]
    pub fn into_string(self) -> String {
        // SAFETY: see `as_str`.
        unsafe { String::from_utf8_unchecked(self.into_vec()) }
    }

    /// Mutable access for rendering more into the same buffer.
    #[inline]
    pub(crate) fn vec_mut(&mut self) -> &mut Vec<u8> {
        &mut self.buf
    }
}

impl Default for PooledBuf {
    fn default() -> Self {
        PooledBuf::new()
    }
}

impl Drop for PooledBuf {
    #[inline]
    fn drop(&mut self) {
        give(std::mem::take(&mut self.buf));
    }
}

impl Deref for PooledBuf {
    type Target = [u8];
    #[inline]
    fn deref(&self) -> &[u8] {
        &self.buf
    }
}

impl AsRef<[u8]> for PooledBuf {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        &self.buf
    }
}

impl AsRef<str> for PooledBuf {
    #[inline]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for PooledBuf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for PooledBuf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

// There is deliberately no `DerefMut`: it would let safe code write invalid
// UTF-8 and break `as_str`. Only this crate writes into a `PooledBuf`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffers_are_reused() {
        let first = take();
        let ptr = first.as_ptr();
        give(first);
        let again = take();
        assert_eq!(again.as_ptr(), ptr);
        assert!(again.is_empty());
        give(again);
    }

    #[test]
    fn oversized_buffers_are_dropped() {
        while pooled() > 0 {
            drop(take());
        }
        give(Vec::with_capacity(MAX_RETAINED_CAPACITY + 1));
        assert_eq!(pooled(), 0);
    }
}
