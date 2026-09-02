//! Dead-code elimination: unreachable blocks, `Nop`s, dead pure assignments, unused slots.
//!
//! [ADR-0022](../../../docs/adr/0022-dce-constprop-shared-arithmetic.md) §4 is this
//! module's specification. It is the first pass in the compiler that *removes*
//! observable behaviour if it gets the rules wrong, which is why the interesting part
//! of it is [`is_pure`] and not the graph work.
//!
//! # The rule that makes this pass dangerous
//!
//! A dead assignment can still trap. `crates/jr-codegen-clif/src/body.rs` says so, in
//! a comment, about the sibling case:
//!
//! > A discarded rvalue is still evaluated, deliberately: an ADR-0002 overflow in an
//! > expression whose result nobody wants still traps.
//!
//! A `Statement::Assign` whose destination nothing reads is semantically that same
//! statement. So "remove assignments nobody reads" is a miscompile, and the pass may
//! only remove one whose rvalue is *provably* free of effects. [`is_pure`] is an
//! exhaustive match for that reason: a future [`Rvalue`] variant is a compile error
//! here rather than something a `_` arm quietly declares harmless.
//!
//! Three families are refused, each for its own reason:
//!
//! - **Trapping arithmetic** — ADR-0002 says overflow always traps and never differs
//!   between build modes, so deleting the operation deletes a trap.
//! - **A call** — it can do anything, up to and including `modules/Basic`'s `exit`.
//! - **A load, and any place reached through a `Deref`** — reading through a dangling
//!   pointer faults. `jr-vm`'s `Trap::BadAddress` docs note that this is reachable
//!   from a *valid* program, because a pointer into a released frame is expressible.
//!
//! # Why the passes are separate functions and this one is not "the optimiser"
//!
//! ADR-0022 §3 puts the ordering in [`crate::optimize`]. This function is one step,
//! reports whether it changed anything, and is `pub` so a test can drive it alone —
//! which is how the dangerous cases above are asserted without also asserting
//! whatever const-prop did.
//!
//! # What it deliberately leaves
//!
//! **Dead SSA values.** A value whose defining statement is removed keeps its
//! `ValueData` entry, so `value_count()` does not shrink and the VM still sizes a
//! frame for it. Compacting the value arena means renumbering every `ValueId` in the
//! body, and unlike a slot a value is named by block parameters too; it is worth
//! doing when a register budget is a measured problem rather than a suspected one.

use jr_pool::Pool;
use rustc_hash::FxHashSet;

use crate::mir::{
    BlockId, Callee, MirBody, Operand, Place, PlaceBase, Projection, Rvalue, SlotId, Statement,
    Terminator, ValueId,
};
use crate::verify;

/// Whether an rvalue can be discarded without changing what the program does.
///
/// See the module docs for why each refusal is a refusal. The match is exhaustive on
/// purpose.
#[must_use]
pub fn is_pure(rvalue: &Rvalue) -> bool {
    match rvalue {
        // A copy, an address and an undefined value do nothing. `Address` of a
        // `Deref` place is still nothing: taking an address does not read through it.
        Rvalue::Use(_) | Rvalue::Address(_) | Rvalue::Undef => true,
        Rvalue::Binary { op, .. } => !op.can_trap(),
        Rvalue::Unary { op, .. } => !op.can_trap(),
        // A conversion **cannot trap**: ADR-0037 §2 makes a narrowing cast truncate rather
        // than check, which is exactly why it is safe to delete an unused one. Were `cast`
        // ever changed to trap, this arm becomes wrong and silently deletes an observable
        // trap — so that reversal needs a new ADR and this line, not just a codegen change.
        Rvalue::Convert { .. } => true,
        // A load faults through a dangling pointer, and `jr-vm` documents that as
        // reachable from a valid program.
        Rvalue::Load(_) => false,
        // Anything at all, including `exit`.
        Rvalue::Call { .. } => false,
        // **Never pure, including a load** (ADR-0176 §2). An ordinary `Load` is impure only because it
        // can fault; an atomic load is impure because *another thread can observe it* — it participates
        // in the ordering that gives the program its meaning. Deleting an unread `atomic_compare_exchange`
        // would delete the lock acquisition while leaving the critical section, which is the exact shape
        // of bug this arm exists to prevent.
        Rvalue::Atomic { .. } => false,
    }
}

