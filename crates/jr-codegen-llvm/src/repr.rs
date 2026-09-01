//! How a Jairs type becomes an LLVM value, and where every byte count comes from.
//!
//! # The rule this module exists to enforce
//!
//! **Nothing here computes a size, an alignment or an offset.** Every one is asked of
//! [`jr_pool`], which is the single layout ADR-0018 §2 put in the pool so that the comptime
//! VM and the native back ends cannot disagree. `jr-codegen-clif`'s `repr` module states the
//! same prohibition, and ADR-0143 §4 states the extra form it takes in a *typed* IR:
//!
//! **No Jairs aggregate acquires an LLVM `StructType`.** Building one would put LLVM's own
//! padding and alignment rules in charge of where a field sits, which is a second
//! computation of the thing that must exist once. Instead an aggregate is bytes, its address
//! is an opaque `ptr`, and a field offset is a byte `getelementptr` over `i8` with an offset
//! this compiler chose.
//!
//! # Why a pointer is an integer
//!
//! A Jairs pointer is a [`Repr::Scalar`] whose LLVM type is an *integer* of the target's
//! pointer width, not `ptr`. That is exactly what the Cranelift back end does — there a
//! pointer is `I64` — and it is what makes ADR-0064's pointer arithmetic one code path
//! rather than two: `p + n` is an integer add in both back ends and in the VM.
//!
//! `ptr` appears only where LLVM insists on it: the operand of a load, a store or a GEP.
//! [`super::body`] converts at those boundaries and nowhere else.

use inkwell::context::Context;
use inkwell::types::{BasicMetadataTypeEnum, BasicTypeEnum, FloatType, FunctionType, IntType};
use jr_codegen::CodegenError;
use jr_pool::{Item, Pool, PoolId, TargetLayout, layout_of};

/// How a value of some Jairs type is carried by generated code.
///
/// The same three cases `jr-codegen-clif`'s `Repr` has, for the same reasons, and
/// deliberately not shared with it: the payload of [`Repr::Scalar`] is an LLVM type, and a
/// shared enum would have to be generic over the back end's type vocabulary to say anything
/// useful. What *is* shared is the classification rule, which is `jr-pool`'s `Item` — so the
/// two matches are the same shape over the same input and a new `Item` is a compile error in
/// both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repr<'ctx> {
    /// Zero bytes: the single value of type `void`.
    ///
    /// Carried by *no* LLVM value at all, which is why this is a case rather than a
    /// zero-width scalar (ADR-0015 §3).
    Void,
    /// Fits one register: `bool`, an integer, a pointer, a procedure, a float.
    Scalar(ScalarRepr<'ctx>),
    /// Lives in memory: `string`, a struct, an array, a view. Carried as a pointer to its
    /// bytes, held as an integer for the reason the module docs give.
    Aggregate {
        /// Size in bytes, from [`jr_pool::layout_of`] — never computed here.
        size: u64,
        /// Alignment in bytes, from [`jr_pool::layout_of`].
        align: u32,
    },
}

/// A scalar's LLVM type and whether it is signed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarRepr<'ctx> {
    /// An integer of the Jairs type's own width — 8, 16, 32 or 64 bits.
    ///
    /// The width is the type's own rather than a uniform 64 bits, because the overflow
    /// intrinsics are defined on the operand width: an `i8` add overflows where an 8-bit add
    /// overflows, so a narrow type traps at its own boundary and not at `s64`'s. Widening
    /// everything would silently move every narrow boundary.
    Int {
        /// The LLVM integer type.
        ty: IntType<'ctx>,
        /// `true` for a signed integer, which decides `sdiv` versus `udiv` and which
        /// overflow intrinsic applies.
        signed: bool,
    },
    /// A `float32` or `float64`.
    Float(FloatType<'ctx>),
}

