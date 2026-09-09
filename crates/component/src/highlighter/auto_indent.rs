use ropey::Rope;
use std::ops::Range;
use tree_sitter::{Query, QueryCursor, StreamingIterator};

use crate::highlighter::{LanguageConfig, LanguageRegistry, SyntaxHighlighter};
use gpui_base::input::RopeExt;

/// Indent suggestion result
#[derive(Debug, Clone)]
pub struct IndentSuggestion {
    /// How to adjust indent relative to basis_row: +1 more, 0 same, -1 less
    pub delta: i32,
    /// The row whose indent level to use as base
    pub basis_row: usize,
    /// If true, insert 2 newlines to split {} onto separate lines
    pub split_brace: bool,
}

impl Default for IndentSuggestion {
    fn default() -> Self {
        Self {
            delta: 0,
            basis_row: 0,
            split_brace: false,
        }
    }
}

/// Point (row, column) for tree-sitter positions
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Point {
    row: usize,
    column: usize,
}

impl Point {
    fn new(row: usize, column: usize) -> Self {
        Self { row, column }
    }

    fn from_ts_pos(pos: tree_sitter::Point) -> Self {
        Self {
            row: pos.row,
            column: pos.column,
        }
    }
}

/// Compute indent suggestion for the new line created by pressing Enter.
///
/// Follows Zed's suggest_autoindents algorithm:
/// 1. Parse indents.scm tree-sitter query to get indent ranges
/// 2. For the new row, determine if it should indent more/less/same vs prev row
/// 3. Default: same indent as previous non-blank row
pub fn suggest_indent(
    language: &str,
    highlighter: &SyntaxHighlighter,
    text: &Rope,
    cursor_offset: usize,
) -> IndentSuggestion {
    if language != "cpp" && language != "c" {
        return IndentSuggestion::default();
    }

    let Some(tree) = highlighter.tree() else {
        return fallback_indent(text, cursor_offset);
    };

    let registry = LanguageRegistry::singleton();
    let Some(config) = registry.language(language) else {
        return fallback_indent(text, cursor_offset);
    };

    if config.indents.is_empty() {
        return fallback_indent(text, cursor_offset);
    }

    tree_sitter_indent(&config, tree, text, cursor_offset)
}

