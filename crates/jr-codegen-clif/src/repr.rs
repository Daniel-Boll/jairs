//! How a Jairs type becomes machine data, and where every byte count comes from.
//!
//! # The rule this module exists to enforce
//!
//! **Nothing here computes a size, an alignment or an offset.** Every one is asked
//! of [`jr_pool`], which is the single layout ADR-0018 §2 put in the pool so that
//! the comptime VM and the native back end cannot disagree. ADR-0019 restates the
//! prohibition and explains why it is a prohibition and not a preference: a struct
//! whose field sits at offset 8 during a `#run` and at offset 12 at runtime is two
//! different programs from one source, with no diagnostic, no verifier complaint,
//! and a failure that surfaces arbitrarily far from its cause. No test catches that
//! in general, so the only defence is that the arithmetic exists once.
//!
//! What this module *does* decide is the machine **representation** of a value —
//! which register class it lives in, and whether it lives in a register at all.
//! That is a back end's own business, and it is derived from `jr-pool`'s numbers
//! rather than invented alongside them.
//!
//! # Why an aggregate is a pointer
//!
//! The VM holds an aggregate as `Value::Aggregate(Vec<u8>)` — by value, with no
//! calling convention at all, because an interpreter needs none. Machine code does.
//! So a [`Repr::Aggregate`] is represented as a **pointer to its bytes**, and the
//! internal Jairs calling convention passes aggregates by that pointer.
//!
//! This is legal precisely because it is *internal*. Every procedure Jairs declares
//! is `ContextKind::Jairs` (ADR-0001), so the compiler owns both sides of the call
//! and may choose. The one boundary it does not own is `#foreign`, which is C, and
//! there an aggregate would need the platform's real struct-passing rules — so an
//! aggregate crossing a `#foreign` boundary is refused rather than guessed at. The
//! slice never needs it: `write` and `exit` take scalars only.
//!
//! # Why the integer width is the type's own
//!
//! `jr-vm` normalises every scalar to its type's width, "bits above `bits` are
//! zero, for signed and unsigned alike", and range-checks arithmetic against that
//! type's `min`/`max` — which is why `execute.rs` asserts that a narrow type traps
//! at *its own* boundary and not at `s64`'s. Using the exact Cranelift type for
//! each width (`I8` for `u8`, `I64` for `s64`) reproduces that for free: Cranelift's
//! overflow instructions are defined on the operand width, so an `I8` add overflows
//! where an 8-bit add overflows. Widening everything to `I64` would have silently
//! moved every narrow boundary.

use cranelift_codegen::ir::{AbiParam, ArgumentPurpose, Signature, Type, types};
use cranelift_codegen::isa::CallConv;
use jr_codegen::CodegenError;
use jr_pool::{Item, Pool, PoolId, TargetLayout, layout_of};

/// How a value of some Jairs type is carried by machine code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repr {
    /// Zero bytes: the single value of type `void`.
    ///
    /// Carried by *no* Cranelift value at all, which is why this is a case rather
    /// than a zero-width scalar. `jr-vm`'s [`Shape`](jr_vm) draws the same
    /// distinction for the same reason: `void` is a real, storable type (ADR-0015
    /// §3), and collapsing it into `Scalar(0)` would make a `void`-returning call's
    /// result compare equal to `false`.
    Void,
    /// Fits one register: `bool`, an integer, a pointer, a procedure.
    Scalar {
        /// The Cranelift type, whose width is the Jairs type's own width.
        ty: Type,
        /// `true` for a signed integer, which decides `sdiv` versus `udiv` and
        /// which overflow instruction applies.
        signed: bool,
    },
    /// Lives in memory: `string`, a struct. Carried as a pointer to its bytes.
    Aggregate {
        /// Size in bytes, from [`jr_pool::layout_of`] — never computed here.
        size: u64,
        /// Alignment in bytes, from [`jr_pool::layout_of`].
        align: u32,
    },
}