/// Removes what cannot affect the program, and reports whether anything changed.
///
/// # Panics
/// In a debug build, if the result is malformed. That is the point.
pub fn dce(body: &mut MirBody, pool: &Pool) -> bool {
    let mut changed = drop_unreachable_blocks(body);
    changed |= drop_dead_assignments(body);
    changed |= drop_dead_stores(body);
    changed |= drop_nops(body);
    changed |= drop_unused_slots(body);
    if changed {
        verify::assert_valid(body, pool);
    }
    changed
}

// ---------------------------------------------------------------------------
// Unreachable blocks
// ---------------------------------------------------------------------------

/// Drops every block the entry cannot reach.
///
/// `reverse_postorder` already computes exactly this set — it is a DFS from the entry
/// — so reachability is a read of a cached fact rather than a new traversal.
fn drop_unreachable_blocks(body: &mut MirBody) -> bool {
    let mut keep = vec![false; body.block_count()];
    for block in body.reverse_postorder() {
        keep[block.index()] = true;
    }
    if keep.iter().all(|k| *k) {
        return false;
    }
    body.retain_blocks(&keep);
    true
}

// ---------------------------------------------------------------------------
// Dead assignments
// ---------------------------------------------------------------------------

/// Turns a dead pure assignment into a `Nop`, repeatedly.
///
/// The loop is inside this function rather than left to [`crate::optimize`]'s
/// bounded outer loop, because a chain of *n* dead assignments needs *n* iterations
/// to collapse and the outer cap is a small constant. Bounded by the value count,
/// since each iteration kills at least one definition.
fn drop_dead_assignments(body: &mut MirBody) -> bool {
    let mut changed = false;
    for _ in 0..=body.value_count() {
        let used = used_values(body);
        let mut hit = false;
        for index in 0..body.block_count() {
            let block = BlockId::from_usize(index);
            for stmt in body.stmts_mut(block) {
                let Statement::Assign { dest, rvalue, .. } = stmt else {
                    continue;
                };
                if used.contains(dest) || !is_pure(rvalue) {
                    continue;
                }
                *stmt = Statement::Nop;
                hit = true;
            }
        }
        if !hit {
            return changed;
        }
        changed = true;
    }
    changed
}

/// Every value read anywhere in the body.
///
/// A block *parameter* is not a read, but every argument supplied to one is, which is
/// what keeps a value alive across an edge. A definition is not a read either — that
/// is the whole point of the set.
fn used_values(body: &MirBody) -> FxHashSet<ValueId> {
    let mut used = FxHashSet::default();
    for block in body.blocks() {
        for stmt in &block.stmts {
            match stmt {
                Statement::Assign { rvalue, .. } | Statement::Discard { rvalue, .. } => {
                    note_rvalue(rvalue, &mut used);
                }
                Statement::Store { place, value, .. } => {
                    note_place(place, &mut used);
                    note_operand(value, &mut used);
                }
                Statement::Zero { place, .. } => note_place(place, &mut used),
                // Both operands are read, and this statement is never deleted, so its
                // operands keep their definitions alive.
                Statement::BoundsCheck { index, len, .. } => {
                    note_operand(index, &mut used);
                    note_operand(len, &mut used);
                }
                Statement::TagCheck { place, .. } => note_place(place, &mut used),
                Statement::Nop => {}
            }
        }
        match &block.term {
            Terminator::Goto(target) => note_args(&target.args, &mut used),
            Terminator::Branch { cond, then_, else_ } => {
                note_operand(cond, &mut used);
                note_args(&then_.args, &mut used);
                note_args(&else_.args, &mut used);
            }
            Terminator::Return(value) => {
                if let Some(operand) = value {
                    note_operand(operand, &mut used);
                }
            }
            Terminator::Unreachable(_) => {}
        }
    }
    used
}

fn note_args(args: &[Operand], used: &mut FxHashSet<ValueId>) {
    for arg in args {
        note_operand(arg, used);
    }
}

fn note_operand(operand: &Operand, used: &mut FxHashSet<ValueId>) {
    match operand {
        Operand::Value(value) => {
            used.insert(*value);
        }
        Operand::Constant(_) => {}
    }
}

