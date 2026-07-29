//! Lowering from HIR to typed SSA, one procedure body at a time.
//!
//! # What this module decides, and what was decided for it
//!
//! ADR-0017 is the specification. This module implements §2 (SSA during
//! lowering, memory only for locals that escape) and §4 (a body that failed to
//! type-check is refused, not lowered). It reads every type from `jr-sema`'s
//! `TypeMap` and recomputes none of them, because two implementations of the same
//! typing rule are two chances to disagree.
//!
//! # Refusing rather than guessing
//!
//! [`lower_body`] returns `Err` before it builds anything, for a body it cannot
//! lower *honestly*. The list is in [`scan`], and three of its entries used to
//! refuse programs that are perfectly legal Jairs. ADR-0018 supplied what each was
//! missing, so each is now a *fallback* — taken only when the corresponding input
//! map is empty — rather than an unconditional refusal:
//!
//! - **`#run`.** ADR-0016 §4 gave `#run e` the type of `e` and no value, because
//!   the tempting lowering — treat it as an ordinary runtime `e` — is precisely the
//!   failure `PLAN.md` §3.1's invariant exists to prevent: it would make
//!   compile-time and runtime evaluation silently disagree. ADR-0018 §3 evaluates
//!   it in a `jr-db` query instead and passes the answer in, so a `#run` with a
//!   value lowers to that value and one without is still refused.
//! - **A reference to a file-level constant.** `jr-sema` records a constant's
//!   *type* but never its *value*. Same fix, same map: `MESSAGE :: "hi";` followed
//!   by `print(MESSAGE)` now emits the interned string.
//! - **A call to an imported procedure.** [`Callee::Direct`] used to name a bare
//!   [`ProcId`], an index into *this file's* `FileHir::procs`, so `Res::Imported`
//!   had nothing to lower to. ADR-0018 §5 widens it to a [`ProcRef`] and has
//!   `jr-db` resolve the name from the other file's *signatures* — never its body,
//!   which is what keeps ADR-0017 §3's rule that the built-MIR query has no
//!   cross-body dependencies intact, and ADR-0016 §5's rule that one file's
//!   analysis never triggers another's full check.
//!
//! What remains unconditionally refused is an imported *constant*: its value would
//! have to come from another file's const evaluation, which is the cross-body read
//! ADR-0017 §3 keeps out. Nothing in the corpus needs one.
//!
//! Every refusal is silent. The body is refused because an earlier phase already
//! reported the cause, or because the feature has not landed; either way a second
//! diagnostic on the same line is noise. This continues the discipline `jr-sema`
//! set, under which `PoolId::ERROR` flows without comment.
//!
//! # What the gate cannot see, and whose job that is
//!
//! The gate tests the `TypeMap` for `PoolId::ERROR`, because that is the only
//! error signal this crate is given. Not every error sema reports poisons a type:
//! `x: u8 = 300;` is E0204, and sema reports it and then carries on with `u8`, so
//! nothing here can tell that body apart from a correct one. `jr-mir` is a pure
//! function over HIR plus types and is handed no diagnostics to consult, so it
//! *cannot* close that hole.
//!
//! Closing it belongs to the caller: nothing may ask for the MIR of a file whose
//! `file_diagnostics` reports errors. ADR-0017 §4 records this as the one place
//! the "require the caller to check first" option it otherwise rejected is still
//! load-bearing, and `tests/lowering.rs` pins the division of responsibility so it
//! is not mistaken for an oversight.
//!
//! # Control flow, and the invariant that shapes it
//!
//! `verify.rs` enforces ADR-0017 §1's no-critical-edges rule, which is why an
//! `if` **always** gets two arm blocks even when the source has no `else`, and why
//! a `while` gets a separate pre-exit block. Letting the branch target the join
//! directly would be one block cheaper and would produce an edge from a
//! two-successor block into a two-predecessor block — exactly the shape that makes
//! placing parallel copies on an edge ambiguous when block parameters are lowered
//! to bytecode later.
//!
//! `&&` and `||` short-circuit, so they are control flow rather than operators:
//! MIR's [`crate::BinOp`] has no `And` or `Or` variant at all, which turns
//! "remember to short-circuit" into a fact the type system checks.
//!
//! # What it cannot know
//!
//! `jr-hir`'s `lower_bin_op`, `lower_un_op` and `lower_assign_op` fall back
//! silently to `Add`, `Neg` and `Assign` on an unrecognised token, emitting no
//! diagnostic. A recovered operator is therefore indistinguishable here from a
//! real one. Bodies containing recovered *syntax* are refused, which covers the
//! cases that matter, but the gap is real and is not this module's to close.

use jr_base::{FileId, Interner, Symbol};
use jr_hir::{
    AssignOp, Body, BodyId, ConstValue, Expr, ExprId, ExprScope, FileHir, ItemKind, Literal,
    LocalId, ParamId, ProcId, Res, ResolveMap, Stmt, StmtId,
};
use jr_pool::{Item, Pool, PoolId};
use jr_sema::{FileSignatures, TypeMap};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::escape::{self, Promotable};
use crate::inputs::{ConstValues, ImportedProcs, OperatorCalls};
use crate::mir::{
    BinOp, BlockId, Callee, Facts, FileMir, MirBody, MirSpan, NumKind, Operand, Place, Poisoned,
    ProcRef, Projection, Rvalue, SlotId, Statement, Target, Terminator, UnOp, Unreachable,
};
use crate::ssa::SsaBuilder;
use crate::verify;

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Lowers every procedure in a file that has a body.
///
/// Bodies are pushed in ascending [`ProcId`] order, so the result — and therefore
/// a dump of it — is deterministic without anything having to sort.
///
/// `consts` and `imports` are what ADR-0018 §3 and §5 added: the values `jr-sema`
/// could not compute and the cross-file callees it deliberately did not resolve.
/// Both may be empty, in which case lowering behaves exactly as ADR-0017 shipped
/// and refuses whatever needed them.
#[must_use]
pub fn lower_file(
    hir: &FileHir,
    resolve: &ResolveMap,
    types: &TypeMap,
    signatures: &FileSignatures,
    consts: &ConstValues,
    imports: &ImportedProcs,
    operators: &OperatorCalls,
    interner: &Interner,
    pool: &mut Pool,
) -> FileMir {
    let mut out = FileMir::new();
    for index in 0..hir.procs.len() {
        let proc = ProcId::from_usize(index);
        if hir.procs[index].body.is_none() {
            // A `#foreign` procedure has no body to lower, and is not a failure.
            continue;
        }
        let lowered = lower_body(
            hir, proc, resolve, types, signatures, consts, imports, operators, interner, pool,
        );
        out.push(proc, lowered);
    }
    out
}

/// Lowers one procedure body.
///
/// # Errors
/// Returns [`Poisoned`] when the body cannot be lowered honestly — see the module
/// docs for the list and the argument for each entry.
pub fn lower_body(
    hir: &FileHir,
    proc: ProcId,
    resolve: &ResolveMap,
    types: &TypeMap,
    signatures: &FileSignatures,
    consts: &ConstValues,
    imports: &ImportedProcs,
    operators: &OperatorCalls,
    interner: &Interner,
    pool: &mut Pool,
) -> Result<MirBody, Poisoned> {
    let proc_data = hir
        .procs
        .get(proc.index())
        .ok_or(Poisoned::Here("no such procedure"))?;
    let body_id = proc_data
        .body
        .ok_or(Poisoned::Here("the procedure has no body"))?;
    let body = hir
        .bodies
        .get(body_id.index())
        .ok_or(Poisoned::Here("the body is missing"))?;
    let sig = signatures
        .proc_sig(proc)
        .ok_or(Poisoned::Here("the signature failed to check"))?;

    if sig.ret == PoolId::ERROR {
        return Err(Poisoned::Here("the return type failed to resolve"));
    }
    if sig.params.contains(&PoolId::ERROR) {
        return Err(Poisoned::Here("a parameter type failed to resolve"));
    }

    let reach = Reach::of(body);
    if let Some(reason) = scan(
        hir, body, body_id, &reach, resolve, types, signatures, consts, imports,
    ) {
        return Err(Poisoned::Here(reason));
    }

    let ret = sig.ret;
    let params: Vec<PoolId> = sig.params.clone();
    let file = proc_data.span.file;

    let promotable = escape::classify(hir, body, body_id, types, pool);
    let mut lower = Lower {
        hir,
        body,
        body_id,
        file,
        resolve,
        types,
        consts,
        imports,
        operators,
        interner,
        pool,
        mir: MirBody::new(ProcRef::new(file, proc), ret),
        ssa: SsaBuilder::new(),
        promotable,
        slots: FxHashMap::default(),
        params: FxHashMap::default(),
        param_slots: FxHashMap::default(),
        current: None,
        loops: Vec::new(),
        defers: Vec::new(),
        stray: Vec::new(),
        failed: None,
        ret,
    };
    lower.run(proc, &params, body.root);
    lower.finish()
}

// ---------------------------------------------------------------------------
// Reachability
// ---------------------------------------------------------------------------

/// The statements and expressions reachable from a body's root.
///
/// Both arenas can hold nodes nothing refers to — `jr-hir` allocates as it walks
/// the CST and error recovery abandons partial subtrees — so scanning the arenas
/// wholesale would refuse bodies for the sake of a node no execution can reach.
struct Reach {
    /// Reachable statements.
    stmts: Vec<StmtId>,
    /// Reachable expressions.
    exprs: Vec<ExprId>,
    /// Expressions in the callee position of a call.
    ///
    /// Tracked because a name is allowed to mean a procedure *here* and nowhere
    /// else: `f()` is a direct call, but bare `f` would be a procedure value, and
    /// a procedure value has no representation this wave.
    callees: FxHashSet<ExprId>,
    /// Locals whose declaration is reachable.
    locals: Vec<LocalId>,
}

impl Reach {
    fn of(body: &Body) -> Self {
        let mut out = Self {
            stmts: Vec::new(),
            exprs: Vec::new(),
            callees: FxHashSet::default(),
            locals: Vec::new(),
        };
        let mut stmt_work = vec![body.root];
        let mut expr_work: Vec<ExprId> = Vec::new();
        let mut seen_stmt = FxHashSet::default();
        let mut seen_expr = FxHashSet::default();

        while let Some(id) = stmt_work.pop() {
            if id.index() >= body.stmts.len() || !seen_stmt.insert(id) {
                continue;
            }
            out.stmts.push(id);
            match body.stmt(id) {
                Stmt::Block(ids, _) => stmt_work.extend(ids.iter().copied()),
                Stmt::Local(local, _) => {
                    out.locals.push(*local);
                    if let Some(local_data) = body.locals.get(local.index())
                        && let Some(init) = local_data.init
                    {
                        expr_work.push(init);
                    }
                }
                Stmt::Item(_, _) => {}
                Stmt::Expr(expr, _) => expr_work.push(*expr),
                Stmt::Assign {
                    lhs,
                    op: _,
                    rhs,
                    span: _,
                } => {
                    expr_work.push(*lhs);
                    expr_work.push(*rhs);
                }
                Stmt::If {
                    cond,
                    then,
                    else_,
                    span: _,
                } => {
                    expr_work.push(*cond);
                    stmt_work.push(*then);
                    if let Some(else_) = else_ {
                        stmt_work.push(*else_);
                    }
                }
                Stmt::While {
                    cond,
                    body: inner,
                    label: _,
                    span: _,
                } => {
                    expr_work.push(*cond);
                    stmt_work.push(*inner);
                }
                Stmt::For {
                    iterable,
                    body: inner,
                    ..
                } => {
                    match iterable {
                        jr_hir::ForIterable::Sequence(e) => expr_work.push(*e),
                        jr_hir::ForIterable::Range { start, end } => {
                            expr_work.push(*start);
                            expr_work.push(*end);
                        }
                    }
                    stmt_work.push(*inner);
                }
                Stmt::Defer(inner, _) => stmt_work.push(*inner),
                Stmt::Return(value, _) => {
                    if let Some(value) = value {
                        expr_work.push(*value);
                    }
                }
                Stmt::Break(_, _) | Stmt::Continue(_, _) | Stmt::Error(_) => {}
            }
        }

        while let Some(id) = expr_work.pop() {
            if id.index() >= body.exprs.len() || !seen_expr.insert(id) {
                continue;
            }
            out.exprs.push(id);
            match body.expr(id) {
                Expr::Literal(_, _)
                | Expr::Name { .. }
                | Expr::Uninit(_)
                | Expr::Directive { .. }
                | Expr::Error(_) => {}
                Expr::Binary {
                    op: _,
                    lhs,
                    rhs,
                    span: _,
                } => {
                    expr_work.push(*lhs);
                    expr_work.push(*rhs);
                }
                Expr::Unary {
                    op: _,
                    operand,
                    span: _,
                } => expr_work.push(*operand),
                Expr::Call {
                    callee,
                    args,
                    span: _,
                } => {
                    out.callees.insert(*callee);
                    expr_work.push(*callee);
                    expr_work.extend(args.iter().copied());
                }
                Expr::Field {
                    receiver,
                    name: _,
                    name_span: _,
                    span: _,
                } => {
                    expr_work.push(*receiver);
                }
                Expr::Index { base, index, .. } => {
                    expr_work.push(*base);
                    expr_work.push(*index);
                }
                Expr::Slice { base, .. } => expr_work.push(*base),
                Expr::Cast { operand, .. } | Expr::Autocast { operand, .. } => {
                    expr_work.push(*operand);
                }
                // A bare `.RED` has no sub-expression: sema resolved it to a member of the
                // context's enum, and the value fold turns it into a constant.
                Expr::Member { .. } => {}
                Expr::Deref(inner, _) | Expr::Run(inner, _) => expr_work.push(*inner),
            }
        }

        out
    }
}

