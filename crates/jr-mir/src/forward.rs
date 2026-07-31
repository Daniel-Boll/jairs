//! Store-to-load forwarding: replacing a load with the value a store just put there.
//!
//! [ADR-0023](../../../docs/adr/0023-store-to-load-forwarding.md) is this module's
//! specification, and ADR-0022's own Context is the reason it exists: the mid-end
//! folded nothing in `PLAN.md` §1.4's exit criterion, because `p` is a `struct` and
//! so lives in a slot, and nothing saw through memory.
//!
//! # Why block-local is enough for the case that motivated it
//!
//! Every store and its matching load in `024-hello.jr` sit in one block:
//!
//! ```text
//!   bb0():
//!     store s0.0 <- 4_s64
//!     store s0.1 <- 5_s64
//!     v0: s64 = load s0.0     ← forwardable
//!     v1: s64 = load s0.1     ← forwardable
//!     goto bb12(v0, v1)
//! ```
//!
//! Forwarding those two is what lets ADR-0022's block-parameter collapse see
//! constants on the edge, which lets the fold produce `9`, which lets the branch
//! collapse, which lets [`crate::dce`] delete the untaken arm and then the slots. One
//! forward walk per block unlocks the whole chain; a dataflow analysis would unlock
//! loops as well, and ADR-0023 §1 defers it deliberately.
//!
//! # The two rules that make this sound
//!
//! **Identical projection paths, never merely overlapping ones.**
//! `modules/Basic`'s `print` is `store s0 <- v0` then `v1: *u8 = load s0.data`. The
//! store supplies the whole aggregate and the load wants one field; MIR has no rvalue
//! that extracts a field from a *value* — [`Projection::Field`] applies to a
//! [`Place`], never to an [`Operand`] — so there is nothing to forward. A prefix
//! relation is therefore a **kill**, not a match: the two do share storage, so
//! treating them as unrelated would forward a stale value.
//!
//! **Two distinct projection steps are disjoint storage, and that is not a layout
//! claim.** `s0.0` and `s0.1` do not overlap because a struct's fields are distinct,
//! not because of where they sit. Nothing here asks for a size or an offset, so
//! ADR-0017 §5 holds and `PLAN.md` §7's first Trap is avoided.
//!
//! # Why an address-taken slot is not simply refused
//!
//! Because `024-hello.jr` takes `*sum` — but in `bb7`, four blocks *after* the
//! store-load pair in `bb11`. Refusing the whole slot would decline the pair that
//! completes the cascade, which is exactly the mistake ADR-0022 §4's first draft made
//! one ADR earlier. So the address-taken guard is applied to the *interval* between
//! the store and the load, not to the body.
//!
//! For a slot whose address is never taken anywhere, a call and an indirect store
//! cannot touch it, because no pointer to it exists. That is the same argument
//! [`crate::dce`] uses to drop a dead store, and [`slot_address_taken`] is the shared
//! predicate rather than two copies of one idea.

use rustc_hash::FxHashSet;

use jr_pool::{Item, Pool, PoolId};

use crate::mir::{
    BlockId, MirBody, Operand, Place, PlaceBase, Projection, Rvalue, SlotId, Statement,
};
use crate::verify;

/// Replaces loads with the values stores put there, and reports whether it changed anything.
///
/// # Panics
/// In a debug build, if the result is malformed. That is the point.
pub fn forward_stores(body: &mut MirBody, pool: &Pool) -> bool {
    let escaping = escaping_slots(body);
    let mut changed = false;
    for index in 0..body.block_count() {
        changed |= forward_in_block(body, BlockId::from_usize(index), &escaping, pool);
    }
    if changed {
        verify::assert_valid(body, pool);
    }
    changed
}

