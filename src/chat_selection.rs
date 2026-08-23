use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// A zero-based cell coordinate in the rendered chat content.
///
/// `column` is a terminal display cell, not a UTF-8 byte or Unicode scalar
/// offset. This distinction matters for Korean, CJK, emoji, and combining text.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
pub struct CellPosition {
    pub row: usize,
    pub column: usize,
}

impl CellPosition {
    pub const fn new(row: usize, column: usize) -> Self {
        Self { row, column }
    }
}

/// An inclusive mouse selection over already-wrapped chat lines.
///
/// The anchor may be before or after the cursor. `text` normalizes that
/// direction and returns the same result for forward and backward drags.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChatSelection {
    anchor: CellPosition,
    cursor: CellPosition,
}

impl ChatSelection {
    pub const fn new(anchor: CellPosition, cursor: CellPosition) -> Self {
        Self { anchor, cursor }
    }

    pub fn set_cursor(&mut self, cursor: CellPosition) {
        self.cursor = cursor;
    }

    pub fn endpoints(self) -> (CellPosition, CellPosition) {
        if self.anchor <= self.cursor {
            (self.anchor, self.cursor)
        } else {
            (self.cursor, self.anchor)
        }
    }

    /// Extracts the selected terminal cells from rendered chat lines.
    ///
    /// Both mouse endpoints are inclusive. Touching either cell occupied by a
    /// double-width grapheme selects the complete grapheme, so this method can
    /// never return half of a displayed character.
    pub fn text<S: AsRef<str>>(self, rendered_lines: &[S]) -> String {
        let Some(last_row) = rendered_lines.len().checked_sub(1) else {
            return String::new();
        };

        let (start, end) = self.endpoints();
        if start.row > last_row {
            return String::new();
        }

        let end_row = end.row.min(last_row);
        (start.row..=end_row)
            .map(|row| {
                let line = rendered_lines[row].as_ref();
                let start_column = if row == start.row { start.column } else { 0 };
                let end_column = if row == end.row {
                    end.column.saturating_add(1)
                } else {
                    usize::MAX
                };
                graphemes_in_cells(line, start_column, end_column)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn graphemes_in_cells(line: &str, start_column: usize, end_column: usize) -> String {
    let mut column: usize = 0;
    line.graphemes(true)
        .filter(|grapheme| {
            let width = UnicodeWidthStr::width(*grapheme);
            let grapheme_start = column;
            let grapheme_end = column.saturating_add(width);
            column = grapheme_end;

            // A zero-width grapheme is kept only when it belongs to the selected
            // cell boundary. Extended grapheme segmentation normally groups
            // combining marks with their base, but this also handles standalone
            // formatting characters without advancing the terminal column.
            if width == 0 {
                grapheme_start >= start_column && grapheme_start < end_column
            } else {
                grapheme_end > start_column && grapheme_start < end_column
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{CellPosition, ChatSelection};

    #[test]
    fn selects_ascii_cells_inclusively() {
        let selection = ChatSelection::new(CellPosition::new(0, 1), CellPosition::new(0, 3));

        assert_eq!(selection.text(&["hello"]), "ell");
    }

    #[test]
    fn backward_drag_is_normalized() {
        let forward = ChatSelection::new(CellPosition::new(0, 1), CellPosition::new(1, 2));
        let backward = ChatSelection::new(CellPosition::new(1, 2), CellPosition::new(0, 1));
        let lines = ["hello", "world"];

        assert_eq!(forward.text(&lines), "ello\nwor");
        assert_eq!(backward.text(&lines), forward.text(&lines));
    }

    #[test]
    fn selecting_either_cell_of_wide_grapheme_keeps_it_whole() {
        let line = ["A한B"];
        let first_cell = ChatSelection::new(CellPosition::new(0, 1), CellPosition::new(0, 1));
        let second_cell = ChatSelection::new(CellPosition::new(0, 2), CellPosition::new(0, 2));

        assert_eq!(first_cell.text(&line), "한");
        assert_eq!(second_cell.text(&line), "한");
    }

    #[test]
    fn keeps_combining_sequence_intact() {
        let line = ["Ae\u{301}B"];
        let selection = ChatSelection::new(CellPosition::new(0, 1), CellPosition::new(0, 1));

        assert_eq!(selection.text(&line), "e\u{301}");
    }

    #[test]
    fn clips_selection_to_available_rows() {
        let lines = ["one", "two"];
        let selection = ChatSelection::new(CellPosition::new(1, 1), CellPosition::new(99, 5));

        assert_eq!(selection.text(&lines), "wo");
    }

    #[test]
    fn selection_below_content_is_empty() {
        let selection = ChatSelection::new(CellPosition::new(2, 0), CellPosition::new(3, 0));

        assert!(selection.text(&["only"]).is_empty());
    }

    #[test]
    fn cursor_can_extend_an_existing_selection() {
        let mut selection = ChatSelection::new(CellPosition::new(0, 0), CellPosition::new(0, 0));
        selection.set_cursor(CellPosition::new(0, 2));

        assert_eq!(selection.text(&["abcd"]), "abc");
    }
}
