//! Diagnostic codes raised by this crate, one constant each.
//!
//! # Why this file exists at the point it does
//!
//! `AGENTS.md` requires every crate to keep its codes in a `code.rs` with one constant
//! per code and a `///` saying exactly what raises it. This crate did not have one: its
//! codes were inline string literals at their emission sites, twenty-nine of them across
//! two files.
//!
//! That is not a tidiness complaint. It is the reason the parser was able to emit
//! **E0200, E0201 and E0202** — three codes that belong to `jr-hir`, whose meanings are
//! "duplicate declaration", "unresolved name" and "use before declaration" — for its
//! three "arrives in wave W*n*" refusals. Nothing collided at compile time, because a
//! `&str` cannot collide, and `AGENTS.md` had carried a standing warning not to filter
//! tests by those codes rather than the collision being fixed. Those three are now
//! **E0120, E0121, E0122**, inside the parser's own E0100–E0199 block, and this module is
//! what makes the next such mistake visible: a code used twice for different reasons is
//! now two constants with contradictory doc comments in one file.
//!
//! # The ranges
//!
//! | Range | Owner |
//! |-------|-------|
//! | E0001–E0006 | this crate's lexer |
//! | E0100–E0199 | this crate's parser |
//! | E0200–E0211 | `jr-hir` (E0210 raised by `jr-db`'s module loader) |
//! | E0212–E0226 | `jr-sema` |
//! | E0227–E0229 | `jr-mir` |
//! | E0230 | `jr-db` const-eval |
//! | E0231 | `jr-db` unused imports |
//! | E0232–E0247, E0250–E0257 | `jr-sema` and `jr-hir`, past the original blocks |
//!
//! **E0258 is the first free code overall**, and **E0131 the first free parser code.**
//!
//! This table and that sentence are the thing most likely to be stale, because a wave that
//! claims a code in another crate has no reason to open this file. ADR-0047 found the same
//! sentence wrong once already. The tests below check what this crate *owns*; they cannot check
//! a claim about somebody else's range, so the claim is a comment and the comment is a liability.

// ---------------------------------------------------------------------------
// Lexer: E0001–E0006
// ---------------------------------------------------------------------------

/// A string literal reached end of line or end of file with no closing quote.
pub(crate) const E0001: &str = "E0001";

/// A block comment was never closed. Reported at the **outermost** `/*`, because block
/// comments nest and that is the one the reader has to fix.
pub(crate) const E0002: &str = "E0002";

/// An escape sequence in a string literal is unknown, or a `\u{…}` escape is malformed.
pub(crate) const E0003: &str = "E0003";

/// A numeric literal has no digits after its base prefix, or carries an invalid suffix.
pub(crate) const E0004: &str = "E0004";

/// A character that cannot begin any token.
pub(crate) const E0005: &str = "E0005";

/// A `#` with no directive name after it.
pub(crate) const E0006: &str = "E0006";

// ---------------------------------------------------------------------------
// Parser: E0100–E0199
// ---------------------------------------------------------------------------

/// The generic "expected X, found Y" produced by [`Parser::expect`](crate::parser).
pub(crate) const E0100: &str = "E0100";

/// A token that cannot start a file-level declaration.
pub(crate) const E0101: &str = "E0101";

/// A declaration was required and the tokens present are not one.
pub(crate) const E0102: &str = "E0102";

/// `name ::` with no value after it.
pub(crate) const E0103: &str = "E0103";

/// `name :=` with no expression after it.
pub(crate) const E0104: &str = "E0104";

/// `name :` with no type after it.
pub(crate) const E0105: &str = "E0105";

/// A procedure declaration has neither a `{ … }` body nor `#foreign`.
///
/// Distinct from [`E0203`](jr_hir) — this is the *syntactic* absence, raised while
/// parsing; `jr-hir` raises E0203 for a procedure that lowered without either.
pub(crate) const E0106: &str = "E0106";

/// A parameter has a name but no type.
pub(crate) const E0107: &str = "E0107";

/// A parameter list contains something that is not a parameter name.
pub(crate) const E0108: &str = "E0108";

/// `->` with no return type after it.
pub(crate) const E0109: &str = "E0109";

