//! JSON string escaping.
//!
//! The output is byte-for-byte what `serde_json` produces: `"` and `\` get a
//! backslash, the five control characters with a short form use it
//! (`\b \f \n \r \t`), every other byte below `0x20` becomes `\u00XX` with
//! lowercase hex, and everything else — including `/`, DEL and all non-ASCII
//! text — is copied verbatim.
//!
//! Most strings need no escaping at all, so the scan is the hot path. It reads
//! eight bytes at a time and tests all of them with a handful of integer
//! operations (SWAR). Clean runs between escapes are copied with one
//! `extend_from_slice` each, never byte by byte.

/// Per-byte escape action: `0` copies the byte, anything else is the letter
/// written after the backslash (`u` selects the `\u00XX` form).
static ESCAPE: [u8; 256] = {
    let mut table = [0u8; 256];
    let mut i = 0;
    while i < 0x20 {
        table[i] = b'u';
        i += 1;
    }
    table[0x08] = b'b';
    table[0x09] = b't';
    table[0x0a] = b'n';
    table[0x0c] = b'f';
    table[0x0d] = b'r';
    table[b'"' as usize] = b'"';
    table[b'\\' as usize] = b'\\';
    table
};

const LO: u64 = 0x0101_0101_0101_0101;
const HI: u64 = 0x8080_8080_8080_8080;

/// Returns a mask whose high bit is set in each byte of `w` that needs
/// escaping.
///
/// Only the lowest set byte is guaranteed exact: a borrow out of a matching
/// byte can flag the byte above it. The caller only ever uses the lowest one
/// and rescans from there, so a false positive higher up costs nothing.
#[inline(always)]
fn special_mask(w: u64) -> u64 {
    // Bytes below 0x20. `& !w` drops bytes whose own high bit was set.
    let control = w.wrapping_sub(LO * 0x20) & !w;
    let quote = w ^ (LO * b'"' as u64);
    let quote = quote.wrapping_sub(LO) & !quote;
    let backslash = w ^ (LO * b'\\' as u64);
    let backslash = backslash.wrapping_sub(LO) & !backslash;
    (control | quote | backslash) & HI
}

/// Input bytes escaped per capacity check. Each block reserves room for its
/// worst case — every byte a six-byte `\u00XX` — so the loop inside writes
/// through a raw pointer with no checks at all. A small block keeps that
/// over-reservation to a few KiB however long the string is.
const BLOCK: usize = 512;

/// Writes the escape sequence for `byte` at `dst` and returns the end.
///
/// # Safety
///
/// `dst` must have room for six bytes.
#[inline(always)]
unsafe fn write_escape(dst: *mut u8, byte: u8) -> *mut u8 {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    unsafe {
        match ESCAPE[byte as usize] {
            b'u' => {
                let seq = [
                    b'\\',
                    b'u',
                    b'0',
                    b'0',
                    HEX[(byte >> 4) as usize],
                    HEX[(byte & 0xf) as usize],
                ];
                dst.copy_from_nonoverlapping(seq.as_ptr(), 6);
                dst.add(6)
            }
            letter => {
                dst.write(b'\\');
                dst.add(1).write(letter);
                dst.add(2)
            }
        }
    }
}

