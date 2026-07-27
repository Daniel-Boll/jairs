//! The typed SSA mid-level IR and its optimisation passes, including the inliner Cranelift does not provide.
//!
//! MIR is where the pipeline stops being about *what the source says* and starts
//! being about *what the machine does*. It is the last representation shared by
//! both back ends, and that sharing is the invariant the whole compiler is built
//! around (`PLAN.md` §3.1): the comptime VM consumes bytecode lowered from the
//! identical MIR that Cranelift consumes. Any other arrangement guarantees that
//! `#run` and runtime silently disagree.
//!
//! **[ADR-0017](../../../docs/adr/0017-mir-shape.md) is this crate's
//! specification.** It settles four decisions, each with its rejected alternative
//! named: blocks are a `Vec` with block *parameters* rather than phi statements;
//! SSA is built during lowering by Braun's algorithm rather than recovered by a
//! later `mem2reg`; one body is one procedure, not one file; and a body that
//! failed to type-check is refused rather than lowered.
//!
//! # What this crate deliberately does not do
//!
//! - **Layout.** Nothing here knows a size, an alignment or a byte offset. A
//!   field access is a [`Projection::Field`] carrying an *index*. ADR-0017 §5
//!   deferred layout so that the VM and Cranelift could not disagree about it, and
//!   ADR-0018 §2 settles it in `jr-pool`, where its inputs already live.
//! - **Diagnostics.** This crate raises none, and defines no diagnostic code;
//!   E0227 is the first free code and belongs to whichever pass claims it.
//!   Lowering *records* the two findings `jr-sema` deferred — see [`Facts`] — and
//!   leaves the reporting to the pass that owns the wording.
//! - **Evaluate anything.** A constant's value and a `#run`'s result are handed in
//!   as [`ConstValues`], computed by `jr-db` from the VM (ADR-0018 §3), because a
//!   second evaluator is exactly what §3.1's invariant forbids.
//! - **Resolve an imported name.** A cross-file callee is handed in as
//!   [`ImportedProcs`] (ADR-0018 §5), so that resolving one never makes this file's
//!   analysis depend on another file's.
//! - **Decide which bodies may be optimised.** [`inline_body`] rewrites whatever
//!   body it is handed. ADR-0021 §2 keeps every body the `#run` closure reaches
//!   byte-identical to its built form, and that is `jr-db`'s decision because the
//!   closure is a query's business — [`const_callees`] is the fact this crate
//!   contributes to it.
//! - **DCE and const-prop.** Still a later wave. A splice leaves [`Statement::Nop`]s
//!   behind and there is nothing yet that removes them. There is no `mem2reg` and
//!   there never will be: Braun's construction means there is nothing for it to do.
//!
//! # What it does not know
//!
//! `jr-hir`'s `lower_bin_op`, `lower_un_op` and `lower_assign_op` fall back
//! silently to `Add`, `Neg` and `Assign` on an unrecognised token, emitting no
//! diagnostic. MIR therefore cannot distinguish a real operator from one that
//! error recovery invented. That cannot be fixed here; it is recorded so the next
//! reader does not mistake it for a MIR bug.

mod build;
mod cfg;
mod code;
mod dump;
mod escape;
mod inline;
mod inputs;
mod mir;
mod span;
mod ssa;
mod thunk;
mod verify;

pub use build::{lower_body, lower_file};
pub use cfg::{body_diagnostics, file_diagnostics};
pub use dump::{dump_body, dump_body_spans, dump_file};
pub use inline::{Callees, MAX_INLINE_STATEMENTS, inline_body, is_inlinable};
pub use inputs::{ConstValues, ImportedProcs};
pub use mir::{
    BinOp, BlockData, BlockId, Callee, Facts, FileMir, MirBody, MirSpan, Operand, Place, PlaceBase,
    Poisoned, ProcRef, Projection, Rvalue, SlotData, SlotId, Statement, Target, Terminator, UnOp,
    UndefinedRead, Unreachable, ValueData, ValueId,
};
pub use span::resolve_span;
pub use thunk::{const_callees, lower_const, thunk_ref};
