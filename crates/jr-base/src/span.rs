//! Source spans.

use crate::source::FileId;
use text_size::{TextRange, TextSize};

/// A byte range within a specific source file.
///
/// Every diagnostic, every AST node, and every IR instruction that can be
/// attributed to source carries one of these. Keeping the [`FileId`] inside the
/// span (rather than threading it separately) is what makes cross-file
/// diagnostics -- "defined here, used there" -- possible without extra
/// plumbing.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    /// The file this span refers to.
    pub file: FileId,
    /// The half-open byte range within that file.
    pub range: TextRange,
}

// `TextRange` is deliberately not `Ord` upstream (ranges have no single natural
// order), but the compiler needs a total order so that diagnostics can be
// emitted in source order regardless of the order analysis discovered them.
// We define it as (file, start, end).
impl Ord for Span {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.file
            .cmp(&other.file)
            .then_with(|| self.range.start().cmp(&other.range.start()))
            .then_with(|| self.range.end().cmp(&other.range.end()))
    }
}

impl PartialOrd for Span {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Span {
    /// Creates a span.
    #[must_use]
    pub const fn new(file: FileId, range: TextRange) -> Self {
        Self { file, range }
    }

    /// Creates a span from raw byte offsets.
    ///
    /// # Panics
    /// Panics if `start > end`.
    #[must_use]
    pub fn from_offsets(file: FileId, start: u32, end: u32) -> Self {
        Self::new(
            file,
            TextRange::new(TextSize::from(start), TextSize::from(end)),
        )
    }

    /// Creates an empty span at `offset`, used for "expected a token here"
    /// diagnostics where nothing was actually consumed.
    #[must_use]
    pub fn empty_at(file: FileId, offset: TextSize) -> Self {
        Self::new(file, TextRange::empty(offset))
    }

    /// The start offset.
    #[must_use]
    pub const fn start(self) -> TextSize {
        self.range.start()
    }

    /// The end offset.
    #[must_use]
    pub const fn end(self) -> TextSize {
        self.range.end()
    }

    /// The length in bytes.
    #[must_use]
    pub const fn len(self) -> TextSize {
        self.range.len()
    }

    /// Returns `true` if the span covers no bytes.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.range.is_empty()
    }

    /// The smallest span covering both `self` and `other`.
    ///
    /// # Panics
    /// Panics if the two spans are in different files; merging across files is
    /// always a bug in the caller.
    #[must_use]
    pub fn cover(self, other: Self) -> Self {
        assert_eq!(
            self.file, other.file,
            "cannot cover spans from different files"
        );
        Self::new(self.file, self.range.cover(other.range))
    }

    /// Returns `true` if `offset` falls within the span, treating the span as
    /// inclusive of its end so that a cursor sitting just after a token still
    /// resolves to it. This is what the language server wants.
    #[must_use]
    pub fn contains_inclusive(self, offset: TextSize) -> bool {
        self.range.contains_inclusive(offset)
    }
}

impl core::fmt::Debug for Span {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{:?}:{}..{}",
            self.file,
            u32::from(self.range.start()),
            u32::from(self.range.end())
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file() -> FileId {
        FileId::from_usize(0)
    }

    #[test]
    fn cover_merges_ranges() {
        let a = Span::from_offsets(file(), 4, 8);
        let b = Span::from_offsets(file(), 20, 24);
        let covered = a.cover(b);
        assert_eq!(u32::from(covered.start()), 4);
        assert_eq!(u32::from(covered.end()), 24);
    }

    #[test]
    #[should_panic(expected = "different files")]
    fn cover_rejects_cross_file() {
        let a = Span::from_offsets(FileId::from_usize(0), 0, 1);
        let b = Span::from_offsets(FileId::from_usize(1), 0, 1);
        let _ = a.cover(b);
    }

    #[test]
    fn empty_span_is_usable_for_expected_here() {
        let s = Span::empty_at(file(), TextSize::from(12));
        assert!(s.is_empty());
        assert!(s.contains_inclusive(TextSize::from(12)));
    }
}
