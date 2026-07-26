//! The diagnostic codes this crate owns.
//!
//! Modelled on `jr-sema`'s `code.rs`, which is the closest thing this workspace
//! has to a registry: one constant per code, next to a `///` saying exactly what
//! condition raises it. There is no central table, so the prose tables in
//! `jr-sema`'s and `jr-hir`'s crate docs are updated by hand — and this crate's
//! docs carry the same table for its own range.
//!
//! E0227 was the first free code when this crate claimed it. E0001–E0006 are the
//! lexer, E0100–E0199 the parser, E0200–E0211 `jr-hir` (with E0210 actually raised
//! by `jr-db`'s module loader and E0204 relocated to `jr-sema`), and E0212–E0226
//! `jr-sema`.
//!
//! # Why these three could not be raised earlier
//!
//! All three are questions about *paths*, not about syntax or types, so none can be
//! answered before a CFG exists. `jr-sema`'s crate docs say so explicitly for the
//! first two, deferring them to "MIR's CFG rather than a syntax walk". The third is
//! stranger: nothing checked it at all — `jr-hir` lowers `break` without asking
//! whether it is inside a loop and `jr-sema` ignores the statement entirely — so
//! MIR is simply the first pass that can see it.

/// A local is read on a path that never assigns it.
///
/// Raised only for a local written `x: T = ---;`. A local declared `x: T;` is
/// default-initialised to its type's zero value
/// (`tests/corpus/valid/005-decl-typed.jr`) and is therefore never reported.
pub(crate) const USE_OF_UNINITIALISED: &str = "E0227";

/// Control can reach the end of a procedure that must return a value.
///
/// Reachability is decided on the CFG, so a procedure whose every path returns is
/// silent even when a `return` is not the syntactically last statement.
pub(crate) const MISSING_RETURN: &str = "E0228";

/// A `break` or `continue` that is not inside a loop.
pub(crate) const JUMP_OUTSIDE_LOOP: &str = "E0229";