/// Every slot whose address is taken somewhere in the body.
///
/// Shared with [`crate::dce`]'s dead-store elimination, which needs the same fact for
/// the same reason: a slot no pointer names cannot be reached indirectly, so neither a
/// call nor a store through a `Deref` can observe or disturb it. One function rather
/// than two copies of one idea, because the two passes would otherwise be free to
/// disagree about what escaping means.
pub(crate) fn escaping_slots(body: &MirBody) -> FxHashSet<SlotId> {
    let mut out = FxHashSet::default();
    for block in body.blocks() {
        for stmt in &block.stmts {
            let rvalue = match stmt {
                Statement::Assign { rvalue, .. } | Statement::Discard { rvalue, .. } => rvalue,
                Statement::Store { .. }
                | Statement::Zero { .. }
                | Statement::BoundsCheck { .. }
                | Statement::TagCheck { .. }
                | Statement::Nop => continue,
            };
            // Only `Address` escapes a slot. A `Load` reads it, a `Store` writes it,
            // and neither hands out a pointer that outlives the statement.
            if let Rvalue::Address(place) = rvalue
                && let PlaceBase::Slot(slot) = place.base
            {
                out.insert(slot);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// One block
// ---------------------------------------------------------------------------

/// How two participating places on the same slot relate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Overlap {
    /// The same storage, exactly. The only case that forwards.
    Same,
    /// Shared storage, but not the same extent — one path is a strict prefix of the
    /// other. Refused, because MIR cannot extract a field from a value (ADR-0023 §3).
    Partial,
    /// Provably different storage: the paths diverge at some step.
    Disjoint,
}

fn forward_in_block(
    body: &mut MirBody,
    block: BlockId,
    escaping: &FxHashSet<SlotId>,
    pool: &Pool,
) -> bool {
    // Collected first, because rewriting needs `&mut` and deciding needs `&`.
    let mut rewrites: Vec<(usize, Operand)> = Vec::new();
    let stmts = &body.block(block).stmts;

    for (position, stmt) in stmts.iter().enumerate() {
        let Statement::Assign {
            rvalue: Rvalue::Load(load),
            ..
        } = stmt
        else {
            continue;
        };
        let Some(slot) = participating_slot(load) else {
            continue;
        };
        if let Some(value) = available_store(stmts, position, load, slot, escaping, pool, body) {
            rewrites.push((position, value));
        }
    }

    if rewrites.is_empty() {
        return false;
    }
    for (position, value) in rewrites {
        if let Statement::Assign { rvalue, .. } = &mut body.stmts_mut(block)[position] {
            *rvalue = Rvalue::Use(value);
        }
    }
    true
}

/// The value a store made available at `load`, searching backwards from `position`.
///
/// Backwards rather than forwards because the *nearest* preceding store is the one
/// that wins, and because the first kill encountered on the way back ends the search —
/// which is the flow-sensitivity ADR-0023 §2 asks for, expressed as a walk rather than
/// as a set that has to be maintained.
fn available_store(
    stmts: &[Statement],
    position: usize,
    load: &Place,
    slot: SlotId,
    escaping: &FxHashSet<SlotId>,
    pool: &Pool,
    body: &MirBody,
) -> Option<Operand> {
    let escapes = escaping.contains(&slot);
    for earlier in stmts[..position].iter().rev() {
        match earlier {
            Statement::Store { place, value, .. } => {
                match store_relation(place, load, slot, pool, body) {
                    Some(Overlap::Same) => return Some(*value),
                    // Shares storage without matching it, so the load's bytes are not
                    // this operand — and no earlier store can be trusted either.
                    Some(Overlap::Partial) => return None,
                    Some(Overlap::Disjoint) => {}
                    // A store this pass cannot reason about: through a pointer, or to
                    // a place with a `Deref` in its projection. Safe only if nothing
                    // can point at our slot.
                    None => {
                        if escapes {
                            return None;
                        }
                    }
                }
            }
            Statement::Assign { rvalue, .. } | Statement::Discard { rvalue, .. } => {
                if kills(rvalue, slot, escapes) {
                    return None;
                }
            }
            // Zeroing writes the *whole* slot, so it kills any earlier store to it and
            // is not itself a source to forward from: MIR cannot extract one element of a
            // zeroed aggregate as a value, which is the same limitation that makes a
            // whole-slot store refuse to feed a field load (ADR-0023 §3).
            Statement::Zero { place, .. } => match participating_slot(place) {
                Some(zeroed) if zeroed != slot => {}
                // Our slot, or a place this pass cannot reason about.
                _ => return None,
            },
            // Reads its operands and may trap. It writes nothing, so it cannot invalidate
            // a store — a trap does not produce a *wrong* value, it ends the program.
            Statement::BoundsCheck { .. } | Statement::TagCheck { .. } => {}
            Statement::Nop => {}
        }
    }
    None
}

/// Whether an rvalue between a store and a load invalidates the forwarding.
fn kills(rvalue: &Rvalue, slot: SlotId, escapes: bool) -> bool {
    match rvalue {
        // Taking the address makes a pointer that could be used before the load — by a
        // call in this same block, for instance. Killing here is what lets the guard
        // above be about the *interval* rather than the whole body.
        Rvalue::Address(place) => matches!(place.base, PlaceBase::Slot(s) if s == slot),
        // A call can write through any pointer it was given, so it kills a slot that
        // has a pointer and cannot touch one that does not.
        Rvalue::Call { .. } => escapes,
        // A load has no effect. Everything else is arithmetic on values.
        // A conversion reads a value and writes a value; it cannot reach memory.
        Rvalue::Load(_)
        | Rvalue::Use(_)
        | Rvalue::Binary { .. }
        | Rvalue::Unary { .. }
        | Rvalue::Convert { .. }
        | Rvalue::Undef => false,
    }
}

/// How a store's place relates to a load's, or `None` if the store is unanalysable.
fn store_relation(
    store: &Place,
    load: &Place,
    slot: SlotId,
    pool: &Pool,
    body: &MirBody,
) -> Option<Overlap> {
    let stored = participating_slot(store)?;
    if stored != slot {
        return Some(Overlap::Disjoint);
    }
    Some(compare_paths(
        &store.projection,
        &load.projection,
        body.slot(slot).ty,
        pool,
    ))
}

/// The slot a place names, if this pass can reason about the place at all.
///
/// `None` for a place based on a `Deref`, and for one whose projection contains a
/// `Deref` step: both name memory somewhere else, whatever the base was.
fn participating_slot(place: &Place) -> Option<SlotId> {
    let PlaceBase::Slot(slot) = place.base else {
        return None;
    };
    if place
        .projection
        .iter()
        .any(|step| matches!(step, Projection::Deref))
    {
        return None;
    }
    Some(slot)
}

/// Whether `ty` is a union *or a variant*, whose fields all share one offset (ADR-0045 §3,
/// ADR-0068 §3).
///
/// **A variant must answer `true` here**, and getting it wrong would be a silent miscompile rather
/// than an error: a variant's cases overlap exactly as a union's do — they share the payload offset —
/// so two reads of *different* cases alias. Answering `false` would let forwarding treat `v.i` and
/// `v.f` as disjoint and forward a stale store across them, which is the "well-typed placeholder"
/// class of bug PLAN §5 names, reached through an omitted match arm instead.
///
/// A variant's leading tag does not change this: the tag is not a case, and no `Projection::Field`
/// names it (ADR-0068 §5 keeps it unspellable).
fn is_overlapping_aggregate(ty: PoolId, pool: &Pool) -> bool {
    ty.index() < pool.len()
        && matches!(
            pool.item(ty),
            Item::UnionType { .. } | Item::VariantType { .. }
        )
}

/// The type one projection step lands on, for tracking the receiver type along a path.
///
/// Conservative: an unresolvable step yields [`PoolId::ERROR`], which `is_overlapping_aggregate` answers
/// `false` for — so a path this cannot follow falls back to the struct rule. That is the safe
/// direction only because a *later* unequal pair on an unknown type is then treated as
/// disjoint, which is why this walks every step rather than only the ones before a difference.
fn step_type(ty: PoolId, step: &Projection, pool: &Pool) -> PoolId {
    if ty.index() >= pool.len() {
        return PoolId::ERROR;
    }
    match step {
        Projection::Field(index) => match pool.item(ty) {
            Item::StructType { decl } | Item::UnionType { decl } | Item::VariantType { decl } => {
                pool.struct_fields(*decl)
                    .and_then(|fields| fields.get(*index as usize))
                    .map_or(PoolId::ERROR, |field| field.ty)
            }
            _ => PoolId::ERROR,
        },
        Projection::Index(_) => match pool.item(ty) {
            Item::ArrayType { elem, .. } | Item::PointerType(elem) => *elem,
            _ => PoolId::ERROR,
        },
        // A `Deref` never reaches here: `participating_slot` refuses a place containing one.
        Projection::Deref => PoolId::ERROR,
        Projection::StringData | Projection::ViewData => PoolId::ERROR,
        Projection::StringCount | Projection::ViewCount => PoolId::S64,
        // The tag is a `u8` and is not a case, so a path through it lands on neither (ADR-0068 §3).
        Projection::VariantTag => PoolId::U8,
    }
}

/// Compares two projection paths on the same slot.
///
/// Step by step, and the first difference means disjoint — which is a claim about a
/// struct having distinct fields, not about where they sit (ADR-0023 §3). `.data` and
/// `.count` are distinct steps and so are disjoint for the same reason.
///
/// # Why an index needs its own case
///
/// Two *different* `Projection::Index` steps are **not** disjoint. `buf[i]` and `buf[j]`
/// name the same element whenever `i == j` at run time, and this pass cannot know that
/// they differ — so the "first difference means disjoint" rule, which is sound for
/// fields because a struct's fields are distinct by construction, would forward the
/// wrong value:
///
/// ```text
///   store buf[i] <- 1
///   store buf[j] <- 2
///   v = load buf[i]      // NOT 1 when i == j
/// ```
///
/// Only two *identical* index operands are the same storage. Anything else is
/// [`Overlap::Partial`] — "shares storage, extent unknown" — which refuses rather than
/// deciding, and is the conservative answer this pass already has a name for.
fn compare_paths(store: &[Projection], load: &[Projection], root: PoolId, pool: &Pool) -> Overlap {
    let mut ty = root;
    for (left, right) in store.iter().zip(load) {
        if left == right {
            ty = step_type(ty, left, pool);
            continue;
        }
        // A field and an index cannot both be steps on the same type, so an unequal pair
        // where *either* side is an index is two indices — but matching both sides
        // explicitly keeps that from being an assumption a later variant can falsify.
        match (left, right) {
            (Projection::Index(_), _) | (_, Projection::Index(_)) => return Overlap::Partial,
            // **Two different fields of a union share storage.** The "first difference means
            // disjoint" rule is a claim about a *struct* having distinct fields, and a union
            // falsifies it: every field is at offset 0 (ADR-0045 §3), so
            //
            //     store m.word <- 0
            //     store m.byte <- 7
            //     v = load m.word      // NOT 0
            //
            // would forward the stale wide store over the narrow one. This was a live wrong
            // answer — the corpus program read 0 where 7 was written — and it is the same
            // shape as the index case above: storage shared, extent unknown, so `Partial`
            // refuses rather than deciding.
            (Projection::Field(_), Projection::Field(_)) if is_overlapping_aggregate(ty, pool) => {
                return Overlap::Partial;
            }
            _ => return Overlap::Disjoint,
        }
    }
    if store.len() == load.len() {
        Overlap::Same
    } else {
        // One is a strict prefix of the other: they share storage but not extent.
        Overlap::Partial
    }
}
