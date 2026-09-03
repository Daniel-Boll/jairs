//! Lowering a *file-level* expression to a MIR body the VM can run.
//!
//! # Why this is separate from `build.rs`
//!
//! `build.rs` lowers a procedure body: locals, control flow, places, Braun's SSA
//! construction. A file-level expression has none of those. Jairs-0's file scope
//! admits `MESSAGE :: "hello";`, `STDOUT :: 1;`, `COMPUTED :: #run add(2, 3);` and
//! `#run report();` — literals, arithmetic, calls, and references to other
//! constants. There is no `if`, no `while`, no local and no assignment, so there is
//! nothing for a CFG or an SSA builder to do.
//!
//! Reusing `build.rs` would mean fabricating a `jr_hir::Body` to hold an expression
//! that is not in one: `FileHir::exprs` is a *different arena* from every
//! `Body::exprs`, which is exactly the distinction [`ExprScope`] exists to make and
//! which has already caused one real collision bug in `jr-hir`'s `ResolveMap`. A
//! forty-line recursive emitter over the subset that can actually appear is smaller
//! than the adapter would be, and it cannot get the arena wrong.
//!
//! # Why a thunk is a procedure at all
//!
//! ADR-0018 §3 evaluates a constant by running it, and the VM runs procedures. So the
//! expression becomes a body with one block that computes it and returns it. The
//! [`ProcRef`] is synthetic — its `ProcId` is past the end of `FileHir::procs`, so it
//! collides with no real procedure — because `PLAN.md` §3.1's invariant is that
//! comptime and runtime execute the same MIR, and the cheapest way to honour that is
//! for comptime to have no second execution path at all.
//!
//! # What it refuses, and why each refusal is right
//!
//! - **A directive.** `libc :: #system_library "c";` is comptime-only and has no
//!   runtime value at all, which `jr_pool::LayoutError::ComptimeOnly` says from the
//!   layout side. Refusing means it simply has no const value, which is correct
//!   rather than a gap.
//! - **An imported constant.** Its value would come from another file's const
//!   evaluation, which is the cross-body read ADR-0017 §3 keeps out of the built-MIR
//!   query.
//! - **A place.** `"abc".data` has no address, and a file-level expression has no
//!   slot to put one in.

use jr_base::FileId;
use jr_hir::{
    ConstValue, Expr, ExprId, ExprScope, FileHir, ItemKind, Literal, ProcId, Res, ResolveMap,
};
use jr_pool::{Pool, PoolId};
use jr_sema::TypeMap;

use crate::inputs::{ConstValues, ImportedProcs};
use crate::mir::{
    BinOp, Callee, MirBody, MirSpan, Operand, Place, Poisoned, ProcRef, Rvalue, Statement,
    Terminator, UnOp,
};
use crate::verify;

/// A [`ProcRef`] for the thunk of the `index`th file-level expression.
///
/// The `ProcId` starts past the end of `FileHir::procs`, so a thunk can never be
/// mistaken for a declared procedure and both can live in one lookup table.
#[must_use]
pub fn thunk_ref(hir: &FileHir, file: FileId, index: usize) -> ProcRef {
    ProcRef::new(file, ProcId::from_usize(hir.procs.len() + index))
}

/// Lowers one file-level expression into a runnable body.
///
/// # Errors
/// Returns [`Poisoned`] when the expression cannot be lowered honestly. See the
/// module docs for the list; every entry means "there is no const value here",
/// which the caller reports or ignores as appropriate.
pub fn lower_const(
    hir: &FileHir,
    file: FileId,
    proc: ProcRef,
    root: ExprId,
    scope: ExprScope,
    resolve: &ResolveMap,
    types: &TypeMap,
    consts: &ConstValues,
    imports: &ImportedProcs,
    pool: &mut Pool,
) -> Result<MirBody, Poisoned> {
    // The arena `scope` names. A body's expressions live in that body, not in the file (ADR-0069 §2),
    // and both start at index 0 — so reading the wrong one yields a different expression rather than an
    // error, which is why the arena travels with the scope rather than being assumed.
    let exprs: &[Expr] = match scope {
        ExprScope::TopLevel => &hir.exprs,
        ExprScope::Body(body) => match hir.bodies.get(body.index()) {
            Some(b) => &b.exprs,
            None => return Err(Poisoned::Here("a `#run` in a body that does not exist")),
        },
    };
    let ret = expr_type(types, scope, root)?;
    let mut thunk = Thunk {
        hir,
        file,
        scope,
        exprs,
        resolve,
        types,
        consts,
        imports,
        pool,
        mir: MirBody::new(proc, ret),
    };
    let value = thunk.expr(root)?;
    let entry = thunk.mir.entry();
    let term = if ret == PoolId::VOID {
        Terminator::Return(None)
    } else {
        Terminator::Return(Some(value))
    };
    thunk.mir.set_terminator(entry, term);
    verify::assert_valid(&thunk.mir, thunk.pool);
    Ok(thunk.mir)
}

