//! The three diagnostics that need a control-flow graph, and nothing else.
//!
//! # Why they live here and not in `jr-sema`
//!
//! `jr-sema`'s crate docs defer two of these by name — definite assignment and
//! missing `return` — on the grounds that "whether every path through a non-`void`
//! procedure returns is control flow, and needs MIR's CFG rather than a syntax
//! walk". A syntax walk can approximate both and gets them wrong in opposite
//! directions: it reports a procedure whose every branch returns, and it misses one
//! that returns only inside an `if`. The CFG answers exactly.
//!
//! The third, a `break` or `continue` outside a loop, is not deferred so much as
//! *forgotten*: `jr-hir` lowers both unconditionally without checking, and
//! `jr-sema` ignores the statements entirely, so before this module nothing in the
//! compiler rejected it.
//!
//! # Why lowering records facts instead of reporting them
//!
//! Two of the three fall out of building the CFG rather than out of inspecting it.
//! An undefined read is precisely a variable Braun's construction found no
//! definition for, and a stray jump is precisely a `break` with an empty loop
//! stack — both are known at the moment they happen and are cheap to note and
//! expensive to rediscover. But `build.rs` raises no diagnostics, deliberately: the
//! wording and the code of a diagnostic belong with the rule, not with the walk
//! that stumbled over it. So lowering writes [`crate::Facts`] and this module turns
//! them into diagnostics.
//!
//! The third genuinely needs the finished graph: whether a `FellOffEnd` terminator
//! is *reachable* cannot be known until every block exists.
//!
//! # Why a refused body is silent
//!
//! A body `lower_body` refused has no CFG, so none of these questions can be asked
//! of it — and it was refused because something earlier already reported the cause
//! (ADR-0017 §4), so adding a second message about the same line would be noise.
//! Refusals contribute nothing here, which is why a poisoned file gets exactly the
//! diagnostics its real error deserves.

use jr_base::Interner;
use jr_diag::{Diagnostic, Diagnostics};
use jr_hir::{FileHir, ProcId};

use crate::code;
use crate::mir::{FileMir, MirBody, Terminator, Unreachable};
use crate::span::resolve_span;

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// The CFG diagnostics for every lowered body in a file.
#[must_use]
pub fn file_diagnostics(hir: &FileHir, mir: &FileMir, interner: &Interner) -> Diagnostics {
    let mut out = Diagnostics::new();
    for (proc, outcome) in mir.iter() {
        // A refused body has no CFG and no unreported error; see the module docs.
        let Ok(body) = outcome else { continue };
        out.extend(body_diagnostics(hir, proc, body, interner).into_vec());
    }
    out
}

/// The CFG diagnostics for one lowered body.
#[must_use]
pub fn body_diagnostics(
    hir: &FileHir,
    proc: ProcId,
    mir: &MirBody,
    interner: &Interner,
) -> Diagnostics {
    let mut out = Diagnostics::new();
    let hir_body = hir
        .procs
        .get(proc.index())
        .and_then(|data| data.body)
        .and_then(|id| hir.bodies.get(id.index()));

    for read in &mir.facts().undefined_reads {
        let name = hir_body
            .and_then(|body| body.locals.get(read.local.index()))
            .map_or_else(
                || String::from("a local"),
                |local| format!("`{}`", hir.symbol_text(local.name, interner)),
            );
        let Some(span) = resolve_span(hir, hir_body, read.span) else {
            continue;
        };
        out.push(
            Diagnostic::error(span, format!("{name} is read before it is assigned"))
                .with_code(code::USE_OF_UNINITIALISED)
                .with_note(
                    "a local declared with `= ---` is deliberately not initialised, so reading \
                     it before an assignment reads whatever was already in that storage",
                )
                .with_help(
                    "assign to it before this read, or drop the `= ---` to zero-initialise it",
                ),
        );
    }

    for span in &mir.facts().stray_jumps {
        let Some(span) = resolve_span(hir, hir_body, *span) else {
            continue;
        };
        out.push(
            Diagnostic::error(span, "`break` or `continue` outside a loop")
                .with_code(code::JUMP_OUTSIDE_LOOP)
                .with_note("there is no enclosing `while` for this statement to act on"),
        );
    }

    out.extend(missing_return(hir, proc, mir).into_vec());
    out
}

// ---------------------------------------------------------------------------
// Missing return
// ---------------------------------------------------------------------------

/// Reports a procedure that must return a value but need not.
///
/// Only *reachable* `FellOffEnd` terminators count, which is the whole reason this
/// waited for a CFG: a procedure ending in `if c { return 1; } else { return 0; }`
/// has a `FellOffEnd` block that no edge reaches, and reporting it would be wrong.
fn missing_return(hir: &FileHir, proc: ProcId, mir: &MirBody) -> Diagnostics {
    let mut out = Diagnostics::new();
    let reachable = mir.reverse_postorder();
    let fell_off = reachable.iter().any(|block| match mir.block(*block).term {
        Terminator::Unreachable(Unreachable::FellOffEnd) => true,
        // `Refused` never reaches here: a refused body has no MIR to check, and the stub is
        // built by the *driver* after this query. Answering `false` rather than matching it
        // with `FellOffEnd` is the honest reading anyway — a stub did not fall off an end, it
        // was never lowered.
        Terminator::Unreachable(
            Unreachable::Trap | Unreachable::StrayJump | Unreachable::Refused,
        )
        | Terminator::Goto(_)
        | Terminator::Branch { .. }
        | Terminator::Return(_) => false,
    });
    if !fell_off {
        return out;
    }
    let Some(span) = hir.procs.get(proc.index()).map(|data| data.span) else {
        return out;
    };
    out.push(
        Diagnostic::error(span, "not every path returns a value")
            .with_code(code::MISSING_RETURN)
            .with_note("this procedure declares a return type, so every path must `return`"),
    );
    out
}
