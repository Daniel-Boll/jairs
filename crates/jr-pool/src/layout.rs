//! Size, alignment and field offsets — the one computation both backends share.
//!
//! # Why layout lives in the pool
//!
//! ADR-0017 §5 deliberately kept layout out of MIR and deferred the question of
//! where it belongs, on the grounds that the VM and Cranelift must agree exactly
//! and so it wants to be *one* computation — "most likely a Pool query added when
//! the second consumer appears and can constrain its shape". ADR-0018 §2 settles
//! it here, because the pool already owns every input: struct identity is a
//! [`DeclId`], the field list is [`Pool::struct_fields`], and pointer types are
//! structural and nest. Layout is a fold over data this crate holds and nobody
//! else does, so putting it anywhere else means re-exposing that data.
//!
//! The rejected homes are argued in ADR-0018 §2. Briefly: a separate `jr-layout`
//! crate would have to re-expose enough of the pool to walk fields, so the
//! coupling survives the split; `jr-codegen-clif` is where the target *ABI*
//! belongs but making `jr-vm` depend on the backend to run a program inverts the
//! real dependency; and an ad-hoc copy inside `jr-vm` is exactly the silent
//! divergence the deferral existed to prevent.
//!
//! # Why the target is a parameter
//!
//! Nothing here reads the host. [`TargetLayout`] carries the pointer width and
//! alignment, and every entry point takes one, so this module is a pure function
//! of `(Pool, TargetLayout, PoolId)`. That keeps `jr-pool` free of any *ambient*
//! notion of a target — it knows a target exists, but never which one — and it is
//! what lets a cross-compile compute a guest layout while the compiler itself
//! runs on the host. The cost, named in ADR-0018's consequences, is that the
//! parameter is viral: every caller must have a [`TargetLayout`] to pass.
//!
//! # Fields are never reordered
//!
//! A struct is laid out in declaration order: each field goes at the next offset
//! that satisfies its own alignment, the struct's alignment is the maximum of its
//! fields', and its size is rounded up to that alignment so arrays of it stay
//! aligned. This is the C rule, and it is chosen because Jairs is a systems
//! language whose structs cross the `#foreign` boundary — a reordering compiler
//! would make every `#foreign` struct declaration a lie. Jai makes the same
//! choice for the same reason. The cost is padding a reordering pass would
//! recover, which is a deliberate trade rather than an oversight.
//!
//! # This is where ADR-0004 stops being prose
//!
//! ADR-0004 fixes `string` as `{data: *u8, count: s64}` and, until now, only in
//! prose: the pool has no fields for it and `jr-sema` hardcodes the two names as
//! pseudo-fields, which is why [`crate::PoolId::STRING`] had no computable size
//! and MIR gave `.data`/`.count` their own symbolic projections. [`string_data`]
//! and [`string_count`] are that layout, executable. `string` is still *not* the
//! struct type of that shape (ADR-0015 §2): the layout is shared, the identity is
//! not.

use crate::item::{DeclId, Item, PoolId};
use crate::pool::Pool;

// ---------------------------------------------------------------------------
// Target parameters
// ---------------------------------------------------------------------------

/// The target-dependent inputs to a layout computation.
///
/// Only pointer width and pointer alignment vary across the targets Jairs cares
/// about; integer widths are spelled in the type itself (`s64` is 64 bits on
/// every target, by ADR-0015's `IntType { bits }`), and `bool` is one byte
/// everywhere. So this deliberately carries two numbers rather than a general
/// data-layout string — there is nothing else for it to carry yet, and inventing
/// fields with no consumer is how a target description becomes wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TargetLayout {
    /// The size of a pointer, in bytes.
    pub pointer_size: u32,
    /// The alignment of a pointer, in bytes.
    pub pointer_align: u32,
}

impl TargetLayout {
    /// 64-bit pointers, 64-bit aligned — arm64 and x86-64 alike.
    ///
    /// Every target in the slice is this one. It is named rather than defaulted
    /// so that a caller states which layout it means.
    pub const LP64: Self = Self {
        pointer_size: 8,
        pointer_align: 8,
    };

    /// The layout of the machine this compiler is running on.
    ///
    /// This is what comptime execution wants: a `#run` that takes the address of
    /// a local is manipulating a pointer inside *this* process, so its width must
    /// be the host's. Runtime layout is the *target's*, and the two are the same
    /// number today only because the slice cross-compiles nowhere.
    #[must_use]
    pub const fn host() -> Self {
        Self {
            pointer_size: size_of::<usize>() as u32,
            pointer_align: align_of::<usize>() as u32,
        }
    }
}

// ---------------------------------------------------------------------------
// The result
// ---------------------------------------------------------------------------

/// A type's size and alignment, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Layout {
    /// The number of bytes an instance occupies, including trailing padding.
    ///
    /// Already rounded up to [`Self::align`], so that consecutive instances in
    /// an array are each aligned.
    pub size: u64,
    /// The required alignment, in bytes. Always a power of two, never zero.
    pub align: u32,
}

impl Layout {
    /// A zero-sized type.
    ///
    /// Alignment is 1 rather than 0: an alignment of zero would make
    /// [`align_up`] divide by zero, and a zero-sized value still has to have a
    /// well-defined address.
    pub const ZERO: Self = Self { size: 0, align: 1 };

    /// A scalar of `size` bytes, aligned to its own size.
    ///
    /// Every scalar Jairs has — `bool`, the integer types, pointers — is
    /// naturally aligned, so this is the only scalar constructor needed.
    #[must_use]
    const fn scalar(size: u32) -> Self {
        Self {
            size: size as u64,
            align: size,
        }
    }
}

/// Why a type has no layout.
///
/// These are all *compiler* errors rather than user errors — a well-typed program
/// that reached the VM cannot produce any of them — which is why they carry
/// enough detail to name the culprit rather than a diagnostic code. ADR-0017 §4's
/// gate is what makes [`Self::Poison`] unreachable in practice, and it is
/// returned rather than asserted so that a hole in that gate surfaces as an error
/// instead of a panic in the middle of interpretation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LayoutError {
    /// [`PoolId::ERROR`] has no layout.
    ///
    /// Reaching this means something asked for the layout of a poisoned type,
    /// which ADR-0017 §4's gate is supposed to have prevented.
    Poison,
    /// The argument names a compile-time *value*, not a type.
    NotAType(PoolId),
    /// A struct's fields have not been recorded yet.
    ///
    /// [`Pool::set_struct_fields`] is called during signature resolution, so this
    /// means layout was asked for before the struct's body was resolved.
    UnresolvedStruct(DeclId),
    /// A struct contains itself without an intervening pointer, so its size is
    /// infinite.
    ///
    /// `jr-sema` does not reject this today, so the guard is here rather than
    /// merely assumed away: without it the fold recurses until the compiler's
    /// stack runs out, which is the one failure mode a compiler must never have.
    Recursive(DeclId),
    /// The type has no runtime representation at all.
    ///
    /// A type used as a value ([`Item::TypeType`], wave W4) and a
    /// `#system_library` handle ([`Item::ForeignLibraryType`], ADR-0016 §3) exist
    /// only during compilation. Asking for their runtime size is a category
    /// error, distinguished from [`Layout::ZERO`] deliberately: `void` genuinely
    /// occupies no bytes and may be stored, whereas these cannot be stored at
    /// all.
    ComptimeOnly(PoolId),
    /// An array's total size overflowed a `u64`.
    ///
    /// `[N]T` is `N` elements of `T`, and both come from source: nothing bounds their
    /// product. Distinguished from [`LayoutError::Recursive`] because that one is a
    /// compiler guard against a shape sema does not reject, while this is arithmetic on
    /// numbers a program wrote.
    ArrayTooLarge {
        /// The element type.
        elem: PoolId,
        /// The requested length.
        len: u64,
    },
    /// Array or struct nesting exceeded this module's depth limit.
    ///
    /// A separate variant from [`LayoutError::Recursive`], which names a *struct*: an
    /// array nests structurally (`[2][2][2]...u8`) with no declaration to blame, so there
    /// is no `DeclId` to report.
    Depth,
}