/// The type sema gave a file-level expression.
fn expr_type(types: &TypeMap, scope: ExprScope, expr: ExprId) -> Result<PoolId, Poisoned> {
    match types.expr_type(scope, expr) {
        None => Err(Poisoned::Here("a file-level expression was never typed")),
        Some(PoolId::ERROR) => Err(Poisoned::Here("a file-level expression has an error type")),
        Some(ty) => Ok(ty),
    }
}

/// Every procedure a file's compile-time evaluation could possibly call.
///
/// ADR-0021 §2 needs this so that the optimized-MIR query can leave those bodies
/// byte-identical to their built form: comptime executes MIR lowered inside
/// `file_consts`, and if the back end were handed an *inlined* version of a body
/// the VM ran uninlined, `PLAN.md` §3.1's invariant would hold only as far as the
/// inliner is correct. Freezing them makes it hold by construction.
///
/// # Why the file arena is walked whole and a body's is not
///
/// `FileHir::exprs` is the file-level expression arena and nothing else — a
/// procedure body's expressions live in its own `Body::exprs`, which is the
/// distinction [`ExprScope`] exists to make. So every expression a *file-level* thunk
/// could be built from is in there, and walking all of it cannot miss a root while
/// costing "a procedure or two of missed inlining".
///
/// **That argument does not transfer to a body** (ADR-0069 §2). A body's arena holds
/// every expression in the body, not just the comptime-reachable ones, so the same
/// whole-arena walk froze almost every procedure in the program — it disabled the
/// bounds-check strip and two `optimized_mir` tests failed immediately. So a body's
/// roots are found first (its `Expr::Run`s) and only their subtrees are walked, with
/// `child_exprs` exhaustive so a new `Expr` variant is a compile error rather than a
/// subtree silently not walked.
///
/// The alternative was to mirror `file_consts`' own notion of what wants
/// evaluating. That is narrower and it is *drift waiting to happen*: the two would
/// have to be changed together forever, and the failure mode of forgetting is a
/// body that comptime runs and the inliner rewrote.
///
/// This is intentionally the direct callees only. The transitive closure needs
/// callee *bodies*, which is a cross-body read and therefore the optimized query's
/// business rather than this function's.
#[must_use]
pub fn const_callees(
    hir: &FileHir,
    file: FileId,
    resolve: &ResolveMap,
    imports: &ImportedProcs,
) -> Vec<ProcRef> {
    let mut out = Vec::new();
    for expr in &hir.exprs {
        let Expr::Call { callee, .. } = expr else {
            continue;
        };
        // Resolution failures are simply skipped: a call a thunk cannot resolve is
        // one it refuses to lower, so comptime never runs it either.
        if let Ok(target) = resolve_callee(
            hir,
            file,
            ExprScope::TopLevel,
            &hir.exprs,
            resolve,
            imports,
            *callee,
        ) && !out.contains(&target)
        {
            out.push(target);
        }
    }
    // **And every call inside a body**, because a `#run` may now live in one (ADR-0069 §2). Missing this
    // would let the inliner rewrite a body that comptime calls, which is exactly the hazard ADR-0021 §2
    // wrote this function to prevent — and it would be *silent*, since the inlined body is still
    // correct at run time and only the comptime result would differ.
    //
    // Over-approximating deliberately, as the docs above argue: every call in every body, not only those
    // a `#run` reaches. The cost is a procedure or two of missed inlining and it cannot be unsound.
    for (index, body) in hir.bodies.iter().enumerate() {
        let scope = ExprScope::Body(jr_hir::BodyId::from_usize(index));
        // **Only the calls a `#run` can reach**, not every call in the body — and this is the one place
        // the whole-arena argument above does *not* transfer. A file-level arena holds only file-level
        // expressions, so walking all of it costs "a procedure or two of missed inlining". A *body's*
        // arena holds every expression in that body, so the same walk froze almost every procedure in
        // the program: it disabled the bounds-check strip and two optimized-MIR tests said so
        // immediately.
        //
        // So the roots are found first — the `#run` expressions — and only their subtrees are walked.
        for (run_index, run) in body.exprs.iter().enumerate() {
            if !matches!(run, Expr::Run(_, _)) {
                continue;
            }
            let mut work = vec![ExprId::from_usize(run_index)];
            let mut seen = vec![false; body.exprs.len()];
            while let Some(id) = work.pop() {
                let Some(slot) = seen.get_mut(id.index()) else {
                    continue;
                };
                if *slot {
                    continue;
                }
                *slot = true;
                let Some(expr) = body.exprs.get(id.index()) else {
                    continue;
                };
                if let Expr::Call { callee, args, .. } = expr {
                    if let Ok(target) =
                        resolve_callee(hir, file, scope, &body.exprs, resolve, imports, *callee)
                        && !out.contains(&target)
                    {
                        out.push(target);
                    }
                    work.extend(args.iter().copied());
                }
                work.extend(child_exprs(expr));
            }
        }
    }
    out
}