/// `#foreign` with no library name after it.
pub(crate) const E0110: &str = "E0110";

/// A type was required and the tokens present are not one.
pub(crate) const E0111: &str = "E0111";

/// A struct body contains something that is not a field name.
pub(crate) const E0112: &str = "E0112";

/// A struct field has a name but no type.
pub(crate) const E0113: &str = "E0113";

/// A token that cannot start a statement, inside a block.
///
/// Also raised by `parse_stmts` for the same fault in an `#insert`'s text (ADR-0072 §1), which is a
/// *reuse* rather than a new code because the fault is identical — a token where a statement belongs —
/// and the position differs only in which text it indexes. `jr-hir` re-points and re-words it as **E0263**
/// before a reader sees it, since this code's span is an offset into the inserted string rather than a
/// position in any file (ADR-0072 §3).
pub(crate) const E0114: &str = "E0114";

/// A `{` that is never closed. Reported at the `{`.
pub(crate) const E0115: &str = "E0115";

/// A control-flow body that is neither a statement nor a block.
pub(crate) const E0116: &str = "E0116";

/// `.` with no field name after it.
pub(crate) const E0117: &str = "E0117";

/// An expression was required and the tokens present are not one.
pub(crate) const E0118: &str = "E0118";

/// A `(` in an argument list that is never closed.
pub(crate) const E0119: &str = "E0119";

// **E0120 is retired, not free.** It meant "a float literal, which lexes but arrives in wave
// W1", and floats arrived in ADR-0040 so the refusal is gone. The number is deliberately not
// reused: a user who searched for E0120 once would find a different error, and the codes are
// meant to be stable enough to look up. E0125 is the next free parser code.

/// A reserved keyword — `enum`, `union`, `cast`, `xx`, `null`, `for`, `defer`, `using` —
/// used where an expression was expected. The message names the wave it arrives in.
///
/// **Was E0201**, which is `jr-hir`'s "unresolved name". See this module's header.
pub(crate) const E0121: &str = "E0121";

// **E0122 is retired, not free.** It meant "a bitwise operator, which arrives in wave W1",
// and they arrived in ADR-0042. Like E0120 before it, the number is deliberately not reused:
// a user who searched for E0122 once should not find a different error. E0126 is the next
// free parser code.

/// `[` with no index expression after it, in `a[i]` (ADR-0039 §5).
pub(crate) const E0123: &str = "E0123";

/// An array type whose length is missing, or a `[]T` view or `[..]T` dynamic array — both
/// of which arrive in a later wave (ADR-0039 §2).
///
/// One code for all three because they are the same shape of refusal — "this array type
/// is not one this wave has" — and the *message* says which.
pub(crate) const E0124: &str = "E0124";

/// A malformed enum member: a missing name, or a `:` type annotation where a `::` value
/// belongs (ADR-0041 §3).
pub(crate) const E0125: &str = "E0125";

/// A malformed `operator` declaration (ADR-0048 §1).
///
/// Two shapes: no operator token after the keyword, and a value that is not a procedure. One code
/// because both are "this is not the `operator OP :: (…)` form" and the *message* says which.
///
/// **Which** operators may be overloaded is deliberately not this code's business: the parser
/// accepts any operator token and `jr-sema` refuses the wrapping and bitwise forms with the reason
/// (ADR-0048 §2), because "expected an operator" would be true and unhelpful for `operator +%`.
pub(crate) const E0126: &str = "E0126";

/// A malformed `for` or `defer` (ADR-0049 §1, §3).
///
/// Four shapes: no loop variable after `for`, no index name after the `,`, nothing to iterate, and
/// no statement after `defer`. One code because each is "this is not the form", and the *message*
/// says which — the same reasoning E0126 uses for a malformed `operator` declaration.
pub(crate) const E0127: &str = "E0127";

/// A malformed `using` declaration (ADR-0050 §1).
///
/// Two shapes: no name after `using`, and a `using` without an explicit type. The second is refused
/// here rather than in sema because promotion needs the type's *field list*, and `using q := f()`
/// would need the inferred type before resolution runs — so the form is not merely unsupported, it
/// cannot mean anything.
pub(crate) const E0128: &str = "E0128";