impl std::fmt::Display for LayoutError {
    /// Renders the culprit, not just the category.
    ///
    /// Every consumer of this type reports a *compiler* fault — `jr-vm`'s
    /// `VmError::Internal` and `jr-codegen`'s `CodegenError::NoLayout` — so the
    /// message is read by whoever has to fix the compiler. Naming the offending
    /// [`PoolId`] or [`DeclId`] is the whole reason these variants carry one, and a
    /// `Debug` rendering at the call site would have thrown that away.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Poison => write!(f, "a poisoned type has no layout"),
            Self::NotAType(id) => {
                write!(f, "pool item {} is a value, not a type", id.index())
            }
            Self::UnresolvedStruct(decl) => {
                write!(f, "struct {decl:?} has no recorded fields yet")
            }
            Self::Recursive(decl) => {
                write!(f, "struct {decl:?} contains itself without a pointer")
            }
            Self::ComptimeOnly(id) => write!(
                f,
                "pool item {} exists only at compile time and has no runtime size",
                id.index()
            ),
            Self::ArrayTooLarge { elem, len } => write!(
                f,
                "an array of {len} elements of pool item {} is larger than a `u64` can \
                 describe",
                elem.index()
            ),
            Self::Depth => write!(f, "type nesting exceeded the layout depth limit"),
        }
    }
}

impl std::error::Error for LayoutError {}

// ---------------------------------------------------------------------------
// Alignment arithmetic
// ---------------------------------------------------------------------------

/// Rounds `offset` up to the next multiple of `align`.
///
/// # Panics
/// Panics if `align` is zero. Every [`Layout`] in this module has a non-zero
/// alignment by construction, so a zero here is a bug in this file rather than
/// bad input.
#[must_use]
pub const fn align_up(offset: u64, align: u32) -> u64 {
    assert!(align != 0, "alignment must be non-zero");
    let align = align as u64;
    offset.next_multiple_of(align)
}

// ---------------------------------------------------------------------------
// The `{data, count}` pair
// ---------------------------------------------------------------------------

/// The offset and layout of the `data` word of a `{data, count}` pair.
///
/// First field, so offset 0, and pointer-shaped.
///
/// **Two types have this layout**: `string` (ADR-0004) and `[]T` (ADR-0044 §1). The
/// arithmetic is shared and the identities are not — a view is not a string and a string is
/// not a byte run (ADR-0015 §2) — so this computes offsets for both while
/// [`crate::Item::StringType`] and [`crate::Item::ViewType`] stay distinct types with
/// distinct projections.
#[must_use]
pub const fn pair_data(target: TargetLayout) -> (u64, Layout) {
    (
        0,
        Layout {
            size: target.pointer_size as u64,
            align: target.pointer_align,
        },
    )
}

/// The offset and layout of the `count` word of a `{data, count}` pair.
///
/// An `s64` placed after `data`, at the next 8-aligned offset. `s64` rather than `u64` for
/// both users, so that `i < xs.count` is not a mixed-signedness comparison ADR-0015's
/// no-coercion rule would refuse (ADR-0044 §1).
#[must_use]
pub const fn pair_count(target: TargetLayout) -> (u64, Layout) {
    let count = Layout::scalar(8);
    (align_up(target.pointer_size as u64, count.align), count)
}

/// The layout of a `{data, count}` pair itself.
#[must_use]
pub const fn pair_layout(target: TargetLayout) -> Layout {
    let (offset, count) = pair_count(target);
    let align = if target.pointer_align > count.align {
        target.pointer_align
    } else {
        count.align
    };
    Layout {
        size: align_up(offset + count.size, align),
        align,
    }
}

/// The offset and layout of a `{data, count, capacity}` triple's `.capacity` field — a
/// dynamic array (ADR-0136 §1). The capacity is an `s64` alongside `count`, both after
/// `data: *T`, so this is `pair_data + 2 * s64.size` in the ordinary case where
/// `pointer_size == 8`.
#[must_use]
pub const fn triple_capacity(target: TargetLayout) -> (u64, Layout) {
    let (data_offset, _) = pair_data(target);
    let s64_layout = Layout::scalar(8);
    let count_offset = align_up(data_offset + target.pointer_size as u64, s64_layout.align);
    let capacity_offset = align_up(count_offset + s64_layout.size, s64_layout.align);
    (capacity_offset, s64_layout)
}

/// The layout of a `{data, count, capacity}` triple — the compiler-known layout of `[..]T`
/// (ADR-0136 §1). One pointer plus two `s64`s.
#[must_use]
pub const fn triple_layout(target: TargetLayout) -> Layout {
    let (capacity_offset, capacity) = triple_capacity(target);
    let pair = pair_layout(target);
    let align = if target.pointer_align > pair.align {
        target.pointer_align
    } else {
        pair.align
    };
    Layout {
        size: align_up(capacity_offset + capacity.size, align),
        align,
    }
}

/// The offset and layout of `string`'s `.data` field (ADR-0004).
#[must_use]
pub const fn string_data(target: TargetLayout) -> (u64, Layout) {
    pair_data(target)
}

/// The offset and layout of `string`'s `.count` field (ADR-0004).
#[must_use]
pub const fn string_count(target: TargetLayout) -> (u64, Layout) {
    pair_count(target)
}

/// The layout of `string` itself.
#[must_use]
pub const fn string_layout(target: TargetLayout) -> Layout {
    pair_layout(target)
}

// ---------------------------------------------------------------------------
// The fold
// ---------------------------------------------------------------------------

/// The maximum struct nesting depth before [`LayoutError::Recursive`].
///
/// A depth counter rather than a visited set: a visited set is the exact answer,
/// but it allocates on every layout query, and the queries here are hot enough
/// (once per field access in the interpreter) that the exact answer is not worth
/// its cost. Anything nested this deep is pathological either way, and the guard
/// exists to convert a stack overflow into an error, not to diagnose precisely.
const MAX_DEPTH: u32 = 64;