/// The procedure a file-level call's callee expression names.
///
/// Shared with [`Thunk::callee`] so that [`const_callees`] cannot disagree with what
/// a thunk would actually call — the two answering differently is the only way
/// ADR-0021 §2's argument could quietly fail.
///
/// # Errors
/// [`Poisoned`] with the specific reason, because a thunk's refusal reason is a
/// snapshot key and each of these three cases reads differently in a dump.
fn resolve_callee(
    hir: &FileHir,
    file: FileId,
    scope: ExprScope,
    exprs: &[Expr],
    resolve: &ResolveMap,
    imports: &ImportedProcs,
    callee: ExprId,
) -> Result<ProcRef, Poisoned> {
    // Read from the arena `scope` names, not from the file's: a `#run` inside a body has its callee
    // expression in *that body* (ADR-0069 §2), and both arenas start at index 0 — so reading the file's
    // found a different expression and reported "a file-level call has no named callee" for a perfectly
    // good call.
    let Some(Expr::Name {
        name: _,
        module: _,
        span: _,
        res,
    }) = exprs.get(callee.index()).cloned()
    else {
        return Err(Poisoned::Here("a `#run` call has no named callee"));
    };
    let res = resolve.get(scope, callee).unwrap_or(res);
    match res {
        Res::Item(item) => {
            let ItemKind::Const {
                value: ConstValue::Proc(proc),
            } = &hir
                .items
                .get(item.index())
                .ok_or(Poisoned::Here("a name resolved to no item"))?
                .kind
            else {
                return Err(Poisoned::Here(
                    "a call to something that is not a procedure",
                ));
            };
            Ok(ProcRef::new(file, *proc))
        }
        Res::Imported(import, name) => imports.get(import, name).ok_or(Poisoned::Here(
            "a cross-file call needs the callee's signatures",
        )),
        // A promoted name cannot occur at file scope: `using` prefixes a local or a parameter, and
        // neither exists outside a body. Listed rather than `_`-armed so a future `Res` variant is
        // a compile error here rather than silently taking this branch.
        Res::Local(_) | Res::Param(_) | Res::Promoted { .. } | Res::Error => {
            Err(Poisoned::Here("a name failed to resolve at file scope"))
        }
    }
}

struct Thunk<'a> {
    hir: &'a FileHir,
    file: FileId,
    /// Which expression arena `exprs` is, so every map lookup keys on the right one.
    ///
    /// A `#run` at file scope and one inside a body index **different arenas that both start at 0**
    /// (ADR-0069 §2), so a `TypeMap` or `ResolveMap` lookup with the wrong scope silently answers about
    /// a different expression. This was `ExprScope::TopLevel` in six hardwired places before a body
    /// `#run` existed.
    scope: ExprScope,
    /// The arena `scope` names — the file's for a constant, a body's for a `#run` inside one.
    exprs: &'a [Expr],
    resolve: &'a ResolveMap,
    types: &'a TypeMap,
    consts: &'a ConstValues,
    imports: &'a ImportedProcs,
    pool: &'a mut Pool,
    mir: MirBody,
}