/// Tree-sitter query based indent following Zed's suggest_autoindents algorithm
fn tree_sitter_indent(
    config: &LanguageConfig,
    tree: &tree_sitter::Tree,
    text: &Rope,
    cursor_offset: usize,
) -> IndentSuggestion {
    let Some(language) = &config.language else {
        return fallback_indent(text, cursor_offset);
    };
    let Ok(query) = Query::new(language, &config.indents) else {
        return fallback_indent(text, cursor_offset);
    };

    let capture_names = query.capture_names();
    let indent_ix = capture_names.iter().position(|n| *n == "indent").map(|i| i as u32);
    let start_ix = capture_names.iter().position(|n| *n == "start").map(|i| i as u32);
    let end_ix = capture_names.iter().position(|n| *n == "end").map(|i| i as u32);
    let outdent_ix = capture_names.iter().position(|n| *n == "outdent").map(|i| i as u32);

    let Some(indent_ix) = indent_ix else {
        return fallback_indent(text, cursor_offset);
    };

    // Build suffixed start captures (e.g., @start.if -> "if")
    let mut suffixed_starts: Vec<(u32, String)> = Vec::new();
    for (ix, name) in capture_names.iter().enumerate() {
        if let Some(suffix) = name.strip_prefix("start.") {
            suffixed_starts.push((ix as u32, suffix.to_string()));
        }
    }

    // Phase 0: Detect if cursor is between { and } on same line → split brace
    let cursor_row = text.offset_to_point(cursor_offset).row;
    let line_start = text.line_start_offset(cursor_row);
    let line_end = text.line_end_offset(cursor_row);
    let text_before_cursor = text.slice(line_start..cursor_offset.min(line_end)).to_string();
    let text_after_cursor = text.slice(cursor_offset.min(line_end)..line_end).to_string();
    let after_trimmed = text_after_cursor.trim_start();

    let before_trimmed = text_before_cursor.trim_end();
    if after_trimmed.starts_with('}') && before_trimmed.ends_with('{') {
        return IndentSuggestion {
            delta: 1,
            basis_row: cursor_row,
            split_brace: true,
        };
    }

    // Phase 1: Process query matches to build indent ranges
    let mut indent_ranges: Vec<Range<Point>> = Vec::new();
    let mut start_positions: Vec<(Point, String)> = Vec::new();
    let mut outdent_positions: Vec<Point> = Vec::new();

    let mut query_cursor = QueryCursor::new();
    let root = tree.root_node();
    let mut matches = query_cursor.matches(&query, root, TextProvider(text));

    while let Some(m) = matches.next() {
        let mut range_start: Option<Point> = None;
        let mut range_end: Option<Point> = None;

        for cap in m.captures {
            let cap_node_start = Point::from_ts_pos(cap.node.start_position());
            let cap_node_end = Point::from_ts_pos(cap.node.end_position());

            if cap.index == indent_ix {
                range_start.get_or_insert(cap_node_start);
                range_end.get_or_insert(cap_node_end);
            } else if Some(cap.index) == start_ix {
                range_start = Some(cap_node_end);
            } else if Some(cap.index) == end_ix {
                range_end = Some(cap_node_start);
            } else if Some(cap.index) == outdent_ix {
                outdent_positions.push(cap_node_start);
            } else {
                for &(query_ix, ref suffix) in &suffixed_starts {
                    if cap.index == query_ix {
                        start_positions.push((cap_node_start, suffix.clone()));
                        break;
                    }
                }
            }
        }

        if let (Some(start), Some(end)) = (range_start, range_end) {
            // Zed: skip single-line ranges
            if start.row == end.row {
                continue;
            }
            let range = start..end;
            match indent_ranges.binary_search_by_key(&range.start, |r| r.start) {
                Err(ix) => indent_ranges.insert(ix, range),
                Ok(ix) => {
                    indent_ranges[ix].end = indent_ranges[ix].end.max(range.end);
                }
            }
        }
    }

    // Phase 2: Apply outdent positions (truncate innermost containing range)
    for outdent_pos in &outdent_positions {
        if let Some(range_to_truncate) = indent_ranges
            .iter_mut()
            .rfind(|r| r.start < *outdent_pos && r.end > *outdent_pos)
        {
            range_to_truncate.end = *outdent_pos;
        }
    }

    // Phase 3: Determine prev_row and new_row
    let prev_row = cursor_row;
    let new_row = cursor_row + 1;

    // Get prev_row's indent (column of first non-whitespace char)
    let prev_indent_col = get_indent_col(text, prev_row);
    let prev_row_start = Point::new(prev_row, prev_indent_col);

    // The new line starts with no content, so its "indent" is 0
    let new_row_start = Point::new(new_row, 0);

    // Phase 4: Compute indent_from_prev_row and outdent_to_row
    let mut indent_from_prev_row = false;
    let mut outdent_to_row = usize::MAX;

    for range in &indent_ranges {
        if range.start.row >= new_row {
            break;
        }
        // If indent range starts at prev_row and extends past the new row
        if range.start.row == prev_row && range.end > new_row_start {
            indent_from_prev_row = true;
        }
        // Pre-edit text has `}` on the next line, which will shift down after newline insertion.
        // Use `<` so ranges ending on the `}` line are not treated as outdents.
        if range.end >= prev_row_start && range.end < new_row_start {
            outdent_to_row = outdent_to_row.min(range.start.row);
        }
    }

    // Phase 5: Check for closing brace AFTER cursor (cursor is BEFORE the brace)
    let line_start = text.line_start_offset(cursor_row);
    let line_end = text.line_end_offset(cursor_row);
    let text_after_cursor = text.slice(cursor_offset.min(line_end)..line_end).to_string();
    let after_trimmed = text_after_cursor.trim_start();

    if after_trimmed.starts_with('}') {
        // Only trigger if cursor is INSIDE a brace pair (between { and })
        // If text_before ends with }, cursor is after a complete brace pair
        let before_trimmed = text_before_cursor.trim_end();
        if !before_trimmed.ends_with('}') {
            // Cursor is between { and }, use tree-sitter to find matching {
            if let Some(open_brace_row) = find_open_brace_row(text, cursor_row, cursor_offset) {
                return IndentSuggestion {
                    delta: 0,
                    basis_row: open_brace_row,
                    split_brace: false,
                };
            }
            return IndentSuggestion {
                delta: 0,
                basis_row: prev_row,
                split_brace: false,
            };
        }
        // Cursor is after a complete }, let tree-sitter outdent logic handle it
    }

    // Phase 6: Check for else keyword
    let line_text = text.slice(line_start..line_end).to_string();
    let line_trimmed = line_text.trim();
    if line_trimmed.starts_with("else") {
        let current_col = get_indent_col(text, cursor_row);
        let mut best_if_row: Option<usize> = None;
        for (pos, suffix) in &start_positions {
            if suffix == "if" && pos.row < cursor_row {
                let if_col = get_indent_col(text, pos.row);
                if if_col <= current_col {
                    best_if_row = Some(pos.row);
                }
            }
        }
        if let Some(if_row) = best_if_row {
            return IndentSuggestion {
                delta: 0,
                basis_row: if_row,
                split_brace: false,
            };
        }
    }

    // Phase 7: Generate suggestion (following Zed's logic exactly)
    if outdent_to_row == prev_row {
        IndentSuggestion {
            delta: 0,
            basis_row: prev_row,
            split_brace: false,
        }
    } else if indent_from_prev_row {
        IndentSuggestion {
            delta: 1,
            basis_row: prev_row,
            split_brace: false,
        }
    } else if outdent_to_row < prev_row {
        IndentSuggestion {
            delta: 0,
            basis_row: outdent_to_row,
            split_brace: false,
        }
    } else {
        // Default: same indent as previous non-empty row (Zed's auto_indent_using_last_non_empty_line)
        let mut basis = prev_row;
        if is_row_blank(text, basis) {
            let mut check = basis.saturating_sub(1);
            while check > 0 && is_row_blank(text, check) {
                check = check.saturating_sub(1);
            }
            if !is_row_blank(text, check) {
                basis = check;
            }
        }
        IndentSuggestion {
            delta: 0,
            basis_row: basis,
            split_brace: false,
        }
    }
}

