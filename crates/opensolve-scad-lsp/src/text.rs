//! Places in a text: byte offsets, which the analysis works in, and lines
//! with columns, which the protocol (UTF-16 code units) and OpenSCAD's
//! messages (characters from 1) give.

/// Where the lines of a text start.
#[derive(Clone, Debug, Default)]
pub struct Lines {
    starts: Vec<usize>,
}

impl Lines {
    pub fn new(text: &str) -> Lines {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        Lines { starts }
    }

    /// The line (from 0) of a byte offset.
    pub fn line_of(&self, offset: usize) -> usize {
        self.starts
            .partition_point(|&s| s <= offset)
            .saturating_sub(1)
    }

    /// The bytes of line `line` (from 0), without its line break.
    pub fn line_range(&self, text: &str, line: usize) -> std::ops::Range<usize> {
        let start = self.starts.get(line).copied().unwrap_or(text.len());
        let mut end = self.starts.get(line + 1).map_or(text.len(), |e| e - 1);
        if end > start && text.as_bytes().get(end - 1) == Some(&b'\r') {
            end -= 1;
        }
        start..end.max(start)
    }

    pub fn count(&self) -> usize {
        self.starts.len()
    }

    /// Line and UTF-16 column (both from 0) of a byte offset.
    pub fn utf16(&self, text: &str, offset: usize) -> (u32, u32) {
        let offset = floor(text, offset.min(text.len()));
        let line = self.line_of(offset);
        let start = self.starts[line];
        let col: usize = text[start..offset].chars().map(char::len_utf16).sum();
        (line as u32, col as u32)
    }

    /// The byte offset of a line and UTF-16 column (both from 0); past the
    /// end of a line is its end.
    pub fn from_utf16(&self, text: &str, line: u32, col: u32) -> usize {
        let range = self.line_range(text, line as usize);
        let mut units = 0;
        for (i, c) in text[range.clone()].char_indices() {
            if units >= col as usize {
                return range.start + i;
            }
            units += c.len_utf16();
        }
        range.end
    }

    /// The byte offset of a line and a column in characters, both from 1,
    /// as OpenSCAD's messages give them.
    pub fn from_chars(&self, text: &str, line: u32, col: u32) -> usize {
        let range = self.line_range(text, line.saturating_sub(1) as usize);
        text[range.clone()]
            .char_indices()
            .nth(col.saturating_sub(1) as usize)
            .map_or(range.end, |(i, _)| range.start + i)
    }
}

/// The nearest character boundary at or before `offset`.
pub fn floor(text: &str, mut offset: usize) -> usize {
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// Whether `c` may be part of a name.
pub fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// The name around (or just before) `offset`.
pub fn word_at(text: &str, offset: usize) -> std::ops::Range<usize> {
    let offset = floor(text, offset.min(text.len()));
    let start = text[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(offset, |(i, _)| i);
    let end = text[offset..]
        .char_indices()
        .find(|(_, c)| !is_word(*c))
        .map_or(text.len(), |(i, _)| offset + i);
    start..end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_and_columns_both_ways() {
        let t = "ab\nü𝄞x\r\nend";
        let l = Lines::new(t);
        assert_eq!(l.count(), 3);
        let x = t.find('x').unwrap();
        assert_eq!(l.utf16(t, x), (1, 3));
        assert_eq!(l.from_utf16(t, 1, 3), x);
        assert_eq!(l.from_utf16(t, 1, 99), x + 1);
        assert_eq!(l.from_chars(t, 2, 3), x);
        assert_eq!(&t[l.line_range(t, 2)], "end");
        assert_eq!(&t[word_at(t, 1)], "ab");
        assert_eq!(&t[word_at(t, 2)], "ab");
    }
}
