//! The traits generated code calls, and their implementations for std types.
//!
//! The output of every implementation here matches `serde_json` exactly, so a
//! template and a `#[derive(Serialize)]` struct over the same data produce
//! the same bytes.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::sync::Arc;

use crate::escape;
use crate::out::Out;

/// A value that renders itself as JSON.
///
/// Templates implement it, and so do the std types a template interpolates:
/// strings, numbers, `bool`, `Option`, slices, maps, smart pointers.
pub trait ToJson {
    /// Appends this value's JSON to `out`.
    fn to_json<O: Out + ?Sized>(&self, out: &mut O);

    /// Bytes this value is expected to render to. Used to size buffers.
    #[inline]
    fn json_size_hint(&self) -> usize {
        0
    }

    /// Records how long a top-level render was, so the next one can
    /// allocate once. Templates implement it; other types ignore it.
    #[doc(hidden)]
    #[inline]
    fn __jt_record_len(&self, _len: usize) {}

    /// Renders to a new `String`. Shorthand for [`crate::to_string`].
    #[inline]
    fn to_json_string(&self) -> String {
        crate::to_string(self)
    }
}

/// A value usable as an object key, which JSON requires to be a string.
///
/// Numbers are quoted, as `serde_json` does for map keys.
pub trait JsonKey {
    /// Appends the quoted key to `out`.
    fn write_key<O: Out + ?Sized>(&self, out: &mut O);
}

/// A value whose entries can be spread into an object with `...value`.
///
/// Maps implement it, and so does every template whose body is an object, so
/// one template can extend another.
pub trait JsonEntries {
    /// Appends `"key":value` pairs to `out`.
    ///
    /// `first` says whether nothing has been written to the enclosing object
    /// yet. Write a `,` before each entry when it is false, and set it to
    /// false after writing one.
    fn write_entries<O: Out + ?Sized>(&self, out: &mut O, first: &mut bool);
}

#[inline(always)]
fn buf<O: Out + ?Sized>(out: &mut O) -> &mut Vec<u8> {
    // SAFETY: every caller in this module writes UTF-8 only.
    unsafe { out.buf_mut() }
}

// --- strings --------------------------------------------------------------

impl ToJson for str {
    #[inline]
    fn to_json<O: Out + ?Sized>(&self, out: &mut O) {
        escape::write_str(buf(out), self);
    }
    #[inline]
    fn json_size_hint(&self) -> usize {
        self.len() + 2
    }
}

impl ToJson for String {
    #[inline]
    fn to_json<O: Out + ?Sized>(&self, out: &mut O) {
        escape::write_str(buf(out), self);
    }
    #[inline]
    fn json_size_hint(&self) -> usize {
        self.len() + 2
    }
}

impl ToJson for Cow<'_, str> {
    #[inline]
    fn to_json<O: Out + ?Sized>(&self, out: &mut O) {
        escape::write_str(buf(out), self);
    }
    #[inline]
    fn json_size_hint(&self) -> usize {
        self.len() + 2
    }
}

impl ToJson for char {
    #[inline]
    fn to_json<O: Out + ?Sized>(&self, out: &mut O) {
        let mut tmp = [0u8; 4];
        escape::write_str(buf(out), self.encode_utf8(&mut tmp));
    }
}

impl JsonKey for str {
    #[inline]
    fn write_key<O: Out + ?Sized>(&self, out: &mut O) {
        escape::write_str(buf(out), self);
    }
}

impl JsonKey for String {
    #[inline]
    fn write_key<O: Out + ?Sized>(&self, out: &mut O) {
        escape::write_str(buf(out), self);
    }
}

impl JsonKey for Cow<'_, str> {
    #[inline]
    fn write_key<O: Out + ?Sized>(&self, out: &mut O) {
        escape::write_str(buf(out), self);
    }
}

impl JsonKey for char {
    #[inline]
    fn write_key<O: Out + ?Sized>(&self, out: &mut O) {
        self.to_json(out)
    }
}

// --- scalars --------------------------------------------------------------

impl ToJson for bool {
    #[inline]
    fn to_json<O: Out + ?Sized>(&self, out: &mut O) {
        out.raw(if *self { "true" } else { "false" });
    }
}

impl ToJson for () {
    #[inline]
    fn to_json<O: Out + ?Sized>(&self, out: &mut O) {
        out.raw("null");
    }
}

