//! What can go wrong while executing bytecode, split by whose fault it is.
//!
//! # Why four variants and not one
//!
//! The VM has three genuinely different failure audiences, and collapsing them
//! costs the ability to respond correctly:
//!
//! - **The program trapped.** ADR-0002 makes `+`, `-`, `*`, `/`, `%` and unary `-`
//!   trap rather than wrap, so a trap is the language working as specified. At
//!   comptime this becomes a diagnostic at the `#run` site; at runtime it becomes a
//!   non-zero exit. Either way it is the *user's* program that is wrong.
//! - **The VM will not do it.** A foreign call during comptime is refused per
//!   ADR-0006 until wave W6 adds `#foreign_at_comptime`. Nothing is wrong; the
//!   feature has not landed. This must not read as a compiler bug.
//! - **The inputs disagree.** A `PlaceBase::Slot` naming no slot, an operand whose
//!   type the pool does not know, a call with the wrong argument count: every one
//!   of these means MIR, the pool and the bytecode do not agree, which is a
//!   *compiler* bug. `jr-mir`'s verifier is supposed to make them unreachable, and
//!   this variant is what makes a hole in it visible instead of silently producing
//!   a wrong answer.
//! - **A resource ran out.** Not a property of the program's meaning at all.
//!
//! Returning [`VmError::Internal`] rather than panicking is deliberate for the same
//! reason [`jr_pool::LayoutError`] is a `Result`: a compiler that aborts inside an
//! interpreter is much harder to diagnose than one that reports where it lost
//! confidence.

use core::fmt;

use jr_mir::{BlockId, MirSpan, ProcRef};

/// Where a trap happened.
///
/// Reported separately from [`VmError::Trap`] rather than inside it, so that the
/// twenty-odd sites which construct a trap — and the tests that pattern-match one —
/// are unchanged. A trap's *identity* is what went wrong; where it happened is
/// context the interpreter adds, and only the interpreter knows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrapSite {
    /// The procedure that was executing.
    pub proc: ProcRef,
    /// The MIR provenance of the instruction it trapped on.
    ///
    /// Resolve it with `jr_mir::resolve_span`, which needs the file's HIR — which is
    /// why the VM reports the identity and leaves the rendering to a caller that has
    /// a `SourceMap`. [`MirSpan::Synthetic`] is a real answer: a compiler-invented
    /// value has no source text, and ADR-0020 §4 argues that reporting no location is
    /// better than reporting a neighbouring one.
    pub span: MirSpan,
}

/// Why execution stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmError {
    /// The program trapped, as the language specifies.
    Trap(Trap),
    /// The VM declines to execute this, and that is not an error in the program.
    Unsupported(String),
    /// MIR, the pool and the bytecode disagree — a compiler bug.
    Internal(String),
    /// A VM resource ran out.
    Exhausted(&'static str),
    /// The program called `exit`, and this is its status.
    ///
    /// Not a failure: it is the program's chosen ending, and `modules/Basic` declares
    /// `exit` precisely so a program has "a way out that does not depend on `main`
    /// returning, which the slice does not model yet". It is an `Err` rather than a
    /// return value because it unwinds every frame at once, which is what `exit`
    /// means — and it must never be the *host* `exit`, which would terminate the
    /// compiler mid-build.
    Exited(i64),
}

impl VmError {
    /// An [`VmError::Internal`] from anything displayable.
    pub(crate) fn internal(what: impl fmt::Display) -> Self {
        Self::Internal(what.to_string())
    }

    /// An [`VmError::Unsupported`] from anything displayable.
    pub(crate) fn unsupported(what: impl fmt::Display) -> Self {
        Self::Unsupported(what.to_string())
    }

    /// An [`VmError::Unsupported`] a caller outside this crate can build.
    ///
    /// Exists because const evaluation in `jr-db` reduces a VM result to something
    /// that outlives the VM, and some results have no such form — a compile-time
    /// struct, for one. Those refusals are the same kind as the VM's own, so they
    /// should read the same way rather than becoming a bare string.
    #[must_use]
    pub fn unsupported_public(what: impl fmt::Display) -> Self {
        Self::Unsupported(what.to_string())
    }
}

