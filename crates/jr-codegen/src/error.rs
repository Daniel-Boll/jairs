//! What a back end refuses, and why.
//!
//! # Why these are errors and not diagnostics
//!
//! Every variant here is a **compiler** fault, not a program fault. A program fault
//! was already reported upstream: a body that failed to type-check was refused by
//! ADR-0017 §4 before MIR existed, and a `#foreign` declaration naming something
//! that is not a library is an E0225 from `jr-sema`. By the time a back end runs,
//! the only things left to go wrong are disagreements between MIR, the pool and the
//! target — which is why this crate defines no diagnostic code and produces no
//! [`jr_diag::Diagnostic`].
//!
//! `jr-vm`'s [`VmError`](jr_vm::VmError) draws the same line for the same reason,
//! and the two are deliberately shaped alike: a native build and a comptime run
//! that disagree should disagree in comparable words.

use jr_mir::ProcRef;
use jr_pool::{LayoutError, PoolId};

/// Why a back end could not produce code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodegenError {
    /// A type has no runtime layout.
    ///
    /// Either poison ([`PoolId::ERROR`], which should have been refused upstream)
    /// or a comptime-only type such as a `#system_library` handle. Carries
    /// [`LayoutError`] so the reason survives, rather than being flattened into
    /// "bad type".
    NoLayout {
        /// The type that has no layout.
        ty: PoolId,
        /// Why [`jr_pool::layout_of`] refused it.
        reason: LayoutError,
    },
    /// A body called a procedure that was never declared.
    ///
    /// A driver-sequencing bug: [`Backend::declare`](crate::Backend::declare) must
    /// be called for every procedure before any body is defined. Reported rather
    /// than repaired by declaring late, because a late declaration would have to
    /// invent a signature.
    Undeclared(ProcRef),
    /// A construct MIR permits and this back end does not implement yet.
    ///
    /// Distinct from [`CodegenError::Internal`]: this is an honest "not yet",
    /// which is what a wave-by-wave slice produces, and it names the construct so
    /// the gap is legible instead of looking like a crash.
    Unsupported {
        /// The procedure whose body contained it.
        proc: ProcRef,
        /// What was not supported, in the words of the IR.
        what: String,
    },
    /// MIR, the pool and the target disagree, or the back end broke its own
    /// invariant.
    ///
    /// Always a bug in the compiler. The message is for whoever has to fix it.
    Internal(String),
}

impl std::fmt::Display for CodegenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoLayout { ty, reason } => {
                write!(f, "type {} has no runtime layout: {reason}", ty.index())
            }
            Self::Undeclared(proc) => write!(
                f,
                "procedure {} in file {} was defined without being declared",
                proc.proc.index(),
                proc.file.index()
            ),
            Self::Unsupported { proc, what } => write!(
                f,
                "procedure {} in file {}: {what} is not supported by this back end yet",
                proc.proc.index(),
                proc.file.index()
            ),
            Self::Internal(message) => write!(f, "internal codegen error: {message}"),
        }
    }
}

impl std::error::Error for CodegenError {}