/// A malformed result list or destructuring target list (ADR-0052 §1, §2).
///
/// Three shapes: a non-type inside `-> (…)`, a target list with no `:=` or `=` after it, and a
/// trailing comma with nothing following. One code because each is "this is not the form", and the
/// *message* says which — the reasoning E0126 and E0127 already use.
pub(crate) const E0129: &str = "E0129";

/// A malformed named argument or default value (ADR-0053 §1, §2).
///
/// Two shapes: `f(a = )` with no value, and `(n: s64 = )` with no default. One code because both are
/// "the `=` has nothing after it", and the *message* says which.
pub(crate) const E0130: &str = "E0130";

/// A `#code` with no braced body (ADR-0080 §1).
///
/// `#code` needs a block, because a braceless form would have to decide where the quoted region ends and
/// "until the next `;`" cannot express two statements — ADR-0063's argument for `push_context` taking a
/// block rather than the two-shape `ControlBody`.
pub(crate) const E0131: &str = "E0131";

/// Input nested more deeply than the parser's depth limit.
///
/// Deliberately at the top of the parser's range rather than in sequence: it is a
/// resource limit rather than a syntax error, and keeping it apart leaves the sequential
/// block free to grow.
pub(crate) const E0199: &str = "E0199";

#[cfg(test)]
mod tests {
    /// Every code this crate can raise, so the test below can look for duplicates.
    const ALL: &[(&str, &str)] = &[
        ("E0001", "unterminated string"),
        ("E0002", "unterminated block comment"),
        ("E0003", "bad escape"),
        ("E0004", "bad numeric literal"),
        ("E0005", "unexpected character"),
        ("E0006", "missing directive name"),
        ("E0100", "expected X found Y"),
        ("E0101", "unexpected token at top level"),
        ("E0102", "expected a declaration"),
        ("E0103", "missing value after ::"),
        ("E0104", "missing expression after :="),
        ("E0105", "missing type after :"),
        ("E0106", "procedure without body or #foreign"),
        ("E0107", "parameter without type"),
        ("E0108", "expected a parameter name"),
        ("E0109", "missing return type"),
        ("E0110", "missing #foreign library"),
        ("E0111", "expected a type"),
        ("E0112", "expected a field name"),
        ("E0113", "field without type"),
        ("E0114", "unexpected token in block"),
        ("E0115", "unclosed brace"),
        ("E0116", "expected a statement or block"),
        ("E0117", "missing field name after ."),
        ("E0118", "expected an expression"),
        ("E0119", "unclosed paren in arguments"),
        ("E0121", "reserved keyword"),
        ("E0123", "missing index expression"),
        ("E0124", "array type not available"),
        ("E0125", "malformed enum member"),
        ("E0126", "malformed operator declaration"),
        ("E0127", "malformed for or defer"),
        ("E0128", "malformed using declaration"),
        ("E0129", "malformed result or target list"),
        ("E0130", "malformed named argument or default"),
        ("E0131", "`#code` without a braced body"),
        ("E0199", "nesting depth limit"),
    ];

    #[test]
    fn no_code_is_used_twice() {
        let mut seen = std::collections::BTreeMap::new();
        for (code, meaning) in ALL {
            if let Some(previous) = seen.insert(*code, *meaning) {
                panic!("{code} means both {previous:?} and {meaning:?}");
            }
        }
        assert_eq!(seen.len(), ALL.len());
    }

    #[test]
    fn every_code_is_in_a_range_this_crate_owns() {
        // The collision this file exists to prevent: a parser code outside E0100–E0199
        // silently takes a meaning from another crate. E0200 was `jr-hir`'s "duplicate
        // declaration" and the parser used it for a float literal.
        for (code, meaning) in ALL {
            let number: u32 = code[1..].parse().expect("a numeric code");
            let owned = (1..=6).contains(&number) || (100..=199).contains(&number);
            assert!(
                owned,
                "{code} ({meaning}) is outside E0001-E0006 and E0100-E0199, which are the \
                 only ranges jr-syntax owns"
            );
        }
    }
}
