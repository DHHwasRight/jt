//! Converts between byte offsets and LSP positions.
//!
//! LSP counts columns in UTF-16 code units by default. The index stores the
//! byte offset of each line start, and walks one line's characters per
//! conversion.

use lsp_types::Position;

pub struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        LineIndex { starts }
    }

    pub fn position(&self, text: &str, offset: usize) -> Position {
        let offset = offset.min(text.len());
        let line = self.starts.partition_point(|&s| s <= offset) - 1;
        let start = self.starts[line];
        let column: usize = text[start..offset].chars().map(char::len_utf16).sum();
        Position::new(line as u32, column as u32)
    }

    pub fn offset(&self, text: &str, position: Position) -> usize {
        let Some(&start) = self.starts.get(position.line as usize) else {
            return text.len();
        };
        let end = self
            .starts
            .get(position.line as usize + 1)
            .map_or(text.len(), |&next| next - 1);
        let mut units = 0;
        for (i, ch) in text[start..end].char_indices() {
            if units >= position.character as usize {
                return start + i;
            }
            units += ch.len_utf16();
        }
        end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_utf16() {
        let text = "ab\n\"é😀\": x\n\nend";
        let index = LineIndex::new(text);
        let x = text.find('x').unwrap();
        let pos = index.position(text, x);
        // `"`, `é`, the emoji as two units, `"`, `:` and a space.
        assert_eq!(pos, Position::new(1, 7));
        assert_eq!(index.offset(text, pos), x);
        assert_eq!(index.position(text, text.len()), Position::new(3, 3));
        assert_eq!(
            index.offset(text, Position::new(2, 5)),
            text.find("\nend").unwrap()
        );
        assert_eq!(index.offset(text, Position::new(9, 0)), text.len());
    }
}
