//! Classifies every local as a candidate SSA value or a stack slot, so lowering never has to guess which.
//!
//! ADR-0017 §2 is the specification this module implements: a local is
//! **promotable** only when a full walk of the body finds no proof that its
//! address is taken and its type is one Cranelift can hold in a register.
//! Nothing here builds SSA — that is `ssa.rs`'s job, running Braun et al.'s
//! algorithm over whatever this module marks promotable. This module answers
//! one yes/no question per local, once, before construction begins.
//!
//! # Classification before construction, not `mem2reg` after it
//!
//! ADR-0017 §2 names the rejected alternative this module exists to avoid:
//! lower every local to a stack slot uniformly, then run our own `mem2reg` pass
//! afterward to promote the ones that never escaped. That needs a dominator
//! tree, dominance frontiers, phi insertion and renaming — machinery whose only
//! job would be to re-derive a fact one linear walk can already settle. Braun's
//! construction (`ssa.rs`) has to know *before* it starts whether a local will
//! ever need a `SlotId`, because it creates an incomplete phi the first time a
//! promotable local is read in a block whose predecessors are not yet sealed;
//! there is no later pass that un-promotes a local once construction has
//! assumed it was safe to keep in a register. Classifying first is what makes
//! that assumption never wrong.
//!
//! # Conservative by construction
//!
//! The default is memory. A local is promoted only when this module can
//! *prove* — by finding no [`jr_hir::UnOp::AddrOf`] anywhere in the body that
//! could name it, and by confirming its type fits a register — that promoting
//! it is safe. The two failure directions are not symmetric: under-promoting
//! costs a stack slot and a load/store pair that codegen happily emits;
//! over-promoting an address-taken local is a miscompile, because an SSA value
//! has no address and a `stack_addr` would have nothing to point at. Every
//! ambiguity in this module therefore resolves toward memory: an absent
//! [`TypeMap`] entry, an [`jr_pool::Item`] variant this module does not
//! recognise as register-representable, and an `AddrOf` operand of any shape
//! all answer "not promotable", never "promotable, probably".
//!
//! # What counts as an escape
//!
//! Per ADR-0011, prefix `*` is [`jr_hir::UnOp::AddrOf`] and postfix `.*` is
//! [`jr_hir::Expr::Deref`] — two different operators that look similar in
//! source. Only the former escapes a local; reading through a pointer never
//! does. An escape is also not limited to `*local`: `*local.field` and
//! `*local.*` both take the address of something reachable *from* `local`, so
//! this module treats every local mentioned anywhere within an `AddrOf`
//! operand's expression tree as escaped, however deep the projection chain is.
//! There is no flow sensitivity: a local whose address is taken only inside a
//! branch that never runs is still not promotable, because the classification
//! is a property of the syntax, not of any particular execution.
//!
//! # No [`jr_hir::ResolveMap`], and why that is safe here
//!
//! [`classify`]'s signature carries no `ResolveMap`. Deciding which local a name
//! reference names uses only the inline `res` field on [`jr_hir::Expr::Name`],
//! which `jr-hir`'s `lower_expr` pre-fills for every local and parameter
//! reference through its scope stack — a `Res::Local` is written only once that
//! local is actually in scope, before name resolution ever runs. That is a
//! narrower guarantee than the full `ResolveMap` gives (which also resolves
//! file-level and imported names, filled in by a later pass), but it is exactly
//! the slice this module needs, since escape analysis cares only about locals.
//! **If a future corpus case is found where a local reference's inline `res` is
//! stale or `Res::Error` where a `ResolveMap` lookup would have found the
//! local, that reference is silently not recognised as an escape of that
//! local** — a correctness gap, not a panic — and [`classify`]'s signature would
//! need to grow a `&ResolveMap` parameter to close it. Nothing in the exercised
//! corpus has shown this gap; it is recorded so the next reader does not have
//! to re-derive why the parameter is absent.