/// Escapes `src` into `dst` and returns the number of bytes written.
///
/// Each clean 8-byte word is copied with one unaligned store before it is
/// even tested; when it holds an escape, the output pointer only advances
/// past the clean prefix, and the next word is read from just after the
/// escaped byte. So a clean string costs one load, one store and a few ALU
/// operations per eight bytes.
///
/// # Safety
///
/// `dst` must have room for `src.len() * 6` bytes.
#[inline(always)]
unsafe fn escape_block(src: &[u8], dst: *mut u8) -> usize {
    let len = src.len();
    let base = src.as_ptr();
    let mut out = dst;
    let mut i = 0;
    unsafe {
        while i + 8 <= len {
            let word = base.add(i).cast::<u64>().read_unaligned();
            out.cast::<u64>().write_unaligned(word);
            let mask = special_mask(u64::from_le(word));
            if mask == 0 {
                out = out.add(8);
                i += 8;
                continue;
            }
            // The clean prefix is already in place; finish the word bytewise.
            let k = (mask.trailing_zeros() / 8) as usize;
            out = write_escape(out.add(k), *base.add(i + k));
            for j in k + 1..8 {
                let byte = *base.add(i + j);
                if ESCAPE[byte as usize] == 0 {
                    out.write(byte);
                    out = out.add(1);
                } else {
                    out = write_escape(out, byte);
                }
            }
            i += 8;
        }
        while i < len {
            let byte = *base.add(i);
            if ESCAPE[byte as usize] == 0 {
                out.write(byte);
                out = out.add(1);
            } else {
                out = write_escape(out, byte);
            }
            i += 1;
        }
        out.offset_from(dst) as usize
    }
}

/// Appends `s` to `buf` as a quoted, escaped JSON string.
#[inline]
pub fn write_str(buf: &mut Vec<u8>, s: &str) {
    // Enough that a string with no escapes never reserves again.
    buf.reserve(s.len() + 2 + 5 * BLOCK.min(s.len()));
    buf.push(b'"');
    write_str_contents(buf, s.as_bytes());
    buf.push(b'"');
}

/// Appends the escaped contents of `bytes`, without quotes.
#[inline]
pub(crate) fn write_str_contents(buf: &mut Vec<u8>, bytes: &[u8]) {
    for block in bytes.chunks(BLOCK) {
        buf.reserve(block.len() * 6);
        let len = buf.len();
        // SAFETY: the reservation covers the worst case, and the block is
        // UTF-8 input with ASCII escapes substituted, so the buffer stays
        // UTF-8 (a block boundary may split a character, but the next block
        // completes it before anyone reads the buffer).
        unsafe {
            let written = escape_block(block, buf.as_mut_ptr().add(len));
            buf.set_len(len + written);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn escaped(s: &str) -> String {
        let mut buf = Vec::new();
        write_str(&mut buf, s);
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn matches_serde_json_for_every_byte() {
        for b in 0u8..=0x7f {
            let s = format!("ab{}cdefghijkl{}", b as char, b as char);
            assert_eq!(
                escaped(&s),
                serde_json::to_string(&s).unwrap(),
                "byte {b:#x}"
            );
        }
    }

    #[test]
    fn matches_serde_json_at_every_offset() {
        let base = "héllo wörld, this is \u{1F600} text with \"quotes\" and \\ and \n\t\r end";
        for start in 0..base.len() {
            if !base.is_char_boundary(start) {
                continue;
            }
            let s = &base[start..];
            assert_eq!(escaped(s), serde_json::to_string(s).unwrap());
        }
    }

    #[test]
    fn long_strings_across_blocks() {
        // Escapes of every kind at every distance from block and word edges.
        let pieces = [
            "a", "\"", "\\", "\n", "\u{1}", "é", "😀", "abcdefg", "\u{1f}",
        ];
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        for len in [7, 8, 9, 511, 512, 513, 1500, 5000] {
            let mut s = String::new();
            while s.len() < len {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                s.push_str(pieces[(seed % pieces.len() as u64) as usize]);
            }
            assert_eq!(escaped(&s), serde_json::to_string(&s).unwrap(), "len {len}");
            let all_quotes = "\"".repeat(len);
            assert_eq!(
                escaped(&all_quotes),
                serde_json::to_string(&all_quotes).unwrap()
            );
            let all_controls = "\u{2}".repeat(len);
            assert_eq!(
                escaped(&all_controls),
                serde_json::to_string(&all_controls).unwrap()
            );
        }
    }

    #[test]
    fn non_ascii_high_bytes_are_not_flagged() {
        let s = "\u{80}\u{ff}\u{7ff}\u{ffff}ÿÿÿÿÿÿÿÿ";
        assert_eq!(escaped(s), serde_json::to_string(s).unwrap());
    }
}