macro_rules! integers {
    ($($t:ty),*) => {$(
        impl ToJson for $t {
            #[inline]
            fn to_json<O: Out + ?Sized>(&self, out: &mut O) {
                let mut b = itoa::Buffer::new();
                buf(out).extend_from_slice(b.format(*self).as_bytes());
            }
        }

        impl JsonKey for $t {
            #[inline]
            fn write_key<O: Out + ?Sized>(&self, out: &mut O) {
                let mut b = itoa::Buffer::new();
                let digits = b.format(*self).as_bytes();
                let buf = buf(out);
                buf.reserve(digits.len() + 2);
                buf.push(b'"');
                buf.extend_from_slice(digits);
                buf.push(b'"');
            }
        }
    )*};
}

integers!(i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize);

macro_rules! floats {
    ($($t:ty),*) => {$(
        impl ToJson for $t {
            /// Non-finite values render as `null`, as in `serde_json`.
            #[inline]
            fn to_json<O: Out + ?Sized>(&self, out: &mut O) {
                if self.is_finite() {
                    let mut b = zmij::Buffer::new();
                    buf(out).extend_from_slice(b.format_finite(*self).as_bytes());
                } else {
                    out.raw("null");
                }
            }
        }
    )*};
}

floats!(f32, f64);

// --- wrappers -------------------------------------------------------------

impl<T: ToJson + ?Sized> ToJson for &T {
    #[inline(always)]
    fn to_json<O: Out + ?Sized>(&self, out: &mut O) {
        (**self).to_json(out)
    }
    #[inline(always)]
    fn json_size_hint(&self) -> usize {
        (**self).json_size_hint()
    }
    #[inline(always)]
    fn __jt_record_len(&self, len: usize) {
        (**self).__jt_record_len(len)
    }
}

impl<T: ToJson + ?Sized> ToJson for &mut T {
    #[inline(always)]
    fn to_json<O: Out + ?Sized>(&self, out: &mut O) {
        (**self).to_json(out)
    }
    #[inline(always)]
    fn json_size_hint(&self) -> usize {
        (**self).json_size_hint()
    }
    #[inline(always)]
    fn __jt_record_len(&self, len: usize) {
        (**self).__jt_record_len(len)
    }
}

macro_rules! smart_pointers {
    ($($p:ident),*) => {$(
        impl<T: ToJson + ?Sized> ToJson for $p<T> {
            #[inline(always)]
            fn to_json<O: Out + ?Sized>(&self, out: &mut O) {
                (**self).to_json(out)
            }
            #[inline(always)]
            fn json_size_hint(&self) -> usize {
                (**self).json_size_hint()
            }
            #[inline(always)]
            fn __jt_record_len(&self, len: usize) {
                (**self).__jt_record_len(len)
            }
        }

        impl<T: JsonKey + ?Sized> JsonKey for $p<T> {
            #[inline(always)]
            fn write_key<O: Out + ?Sized>(&self, out: &mut O) {
                (**self).write_key(out)
            }
        }

        impl<T: JsonEntries + ?Sized> JsonEntries for $p<T> {
            #[inline(always)]
            fn write_entries<O: Out + ?Sized>(&self, out: &mut O, first: &mut bool) {
                (**self).write_entries(out, first)
            }
        }
    )*};
}

smart_pointers!(Box, Rc, Arc);

impl<T: JsonKey + ?Sized> JsonKey for &T {
    #[inline(always)]
    fn write_key<O: Out + ?Sized>(&self, out: &mut O) {
        (**self).write_key(out)
    }
}

impl<T: JsonEntries + ?Sized> JsonEntries for &T {
    #[inline(always)]
    fn write_entries<O: Out + ?Sized>(&self, out: &mut O, first: &mut bool) {
        (**self).write_entries(out, first)
    }
}

impl<T: ToJson> ToJson for Option<T> {
    /// `None` renders as `null`. Use a `"key"?: value` field to omit it.
    #[inline]
    fn to_json<O: Out + ?Sized>(&self, out: &mut O) {
        match self {
            Some(v) => v.to_json(out),
            None => out.raw("null"),
        }
    }
    #[inline]
    fn json_size_hint(&self) -> usize {
        self.as_ref().map_or(4, ToJson::json_size_hint)
    }
}