impl Repr {
    /// The representation of `ty`.
    ///
    /// Classified from the pool item, then sized by [`jr_pool::layout_of`]. The
    /// match is exhaustive so that a new [`Item`] is a compile error here rather
    /// than being silently classified as an aggregate — which would move the wrong
    /// number of bytes instead of failing.
    ///
    /// # Errors
    /// [`CodegenError::NoLayout`] for a type with no runtime layout: poison, which
    /// ADR-0017 §4 should have refused upstream, or a comptime-only type such as a
    /// `#system_library` handle.
    pub fn of(pool: &Pool, target: TargetLayout, ty: PoolId) -> Result<Self, CodegenError> {
        match pool.item(ty) {
            Item::VoidType => Ok(Self::Void),
            Item::BoolType => Ok(Self::Scalar {
                // `bool` is one byte, and `jr-vm` stores it as 0 or 1. `I8` keeps
                // the storage width and the register width the same, so a `bool`
                // loaded out of a struct needs no conversion.
                ty: types::I8,
                signed: false,
            }),
            Item::IntType { signed, bits } => Ok(Self::Scalar {
                ty: int_type(*bits)?,
                signed: *signed,
            }),
            // An enum *is* its backing type in a register (ADR-0041 §3), and the backing
            // type is `s64` for every enum this wave has — so `signed: true` is not a guess,
            // it is `s64`'s signedness. An explicit backing type would read it from the
            // declaration instead.
            Item::EnumType { .. } => Ok(Self::Scalar {
                ty: types::I64,
                signed: true,
            }),
            // A float is a scalar in a *float* register, and `signed` is meaningless for one
            // — IEEE-754 has one signed representation. It is recorded as `true` because the
            // only consumer that reads it for a float is `unary`'s negation, and a float
            // negation is the signed kind: it flips the sign bit rather than subtracting from
            // zero.
            Item::FloatType { bits } => Ok(Self::Scalar {
                ty: match bits {
                    32 => types::F32,
                    64 => types::F64,
                    other => {
                        return Err(CodegenError::Internal(format!(
                            "no Cranelift type for a {other}-bit float"
                        )));
                    }
                },
                signed: true,
            }),
            Item::PointerType(_) | Item::ProcType { .. } => Ok(Self::Scalar {
                ty: pointer_type(target),
                signed: false,
            }),
            // A view joins the aggregates: two words, so it lives in memory and is passed
            // by copy exactly as a `string` is (ADR-0044 §1).
            Item::StringType
            | Item::StructType { .. }
            // A union is an aggregate: it lives in memory and is passed by copy, exactly as a
            // struct is. Its size is its largest field's, which `layout_of` already knows.
            | Item::UnionType { .. }
            | Item::ArrayType { .. }
            // **A results aggregate, which is what makes ADR-0052 free in this crate.** Answering
            // `Aggregate` here is the entire back-end change for multiple returns: ADR-0051's
            // `returns_via_sret` keys off exactly this, so the caller allocates a slot, the callee
            // writes through it, and neither had to learn what a results list is.
            | Item::ResultsType { .. }
            // A context is an aggregate — but note that what a call actually passes is a
            // *pointer* to one (ADR-0057 §2), which is a scalar. This arm is for the pointee.
            | Item::ContextType
            | Item::ViewType { .. } => {
                let layout = layout_of(pool, target, ty)
                    .map_err(|reason| CodegenError::NoLayout { ty, reason })?;
                Ok(Self::Aggregate {
                    size: layout.size,
                    align: layout.align,
                })
            }
            // Every remaining item is either a comptime-only type or a *value*
            // rather than a type. `layout_of` already has the right words for both,
            // so it is asked rather than second-guessed.
            Item::TypeType
            | Item::ErrorType
            | Item::ForeignLibraryType
            | Item::VoidValue
            | Item::BoolValue(_)
            | Item::IntValue { .. }
            | Item::FloatValue { .. }
            | Item::StrValue(_)
            | Item::TypeValue(_)
            | Item::ProcValue { .. }
            | Item::ForeignLibraryValue(_) => Err(CodegenError::NoLayout {
                ty,
                reason: layout_of(pool, target, ty)
                    .err()
                    .unwrap_or(jr_pool::LayoutError::Poison),
            }),
        }
    }

    /// The Cranelift type a value of this representation occupies, if any.
    ///
    /// `None` only for [`Repr::Void`], which occupies no register.
    #[must_use]
    pub fn clif_type(self, target: TargetLayout) -> Option<Type> {
        match self {
            Self::Void => None,
            Self::Scalar { ty, .. } => Some(ty),
            // An aggregate travels as a pointer to its bytes; see the module docs.
            Self::Aggregate { .. } => Some(pointer_type(target)),
        }
    }

    /// Whether this is an aggregate, which decides copy-versus-move at every
    /// assignment.
    #[must_use]
    pub const fn is_aggregate(self) -> bool {
        matches!(self, Self::Aggregate { .. })
    }
}

