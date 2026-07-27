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
    BinOp, Callee, MirBody, MirSpan, Operand, Poisoned, ProcRef, Rvalue, Statement, Terminator,
    UnOp,
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
    resolve: &ResolveMap,
    types: &TypeMap,
    consts: &ConstValues,
    imports: &ImportedProcs,
    pool: &mut Pool,
) -> Result<MirBody, Poisoned> {
    let ret = expr_type(types, root)?;
    let mut thunk = Thunk {
        hir,
        file,
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
fn expr_type(types: &TypeMap, expr: ExprId) -> Result<PoolId, Poisoned> {
    match types.expr_type(ExprScope::TopLevel, expr) {
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
/// # Why it walks the whole arena instead of finding the roots
///
/// `FileHir::exprs` is the file-level expression arena and nothing else — a
/// procedure body's expressions live in its own `Body::exprs`, which is the
/// distinction [`ExprScope`] exists to make. So every expression a thunk could ever
/// be built from is in here, and walking all of it cannot miss a root.
///
/// The alternative was to mirror `file_consts`' own notion of what wants
/// evaluating. That is narrower and it is *drift waiting to happen*: the two would
/// have to be changed together forever, and the failure mode of forgetting is a
/// body that comptime runs and the inliner rewrote. Over-approximating here costs a
/// procedure or two of missed inlining and cannot be unsound.
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
        if let Ok(target) = resolve_callee(hir, file, resolve, imports, *callee)
            && !out.contains(&target)
        {
            out.push(target);
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
    resolve: &ResolveMap,
    imports: &ImportedProcs,
    callee: ExprId,
) -> Result<ProcRef, Poisoned> {
    let Some(Expr::Name {
        name: _,
        span: _,
        res,
    }) = hir.exprs.get(callee.index()).cloned()
    else {
        return Err(Poisoned::Here("a file-level call has no named callee"));
    };
    let res = resolve.get(ExprScope::TopLevel, callee).unwrap_or(res);
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
        Res::Local(_) | Res::Param(_) | Res::Error => {
            Err(Poisoned::Here("a name failed to resolve at file scope"))
        }
    }
}

struct Thunk<'a> {
    hir: &'a FileHir,
    file: FileId,
    resolve: &'a ResolveMap,
    types: &'a TypeMap,
    consts: &'a ConstValues,
    imports: &'a ImportedProcs,
    pool: &'a mut Pool,
    mir: MirBody,
}

impl Thunk<'_> {
    fn define(&mut self, ty: PoolId, rvalue: Rvalue, expr: ExprId) -> Operand {
        let span = MirSpan::Expr(ExprScope::TopLevel, expr);
        let dest = self.mir.push_value(ty, span);
        let entry = self.mir.entry();
        self.mir
            .stmts_mut(entry)
            .push(Statement::Assign { dest, rvalue, span });
        Operand::Value(dest)
    }

    fn expr(&mut self, id: ExprId) -> Result<Operand, Poisoned> {
        let ty = expr_type(self.types, id)?;
        let node = self
            .hir
            .exprs
            .get(id.index())
            .ok_or(Poisoned::Here("a file-level expression is missing"))?
            .clone();

        match node {
            Expr::Literal(literal, _) => Ok(Operand::Constant(self.literal(&literal, ty))),

            // A `#run` whose value is already known folds to it; otherwise evaluating
            // the `#run` *is* evaluating its inner expression, which is what makes a
            // thunk for `COMPUTED :: #run add(2, 3)` compute `add(2, 3)`.
            Expr::Run(inner, _) => match self.consts.run(ExprScope::TopLevel, id) {
                Some(value) => Ok(Operand::Constant(value)),
                None => self.expr(inner),
            },

            Expr::Name {
                name: _,
                span: _,
                res,
            } => {
                let res = self.resolve.get(ExprScope::TopLevel, id).unwrap_or(res);
                match res {
                    Res::Item(item) => self
                        .consts
                        .item(item)
                        .map(Operand::Constant)
                        .ok_or(Poisoned::Here("a file-level item has no value yet")),
                    Res::Imported(_, _) => {
                        Err(Poisoned::Here("an imported constant has no value here"))
                    }
                    Res::Local(_) | Res::Param(_) | Res::Error => {
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

            Expr::Call {
                callee,
                args,
                span: _,
            } => {
                let target = self.callee(callee)?;
                let mut operands = Vec::with_capacity(args.len());
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
            Expr::Field { .. } | Expr::Deref(_, _) => {
                Err(Poisoned::Here("a file-level expression has no place"))
            }
            Expr::Uninit(_) => Err(Poisoned::Here("`---` has no value")),
            Expr::Error(_) => Err(Poisoned::Here("the expression contains recovered syntax")),
        }
    }

    fn literal(&mut self, literal: &Literal, ty: PoolId) -> PoolId {
        match literal {
            // `value` is a magnitude: `-1` is `Neg` applied to `1`.
            Literal::Int {
                value,
                radix: _,
                overflowed: _,
            } => self.pool.int_value(ty, *value),
            Literal::Bool(value) => self.pool.bool_value(*value),
            Literal::Str(text) => self.pool.str_value(text),
        }
    }

    fn callee(&mut self, callee: ExprId) -> Result<ProcRef, Poisoned> {
        resolve_callee(self.hir, self.file, self.resolve, self.imports, callee)
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
        // Prefix `*` needs a place, and a file-level expression has none.
        jr_hir::UnOp::AddrOf => {
            return Err(Poisoned::Here("a file-level expression has no place"));
        }
    })
}