impl<T: JsonEntries> JsonEntries for Option<T> {
    /// `None` spreads nothing.
    #[inline]
    fn write_entries<O: Out + ?Sized>(&self, out: &mut O, first: &mut bool) {
        if let Some(v) = self {
            v.write_entries(out, first)
        }
    }
}

// --- sequences ------------------------------------------------------------

#[inline]
fn write_seq<'a, T, O, I>(out: &mut O, items: I)
where
    T: ToJson + 'a,
    O: Out + ?Sized,
    I: IntoIterator<Item = &'a T>,
{
    buf(out).push(b'[');
    let mut items = items.into_iter();
    if let Some(item) = items.next() {
        item.to_json(out);
        out.checkpoint();
        for item in items {
            buf(out).push(b',');
            item.to_json(out);
            out.checkpoint();
        }
    }
    buf(out).push(b']');
}

#[inline]
fn seq_hint<T: ToJson>(first: Option<&T>, len: usize) -> usize {
    first.map_or(2, |f| (f.json_size_hint() + 1) * len + 1)
}

impl<T: ToJson> ToJson for [T] {
    #[inline]
    fn to_json<O: Out + ?Sized>(&self, out: &mut O) {
        write_seq(out, self)
    }
    #[inline]
    fn json_size_hint(&self) -> usize {
        seq_hint(self.first(), self.len())
    }
}

impl<T: ToJson, const N: usize> ToJson for [T; N] {
    #[inline]
    fn to_json<O: Out + ?Sized>(&self, out: &mut O) {
        write_seq(out, self)
    }
    #[inline]
    fn json_size_hint(&self) -> usize {
        seq_hint(self.first(), N)
    }
}

impl<T: ToJson> ToJson for Vec<T> {
    #[inline]
    fn to_json<O: Out + ?Sized>(&self, out: &mut O) {
        write_seq(out, self)
    }
    #[inline]
    fn json_size_hint(&self) -> usize {
        seq_hint(self.first(), self.len())
    }
}

impl<T: ToJson> ToJson for VecDeque<T> {
    #[inline]
    fn to_json<O: Out + ?Sized>(&self, out: &mut O) {
        write_seq(out, self)
    }
    #[inline]
    fn json_size_hint(&self) -> usize {
        seq_hint(self.front(), self.len())
    }
}

impl<T: ToJson, S> ToJson for HashSet<T, S> {
    #[inline]
    fn to_json<O: Out + ?Sized>(&self, out: &mut O) {
        write_seq(out, self)
    }
}

impl<T: ToJson> ToJson for BTreeSet<T> {
    #[inline]
    fn to_json<O: Out + ?Sized>(&self, out: &mut O) {
        write_seq(out, self)
    }
}

macro_rules! tuples {
    ($(($($n:tt $t:ident),+))*) => {$(
        impl<$($t: ToJson),+> ToJson for ($($t,)+) {
            /// Tuples render as arrays, as in `serde_json`.
            #[inline]
            fn to_json<O: Out + ?Sized>(&self, out: &mut O) {
                buf(out).push(b'[');
                tuples!(@items self out $($n)+);
                buf(out).push(b']');
            }
        }
    )*};
    (@items $s:ident $o:ident $first:tt $($n:tt)*) => {
        $s.$first.to_json($o);
        $( buf($o).push(b','); $s.$n.to_json($o); )*
    };
}

tuples! {
    (0 A)
    (0 A, 1 B)
    (0 A, 1 B, 2 C)
    (0 A, 1 B, 2 C, 3 D)
    (0 A, 1 B, 2 C, 3 D, 4 E)
    (0 A, 1 B, 2 C, 3 D, 4 E, 5 F)
    (0 A, 1 B, 2 C, 3 D, 4 E, 5 F, 6 G)
    (0 A, 1 B, 2 C, 3 D, 4 E, 5 F, 6 G, 7 H)
}

// --- maps -----------------------------------------------------------------

#[inline]
fn write_pairs<'a, K, V, O, I>(out: &mut O, pairs: I, first: &mut bool)
where
    K: JsonKey + ?Sized + 'a,
    V: ToJson + ?Sized + 'a,
    O: Out + ?Sized,
    I: IntoIterator<Item = (&'a K, &'a V)>,
{
    for (k, v) in pairs {
        if !*first {
            buf(out).push(b',');
        }
        *first = false;
        k.write_key(out);
        buf(out).push(b':');
        v.to_json(out);
        out.checkpoint();
    }
}

