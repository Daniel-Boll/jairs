//! Constant propagation: fold, substitute, collapse a single-valued block parameter, fold a branch.
//!
//! [ADR-0022](../../../docs/adr/0022-dce-constprop-shared-arithmetic.md) §5 is this
//! module's specification.
//!
//! # Why four transformations and not one
//!
//! Folding an operation whose operands are already constants would be a **no-op on
//! the case this pass exists for**. After ADR-0021's splice, `024-hello.jr`'s `main`
//! reads
//!
//! ```text
//!     goto bb12(v0, v1)
//!   bb12(v11: s64, v12: s64):
//!     v13: s64 = v11 + v12
//! ```
//!
//! The constants arrive as *edge arguments*, so a fold-only pass sees two
//! `Operand::Value`s and declines. Collapsing a block parameter whose every
//! predecessor supplies the same constant is what turns that into something foldable,
//! and folding the branch it then feeds is what lets [`crate::dce`] delete the arm
//! that cannot run. Each of the four exists because the one before it produces work
//! for it.
//!
//! # Where the arithmetic comes from, and why not from here
//!
//! From `jr-pool` — [`jr_pool::int_binary`], [`jr_pool::int_compare`],
//! [`jr_pool::int_negate`] — which is the same code `jr-vm`'s interpreter runs.
//! ADR-0022 §2 moved it there rather than letting this module have its own, and the
//! reason is specific: a fold happens at *compile* time and bakes its answer into a
//! `PoolId` that **both** engines then consume. A disagreement with the interpreter
//! would not show up as two engines disagreeing; it would show up as two engines
//! agreeing on the wrong constant, which `differential.rs` cannot see. That is the
//! one failure shape `PLAN.md` §3.1's invariant exists to prevent.
//!
//! # A fold that would trap is not performed
//!
//! The statement is left exactly as it was, so the trap happens at run time with the
//! location ADR-0020 gives it. Turning it into a compile-time diagnostic is a
//! *language* decision — whether `MAX + 1` in unreachable-ish code is an error — and
//! nothing in Jairs-0 has taken it. Folding it to some value would be a miscompile,
//! and folding it to a trap terminator would silently change which diagnostic a
//! program gets.
//!
//! # What it does not do
//!
//! No lattice, no reachability worklist: this is not SCCP. ADR-0022 §5 rejected that
//! for a specific reason rather than for size — SCCP discovers unreachability itself,
//! which would mean it and [`crate::dce`] both delete blocks, and two passes editing
//! the same structure is how an ordering bug becomes a miscompile rather than a
//! missed optimisation.

use jr_pool::{IntKind, Item, Pool, PoolId};

use crate::mir::{
    BinOp, BlockId, Callee, MirBody, Operand, Place, PlaceBase, Rvalue, Statement, Target,
    Terminator, UnOp, ValueId,
};
use crate::verify;

/// Folds and propagates constants, and reports whether anything changed.
///
/// Needs `&mut Pool` because a folded result is a *new* interned value: ADR-0015 keys
/// an integer value on its type as well as its bits, so `4 + 5` at `s64` is a pool
/// entry that may not exist yet.
///
/// # Panics
/// In a debug build, if the result is malformed. That is the point.
pub fn const_prop(body: &mut MirBody, pool: &mut Pool) -> bool {
    let mut changed = collapse_block_params(body);
    changed |= substitute_and_fold(body, pool);
    changed |= fold_branches(body, pool);
    if changed {
        verify::assert_valid(body, pool);
    }
    changed
}

// ---------------------------------------------------------------------------
// Reading a constant
// ---------------------------------------------------------------------------

/// The mathematical value of a constant integer, with its kind.
fn as_int(pool: &Pool, id: PoolId) -> Option<(IntKind, i128)> {
    let Item::IntValue { ty, bits } = pool.item(id) else {
        return None;
    };
    let kind = IntKind::of(pool, *ty)?;
    Some((kind, kind.decode(*bits)))
}

