//! Turning MIR's HIR-identity provenance back into a source span.
//!
//! # Why MIR does not simply store spans
//!
//! ADR-0017's follow-on work argues it: a byte range in MIR would make built MIR
//! depend on where the text sits, so a whitespace edit anywhere above a procedure
//! would invalidate its lowering. MIR therefore stores *identity* — a
//! [`MirSpan`](crate::MirSpan) naming an HIR expression, local, statement or
//! parameter — and a consumer that needs a location resolves one on demand, here.
//!
//! # Why this is cheap, and why an `AstIdMap` is not needed for it
//!
//! Because ADR-0013 decided that **HIR nodes carry their spans directly**.
//! Resolving one is therefore a field read through the arena that owns the node,
//! not a lookup in a side table. `AstIdMap` — which ADR-0013 deferred — would make
//! node *identity* stable across unrelated edits, which is a different and
//! unmeasured problem; it was never what stood between a trap and its location.
//!
//! ADR-0019 §3 records this because the project believed otherwise for a wave: the
//! `jr-vm` handoff asserted that "nothing resolves a `MirSpan` back to a span" and
//! named the deferred `AstIdMap` as the blocker, while this function had already
//! been resolving them for the CFG diagnostics E0227–E0229. It was private, which
//! is a visibility problem wearing a design problem's clothes.
//!
//! # Why `None` is a real answer
//!
//! [`MirSpan::Synthetic`](crate::MirSpan::Synthetic) marks a value the compiler
//! invented — a block parameter merging two arms, a spill of an aggregate
//! parameter — and there is no source text to point at. An identity may also fail
//! to resolve if it is out of range for its arena, which is a compiler bug rather
//! than a program one. Both cases return `None`, and every caller must treat that
//! as *report without a location* rather than substituting a nearby one: a
//! diagnostic pointing at the wrong line is worse than one that is missing.

use jr_base::Span;
use jr_hir::{Body, FileHir};

use crate::mir::MirSpan;

/// Turns MIR's HIR-identity provenance back into a source span.
///
/// `body` is the HIR body the MIR body was lowered from, and is `None` for a
/// file-level thunk (a `#run` or a constant initialiser), which has locals and
/// statements in no body of its own. Passing `None` for a span that names one
/// yields `None` rather than a wrong answer.
///
/// See the module docs for why `None` must not be papered over.
#[must_use]
pub fn resolve_span(hir: &FileHir, body: Option<&Body>, span: MirSpan) -> Option<Span> {
    match span {
        MirSpan::Expr(_, expr) => {
            let body = body?;
            (expr.index() < body.exprs.len()).then(|| body.expr_span(expr))
        }
        MirSpan::Local(_, local) => body?.locals.get(local.index()).map(|local| local.span),
        MirSpan::Stmt(_, stmt) => {
            let body = body?;
            (stmt.index() < body.stmts.len()).then(|| stmt_span(body.stmt(stmt)))
        }
        MirSpan::Param(proc, index) => hir
            .procs
            .get(proc.index())?
            .params
            .get(index as usize)
            .map(|param| param.name_span),
        MirSpan::Synthetic => None,
    }
}

/// The span of a statement.
///
/// `jr-hir` gives `Expr` a `span()` accessor but not `Stmt`, so the match lives
/// here. It is exhaustive so that a new statement kind is a compile error rather
/// than a silently unspanned diagnostic.
fn stmt_span(stmt: &jr_hir::Stmt) -> Span {
    use jr_hir::Stmt;
    match stmt {
        Stmt::Block(_, span)
        | Stmt::ReturnTuple(_, span)
        | Stmt::LocalTuple { span, .. }
        | Stmt::AssignTuple { span, .. }
        | Stmt::Local(_, span)
        | Stmt::Item(_, span)
        | Stmt::Expr(_, span)
        | Stmt::Return(_, span)
        | Stmt::Error(span) => *span,
        Stmt::Break(_, span) | Stmt::Continue(_, span) | Stmt::Defer(_, span) => *span,
        Stmt::Assign { span, .. }
        | Stmt::If { span, .. }
        | Stmt::While { span, .. }
        | Stmt::For { span, .. } => *span,
    }
}