use jr_hir::{Body, BodyId, Expr, ExprId, FileHir, LocalId, Res, Stmt, UnOp};
use jr_pool::{Item, Pool, PoolId};
use jr_sema::TypeMap;
use rustc_hash::FxHashSet;

// ---------------------------------------------------------------------------
// Promotable
// ---------------------------------------------------------------------------

/// The outcome of classifying every local in one [`Body`].
///
/// A local not covered by this classification — one whose [`LocalId`] came
/// from a different body — is a programmer error in the caller, not something
/// this type can detect; see [`Promotable::is_promotable`].
pub(crate) struct Promotable {
    /// Whether each local may become an SSA value, indexed by [`LocalId`].
    ///
    /// Parallel to [`Body::locals`]: entry `i` answers for `LocalId::from_usize(i)`.
    flags: Vec<bool>,
    /// Parameters whose address is taken, and which therefore need a stack slot.
    ///
    /// Parameters are **not** locals — `MirBody::params` says so, and `jr-hir`'s `Body` does
    /// not store them — so they cannot share `flags`. They are carried here anyway because
    /// this is the pass that already walks for `AddrOf`, and a second walk in `build.rs`
    /// would be a second opinion about the same question.
    addr_taken_params: FxHashSet<jr_hir::ParamId>,
}

impl Promotable {
    /// Whether `local` may become an SSA value rather than a stack slot.
    ///
    /// # Panics
    /// Panics if `local` does not belong to the body this was computed from.
    #[must_use]
    pub(crate) fn is_promotable(&self, local: LocalId) -> bool {
        self.flags[local.index()]
    }

    /// Whether `param`'s address is taken, and so whether it must be spilled at entry.
    ///
    /// A scalar parameter used only by value stays a block parameter, which is the common
    /// case and costs nothing.
    #[must_use]
    pub(crate) fn param_needs_slot(&self, param: jr_hir::ParamId) -> bool {
        self.addr_taken_params.contains(&param)
    }

    /// How many locals were promoted. For tests and the dump.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn promoted_count(&self) -> usize {
        self.flags.iter().filter(|promotable| **promotable).count()
    }
}

// ---------------------------------------------------------------------------
// Register representability
// ---------------------------------------------------------------------------