impl Thunk<'_> {
    fn define(&mut self, ty: PoolId, rvalue: Rvalue, expr: ExprId) -> Operand {
        let span = MirSpan::Expr(self.scope, expr);
        let dest = self.mir.push_value(ty, span);
        let entry = self.mir.entry();
        self.mir
            .stmts_mut(entry)
            .push(Statement::Assign { dest, rvalue, span });
        Operand::Value(dest)
    }

    fn expr(&mut self, id: ExprId) -> Result<Operand, Poisoned> {
        let ty = expr_type(self.types, self.scope, id)?;
        let node = self
            .exprs
            .get(id.index())
            .ok_or(Poisoned::Here("a file-level expression is missing"))?
            .clone();

        match node {
            Expr::Literal(literal, _) => Ok(Operand::Constant(self.literal(&literal, ty))),

            // **A fixed array literal is refused at compile time** (ADR-0194 §4), and refused rather than
            // left to a placeholder: a thunk produces one `Operand`, and an array's value is a run of
            // bytes that would have to be interned as a static array and referred to by address. The pool
            // *can* build one — `static_array` is what the field and member tables use — but wiring it
            // here means deciding what a `ConstValue` holding an aggregate is, which no caller has needed.
            //
            // So `A :: s64.[1, 2, 3];` at file scope says so, and `a := s64.[1, 2, 3];` inside a body —
            // which is every counted use in real Jai code — lowers to a slot and works.
            Expr::ArrayLit { .. } => Err(Poisoned::Here(
                "an array literal has no compile-time value yet (ADR-0194 §4)",
            )),

            // A `#run` whose value is already known folds to it; otherwise evaluating
            // the `#run` *is* evaluating its inner expression, which is what makes a
            // thunk for `COMPUTED :: #run add(2, 3)` compute `add(2, 3)`.
            Expr::Run(inner, _) => match self.consts.run(self.scope, id) {
                Some(value) => Ok(Operand::Constant(value)),
                None => self.expr(inner),
            },

            Expr::Name {
                name: _,
                module: _,
                span: _,
                res,
            } => {
                let res = self.resolve.get(self.scope, id).unwrap_or(res);
                match res {
                    Res::Item(item) => self
                        .consts
                        .item(item)
                        .map(Operand::Constant)
                        .ok_or(Poisoned::Here("a file-level item has no value yet")),
                    Res::Imported(_, _) => {
                        Err(Poisoned::Here("an imported constant has no value here"))
                    }
                    // Same as above: no `using` binding exists at file scope.
                    Res::Local(_) | Res::Param(_) | Res::Promoted { .. } | Res::Error => {
                        Err(Poisoned::Here("a name failed to resolve at file scope"))
                    }
                }
            }

            Expr::Binary {
                op,
                lhs,
                rhs,
                span: _,
            } => {
                let op = bin_op(op)?;
                let lhs = self.expr(lhs)?;
                let rhs = self.expr(rhs)?;
                Ok(self.define(ty, Rvalue::Binary { op, lhs, rhs }, id))
            }

            Expr::Unary {
                op,
                operand,
                span: _,
            } => {
                let op = un_op(op)?;
                let operand = self.expr(operand)?;
                Ok(self.define(ty, Rvalue::Unary { op, operand }, id))
            }

            // A `cast` inside a `#run` is evaluated, not skipped: `COMPUTED :: #run
            // cast(u8, 65)` must fold in the VM exactly as it would at runtime, which is
            // PLAN.md §3.1's same-MIR invariant applied to this wave's new rvalue.
            Expr::Cast {
                ty: _,
                operand,
                span: _,
            } => {
                let from_ty = expr_type(self.types, self.scope, operand)?;
                let from = crate::mir::NumKind::of(self.pool, from_ty).ok_or(Poisoned::Here(
                    "a cast from a type comptime evaluation cannot reduce to a number",
                ))?;
                let operand = self.expr(operand)?;
                Ok(self.define(ty, Rvalue::Convert { operand, from }, id))
            }

            Expr::Call {
                callee,
                args,
                arg_names: _,
                span: _,
            } => {
                // **A call sema already folded is its value** (ADR-0180 §3). `jr-mir`'s body builder
                // has consulted this channel since `type_info` needed it — a folded call *names no
                // procedure*, so resolving a callee for one is asking the wrong question — and this
                // arm never did. The asymmetry was one line, and its whole visible effect was that an
                // intrinsic worked in a procedure body and was E0230 at file scope:
                //
                //     HERE :: os();   // "a name failed to resolve at file scope"
                //
                // reported against the *callee*, because `callee()` looked `os` up as a name and it is
                // not one. That is what forced `Window.layout_is_sdl2` to be a procedure rather than a
                // constant, and it would have made `os()` unusable for the one thing it is for:
                // selecting a per-OS constant at file scope.
                if let Some(value) = self.consts.run(self.scope, id) {
                    return Ok(Operand::Constant(value));
                }
                let target = self.callee(callee)?;
                let mut operands = Vec::with_capacity(args.len() + 1);
                // **A comptime call passes a context too** (ADR-0057 §2), because the callee's
                // signature takes one — a thunk is a `#c_call`-shaped entry with no context of its
                // own, so it allocates a fresh zeroed one per call. Without this the callee was
                // "called a procedure taking 3 arguments with 2", the shift ADR-0053 §1 records.
                //
                // The context need not persist between calls: const-eval reads only a constant's
                // *result*, and nothing at file scope observes a mutation (E0254 refuses `context`
                // there). A fresh slot each time is therefore correct and simplest.
                if self.callee_receives_context(target) {
                    let ctx_ty = self.pool.context_type();
                    let slot = self.mir.push_slot(ctx_ty, None, MirSpan::Synthetic);
                    let ptr_ty = self.pool.context_pointer();
                    let address = self.define(ptr_ty, Rvalue::Address(Place::slot(slot)), id);
                    operands.push(address);
                }
                // **The written arguments must be all of them.** Const-eval runs before the check phase
                // that fills defaults and reorders named arguments (`consts.rs` argues why, ADR-0018
                // §3), so a call that omits a defaulted argument arrives here one operand short. Left
                // unchecked it built a short call and the *interpreter* reported
                // "internal compiler error: called a procedure taking 3 arguments with 2" — compiler
                // internals shown for a program whose only fault is a construct const-eval does not
                // support yet (ADR-0069 §3). Refused here, where the reason can be said plainly.
                if let Some(declared) = self.declared_param_count(target)
                    && declared != args.len()
                {
                    return Err(Poisoned::Here(
                        "a `#run` call must pass every argument: a default or named argument needs the \
                         check phase, which has not run yet",
                    ));
                }
                for arg in &args {
                    operands.push(self.expr(*arg)?);
                }
                Ok(self.define(
                    ty,
                    Rvalue::Call {
                        callee: Callee::Direct(target),
                        args: operands,
                    },
                    id,
                ))
            }

            // Each of these is argued in the module docs.
            Expr::Directive { .. } => Err(Poisoned::Here("a directive has no runtime value")),
            // An index needs a place for the same reason a field access does — and a
            // file-level expression has none. `[N]u8` cannot appear in a `::` constant
            // anyway, since there is no array literal to initialise one with (ADR-0039 §6).
            Expr::Field { .. } | Expr::Index { .. } | Expr::Deref(_, _) => {
                Err(Poisoned::Here("a file-level expression has no place"))
            }
            // `buf[]` needs the *address* of a local's storage, and a file-level expression
            // has no frame to take one in. Refused rather than half-built.
            Expr::Slice { .. } => Err(Poisoned::Here("`[]` has no place at file level")),
            // Both are legal at file level in principle, and neither is reachable: a `::`
            // constant's initialiser is typed with no expectation, so sema has already refused
            // `X :: xx 1;` (E0242) and `X :: .RED;` (E0244) before a thunk is built. Refused
            // here rather than lowered, because a thunk that guessed a target type would be
            // inventing the very thing the diagnostic says is missing.
            Expr::Autocast { .. } => Err(Poisoned::Here("`xx` has no context at file level")),
            Expr::Member { .. } => Err(Poisoned::Here(
                "a bare enum member has no context at file level",
            )),
            // `context` at file scope is refused by sema (E0254, ADR-0057 §3): a constant's value is
            // computed before any call, so no context has been passed. Reaching here means sema and
            // const-eval disagree, which is a poison rather than a placeholder.
            Expr::Context(_) => Err(Poisoned::Here("`context` has no value at file scope")),
            Expr::Uninit(_) => Err(Poisoned::Here("`---` has no value")),
            Expr::Error(_) => Err(Poisoned::Here("the expression contains recovered syntax")),
        }
    }

    fn literal(&mut self, literal: &Literal, ty: PoolId) -> PoolId {
        match literal {
            // Wrapped into the destination's kind, because `int_value` takes raw bits and a
            // negative literal's bits are its two's-complement encoding. The same
            // `IntKind::wrap` the interpreter and `constprop` use, so a constant folded here
            // and one computed at run time cannot differ (ADR-0038 §2).
            //
            // A literal whose type is not an integer — which sema has already rejected —
            // interns its low 64 bits rather than panicking, since lowering must not
            // introduce a second refusal path.
            Literal::Int {
                value,
                radix: _,
                overflowed: _,
            } => {
                let bits = jr_pool::IntKind::of(self.pool, ty)
                    .map_or(*value as u64, |kind| kind.wrap(*value));
                self.pool.int_value(ty, bits)
            }
            // Re-encoded into the destination width, exactly as `build.rs`'s `constant` does
            // — the two must agree, because a `#run` folds through this path and the same
            // literal at run time folds through that one (PLAN.md §3.1).
            Literal::Float { bits, malformed: _ } => {
                let value = f64::from_bits(*bits);
                let encoded =
                    jr_pool::FloatKind::of(self.pool, ty).map_or(*bits, |kind| kind.encode(value));
                self.pool.float_value(ty, encoded)
            }
            Literal::Bool(value) => self.pool.bool_value(*value),
            Literal::Str(text) => self.pool.str_value(text),
            // `null` is the zero pointer of `ty` (ADR-0060 §1), the same `int_value(ty, 0)`
            // `build.rs` folds — the two must agree because a `#run` folds through here and the
            // same literal at run time folds through there (PLAN.md §3.1). A comptime `null` is
            // fine; it is a comptime *`malloc`* that ADR-0006 refuses, not a null pointer.
            Literal::Null => self.pool.int_value(ty, 0),
        }
    }

    /// Whether a comptime callee receives the implicit context (ADR-0057 §3).
    ///
    /// Only a *local* callee is reachable in a thunk — file-level const-eval calls procedures in the
    /// same file — so its `c_call`/`foreign` flags are in this HIR, the same two `jr-mir`'s lowering
    /// reads. A cross-file comptime call is refused elsewhere, so `false` for one is harmless.
    fn callee_receives_context(&self, target: ProcRef) -> bool {
        // A same-file callee is asked of this file's HIR; an **imported** one is asked of
        // `ImportedProcs`, which records the flag for exactly this reason — the callee's
        // `#c_call`/`#foreign` status is in its *own* file's HIR, which lowering does not have
        // (ADR-0057 §3, and `ImportedProc::receives_context`'s own docs).
        //
        // **The `target.file == self.file` gate used to be the whole answer**, which was correct only
        // while a cross-file `#run` was impossible: an imported callee answered `false`, so no context
        // was passed and the interpreter reported "called a procedure taking 2 arguments with 1". A
        // silent argument-count mismatch, and the reason ADR-0069 §1 could not stop at putting the
        // bytecode in the program.
        if target.file == self.file {
            return self
                .hir
                .procs
                .get(target.proc.index())
                .is_some_and(|p| !(p.c_call || p.foreign.is_some()));
        }
        self.imports.receives_context(target)
    }

    /// How many parameters `target` declares, for the arity check above.
    ///
    /// `None` for a procedure in another file: its parameter list is in its own HIR, which this crate
    /// does not have (the same limit `ImportedProc::receives_context` exists to work around). A
    /// cross-file call therefore keeps the old behaviour — the interpreter's arity error — rather than a
    /// wrong refusal, which is the safe direction.
    fn declared_param_count(&self, target: ProcRef) -> Option<usize> {
        (target.file == self.file)
            .then(|| self.hir.procs.get(target.proc.index()))
            .flatten()
            .map(|proc| proc.params.len())
    }

    fn callee(&mut self, callee: ExprId) -> Result<ProcRef, Poisoned> {
        resolve_callee(
            self.hir,
            self.file,
            self.scope,
            self.exprs,
            self.resolve,
            self.imports,
            callee,
        )
    }
}

