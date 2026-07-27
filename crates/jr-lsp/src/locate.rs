//! Turning a byte offset into the HIR node it sits inside.
//!
//! # Why this exists at all, when nothing else in the compiler needs it
//!
//! Because a batch compiler is never asked "what is *here*". Every other consumer of
//! HIR walks it top-down; only an editor starts from a cursor. ADR-0013 deferred
//! `AstIdMap`, which is the structure that would answer this in one lookup, so
//! [ADR-0024](../../../docs/adr/0024-language-server.md) §1 answers it by scanning the
//! spans ADR-0013 *did* put on every node.
//!
//! # Innermost wins, and why that is the whole rule
//!
//! Spans nest: in `add(p.x, 1)` the offset of `x` is inside the field access, which is
//! inside the call. The node a user means is the smallest one containing the cursor, so
//! the scan keeps the narrowest hit rather than the first. Ties — two nodes with
//! identical spans, which lowering does produce — keep the later one, because lowering
//! emits inner nodes after their parents.
//!
//! # Why the arena is part of the answer
//!
//! `FileHir::exprs` and every `Body::exprs` start at index 0, so a bare [`ExprId`] does
//! not say which arena it belongs to. That is the collision [`ExprScope`] exists to
//! prevent, and it has already caused one real bug in `jr-hir`'s `ResolveMap`. So a
//! result is always a `(ExprScope, ExprId)` pair, and every consumer — `ResolveMap`,
//! `TypeMap` — is keyed the same way.
//!
//! # The cost, stated
//!
//! O(nodes in the file) per request. ADR-0013 named exactly this as its own revisit
//! trigger: measure keystroke-to-diagnostic latency, then decide whether `AstIdMap`
//! earns its keep. This module is what makes that measurable; it does not pre-empt it.

use jr_base::{Span, TextSize};
use jr_hir::{Body, ExprId, ExprScope, FileHir};

/// An expression the cursor is inside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Located {
    /// Which expression arena [`Self::expr`] indexes.
    pub scope: ExprScope,
    /// The innermost expression containing the offset.
    pub expr: ExprId,
    /// Its span, so a caller can highlight it without looking it up again.
    pub span: Span,
}

/// The innermost expression in `hir` containing `offset`.
///
/// `None` when the offset is in whitespace, in a comment, or on a token that lowering
/// did not turn into an expression — a type annotation, a parameter name, a brace. That
/// is a real answer and not a failure: an editor showing nothing is correct there.
#[must_use]
pub fn locate(hir: &FileHir, offset: TextSize) -> Option<Located> {
    let mut best: Option<Located> = None;

    // File-level expressions: a constant's value, a `#run`.
    consider(&mut best, ExprScope::TopLevel, &hir.expr_spans, offset);

    // Then every body, under its own scope.
    for (index, body) in hir.bodies.iter().enumerate() {
        let scope = ExprScope::Body(jr_hir::BodyId::from_usize(index));
        consider(&mut best, scope, &body.expr_spans, offset);
    }

    best
}

/// Narrows `best` with any span in `spans` that contains `offset`.
fn consider(best: &mut Option<Located>, scope: ExprScope, spans: &[Span], offset: TextSize) {
    for (index, span) in spans.iter().enumerate() {
        if !contains(*span, offset) {
            continue;
        }
        let candidate = Located {
            scope,
            expr: ExprId::from_usize(index),
            span: *span,
        };
        // `<=` rather than `<`: on a tie the later node wins, and lowering emits an
        // inner expression after the outer one that contains it.
        let better = best.is_none_or(|current| width(*span) <= width(current.span));
        if better {
            *best = Some(candidate);
        }
    }
}

/// Whether `span` contains `offset`, treating the end as exclusive.
///
/// Exclusive because a cursor placed immediately after a name is conventionally still
/// "on" it in an editor, and the *next* token's span starts there — so an inclusive end
/// would make two nodes tie for every boundary and the tie-break would decide silently.
/// A cursor exactly at the end of the last token in a file therefore matches nothing,
/// which is correct: there is no expression there.
fn contains(span: Span, offset: TextSize) -> bool {
    span.start() <= offset && offset < span.end()
}

