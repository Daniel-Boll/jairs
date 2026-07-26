//! The diagnostic codes owned by semantic analysis.
//!
//! `E02xx` is semantic analysis as a whole, shared with `jr-hir`: E0200–E0211 are
//! lowering and name resolution, and everything from E0212 up is this crate.
//! [`E0204`] is the exception — it is a *relocated* code, not a new one (see its
//! own documentation).
//!
//! Two of these are deliberately shared with another crate rather than
//! duplicated under a new number, because a user reading `E0211` should not have
//! to know which phase noticed the problem.

/// An integer literal does not fit the type its context gives it.
///
/// **Relocated from lowering.** ADR-0016 §1 makes an integer literal's type come
/// from its context, and lowering does not know the context — it tested every
/// literal against `s64`, which silently accepted `x: u8 = 300;` and worded the
/// error after the wrong type. The check moved here and kept its number, because
/// it is the same error about the same source text.
pub(crate) const E0204: &str = "E0204";

/// A name is provided by two or more imported modules.
///
/// **Shared with `jr-hir`**, which raises the same code when the ambiguous name
/// is used in an *expression*. This crate raises it for a use in *type* position,
/// which name resolution never sees: `ResolveMap` covers `Expr::Name` only, and a
/// `TypeRef::Name` is not an expression.
pub(crate) const E0211: &str = "E0211";

/// A type annotation names something that does not exist.
pub(crate) const E0212: &str = "E0212";

/// A type annotation names something that exists but is not a type.
pub(crate) const E0213: &str = "E0213";

/// Mismatched types.
pub(crate) const E0214: &str = "E0214";

/// The callee of a call is not a procedure.
pub(crate) const E0215: &str = "E0215";

/// A call passes the wrong number of arguments.
pub(crate) const E0216: &str = "E0216";

/// A declaration binds the result of a procedure that returns nothing
/// (ADR-0016 §2).
pub(crate) const E0217: &str = "E0217";

/// A field access names a field the type does not have.
pub(crate) const E0218: &str = "E0218";

/// `.*` applied to something that is not a pointer.
pub(crate) const E0219: &str = "E0219";

/// The left-hand side of an assignment is not something that can be assigned to.
pub(crate) const E0220: &str = "E0220";

/// Prefix `*` applied to something with no address.
pub(crate) const E0221: &str = "E0221";

/// The condition of an `if` or `while` is not a `bool`.
pub(crate) const E0222: &str = "E0222";

/// An operator does not apply to the type of its operands.
pub(crate) const E0223: &str = "E0223";

/// A `return` disagrees with the enclosing procedure's return type.
pub(crate) const E0224: &str = "E0224";

/// The library operand of `#foreign` is not a foreign library (ADR-0016 §3).
pub(crate) const E0225: &str = "E0225";

/// A constant's type depends on its own type.
pub(crate) const E0226: &str = "E0226";
