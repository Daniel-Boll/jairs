//! The bounds-check strip pass: what `--no-bounds-check` actually does.
//!
//! [ADR-0058](../../../docs/adr/0058-bounds-check-build-setting.md) §1 is this module's
//! specification, and [ADR-0003](../../../docs/adr/0003-bounds-checks-build-setting.md)
//! is why there is a pass to write at all.
//!
//! # Why this is four lines, and why that is the point
//!
//! ADR-0003 decided in the *vertical slice* — before arrays existed, before there was
//! anything to index — that the bounds check would be an **explicit MIR operation** a
//! build-configuration pass strips, rather than a decision made while lowering an index
//! expression. Its rejected alternative said what the cost of the other choice would
//! be:
//!
//! > a check that only exists as a decision made during lowering cannot be inspected,
//! > cannot be stripped as a unit, and cannot be individually eliminated by an
//! > optimisation pass.
//!
//! This module is the bill for that foresight arriving, and it is four lines: find every
//! [`Statement::BoundsCheck`], overwrite it with [`Statement::Nop`]. Twelve waves of
//! keeping the check visible in the IR is what makes the pass trivial instead of a
//! rewrite of the lowering path.
//!
//! # Why `Nop` rather than removing the statement
//!
//! [`Statement::Nop`]'s own doc comment gives the reason — deleting an element shifts
//! every later index in the block — and adds that "nothing produces it yet; the mid-end
//! will". This is that producer, twelve waves later.
//!
//! Nothing in MIR currently holds a statement index across a mutation, so removal would
//! work *today*. That is precisely what makes it the wrong choice: it would keep working
//! until something held an index, and then it would stop, in a pass whose failure mode
//! is a deleted check with the access left behind.
//!
//! # Why it is not in the pipeline loop
//!
//! [`crate::optimize`] iterates because its passes cascade: folding a branch exposes a
//! call to inline, which exposes a constant to fold. This pass cannot cascade — a body
//! never grows a new `BoundsCheck` — so running it each round would re-scan a body to
//! find nothing. `jr-db` runs it once, before the pipeline, and ADR-0058 §1 records that
//! it is a *configuration applied to the body* rather than an optimisation.
//!
//! # What is deliberately absent
//!
//! **A per-index `#no_abc`.** ADR-0058 §3 amends ADR-0003 to put the directive on the
//! procedure, so the local opt-out is a `MirBody` that simply never had the checks
//! emitted — handled in `build.rs`, not here. This pass has one input and no notion of
//! which index it is looking at.

use crate::mir::{BlockId, MirBody, Statement};

/// Replaces every bounds check in `body` with a [`Statement::Nop`].
///
/// Returns how many were stripped, which is what lets a test distinguish "the pass ran
/// and there was nothing to do" from "the pass did not run" — the difference that
/// matters when the observable behaviour of a correct program is identical either way
/// (ADR-0058 §5).
///
/// The caller decides *whether* to call this. ADR-0058 §2 puts the setting in a salsa
/// input read by `jr-db`, and ADR-0058 §4 records the consequence of const-eval reaching
/// MIR by a path that never calls it: compile-time execution always checks.
pub fn strip_bounds_checks(body: &mut MirBody) -> usize {
    let mut stripped = 0;
    // `stmts_mut` rather than `blocks_mut`, because replacing a statement with a `Nop`
    // cannot change the CFG and `blocks_mut` would invalidate the cached predecessors
    // and block order for nothing. rustc draws the same line with
    // `as_mut_preserves_cfg`, which is the comment `stmts_mut` carries.
    for index in 0..body.blocks().len() {
        for stmt in body.stmts_mut(BlockId::from_usize(index)) {
            // An exhaustive match would be house style for a *dispatch*; this is a
            // filter over one variant, and `if let` says that. Adding a `Statement`
            // variant must not force an edit here, because a new statement kind is not
            // a new bounds check.
            if let Statement::BoundsCheck { .. } = stmt {
                *stmt = Statement::Nop;
                stripped += 1;
            }
        }
    }
    stripped
}