/// The layout of a type.
///
/// # Errors
/// Returns [`LayoutError`] for a poisoned type, a value mistaken for a type, a
/// struct whose fields are not yet recorded, an infinitely recursive struct, or a
/// comptime-only type. See that enum for which is which.
pub fn layout_of(pool: &Pool, target: TargetLayout, ty: PoolId) -> Result<Layout, LayoutError> {
    layout_at_depth(pool, target, ty, 0)
}

fn layout_at_depth(
    pool: &Pool,
    target: TargetLayout,
    ty: PoolId,
    depth: u32,
) -> Result<Layout, LayoutError> {
    // Matched exhaustively by variant so that a new `Item` is a compile error
    // here rather than a type that silently reports someone else's size.
    match pool.item(ty) {
        Item::ErrorType => Err(LayoutError::Poison),

        // `void` occupies no bytes but is a real type (ADR-0015 §3), so a
        // procedure returning it still has a total return layout.
        Item::VoidType => Ok(Layout::ZERO),

        Item::BoolType => Ok(Layout::scalar(1)),

        // ADR-0015 spells the width in the type, so no target lookup is needed.
        // Rounded up to whole bytes: a hypothetical `u1` occupies a byte, because
        // sub-byte addressing is not a thing a pointer can express.
        Item::IntType { bits, .. } => Ok(Layout::scalar(u32::from(bits.div_ceil(8)).max(1))),

        // Self-aligned, like every IEEE-754 type on both targets: 4 bytes for `float32`,
        // 8 for `float64` (ADR-0040 §2). The width is spelled in the type, so no target
        // lookup is needed — the same reason `IntType` above needs none.
        Item::FloatType { bits } => Ok(Layout::scalar(u32::from(bits / 8))),

        Item::StringType => Ok(string_layout(target)),

        // A procedure used as a value is a code pointer (ADR-0012), so it is
        // pointer-shaped. `Callee::Indirect` is what consumes this.
        Item::PointerType(_) | Item::ProcType { .. } => Ok(Layout {
            size: u64::from(target.pointer_size),
            align: target.pointer_align,
        }),

        Item::TypeType | Item::ForeignLibraryType => Err(LayoutError::ComptimeOnly(ty)),

        // `size = stride * len`, where the stride is the element size rounded up to the
        // element's own alignment. For every type Jairs has today the two are equal,
        // because `struct_layout_at_depth` below already rounds a struct's size up to its
        // alignment for exactly this reason — but computing the stride explicitly means an
        // element type that ever *stops* being self-aligned cannot silently overlap.
        //
        // A zero-length array is legal and zero-sized. It keeps the element's alignment,
        // so `*[0]T` is still a properly aligned pointer.
        Item::ArrayType { elem, len } => {
            if depth >= MAX_DEPTH {
                return Err(LayoutError::Depth);
            }
            let elem_layout = layout_at_depth(pool, target, *elem, depth + 1)?;
            let stride = align_up(elem_layout.size, elem_layout.align);
            Ok(Layout {
                size: stride.checked_mul(*len).ok_or(LayoutError::ArrayTooLarge {
                    elem: *elem,
                    len: *len,
                })?,
                align: elem_layout.align,
            })
        }

        // A vector is the *same bytes* as the array of its element type (ADR-0148 §1): sixteen of
        // them, contiguous. This arm therefore deliberately duplicates no arithmetic — it defers to
        // the array computation above by doing the same two lines, and the alignment is the whole
        // vector's rather than the element's, because a machine vector load wants its register
        // width. Sema has already refused every width but 16 (E0285), so the multiply cannot
        // overflow and no `ArrayTooLarge` is reachable here.
        Item::VectorType { elem, lanes } => {
            if depth >= MAX_DEPTH {
                return Err(LayoutError::Depth);
            }
            let elem_layout = layout_at_depth(pool, target, *elem, depth + 1)?;
            let size = elem_layout.size.checked_mul(*lanes).ok_or(
                LayoutError::ArrayTooLarge {
                    elem: *elem,
                    len: *lanes,
                },
            )?;
            Ok(Layout {
                size,
                // 16 by construction, and written as the size rather than the literal so that a
                // future width (a 256-bit vector, if a back end ever carries one) needs no edit
                // here — the constraint lives in sema, which is where it can produce a message.
                align: u32::try_from(size).unwrap_or(16),
            })
        }

        // A view is the same two words `string` is (ADR-0044 §1), and the element type does
        // not enter the layout at all: a view of a `[100]u8` and a view of a `u8` are both
        // one pointer and one count. That is what makes `[]T` passable where `[N]T` is not.
        Item::ViewType { .. } => Ok(pair_layout(target)),

        // A dynamic array is a view *plus* a capacity — three words instead of two (ADR-0136 §1).
        // Like a view, the element type does not enter the layout, so `[..]s64` and `[..]u8`
        // have the same size and alignment.
        Item::DynamicArrayType { .. } => Ok(triple_layout(target)),

        // An enum's layout **is** its backing type's (ADR-0041 §3), which is `s64` for every
        // enum this wave has. Delegating rather than repeating `Layout::scalar(8)` is what
        // makes an explicit backing type — `enum u8 { … }` — a change to one line here
        // rather than a change to layout.
        Item::EnumType { .. } => layout_at_depth(pool, target, PoolId::S64, depth + 1),

        Item::StructType { decl, .. } => struct_layout_at_depth(pool, target, ty, *decl, depth),

        // A results aggregate lays out exactly as a struct of the same field types, through the
        // *same* function (ADR-0052 §1). The element list is right here, so unlike a struct there
        // is no side table to be unresolved.
        Item::ResultsType { elems } => {
            let placed: Vec<Placement> = elems.iter().copied().map(Placement::plain).collect();
            sequential_layout(pool, target, &placed, depth)
        }

        // The context's fields are fixed by the compiler (ADR-0057 §1), so they are listed here
        // rather than read from the struct side table — there is no `DeclId` to key one on.
        Item::ContextType => {
            let placed: Vec<Placement> = CONTEXT_FIELD_TYPES
                .iter()
                .copied()
                .map(Placement::plain)
                .collect();
            sequential_layout(pool, target, &placed, depth)
        }

        // A union: every field at offset 0, so the size is the **largest** field's rather than
        // the running sum, and the alignment is the strictest (ADR-0045 §3). The size is then
        // rounded up to that alignment exactly as a struct's is, so an array of unions stays
        // aligned at every element.
        //
        // This and `field_offset`'s union arm are the two places a union differs from a struct
        // at all. Both live here because ADR-0018 §2 makes this the one place layout may be
        // computed — and for a union that is not a formality: a layout disagreement between the
        // engines would be *invisible*, since both would read plausible bits from the wrong
        // place rather than crashing.
        Item::UnionType { decl, .. } => union_layout_at_depth(pool, target, ty, *decl, depth),
        Item::VariantType { decl, .. } => variant_layout_at_depth(pool, target, ty, *decl, depth),

        Item::VoidValue
        | Item::BoolValue(_)
        | Item::IntValue { .. }
        | Item::FloatValue { .. }
        | Item::StrValue(_)
        | Item::TypeValue(_)
        | Item::ProcValue { .. }
        | Item::ForeignLibraryValue(_)
        // An aggregate *value* has no layout of its own — its **type** has one, and that arm already
        // works (ADR-0074's consequences). Asking this of the value is the same category error as
        // asking it of an `IntValue`, so it answers the same way.
        | Item::AggregateValue { .. } => Err(LayoutError::NotAType(ty)),
    }
}