/// The Cranelift type for an integer of `bits` bits.
///
/// # Errors
/// [`CodegenError::Internal`] for a width no machine register has. `jr-sema` only
/// ever produces 8, 16, 32 and 64, so reaching this means the pool and this
/// function disagree about what an integer type is.
pub fn int_type(bits: u16) -> Result<Type, CodegenError> {
    match bits {
        8 => Ok(types::I8),
        16 => Ok(types::I16),
        32 => Ok(types::I32),
        64 => Ok(types::I64),
        other => Err(CodegenError::Internal(format!(
            "no machine type for a {other}-bit integer"
        ))),
    }
}

/// The Cranelift type of a pointer on this target.
///
/// Derived from [`TargetLayout::pointer_size`] rather than from the host, because
/// comptime and runtime layouts are distinct even where they are numerically equal
/// today (ADR-0018 §2). A width the machine has no register for falls back to `I64`,
/// which is the only width the slice's targets have.
#[must_use]
pub fn pointer_type(target: TargetLayout) -> Type {
    match target.pointer_size {
        4 => types::I32,
        _ => types::I64,
    }
}

/// Builds the Cranelift signature for a procedure.
///
/// `void` contributes no parameter and no result, which is what makes
/// [`Repr::Void`] a case rather than a zero-width scalar.
///
/// An aggregate return is realised as a **caller-allocated `sret` pointer** in the
/// leading parameter position (ADR-0051 §1): the caller allocates a slot, passes its
/// address, and the callee writes through it. [`returns_via_sret`] is the one predicate
/// that decides this, so the signature and the body cannot disagree about whether a
/// given procedure has the hidden parameter.
///
/// # Errors
/// [`CodegenError::NoLayout`] when a parameter or return type has none.
/// [`CodegenError::Unsupported`] when an aggregate would cross a `#foreign` boundary —
/// as a parameter *or* as a return — because that needs each platform's own struct
/// classification rules and guessing them puts garbage in a register with no
/// diagnostic (ADR-0051 §4). A Jairs-to-Jairs aggregate return is **not** refused: both
/// sides are compiled here and only need to agree with each other.
pub fn signature(
    pool: &Pool,
    target: TargetLayout,
    params: &[PoolId],
    ret: PoolId,
    call_conv: CallConv,
    foreign: bool,
    receives_context: bool,
    describe: &dyn Fn(&str) -> CodegenError,
) -> Result<Signature, CodegenError> {
    let mut sig = Signature::new(call_conv);

    let ret_repr = Repr::of(pool, target, ret)?;
    if ret_repr.is_aggregate() {
        if foreign {
            return Err(describe(
                "returning an aggregate from a `#foreign` procedure, whose C struct \
                 convention this back end does not implement",
            ));
        }
        // **First**, matching every C ABI that uses this convention, and marked
        // `StructReturn` rather than passed as a plain pointer so Cranelift's verifier
        // and any later ABI work can see what it is.
        sig.params.push(AbiParam::special(
            pointer_type(target),
            ArgumentPurpose::StructReturn,
        ));
    }

    // **The context is the second hidden parameter** (ADR-0057 §4), after `sret` and before the
    // declared ones. A plain pointer rather than a special-purpose one: Cranelift has no
    // `ArgumentPurpose` for it, and it is an ordinary `*Context` the callee reads through.
    if receives_context {
        sig.params.push(AbiParam::new(pointer_type(target)));
    }

    for ty in params {
        let repr = Repr::of(pool, target, *ty)?;
        if foreign && repr.is_aggregate() {
            return Err(describe(
                "an aggregate parameter on a `#foreign` procedure, whose C struct \
                 convention this back end does not implement",
            ));
        }
        if let Some(clif) = repr.clif_type(target) {
            sig.params.push(AbiParam::new(clif));
        }
    }

    // An `sret` procedure returns *nothing*: the result travels through the pointer, so
    // adding a return value as well would describe a convention neither side implements.
    if !ret_repr.is_aggregate()
        && let Some(clif) = ret_repr.clif_type(target)
    {
        sig.returns.push(AbiParam::new(clif));
    }
    Ok(sig)
}

/// Whether a procedure returning `ret` uses the `sret` convention (ADR-0051 §1).
///
/// The single predicate both the signature and the body consult. Two separate tests for
/// "does this have a hidden first parameter" would be two chances to disagree, and a
/// disagreement here shifts *every* argument by one position — a silent miscompile
/// rather than a crash, which is this project's named first failure mode.
///
/// # Errors
/// [`CodegenError::NoLayout`] when `ret` has no layout.
pub fn returns_via_sret(
    pool: &Pool,
    target: TargetLayout,
    ret: PoolId,
) -> Result<bool, CodegenError> {
    Ok(Repr::of(pool, target, ret)?.is_aggregate())
}