impl<'ctx> Repr<'ctx> {
    /// The representation of `ty`.
    ///
    /// Classified from the pool item, then sized by [`jr_pool::layout_of`]. The match is
    /// exhaustive so that a new [`Item`] is a compile error here rather than being silently
    /// classified as an aggregate — which would move the wrong number of bytes instead of
    /// failing.
    ///
    /// # Errors
    /// [`CodegenError::NoLayout`] for a type with no runtime layout: poison, or a
    /// comptime-only type such as a `#system_library` handle.
    pub fn of(
        context: &'ctx Context,
        pool: &Pool,
        target: TargetLayout,
        ty: PoolId,
    ) -> Result<Self, CodegenError> {
        match pool.item(ty) {
            Item::VoidType => Ok(Self::Void),
            // `bool` is one byte and the VM stores it as 0 or 1, so `i8` keeps the storage
            // width and the register width the same — a `bool` loaded out of a struct needs
            // no conversion. **Not `i1`**, which is LLVM's natural boolean and one *bit*: it
            // would make a `bool` field's storage disagree with `layout_of`'s one byte.
            Item::BoolType => Ok(Self::Scalar(ScalarRepr::Int {
                ty: context.i8_type(),
                signed: false,
            })),
            Item::IntType { signed, bits } => Ok(Self::Scalar(ScalarRepr::Int {
                ty: int_type(context, *bits)?,
                signed: *signed,
            })),
            // An enum *is* its backing type in a register (ADR-0041 §3), and that is `s64`
            // for every enum this language has — so `signed: true` is `s64`'s signedness
            // rather than a guess.
            Item::EnumType { .. } => Ok(Self::Scalar(ScalarRepr::Int {
                ty: context.i64_type(),
                signed: true,
            })),
            Item::FloatType { bits } => Ok(Self::Scalar(ScalarRepr::Float(match bits {
                32 => context.f32_type(),
                64 => context.f64_type(),
                other => {
                    return Err(CodegenError::Internal(format!(
                        "no LLVM type for a {other}-bit float"
                    )));
                }
            }))),
            // An integer of the target's pointer width; see the module docs.
            Item::PointerType(_) | Item::ProcType { .. } => Ok(Self::Scalar(ScalarRepr::Int {
                ty: pointer_int(context, target),
                signed: false,
            })),
            Item::StringType
            | Item::StructType { .. }
            | Item::UnionType { .. }
            | Item::VariantType { .. }
            | Item::ArrayType { .. }
            | Item::ResultsType { .. }
            | Item::ContextType
            | Item::ViewType { .. }
            | Item::DynamicArrayType { .. } => {
                let layout = layout_of(pool, target, ty)
                    .map_err(|reason| CodegenError::NoLayout { ty, reason })?;
                Ok(Self::Aggregate {
                    size: layout.size,
                    align: layout.align,
                })
            }
            // Every remaining item is either a comptime-only type or a *value* rather than a
            // type. `layout_of` already has the right words for both, so it is asked rather
            // than second-guessed.
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
            | Item::AggregateValue { .. } => Err(CodegenError::NoLayout {
                ty,
                reason: layout_of(pool, target, ty)
                    .err()
                    .unwrap_or(jr_pool::LayoutError::Poison),
            }),
        }
    }

    /// The LLVM type a value of this representation occupies, if any.
    ///
    /// `None` only for [`Repr::Void`], which occupies no register.
    #[must_use]
    pub fn llvm_type(
        self,
        context: &'ctx Context,
        target: TargetLayout,
    ) -> Option<BasicTypeEnum<'ctx>> {
        match self {
            Self::Void => None,
            Self::Scalar(ScalarRepr::Int { ty, .. }) => Some(ty.into()),
            Self::Scalar(ScalarRepr::Float(ty)) => Some(ty.into()),
            // An aggregate travels as a pointer to its bytes, held as an integer.
            Self::Aggregate { .. } => Some(pointer_int(context, target).into()),
        }
    }

    /// Whether this is an aggregate, which decides copy-versus-move at every assignment.
    #[must_use]
    pub const fn is_aggregate(self) -> bool {
        matches!(self, Self::Aggregate { .. })
    }
}

/// The LLVM integer type of `bits` bits.
///
/// # Errors
/// [`CodegenError::Internal`] for a width no machine register has. `jr-sema` only ever
/// produces 8, 16, 32 and 64, so reaching this means the pool and this function disagree
/// about what an integer type is.
pub fn int_type(context: &Context, bits: u16) -> Result<IntType<'_>, CodegenError> {
    match bits {
        8 => Ok(context.i8_type()),
        16 => Ok(context.i16_type()),
        32 => Ok(context.i32_type()),
        64 => Ok(context.i64_type()),
        other => Err(CodegenError::Internal(format!(
            "no LLVM integer type for {other} bits"
        ))),
    }
}

