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
use crate::inputs::{ConstValues, ImportedProcs};
use crate::mir::{
    BinOp, BlockId, Callee, Facts, FileMir, MirBody, MirSpan, Operand, Place, Poisoned, ProcRef,
    Projection, Rvalue, SlotId, Statement, Target, Terminator, UnOp, Unreachable,
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
            hir, proc, resolve, types, signatures, consts, imports, interner, pool,
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
                    span: _,
                } => {
                    expr_work.push(*cond);
                    stmt_work.push(*inner);
                }
                Stmt::Return(value, _) => {
                    if let Some(value) = value {
                        expr_work.push(*value);
                    }
                }
                Stmt::Break(_) | Stmt::Continue(_) | Stmt::Error(_) => {}
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
            | Stmt::Break(_)
            | Stmt::Continue(_) => {}
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
                if let Some(reason) = scan_name(hir, signatures, reach, consts, imports, *id, res) {
                    return Some(reason);
                }
            }
            Expr::Literal(_, _)
            | Expr::Binary { .. }
            | Expr::Unary { .. }
            | Expr::Call { .. }
            | Expr::Field { .. }
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

/// One iteration of a loop, so `break` and `continue` know where to go.
struct LoopFrame {
    /// Where `continue` jumps.
    header: BlockId,
    /// Where `break` jumps.
    exit: BlockId,
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
            if !escape::is_register_representable(self.pool, *ty) {
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
                for inner in ids {
                    self.stmt(inner);
                }
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
                span: _,
            } => self.while_stmt(cond, body),
            Stmt::Return(value, _) => self.return_stmt(value),
            Stmt::Break(_) => self.jump(true, id),
            Stmt::Continue(_) => self.jump(false, id),
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
        if let Some(init) = init {
            let value = self.expr(init);
            self.emit(Statement::Store {
                place: Place::slot(slot),
                value,
                span,
            });
        }
        // A non-promotable local needs no zero store to avoid a false report: a slot
        // is memory and never an SSA variable, so no read of it can reach the
        // no-definition path at all. Emitting the zeroing that a struct or ADR-0004's
        // `{data, count}` actually requires is codegen's job, because it needs the
        // layout this crate deliberately does not have (ADR-0017 §5).
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
            Item::BoolType => Some(PoolId::FALSE),
            Item::VoidType
            | Item::StringType
            | Item::TypeType
            | Item::ErrorType
            | Item::ForeignLibraryType
            | Item::PointerType(_)
            | Item::StructType { .. }
            | Item::ProcType { .. }
            | Item::VoidValue
            | Item::BoolValue(_)
            | Item::IntValue { .. }
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
    fn jump(&mut self, is_break: bool, at: StmtId) {
        let Some(block) = self.current else { return };
        match self.loops.last() {
            Some(frame) => {
                let target = if is_break { frame.exit } else { frame.header };
                self.mir
                    .set_terminator(block, Terminator::Goto(Target::new(target)));
            }
            None => {
                // Nothing rejects this today: `jr-hir` lowers `break` without
                // checking that it is inside a loop and `jr-sema` ignores it
                // entirely, so MIR is the first pass that can see it. Record it and
                // keep going; the diagnostic belongs to the next wave.
                self.stray.push(MirSpan::Stmt(self.body_id, at));
                self.mir
                    .set_terminator(block, Terminator::Unreachable(Unreachable::StrayJump));
            }
        }
        self.current = None;
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

    fn while_stmt(&mut self, cond: ExprId, body: StmtId) {
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

        self.loops.push(LoopFrame { header, exit });
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
            } => self.binary(op, lhs, rhs, ty, span),
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
            Expr::Field { .. } | Expr::Deref(_, _) => match self.place(id) {
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
            Literal::Int {
                value,
                radix: _,
                overflowed: _,
            } => self.pool.int_value(ty, *value),
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
            | jr_hir::BinOp::Ge => {}
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
            Expr::Literal(_, _)
            | Expr::Binary { .. }
            | Expr::Unary { .. }
            | Expr::Call { .. }
            | Expr::Uninit(_)
            | Expr::Run(_, _)
            | Expr::Directive { .. }
            | Expr::Error(_) => None,
        }
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

        let decl = match self.pool.item(ty) {
            Item::StructType { decl } => *decl,
            Item::VoidType
            | Item::BoolType
            | Item::IntType { .. }
            | Item::StringType
            | Item::TypeType
            | Item::ErrorType
            | Item::ForeignLibraryType
            | Item::PointerType(_)
            | Item::ProcType { .. }
            | Item::VoidValue
            | Item::BoolValue(_)
            | Item::IntValue { .. }
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
    }
}