/// The layout of a union: the largest field's size, the strictest field's alignment.
///
/// An **empty union is zero-sized** with alignment 1, matching an empty struct. Refusing it
/// would be a special case with no argument behind it (ADR-0045 §3).
fn union_layout_at_depth(
    pool: &Pool,
    target: TargetLayout,
    ty: PoolId,
    decl: DeclId,
    depth: u32,
) -> Result<Layout, LayoutError> {
    if depth >= MAX_DEPTH {
        return Err(LayoutError::Recursive(decl));
    }
    let fields = pool
        .fields_of(ty)
        .ok_or(LayoutError::UnresolvedStruct(decl))?;

    let mut size = 0u64;
    let mut align = 1u32;
    for field in fields {
        let field_layout = layout_at_depth(pool, target, field.ty, depth + 1)?;
        size = size.max(field_layout.size);
        align = align.max(field_layout.align);
    }
    Ok(Layout {
        size: align_up(size, align),
        align,
    })
}

/// The layout of a `variant`: a leading tag byte, then the union of its cases (ADR-0068 §3).
///
/// Built from the *existing* pieces rather than a new algorithm — the cases' union is
/// [`union_layout_at_depth`], and the tag is one byte before it — so this crate gains a case and no new
/// layout rule. The tag sits at offset **0** (§3): a leading field's offset does not depend on what
/// follows, so nothing has to derive a position from the case count.
///
/// The whole thing takes the cases' alignment, so the payload starts at that alignment and the tag's
/// byte is usually absorbed by the padding the union needed anyway — which is why the size cost
/// ADR-0045 §1 warned about is real but small.
fn variant_layout_at_depth(
    pool: &Pool,
    target: TargetLayout,
    ty: PoolId,
    decl: DeclId,
    depth: u32,
) -> Result<Layout, LayoutError> {
    let cases = union_layout_at_depth(pool, target, ty, decl, depth)?;
    // The payload begins after the tag, rounded up to the payload's own alignment.
    let align = cases.align.max(TAG_ALIGN);
    let payload_at = align_up(u64::from(TAG_SIZE), align);
    Ok(Layout {
        size: align_up(payload_at + cases.size, align),
        align,
    })
}

/// The byte offset a `variant`'s payload starts at (ADR-0068 §3).
///
/// Every case shares it, because a variant is a tag plus a *union* — so this is the one number both
/// engines need to reach a case, and it is computed here rather than in either of them (ADR-0018 §2).
///
/// # Errors
/// [`LayoutError`] when the variant's cases have no layout.
pub fn variant_payload_offset(
    pool: &Pool,
    target: TargetLayout,
    ty: PoolId,
    decl: DeclId,
) -> Result<u64, LayoutError> {
    let cases = union_layout_at_depth(pool, target, ty, decl, 0)?;
    let align = cases.align.max(TAG_ALIGN);
    Ok(align_up(u64::from(TAG_SIZE), align))
}

/// The size of a `variant`'s tag, in bytes (ADR-0068 §3).
///
/// One byte: no variant the language can express has more than 256 cases, and a wider tag would cost
/// size for a range nothing reaches.
pub const TAG_SIZE: u8 = 1;

/// The alignment of a `variant`'s tag — one byte, like its size.
pub const TAG_ALIGN: u32 = 1;

fn struct_layout_at_depth(
    pool: &Pool,
    target: TargetLayout,
    ty: PoolId,
    decl: DeclId,
    depth: u32,
) -> Result<Layout, LayoutError> {
    if depth >= MAX_DEPTH {
        return Err(LayoutError::Recursive(decl));
    }
    let fields = pool
        .fields_of(ty)
        .ok_or(LayoutError::UnresolvedStruct(decl))?;
    let placed: Vec<Placement> = fields.iter().map(Placement::of).collect();
    sequential_layout(pool, target, &placed, depth)
}

/// The field types of [`Item::ContextType`], in order (ADR-0057 §1).
///
/// A `const` rather than a function so that layout, field lookup and both engines read the *same*
/// list — three copies of "what fields does a context have" would be three chances to disagree, which
/// is the duplication ADR-0052 found for field types across three crates.
///
/// The two temporary-storage fields (ADR-0065) cost no new well-known id: `temp_data` is
/// [`PoolId::PTR_U8`] and `temp_mark` is [`PoolId::S64`], both already well-known — unlike the
/// allocator's proc-pointer types, which had to be pre-interned. So `WELL_KNOWN_COUNT` does not move.
pub const CONTEXT_FIELD_TYPES: &[PoolId] = &[
    PoolId::ALLOC_FN,
    PoolId::FREE_FN,
    PoolId::S64,
    PoolId::PTR_U8,
    PoolId::S64,
];

/// The field *names* of [`Item::ContextType`], parallel to [`CONTEXT_FIELD_TYPES`].
///
/// Five fields: the two halves of an allocator and its state word (ADR-0062 §2), then the temporary
/// storage arena's region pointer and bump cursor (ADR-0065). Flattened into the context rather than
/// nested, because a nested struct type would need a `DeclId` a compiler-declared type has not got —
/// the same problem ADR-0057 §1 met and solved by going structural. `temp_mark` is a *byte count*
/// (the next allocation is at `temp_data + temp_mark`), so a reset is one integer store.
pub const CONTEXT_FIELD_NAMES: &[&str] = &[
    "allocator",
    "allocator_free",
    "allocator_data",
    "temp_data",
    "temp_mark",
];

/// One field's type and its layout attributes, as the fold sees it (ADR-0144 §5).
///
/// A struct's fields carry attributes; a results aggregate's and the context's do not, because
/// neither has a source declaration to write one on. So the fold takes this rather than a bare
/// [`PoolId`], and the two attribute-free callers build a default — which keeps *one* fold rather
/// than an attributed one beside a plain one, the duplication ADR-0018 §2 exists to avoid.
#[derive(Debug, Clone, Copy)]
struct Placement {
    /// The field's type.
    ty: PoolId,
    /// `#align N`, a minimum (ADR-0144 §3).
    align: Option<u32>,
    /// `#place N`, an exact offset (ADR-0144 §4).
    place: Option<u64>,
}

impl Placement {
    /// The placement of a declared field.
    fn of(field: &crate::Field) -> Self {
        Self {
            ty: field.ty,
            align: field.align,
            place: field.place,
        }
    }

    /// A field with no layout attributes, for an aggregate whose fields cannot carry any.
    const fn plain(ty: PoolId) -> Self {
        Self {
            ty,
            align: None,
            place: None,
        }
    }

    /// This field's alignment, given its type's own.
    fn effective_align(self, natural: u32) -> u32 {
        natural.max(self.align.unwrap_or(1))
    }
}