// ---------------------------------------------------------------------------
// The poison gate
// ---------------------------------------------------------------------------

/// Why this body cannot be lowered, or `None` if it can.
///
/// ADR-0017 §4. Every reason is a short static string: it is a snapshot key and a
/// debugging aid, not user-facing prose, because this crate raises no diagnostics.
fn scan(
    hir: &FileHir,
    body: &Body,
    body_id: BodyId,
    reach: &Reach,
    resolve: &ResolveMap,
    types: &TypeMap,
    signatures: &FileSignatures,
    consts: &ConstValues,
    imports: &ImportedProcs,
) -> Option<&'static str> {
    let scope = ExprScope::Body(body_id);

    for id in &reach.stmts {
        match body.stmt(*id) {
            Stmt::Error(_) => return Some("the body contains recovered syntax"),
            Stmt::Block(_, _)
            | Stmt::Local(_, _)
            | Stmt::Item(_, _)
            | Stmt::Expr(_, _)
            | Stmt::Assign { .. }
            | Stmt::If { .. }
            | Stmt::While { .. }
            | Stmt::Return(_, _)
            | Stmt::Break(_, _)
            | Stmt::Continue(_, _)
            | Stmt::For { .. }
            | Stmt::Defer(_, _) => {}
        }
    }

    for local in &reach.locals {
        match types.local_type(body_id, *local) {
            None => return Some("a local was never typed"),
            Some(PoolId::ERROR) => return Some("a local has an error type"),
            Some(_) => {}
        }
    }

    for id in &reach.exprs {
        match types.expr_type(scope, *id) {
            None => return Some("an expression was never typed"),
            Some(PoolId::ERROR) => return Some("an expression has an error type"),
            Some(_) => {}
        }
        match body.expr(*id) {
            Expr::Error(_) => return Some("the body contains recovered syntax"),
            // ADR-0018 §3: a `#run` lowers exactly when the const query has
            // already evaluated it. Without a value there is still nothing
            // honest to emit, so the ADR-0017 refusal survives as the fallback.
            Expr::Run(_, _) => {
                if consts.run(scope, *id).is_none() {
                    return Some("#run has no value until jr-vm (ADR-0016 §4)");
                }
            }
            Expr::Directive { .. } => return Some("a directive has no runtime value"),
            Expr::Name {
                name: _,
                span: _,
                res,
            } => {
                let res = resolve.get(scope, *id).unwrap_or(*res);
                // A name whose *type* is `type` denotes a type rather than a value (ADR-0012),
                // so it needs no runtime value and must not be refused for lacking one. This is
                // the receiver of `Colour.RED` — including an **imported** `Colour`, which was
                // refused as "an imported name has no value" and surfaced as an ICE
                // (ADR-0047 §1). The member fold in `expr` replaces the whole field access with
                // a constant, so the receiver is never emitted at all.
                //
                // Asked of the `TypeMap` rather than of the `Res`, because that is the one
                // question with the same answer for a local and an imported declaration.
                let denotes_a_type = types.expr_type(scope, *id) == Some(PoolId::TYPE);
                if !denotes_a_type
                    && let Some(reason) =
                        scan_name(hir, signatures, reach, consts, imports, *id, res)
                {
                    return Some(reason);
                }
            }
            Expr::Literal(_, _)
            | Expr::Binary { .. }
            | Expr::Unary { .. }
            | Expr::Call { .. }
            | Expr::Field { .. }
            | Expr::Index { .. }
            | Expr::Slice { .. }
            | Expr::Cast { .. }
            | Expr::Autocast { .. }
            | Expr::Member { .. }
            | Expr::Deref(_, _)
            | Expr::Uninit(_) => {}
        }
    }

    None
}

