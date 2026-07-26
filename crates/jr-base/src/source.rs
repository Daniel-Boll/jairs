//! The source map: the compiler's view of the files it was given.

use rustc_hash::FxHashMap;
use std::path::{Path, PathBuf};
use text_size::{TextLen, TextSize};

crate::newtype_index! {
    /// Identifies a source file within a [`SourceMap`].
    pub struct FileId;
}

/// A 1-based line and column, for display to humans.
///
/// The column counts Unicode scalar values, not bytes, because that is what
/// editors and users mean by "column".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineCol {
    /// 1-based line number.
    pub line: u32,
    /// 1-based column, counted in characters.
    pub col: u32,
}

/// A single source file, together with a precomputed line index.
#[derive(Debug, Clone)]
pub struct SourceFile {
    id: FileId,
    path: PathBuf,
    text: String,
    /// Byte offset of the start of each line. Always begins with 0.
    line_starts: Vec<TextSize>,
}

impl SourceFile {
    /// This file's ID.
    #[must_use]
    pub fn id(&self) -> FileId {
        self.id
    }

    /// The path this file was loaded from.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The path as a display string, for diagnostics.
    #[must_use]
    pub fn name(&self) -> std::borrow::Cow<'_, str> {
        self.path.to_string_lossy()
    }

    /// The full text of the file.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The number of lines. A file always has at least one line, even when
    /// empty, which keeps the diagnostic renderer from special-casing it.
    #[must_use]
    pub fn line_count(&self) -> u32 {
        self.line_starts.len() as u32
    }

    /// Converts a byte offset to a 1-based line and column.
    ///
    /// Offsets past the end of the file clamp to the final position rather than
    /// panicking: a parser that reports "unexpected end of file" legitimately
    /// produces an offset equal to the file length.
    #[must_use]
    pub fn line_col(&self, offset: TextSize) -> LineCol {
        let offset = offset.min(self.text.text_len());
        // `partition_point` gives the number of line starts <= offset, which is
        // exactly the 1-based line number.
        let line_index = self.line_starts.partition_point(|&start| start <= offset) - 1;
        let line_start = self.line_starts[line_index];
        let col = self.text[usize::from(line_start)..usize::from(offset)]
            .chars()
            .count() as u32
            + 1;
        LineCol {
            line: line_index as u32 + 1,
            col,
        }
    }

    /// Returns the byte offset at which `line` (1-based) starts.
    #[must_use]
    pub fn line_start(&self, line: u32) -> Option<TextSize> {
        self.line_starts.get(line.checked_sub(1)? as usize).copied()
    }

    /// Returns the text of `line` (1-based), excluding its terminator.
    #[must_use]
    pub fn line_text(&self, line: u32) -> Option<&str> {
        let start = usize::from(self.line_start(line)?);
        let end = self
            .line_start(line + 1)
            .map_or(self.text.len(), usize::from);
        Some(self.text[start..end].trim_end_matches(['\n', '\r']))
    }

    fn compute_line_starts(text: &str) -> Vec<TextSize> {
        let mut starts = vec![TextSize::from(0)];
        starts.extend(
            text.bytes()
                .enumerate()
                .filter(|&(_, b)| b == b'\n')
                .map(|(i, _)| TextSize::from(i as u32 + 1)),
        );
        starts
    }
}

/// Owns every source file in a compilation.
///
/// [`FileId`]s are stable for the lifetime of the map, so spans remain valid.
#[derive(Debug, Clone, Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
    by_path: FxHashMap<PathBuf, FileId>,
}