fn is_row_blank(text: &Rope, row: usize) -> bool {
    let line_start = text.line_start_offset(row);
    let line_end = text.line_end_offset(row);
    text.slice(line_start..line_end).chars().all(|c| c.is_whitespace())
}

/// Get the column of the first non-whitespace character on a row
pub(crate) fn get_indent_col(text: &Rope, row: usize) -> usize {
    let line_start = text.line_start_offset(row);
    let line_end = text.line_end_offset(row);
    let mut col = 0;
    for ch in text.slice(line_start..line_end).chars() {
        if ch == ' ' {
            col += 1;
        } else if ch == '\t' {
            col += 4;
        } else {
            break;
        }
    }
    col
}

/// Find the row of the opening brace that matches a closing brace
fn find_open_brace_row(text: &Rope, cursor_row: usize, cursor_offset: usize) -> Option<usize> {
    let mut depth = 1usize;
    let line_start = text.line_start_offset(cursor_row);
    let end = cursor_offset.min(text.len());
    // Scan backward from cursor to find the matching {
    let chars_before: Vec<char> = text.slice(line_start..end).chars().collect();
    for &ch in chars_before.iter().rev() {
        match ch {
            '}' => depth += 1,
            '{' => {
                depth -= 1;
                if depth == 0 {
                    return Some(text.offset_to_point(end).row);
                }
            }
            _ => {}
        }
    }
    // Scan upward through previous lines
    let mut row = cursor_row.saturating_sub(1);
    let mut prev_row = cursor_row;
    while row < prev_row {
        let line_start = text.line_start_offset(row);
        let line_end = text.line_end_offset(row);
        let chars: Vec<char> = text.slice(line_start..line_end).chars().collect();
        for &ch in chars.iter().rev() {
            match ch {
                '}' => depth += 1,
                '{' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(row);
                    }
                }
                _ => {}
            }
        }
        if row == 0 {
            break;
        }
        prev_row = row;
        row -= 1;
    }
    None
}