#[inline]
fn write_map<'a, K, V, O, I>(out: &mut O, pairs: I)
where
    K: JsonKey + ?Sized + 'a,
    V: ToJson + ?Sized + 'a,
    O: Out + ?Sized,
    I: IntoIterator<Item = (&'a K, &'a V)>,
{
    buf(out).push(b'{');
    write_pairs(out, pairs, &mut true);
    buf(out).push(b'}');
}

impl<K: JsonKey, V: ToJson, S> ToJson for HashMap<K, V, S> {
    #[inline]
    fn to_json<O: Out + ?Sized>(&self, out: &mut O) {
        write_map(out, self)
    }
}

impl<K: JsonKey, V: ToJson> ToJson for BTreeMap<K, V> {
    #[inline]
    fn to_json<O: Out + ?Sized>(&self, out: &mut O) {
        write_map(out, self)
    }
}

impl<K: JsonKey, V: ToJson, S> JsonEntries for HashMap<K, V, S> {
    #[inline]
    fn write_entries<O: Out + ?Sized>(&self, out: &mut O, first: &mut bool) {
        write_pairs(out, self, first)
    }
}

impl<K: JsonKey, V: ToJson> JsonEntries for BTreeMap<K, V> {
    #[inline]
    fn write_entries<O: Out + ?Sized>(&self, out: &mut O, first: &mut bool) {
        write_pairs(out, self, first)
    }
}

impl<K: JsonKey, V: ToJson> JsonEntries for [(K, V)] {
    #[inline]
    fn write_entries<O: Out + ?Sized>(&self, out: &mut O, first: &mut bool) {
        write_pairs(out, self.iter().map(|(k, v)| (k, v)), first)
    }
}

impl<K: JsonKey, V: ToJson, const N: usize> JsonEntries for [(K, V); N] {
    #[inline]
    fn write_entries<O: Out + ?Sized>(&self, out: &mut O, first: &mut bool) {
        write_pairs(out, self.iter().map(|(k, v)| (k, v)), first)
    }
}

impl<K: JsonKey, V: ToJson> JsonEntries for Vec<(K, V)> {
    #[inline]
    fn write_entries<O: Out + ?Sized>(&self, out: &mut O, first: &mut bool) {
        write_pairs(out, self.iter().map(|(k, v)| (k, v)), first)
    }
}

// --- escape hatches -------------------------------------------------------

/// Pre-rendered JSON, written verbatim.
///
/// The text is not checked. Passing anything that is not valid JSON produces
/// invalid output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Raw<'a>(pub &'a str);

impl ToJson for Raw<'_> {
    #[inline]
    fn to_json<O: Out + ?Sized>(&self, out: &mut O) {
        out.raw(self.0)
    }
    #[inline]
    fn json_size_hint(&self) -> usize {
        self.0.len()
    }
}

/// Renders a value's `Display` output as a JSON string, without an
/// intermediate `String`.
#[derive(Clone, Copy, Debug)]
pub struct Display<T>(pub T);

impl<T: std::fmt::Display> ToJson for Display<T> {
    fn to_json<O: Out + ?Sized>(&self, out: &mut O) {
        use std::fmt::Write as _;

        struct Escaper<'a>(&'a mut Vec<u8>);
        impl std::fmt::Write for Escaper<'_> {
            #[inline]
            fn write_str(&mut self, s: &str) -> std::fmt::Result {
                escape::write_str_contents(self.0, s.as_bytes());
                Ok(())
            }
        }

        let buf = buf(out);
        buf.push(b'"');
        let _ = write!(Escaper(buf), "{}", self.0);
        buf.push(b'"');
    }
}

/// Renders any `serde::Serialize` value through `serde_json`.
///
/// The bridge for types that already derive `Serialize`: they can sit inside
/// a template without writing a template for them.
#[cfg(feature = "serde")]
#[derive(Clone, Copy, Debug)]
pub struct Serde<T>(pub T);

#[cfg(feature = "serde")]
impl<T: serde::Serialize> ToJson for Serde<T> {
    fn to_json<O: Out + ?Sized>(&self, out: &mut O) {
        // `serde_json` writes UTF-8 only, and a `Vec` sink cannot fail.
        let _ = serde_json::to_writer(buf(out), &self.0);
    }
}