fn width(span: Span) -> u32 {
    u32::from(span.end()) - u32::from(span.start())
}

/// The span of the name that declares a local, for goto-definition.
///
/// Separate from [`locate`] because a *declaration* is not an expression: a `Local`
/// carries its own `name_span`, and nothing in the expression arenas points at it.
#[must_use]
pub fn local_name_span(body: &Body, local: jr_hir::LocalId) -> Option<Span> {
    body.locals.get(local.index()).map(|local| local.name_span)
}

/// The span of the name that declares a parameter.
///
/// Parameters are not locals: `jr-hir`'s `Body` does not store them at all, so the span
/// lives on `Proc::params`. That asymmetry is `jr-mir`'s too — `MirBody::params`
/// reconstructs them from `Proc::params` — so it is the shape of the HIR rather than an
/// oversight here.
#[must_use]
pub fn param_name_span(
    hir: &FileHir,
    proc: jr_hir::ProcId,
    param: jr_hir::ParamId,
) -> Option<Span> {
    hir.procs
        .get(proc.index())?
        .params
        .get(param.index())
        .map(|param| param.name_span)
}

/// The span of the name that declares a file-level item.
#[must_use]
pub fn item_name_span(hir: &FileHir, item: jr_hir::ItemId) -> Option<Span> {
    hir.items.get(item.index()).map(|item| item.name_span)
}

// ---------------------------------------------------------------------------
// Declaration sites
// ---------------------------------------------------------------------------

/// A declaration's own name token, which is not an expression.
///
/// [`locate`] scans expression arenas, so it answers `None` on the `add` in
/// `add :: (a: s64)` — there is no `Expr::Name` there, only an `Item::name_span`. That
/// made hover on a declaration silently empty, and `verify.lua`'s first draft asserted
/// the emptiness as correct. This is the other half of the lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclSite {
    /// A file-level declaration: a procedure, struct, constant, variable or import.
    Item(jr_hir::ItemId),
    /// A parameter, in the procedure that declares it.
    Param {
        /// The declaring procedure.
        proc: jr_hir::ProcId,
        /// Which parameter.
        param: jr_hir::ParamId,
    },
    /// A local, in the body that declares it.
    Local {
        /// The declaring body.
        body: jr_hir::BodyId,
        /// Which local.
        local: jr_hir::LocalId,
    },
}

/// The declaration whose *name* contains `offset`, if any.
///
/// Deliberately checked only after [`locate`] returns `None`: where a name is used as an
/// expression the resolution is the better answer, because it follows a name to what it
/// means rather than to where the cursor happens to be. Name spans do not overlap
/// expression spans, so the order is a preference rather than a conflict.
///
/// Narrowest-first is unnecessary here: a name token cannot contain another one.
#[must_use]
pub fn locate_declaration(hir: &FileHir, offset: TextSize) -> Option<DeclSite> {
    for (index, item) in hir.items.iter().enumerate() {
        // An unnamed item — a top-level `#run` — has a `name_span` that is not a name.
        // Skipped rather than matched, or hovering `#run` would render the item that
        // happens to be at that index.
        if item.name.is_some() && contains(item.name_span, offset) {
            return Some(DeclSite::Item(jr_hir::ItemId::from_usize(index)));
        }
    }

    for (proc_index, proc) in hir.procs.iter().enumerate() {
        for (param_index, param) in proc.params.iter().enumerate() {
            if contains(param.name_span, offset) {
                return Some(DeclSite::Param {
                    proc: jr_hir::ProcId::from_usize(proc_index),
                    param: jr_hir::ParamId::from_usize(param_index),
                });
            }
        }
    }

    for (body_index, body) in hir.bodies.iter().enumerate() {
        for (local_index, local) in body.locals.iter().enumerate() {
            if contains(local.name_span, offset) {
                return Some(DeclSite::Local {
                    body: jr_hir::BodyId::from_usize(body_index),
                    local: jr_hir::LocalId::from_usize(local_index),
                });
            }
        }
    }

    None
}
