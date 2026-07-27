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
//!
//! **E0231 is the first free code overall**, and **E0123 the first free parser code.**

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

/// A floating-point literal, which lexes but arrives in wave W1.
///
/// **Was E0200**, which is `jr-hir`'s "duplicate declaration". See this module's header.
pub(crate) const E0120: &str = "E0120";

/// A reserved keyword — `enum`, `union`, `cast`, `xx`, `null`, `for`, `defer`, `using` —
/// used where an expression was expected. The message names the wave it arrives in.
///
/// **Was E0201**, which is `jr-hir`'s "unresolved name". See this module's header.
pub(crate) const E0121: &str = "E0121";

/// A bitwise operator used as a prefix operator; they arrive in wave W1.
///
/// **Was E0202**, which is `jr-hir`'s "use of a local before its declaration". See this
/// module's header.
pub(crate) const E0122: &str = "E0122";

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
        ("E0120", "float literal reserved"),
        ("E0121", "reserved keyword"),
        ("E0122", "bitwise operator reserved"),
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