impl SourceMap {
    /// Creates an empty source map.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a file, returning its ID.
    ///
    /// Adding the same path twice replaces the previous contents but keeps the
    /// same [`FileId`], so already-recorded spans continue to point at the same
    /// file (though possibly at shifted text -- callers doing incremental
    /// reloads must re-analyse).
    pub fn add(&mut self, path: impl Into<PathBuf>, text: impl Into<String>) -> FileId {
        let path = path.into();
        let text = text.into();
        let line_starts = SourceFile::compute_line_starts(&text);

        if let Some(&id) = self.by_path.get(&path) {
            let file = &mut self.files[id.index()];
            file.text = text;
            file.line_starts = line_starts;
            return id;
        }

        let id = FileId::from_usize(self.files.len());
        self.files.push(SourceFile {
            id,
            path: path.clone(),
            text,
            line_starts,
        });
        self.by_path.insert(path, id);
        id
    }

    /// Looks up a file by ID.
    ///
    /// # Panics
    /// Panics if `id` did not come from this map.
    #[must_use]
    pub fn file(&self, id: FileId) -> &SourceFile {
        &self.files[id.index()]
    }

    /// Looks up a file by path.
    #[must_use]
    pub fn file_id(&self, path: impl AsRef<Path>) -> Option<FileId> {
        self.by_path.get(path.as_ref()).copied()
    }

    /// Iterates over every file, in insertion order.
    pub fn files(&self) -> impl ExactSizeIterator<Item = &SourceFile> {
        self.files.iter()
    }

    /// The number of files.
    #[must_use]
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Returns `true` if no files have been added.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map_with(text: &str) -> (SourceMap, FileId) {
        let mut map = SourceMap::new();
        let id = map.add("test.jr", text);
        (map, id)
    }

    #[test]
    fn empty_file_has_one_line() {
        let (map, id) = map_with("");
        let file = map.file(id);
        assert_eq!(file.line_count(), 1);
        assert_eq!(
            file.line_col(TextSize::from(0)),
            LineCol { line: 1, col: 1 }
        );
    }

    #[test]
    fn line_col_is_one_based() {
        let (map, id) = map_with("abc\ndef\n");
        let file = map.file(id);
        assert_eq!(
            file.line_col(TextSize::from(0)),
            LineCol { line: 1, col: 1 }
        );
        assert_eq!(
            file.line_col(TextSize::from(2)),
            LineCol { line: 1, col: 3 }
        );
        // offset 3 is the '\n' itself: still on line 1
        assert_eq!(
            file.line_col(TextSize::from(3)),
            LineCol { line: 1, col: 4 }
        );
        assert_eq!(
            file.line_col(TextSize::from(4)),
            LineCol { line: 2, col: 1 }
        );
    }

    #[test]
    fn columns_count_characters_not_bytes() {
        let (map, id) = map_with("héllo");
        // 'é' is two bytes, so byte offset 3 is the 3rd character.
        assert_eq!(
            map.file(id).line_col(TextSize::from(3)),
            LineCol { line: 1, col: 3 }
        );
    }

    #[test]
    fn offset_past_end_clamps() {
        let (map, id) = map_with("ab");
        assert_eq!(
            map.file(id).line_col(TextSize::from(999)),
            LineCol { line: 1, col: 3 }
        );
    }

    #[test]
    fn line_text_strips_terminators() {
        let (map, id) = map_with("one\r\ntwo\nthree");
        let file = map.file(id);
        assert_eq!(file.line_text(1), Some("one"));
        assert_eq!(file.line_text(2), Some("two"));
        assert_eq!(file.line_text(3), Some("three"));
        assert_eq!(file.line_text(4), None);
    }

    #[test]
    fn re_adding_path_keeps_file_id() {
        let mut map = SourceMap::new();
        let first = map.add("a.jr", "old");
        let second = map.add("a.jr", "new text");
        assert_eq!(first, second, "FileId must be stable across reload");
        assert_eq!(map.file(first).text(), "new text");
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn distinct_paths_get_distinct_ids() {
        let mut map = SourceMap::new();
        let a = map.add("a.jr", "");
        let b = map.add("b.jr", "");
        assert_ne!(a, b);
        assert_eq!(map.file_id("b.jr"), Some(b));
        assert_eq!(map.file_id("missing.jr"), None);
    }
}