/// The value of a constant `bool`.
fn as_bool(pool: &Pool, id: PoolId) -> Option<bool> {
    match pool.item(id) {
        Item::BoolValue(value) => Some(*value),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Collapsing a single-valued block parameter
// ---------------------------------------------------------------------------

/// Replaces a block parameter that every predecessor supplies the same constant for.
///
/// The parameter is dropped, every incoming `Target::args` entry at the same position
/// is dropped with it, and every use of the parameter becomes the constant. The
/// parameter list and the argument lists must move together — which is exactly what
/// the verifier's edge-arity check exists to catch, so a mistake here is a debug
/// assertion rather than a wrong answer.
///
/// The entry block is skipped: its parameters are the procedure's parameters
/// (`MirBody::params`), and its only "predecessor" is the caller, whose arguments this
/// body cannot see. Collapsing one would change the procedure's signature.
fn collapse_block_params(body: &mut MirBody) -> bool {
    let mut changed = false;
    // One parameter per pass over the body, because dropping a parameter shifts the
    // positions of the ones after it in every predecessor's argument list, and
    // recomputing is cheaper to reason about than adjusting indices in flight.
    loop {
        let Some((block, position, constant)) = find_collapsible_param(body) else {
            return changed;
        };
        let param = body.block(block).params[position];

        {
            let blocks = body.blocks_mut();
            blocks[block.index()].params.remove(position);
            for data in blocks.iter_mut() {
                for target in edge_targets_mut(&mut data.term) {
                    if target.block == block {
                        target.args.remove(position);
                    }
                }
            }
        }
        substitute_value(body, param, Operand::Constant(constant));
        changed = true;
    }
}

/// A block parameter every predecessor supplies one identical constant for.
fn find_collapsible_param(body: &MirBody) -> Option<(BlockId, usize, PoolId)> {
    let entry = body.entry();
    let predecessors = body.predecessors().to_vec();
    for (index, preds) in predecessors.iter().enumerate() {
        let block = BlockId::from_usize(index);
        if block == entry {
            continue;
        }
        // No predecessor at all means the block is unreachable, which is `dce`'s
        // business: collapsing a parameter to "every one of no constants" would be
        // vacuously true and would produce an unsound substitution.
        if preds.is_empty() {
            continue;
        }
        let params = body.block(block).params.len();
        for position in 0..params {
            let mut agreed: Option<PoolId> = None;
            let mut ok = true;
            for pred in preds {
                for target in body.block(*pred).term.targets() {
                    if target.block != block {
                        continue;
                    }
                    match target.args.get(position) {
                        Some(Operand::Constant(id)) if agreed.is_none_or(|a| a == *id) => {
                            agreed = Some(*id);
                        }
                        _ => ok = false,
                    }
                }
            }
            if ok && let Some(constant) = agreed {
                return Some((block, position, constant));
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Substituting and folding
// ---------------------------------------------------------------------------

/// Substitutes every value defined as `Use(Constant)`, then folds what that enables.
fn substitute_and_fold(body: &mut MirBody, pool: &mut Pool) -> bool {
    let mut changed = false;

    // Substitution first, so that a fold sees the constants it produced.
    let constants = constant_definitions(body);
    for (value, constant) in constants {
        substitute_value(body, value, Operand::Constant(constant));
        changed = true;
    }

    for index in 0..body.block_count() {
        let block = BlockId::from_usize(index);
        // Collected rather than folded in place: folding needs `&mut Pool`, and the
        // statement list is borrowed from the body.
        let folds: Vec<(usize, PoolId)> = body
            .block(block)
            .stmts
            .iter()
            .enumerate()
            .filter_map(|(position, stmt)| {
                let Statement::Assign { dest, rvalue, .. } = stmt else {
                    return None;
                };
                fold(pool, rvalue, body.value(*dest).ty).map(|value| (position, value))
            })
            .collect();
        if folds.is_empty() {
            continue;
        }
        for (position, value) in folds {
            if let Statement::Assign { rvalue, .. } = &mut body.stmts_mut(block)[position] {
                *rvalue = Rvalue::Use(Operand::Constant(value));
            }
            changed = true;
        }
    }
    changed
}

/// Every value whose single definition is `Use(Constant)`.
///
/// SSA is what makes this a map rather than a dataflow problem: a value is defined
/// exactly once (ADR-0017 §1, checked by the verifier), so its definition *is* its
/// value everywhere.
fn constant_definitions(body: &MirBody) -> Vec<(ValueId, PoolId)> {
    let mut out = Vec::new();
    for block in body.blocks() {
        for stmt in &block.stmts {
            if let Statement::Assign {
                dest,
                rvalue: Rvalue::Use(Operand::Constant(id)),
                ..
            } = stmt
            {
                out.push((*dest, *id));
            }
        }
    }
    out
}

/// The constant an rvalue evaluates to, if it does and if that does not trap.
///
/// `ty` is the destination's type, which is what the result must fit —
/// `jr_pool::int_binary` normalises into it, exactly as the interpreter does with the
/// destination register's kind.
fn fold(pool: &mut Pool, rvalue: &Rvalue, ty: PoolId) -> Option<PoolId> {
    match rvalue {
        Rvalue::Binary { op, lhs, rhs } => fold_binary(pool, *op, *lhs, *rhs, ty),
        Rvalue::Unary { op, operand } => fold_unary(pool, *op, *operand, ty),
        // `Use` of a constant is already folded; everything else either has no
        // constant answer or must not be given one here.
        Rvalue::Use(_)
        | Rvalue::Call { .. }
        | Rvalue::Load(_)
        | Rvalue::Address(_)
        | Rvalue::Undef => None,
    }
}

fn fold_binary(
    pool: &mut Pool,
    op: BinOp,
    lhs: Operand,
    rhs: Operand,
    ty: PoolId,
) -> Option<PoolId> {
    let (Operand::Constant(left), Operand::Constant(right)) = (lhs, rhs) else {
        return None;
    };

    // A comparison's operands carry the kind; its result is a `bool` and so must not
    // be normalised through `ty`. `BinOp::as_int_cmp` and `as_int_op` are what keep
    // that split from being a matter of care (ADR-0022 §2).
    if let Some(cmp) = op.as_int_cmp() {
        if let (Some((_, a)), Some((_, b))) = (as_int(pool, left), as_int(pool, right)) {
            return Some(pool.bool_value(jr_pool::int_compare(cmp, a, b)));
        }
        // Equality on a `bool` is the one non-integer comparison the VM defines; `<`
        // on pointers is deliberately not in the subset, so nothing else folds.
        return match (op, as_bool(pool, left), as_bool(pool, right)) {
            (BinOp::Eq, Some(a), Some(b)) => Some(pool.bool_value(a == b)),
            (BinOp::Ne, Some(a), Some(b)) => Some(pool.bool_value(a != b)),
            _ => None,
        };
    }

    let arith = op.as_int_op()?;
    let (_, a) = as_int(pool, left)?;
    let (_, b) = as_int(pool, right)?;
    let out = IntKind::of(pool, ty)?;
    // An operation that would trap is left alone: ADR-0022 §5. The trap then happens
    // at run time, where ADR-0020 gives it a location.
    let bits = jr_pool::int_binary(arith, out, a, b).ok()?;
    Some(pool.int_value(ty, bits))
}

fn fold_unary(pool: &mut Pool, op: UnOp, operand: Operand, ty: PoolId) -> Option<PoolId> {
    let Operand::Constant(id) = operand else {
        return None;
    };
    match op {
        UnOp::Not => {
            let value = as_bool(pool, id)?;
            Some(pool.bool_value(!value))
        }
        UnOp::Neg => {
            let (_, a) = as_int(pool, id)?;
            let out = IntKind::of(pool, ty)?;
            let bits = jr_pool::int_negate(out, a).ok()?;
            Some(pool.int_value(ty, bits))
        }
    }
}

// ---------------------------------------------------------------------------
// Folding a branch
// ---------------------------------------------------------------------------

/// Turns a `Branch` on a constant condition into a `Goto`.
///
/// This is what gives [`crate::dce`] something to remove: the untaken arm loses its
/// last predecessor and becomes unreachable.
fn fold_branches(body: &mut MirBody, pool: &Pool) -> bool {
    let mut rewrites: Vec<(BlockId, Target)> = Vec::new();
    for index in 0..body.block_count() {
        let block = BlockId::from_usize(index);
        let Terminator::Branch { cond, then_, else_ } = &body.block(block).term else {
            continue;
        };
        let Operand::Constant(id) = cond else {
            continue;
        };
        let Some(value) = as_bool(pool, *id) else {
            continue;
        };
        rewrites.push((block, if value { then_.clone() } else { else_.clone() }));
    }
    if rewrites.is_empty() {
        return false;
    }
    for (block, target) in rewrites {
        body.set_terminator(block, Terminator::Goto(target));
    }
    true
}

// ---------------------------------------------------------------------------
// Substitution
// ---------------------------------------------------------------------------

/// Replaces every *use* of `value` with `with`, leaving its definition alone.
///
/// The definition is left because removing it is [`crate::dce`]'s job and because
/// `Rvalue::Use(Constant)` is idempotent under this function — so a second round does
/// not loop.
fn substitute_value(body: &mut MirBody, value: ValueId, with: Operand) {
    let subst = |operand: &mut Operand| {
        if *operand == Operand::Value(value) {
            *operand = with;
        }
    };
    for block in body.blocks_mut() {
        for stmt in &mut block.stmts {
            match stmt {
                Statement::Assign { dest, rvalue, .. } => {
                    // Never rewrite the definition itself into a self-reference.
                    if *dest == value {
                        continue;
                    }
                    substitute_rvalue(rvalue, &subst);
                }
                Statement::Discard { rvalue, .. } => substitute_rvalue(rvalue, &subst),
                Statement::Store {
                    place, value: v, ..
                } => {
                    substitute_place(place, &subst);
                    subst(v);
                }
                Statement::Nop => {}
            }
        }
        match &mut block.term {
            Terminator::Goto(target) => target.args.iter_mut().for_each(&subst),
            Terminator::Branch { cond, then_, else_ } => {
                subst(cond);
                then_.args.iter_mut().for_each(&subst);
                else_.args.iter_mut().for_each(&subst);
            }
            Terminator::Return(operand) => {
                if let Some(operand) = operand {
                    subst(operand);
                }
            }
            Terminator::Unreachable(_) => {}
        }
    }
}

fn substitute_place(place: &mut Place, subst: &impl Fn(&mut Operand)) {
    match &mut place.base {
        PlaceBase::Slot(_) => {}
        PlaceBase::Deref(operand) => subst(operand),
    }
}

fn substitute_rvalue(rvalue: &mut Rvalue, subst: &impl Fn(&mut Operand)) {
    match rvalue {
        Rvalue::Use(operand) => subst(operand),
        Rvalue::Binary { op: _, lhs, rhs } => {
            subst(lhs);
            subst(rhs);
        }
        Rvalue::Unary { op: _, operand } => subst(operand),
        Rvalue::Call { callee, args } => {
            match callee {
                Callee::Direct(_) => {}
                Callee::Indirect(operand) => subst(operand),
            }
            args.iter_mut().for_each(subst);
        }
        Rvalue::Load(place) | Rvalue::Address(place) => substitute_place(place, subst),
        Rvalue::Undef => {}
    }
}

/// The edges leaving a terminator, mutably.
///
/// A private duplicate of `mir.rs`'s helper: that one is private because handing out
/// `&mut Target` generally would let a caller rewrite an edge without invalidating the
/// CFG cache, and here the caller is already inside a `blocks_mut` borrow, which
/// invalidated it.
fn edge_targets_mut(term: &mut Terminator) -> Vec<&mut Target> {
    match term {
        Terminator::Goto(target) => vec![target],
        Terminator::Branch {
            cond: _,
            then_,
            else_,
        } => vec![then_, else_],
        Terminator::Return(_) | Terminator::Unreachable(_) => Vec::new(),
    }
}