/// Every expression one expression directly contains (ADR-0069 §2).
///
/// Used to walk a `#run`'s subtree inside a body without walking the whole body — see
/// [`const_callees`] for why the difference matters. **Exhaustive**, so a new `Expr` variant is a
/// compile error here rather than a subtree silently not walked, which would leave a comptime-called
/// body unfrozen and let the inliner rewrite it (ADR-0021 §2's hazard).
///
/// A call's *arguments* are handled by the caller, which also needs the callee separately.
fn child_exprs(expr: &Expr) -> Vec<ExprId> {
    match expr {
        Expr::Binary { lhs, rhs, .. } => vec![*lhs, *rhs],
        Expr::Unary { operand, .. } | Expr::Autocast { operand, .. } => vec![*operand],
        Expr::Cast { operand, .. } => vec![*operand],
        Expr::Run(inner, _) => vec![*inner],
        Expr::ArrayLit { elem_ty, elems, .. } => {
            let mut out = vec![*elem_ty];
            out.extend(elems.iter().copied());
            out
        }
        Expr::Field { receiver, .. } => vec![*receiver],
        Expr::Index { base, index, .. } => vec![*base, *index],
        Expr::Slice { base, .. } => vec![*base],
        Expr::Deref(inner, _) => vec![*inner],
        Expr::Call { callee, args, .. } => {
            let mut out = vec![*callee];
            out.extend(args.iter().copied());
            out
        }
        // Leaves: nothing to walk into.
        Expr::Literal(_, _)
        | Expr::Name { .. }
        | Expr::Member { .. }
        | Expr::Context(_)
        | Expr::Uninit(_)
        | Expr::Directive { .. }
        | Expr::Error(_) => Vec::new(),
    }
}