/// The integer type a pointer is carried in on this target.
///
/// Derived from [`TargetLayout::pointer_size`] rather than from the host, because comptime
/// and runtime layouts are distinct even where they are numerically equal today
/// (ADR-0018 §2). A width with no machine register falls back to 64 bits, which is the only
/// width this project's targets have.
#[must_use]
pub fn pointer_int(context: &Context, target: TargetLayout) -> IntType<'_> {
    match target.pointer_size {
        4 => context.i32_type(),
        _ => context.i64_type(),
    }
}

/// Builds the LLVM function type for a procedure.
///
/// `void` contributes no parameter and no result, which is what makes [`Repr::Void`] a case
/// rather than a zero-width scalar.
///
/// An aggregate return is realised as a **caller-allocated result pointer** in the leading
/// parameter position, exactly as the Cranelift back end's `sret` is (ADR-0051 §1), and
/// [`returns_via_sret`] is the one predicate that decides it — so the function type and the
/// body cannot disagree about whether a given procedure has the hidden parameter.
///
/// The pointer is an ordinary integer parameter rather than an LLVM `sret` attribute: both
/// sides of every such call are generated here, so nothing needs LLVM's own struct-return
/// lowering, and asking for it would hand LLVM a struct type this back end deliberately does
/// not build (ADR-0143 §4).
///
/// # Errors
/// [`CodegenError::NoLayout`] when a parameter or return type has none.
/// [`CodegenError::Unsupported`] when an aggregate would cross a `#foreign` boundary — as a
/// parameter *or* as a return — because that needs each platform's own struct classification
/// rules and guessing them puts garbage in a register with no diagnostic (ADR-0051 §4).
pub fn function_type<'ctx>(
    context: &'ctx Context,
    pool: &Pool,
    target: TargetLayout,
    params: &[PoolId],
    ret: PoolId,
    foreign: bool,
    receives_context: bool,
    describe: &dyn Fn(&str) -> CodegenError,
) -> Result<FunctionType<'ctx>, CodegenError> {
    let mut arguments: Vec<BasicMetadataTypeEnum<'ctx>> = Vec::new();

    let ret_repr = Repr::of(context, pool, target, ret)?;
    if ret_repr.is_aggregate() {
        if foreign {
            return Err(describe(
                "an aggregate returned across a `#foreign` boundary",
            ));
        }
        arguments.push(pointer_int(context, target).into());
    }

    // The context is a *pointer* to the caller's context, so it is a scalar (ADR-0057 §2),
    // and it sits after the result pointer and before the declared parameters — the same
    // order the Cranelift back end uses, because one shared predicate computes the offset.
    if receives_context {
        arguments.push(pointer_int(context, target).into());
    }

    for param in params {
        match Repr::of(context, pool, target, *param)? {
            Repr::Void => {}
            Repr::Scalar(_) => {
                let ty = Repr::of(context, pool, target, *param)?
                    .llvm_type(context, target)
                    .ok_or_else(|| describe("a parameter with no register representation"))?;
                arguments.push(ty.into());
            }
            Repr::Aggregate { .. } => {
                if foreign {
                    return Err(describe("an aggregate passed across a `#foreign` boundary"));
                }
                arguments.push(pointer_int(context, target).into());
            }
        }
    }

    Ok(match ret_repr {
        // An aggregate return writes through the leading pointer and returns nothing.
        Repr::Void | Repr::Aggregate { .. } => context.void_type().fn_type(&arguments, false),
        Repr::Scalar(ScalarRepr::Int { ty, .. }) => ty.fn_type(&arguments, false),
        Repr::Scalar(ScalarRepr::Float(ty)) => ty.fn_type(&arguments, false),
    })
}

/// Whether a procedure returning `ret` uses the caller-allocated result pointer.
///
/// The single predicate both the function type and the body consult. Two separate tests for
/// "does this have a hidden first parameter" would be two chances to disagree, and a
/// disagreement here shifts *every* argument by one position — a silent miscompile rather
/// than a crash.
///
/// # Errors
/// [`CodegenError::NoLayout`] when `ret` has no layout.
pub fn returns_via_sret(
    context: &Context,
    pool: &Pool,
    target: TargetLayout,
    ret: PoolId,
) -> Result<bool, CodegenError> {
    Ok(Repr::of(context, pool, target, ret)?.is_aggregate())
}
