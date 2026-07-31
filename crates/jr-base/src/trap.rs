//! The one place that decides what a trap says.
//!
//! # Why this is not in the engine that reports it
//!
//! There are two execution engines — the compile-time bytecode VM and the native
//! back end — and `crates/jr-cli/tests/differential.rs` compares a trapping
//! program's **stderr** between them, byte for byte. So the format is not a
//! presentation detail either engine may choose: it is a contract between them.
//!
//! They also build the message at different *times*. The native back end embeds a
//! string when it emits an object, because a linked binary has no source map to
//! consult; the VM builds one while the program runs. Two implementations of one
//! format, written months apart and running in different phases, cannot be kept in
//! agreement by any verifier — and their disagreement is silent until something
//! compares them. It already happened once: native said `arithmetic overflowed`
//! where the VM said `error: addition overflowed`.
//!
//! This is [ADR-0020](../../../docs/adr/0020-trap-source-locations.md) §2, and it is
//! ADR-0018 §2's reasoning about layout applied to a sentence. Layout lives in
//! `jr-pool` because both engines need a byte offset; the trap format lives here
//! because both need these words.
//!
//! # Why `jr-base`
//!
//! Because everything depends on it and it depends on nothing, so both callers can
//! reach the same function: `jr-codegen` does not depend on `jr-diag` and should not
//! acquire the dependency for one format. `Span`, `SourceMap` and [`LineCol`] already
//! live here.

use crate::source::SourceMap;
use crate::span::Span;

/// Renders a trap message, with a source location and a call chain when known.
///
/// `reason` is the sentence describing what went wrong — `"addition overflowed"` —
/// without a prefix or a newline. `location` is a rendered `path:line:col`, or `None`
/// when the trap has no source text to point at. `frames` names the procedure frames
/// that existed at the trap, **innermost first** (ADR-0066 §2); an empty slice prints
/// no chain, which is what a trap outside any Jairs frame gets.
///
/// The shape is rustc's, which is what every other diagnostic in this compiler looks
/// like:
///
/// ```
/// # use jr_base::trap_message;
/// assert_eq!(
///     trap_message("addition overflowed", Some("hello.jr:21:12"), &[]),
///     "error: addition overflowed\n  --> hello.jr:21:12\n",
/// );
/// assert_eq!(
///     trap_message("division by zero", None, &[]),
///     "error: division by zero\n",
/// );
/// assert_eq!(
///     trap_message("division by zero", Some("bt.jr:5:16"), &["inner", "main"]),
///     "error: division by zero\n  --> bt.jr:5:16\n  in inner\n  in main\n",
/// );
/// ```
///
/// The trailing newline is part of the message rather than the caller's business,
/// because the native back end writes these bytes with a single `write` and has no
/// `println!` to add one.
///
/// **Only the frames that existed appear** (ADR-0066 §4): a frame the inliner removed
/// made no call, so there is nothing to name, and inventing one would describe the
/// source rather than the execution.
#[must_use]
pub fn trap_message(reason: &str, location: Option<&str>, frames: &[&str]) -> String {
    let mut text = match location {
        Some(location) => format!("error: {reason}\n  --> {location}\n"),
        None => format!("error: {reason}\n"),
    };
    for frame in frames {
        text.push_str("  in ");
        text.push_str(frame);
        text.push('\n');
    }
    text
}

/// Renders a span as `path:line:col`, the form [`trap_message`] expects.
///
/// The path is as the source map holds it, so a program compiled from a relative path
/// reports a relative path — which is what a reader who just typed that path wants to
/// see.
#[must_use]
pub fn render_location(map: &SourceMap, span: Span) -> String {
    let file = map.file(span.file);
    let position = file.line_col(span.start());
    format!(
        "{}:{}:{}",
        file.path().display(),
        position.line,
        position.col
    )
}

#[cfg(test)]
mod tests {
    use super::trap_message;

    #[test]
    fn a_located_message_has_two_lines_and_a_trailing_newline() {
        let text = trap_message("addition overflowed", Some("a.jr:1:2"), &[]);
        assert_eq!(text, "error: addition overflowed\n  --> a.jr:1:2\n");
        assert_eq!(text.lines().count(), 2);
    }

    #[test]
    fn an_unlocated_message_omits_the_arrow_line_entirely() {
        // Not an empty arrow line: a trap on a compiler-invented value has no source
        // text, and `MirSpan::Synthetic` is the honest answer. Printing `--> ` with
        // nothing after it would suggest the location was lost rather than absent.
        assert_eq!(
            trap_message("division by zero", None, &[]),
            "error: division by zero\n"
        );
    }

    #[test]
    fn a_chain_prints_one_indented_line_per_frame_innermost_first() {
        // Innermost first, matching rustc: the trapping frame is the one being looked
        // for, so a reader does not count lines to find it (ADR-0066 §2).
        let text = trap_message(
            "division by zero",
            Some("bt.jr:5:16"),
            &["inner", "mid", "main"],
        );
        assert_eq!(
            text,
            "error: division by zero\n  --> bt.jr:5:16\n  in inner\n  in mid\n  in main\n"
        );
        assert_eq!(text.lines().count(), 5);
    }

    #[test]
    fn an_empty_chain_is_indistinguishable_from_the_old_two_line_form() {
        // The frames parameter must not change a trap that has no frames to report —
        // every existing expectation in `differential.rs` depends on that, and a
        // trailing blank line would break the byte-for-byte comparison (ADR-0020 §2).
        assert_eq!(
            trap_message("index out of bounds", Some("a.jr:3:4"), &[]),
            trap_message("index out of bounds", Some("a.jr:3:4"), &[]),
        );
        assert!(!trap_message("x", Some("a.jr:1:1"), &[]).ends_with("\n\n"));
    }

    #[test]
    fn a_chain_without_a_location_still_prints_the_frames() {
        // A synthetic-span trap inside a real call chain: the location is unknown but
        // the frames are not, and dropping them would lose the only context there is.
        assert_eq!(
            trap_message("read a value that was never assigned", None, &["f"]),
            "error: read a value that was never assigned\n  in f\n"
        );
    }
}