fn note_place(place: &Place, used: &mut FxHashSet<ValueId>) {
    match &place.base {
        PlaceBase::Slot(_) => {}
        PlaceBase::Deref(operand) => note_operand(operand, used),
    }
    // Projections used to hold no operands, so this walk did not exist.
    // `Projection::Index` carries one, and missing it here would let DCE delete the
    // definition of an index that a surviving access still reads — a dangling `ValueId`
    // in a place, which the verifier catches but only after the damage is expressible.
    for projection in &place.projection {
        match projection {
            Projection::Index(operand) => note_operand(operand, used),
            Projection::Field(_)
            | Projection::Deref
            | Projection::StringData
            | Projection::StringCount
            | Projection::ViewData
            | Projection::ViewCount
            | Projection::DynamicArrayData
            | Projection::DynamicArrayCount
            | Projection::DynamicArrayCapacity
            | Projection::VariantTag => {}
        }
    }
}

fn note_rvalue(rvalue: &Rvalue, used: &mut FxHashSet<ValueId>) {
    match rvalue {
        // A pure operand walk. Renaming an atomic's operands never changes what it does or when, which is
        // why every such pass may touch one while none may move, duplicate or delete it (ADR-0176 §2).
        Rvalue::Atomic {
            op: _,
            address,
            value,
            expected,
        } => {
            note_operand(address, used);
            if let Some(value) = value {
                note_operand(value, used);
            }
            if let Some(expected) = expected {
                note_operand(expected, used);
            }
        }
        Rvalue::Use(operand) => note_operand(operand, used),
        Rvalue::Binary { op: _, lhs, rhs } => {
            note_operand(lhs, used);
            note_operand(rhs, used);
        }
        Rvalue::Unary { op: _, operand } => note_operand(operand, used),
        Rvalue::Convert { operand, from: _ } => note_operand(operand, used),
        Rvalue::Call { callee, args } => {
            match callee {
                Callee::Direct(_) => {}
                Callee::Indirect(operand) => note_operand(operand, used),
            }
            note_args(args, used);
        }
        Rvalue::Load(place) | Rvalue::Address(place) => note_place(place, used),
        Rvalue::Undef => {}
    }
}

// ---------------------------------------------------------------------------
// Dead stores
// ---------------------------------------------------------------------------

/// Drops a store to a slot that is never loaded and never address-taken.
///
/// Without this, [`drop_unused_slots`] cannot fire on the case ADR-0022 §4 was
/// written for: `print_line`'s spill slot is kept alive by the dead store that fills
/// it, so "remove slots nothing mentions" removes nothing.
///
/// Sound because the address was never taken. Nothing can alias the slot, so nothing
/// can observe the write. A store through a [`PlaceBase::Deref`] is never dropped,
/// because what it aliases is unknown — that is the whole difference between a slot
/// and a pointer here.
///
/// The stored *operand* may be a value this makes dead;
/// [`drop_dead_assignments`] is not re-run inside this function, because
/// [`crate::optimize`]'s bounded loop will call `dce` again and pick it up.
fn drop_dead_stores(body: &mut MirBody) -> bool {
    // A slot is observable if a pointer to it exists — the shared predicate, so that
    // this pass and `forward.rs` cannot disagree about what escaping means — or if
    // something loads from it.
    let mut observed: FxHashSet<SlotId> = crate::forward::escaping_slots(body);
    for block in body.blocks() {
        for stmt in &block.stmts {
            match stmt {
                Statement::Assign { rvalue, .. } | Statement::Discard { rvalue, .. } => {
                    if let Rvalue::Load(place) = rvalue {
                        note_place_slots(place, &mut observed);
                    }
                }
                // Deliberately *not* the store's own destination: a write nobody can
                // read is what this function exists to find.
                //
                // `Zero` is a **write**, so it belongs here with `Store` and not in the
                // observed set. Putting it there — the first draft of this change did —
                // marks the slot as read and pins every dead store to it, which stopped
                // `024-hello.jr`'s `Point` from being optimised away entirely. The program
                // stayed correct and the optimisation silently stopped happening, which is
                // what the optimized-MIR snapshot is for.
                // **A `TagCheck` *reads* the tag, so it observes its slot.** Grouping it with the
                // writes below — the first draft of this change did — made DCE see nothing reading the
                // variant and delete *both* the tag store and the value store, so a correct read
                // trapped with `tag=0`. The stores vanished silently and only the trap showed it,
                // which is the "well-typed placeholder" failure mode reached through a dead-code pass.
                Statement::TagCheck { place, .. } => note_place_slots(place, &mut observed),
                Statement::Store { .. }
                | Statement::Zero { .. }
                | Statement::BoundsCheck { .. }
                | Statement::Nop => {}
            }
        }
    }

    let mut changed = false;
    for index in 0..body.block_count() {
        for stmt in body.stmts_mut(BlockId::from_usize(index)) {
            // `Zero` is deleted on the same terms as `Store`: it writes a slot, so if
            // nothing can read that slot the write is dead. Leaving it out would keep a
            // zeroing whose slot every other pass had already dropped, and
            // `drop_unused_slots` would then keep the slot alive to hold it.
            let (Statement::Store { place, .. } | Statement::Zero { place, .. }) = stmt else {
                continue;
            };
            let PlaceBase::Slot(slot) = place.base else {
                continue;
            };
            if observed.contains(&slot) {
                continue;
            }
            *stmt = Statement::Nop;
            changed = true;
        }
    }
    changed
}

