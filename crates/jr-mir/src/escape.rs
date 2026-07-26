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
fn is_register_representable(pool: &Pool, ty: PoolId) -> bool {
    match pool.item(ty) {
        // Register-representable: a bit pattern of fixed, small width.
        Item::BoolType | Item::IntType { .. } | Item::PointerType(_) => true,

        // Every aggregate, wide, or non-value item lives in memory. `StringType`
        // is here deliberately: ADR-0004 makes `string` a two-word
        // `{data: *u8, count: s64}` pair, which is not one register's worth.
        Item::VoidType
        | Item::StringType
        | Item::TypeType
        | Item::ErrorType
        | Item::ForeignLibraryType
        | Item::StructType { .. }
        | Item::ProcType { .. }
        | Item::VoidValue
        | Item::BoolValue(_)
        | Item::IntValue { .. }
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
        UnOp::Neg | UnOp::Not => false,
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
fn addr_taken_locals(body: &Body) -> FxHashSet<LocalId> {
    let mut escaped = FxHashSet::default();

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
            Stmt::Break(_) | Stmt::Continue(_) | Stmt::Error(_) => {}
        }
    }

    while let Some((expr_id, under_addr_of)) = expr_worklist.pop() {
        match body.expr(expr_id) {
            Expr::Name { res, .. } => {
                if under_addr_of {
                    if let Res::Local(local) = res {
                        escaped.insert(*local);
                    }
                }
            }
            Expr::Binary { lhs, rhs, .. } => {
                expr_worklist.push((*lhs, under_addr_of));
                expr_worklist.push((*rhs, under_addr_of));
            }
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
            Expr::Literal(_, _) | Expr::Uninit(_) | Expr::Directive { .. } | Expr::Error(_) => {}
        }
    }

    escaped
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
    let escaped = addr_taken_locals(body);
    let flags = (0..body.locals.len())
        .map(LocalId::from_usize)
        .map(|local| {
            !escaped.contains(&local)
                && types
                    .local_type(body_id, local)
                    .is_some_and(|ty| is_register_representable(pool, ty))
        })
        .collect();
    Promotable { flags }
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