/// Whether one name reference is lowerable.
fn scan_name(
    hir: &FileHir,
    signatures: &FileSignatures,
    reach: &Reach,
    consts: &ConstValues,
    imports: &ImportedProcs,
    id: ExprId,
    res: Res,
) -> Option<&'static str> {
    match res {
        Res::Local(_) | Res::Param(_) => None,
        Res::Error => Some("a name failed to resolve"),
        Res::Imported(import, name) => {
            if reach.callees.contains(&id) {
                // ADR-0018 §5 made this representable: `Callee::Direct` carries a
                // `ProcRef`, and `jr-db` resolved the name to one from the other
                // file's signatures. Without that resolution the ADR-0017 refusal
                // still stands.
                if imports.get(import, name).is_some() {
                    None
                } else {
                    Some("a cross-file call needs the callee's signatures")
                }
            } else {
                // An imported *constant*'s value would have to come from the other
                // file's const evaluation, which is the cross-body read ADR-0017 §3
                // keeps out of this query. Nothing in the corpus needs it.
                Some("an imported name has no value until jr-vm")
            }
        }
        Res::Item(item) => {
            let Some(item_data) = hir.items.get(item.index()) else {
                return Some("a name resolved to no item");
            };
            let is_proc = matches!(
                &item_data.kind,
                ItemKind::Const {
                    value: ConstValue::Proc(_)
                }
            );
            if reach.callees.contains(&id) {
                // A direct call to a procedure declared in this file is the one
                // file-level reference that lowers, because `ProcId` names it.
                if is_proc {
                    None
                } else {
                    Some("a call to something that is not a procedure")
                }
            } else if consts.item(item).is_some() {
                // ADR-0018 §3: the const query evaluated it, so there is a value
                // to emit.
                None
            } else if matches!(
                &item_data.kind,
                ItemKind::Const {
                    value: ConstValue::Enum(_) | ConstValue::Struct(_) | ConstValue::Union(_)
                }
            ) {
                // A *type* name used as a receiver — `Colour` in `Colour.GREEN` — has no
                // runtime value and needs none: the member fold in `expr` replaces the whole
                // field access with a constant, so the receiver is never emitted (ADR-0041 §5,
                // ADR-0047 §1). Refusing here would refuse every body that names an enum
                // member, which is exactly what it did until this arm existed.
                None
            } else {
                let _ = signatures;
                Some("a file-level item has no value until jr-vm")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The lowering context
// ---------------------------------------------------------------------------

/// Where a `for` loop's induction variable lives (ADR-0049 §4).
///
/// A sequence loop with no named index needs a counter *distinct from* the element variable:
/// writing the element at the top of the body would otherwise overwrite the counter, which is an
/// infinite loop rather than a wrong answer. A range loop, and a `for x, i:` loop, use a real local
/// — for a range the index genuinely *is* the value.
#[derive(Clone, Copy)]
enum Counter {
    /// A user-written local: the named index, or a range's single variable.
    Local(LocalId),
    /// A stack slot no name reaches, for `for x: buf`.
    Slot(SlotId),
}

/// A `for` loop's bounds, and where its elements come from (ADR-0049 §4).
///
/// `element` is `None` for a range: there is nothing to load, because the index *is* the value.
struct ForBounds {
    /// The first index the loop visits, and the lower bound of a reverse loop.
    start: Operand,
    /// One past the last index, and the length a bounds check compares against.
    end: Operand,
    /// The place elements are read from — an array's storage or a view's `data` word.
    element: Option<Place>,
}

/// One iteration of a loop, so `break` and `continue` know where to go.
struct LoopFrame {
    /// Where `continue` jumps.
    header: BlockId,
    /// Where `break` jumps.
    exit: BlockId,
    /// The label naming this loop, when one was written (ADR-0049 §2).
    ///
    /// Resolved **here** rather than in `ResolveMap`: a label names a loop, not a value, so the
    /// only place its identity exists is this stack. `jump` searches outward from the innermost
    /// frame, which is why an unlabelled `break` still finds `last()`.
    label: Option<Symbol>,
    /// How many `defer` statements were pending when this loop was entered (ADR-0049 §3).
    ///
    /// A `break` out of this loop must run every `defer` registered *inside* it and none from
    /// outside, and this is the mark that says where inside begins.
    defer_depth: usize,
}

struct Lower<'a> {
    hir: &'a FileHir,
    body: &'a Body,
    body_id: BodyId,
    file: FileId,
    resolve: &'a ResolveMap,
    types: &'a TypeMap,
    consts: &'a ConstValues,
    imports: &'a ImportedProcs,
    operators: &'a OperatorCalls,
    interner: &'a Interner,
    pool: &'a mut Pool,
    mir: MirBody,
    ssa: SsaBuilder,
    promotable: Promotable,
    slots: FxHashMap<LocalId, SlotId>,
    params: FxHashMap<ParamId, Operand>,
    /// The spill slot of each aggregate parameter, so a field of one has a place.
    ///
    /// Only parameters whose type is not register-representable appear here; a
    /// scalar parameter stays purely in a register, and asking for its address is
    /// not expressible in Jairs-0 anyway.
    param_slots: FxHashMap<ParamId, SlotId>,
    /// The block being filled, or `None` once control cannot reach further.
    ///
    /// `None` rather than a "terminated" flag on a block, because a statement
    /// after a `return` must be *skipped*, not lowered into a fresh unreachable
    /// block. Lowering it would ask the SSA builder to read variables in a block
    /// with no predecessors, and every such read is recorded as a
    /// possibly-undefined use — turning dead code into spurious findings for the
    /// definite-assignment diagnostic that consumes them.
    current: Option<BlockId>,
    loops: Vec<LoopFrame>,
    /// `defer` statements registered in the scopes currently open, outermost first (ADR-0049 §3).
    ///
    /// A stack rather than a per-block list, because an exit may leave *several* scopes at once —
    /// a `break` out of two blocks runs both sets, innermost first — and a stack with a recorded
    /// depth per scope is what makes "everything registered since here" expressible.
    defers: Vec<StmtId>,
    stray: Vec<MirSpan>,
    /// Why lowering gave up partway through, if it did.
    ///
    /// [`scan`] refuses what it can see *before* lowering starts, but some failures
    /// are only discoverable while building — a memory reference whose place cannot
    /// be formed is the one that exists. Recording it here and failing in
    /// [`Lower::finish`] is what turns such a case into a refusal instead of an
    /// `Rvalue::Undef` that reads as a legitimate uninitialised value.
    ///
    /// This exists because the alternative already shipped a bug: a field of an
    /// aggregate *parameter* had no place, lowering emitted `Undef`, the verifier
    /// had no objection because `Undef` is well-typed, and `modules/Basic`'s `print`
    /// quietly passed a garbage pointer to `write`. ADR-0017 §4's discipline is that
    /// a body that cannot be lowered honestly is refused; this is the channel that
    /// makes it true for failures found mid-build.
    failed: Option<&'static str>,
    ret: PoolId,
}

impl Lower<'_> {
    fn scope(&self) -> ExprScope {
        ExprScope::Body(self.body_id)
    }

    fn span(&self, expr: ExprId) -> MirSpan {
        MirSpan::Expr(self.scope(), expr)
    }

    /// The type sema gave an expression.
    ///
    /// [`scan`] has already refused any body with an untyped or error-typed
    /// reachable expression, so this cannot legitimately fail; it returns
    /// [`PoolId::ERROR`] rather than panicking so that a bug upstream surfaces as a
    /// verifier finding instead of a crash.
    fn ty(&self, expr: ExprId) -> PoolId {
        self.types
            .expr_type(self.scope(), expr)
            .unwrap_or(PoolId::ERROR)
    }

    fn local_ty(&self, local: LocalId) -> PoolId {
        self.types
            .local_type(self.body_id, local)
            .unwrap_or(PoolId::ERROR)
    }

    fn pointee(&self, ty: PoolId) -> Option<PoolId> {
        if ty.index() >= self.pool.len() {
            return None;
        }
        match self.pool.item(ty) {
            Item::PointerType(inner) => Some(*inner),
            Item::VoidType
            | Item::BoolType
            | Item::IntType { .. }
            | Item::FloatType { .. }
            | Item::StringType
            | Item::TypeType
            | Item::ErrorType
            | Item::ForeignLibraryType
            | Item::ArrayType { .. }
            | Item::ViewType { .. }
            | Item::EnumType { .. }
            | Item::StructType { .. }
            | Item::UnionType { .. }
            | Item::ProcType { .. }
            | Item::VoidValue
            | Item::BoolValue(_)
            | Item::IntValue { .. }
            | Item::FloatValue { .. }
            | Item::StrValue(_)
            | Item::TypeValue(_)
            | Item::ProcValue { .. }
            | Item::ForeignLibraryValue(_) => None,
        }
    }

    // -------------------------------------------------------------------
    // Driving
    // -------------------------------------------------------------------

    fn run(&mut self, proc: ProcId, params: &[PoolId], root: StmtId) {
        let entry = self.mir.entry();
        self.current = Some(entry);

        // Parameters are bound to entry-block parameters. They are deliberately
        // *not* routed through the SSA builder: `Res::Param` indexes
        // `Proc::params`, `SsaBuilder` is keyed on `LocalId`, and `jr-hir`'s `Body`
        // does not store parameters at all, so there is no local to key on.
        let mut param_values = Vec::with_capacity(params.len());
        for (index, ty) in params.iter().enumerate() {
            let span = MirSpan::Param(proc, u32::try_from(index).unwrap_or(u32::MAX));
            let value = self.mir.push_block_param(entry, *ty, span);
            param_values.push(value);
            self.params
                .insert(ParamId::from_usize(index), Operand::Value(value));

            // An aggregate parameter needs an address, because the only way to read
            // a field is to project a `Place` and a block parameter is a register.
            // `print :: (s: string) { write(STDOUT, s.data, s.count); }` is the case
            // that matters: without this, `s.data` had no place, and lowering
            // silently produced `Rvalue::Undef` — a `write` from a garbage pointer,
            // with no diagnostic anywhere. So spill it once at entry and project the
            // slot thereafter.
            //
            // This is `escape.rs`'s memory-first default applied to parameters,
            // which it does not classify: `MirBody::params` documents that
            // parameters are *not* locals, so `Promotable` has no entry for one.
            // Two reasons to spill, and the second was a live miscompile before it was added:
            // `p := *b` on a scalar *parameter* takes its address, and a block parameter has
            // none — so `place` answered `None` and the body produced `Rvalue::Undef`.
            // `escape.rs` reports which parameters that applies to, since it already walks for
            // `AddrOf` and a second walk here would be a second opinion.
            if !escape::is_register_representable(self.pool, *ty)
                || self.promotable.param_needs_slot(ParamId::from_usize(index))
            {
                let slot = self.mir.push_slot(*ty, None, span);
                self.param_slots.insert(ParamId::from_usize(index), slot);
                self.emit(Statement::Store {
                    place: Place::slot(slot),
                    value: Operand::Value(value),
                    span,
                });
            }
        }
        self.mir.set_params(param_values);

        // The entry block's only predecessor is the call itself, so it is sealed
        // from the start.
        self.ssa.seal_block(&mut self.mir, entry);

        self.stmt(root);

        if let Some(block) = self.current {
            let term = if self.ret == PoolId::VOID {
                Terminator::Return(None)
            } else {
                // Whether this is *reachable* is the missing-`return` diagnostic,
                // which needs this CFG and which the next wave owns.
                Terminator::Unreachable(Unreachable::FellOffEnd)
            };
            self.mir.set_terminator(block, term);
        }
    }

    fn finish(self) -> Result<MirBody, Poisoned> {
        let Self {
            mut mir,
            ssa,
            stray,
            failed,
            pool,
            ..
        } = self;
        if let Some(reason) = failed {
            return Err(Poisoned::Here(reason));
        }
        mir.set_facts(Facts {
            undefined_reads: ssa.into_undefined_reads(),
            stray_jumps: stray,
        });
        verify::assert_valid(&mir, pool);
        Ok(mir)
    }

    /// Records that lowering cannot continue honestly.
    ///
    /// Keeps the first reason: the later ones are usually consequences of it, and a
    /// refusal reason is a snapshot key that should be stable.
    fn give_up(&mut self, reason: &'static str) {
        if self.failed.is_none() {
            self.failed = Some(reason);
        }
    }

    fn emit(&mut self, stmt: Statement) {
        if let Some(block) = self.current {
            self.mir.stmts_mut(block).push(stmt);
        }
    }

    /// Defines a fresh SSA value from an rvalue.
    fn define(&mut self, ty: PoolId, rvalue: Rvalue, span: MirSpan) -> Operand {
        let value = self.mir.push_value(ty, span);
        self.emit(Statement::Assign {
            dest: value,
            rvalue,
            span,
        });
        Operand::Value(value)
    }

    // -------------------------------------------------------------------
    // Statements
    // -------------------------------------------------------------------

    fn stmt(&mut self, id: StmtId) {
        if self.current.is_none() || id.index() >= self.body.stmts.len() {
            return;
        }
        match self.body.stmt(id).clone() {
            Stmt::Block(ids, _) => {
                // Everything registered *inside* this block runs when it is left, and nothing from
                // outside it (ADR-0049 §3). The depth is the mark that says where inside begins.
                let depth = self.defers.len();
                for inner in ids {
                    self.stmt(inner);
                }
                // Only on the fall-through path: a `break`, `continue` or `return` inside the
                // block already ran them on its way out, and `self.current` is `None` there.
                self.run_defers_from(depth);
                self.defers.truncate(depth);
            }
            Stmt::Local(local, _) => self.local_decl(local),
            // Constructed by nothing today. Matched explicitly so that the day
            // lowering starts producing it, this arm is the thing to change rather
            // than a silent `_`.
            Stmt::Item(_, _) => {}
            Stmt::Expr(expr, _) => self.stmt_expr(expr),
            Stmt::Assign {
                lhs,
                op,
                rhs,
                span: _,
            } => self.assign(lhs, op, rhs),
            Stmt::If {
                cond,
                then,
                else_,
                span: _,
            } => self.if_stmt(cond, then, else_),
            Stmt::While {
                cond,
                body,
                label,
                span: _,
            } => self.while_stmt(cond, body, label),
            Stmt::For {
                value,
                index,
                iterable,
                reverse,
                body,
                label,
                span: _,
            } => self.for_stmt(value, index, &iterable, reverse, body, label),
            // Registered, not lowered: the statements run at every *exit* from the enclosing
            // scope, so `block` emits them before each terminator that leaves (ADR-0049 §3).
            Stmt::Defer(inner, _) => self.defers.push(inner),
            Stmt::Return(value, _) => self.return_stmt(value),
            Stmt::Break(label, _) => self.jump(true, label, id),
            Stmt::Continue(label, _) => self.jump(false, label, id),
            Stmt::Error(_) => {}
        }
    }

    fn local_decl(&mut self, local: LocalId) {
        let Some(data) = self.body.locals.get(local.index()).cloned() else {
            return;
        };
        let ty = self.local_ty(local);
        let span = MirSpan::Local(self.body_id, local);

        // Three cases, which `tests/corpus/valid/005-decl-typed.jr` distinguishes
        // explicitly:
        //
        // - `a: s64 = 7;`   — an initialiser, bound or stored as written.
        // - `b: s64;`       — "default-initialised to the type's zero value", so it
        //                     *is* defined and reading it is not an error.
        // - `c: s64 = ---;` — "explicitly uninitialised … reading it before
        //                     assignment is an error caught in wave W3". No
        //                     definition is emitted, which is what makes `ssa.rs`
        //                     record an undefined read at the first use — the fact
        //                     the definite-assignment diagnostic consumes.
        //
        // Collapsing the middle case onto the last would report a variable the
        // language guarantees is zeroed: a false positive on legal code.
        let init = if data.uninit { None } else { data.init };

        if self.promotable.is_promotable(local) {
            let value = match init {
                Some(init) => Some(self.expr(init)),
                None if data.uninit => None,
                None => self.zero_value(ty).map(Operand::Constant),
            };
            if let Some(value) = value
                && let Some(block) = self.current
            {
                self.ssa.write_variable(block, local, value);
            }
            return;
        }

        let slot = self.slot_for(local, ty, span);
        match init {
            Some(init) => {
                let value = self.expr(init);
                self.emit(Statement::Store {
                    place: Place::slot(slot),
                    value,
                    span,
                });
            }
            // **A default-initialised aggregate must be zeroed here, and this comment used to
            // say it was codegen's job.** It was not: neither back end did it. The VM zeroes a
            // freshly allocated frame, so `p: Point; exit(p.x + p.y);` looked right there;
            // Cranelift's `ExplicitSlot` is uninitialised stack, so the native binary read
            // whatever the last call left — 184, then 200 on a rebuild. The two engines
            // disagreed about a legal program and nothing caught it, because `differential.rs`
            // compares *observable* output and no corpus program observed one (ADR-0039 §4a).
            //
            // `Statement::Zero` carries no size: both back ends know the slot's type and each
            // computes the byte count from the layout it already asks `jr-pool` for, so
            // ADR-0017 §5 stays intact.
            //
            // A *scalar* takes the `Store` path above through `zero_value`; this is only for
            // the aggregates that have no scalar zero — and `---` skips it, since `data.uninit`
            // makes `init` `None` *and* is what `Rvalue::Undef` exists for.
            None if !data.uninit && self.zero_value(ty).is_none() => {
                self.emit(Statement::Zero {
                    place: Place::slot(slot),
                    span,
                });
            }
            None => {
                // A scalar slot: zeroed by an ordinary store of the type's zero constant, or
                // left undefined when `---` asked for that.
                if let Some(zero) = self.zero_value(ty).filter(|_| !data.uninit) {
                    self.emit(Statement::Store {
                        place: Place::slot(slot),
                        value: Operand::Constant(zero),
                        span,
                    });
                }
            }
        }
    }

    /// The zero value of a type, for a default-initialised local.
    ///
    /// `None` for a type whose zero this crate cannot name: a pointer's zero is
    /// null and the pool interns no null, and an aggregate's zero needs a layout
    /// (ADR-0017 §5). A *promotable* local is always an integer, a `bool` or a
    /// pointer, so only the pointer case is reachable, and it degrades to being
    /// treated as uninitialised rather than to a wrong value.
    fn zero_value(&mut self, ty: PoolId) -> Option<PoolId> {
        if ty.index() >= self.pool.len() {
            return None;
        }
        match self.pool.item(ty) {
            Item::IntType { .. } => Some(self.pool.int_value(ty, 0)),
            // `+0.0`, whose bits are all zero — the same value clearing a slot would give,
            // so a promoted `float64` local and a spilled one start out equal.
            Item::FloatType { .. } => Some(self.pool.float_value(ty, 0)),
            // An enum's zero is the *integer* 0 at the enum's own type, which may name no
            // member at all — `enum { A :: 5; }` has no zero member. That is deliberate and
            // matches C: a default-initialised enum holds 0 whether or not it is named, and
            // inventing a "first member" default would differ from the backing type's zero.
            Item::EnumType { .. } => Some(self.pool.int_value(ty, 0)),
            Item::BoolType => Some(PoolId::FALSE),
            Item::VoidType
            | Item::StringType
            | Item::TypeType
            | Item::ErrorType
            | Item::ForeignLibraryType
            | Item::PointerType(_)
            // An array has no *scalar* zero: it is zeroed by clearing its slot, not by
            // assigning a constant, which is what `Statement::Zero` is for. A view is two
            // words, so the same applies — and a zeroed view is `{null, 0}`, which indexes
            // nothing because every index fails the bounds check against a count of 0.
            | Item::ArrayType { .. }
            | Item::ViewType { .. }
            | Item::StructType { .. }
            | Item::UnionType { .. }
            | Item::ProcType { .. }
            | Item::VoidValue
            | Item::BoolValue(_)
            | Item::IntValue { .. }
            | Item::FloatValue { .. }
            | Item::StrValue(_)
            | Item::TypeValue(_)
            | Item::ProcValue { .. }
            | Item::ForeignLibraryValue(_) => None,
        }
    }

    fn slot_for(&mut self, local: LocalId, ty: PoolId, span: MirSpan) -> SlotId {
        if let Some(slot) = self.slots.get(&local) {
            return *slot;
        }
        let slot = self.mir.push_slot(ty, Some(local), span);
        self.slots.insert(local, slot);
        slot
    }

    fn stmt_expr(&mut self, expr: ExprId) {
        // A call in statement position is the one expression whose value is
        // genuinely discarded; `Statement::Discard` exists so that a dump shows
        // that rather than an unused definition.
        if expr.index() < self.body.exprs.len()
            && let Expr::Call {
                callee,
                args,
                span: _,
            } = self.body.expr(expr).clone()
            && let Some(rvalue) = self.call_rvalue(callee, &args)
        {
            let span = self.span(expr);
            self.emit(Statement::Discard { rvalue, span });
            return;
        }
        let _ = self.expr(expr);
    }

    fn assign(&mut self, lhs: ExprId, op: AssignOp, rhs: ExprId) {
        // A promotable local is an SSA variable, not a place, so `i = i + 1` is a
        // write to the builder rather than a store to memory.
        if let Some(local) = self.promotable_local(lhs) {
            let ty = self.local_ty(local);
            let span = self.span(lhs);
            let value = match bin_op_of_assign(op) {
                None => self.expr(rhs),
                Some(bin) => {
                    let Some(block) = self.current else { return };
                    let old = self
                        .ssa
                        .read_variable(&mut self.mir, block, local, ty, span);
                    let new = self.expr(rhs);
                    self.define(
                        ty,
                        Rvalue::Binary {
                            op: bin,
                            lhs: old,
                            rhs: new,
                        },
                        span,
                    )
                }
            };
            if let Some(block) = self.current {
                self.ssa.write_variable(block, local, value);
            }
            return;
        }

        let Some((place, ty)) = self.place(lhs) else {
            // `scan` refuses every body whose left-hand side is not a place, so
            // reaching here means an upstream invariant broke; a trap is louder
            // than silently dropping the assignment.
            if let Some(block) = self.current {
                self.mir
                    .set_terminator(block, Terminator::Unreachable(Unreachable::Trap));
                self.current = None;
            }
            return;
        };
        let span = self.span(lhs);
        let value = match bin_op_of_assign(op) {
            None => self.expr(rhs),
            Some(bin) => {
                let old = self.define(ty, Rvalue::Load(place.clone()), span);
                let new = self.expr(rhs);
                self.define(
                    ty,
                    Rvalue::Binary {
                        op: bin,
                        lhs: old,
                        rhs: new,
                    },
                    span,
                )
            }
        };
        self.emit(Statement::Store { place, value, span });
    }

    fn return_stmt(&mut self, value: Option<ExprId>) {
        let operand = value.map(|expr| self.expr(expr));
        let Some(block) = self.current else { return };
        // A `void` procedure must return nothing and a valued one must return
        // something; `verify` checks both, so honour the signature rather than the
        // syntax if error recovery produced a mismatch.
        let term = if self.ret == PoolId::VOID {
            Terminator::Return(None)
        } else {
            match operand {
                Some(operand) => Terminator::Return(Some(operand)),
                None => Terminator::Unreachable(Unreachable::FellOffEnd),
            }
        };
        self.mir.set_terminator(block, term);
        self.current = None;
    }

    /// `break` (`is_break`) or `continue`.
    /// `break` (`is_break`) or `continue`, optionally naming a label (ADR-0049 §2).
    ///
    /// The label is resolved against the loop stack — the only place a loop's identity exists —
    /// by searching **outward from the innermost**, so an unlabelled jump still finds the nearest
    /// loop and `break outer` skips past any number of inner ones.
    fn jump(&mut self, is_break: bool, label: Option<Symbol>, at: StmtId) {
        if self.current.is_none() {
            return;
        }
        let found = match label {
            None => self.loops.last().map(|f| (f.exit, f.header, f.defer_depth)),
            Some(name) => self
                .loops
                .iter()
                .rev()
                .find(|f| f.label == Some(name))
                .map(|f| (f.exit, f.header, f.defer_depth)),
        };
        let Some((exit, header, depth)) = found else {
            // Two mistakes with one shape: a jump outside any loop, and one naming a label that
            // does not exist. Both are recorded on the same channel E0229 already reads, and the
            // *message* distinguishes them (ADR-0049 §2).
            self.stray.push(MirSpan::Stmt(self.body_id, at));
            if let Some(block) = self.current {
                self.mir
                    .set_terminator(block, Terminator::Unreachable(Unreachable::StrayJump));
            }
            self.current = None;
            return;
        };

        // **Every `defer` registered inside the loop being left runs first** (ADR-0049 §3). This
        // is emitted *before* the terminator because a terminator is set once and carries no
        // statement list — so a deferred statement appears in the MIR once per exit path, which is
        // duplication of statements and not of evaluation.
        //
        // A `continue` leaves the *iteration* rather than the loop, so it runs the same set: a
        // `defer` in a loop body runs per iteration, which is §3's decision.
        self.run_defers_from(depth);

        let Some(block) = self.current else { return };
        let target = if is_break { exit } else { header };
        self.mir
            .set_terminator(block, Terminator::Goto(Target::new(target)));
        self.current = None;
    }

    /// Lowers every `defer` registered at or after `depth`, innermost first (ADR-0049 §3).
    ///
    /// Reverse order within a scope, so `defer a(); defer b();` runs `b` then `a` — anything else
    /// makes paired acquisition and release inexpressible.
    ///
    /// The stack is **not** truncated here: the same defers may need to run again on a *different*
    /// exit path from the same scope, and the owner of the scope is what pops them.
    fn run_defers_from(&mut self, depth: usize) {
        if self.current.is_none() || self.defers.len() <= depth {
            return;
        }
        let pending: Vec<StmtId> = self.defers[depth..].iter().rev().copied().collect();
        for stmt in pending {
            // A `defer` whose own body contains a `defer` would otherwise register into the list
            // being walked. Guarded by taking `pending` first, so this loop reads a snapshot.
            self.stmt(stmt);
            if self.current.is_none() {
                // The deferred statement itself left the scope — a `return` inside a `defer`.
                // Nothing after it can run, and pretending otherwise would emit unreachable code.
                return;
            }
        }
    }

    // -------------------------------------------------------------------
    // Control flow
    // -------------------------------------------------------------------

    fn if_stmt(&mut self, cond: ExprId, then: StmtId, else_: Option<StmtId>) {
        let cond_operand = self.expr(cond);
        let Some(head) = self.current else { return };

        let then_bb = self.mir.push_block();
        // An `else` block is created even when the source has none, so that the
        // branch's two edges each land on a single-predecessor block. Targeting the
        // join directly would create a critical edge, which `verify` rejects.
        let else_bb = self.mir.push_block();
        let join = self.mir.push_block();

        self.mir.set_terminator(
            head,
            Terminator::Branch {
                cond: cond_operand,
                then_: Target::new(then_bb),
                else_: Target::new(else_bb),
            },
        );
        self.ssa.seal_block(&mut self.mir, then_bb);
        self.ssa.seal_block(&mut self.mir, else_bb);

        self.current = Some(then_bb);
        self.stmt(then);
        let then_fell_through = self.goto(join);

        self.current = Some(else_bb);
        if let Some(else_) = else_ {
            self.stmt(else_);
        }
        let else_fell_through = self.goto(join);

        self.ssa.seal_block(&mut self.mir, join);
        self.current = (then_fell_through || else_fell_through).then_some(join);
    }

    fn while_stmt(&mut self, cond: ExprId, body: StmtId, label: Option<Symbol>) {
        let Some(pre) = self.current else { return };
        let header = self.mir.push_block();
        let body_bb = self.mir.push_block();
        // The header branches two ways, and `break` gives the exit extra
        // predecessors, so the header's false edge goes through its own block to
        // keep every edge non-critical.
        let pre_exit = self.mir.push_block();
        let exit = self.mir.push_block();

        self.mir
            .set_terminator(pre, Terminator::Goto(Target::new(header)));

        // The header is *filled* now but cannot be *sealed* until the back edge
        // exists — the distinction `ssa.rs` keeps two bits for.
        self.current = Some(header);
        let cond_operand = self.expr(cond);
        let cond_block = self.current.unwrap_or(header);
        self.mir.set_terminator(
            cond_block,
            Terminator::Branch {
                cond: cond_operand,
                then_: Target::new(body_bb),
                else_: Target::new(pre_exit),
            },
        );
        self.mir
            .set_terminator(pre_exit, Terminator::Goto(Target::new(exit)));
        self.ssa.seal_block(&mut self.mir, body_bb);
        self.ssa.seal_block(&mut self.mir, pre_exit);

        self.loops.push(LoopFrame {
            header,
            exit,
            label,
            // The depth *before* the body runs, so a `break` runs the body's defers and none from
            // the enclosing scope (ADR-0049 §3).
            defer_depth: self.defers.len(),
        });
        self.current = Some(body_bb);
        self.stmt(body);
        self.goto(header);
        self.loops.pop();

        // Every edge into the header now exists: the pre-header, the back edge, and
        // any `continue`. Only now can it be sealed.
        self.ssa.seal_block(&mut self.mir, header);
        self.ssa.seal_block(&mut self.mir, exit);
        self.current = Some(exit);
    }

    /// A `for` loop's bounds and, for a sequence, where its elements live.
    ///
    /// Computed once before the loop, which is what makes `for i: 0..f()` call `f` exactly once.
    fn for_bounds(&mut self, iterable: &jr_hir::ForIterable, span: MirSpan) -> Option<ForBounds> {
        match iterable {
            jr_hir::ForIterable::Range { start, end } => {
                let start = self.expr(*start);
                let end = self.expr(*end);
                Some(ForBounds {
                    start,
                    end,
                    element: None,
                })
            }
            jr_hir::ForIterable::Sequence(expr) => {
                let seq_ty = self.ty(*expr);
                // Auto-deref, matching `index_place`: `p: *[4]u8` iterates through the pointer.
                let (mut place, mut ty) = if self.pointee(seq_ty).is_some() {
                    let operand = self.expr(*expr);
                    let pointee = self.pointee(seq_ty)?;
                    (Place::deref(operand), pointee)
                } else {
                    self.place(*expr)?
                };
                while let Some(pointee) = self.pointee(ty) {
                    place = place.project(Projection::Deref);
                    ty = pointee;
                }

                let zero = Operand::Constant(self.pool.int_value(PoolId::S64, 0));
                // The length: an array's is a constant from its type, a view's is a *load* of its
                // `.count` — the same two shapes `index_place` distinguishes, so a `for` over
                // either needs nothing new (ADR-0039 §1, ADR-0044 §4).
                if let Some(len) = self.array_len(ty) {
                    let end = Operand::Constant(self.pool.int_value(PoolId::S64, len));
                    Some(ForBounds {
                        start: zero,
                        end,
                        element: Some(place),
                    })
                } else {
                    self.view_elem(ty)?;
                    let count = self.define(
                        PoolId::S64,
                        Rvalue::Load(place.clone().project(Projection::ViewCount)),
                        span,
                    );
                    Some(ForBounds {
                        start: zero,
                        end: count,
                        element: Some(place.project(Projection::ViewData)),
                    })
                }
            }
        }
    }

    /// Allocates a counter slot no user name reaches.
    fn synthetic_counter(&mut self, span: MirSpan) -> Counter {
        Counter::Slot(self.mir.push_slot(PoolId::S64, None, span))
    }

    /// Writes the induction variable, wherever it lives.
    fn write_counter(&mut self, counter: Counter, value: Operand, span: MirSpan) {
        match counter {
            Counter::Local(local) => self.write_local(local, value, span),
            Counter::Slot(slot) => self.emit(Statement::Store {
                place: Place::slot(slot),
                value,
                span,
            }),
        }
    }

    /// Reads the induction variable.
    fn read_counter(&mut self, counter: Counter, span: MirSpan) -> Operand {
        match counter {
            Counter::Local(local) => {
                let Some(block) = self.current else {
                    return Operand::Constant(self.pool.int_value(PoolId::S64, 0));
                };
                let ty = self.local_ty(local);
                if self.promotable.is_promotable(local) {
                    return self
                        .ssa
                        .read_variable(&mut self.mir, block, local, ty, span);
                }
                let slot = self.slot_for(local, ty, span);
                self.define(ty, Rvalue::Load(Place::slot(slot)), span)
            }
            Counter::Slot(slot) => self.define(PoolId::S64, Rvalue::Load(Place::slot(slot)), span),
        }
    }

    /// Writes a local, taking the promoted or the spilled path.
    ///
    /// A `for`'s variables are ordinary locals, so both paths are reachable: the element variable
    /// is spilled when its address is taken, and the induction variable is promoted in every loop
    /// that does not.
    fn write_local(&mut self, local: LocalId, value: Operand, span: MirSpan) {
        if self.promotable.is_promotable(local) {
            if let Some(block) = self.current {
                self.ssa.write_variable(block, local, value);
            }
            return;
        }
        let ty = self.local_ty(local);
        let slot = self.slot_for(local, ty, span);
        self.emit(Statement::Store {
            place: Place::slot(slot),
            value,
            span,
        });
    }

    /// Lowers `for x: iterable { … }` (ADR-0049 §1, §4).
    ///
    /// The `while` shape with an induction variable: a header that compares, a body that reads the
    /// element and bumps the index, and the same non-critical-edge discipline (ADR-0017 §1) —
    /// including the pre-exit block, because `break` gives the exit extra predecessors.
    ///
    /// Reverse (`for < x: buf`) counts **down from `len - 1`**, so it visits the same elements in
    /// the opposite order and an empty sequence still runs zero times. Expressed by choosing the
    /// initial value and the step rather than by a second loop shape.
    fn for_stmt(
        &mut self,
        value: LocalId,
        index: Option<LocalId>,
        iterable: &jr_hir::ForIterable,
        reverse: bool,
        body: StmtId,
        label: Option<Symbol>,
    ) {
        let span = MirSpan::Local(self.body_id, value);
        // The bounds and the element source, computed **before** the loop: a range's ends and a
        // sequence's length are evaluated once, which is what makes `for i: 0..f()` call `f` once.
        let Some(bounds) = self.for_bounds(iterable, span) else {
            self.give_up("a `for` over something with no length");
            return;
        };

        let Some(pre) = self.current else { return };
        let header = self.mir.push_block();
        let body_bb = self.mir.push_block();
        // **The step gets its own block, and `continue` targets it rather than the header.** The
        // first draft emitted the step at the end of the body and claimed a `continue` would run
        // it "because `continue` targets `header`" — which is exactly backwards: jumping to the
        // header *bypasses* code at the end of the body, so `continue` never advanced the counter
        // and the loop hung. Caught by running it.
        let step_bb = self.mir.push_block();
        let pre_exit = self.mir.push_block();
        let exit = self.mir.push_block();

        // The induction variable, and it must **not** be the element variable: writing the element
        // at the top of the body would overwrite the counter, which produced an infinite loop when
        // this was `index.unwrap_or(value)`. For a *range* they are genuinely the same variable —
        // the index is the value — and for a sequence the counter needs its own storage.
        //
        // `for x, i: buf` reuses `i`, because the user asked for the index by name and it is the
        // counter. `for x: buf` allocates a slot no name reaches, which is the unspellable-name
        // trick ADR-0048 used for `operator+`.
        // **The counter always lives in a slot**, even when a user named it. A slot is memory, so
        // reading it is a `Load` that creates no phi — and the step block reads it *after* the body
        // has been lowered but *before* the header is sealed, which with SSA reads produced an
        // incomplete phi in the header that could not see a definition from outside the loop. The
        // symptom was a definite-assignment false positive on a variable the loop never touched.
        //
        // A named index is *also* written to its own local at the top of the body, so the user sees
        // an ordinary variable and the mid-end can still promote it.
        let counter = match (index, bounds.element.is_some()) {
            // A sequence with no named index: the counter is a fresh slot, distinct from `value`.
            (None, true) => self.synthetic_counter(span),
            // A named index, or a range where the index *is* the value.
            (Some(i), _) => Counter::Local(i),
            (None, false) => Counter::Local(value),
        };
        let start = if reverse {
            // `len - 1`, wrapping is impossible because the loop does not run when `len` is 0 —
            // the header's `>= 0` test fails immediately.
            let one = Operand::Constant(self.pool.int_value(PoolId::S64, 1));
            self.define(
                PoolId::S64,
                Rvalue::Binary {
                    op: BinOp::Sub,
                    lhs: bounds.end,
                    rhs: one,
                },
                span,
            )
        } else {
            bounds.start
        };
        self.write_counter(counter, start, span);

        self.mir
            .set_terminator(pre, Terminator::Goto(Target::new(header)));

        // Filled now, sealed after the back edge exists — the distinction `ssa.rs` keeps two bits
        // for, and the reason this cannot simply be a `while` over a rewritten condition.
        self.current = Some(header);
        let current = self.read_counter(counter, span);
        let cond = if reverse {
            // Counting down: continue while the index is at or above the range's start.
            self.define(
                PoolId::BOOL,
                Rvalue::Binary {
                    op: BinOp::Ge,
                    lhs: current,
                    rhs: bounds.start,
                },
                span,
            )
        } else {
            self.define(
                PoolId::BOOL,
                Rvalue::Binary {
                    op: BinOp::Lt,
                    lhs: current,
                    rhs: bounds.end,
                },
                span,
            )
        };
        let cond_block = self.current.unwrap_or(header);
        self.mir.set_terminator(
            cond_block,
            Terminator::Branch {
                cond,
                then_: Target::new(body_bb),
                else_: Target::new(pre_exit),
            },
        );
        self.mir
            .set_terminator(pre_exit, Terminator::Goto(Target::new(exit)));
        // `pre_exit` has exactly one predecessor, so it can be sealed now.
        //
        // `body_bb` **cannot**, and that is this wave's subtlest bug. Its only predecessor is the
        // header, so sealing looks safe — but sealing it resolves any *incomplete phi* created
        // while lowering the body, and a nested loop's `break outer` adds an edge to **this**
        // loop's exit after that resolution has happened. The read then resolved against a
        // predecessor set that was still growing, and reported a definite-assignment false
        // positive on a variable assigned right there. Sealed after the body instead.
        self.ssa.seal_block(&mut self.mir, pre_exit);
        // **The step's back edge is set now, before the body is lowered.** `MirBody::predecessors`
        // is a `OnceLock` computed from terminators, so a block sealed while an edge into it does not
        // yet exist reads a stale set — and the header's back edge comes from the step. `while_stmt`
        // never hit this because its back edge is the body's own terminator, set by `goto` before
        // anything reads through the header.
        //
        // Statements are appended to `step_bb` later; only the *terminator* has to be early.
        self.mir
            .set_terminator(step_bb, Terminator::Goto(Target::new(header)));
        // `body_bb`'s only predecessor is the header, so it is sealed before the body runs. That is
        // what `while_stmt` does, and doing it *after* the body — an attempted fix for a nested
        // `break` — resolved the body's incomplete phis too late and produced the very
        // false positive it was meant to remove.
        self.ssa.seal_block(&mut self.mir, body_bb);

        let body_defer_depth = self.defers.len();
        self.loops.push(LoopFrame {
            // `continue` goes to the *step*, which then falls through to the header. That is what
            // makes `continue` advance the counter.
            header: step_bb,
            exit,
            label,
            defer_depth: self.defers.len(),
        });
        self.current = Some(body_bb);

        // The element, read at the top of the body. **A copy**, so `x = 0` inside the loop
        // modifies the local rather than the sequence (ADR-0049 §4) — which follows from `x` being
        // a local rather than needing a rule.
        if let Some(place) = bounds.element.clone() {
            let idx = self.read_counter(counter, span);
            // The same `BoundsCheck` an ordinary index emits. A `for` provably stays in range and
            // const-prop may delete it, which is ADR-0003's point: a pass *proves* it redundant
            // rather than lowering skipping it.
            self.emit(Statement::BoundsCheck {
                index: idx,
                len: bounds.end,
                span,
            });
            let elem_place = place.project(Projection::Index(idx));
            let elem_ty = self.local_ty(value);
            let loaded = self.define(elem_ty, Rvalue::Load(elem_place), span);
            self.write_local(value, loaded, span);
        }

        self.stmt(body);

        // The body falls through to the step; a `continue` jumps straight to it.
        self.goto(step_bb);
        self.loops.pop();
        // **The loop body's defers are popped here**, and forgetting it made a later loop's
        // `defer` run the *earlier* loop's statements too — which read a variable declared between
        // them and produced a definite-assignment false positive rather than a wrong answer.
        self.defers.truncate(body_defer_depth);

        // The step, in its own block so that **both** paths run it.
        //
        // Sealed after the body, because a `continue` inside the body is one of its predecessors.
        self.current = Some(step_bb);
        let idx = self.read_counter(counter, span);
        let one = Operand::Constant(self.pool.int_value(PoolId::S64, 1));
        let next = self.define(
            PoolId::S64,
            Rvalue::Binary {
                op: if reverse { BinOp::Sub } else { BinOp::Add },
                lhs: idx,
                rhs: one,
            },
            span,
        );
        self.write_counter(counter, next, span);
        // The terminator was set before the body; `current` is cleared by hand because `goto` would
        // overwrite it.
        self.current = None;

        // **Order matters, and getting it wrong was a definite-assignment false positive.** The
        // header must be sealed *before* the step, because the step's own reads resolve through the
        // header's phis — and the header's predecessor set is complete once the step block exists
        // as a block, not once it is sealed. `while_stmt` has no step block, which is why it never
        // hit this.
        // Every edge into the step exists now: the body's fall-through and any `continue`. The
        // header follows, because its back edge comes from the step.
        self.ssa.seal_block(&mut self.mir, step_bb);
        self.ssa.seal_block(&mut self.mir, header);
        self.ssa.seal_block(&mut self.mir, exit);
        self.current = Some(exit);
    }

    /// Terminates the current block with a jump to `target`.
    ///
    /// Returns whether there was a block to terminate — that is, whether control
    /// actually falls through to `target`.
    fn goto(&mut self, target: BlockId) -> bool {
        match self.current.take() {
            Some(block) => {
                self.mir
                    .set_terminator(block, Terminator::Goto(Target::new(target)));
                true
            }
            None => false,
        }
    }

    // -------------------------------------------------------------------
    // Expressions
    // -------------------------------------------------------------------

    fn expr(&mut self, id: ExprId) -> Operand {
        if id.index() >= self.body.exprs.len() {
            return Operand::Constant(PoolId::VOID_VALUE);
        }
        let ty = self.ty(id);
        let span = self.span(id);
        match self.body.expr(id).clone() {
            Expr::Literal(literal, _) => Operand::Constant(self.constant(&literal, ty)),
            Expr::Name {
                name: _,
                span: _,
                res,
            } => self.name(id, res, ty, span),
            Expr::Binary {
                op,
                lhs,
                rhs,
                span: _,
            } => {
                // **An overload lowers to an ordinary direct call** (ADR-0048 §5): no new MIR
                // node, no new callee kind, no change to either back end — which is the evidence
                // the design fits the existing shape rather than a convenience. It is also what
                // makes an overload inlinable by ADR-0021's inliner with no special case.
                //
                // The target is read from `jr-sema`'s map rather than re-resolved: this crate
                // reads types instead of computing them, and resolution is the same kind of rule.
                match self.operators.get(self.scope(), id) {
                    Some(target) => {
                        let args = vec![self.expr(lhs), self.expr(rhs)];
                        self.define(
                            ty,
                            Rvalue::Call {
                                callee: Callee::Direct(target),
                                args,
                            },
                            span,
                        )
                    }
                    None => self.binary(op, lhs, rhs, ty, span),
                }
            }
            Expr::Unary {
                op,
                operand,
                span: _,
            } => self.unary(op, operand, ty, span),
            Expr::Call {
                callee,
                args,
                span: _,
            } => match self.call_rvalue(callee, &args) {
                Some(rvalue) => self.define(ty, rvalue, span),
                None => {
                    self.give_up("a call has no resolvable callee");
                    self.define(ty, Rvalue::Undef, span)
                }
            },
            Expr::Cast {
                ty: _,
                operand,
                span: _,
            } => self.cast(operand, ty, span),
            // `xx` lowers through the **same** path `cast` does, which is the payoff for
            // ADR-0037 §2 having put the conversion in `Rvalue::Convert` with an explicit
            // source kind: by the time MIR runs, the target is simply this expression's type
            // (ADR-0046 §2).
            Expr::Autocast { operand, span: _ } => self.cast(operand, ty, span),
            // A bare `.RED` is a constant, exactly as `Colour.RED` is (ADR-0041 §5), interned
            // at the enum sema chose from the context — so the two spellings produce the
            // identical operand.
            Expr::Member { name, .. } => match self.enum_member_value(ty, name) {
                Some(value) => Operand::Constant(self.pool.int_value(ty, value as u64)),
                None => {
                    self.give_up("a bare enum member sema did not resolve");
                    self.define(ty, Rvalue::Undef, span)
                }
            },
            // A *view's* `.count` is a load, not a constant — the one place the two indexable
            // types differ in more than their length's provenance (ADR-0044 §4). Before the
            // array arm because `is_array_count` looks through pointers and a `*[]T` must
            // reach this one.
            Expr::Field { receiver, name, .. } if self.is_view_count(receiver, name) => {
                match self.place(receiver) {
                    Some((place, _)) => {
                        self.define(ty, Rvalue::Load(place.project(Projection::ViewCount)), span)
                    }
                    None => {
                        self.give_up("a view's `.count` with no place");
                        self.define(ty, Rvalue::Undef, span)
                    }
                }
            }
            // `array.count` is a *constant* from the type, not a load: nothing is stored
            // anywhere to read it from (ADR-0039 §5). Handled before the place attempt below,
            // because `field_place` correctly answers `None` for it — and that `None` reaches
            // `give_up`, so without this arm every use of `.count` refused the whole body.
            Expr::Field { receiver, name, .. } if self.is_array_count(receiver, name) => {
                let len = self
                    .array_len_through_pointers(self.ty(receiver))
                    .unwrap_or(0);
                Operand::Constant(self.pool.int_value(PoolId::S64, len))
            }
            // The member comes from the expression's **own type**, which sema has already
            // resolved to the enum — so this works for an *imported* enum too (ADR-0047 §1).
            // Same shape as `.count` above and for the same reason.
            Expr::Field { name, .. } if self.enum_member_value(ty, name).is_some() => {
                let value = self.enum_member_value(ty, name).expect("just checked");
                // Interned at the *enum's* type, not at `s64`: the constant's type is what
                // makes `Colour.RED` a `Colour` rather than a number that compares equal to one.
                Operand::Constant(self.pool.int_value(ty, value as u64))
            }
            // `buf[]` constructs a two-word value, so it is neither a place nor a single
            // rvalue: it needs a slot to assemble the pair in (ADR-0044 §1).
            Expr::Slice { base, span: _ } => match self.slice_value(base, ty, span) {
                Some(operand) => operand,
                None => {
                    self.give_up("a slice of something with no place");
                    self.define(ty, Rvalue::Undef, span)
                }
            },
            Expr::Field { .. } | Expr::Index { .. } | Expr::Deref(_, _) => match self.place(id) {
                Some((place, _)) => self.define(ty, Rvalue::Load(place), span),
                None => {
                    // Never `Undef`: that would read as a legitimate uninitialised
                    // value and pass the verifier, which is how a field of an
                    // aggregate parameter once became a garbage pointer handed to
                    // `write`. Refuse instead.
                    self.give_up("a memory reference has no place");
                    self.define(ty, Rvalue::Undef, span)
                }
            },
            // `---` names no value at all. The `Undef` definition exists so the IR
            // stays well-formed; reading it is what the definite-assignment
            // diagnostic reports.
            Expr::Uninit(_) => self.define(ty, Rvalue::Undef, span),
            // A `#run` the const query evaluated is indistinguishable from a
            // literal, which is exactly what ADR-0016 §4 promised folding would
            // buy. Without a value `scan` already refused the body.
            Expr::Run(_, _) => match self.consts.run(self.scope(), id) {
                Some(value) => Operand::Constant(value),
                None => self.define(ty, Rvalue::Undef, span),
            },
            // Both are refused by `scan` before lowering starts.
            Expr::Directive { .. } | Expr::Error(_) => self.define(ty, Rvalue::Undef, span),
        }
    }

    fn constant(&mut self, literal: &Literal, ty: PoolId) -> PoolId {
        match literal {
            // `value` is a magnitude: `-1` is `Neg` applied to `1`, so no sign is
            // reconstructed here.
            // Wrapped into the destination's kind, because `int_value` takes raw bits and a
            // negative literal's bits are its two's-complement encoding. The same
            // `IntKind::wrap` the interpreter and `constprop` use, so a constant folded here
            // and one computed at run time cannot differ (ADR-0038 §2).
            //
            // A literal whose type is not an integer — which sema has already rejected —
            // falls back to `s64`'s wrapping rather than panicking: this is the poison path,
            // and a body containing it is refused before it can run.
            Literal::Int {
                value,
                radix: _,
                overflowed: _,
            } => {
                let kind = jr_pool::IntKind::of(self.pool, ty).unwrap_or(jr_pool::IntKind::S64);
                let bits = kind.wrap(*value);
                self.pool.int_value(ty, bits)
            }
            // Narrowed to the destination's width at interning time, which is where ADR-0040 §5
            // says a `float32` context rounds — IEEE-754 saturates rather than failing, so
            // unlike an integer literal there is nothing to reject.
            Literal::Float { bits, malformed: _ } => {
                let kind = jr_pool::FloatKind::of(self.pool, ty).unwrap_or(jr_pool::FloatKind::F64);
                self.pool
                    .float_value(ty, kind.encode(f64::from_bits(*bits)))
            }
            Literal::Bool(value) => self.pool.bool_value(*value),
            Literal::Str(text) => self.pool.str_value(text),
        }
    }

    fn name(&mut self, id: ExprId, res: Res, ty: PoolId, span: MirSpan) -> Operand {
        let res = self.resolve.get(self.scope(), id).unwrap_or(res);
        match res {
            Res::Local(local) => {
                if self.promotable.is_promotable(local) {
                    let Some(block) = self.current else {
                        return Operand::Constant(PoolId::VOID_VALUE);
                    };
                    let local_ty = self.local_ty(local);
                    self.ssa
                        .read_variable(&mut self.mir, block, local, local_ty, span)
                } else {
                    let local_ty = self.local_ty(local);
                    let slot = self.slot_for(local, local_ty, span);
                    self.define(local_ty, Rvalue::Load(Place::slot(slot)), span)
                }
            }
            Res::Param(param) => self
                .params
                .get(&param)
                .copied()
                .unwrap_or(Operand::Constant(PoolId::VOID_VALUE)),
            // A file-level constant the const query evaluated is a constant
            // operand (ADR-0018 §3). Everything else here `scan` already refused.
            Res::Item(item) => match self.consts.item(item) {
                Some(value) => Operand::Constant(value),
                None => self.define(ty, Rvalue::Undef, span),
            },
            Res::Imported(_, _) | Res::Error => self.define(ty, Rvalue::Undef, span),
        }
    }

    fn binary(
        &mut self,
        op: jr_hir::BinOp,
        lhs: ExprId,
        rhs: ExprId,
        ty: PoolId,
        span: MirSpan,
    ) -> Operand {
        // `&&` and `||` are control flow, not operators: MIR's `BinOp` has no
        // variant for them, which is what makes forgetting to short-circuit a
        // compile error rather than a silent semantic change.
        match op {
            jr_hir::BinOp::And => return self.short_circuit(lhs, rhs, false, span),
            jr_hir::BinOp::Or => return self.short_circuit(lhs, rhs, true, span),
            jr_hir::BinOp::Add
            | jr_hir::BinOp::Sub
            | jr_hir::BinOp::Mul
            | jr_hir::BinOp::Div
            | jr_hir::BinOp::Rem
            | jr_hir::BinOp::WrapAdd
            | jr_hir::BinOp::WrapSub
            | jr_hir::BinOp::WrapMul
            | jr_hir::BinOp::Eq
            | jr_hir::BinOp::Ne
            | jr_hir::BinOp::Lt
            | jr_hir::BinOp::Le
            | jr_hir::BinOp::Gt
            | jr_hir::BinOp::Ge
            | jr_hir::BinOp::BitAnd
            | jr_hir::BinOp::BitOr
            | jr_hir::BinOp::BitXor
            | jr_hir::BinOp::Shl
            | jr_hir::BinOp::Shr => {}
        }
        let lhs_operand = self.expr(lhs);
        let rhs_operand = self.expr(rhs);
        let Some(op) = mir_bin_op(op) else {
            return self.define(ty, Rvalue::Undef, span);
        };
        self.define(
            ty,
            Rvalue::Binary {
                op,
                lhs: lhs_operand,
                rhs: rhs_operand,
            },
            span,
        )
    }

    /// Lowers `&&` (`short_on = false`) or `||` (`short_on = true`).
    ///
    /// `short_on` is the value of the left operand that makes the right operand
    /// unnecessary, and is also the result in that case.
    fn short_circuit(
        &mut self,
        lhs: ExprId,
        rhs: ExprId,
        short_on: bool,
        span: MirSpan,
    ) -> Operand {
        let lhs_operand = self.expr(lhs);
        let Some(head) = self.current else {
            return Operand::Constant(PoolId::FALSE);
        };

        let rhs_bb = self.mir.push_block();
        let short_bb = self.mir.push_block();
        let merge = self.mir.push_block();
        // The merged result is an ordinary block parameter — the same mechanism
        // `ssa.rs` uses for a variable, managed directly here because this value
        // belongs to no local.
        let result = self.mir.push_block_param(merge, PoolId::BOOL, span);

        let (then_, else_) = if short_on {
            // `||`: a true left operand short-circuits.
            (Target::new(short_bb), Target::new(rhs_bb))
        } else {
            // `&&`: a true left operand means the right one decides.
            (Target::new(rhs_bb), Target::new(short_bb))
        };
        self.mir.set_terminator(
            head,
            Terminator::Branch {
                cond: lhs_operand,
                then_,
                else_,
            },
        );
        self.ssa.seal_block(&mut self.mir, rhs_bb);
        self.ssa.seal_block(&mut self.mir, short_bb);

        let short_value = Operand::Constant(if short_on {
            PoolId::TRUE
        } else {
            PoolId::FALSE
        });
        self.mir.set_terminator(
            short_bb,
            Terminator::Goto(Target::with_args(merge, vec![short_value])),
        );

        self.current = Some(rhs_bb);
        let rhs_operand = self.expr(rhs);
        if let Some(block) = self.current.take() {
            self.mir.set_terminator(
                block,
                Terminator::Goto(Target::with_args(merge, vec![rhs_operand])),
            );
        }

        self.ssa.seal_block(&mut self.mir, merge);
        self.current = Some(merge);
        Operand::Value(result)
    }

    /// Lowers `cast(T, x)` and `xx x` alike (ADR-0037 §2, ADR-0046 §2).
    fn cast(&mut self, operand: ExprId, ty: PoolId, span: MirSpan) -> Operand {
        let from_ty = self.ty(operand);
        // An enum converts as its **backing integer** (ADR-0041 §3): the value in the register
        // already *is* an `s64`, so `cast(s64, c)` is a same-width no-op and `cast(u8, c)`
        // narrows like any integer. Recording `s64` rather than the enum type is what lets the
        // verifier's source check keep working — `NumKind` has no enum variant, deliberately,
        // because a conversion has nothing nominal to do.
        let from_num = NumKind::of(self.pool, from_ty).or_else(|| {
            matches!(self.pool.item(from_ty), Item::EnumType { .. })
                .then_some(NumKind::Int(jr_pool::IntKind::S64))
        });
        let (Some(from), Some(_to)) = (from_num, NumKind::of(self.pool, ty)) else {
            self.give_up("a cast between types sema did not reduce to numbers");
            return Operand::Constant(PoolId::VOID_VALUE);
        };
        let value = self.expr(operand);
        self.define(
            ty,
            Rvalue::Convert {
                operand: value,
                from,
            },
            span,
        )
    }

    /// Lowers `base[]` into a fresh slot and returns a load of it (ADR-0044 §2).
    ///
    /// A view is a two-word aggregate, so this is three statements rather than one rvalue: a
    /// `Zero` of the slot, a `Store` of the base's address into `.view_data`, and a `Store` of
    /// the length into `.view_count`. There is no `Rvalue::MakeView`, because MIR has no
    /// aggregate-construction rvalue for `struct` either — a struct is built field by field
    /// through places, and a view is built the same way.
    ///
    /// The `Zero` is not redundant with the two stores: on a target whose pointer is narrower
    /// than its count word, `pair_count` leaves padding between them, and zeroing first makes
    /// that padding defined rather than whatever the stack held.
    fn slice_value(&mut self, base: ExprId, view_ty: PoolId, span: MirSpan) -> Option<Operand> {
        let base_ty = self.ty(base);
        // Auto-deref, matching `jr-sema`'s `check_slice`: `p: *[4]u8` slices through the
        // pointer, and the place is then a deref of that pointer's value.
        let (mut place, mut ty) = if self.pointee(base_ty).is_some() {
            let operand = self.expr(base);
            let pointee = self.pointee(base_ty)?;
            (Place::deref(operand), pointee)
        } else {
            self.place(base)?
        };
        while let Some(pointee) = self.pointee(ty) {
            place = place.project(Projection::Deref);
            ty = pointee;
        }

        let len = self.array_len(ty)?;
        let elem = self.array_elem(ty)?;

        // The `data` word points at element 0. `Projection::Index` of a zero constant rather
        // than the array's own address, so that the pointer's *type* is `*elem` — which is what
        // `Projection::ViewData` promises and what indexing the view will assume for its stride.
        let zero = Operand::Constant(self.pool.int_value(PoolId::S64, 0));
        let first = place.project(Projection::Index(zero));
        let elem_ptr = self.pool.pointer_to(elem);
        let data = self.define(elem_ptr, Rvalue::Address(first), span);

        let slot = self.mir.push_slot(view_ty, None, span);
        let view = Place::slot(slot);
        self.emit(Statement::Zero {
            place: view.clone(),
            span,
        });
        self.emit(Statement::Store {
            place: view.clone().project(Projection::ViewData),
            value: data,
            span,
        });
        let len_constant = Operand::Constant(self.pool.int_value(PoolId::S64, len));
        self.emit(Statement::Store {
            place: view.clone().project(Projection::ViewCount),
            value: len_constant,
            span,
        });

        Some(self.define(view_ty, Rvalue::Load(view), span))
    }

    /// Lowers `base[index]` to a place, emitting the bounds check before it.
    ///
    /// The check is emitted **here**, in the place helper, rather than at each of the two
    /// callers — a load and a store — because a store that skipped it would be the dangerous
    /// half. One emission point means `buf[i] = x` and `x = buf[i]` cannot disagree about
    /// whether the index was checked (ADR-0039 §1).
    fn index_place(&mut self, base: ExprId, index: ExprId) -> Option<(Place, PoolId)> {
        let base_ty = self.ty(base);
        // Auto-deref, matching `jr-sema`'s `check_index`: `p: *[4]u8` indexes through the
        // pointer, and the *place* is then a deref of that pointer.
        let (mut place, mut ty) = if self.pointee(base_ty).is_some() {
            let operand = self.expr(base);
            let pointee = self.pointee(base_ty)?;
            (Place::deref(operand), pointee)
        } else {
            self.place(base)?
        };
        while let Some(pointee) = self.pointee(ty) {
            place = place.project(Projection::Deref);
            ty = pointee;
        }

        let span = self.span(index);

        // Two indexable types, differing in *where the length comes from* and in nothing else.
        // This is the shape ADR-0039 §1 paid for in advance by making `BoundsCheck`'s `len` an
        // `Operand`: a view needs no new statement and no second checking path, so `buf[i]` and
        // `xs[i]` cannot disagree about whether an index was checked.
        //
        // The element place also differs. An array's is a projection *of the array's own
        // storage*; a view's is its `data` word indexed directly, and `Projection::Index` on a
        // pointer place reads through it — the same type-directed rule that makes `p: *[4]u8`
        // indexable at the source level.
        let (mut place, elem, len) = if let Some(elem) = self.array_elem(ty) {
            let len = self.array_len(ty)?;
            // `int_value` takes the raw bit pattern, so the length goes in as-is: a `u64`
            // length is already the two's-complement encoding of the `s64` that holds it, and a
            // length above `i64::MAX` cannot occur — `layout_of` refuses an array that large.
            let constant = self.pool.int_value(PoolId::S64, len);
            (place, elem, Operand::Constant(constant))
        } else {
            let elem = self.view_elem(ty)?;
            let count = self.define(
                PoolId::S64,
                Rvalue::Load(place.clone().project(Projection::ViewCount)),
                span,
            );
            (place.project(Projection::ViewData), elem, count)
        };

        let index_operand = self.expr(index);

        // ADR-0003's explicit check, as a statement of its own before the access. The
        // comparison is unsigned, so one test covers a negative index too.
        self.emit(Statement::BoundsCheck {
            index: index_operand,
            len,
            span,
        });

        place = place.project(Projection::Index(index_operand));
        Some((place, elem))
    }

    fn unary(&mut self, op: jr_hir::UnOp, operand: ExprId, ty: PoolId, span: MirSpan) -> Operand {
        match op {
            // Prefix `*` (ADR-0011) is not arithmetic on a value: it is the address
            // of a place, which is the only form Cranelift's `stack_addr` and a
            // future load/store optimiser can reason about.
            jr_hir::UnOp::AddrOf => match self.place(operand) {
                Some((place, _)) => self.define(ty, Rvalue::Address(place), span),
                None => self.define(ty, Rvalue::Undef, span),
            },
            jr_hir::UnOp::Neg => {
                let value = self.expr(operand);
                self.define(
                    ty,
                    Rvalue::Unary {
                        op: UnOp::Neg,
                        operand: value,
                    },
                    span,
                )
            }
            // `~` is an ordinary value operation, unlike `*`: it reads a value and produces
            // one, so it needs no place (ADR-0042 §4).
            jr_hir::UnOp::BitNot => {
                let value = self.expr(operand);
                self.define(
                    ty,
                    Rvalue::Unary {
                        op: UnOp::BitNot,
                        operand: value,
                    },
                    span,
                )
            }
            jr_hir::UnOp::Not => {
                let value = self.expr(operand);
                self.define(
                    ty,
                    Rvalue::Unary {
                        op: UnOp::Not,
                        operand: value,
                    },
                    span,
                )
            }
        }
    }

    /// Builds a call rvalue, or `None` if the callee is not one this wave lowers.
    fn call_rvalue(&mut self, callee: ExprId, args: &[ExprId]) -> Option<Rvalue> {
        let target = self.direct_callee(callee)?;
        let operands: Vec<Operand> = args.iter().map(|arg| self.expr(*arg)).collect();
        Some(Rvalue::Call {
            callee: Callee::Direct(target),
            args: operands,
        })
    }

    /// The procedure a callee expression names.
    ///
    /// Handles both a procedure declared in this file and, since ADR-0018 §5, one
    /// reached through an `#import` — the latter resolved by `jr-db` from the other
    /// file's signatures rather than looked up here, because ADR-0016 §5 keeps one
    /// file's analysis off another's.
    fn direct_callee(&self, callee: ExprId) -> Option<ProcRef> {
        if callee.index() >= self.body.exprs.len() {
            return None;
        }
        let Expr::Name {
            name: _,
            span: _,
            res,
        } = self.body.expr(callee)
        else {
            return None;
        };
        let res = self.resolve.get(self.scope(), callee).unwrap_or(*res);
        match res {
            Res::Item(item) => {
                let ItemKind::Const {
                    value: ConstValue::Proc(proc),
                } = &self.hir.items.get(item.index())?.kind
                else {
                    return None;
                };
                Some(ProcRef::new(self.file, *proc))
            }
            Res::Imported(import, name) => self.imports.get(import, name),
            Res::Local(_) | Res::Param(_) | Res::Error => None,
        }
    }

    // -------------------------------------------------------------------
    // Places
    // -------------------------------------------------------------------

    /// The local a left-hand side names, if it is one held in a register.
    fn promotable_local(&self, expr: ExprId) -> Option<LocalId> {
        if expr.index() >= self.body.exprs.len() {
            return None;
        }
        let Expr::Name {
            name: _,
            span: _,
            res,
        } = self.body.expr(expr)
        else {
            return None;
        };
        let res = self.resolve.get(self.scope(), expr).unwrap_or(*res);
        match res {
            Res::Local(local) => self.promotable.is_promotable(local).then_some(local),
            Res::Param(_) | Res::Item(_) | Res::Imported(_, _) | Res::Error => None,
        }
    }

    /// The memory location an expression names, and the type stored there.
    fn place(&mut self, expr: ExprId) -> Option<(Place, PoolId)> {
        if expr.index() >= self.body.exprs.len() {
            return None;
        }
        match self.body.expr(expr).clone() {
            Expr::Name {
                name: _,
                span: _,
                res,
            } => {
                let res = self.resolve.get(self.scope(), expr).unwrap_or(res);
                match res {
                    Res::Local(local) => {
                        if self.promotable.is_promotable(local) {
                            // A register-held local has no address. That is exactly
                            // what `escape.rs` guarantees by refusing to promote
                            // anything whose address is taken, so reaching here means
                            // the caller wanted a value and should have asked for one.
                            return None;
                        }
                        let ty = self.local_ty(local);
                        let span = MirSpan::Local(self.body_id, local);
                        let slot = self.slot_for(local, ty, span);
                        Some((Place::slot(slot), ty))
                    }
                    // An aggregate parameter was spilled at entry precisely so that
                    // it has one. A scalar parameter has no place, and nothing in
                    // Jairs-0 can ask for its address.
                    Res::Param(param) => {
                        let slot = self.param_slots.get(&param).copied()?;
                        Some((Place::slot(slot), self.mir.slot(slot).ty))
                    }
                    Res::Item(_) | Res::Imported(_, _) | Res::Error => None,
                }
            }
            Expr::Deref(inner, _) => {
                let inner_ty = self.ty(inner);
                let pointee = self.pointee(inner_ty)?;
                let operand = self.expr(inner);
                Some((Place::deref(operand), pointee))
            }
            Expr::Field {
                receiver,
                name,
                name_span: _,
                span: _,
            } => self.field_place(receiver, name),
            Expr::Index {
                base,
                index,
                index_span: _,
                span: _,
            } => self.index_place(base, index),
            Expr::Literal(_, _)
            | Expr::Binary { .. }
            | Expr::Unary { .. }
            | Expr::Call { .. }
            | Expr::Uninit(_)
            // `buf[]` builds a two-word *value*, so it has no place — matching `jr-sema`'s
            // `is_place`. The value path constructs it.
            | Expr::Slice { .. }
            // A cast yields a value, not a location — matching `jr-sema`'s `is_place`. Both
            // `xx` and a bare `.RED` are values too (ADR-0046 §1).
            | Expr::Cast { .. }
            | Expr::Autocast { .. }
            | Expr::Member { .. }
            | Expr::Run(_, _)
            | Expr::Directive { .. }
            | Expr::Error(_) => None,
        }
    }

    /// The element count of `ty`, looking through any number of pointers.
    ///
    /// Auto-deref, matching `jr-sema`'s `check_field` and `check_index`: `p: *[4]u8` has a
    /// `.count` of 4.
    fn array_len_through_pointers(&self, mut ty: PoolId) -> Option<u64> {
        while let Some(pointee) = self.pointee(ty) {
            ty = pointee;
        }
        self.array_len(ty)
    }

    /// Whether `receiver.name` is an array's `.count` pseudo-field.
    fn is_array_count(&self, receiver: ExprId, name: Symbol) -> bool {
        self.interner.resolve(name) == "count"
            && self.array_len_through_pointers(self.ty(receiver)).is_some()
    }

    /// The element count of `ty`, if it is a `[N]T`.
    fn array_len(&self, ty: PoolId) -> Option<u64> {
        if ty.index() >= self.pool.len() {
            return None;
        }
        match self.pool.item(ty) {
            Item::ArrayType { len, .. } => Some(*len),
            _ => None,
        }
    }

    /// The element type of `ty`, if it is a `[N]T`.
    fn array_elem(&self, ty: PoolId) -> Option<PoolId> {
        if ty.index() >= self.pool.len() {
            return None;
        }
        match self.pool.item(ty) {
            Item::ArrayType { elem, .. } => Some(*elem),
            _ => None,
        }
    }

    /// The element type of `ty` when it is a view, looking through pointers.
    ///
    /// Separate from [`Lower::array_elem`] rather than folded into it, because the two answer
    /// for different types and a caller that accepted either would index an array where it
    /// meant to index a view — which differ in whether the length is a constant.
    fn view_elem(&self, mut ty: PoolId) -> Option<PoolId> {
        while let Some(pointee) = self.pointee(ty) {
            ty = pointee;
        }
        if ty.index() >= self.pool.len() {
            return None;
        }
        match self.pool.item(ty) {
            Item::ViewType { elem } => Some(*elem),
            _ => None,
        }
    }

    /// Whether `receiver.name` is a *view's* `.count`.
    ///
    /// Distinguished from [`Lower::is_array_count`] because the two lower differently: an
    /// array's `.count` folds to a constant and a view's is a load (ADR-0044 §4).
    fn is_view_count(&self, receiver: ExprId, name: Symbol) -> bool {
        self.interner.resolve(name) == "count" && self.view_elem(self.ty(receiver)).is_some()
    }

    /// The value of `name` in the enum `ty`, if `ty` is an enum with such a member.
    ///
    /// Type-directed, which is what makes an *imported* enum work: the enum comes from the
    /// expression's own type rather than from a `Res` and an `EnumId` in another file's arena
    /// (ADR-0047 §1). The pool's member table is keyed on `DeclId` and `record_in` has already
    /// filled it for every import, so no cross-body read is created.
    fn enum_member_value(&self, ty: PoolId, name: Symbol) -> Option<i64> {
        if ty.index() >= self.pool.len() {
            return None;
        }
        let Item::EnumType { decl, .. } = self.pool.item(ty) else {
            return None;
        };
        self.pool
            .enum_members(*decl)?
            .iter()
            .find(|m| m.name == name)
            .map(|m| m.value)
    }

    fn field_place(&mut self, receiver: ExprId, name: Symbol) -> Option<(Place, PoolId)> {
        let receiver_ty = self.ty(receiver);

        // Field access auto-dereferences (`tests/corpus/valid/015-pointers.jr`), so
        // a pointer receiver becomes a dereference of its *value* rather than a
        // projection of a place — a pointer is a register type, and it may not have
        // a place at all.
        let (mut place, mut ty) = if self.pointee(receiver_ty).is_some() {
            let operand = self.expr(receiver);
            let pointee = self.pointee(receiver_ty)?;
            (Place::deref(operand), pointee)
        } else {
            self.place(receiver)?
        };
        while let Some(pointee) = self.pointee(ty) {
            place = place.project(Projection::Deref);
            ty = pointee;
        }

        // `string`'s `.data` and `.count` are pseudo-fields: ADR-0004 fixes the
        // layout in prose only, the pool holds no fields for it, and `jr-sema`
        // hardcodes the two names. Modelling them as struct fields 0 and 1 would
        // assert a layout nothing has committed to.
        if ty == PoolId::STRING {
            let text = self.interner.resolve(name);
            return match text {
                "data" => Some((place.project(Projection::StringData), PoolId::PTR_U8)),
                "count" => Some((place.project(Projection::StringCount), PoolId::S64)),
                _ => None,
            };
        }

        // `[N]T`'s only pseudo-field is `.count`, and it is not a place: the length lives
        // in the *type*, so there is nothing to load. `field_place` returns `None` and the
        // value path folds it to a constant instead (ADR-0039 §5).
        if self.array_len(ty).is_some() {
            return None;
        }

        // A view's `.count` *is* a place — the second word of the pair — which is the one
        // way it differs from an array's (ADR-0044 §4). `.data` is deliberately not offered:
        // sema refuses the name, so reaching here with it would be a sema bug rather than
        // something to serve.
        if let Item::ViewType { .. } = self.pool.item(ty) {
            let text = self.interner.resolve(name);
            return match text {
                "count" => Some((place.project(Projection::ViewCount), PoolId::S64)),
                _ => None,
            };
        }

        let decl = match self.pool.item(ty) {
            // A union's field access is a struct's, identically: the same `Projection::Field`
            // by the same index into the same side table. What differs is the *offset* that
            // index resolves to, which is `jr-pool`'s (ADR-0045 §5).
            Item::StructType { decl } | Item::UnionType { decl } => *decl,
            Item::VoidType
            | Item::BoolType
            | Item::IntType { .. }
            | Item::FloatType { .. }
            | Item::StringType
            | Item::TypeType
            | Item::ErrorType
            | Item::ForeignLibraryType
            | Item::PointerType(_)
            | Item::ArrayType { .. }
            | Item::ViewType { .. }
            | Item::EnumType { .. }
            | Item::ProcType { .. }
            | Item::VoidValue
            | Item::BoolValue(_)
            | Item::IntValue { .. }
            | Item::FloatValue { .. }
            | Item::StrValue(_)
            | Item::TypeValue(_)
            | Item::ProcValue { .. }
            | Item::ForeignLibraryValue(_) => return None,
        };
        let _ = self.file;
        let fields = self.pool.struct_fields(decl)?;
        let index = fields.iter().position(|field| field.name == name)?;
        let field_ty = fields[index].ty;
        let index = u32::try_from(index).ok()?;
        Some((place.project(Projection::Field(index)), field_ty))
    }
}

// ---------------------------------------------------------------------------
// Operator translation
// ---------------------------------------------------------------------------

/// Translates an HIR operator, or `None` for one MIR cannot express.
///
/// `And` and `Or` are the only such operators, and they return `None` because
/// they are lowered as control flow before this is reached. The match is
/// exhaustive so that a new HIR operator is a compile error here.
fn mir_bin_op(op: jr_hir::BinOp) -> Option<BinOp> {
    match op {
        jr_hir::BinOp::Add => Some(BinOp::Add),
        jr_hir::BinOp::Sub => Some(BinOp::Sub),
        jr_hir::BinOp::Mul => Some(BinOp::Mul),
        jr_hir::BinOp::Div => Some(BinOp::Div),
        jr_hir::BinOp::Rem => Some(BinOp::Rem),
        jr_hir::BinOp::WrapAdd => Some(BinOp::WrapAdd),
        jr_hir::BinOp::WrapSub => Some(BinOp::WrapSub),
        jr_hir::BinOp::WrapMul => Some(BinOp::WrapMul),
        jr_hir::BinOp::Eq => Some(BinOp::Eq),
        jr_hir::BinOp::Ne => Some(BinOp::Ne),
        jr_hir::BinOp::Lt => Some(BinOp::Lt),
        jr_hir::BinOp::Le => Some(BinOp::Le),
        jr_hir::BinOp::Gt => Some(BinOp::Gt),
        jr_hir::BinOp::Ge => Some(BinOp::Ge),
        // ADR-0042's bitwise operators. `Shl`/`Shr` are the one binary form whose operands
        // may differ in type, which the verifier allows for exactly these two.
        jr_hir::BinOp::BitAnd => Some(BinOp::BitAnd),
        jr_hir::BinOp::BitOr => Some(BinOp::BitOr),
        jr_hir::BinOp::BitXor => Some(BinOp::BitXor),
        jr_hir::BinOp::Shl => Some(BinOp::Shl),
        jr_hir::BinOp::Shr => Some(BinOp::Shr),
        jr_hir::BinOp::And | jr_hir::BinOp::Or => None,
    }
}

/// The arithmetic hidden inside a compound assignment, or `None` for plain `=`.
///
/// ADR-0002's trapping and wrapping forms stay distinct: `+=` traps and `+%=`
/// wraps, and collapsing them here would discard that quietly.
fn bin_op_of_assign(op: AssignOp) -> Option<BinOp> {
    match op {
        AssignOp::Assign => None,
        AssignOp::AddAssign => Some(BinOp::Add),
        AssignOp::SubAssign => Some(BinOp::Sub),
        AssignOp::MulAssign => Some(BinOp::Mul),
        AssignOp::DivAssign => Some(BinOp::Div),
        AssignOp::RemAssign => Some(BinOp::Rem),
        AssignOp::WrapAddAssign => Some(BinOp::WrapAdd),
        AssignOp::WrapSubAssign => Some(BinOp::WrapSub),
        AssignOp::WrapMulAssign => Some(BinOp::WrapMul),
        // The five bitwise compound forms (ADR-0042 §6).
        AssignOp::BitAndAssign => Some(BinOp::BitAnd),
        AssignOp::BitOrAssign => Some(BinOp::BitOr),
        AssignOp::BitXorAssign => Some(BinOp::BitXor),
        AssignOp::ShlAssign => Some(BinOp::Shl),
        AssignOp::ShrAssign => Some(BinOp::Shr),
    }
}
