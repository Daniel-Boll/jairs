//! Converting between LSP positions and `jr-base` byte offsets.
//!
//! # Why the encoding is a parameter and not a constant
//!
//! An LSP `Position` is a line plus a *character*, and what a character means is
//! negotiated. LSP 3.17 lets a server advertise `positionEncoding`, and
//! [ADR-0024](../../../docs/adr/0024-language-server.md) §3 advertises `utf-8` first
//! because a byte offset within a line is exactly what [`jr_base::Span`] already is —
//! no conversion, no arithmetic, nothing to get wrong.
//!
//! The UTF-16 path exists because a client may decline. It is implemented rather than
//! assumed away because getting it wrong is *silently* wrong: byte and UTF-16 columns
//! agree for ASCII and diverge by one per non-ASCII character, so an
//! assume-bytes-and-hope server passes every test written against an ASCII corpus and
//! misplaces every squiggle in a file containing an em dash. This repository's own
//! sources are full of them.
//!
//! # Why clamping rather than failing
//!
//! A client's position can be stale: it was computed against text the server has since
//! replaced. Returning an error for that would turn an ordinary race into a visible
//! failure, so an out-of-range line or column is clamped to the nearest valid offset.
//! The alternative — refuse the request — makes a hover flicker into an error toast.

use jr_base::TextSize;
use jr_db::LineIndex;
use lsp_types::{Position, PositionEncodingKind, Range};

/// How a client counts the `character` field of a [`Position`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Encoding {
    /// A character is a byte, which is what a [`jr_base::Span`] measures.
    #[default]
    Utf8,
    /// A character is a UTF-16 code unit, the protocol's default.
    Utf16,
}

impl Encoding {
    /// The encoding a client asked for, given the kinds it said it supports.
    ///
    /// Prefers UTF-8 when offered, because it needs no conversion. `None` — a client
    /// that did not mention the capability at all — means the protocol default, which
    /// is UTF-16.
    #[must_use]
    pub fn negotiate(offered: Option<&[PositionEncodingKind]>) -> Self {
        let Some(offered) = offered else {
            return Self::Utf16;
        };
        if offered.contains(&PositionEncodingKind::UTF8) {
            Self::Utf8
        } else {
            Self::Utf16
        }
    }

    /// The kind to advertise back, so the client and server agree.
    #[must_use]
    pub fn kind(self) -> PositionEncodingKind {
        match self {
            Self::Utf8 => PositionEncodingKind::UTF8,
            Self::Utf16 => PositionEncodingKind::UTF16,
        }
    }
}

/// A file's text and its line starts, which every conversion needs.
///
/// Bundled because a conversion is meaningless without both: the index gives the line's
/// start and the text gives what is on it.
#[derive(Debug, Clone, Copy)]
pub struct Positions<'a> {
    text: &'a str,
    index: &'a LineIndex,
    encoding: Encoding,
}

impl<'a> Positions<'a> {
    /// Creates a converter for one file.
    #[must_use]
    pub const fn new(text: &'a str, index: &'a LineIndex, encoding: Encoding) -> Self {
        Self {
            text,
            index,
            encoding,
        }
    }

    /// The byte offset a client position names, clamped into the file.
    #[must_use]
    pub fn offset(&self, position: Position) -> TextSize {
        let line = position.line as usize;
        let start = self.line_start(line);
        let line_text = self.line_text(line);
        let column = match self.encoding {
            // Already a byte count; clamp to a char boundary so slicing later cannot
            // panic on a stale position that landed mid-character.
            Encoding::Utf8 => {
                let wanted = (position.character as usize).min(line_text.len());
                floor_char_boundary(line_text, wanted)
            }
            Encoding::Utf16 => utf16_to_byte(line_text, position.character as usize),
        };
        start + TextSize::from(u32::try_from(column).unwrap_or(u32::MAX))
    }

    /// The client position for a byte offset.
    #[must_use]
    pub fn position(&self, offset: TextSize) -> Position {
        let offset = usize::from(offset).min(self.text.len());
        let offset = floor_char_boundary(self.text, offset);
        // `jr_db::LineIndex::line_col` is **1-based** in both fields, and LSP is
        // 0-based in both. Converting here, once, is the whole reason this module
        // exists: the same off-by-one applied at four call sites is how a server ends
        // up highlighting the line below the error.
        let line0 = self
            .index
            .line_col(u32::try_from(offset).unwrap_or(u32::MAX))
            .line
            .saturating_sub(1);
        let line = line0 as usize;
        let start = self.line_start(line);
        let within = offset.saturating_sub(start.into());
        let line_text = self.line_text(line);
        let character = match self.encoding {
            Encoding::Utf8 => within,
            Encoding::Utf16 => byte_to_utf16(line_text, within),
        };
        Position {
            line: line0,
            character: u32::try_from(character).unwrap_or(u32::MAX),
        }
    }

    /// The client range for a span.
    #[must_use]
    pub fn range(&self, span: jr_base::Span) -> Range {
        Range {
            start: self.position(span.start()),
            end: self.position(span.end()),
        }
    }