// ---------------------------------------------------------------------------
// Nops
// ---------------------------------------------------------------------------

/// Drops `Nop` statements.
///
/// ADR-0017 §1 kept the variant so a pass could delete a statement in O(1) without
/// shifting a block's later indices. This is the pass that pays that debt back: the
/// indices are no longer needed once every pass in a round has run.
fn drop_nops(body: &mut MirBody) -> bool {
    let mut changed = false;
    for index in 0..body.block_count() {
        let stmts = body.stmts_mut(BlockId::from_usize(index));
        let before = stmts.len();
        stmts.retain(|stmt| !matches!(stmt, Statement::Nop));
        changed |= stmts.len() != before;
    }
    changed
}

// ---------------------------------------------------------------------------
// Unused slots
// ---------------------------------------------------------------------------

/// Drops slots nothing stores to, loads from, or takes the address of.
///
/// This is the symptom `PLAN.md` §7 named: `print_line` in `modules/Basic` keeps a
/// spill slot it never reads. A slot is not an SSA value, so there is no definition
/// to trace — liveness is simply "mentioned by some surviving place".
fn drop_unused_slots(body: &mut MirBody) -> bool {
    let mut used: FxHashSet<SlotId> = FxHashSet::default();
    for block in body.blocks() {
        for stmt in &block.stmts {
            match stmt {
                Statement::Assign { rvalue, .. } | Statement::Discard { rvalue, .. } => {
                    note_rvalue_slots(rvalue, &mut used);
                }
                // **`TagCheck` belongs here, not with `BoundsCheck`**: it carries a `Place`, so the
                // slot that place names is observed. Grouping it with the operand-only checks dropped
                // the slot and then panicked in `remap_place_slots` with "a live place named a dropped
                // slot" — the verifier catching an omission rather than a wrong answer, which is what
                // it is for.
                Statement::Store { place, .. }
                | Statement::Zero { place, .. }
                | Statement::TagCheck { place, .. } => note_place_slots(place, &mut used),
                Statement::BoundsCheck { .. } => {}
                Statement::Nop => {}
            }
        }
    }
    let keep: Vec<bool> = (0..body.slot_count())
        .map(|index| used.contains(&SlotId::from_usize(index)))
        .collect();
    if keep.iter().all(|k| *k) {
        return false;
    }
    body.retain_slots(&keep);
    true
}

fn note_place_slots(place: &Place, used: &mut FxHashSet<SlotId>) {
    match &place.base {
        PlaceBase::Slot(slot) => {
            used.insert(*slot);
        }
        PlaceBase::Deref(_) => {}
    }
}

fn note_rvalue_slots(rvalue: &Rvalue, used: &mut FxHashSet<SlotId>) {
    match rvalue {
        Rvalue::Load(place) | Rvalue::Address(place) => note_place_slots(place, used),
        // An atomic reaches memory through a *pointer operand*, never through a `Place` — so it names no
        // slot directly. The slot it points into is kept alive by whatever produced that pointer, which is
        // an `Address` and is already handled above.
        Rvalue::Use(_)
        | Rvalue::Binary { .. }
        | Rvalue::Unary { .. }
        | Rvalue::Convert { .. }
        | Rvalue::Call { .. }
        | Rvalue::Atomic { .. }
        | Rvalue::Undef => {}
    }
}