/// Returns `true` if a value of type `ty` fits in a machine register.
///
/// Matched exhaustively over every [`Item`] variant, the same discipline
/// [`Item::is_type`] uses, so that a new variant is a compile error here rather
/// than a silent "not representable" that this module's conservative default
/// would then hide as an ordinary escape-free rejection.
///
/// # Panics
/// Panics if `ty` did not come from `pool` — see [`Pool::item`].
pub(crate) fn is_register_representable(pool: &Pool, ty: PoolId) -> bool {
    match pool.item(ty) {
        // Register-representable: a bit pattern of fixed, small width.
        // A float is register-representable: it is a fixed, small bit pattern, and both
        // targets have float registers. Adding it here is what lets a `float64` local be
        // promoted to SSA rather than spilled to a slot.
        // An enum is its backing integer at run time (ADR-0041 §3), so it lives in a
        // register like one. Classifying it as an aggregate would spill every enum local to
        // a slot for no reason.
        Item::BoolType
        | Item::IntType { .. }
        | Item::FloatType { .. }
        | Item::EnumType { .. }
        | Item::PointerType(_) => true,

        // Every aggregate, wide, or non-value item lives in memory. `StringType`
        // is here deliberately: ADR-0004 makes `string` a two-word
        // `{data: *u8, count: s64}` pair, which is not one register's worth.
        Item::VoidType
        | Item::StringType
        | Item::TypeType
        | Item::ErrorType
        | Item::ForeignLibraryType
        | Item::ArrayType { .. }
        // A view is two words, so it is no more register-representable than the `string` it
        // shares a layout with (ADR-0044 §1).
        | Item::ViewType { .. }
        | Item::ResultsType { .. }
        | Item::ContextType
        | Item::StructType { .. }
        | Item::VariantType { .. }
        | Item::UnionType { .. }
        | Item::ProcType { .. }
        | Item::VoidValue
        | Item::BoolValue(_)
        | Item::IntValue { .. }
        | Item::FloatValue { .. }
        | Item::StrValue(_)
        | Item::TypeValue(_)
        | Item::ProcValue { .. }
        | Item::ForeignLibraryValue(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Escape detection
// ---------------------------------------------------------------------------

/// Returns `true` if `op` is prefix `*` (address-of), the only [`UnOp`] that
/// takes an address rather than computing a value from one (ADR-0011).
///
/// Matched exhaustively, per the house rule against `matches!` where a variant
/// list would do, so that a future fourth [`UnOp`] variant is a compile error
/// here rather than a silently non-escaping operator.
fn is_addr_of(op: UnOp) -> bool {
    match op {
        UnOp::AddrOf => true,
        // `~` reads a value, never an address — the same as `-` and `!`.
        UnOp::Neg | UnOp::Not | UnOp::BitNot => false,
    }
}

/// Returns every local whose address is taken anywhere in `body`.
///
/// A single explicit-worklist walk from [`Body::root`], covering every
/// statement and every expression, so a deeply nested body cannot overflow the
/// compiler's own stack (the same discipline `mir.rs`'s `reverse_postorder`
/// uses). The expression worklist carries one extra bit per entry: whether that
/// expression is reachable by descending through an [`UnOp::AddrOf`] operand.
/// Whenever a [`Res::Local`] is reached with that bit set, its local is
/// recorded as escaped — which is what makes `*local.field` and `*local.*`
/// escape `local` exactly as directly as `*local` does, without a second pass:
/// the bit, once set on entry to an `AddrOf` operand, stays set for every
/// descendant of that operand.
///
/// There is no flow sensitivity anywhere in this walk: every statement and
/// every expression the arena holds is visited once, regardless of whether a
/// branch containing it could ever run. That is deliberate — see the module
/// docs' "no flow sensitivity" argument.
/// The parameters whose address is taken somewhere in this body.
///
/// # Why this exists, and what went wrong without it
///
/// `build.rs` spilled an *aggregate* parameter to a slot so that `s.data` had a place to
/// project, and its comment recorded the failure that forced it: without the spill, lowering
/// "silently produced `Rvalue::Undef` — a `write` from a garbage pointer, with no diagnostic
/// anywhere".
///
/// A **scalar** parameter had the same hole and it was live. `place()` said a scalar parameter
/// has no place because "nothing in Jairs-0 can ask for its address" — but `*b` on a parameter
/// asks exactly that, so `place()` answered `None`, `unary`'s `AddrOf` fell back to
/// `Rvalue::Undef`, and the program read an unassigned value. `put_byte :: (b: u8) { p := *b; }`
/// is the case that surfaced it, and it predates this wave: nothing in the corpus took the
/// address of a parameter, so nothing noticed.
///
/// Collected by the same walk that finds escaping locals, tagged the same way.
fn addr_taken(body: &Body) -> (FxHashSet<LocalId>, FxHashSet<jr_hir::ParamId>) {
    let mut escaped = FxHashSet::default();
    let mut escaped_params: FxHashSet<jr_hir::ParamId> = FxHashSet::default();

    let mut stmt_worklist = vec![body.root];
    // One entry per expression still to visit, tagged with whether it is
    // reachable through an `AddrOf` operand.
    let mut expr_worklist: Vec<(ExprId, bool)> = Vec::new();

    while let Some(stmt_id) = stmt_worklist.pop() {
        match body.stmt(stmt_id) {
            Stmt::Block(stmts, _) => stmt_worklist.extend(stmts.iter().copied()),
            Stmt::Local(local, _) => {
                if let Some(init) = body.local(*local).init {
                    expr_worklist.push((init, false));
                }
            }
            // **A multi-result call's results are read through a place**, so the call's value is
            // dereferenced — which needs no *local* to escape, because the storage is the callee's
            // result slot rather than any variable here. The targets are ordinary writes.
            //
            // `false` for the call: it is not under an `AddrOf`, and its own aggregate-ness is what
            // gives it storage (ADR-0051 §1) rather than anything this walk decides.
            Stmt::LocalTuple { call, .. } => expr_worklist.push((*call, false)),
            Stmt::AssignTuple { targets, call, .. } => {
                for target in targets.iter().flatten() {
                    expr_worklist.push((*target, false));
                }
                expr_worklist.push((*call, false));
            }
            // Each returned value is an ordinary operand; the *aggregate* they are stored into is a
            // synthesised slot, which no local names and so nothing here can promote.
            Stmt::ReturnTuple(exprs, _) => {
                for expr in exprs {
                    expr_worklist.push((*expr, false));
                }
            }
            // Nothing constructs `Stmt::Item` today (`crates/jr-sema/src/check.rs`
            // treats it identically, at check.rs:171-173), but it is matched
            // explicitly rather than folded into a wildcard so the day something
            // does construct it, this walk fails to compile instead of silently
            // ignoring a nested declaration's body.
            Stmt::Item(_, _) => {}
            Stmt::Expr(expr, _) => expr_worklist.push((*expr, false)),
            Stmt::Assign { lhs, rhs, .. } => {
                expr_worklist.push((*lhs, false));
                expr_worklist.push((*rhs, false));
            }
            Stmt::If {
                cond, then, else_, ..
            } => {
                expr_worklist.push((*cond, false));
                stmt_worklist.push(*then);
                if let Some(else_id) = else_ {
                    stmt_worklist.push(*else_id);
                }
            }
            Stmt::While {
                cond,
                body: loop_body,
                ..
            } => {
                expr_worklist.push((*cond, false));
                stmt_worklist.push(*loop_body);
            }
            Stmt::Return(expr, _) => {
                if let Some(e) = expr {
                    expr_worklist.push((*e, false));
                }
            }
            // A `for`'s iterable reaches an **address**: the loop reads elements through a place,
            // so an array iterated over must be spilled exactly as one indexed by hand is. `true`
            // rather than `under_addr_of`, for the reason `Expr::Slice` uses it (ADR-0044 §2) — the
            // loop takes an address whether or not it sits under a `*`.
            Stmt::For {
                iterable,
                body: loop_body,
                ..
            } => {
                match iterable {
                    jr_hir::ForIterable::Sequence(e) => expr_worklist.push((*e, true)),
                    jr_hir::ForIterable::Range { start, end } => {
                        expr_worklist.push((*start, false));
                        expr_worklist.push((*end, false));
                    }
                }
                stmt_worklist.push(*loop_body);
            }
            Stmt::Defer(inner, _) => stmt_worklist.push(*inner),
            // A `push_context` block's statements are walked like any block's. The context copy it
            // introduces is a synthesised slot no *local* names, so nothing here promotes it — the
            // block's own locals are reached through `inner` (ADR-0063 §2).
            Stmt::PushContext(inner, _) => stmt_worklist.push(*inner),
            // A `switch`'s scrutinee and every arm's case value are ordinary operands, and each arm's
            // body is a block — nothing here takes an address, so nothing is promoted differently
            // (ADR-0067 §6).
            Stmt::Switch { value, arms, .. } => {
                expr_worklist.push((*value, false));
                for arm in arms {
                    if let Some(case) = arm.value {
                        expr_worklist.push((case, false));
                    }
                    stmt_worklist.push(arm.body);
                }
            }
            Stmt::Break(_, _) | Stmt::Continue(_, _) | Stmt::Error(_) => {}
        }
    }

    while let Some((expr_id, under_addr_of)) = expr_worklist.pop() {
        match body.expr(expr_id) {
            Expr::Name { res, .. } => {
                // **A promoted name escapes its base unconditionally** (ADR-0050 §2), and this is
                // not defence in depth — it is load-bearing. `x` where `using p: Point` is in
                // scope lowers to a *projection of `p`'s place*, and a register-held local has no
                // place at all. Without this the base would stay promotable and `res_place` would
                // ask `slot_for` for storage the escape analysis had decided did not exist.
                //
                // `true` rather than `under_addr_of`, for exactly the reason `Expr::Slice` uses it
                // (ADR-0044 §2): the projection needs an address whether or not a `*` is written,
                // and a walk that only counted syntactic `AddrOf` would miss it — which that ADR
                // records as a miscompile rather than a diagnostic.
                let effective = under_addr_of || matches!(res, Res::Promoted { .. });
                if effective {
                    // The *root* of a promoted chain is what needs storage, so an embedded
                    // promotion walks down to the binding it ultimately reaches.
                    let mut target = res;
                    while let Res::Promoted { base, .. } = target {
                        target = base;
                    }
                    match target {
                        Res::Local(local) => {
                            escaped.insert(*local);
                        }
                        // The parameter case, which is why this returns two sets.
                        Res::Param(param) => {
                            escaped_params.insert(*param);
                        }
                        // Unreachable: the loop above strips every `Promoted` layer, so `target`
                        // is never one. Listed rather than `_`-armed so that a future `Res`
                        // variant is a compile error here.
                        Res::Promoted { .. } | Res::Item(_) | Res::Imported(_, _) | Res::Error => {}
                    }
                }
            }
            Expr::Binary { lhs, rhs, .. } => {
                expr_worklist.push((*lhs, under_addr_of));
                expr_worklist.push((*rhs, under_addr_of));
            }
            // `*a[i]` takes the address of an *element*, which escapes the whole array —
            // MIR tracks address-taken-ness per slot, and there is one slot for the array.
            // The index is an ordinary value and never under the `*`, because `*a[i]` is
            // `*(a[i])`: the postfix binds tighter.
            Expr::Index { base, index, .. } => {
                expr_worklist.push((*base, under_addr_of));
                expr_worklist.push((*index, false));
            }
            // **`buf[]` escapes `buf`, and this arm is why the operator is explicit.**
            // A view's `data` word holds the address of its base's storage, so the base must
            // *have* storage — and a promoted local does not. `true` rather than
            // `under_addr_of`: the slice takes an address whether or not it sits under a `*`,
            // exactly as `UnOp::AddrOf` does.
            //
            // ADR-0044 §2 rejected an implicit array-to-view coercion partly on this: a
            // coercion takes an address at a site containing no `AddrOf`, this walk would not
            // see it, and the result would be a promoted local with no address for the view to
            // point at — a miscompile, not a diagnostic.
            Expr::Slice { base, .. } => expr_worklist.push((*base, true)),
            // A cast does not take an address, and the target is a type rather than an
            // expression, so `under_addr_of` passes through to the operand unchanged.
            Expr::Cast { operand, .. } => expr_worklist.push((*operand, under_addr_of)),
            // Same as a cast: reads a value, produces a value, takes no address.
            Expr::Autocast { operand, .. } => expr_worklist.push((*operand, under_addr_of)),
            Expr::Unary { op, operand, .. } => {
                let operand_under_addr_of = under_addr_of || is_addr_of(*op);
                expr_worklist.push((*operand, operand_under_addr_of));
            }
            Expr::Call { callee, args, .. } => {
                expr_worklist.push((*callee, under_addr_of));
                for arg in args {
                    expr_worklist.push((*arg, under_addr_of));
                }
            }
            Expr::Field { receiver, .. } => expr_worklist.push((*receiver, under_addr_of)),
            // Postfix `.*` (ADR-0011) is a read through a pointer, never an
            // escape by itself. The bit still propagates to `inner`: `*p.*`
            // (address-of applied to a dereference) reaches this arm with
            // `under_addr_of` already `true`, and `p` must still be treated as
            // escaped — see the module docs' over-approximation note.
            Expr::Deref(inner, _) => expr_worklist.push((*inner, under_addr_of)),
            Expr::Run(inner, _) => expr_worklist.push((*inner, under_addr_of)),
            // A bare `.RED` is a constant with no operand and no storage.
            // `context` is the hidden parameter's value; nothing escapes through reading it.
            Expr::Context(_) => {}
            Expr::Member { .. }
            | Expr::Literal(_, _)
            | Expr::Uninit(_)
            | Expr::Directive { .. }
            | Expr::Error(_) => {}
        }
    }

    (escaped, escaped_params)
}

// ---------------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------------

/// Classifies every local in `body`.
///
/// `_hir` is part of the frozen interface this function is called through, but
/// this implementation does not need it: every lookup below either walks
/// `body`'s own arenas (which never mix with `FileHir`'s — see the arena-trap
/// discussion in `jr-sema/src/ctx.rs`) or queries `types` by `(body_id, local)`
/// directly, with no expression lookup and therefore no [`jr_hir::ExprScope`]
/// involved. It is kept, unused, because `build.rs` is coded against this exact
/// signature; report to the wave owner if that turns out to be wrong.
pub(crate) fn classify(
    _hir: &FileHir,
    body: &Body,
    body_id: BodyId,
    types: &TypeMap,
    pool: &Pool,
) -> Promotable {
    let (escaped, escaped_params) = addr_taken(body);
    let flags = (0..body.locals.len())
        .map(LocalId::from_usize)
        .map(|local| {
            !escaped.contains(&local)
                && types
                    .local_type(body_id, local)
                    .is_some_and(|ty| is_register_representable(pool, ty))
        })
        .collect();
    Promotable {
        flags,
        addr_taken_params: escaped_params,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use jr_base::{FileId, Interner};
    use jr_hir::FileHir;
    use jr_pool::Pool;
    use jr_sema::TypeMap;

    use super::*;

    /// Runs the real front end (parse → lower → resolve → signatures → check)
    /// over `source` and returns everything [`classify`] needs.
    ///
    /// There is exactly one procedure body in every fixture this module tests,
    /// so it is always [`BodyId::from_usize(0)`] — bodies are pushed in the
    /// order lowering encounters them, and every fixture below declares its one
    /// procedure first.
    fn analyse(source: &str) -> (FileHir, BodyId, TypeMap, Pool) {
        let interner = Interner::new();
        let mut pool = Pool::new();
        let file = FileId::from_usize(0);

        let parsed = jr_syntax::parse(source, file);
        let (hir, _) = jr_hir::lower_file(&parsed, file, &interner);
        let (resolve, _) = jr_hir::resolve(&hir, &[], &interner);
        let sigs = jr_sema::file_signatures(&hir, file, &resolve, &[], &mut pool, &interner);
        let checked = jr_sema::check_file(
            &hir,
            file,
            &resolve,
            &sigs.signatures,
            &[],
            &mut pool,
            &interner,
        );

        let mut types = sigs.types;
        types.absorb(&checked.types);

        (hir, BodyId::from_usize(0), types, pool)
    }

    /// Like [`analyse`], but stops after signatures — so no body local is ever
    /// visited and every `local_type` lookup returns `None`. This is how the
    /// "not visited" case (ADR-0017 §Context, `jr-sema/src/map.rs`) is
    /// reproduced for a real local rather than an invented `LocalId`.
    fn analyse_without_checking_bodies(source: &str) -> (FileHir, BodyId, TypeMap, Pool) {
        let interner = Interner::new();
        let mut pool = Pool::new();
        let file = FileId::from_usize(0);

        let parsed = jr_syntax::parse(source, file);
        let (hir, _) = jr_hir::lower_file(&parsed, file, &interner);
        let (resolve, _) = jr_hir::resolve(&hir, &[], &interner);
        let sigs = jr_sema::file_signatures(&hir, file, &resolve, &[], &mut pool, &interner);

        (hir, BodyId::from_usize(0), sigs.types, pool)
    }

    #[test]
    fn a_plain_s64_local_is_promotable() {
        let (hir, body_id, types, pool) = analyse(
            "main :: () {
                x: s64 = 1;
            }",
        );
        let body = hir.body(body_id);
        let promotable = classify(&hir, body, body_id, &types, &pool);
        assert!(promotable.is_promotable(LocalId::from_usize(0)));
        assert_eq!(promotable.promoted_count(), 1);
    }

    #[test]
    fn slicing_a_local_marks_it_escaped() {
        // ADR-0044 §2's third point, pinned at the level where it is actually true. An array
        // is not register-representable, so `buf` would get a slot with or without this — the
        // assertion is therefore about the **escape set**, not about promotability, because
        // asserting the latter would pass even if the `Expr::Slice` arm were deleted.
        //
        // What it protects: a view's `data` word holds the address of its base's storage, so
        // if arrays ever become register-representable, an unmarked slice would promote a local
        // that has no address for the view to point at.
        let (hir, body_id, types, _pool) = analyse(
            "main :: () {
                buf: [2]s64;
                xs := buf[];
            }",
        );
        let _ = types;
        let body = hir.body(body_id);
        let (escaped, _params) = addr_taken(body);
        assert!(
            escaped.contains(&LocalId::from_usize(0)),
            "`buf[]` must escape `buf`, exactly as `*buf` does"
        );
    }

    #[test]
    fn an_address_taken_local_is_not_promotable() {
        let (hir, body_id, types, pool) = analyse(
            "main :: () {
                x: s64 = 1;
                p := *x;
            }",
        );
        let body = hir.body(body_id);
        let promotable = classify(&hir, body, body_id, &types, &pool);
        assert!(!promotable.is_promotable(LocalId::from_usize(0)));
    }

    #[test]
    fn a_struct_local_is_not_promotable() {
        let (hir, body_id, types, pool) = analyse(
            "Point :: struct {
                x: s64;
                y: s64;
            }
            main :: () {
                p: Point = ---;
            }",
        );
        let body = hir.body(body_id);
        let promotable = classify(&hir, body, body_id, &types, &pool);
        assert!(!promotable.is_promotable(LocalId::from_usize(0)));
    }

    #[test]
    fn a_string_local_is_not_promotable() {
        let (hir, body_id, types, pool) = analyse(
            "main :: () {
                s: string = \"hi\";
            }",
        );
        let body = hir.body(body_id);
        let promotable = classify(&hir, body, body_id, &types, &pool);
        assert!(!promotable.is_promotable(LocalId::from_usize(0)));
    }

    #[test]
    fn a_local_with_no_recorded_type_is_not_promotable() {
        let (hir, body_id, types, pool) = analyse_without_checking_bodies(
            "main :: () {
                x: s64 = 1;
            }",
        );
        let body = hir.body(body_id);
        assert_eq!(
            types.local_type(body_id, LocalId::from_usize(0)),
            None,
            "the fixture must prove `None`, not merely assume it"
        );
        let promotable = classify(&hir, body, body_id, &types, &pool);
        assert!(!promotable.is_promotable(LocalId::from_usize(0)));
    }

    #[test]
    fn a_local_read_through_postfix_deref_is_still_promotable_if_its_address_is_never_taken() {
        let (hir, body_id, types, pool) = analyse(
            "main :: () {
                p: *s64 = ---;
                v := p.*;
            }",
        );
        let body = hir.body(body_id);
        let promotable = classify(&hir, body, body_id, &types, &pool);
        assert!(promotable.is_promotable(LocalId::from_usize(0)));
    }
}
