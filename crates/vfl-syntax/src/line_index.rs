//! Mapping byte offsets to lines and columns.

use rowan::TextSize;

/// Line start offsets of a text, for turning offsets into line/column pairs.
#[derive(Debug, Clone)]
pub struct LineIndex {
    starts: Vec<TextSize>,
}

/// A zero-based line, and a zero-based column counted in characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineCol {
    pub line: u32,
    pub col: u32,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut starts = vec![TextSize::from(0)];
        starts.extend(
            text.match_indices('\n')
                .map(|(i, _)| TextSize::from(i as u32 + 1)),
        );
        LineIndex { starts }
    }

    /// The line and column of `offset` in `text`, which must be the text
    /// this index was built from.
    pub fn line_col(&self, text: &str, offset: TextSize) -> LineCol {
        let line = self.starts.partition_point(|&start| start <= offset) - 1;
        let start = usize::from(self.starts[line]);
        let col = text[start..usize::from(offset)].chars().count();
        LineCol {
            line: line as u32,
            col: col as u32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_and_columns() {
        let text = "ab\ncdé\n\nx";
        let index = LineIndex::new(text);
        let at = |offset: u32| index.line_col(text, offset.into());
        assert_eq!(at(0), LineCol { line: 0, col: 0 });
        assert_eq!(at(3), LineCol { line: 1, col: 0 });
        assert_eq!(at(7), LineCol { line: 1, col: 3 });
        assert_eq!(at(9), LineCol { line: 3, col: 0 });
    }
}
