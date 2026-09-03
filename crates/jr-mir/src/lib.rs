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
//! - **`mem2reg`.** There is none and there never will be: Braun's construction
//!   (ADR-0017 §2) means there is nothing for it to do.
//! - **Compact the SSA value arena.** [`dce`] removes a dead definition but keeps its
//!   `ValueData`, so `value_count()` never shrinks and the VM still sizes a frame for
//!   it. ADR-0022 leaves this undone deliberately: unlike a slot, a value is named by
//!   block parameters too, so compaction is a wider rewrite than it looks.
//!
//! # What it does not know
//!
//! `jr-hir`'s `lower_bin_op`, `lower_un_op` and `lower_assign_op` fall back
//! silently to `Add`, `Neg` and `Assign` on an unrecognised token, emitting no
//! diagnostic. MIR therefore cannot distinguish a real operator from one that
//! error recovery invented. That cannot be fixed here; it is recorded so the next
//! reader does not mistake it for a MIR bug.

mod bounds;
mod build;
mod cfg;
mod code;
mod constprop;
mod dce;
mod dump;
mod escape;
mod forward;
mod inline;
mod inputs;
mod mir;
mod optimize;
mod span;
mod ssa;
mod thunk;
mod verify;

pub use bounds::strip_bounds_checks;
pub use build::{lower_body, lower_file};
pub use cfg::{body_diagnostics, file_diagnostics};
pub use constprop::const_prop;
pub use dce::{dce, is_pure};
pub use dump::{dump_body, dump_body_spans, dump_file};
pub use forward::forward_stores;
pub use inline::{
    Callees, MAX_INLINE_ROUNDS, MAX_INLINE_STATEMENTS, MAX_INLINED_STATEMENTS, inline_body,
    is_inlinable,
};
pub use inputs::{
    AnyLowering, ConstValues, FilledArg, FilledArgs, ImportedProc, ImportedProcs, ImportedValues,
    OperatorCalls,
};
pub use mir::{
    AtomicOp, BinOp, BlockData, BlockId, Callee, Facts, FileMir, GlobalData, GlobalRef, MirBody,
    MirSpan, NumKind, Operand, Place, PlaceBase, Poisoned, ProcRef, Projection, Rvalue, SlotData,
    SlotId, Statement, Target, Terminator, UnOp, UndefinedRead, Unreachable, ValueData, ValueId,
};
pub use optimize::{MAX_OPT_ROUNDS, OptStats, optimize};
pub use span::resolve_span;
pub use thunk::{const_callees, lower_const, thunk_ref};