/// The layout of a sequence of fields laid out in order, C-style.
///
/// Shared by a struct's layout and a **results aggregate**'s (ADR-0052 §1), because the two are the
/// same computation over the same rules. Factored out rather than copied: a second implementation of
/// field offsets would be a *silent wrong offset* rather than a crash — the failure mode ADR-0018 §2
/// made one shared layout function to prevent, and which no verifier can catch.
///
/// A `#place`d field goes at exactly its offset and the cursor advances to the **maximum** end
/// reached so far, so placing one field cannot silently move another (ADR-0144 §4). With no
/// attributes anywhere this is byte-for-byte the fold it replaced, which the unchanged MIR
/// snapshots are the evidence for.
fn sequential_layout(
    pool: &Pool,
    target: TargetLayout,
    fields: &[Placement],
    depth: u32,
) -> Result<Layout, LayoutError> {
    let mut end = 0u64;
    let mut align = 1u32;
    for field in fields {
        let field_layout = layout_at_depth(pool, target, field.ty, depth + 1)?;
        let field_align = field.effective_align(field_layout.align);
        let at = match field.place {
            Some(offset) => offset,
            None => align_up(end, field_align),
        };
        end = end.max(at + field_layout.size);
        align = align.max(field_align);
    }
    // Rounding the total up to the aggregate's own alignment is what makes an array
    // of it aligned at every element, which is why it is not merely the offset
    // past the last field.
    Ok(Layout {
        size: align_up(end, align),
        align,
    })
}

/// The byte offset and layout of one field of a struct type.
///
/// `index` is a [`crate::Field`] position in [`Pool::struct_fields`] — the same
/// index `jr_mir`'s `Projection::Field` carries, which is the whole reason MIR was
/// allowed to stay symbolic.
///
/// # Errors
/// Returns [`LayoutError`] as [`layout_of`] does, and additionally
/// [`LayoutError::NotAType`] if `ty` is not a struct type or `index` is out of
/// range — both of which mean the caller's MIR disagrees with the pool.
pub fn field_offset(
    pool: &Pool,
    target: TargetLayout,
    ty: PoolId,
    index: u32,
) -> Result<(u64, Layout), LayoutError> {
    // **Every field of a union is at offset 0.** This is the single line that makes a union a
    // union, it is shared by both engines, and getting it wrong would be a silent
    // wrong-address bug rather than an error (ADR-0045 §3).
    if let Item::UnionType { decl, .. } = pool.item(ty) {
        let fields = pool
            .fields_of(ty)
            .ok_or(LayoutError::UnresolvedStruct(*decl))?;
        let field = fields
            .get(index as usize)
            .ok_or(LayoutError::NotAType(ty))?;
        return Ok((0, layout_of(pool, target, field.ty)?));
    }

    // **Every case of a variant is at the payload offset**, which is the same line one step along: a
    // variant is a tag followed by a union, so its cases overlap each other but sit *after* the tag
    // (ADR-0068 §3). Getting this wrong would read the tag as part of a case, or a case as the tag —
    // a silent wrong-address bug of exactly the kind the union arm above warns about, which is why
    // the offset is computed here and in neither engine.
    if let Item::VariantType { decl, .. } = pool.item(ty) {
        let fields = pool
            .fields_of(ty)
            .ok_or(LayoutError::UnresolvedStruct(*decl))?;
        let field = fields
            .get(index as usize)
            .ok_or(LayoutError::NotAType(ty))?;
        let at = variant_payload_offset(pool, target, ty, *decl)?;
        return Ok((at, layout_of(pool, target, field.ty)?));
    }

    // A results aggregate's fields are laid out in order exactly as a struct's, and its element
    // list is right here rather than in a side table (ADR-0052 §1). Sharing
    // `sequential_field_offset` below is what keeps the two from disagreeing: **omitting this
    // returned `NotAType` for every result after the first**, which surfaced as a destructuring
    // statement binding the wrong values rather than as an error.
    let placed: Vec<Placement> = match pool.item(ty) {
        Item::ResultsType { elems } => elems.iter().copied().map(Placement::plain).collect(),
        // The context's fields, from the one list every consumer reads (ADR-0057 §1).
        Item::ContextType => CONTEXT_FIELD_TYPES
            .iter()
            .copied()
            .map(Placement::plain)
            .collect(),
        Item::StructType { decl, .. } => pool
            .fields_of(ty)
            .ok_or(LayoutError::UnresolvedStruct(*decl))?
            .iter()
            .map(Placement::of)
            .collect(),
        _ => return Err(LayoutError::NotAType(ty)),
    };
    sequential_field_offset(pool, target, &placed, index).ok_or(LayoutError::NotAType(ty))?
}

