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
}

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
// string
// ---------------------------------------------------------------------------

/// The offset and layout of `string`'s `.data` field (ADR-0004).
///
/// First field, so offset 0, and a `*u8`.
#[must_use]
pub const fn string_data(target: TargetLayout) -> (u64, Layout) {
    (
        0,
        Layout {
            size: target.pointer_size as u64,
            align: target.pointer_align,
        },
    )
}

/// The offset and layout of `string`'s `.count` field (ADR-0004).
///
/// An `s64` placed after `.data`, at the next 8-aligned offset.
#[must_use]
pub const fn string_count(target: TargetLayout) -> (u64, Layout) {
    let count = Layout::scalar(8);
    (align_up(target.pointer_size as u64, count.align), count)
}

/// The layout of `string` itself.
#[must_use]
pub const fn string_layout(target: TargetLayout) -> Layout {
    let (offset, count) = string_count(target);
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

        Item::StringType => Ok(string_layout(target)),

        // A procedure used as a value is a code pointer (ADR-0012), so it is
        // pointer-shaped. `Callee::Indirect` is what consumes this.
        Item::PointerType(_) | Item::ProcType { .. } => Ok(Layout {
            size: u64::from(target.pointer_size),
            align: target.pointer_align,
        }),

        Item::TypeType | Item::ForeignLibraryType => Err(LayoutError::ComptimeOnly(ty)),

        Item::StructType { decl } => struct_layout_at_depth(pool, target, *decl, depth),

        Item::VoidValue
        | Item::BoolValue(_)
        | Item::IntValue { .. }
        | Item::StrValue(_)
        | Item::TypeValue(_)
        | Item::ProcValue { .. }
        | Item::ForeignLibraryValue(_) => Err(LayoutError::NotAType(ty)),
    }
}

fn struct_layout_at_depth(
    pool: &Pool,
    target: TargetLayout,
    decl: DeclId,
    depth: u32,
) -> Result<Layout, LayoutError> {
    if depth >= MAX_DEPTH {
        return Err(LayoutError::Recursive(decl));
    }
    let fields = pool
        .struct_fields(decl)
        .ok_or(LayoutError::UnresolvedStruct(decl))?;

    let mut size = 0u64;
    let mut align = 1u32;
    for field in fields {
        let field_layout = layout_at_depth(pool, target, field.ty, depth + 1)?;
        size = align_up(size, field_layout.align) + field_layout.size;
        align = align.max(field_layout.align);
    }
    // Rounding the total up to the struct's own alignment is what makes an array
    // of it aligned at every element, which is why it is not merely the offset
    // past the last field.
    Ok(Layout {
        size: align_up(size, align),
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
    let Item::StructType { decl } = pool.item(ty) else {
        return Err(LayoutError::NotAType(ty));
    };
    let fields = pool
        .struct_fields(*decl)
        .ok_or(LayoutError::UnresolvedStruct(*decl))?;

    let mut offset = 0u64;
    for (position, field) in fields.iter().enumerate() {
        let field_layout = layout_of(pool, target, field.ty)?;
        offset = align_up(offset, field_layout.align);
        if position == index as usize {
            return Ok((offset, field_layout));
        }
        offset += field_layout.size;
    }
    Err(LayoutError::NotAType(ty))
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
}