/// Fallback indent when tree-sitter is not available
fn fallback_indent(text: &Rope, cursor_offset: usize) -> IndentSuggestion {
    let cursor_row = text.offset_to_point(cursor_offset).row;
    let line_start = text.line_start_offset(cursor_row);
    let line_end = text.line_end_offset(cursor_row);

    // Check text before cursor on current line
    let text_before = text.slice(line_start..cursor_offset.min(line_end)).to_string();
    let before_trimmed = text_before.trim_end();

    // Check text after cursor for closing brace
    let text_after = text.slice(cursor_offset.min(line_end)..line_end).to_string();
    let after_trimmed = text_after.trim_start();

    // Split brace: cursor between { and } on same line
    if after_trimmed.starts_with('}') && before_trimmed.ends_with('{') {
        return IndentSuggestion {
            delta: 1,
            basis_row: cursor_row,
            split_brace: true,
        };
    }

    if before_trimmed.ends_with('{') {
        return IndentSuggestion {
            delta: 1,
            basis_row: cursor_row,
            split_brace: false,
        };
    }

    let tokens: Vec<&str> = before_trimmed.split_whitespace().collect();
    if let Some(&last) = tokens.last() {
        if matches!(last, "if" | "else" | "for" | "while" | "do" | "switch") {
            return IndentSuggestion {
                delta: 1,
                basis_row: cursor_row,
                split_brace: false,
            };
        }
        if tokens.len() >= 2 && tokens[tokens.len() - 2] == "else" && last == "if" {
            return IndentSuggestion {
                delta: 1,
                basis_row: cursor_row,
                split_brace: false,
            };
        }
    }

    // Only trigger if cursor is INSIDE a brace pair (between { and })
    if after_trimmed.starts_with('}') && !before_trimmed.ends_with('}') {
        if let Some(open_row) = find_open_brace_row(text, cursor_row, cursor_offset) {
            return IndentSuggestion {
                delta: 0,
                basis_row: open_row,
                split_brace: false,
            };
        }
    }

    // Check previous line
    let prev_row = cursor_row.saturating_sub(1);
    if prev_row != cursor_row {
        let prev_line_start = text.line_start_offset(prev_row);
        let prev_line_end = text.line_end_offset(prev_row);
        let prev_line = text.slice(prev_line_start..prev_line_end).to_string();
        let prev_trimmed = prev_line.trim_end();

        if prev_trimmed.ends_with('{') {
            return IndentSuggestion {
                delta: 1,
                basis_row: prev_row,
                split_brace: false,
            };
        }

        let tokens: Vec<&str> = prev_trimmed.split_whitespace().collect();
        if let Some(&last) = tokens.last() {
            if matches!(last, "if" | "else" | "for" | "while" | "do" | "switch") {
                return IndentSuggestion {
                    delta: 1,
                    basis_row: prev_row,
                    split_brace: false,
                };
            }
            if tokens.len() >= 2 && tokens[tokens.len() - 2] == "else" && last == "if" {
                return IndentSuggestion {
                    delta: 1,
                    basis_row: prev_row,
                    split_brace: false,
                };
            }
        }
    }

    // Default: same indent as previous row
    IndentSuggestion {
        delta: 0,
        basis_row: prev_row,
        split_brace: false,
    }
}

struct TextProvider<'a>(pub &'a Rope);

pub struct ByteChunks<'a> {
    cursor: ropey::ChunkCursor<'a>,
    node_start: usize,
    node_end: usize,
    at_first: bool,
}

impl<'a> Iterator for ByteChunks<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        if !self.at_first {
            if !self.cursor.next() {
                return None;
            }
        }
        self.at_first = false;

        let chunk_byte_start = self.cursor.byte_offset();
        if chunk_byte_start >= self.node_end {
            return None;
        }

        let chunk = self.cursor.chunk().as_bytes();
        let start_in_chunk = self.node_start.saturating_sub(chunk_byte_start);
        let end_in_chunk = (self.node_end - chunk_byte_start).min(chunk.len());

        if start_in_chunk >= end_in_chunk {
            return None;
        }

        Some(&chunk[start_in_chunk..end_in_chunk])
    }
}

impl<'a> tree_sitter::TextProvider<&'a [u8]> for TextProvider<'a> {
    type I = ByteChunks<'a>;

    fn text(&mut self, node: tree_sitter::Node) -> Self::I {
        let range = node.byte_range();
        let cursor = self.0.chunk_cursor_at(range.start);
        ByteChunks {
            cursor,
            node_start: range.start,
            node_end: range.end,
            at_first: true,
        }
    }
}