/// MIR's operator for a HIR one.
///
/// `&&` and `||` are refused rather than translated: MIR has no such operator
/// because they short-circuit, and short-circuiting needs control flow that a
/// single-block thunk does not have. Nothing at file scope in the corpus uses one.
fn bin_op(op: jr_hir::BinOp) -> Result<BinOp, Poisoned> {
    Ok(match op {
        jr_hir::BinOp::Add => BinOp::Add,
        jr_hir::BinOp::Sub => BinOp::Sub,
        jr_hir::BinOp::Mul => BinOp::Mul,
        jr_hir::BinOp::Div => BinOp::Div,
        jr_hir::BinOp::Rem => BinOp::Rem,
        jr_hir::BinOp::WrapAdd => BinOp::WrapAdd,
        jr_hir::BinOp::WrapSub => BinOp::WrapSub,
        jr_hir::BinOp::WrapMul => BinOp::WrapMul,
        jr_hir::BinOp::BitAnd => BinOp::BitAnd,
        jr_hir::BinOp::BitOr => BinOp::BitOr,
        jr_hir::BinOp::BitXor => BinOp::BitXor,
        jr_hir::BinOp::Shl => BinOp::Shl,
        jr_hir::BinOp::Shr => BinOp::Shr,
        jr_hir::BinOp::Eq => BinOp::Eq,
        jr_hir::BinOp::Ne => BinOp::Ne,
        jr_hir::BinOp::Lt => BinOp::Lt,
        jr_hir::BinOp::Le => BinOp::Le,
        jr_hir::BinOp::Gt => BinOp::Gt,
        jr_hir::BinOp::Ge => BinOp::Ge,
        jr_hir::BinOp::And | jr_hir::BinOp::Or => {
            return Err(Poisoned::Here(
                "short-circuiting at file scope needs control flow a thunk has not got",
            ));
        }
    })
}

fn un_op(op: jr_hir::UnOp) -> Result<UnOp, Poisoned> {
    Ok(match op {
        jr_hir::UnOp::Neg => UnOp::Neg,
        jr_hir::UnOp::Not => UnOp::Not,
        jr_hir::UnOp::BitNot => UnOp::BitNot,
        // Prefix `*` needs a place, and a file-level expression has none.
        jr_hir::UnOp::AddrOf => {
            return Err(Poisoned::Here("a file-level expression has no place"));
        }
    })
}