/// The offset and layout of one field in a sequentially laid-out aggregate.
///
/// Shared by a struct's field lookup and a results aggregate's, for the reason `sequential_layout`
/// is shared: two implementations of a field offset would be two chances to produce a silent wrong
/// address, which no verifier catches.
///
/// The cursor rule is [`sequential_layout`]'s, restated in one place rather than two: a `#place`d
/// field is at its own offset and the cursor takes the maximum end so far (ADR-0144 §4). The two
/// walks must agree, because one answers "where is field *n*" and the other "how big is the whole
/// thing", and a disagreement is a field written past the end of its own storage.
///
/// The returned [`Layout`] carries the field's **effective** alignment, so a consumer that claims
/// an alignment claims the one `#align` asked for.
///
/// Returns `None` when `index` is out of range, and `Some(Err(..))` when a field's own layout fails.
fn sequential_field_offset(
    pool: &Pool,
    target: TargetLayout,
    fields: &[Placement],
    index: u32,
) -> Option<Result<(u64, Layout), LayoutError>> {
    let mut end = 0u64;
    for (position, field) in fields.iter().enumerate() {
        let field_layout = match layout_of(pool, target, field.ty) {
            Ok(layout) => layout,
            Err(reason) => return Some(Err(reason)),
        };
        let align = field.effective_align(field_layout.align);
        let at = match field.place {
            Some(offset) => offset,
            None => align_up(end, align),
        };
        if position == index as usize {
            return Some(Ok((
                at,
                Layout {
                    size: field_layout.size,
                    align,
                },
            )));
        }
        end = end.max(at + field_layout.size);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::Field;
    use jr_base::{FileId, Interner};

    const T: TargetLayout = TargetLayout::LP64;

    fn point(pool: &mut Pool, interner: &Interner) -> PoolId {
        let decl = DeclId::new(FileId::from_usize(0), 0);
        let ty = pool.struct_type(decl);
        pool.set_struct_fields(
            decl,
            vec![
                Field::new(interner.intern("x"), PoolId::S64),
                Field::new(interner.intern("y"), PoolId::S64),
            ],
        );
        ty
    }

    #[test]
    fn scalars_are_naturally_aligned() {
        let pool = Pool::new();
        assert_eq!(
            layout_of(&pool, T, PoolId::BOOL),
            Ok(Layout { size: 1, align: 1 })
        );
        assert_eq!(
            layout_of(&pool, T, PoolId::S64),
            Ok(Layout { size: 8, align: 8 })
        );
        assert_eq!(
            layout_of(&pool, T, PoolId::U8),
            Ok(Layout { size: 1, align: 1 })
        );
        assert_eq!(
            layout_of(&pool, T, PoolId::PTR_U8),
            Ok(Layout { size: 8, align: 8 })
        );
    }

    #[test]
    fn void_is_zero_sized_but_still_has_a_layout() {
        // ADR-0015 §3 makes `void` a real type so a return layout is total.
        let pool = Pool::new();
        assert_eq!(layout_of(&pool, T, PoolId::VOID), Ok(Layout::ZERO));
        assert_eq!(
            Layout::ZERO.align,
            1,
            "a zero alignment would make align_up divide by zero"
        );
    }

    #[test]
    fn string_is_adr_0004s_two_fields() {
        let pool = Pool::new();
        assert_eq!(string_data(T), (0, Layout { size: 8, align: 8 }));
        assert_eq!(string_count(T), (8, Layout { size: 8, align: 8 }));
        assert_eq!(
            layout_of(&pool, T, PoolId::STRING),
            Ok(Layout { size: 16, align: 8 })
        );
    }

    /// A union of `u8` and `s64` at `decl`, for the layout tests.
    fn union_of(pool: &mut Pool, interner: &Interner, index: u32, tys: &[PoolId]) -> PoolId {
        let decl = DeclId::new(FileId::from_usize(0), index);
        let ty = pool.union_type(decl);
        let fields = tys
            .iter()
            .enumerate()
            .map(|(i, t)| Field::new(interner.intern(&format!("f{i}")), *t))
            .collect();
        pool.set_struct_fields(decl, fields);
        ty
    }

    /// A variant of the given case types at `decl`, for the layout tests.
    fn variant_of(pool: &mut Pool, interner: &Interner, index: u32, tys: &[PoolId]) -> PoolId {
        let decl = DeclId::new(FileId::from_usize(0), index);
        let ty = pool.variant_type(decl);
        let fields = tys
            .iter()
            .enumerate()
            .map(|(i, t)| Field::new(interner.intern(&format!("f{i}")), *t))
            .collect();
        pool.set_struct_fields(decl, fields);
        ty
    }

    #[test]
    fn a_variant_is_a_tag_then_the_union_of_its_cases() {
        // ADR-0068 §3. Two `s64` cases: the union is 8 bytes aligned 8, so the tag's byte is padded
        // out to 8 and the whole thing is 16. Every case sits at the payload offset, **not** at 0 —
        // reading a case at 0 would read the tag, which is the silent wrong-address bug the ADR
        // warns about.
        let interner = Interner::new();
        let mut pool = Pool::new();
        let ty = variant_of(&mut pool, &interner, 60, &[PoolId::S64, PoolId::S64]);
        assert_eq!(
            layout_of(&pool, T, ty),
            Ok(Layout { size: 16, align: 8 }),
            "a tag byte padded to the cases' alignment, then the cases"
        );
        for index in 0..2 {
            let (offset, _) = field_offset(&pool, T, ty, index).expect("a variant case");
            assert_eq!(offset, 8, "case {index} must sit past the tag");
        }
    }

    #[test]
    fn a_variant_of_bytes_puts_its_cases_right_after_the_tag() {
        // The case where the padding does *not* absorb the tag: `u8` cases align to 1, so the payload
        // starts at byte 1 and the whole variant is 2 bytes. This is the arithmetic that would be
        // hidden by only ever testing 8-aligned cases.
        let interner = Interner::new();
        let mut pool = Pool::new();
        let ty = variant_of(&mut pool, &interner, 61, &[PoolId::U8, PoolId::U8]);
        assert_eq!(layout_of(&pool, T, ty), Ok(Layout { size: 2, align: 1 }));
        let (offset, _) = field_offset(&pool, T, ty, 0).expect("a variant case");
        assert_eq!(offset, 1, "a byte case sits immediately after the tag");
    }

    #[test]
    fn a_union_is_its_largest_field_with_every_field_at_zero() {
        // ADR-0045 §3. The `u8` first and the `s64` second, so a struct's running-sum rule
        // would give size 16 and offset 8 for the second field — both visibly wrong here.
        let interner = Interner::new();
        let mut pool = Pool::new();
        let ty = union_of(&mut pool, &interner, 40, &[PoolId::U8, PoolId::S64]);
        assert_eq!(
            layout_of(&pool, T, ty),
            Ok(Layout { size: 8, align: 8 }),
            "the largest field's size, not the sum"
        );
        for index in 0..2 {
            let (offset, _) = field_offset(&pool, T, ty, index).expect("a union field");
            assert_eq!(offset, 0, "field {index} of a union must be at offset 0");
        }
    }

    #[test]
    fn a_unions_size_is_rounded_up_to_its_alignment() {
        // So that an array of unions stays aligned at every element — the same reason a
        // struct's size is rounded up.
        let interner = Interner::new();
        let mut pool = Pool::new();
        let ty = union_of(&mut pool, &interner, 41, &[PoolId::U8, PoolId::U8]);
        assert_eq!(layout_of(&pool, T, ty), Ok(Layout { size: 1, align: 1 }));

        let wide = union_of(&mut pool, &interner, 42, &[PoolId::S64, PoolId::U8]);
        assert_eq!(layout_of(&pool, T, wide), Ok(Layout { size: 8, align: 8 }));
    }

    #[test]
    fn an_empty_union_is_zero_sized() {
        // Legal, matching an empty struct (ADR-0045 §3).
        let interner = Interner::new();
        let mut pool = Pool::new();
        let ty = union_of(&mut pool, &interner, 43, &[]);
        assert_eq!(layout_of(&pool, T, ty), Ok(Layout::ZERO));
    }

    #[test]
    fn a_union_and_a_struct_of_the_same_fields_are_different_types() {
        // Nominal identity, and the layouts differ — which is the whole reason `UnionType` is
        // a separate `Item` variant rather than a flag (ADR-0045 §4).
        let interner = Interner::new();
        let mut pool = Pool::new();
        let u = union_of(&mut pool, &interner, 44, &[PoolId::S64, PoolId::S64]);
        let s_decl = DeclId::new(FileId::from_usize(0), 45);
        let st = pool.struct_type(s_decl);
        pool.set_struct_fields(
            s_decl,
            vec![
                Field::new(interner.intern("f0"), PoolId::S64),
                Field::new(interner.intern("f1"), PoolId::S64),
            ],
        );
        assert_ne!(u, st);
        assert_eq!(layout_of(&pool, T, u), Ok(Layout { size: 8, align: 8 }));
        assert_eq!(layout_of(&pool, T, st), Ok(Layout { size: 16, align: 8 }));
    }

    #[test]
    fn a_view_has_strings_layout_whatever_its_element_is() {
        // ADR-0044 §1: the element type does not enter a view's layout at all. A view of a
        // `[100]u8` is the same two words as a view of a `u8`, which is exactly what makes
        // `[]T` passable where `[N]T` is not.
        let mut pool = Pool::new();
        let big = pool.array_of(PoolId::U8, 100);
        let of_big = pool.view_of(big);
        let of_byte = pool.view_of(PoolId::U8);
        let expected = Layout { size: 16, align: 8 };
        assert_eq!(layout_of(&pool, T, of_big), Ok(expected));
        assert_eq!(layout_of(&pool, T, of_byte), Ok(expected));
        assert_eq!(
            layout_of(&pool, T, PoolId::STRING),
            Ok(expected),
            "a view and a string share the layout and not the identity (ADR-0015 §2)"
        );
    }

    #[test]
    fn a_view_of_a_view_nests() {
        // Structural interning, so this needs no rule of its own (ADR-0044 §6).
        let mut pool = Pool::new();
        let once = pool.view_of(PoolId::S64);
        let twice = pool.view_of(once);
        assert_ne!(once, twice);
        assert_eq!(
            pool.view_of(PoolId::S64),
            once,
            "structural: one type per elem"
        );
        assert_eq!(
            layout_of(&pool, T, twice),
            Ok(Layout { size: 16, align: 8 })
        );
    }

    #[test]
    fn string_layout_follows_a_narrower_pointer() {
        // The point of taking the target as a parameter: a 32-bit pointer moves
        // `.count`, and nothing here reads the host to find that out.
        let ilp32 = TargetLayout {
            pointer_size: 4,
            pointer_align: 4,
        };
        assert_eq!(string_data(ilp32), (0, Layout { size: 4, align: 4 }));
        assert_eq!(
            string_count(ilp32),
            (8, Layout { size: 8, align: 8 }),
            "an s64 count must still be 8-aligned, so 4 bytes of padding appear"
        );
        assert_eq!(string_layout(ilp32), Layout { size: 16, align: 8 });
    }

    #[test]
    fn a_struct_lays_its_fields_out_in_declaration_order() {
        let interner = Interner::new();
        let mut pool = Pool::new();
        let ty = point(&mut pool, &interner);
        assert_eq!(layout_of(&pool, T, ty), Ok(Layout { size: 16, align: 8 }));
        assert_eq!(
            field_offset(&pool, T, ty, 0),
            Ok((0, Layout { size: 8, align: 8 }))
        );
        assert_eq!(
            field_offset(&pool, T, ty, 1),
            Ok((8, Layout { size: 8, align: 8 }))
        );
    }

    #[test]
    fn padding_appears_where_alignment_demands_it_and_is_not_reordered() {
        // { a: u8; b: s64; c: u8; } is 24 bytes, not 16: fields are never
        // reordered, because a #foreign struct declaration must not be a lie.
        let interner = Interner::new();
        let mut pool = Pool::new();
        let decl = DeclId::new(FileId::from_usize(0), 1);
        let ty = pool.struct_type(decl);
        pool.set_struct_fields(
            decl,
            vec![
                Field::new(interner.intern("a"), PoolId::U8),
                Field::new(interner.intern("b"), PoolId::S64),
                Field::new(interner.intern("c"), PoolId::U8),
            ],
        );
        assert_eq!(layout_of(&pool, T, ty), Ok(Layout { size: 24, align: 8 }));
        assert_eq!(field_offset(&pool, T, ty, 0).unwrap().0, 0);
        assert_eq!(field_offset(&pool, T, ty, 1).unwrap().0, 8);
        assert_eq!(field_offset(&pool, T, ty, 2).unwrap().0, 16);
    }

    #[test]
    fn a_struct_of_structs_nests() {
        let interner = Interner::new();
        let mut pool = Pool::new();
        let inner = point(&mut pool, &interner);
        let decl = DeclId::new(FileId::from_usize(0), 2);
        let outer = pool.struct_type(decl);
        pool.set_struct_fields(
            decl,
            vec![
                Field::new(interner.intern("flag"), PoolId::BOOL),
                Field::new(interner.intern("at"), inner),
            ],
        );
        assert_eq!(
            layout_of(&pool, T, outer),
            Ok(Layout { size: 24, align: 8 })
        );
        assert_eq!(field_offset(&pool, T, outer, 1).unwrap().0, 8);
    }

    #[test]
    fn a_pointer_to_a_struct_needs_no_struct_body() {
        // This is what lets `Node :: struct { next: *Node; }` work at all.
        let mut pool = Pool::new();
        let unresolved = pool.struct_type(DeclId::new(FileId::from_usize(0), 9));
        let pointer = pool.pointer_to(unresolved);
        assert_eq!(
            layout_of(&pool, T, pointer),
            Ok(Layout { size: 8, align: 8 })
        );
        assert_eq!(
            layout_of(&pool, T, unresolved),
            Err(LayoutError::UnresolvedStruct(DeclId::new(
                FileId::from_usize(0),
                9
            )))
        );
    }

    #[test]
    fn a_directly_recursive_struct_errors_rather_than_overflowing_the_stack() {
        let interner = Interner::new();
        let mut pool = Pool::new();
        let decl = DeclId::new(FileId::from_usize(0), 3);
        let ty = pool.struct_type(decl);
        pool.set_struct_fields(decl, vec![Field::new(interner.intern("me"), ty)]);
        assert_eq!(layout_of(&pool, T, ty), Err(LayoutError::Recursive(decl)));
    }

    #[test]
    fn poison_has_no_layout() {
        let pool = Pool::new();
        assert_eq!(layout_of(&pool, T, PoolId::ERROR), Err(LayoutError::Poison));
    }

    #[test]
    fn a_value_is_not_a_type() {
        let pool = Pool::new();
        assert_eq!(
            layout_of(&pool, T, PoolId::TRUE),
            Err(LayoutError::NotAType(PoolId::TRUE))
        );
    }

    #[test]
    fn comptime_only_types_are_distinguished_from_zero_sized_ones() {
        let pool = Pool::new();
        assert_eq!(
            layout_of(&pool, T, PoolId::TYPE),
            Err(LayoutError::ComptimeOnly(PoolId::TYPE))
        );
        assert_eq!(
            layout_of(&pool, T, PoolId::FOREIGN_LIBRARY),
            Err(LayoutError::ComptimeOnly(PoolId::FOREIGN_LIBRARY))
        );
        assert_eq!(
            layout_of(&pool, T, PoolId::VOID),
            Ok(Layout::ZERO),
            "void is storable and zero-sized; these others are not storable at all"
        );
    }

    #[test]
    fn a_procedure_type_is_pointer_shaped() {
        let mut pool = Pool::new();
        let ty = pool.proc_type(vec![PoolId::S64], PoolId::S64, crate::ContextKind::Jairs);
        assert_eq!(layout_of(&pool, T, ty), Ok(Layout { size: 8, align: 8 }));
    }

    #[test]
    fn field_offset_rejects_an_index_past_the_end() {
        let interner = Interner::new();
        let mut pool = Pool::new();
        let ty = point(&mut pool, &interner);
        assert_eq!(
            field_offset(&pool, T, ty, 2),
            Err(LayoutError::NotAType(ty))
        );
    }

    #[test]
    fn align_up_is_idempotent_on_an_aligned_offset() {
        assert_eq!(align_up(0, 8), 0);
        assert_eq!(align_up(1, 8), 8);
        assert_eq!(align_up(8, 8), 8);
        assert_eq!(align_up(9, 8), 16);
        assert_eq!(align_up(7, 1), 7);
    }

    // -----------------------------------------------------------------------
    // ADR-0144: `#align` and `#place`
    // -----------------------------------------------------------------------

    /// Builds a struct out of `(name, type, align, place)` tuples.
    fn placed_struct(
        pool: &mut Pool,
        interner: &Interner,
        index: u32,
        fields: &[(&str, PoolId, Option<u32>, Option<u64>)],
    ) -> PoolId {
        let decl = DeclId::new(FileId::from_usize(0), index);
        let ty = pool.struct_type(decl);
        pool.set_struct_fields(
            decl,
            fields
                .iter()
                .map(|(name, field_ty, align, place)| {
                    Field::new(interner.intern(name), *field_ty).placed(*align, *place)
                })
                .collect(),
        );
        ty
    }

    #[test]
    fn align_raises_a_fields_alignment_and_the_structs() {
        // The whole of `#align`: the field moves to the requested boundary, the struct inherits
        // the alignment as the maximum of its fields', and the size rounds up to it so an array of
        // the struct is aligned at every element.
        let mut pool = Pool::new();
        let interner = Interner::new();
        let ty = placed_struct(
            &mut pool,
            &interner,
            10,
            &[
                ("tag", PoolId::U8, None, None),
                ("value", PoolId::S64, Some(16), None),
            ],
        );
        assert_eq!(
            layout_of(&pool, T, ty),
            Ok(Layout {
                size: 32,
                align: 16
            }),
            "the struct takes the field's requested alignment"
        );
        assert_eq!(
            field_offset(&pool, T, ty, 1),
            Ok((16, Layout { size: 8, align: 16 })),
            "the field sits at the requested boundary, and reports that alignment"
        );
    }

    #[test]
    fn align_below_the_natural_alignment_is_already_satisfied() {
        // ADR-0144 §3: `#align` is a **minimum**, so a value under the type's own is not an error
        // and not a packing request — it changes nothing. Asserted rather than left implicit,
        // because the alternative reading (honour it, and underalign the field) is undefined
        // behaviour in the LLVM back end and merely slow in the other two.
        let mut pool = Pool::new();
        let interner = Interner::new();
        let ty = placed_struct(
            &mut pool,
            &interner,
            11,
            &[
                ("a", PoolId::U8, None, None),
                ("b", PoolId::S64, Some(1), None),
            ],
        );
        assert_eq!(
            layout_of(&pool, T, ty),
            Ok(Layout { size: 16, align: 8 }),
            "`#align 1` on an `s64` leaves an ordinary struct"
        );
        assert_eq!(
            field_offset(&pool, T, ty, 1),
            Ok((8, Layout { size: 8, align: 8 }))
        );
    }

    #[test]
    fn place_puts_a_field_at_an_exact_offset_and_overlap_is_allowed() {
        // ADR-0144 §4. Three fields over the same eight bytes, which is what a `union` cannot say:
        // `header` does not overlap and only the last three do. Every offset is asserted, because
        // the failure mode of a placement bug is a *wrong address* rather than an error.
        let mut pool = Pool::new();
        let interner = Interner::new();
        let ty = placed_struct(
            &mut pool,
            &interner,
            12,
            &[
                ("header", PoolId::S64, None, None),
                ("lo", PoolId::U8, None, Some(8)),
                ("hi", PoolId::U8, None, Some(9)),
                ("both", PoolId::S64, None, Some(8)),
            ],
        );
        assert_eq!(field_offset(&pool, T, ty, 0).map(|(at, _)| at), Ok(0));
        assert_eq!(field_offset(&pool, T, ty, 1).map(|(at, _)| at), Ok(8));
        assert_eq!(field_offset(&pool, T, ty, 2).map(|(at, _)| at), Ok(9));
        assert_eq!(
            field_offset(&pool, T, ty, 3).map(|(at, _)| at),
            Ok(8),
            "two fields may share an offset, which is the point of the attribute"
        );
        assert_eq!(
            layout_of(&pool, T, ty),
            Ok(Layout { size: 16, align: 8 }),
            "the size is the maximum end reached, not the sum of the fields"
        );
    }

    #[test]
    fn a_placed_field_does_not_move_the_ones_after_it() {
        // ADR-0144 §4's cursor rule, which is the part a reader would not guess: the cursor takes
        // the *maximum* end so far, so placing one field low cannot pull a later one backwards over
        // it. A running cursor set to the placed field's end would put `tail` at 16 here.
        let mut pool = Pool::new();
        let interner = Interner::new();
        let ty = placed_struct(
            &mut pool,
            &interner,
            13,
            &[
                ("wide", PoolId::S64, None, None),
                ("early", PoolId::U8, None, Some(0)),
                ("tail", PoolId::S64, None, None),
            ],
        );
        assert_eq!(field_offset(&pool, T, ty, 2).map(|(at, _)| at), Ok(8));
        assert_eq!(layout_of(&pool, T, ty), Ok(Layout { size: 16, align: 8 }));
    }

    #[test]
    fn a_placed_field_may_be_unaligned() {
        // Deliberately not refused (ADR-0144 §4): every engine computes its own addresses, so an
        // offset the type's alignment does not divide is slow rather than wrong. Asserted so that
        // adding a refusal later is a visible decision rather than a silent one.
        let mut pool = Pool::new();
        let interner = Interner::new();
        let ty = placed_struct(
            &mut pool,
            &interner,
            14,
            &[
                ("pad", PoolId::U8, None, None),
                ("value", PoolId::S64, None, Some(3)),
            ],
        );
        assert_eq!(field_offset(&pool, T, ty, 1).map(|(at, _)| at), Ok(3));
        assert_eq!(
            layout_of(&pool, T, ty),
            Ok(Layout { size: 16, align: 8 }),
            "the end is 11, rounded up to the struct's own alignment"
        );
    }

    #[test]
    fn a_struct_with_no_attributes_lays_out_exactly_as_before() {
        // The control, and the claim the unchanged MIR snapshots stand for: the rewritten fold is
        // byte-for-byte the old one when nothing asks for anything.
        let mut pool = Pool::new();
        let interner = Interner::new();
        let ty = placed_struct(
            &mut pool,
            &interner,
            15,
            &[
                ("a", PoolId::U8, None, None),
                ("b", PoolId::S64, None, None),
                ("c", PoolId::U8, None, None),
            ],
        );
        assert_eq!(layout_of(&pool, T, ty), Ok(Layout { size: 24, align: 8 }));
        assert_eq!(field_offset(&pool, T, ty, 0).map(|(at, _)| at), Ok(0));
        assert_eq!(field_offset(&pool, T, ty, 1).map(|(at, _)| at), Ok(8));
        assert_eq!(field_offset(&pool, T, ty, 2).map(|(at, _)| at), Ok(16));
    }
}
