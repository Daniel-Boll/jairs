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
    /// Fits one *vector* register: `#simd [N]T` (ADR-0148 §1).
    ///
    /// Its own case rather than an [`Repr::Aggregate`] with a vector type bolted on, because the
    /// two answer the load/store question differently: an aggregate is *carried as a pointer to its
    /// bytes* and copied with `memcpy`, while a vector is loaded into a register, operated on with
    /// one instruction, and stored back. Sharing the case would mean every arithmetic site asking
    /// "but is this aggregate secretly a vector", which is the question this enum exists to answer
    /// once.
    Vector {
        /// The Cranelift vector type, exactly 128 bits wide.
        ///
        /// **The width is not re-derived here.** `jr-sema` has already refused every other width
        /// (E0285), because a vector operation compiles at exactly 128 bits and nowhere else — which
        /// is what probing found before ADR-0148 was written.
        ty: Type,
        /// `true` for a signed integer element, which decides `sshr` versus `ushr` and which
        /// widening applies. Meaningless for a float element, recorded `true` for the reason
        /// [`Repr::Scalar`]'s float case gives.
        signed: bool,
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
            // A vector: one register, and the lane type comes from the element (ADR-0148 §1).
            // `Type::by` is the constructor Cranelift offers, and it happily makes an `I64X4` that
            // no backend can compile — so its `None` is not the only failure mode, and the width
            // constraint that matters lives in sema rather than here.
            Item::VectorType { elem, lanes } => {
                let Repr::Scalar { ty, signed } = Self::of(pool, target, *elem)? else {
                    return Err(CodegenError::Internal(format!(
                        "a vector of a non-scalar element {}",
                        elem.index()
                    )));
                };
                let lanes = u32::try_from(*lanes).map_err(|_| {
                    CodegenError::Internal(format!("a vector of {lanes} lanes"))
                })?;
                let vector = ty.by(lanes).ok_or_else(|| {
                    CodegenError::Internal(format!("no Cranelift vector type for {lanes}x{ty}"))
                })?;
                Ok(Self::Vector {
                    ty: vector,
                    signed,
                })
            }
            // A compiler-emitted table travels as the view it materialises to (ADR-0152 §1).
            Item::StaticArray { .. }
            | Item::StringType
            | Item::StructType { .. }
            // A union is an aggregate: it lives in memory and is passed by copy, exactly as a
            // struct is. Its size is its largest field's, which `layout_of` already knows.
        | Item::UnionType { .. }
        | Item::VariantType { .. }
            | Item::ArrayType { .. }
            // **A results aggregate, which is what makes ADR-0052 free in this crate.** Answering
            // `Aggregate` here is the entire back-end change for multiple returns: ADR-0051's
            // `returns_via_sret` keys off exactly this, so the caller allocates a slot, the callee
            // writes through it, and neither had to learn what a results list is.
            | Item::ResultsType { .. }
            // A context is an aggregate — but note that what a call actually passes is a
            // *pointer* to one (ADR-0057 §2), which is a scalar. This arm is for the pointee.
            | Item::ContextType
            | Item::ViewType { .. }
            // A dynamic array is a view + a capacity word — three fields living in memory,
            // passed by copy exactly as a view is (ADR-0136 §1).
            | Item::DynamicArrayType { .. } => {
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
            | Item::ForeignLibraryValue(_)
            // A *value* has no representation of its own — its **type** does, and that arm already
            // works. Asking this of an aggregate constant is the same category error as asking it of
            // an `IntValue` (ADR-0074 §1).
            | Item::AggregateValue { .. } => Err(CodegenError::NoLayout {
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
            // A vector travels *as itself*, in a vector register — which is the entire difference
            // from the aggregate case and the reason ADR-0148 §1 made it its own `Item`. On both
            // targets a 128-bit vector is a legal parameter and return type, so nothing has to
            // spill it to make a call.
            Self::Vector { ty, .. } => Some(ty),
        }
    }

    /// Whether this is an aggregate, which decides copy-versus-move at every
    /// assignment.
    #[must_use]
    pub const fn is_aggregate(self) -> bool {
        // A vector is deliberately **not** one, even though its Jairs type has sixteen bytes and a
        // layout: this predicate decides copy-versus-move and `returns_via_sret`, and a vector needs
        // neither — it is one register in, one register out (ADR-0148 §1). Answering `true` here
        // would make every vector-returning procedure allocate a hidden slot for a value that fits
        // in `v0`.
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
    // **A classified aggregate return comes back in registers** (ADR-0160 part 2): the shared classification
    // says how many and of which file, and the body reassembles them into a slot. Computed here and *emitted
    // at the end*, because the returns follow every parameter — an early return from this function would
    // produce a signature with the results and no arguments, which the verifier catches as
    // "mismatched argument count" at the first call site.
    let c_returns = if foreign && ret_repr.is_aggregate() {
        match foreign_class(pool, target, ret, &describe)? {
            jr_pool::Class::Integer { words } => {
                Some(vec![AbiParam::new(pointer_type(target)); words as usize])
            }
            jr_pool::Class::Float { kind, count } => {
                Some(vec![AbiParam::new(float_type(kind)); count as usize])
            }
            jr_pool::Class::Memory => {
                return Err(describe(
                    "returning this aggregate from a `#foreign` procedure needs a register class \
                     this back end does not implement — at most two words, or up to four floats of \
                     one width",
                ));
            }
        }
    } else {
        None
    };
    if ret_repr.is_aggregate() && c_returns.is_none() {
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
            // The same classification the return uses, so a struct that can be *returned* can be *passed*
            // — one predicate, one answer, and no shape that works in one direction and not the other.
            match foreign_class(pool, target, *ty, &describe)? {
                jr_pool::Class::Integer { words } => {
                    for _ in 0..words {
                        sig.params.push(AbiParam::new(pointer_type(target)));
                    }
                }
                jr_pool::Class::Float { kind, count } => {
                    for _ in 0..count {
                        sig.params.push(AbiParam::new(float_type(kind)));
                    }
                }
                jr_pool::Class::Memory => {
                    return Err(describe(
                        "an aggregate parameter on a `#foreign` procedure needs a register class this \
                         back end does not implement — at most two words, or up to four floats of one \
                         width",
                    ));
                }
            }
            continue;
        }
        if let Some(clif) = repr.clif_type(target) {
            sig.params.push(AbiParam::new(clif));
        }
    }

    // A classified C aggregate return, emitted here so it follows every parameter.
    if let Some(returns) = c_returns {
        sig.returns.extend(returns);
        return Ok(sig);
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

/// The C ABI class of an aggregate crossing a `#foreign` boundary.
///
/// A thin wrapper turning [`jr_pool::classify`]'s `Option` and layout error into this crate's error type, so
/// the two call sites above read as one line each. It answers [`jr_pool::Class::Memory`] for a scalar too,
/// which cannot happen — the callers check `is_aggregate` first — and answering rather than panicking keeps a
/// future caller's mistake a diagnostic.
fn foreign_class(
    pool: &Pool,
    target: TargetLayout,
    ty: PoolId,
    describe: &dyn Fn(&str) -> CodegenError,
) -> Result<jr_pool::Class, CodegenError> {
    match jr_pool::classify(pool, target, ty) {
        Ok(Some(class)) => Ok(class),
        Ok(None) => Ok(jr_pool::Class::Memory),
        Err(_) => Err(describe(
            "an aggregate at a `#foreign` boundary whose layout cannot be computed",
        )),
    }
}

/// The Cranelift type a float class's members travel in.
///
/// Separate from [`Repr::clif_type`] because a class carries a [`jr_pool::FloatKind`] rather than a `PoolId`:
/// the classification flattened the aggregate, so there is no member type left to look up.
const fn float_type(kind: jr_pool::FloatKind) -> Type {
    if kind.bits == 32 {
        cranelift_codegen::ir::types::F32
    } else {
        cranelift_codegen::ir::types::F64
    }
}