/// A trap the language defines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trap {
    /// Arithmetic overflowed. ADR-0002: `+`, `-`, `*` and unary `-` trap rather
    /// than wrap, and `+%`, `-%`, `*%` are the opt-out.
    Overflow {
        /// A short description of the operation, for the message.
        what: &'static str,
    },
    /// A divisor was zero.
    DivideByZero,
    /// A shift count was negative or `>=` the type's width (ADR-0042 §3).
    ShiftOutOfRange,
    /// `Terminator::Unreachable(Unreachable::Trap)` was reached.
    Deliberate,
    /// A `break` or `continue` outside a loop was reached at run time.
    ///
    /// E0229 reports this statically, so reaching it means the program was run
    /// without being checked.
    StrayJump,
    /// Control fell off the end of a procedure that must return a value.
    ///
    /// E0228 reports this statically, for the same reason.
    FellOffEnd,
    /// A value that was never assigned was read.
    ///
    /// E0227 reports this statically. `Rvalue::Undef` is *not* poison — it is a
    /// well-typed value that has no bits — so the VM traps on use rather than
    /// inventing a zero, which would hide the bug the diagnostic exists to report.
    UninitialisedRead,
    /// An array index was outside the array (ADR-0003, ADR-0039 §2).
    ///
    /// Distinct from [`Trap::BadAddress`], which is a property of this VM's linear
    /// memory model and has no native counterpart. This one is a *language* trap that
    /// both engines raise, from the explicit `Statement::BoundsCheck` the index lowered
    /// to, so `differential.rs` compares the two.
    ///
    /// Carries no index or length: the message is static so that it matches
    /// `jr-codegen-clif`'s `TrapKind::reason`, which is a `&'static str` because native
    /// code raises a trap through a helper taking a pointer to a constant string
    /// (ADR-0039 §2).
    IndexOutOfBounds,
    /// A `variant`'s case was read while its tag named a different one (ADR-0068 §4).
    ///
    /// The tag is the only thing that knows which case is live, so this is the trap that turns an
    /// undetectable bit reinterpretation — what a `union` does, by design (ADR-0045 §1) — into a
    /// located failure. Not statically decidable, which is why it is a trap rather than a diagnostic:
    /// the last write may be in another procedure, behind a pointer, or in a loop.
    WrongVariantCase,
    /// A call **through a null procedure pointer** (ADR-0110 §1).
    ///
    /// A proc pointer is a scalar handle, and a null one is zero — which decodes to file 0, procedure 0: an
    /// arbitrary real procedure. So calling one used to *call something else*, and the symptom was whatever that
    /// procedure's arity happened to be: `called a procedure taking 1 arguments with 2`, an internal compiler
    /// error for the ordinary configuration mistake of using `context.allocator` before installing one.
    ///
    /// A **language** trap that both engines raise, so `differential.rs` compares the two — unlike
    /// [`Trap::BadAddress`], which is a property of this VM's memory model and has no native counterpart. It is
    /// the honest failure for a null callee: there is no procedure to call, and no answer to invent.
    NullCall,
    /// A memory access was out of bounds or misaligned.
    ///
    /// Reachable from a valid program: `ptr := *sum;` followed by arithmetic on the
    /// pointer is not expressible in Jairs-0, but a dangling pointer into a
    /// released frame is.
    BadAddress {
        /// The offending address.
        address: u64,
        /// How many bytes were wanted.
        size: u64,
    },
}

impl fmt::Display for Trap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Overflow { what } => write!(f, "{what} overflowed"),
            Self::DivideByZero => write!(f, "division by zero"),
            Self::ShiftOutOfRange => write!(f, "shift count out of range"),
            Self::Deliberate => write!(f, "reached a deliberate trap"),
            Self::StrayJump => write!(f, "a `break` or `continue` outside a loop was reached"),
            Self::FellOffEnd => write!(
                f,
                "control reached the end of a procedure that must return a value"
            ),
            Self::UninitialisedRead => write!(f, "read a value that was never assigned"),
            Self::IndexOutOfBounds => write!(f, "index out of bounds"),
            Self::NullCall => write!(f, "call through a null procedure pointer"),
            Self::WrongVariantCase => write!(f, "read the wrong variant case"),
            Self::BadAddress { address, size } => {
                write!(f, "invalid access of {size} bytes at address {address:#x}")
            }
        }
    }
}

impl fmt::Display for VmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Trap(trap) => write!(f, "{trap}"),
            Self::Unsupported(what) => write!(f, "{what}"),
            Self::Internal(what) => write!(f, "internal compiler error: {what}"),
            Self::Exhausted(what) => write!(f, "the compile-time interpreter ran out of {what}"),
            Self::Exited(status) => write!(f, "the program called `exit({status})`"),
        }
    }
}

impl core::error::Error for VmError {}

// ---------------------------------------------------------------------------
// Internal-error constructors
// ---------------------------------------------------------------------------

/// The messages [`VmError::Internal`] is built from, kept together so that they
/// read consistently and so that a grep for one finds its only producer.
pub(crate) mod ice {
    use super::*;

    pub(crate) fn no_such_routine(target: ProcRef) -> VmError {
        VmError::Internal(format!(
            "no routine for file {} proc {}",
            target.file.index(),
            target.proc.index()
        ))
    }

    pub(crate) fn no_such_block(block: BlockId) -> VmError {
        VmError::Internal(format!("block {} has no bytecode", block.index()))
    }
}
