//! Compile-time JSON templates for Rust.
//!
//! A template is a Rust function whose body is JSON with holes:
//!
//! ```
//! jt::template! {
//!     pub fn User(id: u64, name: &str, email: Option<&str>, tags: &[&str]) -> Json {
//!         "id": id,
//!         "name": name,
//!         "email"?: email,              // omitted when `None`
//!         "tags": [ for t in tags { t } ],
//!         "kind": "user",
//!     }
//! }
//!
//! let json = jt::to_string(&User(7, "Ada", None, &["admin"]));
//! assert_eq!(json, r#"{"id":7,"name":"Ada","tags":["admin"],"kind":"user"}"#);
//! ```
//!
//! The macro turns the body into straight-line code. Every static part —
//! keys, punctuation, literal values — is escaped at compile time and merged
//! into one string, so the template above performs five writes: the static
//! prefix `{"id":`, the number, `,"name":`, the escaped name, and so on. There
//! is no intermediate tree, no reflection over fields and no per-key escaping
//! at runtime.
//!
//! # Output
//!
//! | Call | Result |
//! | --- | --- |
//! | [`to_string`] | a `String`, sized from the last render |
//! | [`to_vec`] | a `Vec<u8>` |
//! | [`to_pooled`] | a [`PooledBuf`] that goes back to the [`pool`] on drop |
//! | [`write_into`] | appends to a buffer you reuse |
//! | [`to_writer`] | streams into any `io::Write` in 8 KiB chunks |
//!
//! See the crate README for the full template syntax.

extern crate self as jt;

mod escape;
pub mod grammar;
mod out;
pub mod pool;
mod value;

use std::io;

pub use out::{Out, Stream, DEFAULT_CHUNK};
pub use pool::PooledBuf;
pub use value::{Display, JsonEntries, JsonKey, Raw, ToJson};

#[cfg(feature = "serde")]
pub use value::Serde;

#[cfg(feature = "macros")]
pub use jt_macros::{json, load, load_dir, template, write_json};

/// Appends `s` to `buf` as a quoted, escaped JSON string.
pub use escape::write_str as write_escaped_str;

/// Renders `value` into a new `String`.
///
/// The buffer is allocated once, at the size the template rendered to last
/// time, so a steady stream of similar documents does not reallocate.
#[inline]
pub fn to_string<T: ToJson + ?Sized>(value: &T) -> String {
    // SAFETY: rendering writes UTF-8 only.
    unsafe { String::from_utf8_unchecked(to_vec(value)) }
}

/// Renders `value` into a new `Vec<u8>`.
#[inline]
pub fn to_vec<T: ToJson + ?Sized>(value: &T) -> Vec<u8> {
    let mut buf = Vec::with_capacity(value.json_size_hint().max(64));
    value.to_json(&mut buf);
    value.__jt_record_len(buf.len());
    buf
}

/// Renders `value` into a buffer from the [`pool`].
///
/// Once the pool is warm this allocates nothing. Drop the result to return
/// the buffer.
#[inline]
pub fn to_pooled<T: ToJson + ?Sized>(value: &T) -> PooledBuf {
    let mut buf = pool::take();
    value.to_json(&mut buf);
    // SAFETY: rendering writes UTF-8 only.
    unsafe { PooledBuf::from_vec_unchecked(buf) }
}

/// Appends `value` to `buf`. Clear and reuse `buf` to stop allocating.
#[inline]
pub fn write_into<T: ToJson + ?Sized>(buf: &mut Vec<u8>, value: &T) {
    value.to_json(buf);
}

/// Streams `value` into `writer`, buffering at most about
/// [`DEFAULT_CHUNK`] bytes plus one array element.
///
/// The chunk buffer comes from the [`pool`]. The writer gets few, large
/// writes, so wrapping it in a `BufWriter` is unnecessary.
pub fn to_writer<W: io::Write, T: ToJson + ?Sized>(writer: W, value: &T) -> io::Result<()> {
    let mut stream = Stream::new(writer);
    value.to_json(&mut stream);
    stream.finish()
}

/// Support code for generated templates. Not a public API.
#[doc(hidden)]
pub mod __private {
    pub use core::sync::atomic::{AtomicUsize, Ordering};

    use crate::Out;

    /// Writes a fragment the macro produced, already escaped.
    #[inline(always)]
    pub fn lit<O: Out + ?Sized>(out: &mut O, s: &'static str) {
        out.raw(s)
    }

    /// Sets a comma flag. A call, not an assignment, so a flag that is
    /// written and never read again does not trip `unused_assignments` in
    /// expansions where no `#[allow]` can be attached.
    #[inline(always)]
    pub fn set(flag: &mut bool, value: bool) {
        *flag = value;
    }

    #[inline(always)]
    pub fn size_hint(hint: &AtomicUsize) -> usize {
        hint.load(Ordering::Relaxed)
    }

    /// Remembers a render's length. Stores only on a large change, so
    /// renders of similar documents on many threads do not keep writing the
    /// same cache line.
    #[inline(always)]
    pub fn record_len(hint: &AtomicUsize, len: usize) {
        let old = hint.load(Ordering::Relaxed);
        if len > old || len < old / 2 {
            hint.store(len + len / 8, Ordering::Relaxed);
        }
    }

    /// The buffer behind `jt::json!`. Safe code can only fill it through
    /// [`Out`], which writes UTF-8 only, so [`Buf::into_string`] needs no
    /// check.
    pub struct Buf(Vec<u8>);

    impl Buf {
        #[inline(always)]
        pub fn with_capacity(capacity: usize) -> Self {
            Buf(Vec::with_capacity(capacity))
        }

        #[inline(always)]
        pub fn into_string(self) -> String {
            // SAFETY: see the type's documentation.
            unsafe { String::from_utf8_unchecked(self.0) }
        }
    }

    impl Out for Buf {
        #[inline(always)]
        unsafe fn buf_mut(&mut self) -> &mut Vec<u8> {
            &mut self.0
        }
    }
}