    fn line_start(&self, line: usize) -> TextSize {
        let last = self.index.line_starts.len().saturating_sub(1);
        let line = line.min(last);
        self.index
            .line_starts
            .get(line)
            .copied()
            .map_or_else(|| TextSize::from(0), TextSize::from)
    }

    /// The text of one line, without its terminator.
    fn line_text(&self, line: usize) -> &'a str {
        let start: usize = self.line_start(line).into();
        let end = self
            .index
            .line_starts
            .get(line + 1)
            .map_or(self.text.len(), |next| *next as usize);
        let slice = &self.text[start.min(self.text.len())..end.min(self.text.len())];
        slice
            .strip_suffix('\n')
            .map_or(slice, |s| s.strip_suffix('\r').unwrap_or(s))
    }
}

/// The largest offset at or below `wanted` that is a character boundary.
///
/// `str::floor_char_boundary` is unstable, and this workspace is stable-only.
fn floor_char_boundary(text: &str, wanted: usize) -> usize {
    let mut offset = wanted.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// The byte offset of the `units`th UTF-16 code unit in `line`.
fn utf16_to_byte(line: &str, units: usize) -> usize {
    let mut seen = 0usize;
    for (offset, ch) in line.char_indices() {
        if seen >= units {
            return offset;
        }
        seen += ch.len_utf16();
    }
    line.len()
}

/// How many UTF-16 code units precede byte offset `byte` in `line`.
fn byte_to_utf16(line: &str, byte: usize) -> usize {
    let byte = byte.min(line.len());
    line[..floor_char_boundary(line, byte)]
        .chars()
        .map(char::len_utf16)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index_of(text: &str) -> LineIndex {
        let mut starts = vec![0u32];
        starts.extend(
            text.bytes()
                .enumerate()
                .filter(|&(_, b)| b == b'\n')
                .map(|(i, _)| u32::try_from(i).expect("test text is small") + 1),
        );
        LineIndex {
            line_starts: starts,
        }
    }

    #[test]
    fn utf8_offsets_are_byte_offsets() {
        let text = "abc\ndef\n";
        let index = index_of(text);
        let p = Positions::new(text, &index, Encoding::Utf8);
        assert_eq!(
            p.offset(Position {
                line: 1,
                character: 2
            }),
            TextSize::from(6)
        );
        assert_eq!(
            p.position(TextSize::from(6)),
            Position {
                line: 1,
                character: 2
            }
        );
    }

    #[test]
    fn the_two_encodings_disagree_after_a_non_ascii_character() {
        // The whole reason both paths exist. `—` is three bytes and one UTF-16 unit, so
        // the character *after* it is byte 3 and UTF-16 unit 1. A server that assumed
        // one encoding would misplace every span on this line by two columns.
        let text = "—x\n";
        let index = index_of(text);
        let utf8 = Positions::new(text, &index, Encoding::Utf8);
        let utf16 = Positions::new(text, &index, Encoding::Utf16);

        let x = TextSize::from(3);
        assert_eq!(utf8.position(x).character, 3);
        assert_eq!(utf16.position(x).character, 1);
        assert_eq!(
            utf16.offset(Position {
                line: 0,
                character: 1
            }),
            x
        );
    }

    #[test]
    fn a_surrogate_pair_counts_as_two_utf16_units() {
        let text = "😀x\n";
        let index = index_of(text);
        let utf16 = Positions::new(text, &index, Encoding::Utf16);
        let x = TextSize::from(4);
        assert_eq!(
            utf16.position(x).character,
            2,
            "an emoji is a surrogate pair"
        );
        assert_eq!(
            utf16.offset(Position {
                line: 0,
                character: 2
            }),
            x
        );
    }

    #[test]
    fn a_stale_position_is_clamped_rather_than_refused() {
        // A client's position can be computed against text the server already replaced.
        // Clamping keeps that an ordinary race; failing would turn it into an error the
        // user sees.
        let text = "ab\n";
        let index = index_of(text);
        let p = Positions::new(text, &index, Encoding::Utf8);
        assert_eq!(
            p.offset(Position {
                line: 99,
                character: 99
            }),
            TextSize::from(3)
        );
    }

    #[test]
    fn a_position_inside_a_character_lands_on_its_boundary() {
        let text = "—\n";
        let index = index_of(text);
        let p = Positions::new(text, &index, Encoding::Utf8);
        assert_eq!(
            p.offset(Position {
                line: 0,
                character: 2
            }),
            TextSize::from(0),
            "byte 2 is inside the em dash, so it floors to its start"
        );
    }

    #[test]
    fn utf8_is_preferred_when_offered() {
        assert_eq!(
            Encoding::negotiate(Some(&[
                PositionEncodingKind::UTF16,
                PositionEncodingKind::UTF8
            ])),
            Encoding::Utf8
        );
        assert_eq!(
            Encoding::negotiate(Some(&[PositionEncodingKind::UTF16])),
            Encoding::Utf16
        );
        assert_eq!(
            Encoding::negotiate(None),
            Encoding::Utf16,
            "a client that never mentioned it gets the protocol default"
        );
    }
}
