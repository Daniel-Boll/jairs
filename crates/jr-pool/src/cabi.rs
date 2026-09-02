//! The C ABI classification for an aggregate crossing a `#foreign` boundary (ADR-0160).
//!
//! # Why this lives in `jr-pool` rather than in a back end
//!
//! Three engines cross this boundary — the comptime VM through libffi, Cranelift, and LLVM — and each of them
//! *could* classify a struct itself. That is exactly what must not happen. A struct in the wrong register is a
//! silent wrong answer with no diagnostic, and three implementations of the same platform rules would give
//! three chances to disagree; ADR-0020 §2 made the same argument about trap messages, and it applies with far
//! more force here, because a mis-rendered message is visible and a mis-placed argument is not.
//!
//! So the rules live once, beside the layout computation they depend on, and each engine *asks*.
//!
//! # What is classified, and what is refused
//!
//! Two shapes are supported, and they are the two that the platform rules make decidable without ambiguity:
//!
//! * **A small integer aggregate** — every scalar in it is an integer, a pointer or a `bool`, and the whole
//!   thing is at most two words. Passed and returned in up to two general-purpose registers.
//! * **A homogeneous float aggregate** (HFA) — every scalar in it is the *same* float type, and there are at
//!   most four of them. Passed and returned in up to four floating-point registers.
//!
//! Everything else is [`Class::Memory`], which the caller passes by address: a struct larger than two words
//! that is not an HFA, or one that mixes integers and floats. That is not a refusal — an indirect pass is a
//! *correct* convention on both supported targets for a large composite — but it is a **narrower** claim than
//! "we implement the C ABI", and the difference is stated rather than glossed.
//!
//! # Why mixed aggregates are `Memory` rather than classified
//!
//! System V on x86-64 classifies each eightbyte *independently* — `struct { double a; long b; }` puts `a` in
//! `xmm0` and `b` in `rdi`, in that order, interleaving two register files. AAPCS64 does not: the same struct
//! is not an HFA, is 16 bytes, and goes in `x0`/`x1`. So the two targets genuinely disagree about where a
//! mixed struct's fields live, and getting that right means implementing both classifications in full.
//!
//! Sending it to memory instead is correct on **neither** target for a 16-byte mixed struct, which is why
//! `Memory` here means "this compiler will not pass it" rather than "pass it indirectly": see
//! [`Class::Memory`]'s own docs. The refusal stays, with a message that names the two supported shapes — an
//! honest narrower rule beats a wrong wider one, and this is the same judgement ADR-0112 made about `sqrt`.
//!
//! # Why an HFA is not size-limited
//!
//! A `CGRect` is `{ CGPoint origin; CGSize size; }` — four `float64`s, thirty-two bytes, and an HFA. Both
//! AAPCS64 and System V pass it in four floating-point registers, and a size test would send it to memory
//! and break every graphics call W10 needs. The limit is **four scalars**, not sixteen bytes, and the
//! distinction is the whole reason this classification is worth having rather than a size check.

use crate::item::Item;
use crate::layout::{Layout, TargetLayout, layout_of};
use crate::pool::Pool;
use crate::{FloatKind, PoolId};

/// How an aggregate crosses a C boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Class {
    /// In up to two general-purpose registers, as `words.len()` machine words.
    ///
    /// Each word is a full register even when the struct's tail is shorter: `struct { s64 a; u8 b; }` is
    /// sixteen bytes after padding and occupies two registers, the second holding one meaningful byte. That
    /// is what both ABIs specify, and it is why the caller loads whole words from a padded slot rather than
    /// computing per-field register assignments.
    Integer {
        /// How many words, one or two.
        words: u32,
    },
    /// In up to four floating-point registers, one per member.
    Float {
        /// The member type — every member has this type, which is what "homogeneous" means.
        kind: FloatKind,
        /// How many members, one to four.
        count: u32,
    },
    /// Not classified by this compiler.
    ///
    /// **Refused rather than passed indirectly.** An indirect pass is the correct convention for a *large*
    /// composite, but this case also covers a small mixed one — where both supported targets pass in
    /// registers and disagree about which — so treating the whole case as "pass a pointer" would be wrong
    /// for the small half and right for the large half. One case with two correct answers is a case that has
    /// to be refused until it is split, and the diagnostic names what *is* supported so a caller can reshape.
    Memory,
}

/// How the aggregate `ty` crosses a C boundary on `target`.
///
/// `None` when `ty` is not an aggregate at all — a scalar needs no classification, and returning a `Class`
/// for one would invite a caller to route scalars through this at the cost of the clarity that makes the
/// aggregate path checkable.
///
/// # Errors
/// Propagates [`crate::layout::LayoutError`] when the type has no runtime layout, because a type whose size
/// is unknown cannot be classified and guessing is the failure this module exists to prevent.
pub fn classify(
    pool: &Pool,
    target: TargetLayout,
    ty: PoolId,
) -> Result<Option<Class>, crate::layout::LayoutError> {
    if !is_aggregate(pool, ty) {
        return Ok(None);
    }
    let layout = layout_of(pool, target, ty)?;
    let mut scalars = Vec::new();
    if !flatten(pool, ty, &mut scalars) {
        return Ok(Some(Class::Memory));
    }
    Ok(Some(classify_flattened(&scalars, layout, target)))
}

/// One scalar found inside an aggregate, for classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scalar {
    /// An integer, a pointer or a `bool` — anything that lives in a general-purpose register.
    Word,
    /// A float of this kind.
    Float(FloatKind),
}

/// Whether `ty` is an aggregate — the same question [`Repr`](crate) asks, answered here so this module does
/// not depend on a back end's view of it.
///
/// A `string` counts: it is two words and crosses as a pointer today (ADR-0004), and a C function taking a
/// `{char*, long}` by value is a real shape. A view and a dynamic array likewise.
fn is_aggregate(pool: &Pool, ty: PoolId) -> bool {
    matches!(
        pool.item(ty),
        Item::StructType { .. }
            | Item::UnionType { .. }
            | Item::VariantType { .. }
            | Item::ArrayType { .. }
            | Item::ViewType { .. }
            | Item::DynamicArrayType { .. }
            | Item::StringType
    )
}

/// Flattens `ty`'s scalars into `out`, returning `false` when it cannot be flattened.
///
/// Takes no `TargetLayout`: a scalar's *kind* — word or float — does not depend on the target, and the only
/// target-dependent question (how many words a size occupies) is asked in [`classify_flattened`] where the
/// layout is already in hand. Threading it here would suggest a dependency that does not exist.
///
/// Returns `false` for a **union** or a **variant**: their members overlap, so there is no single sequence of
/// scalars, and every C ABI classifies a union by treating its bytes as opaque — which is
/// [`Class::Memory`]'s territory. Returns `false` past five scalars too, since no supported class holds more
/// and continuing would walk a large array for nothing.
fn flatten(pool: &Pool, ty: PoolId, out: &mut Vec<Scalar>) -> bool {
    if out.len() > 4 {
        return false;
    }
    match pool.item(ty) {
        Item::IntType { .. } | Item::BoolType | Item::PointerType(_) | Item::ProcType { .. } => {
            out.push(Scalar::Word);
            true
        }
        Item::FloatType { bits } => {
            out.push(Scalar::Float(FloatKind { bits: *bits }));
            true
        }
        Item::StructType { .. } => {
            // `fields_of` rather than a field on the `Item`: a parameterised struct's fields live in an
            // instance side table (ADR-0085 §2), and this is the one accessor that answers for both.
            let Some(fields) = pool.fields_of(ty) else {
                return false;
            };
            let types: Vec<PoolId> = fields.iter().map(|field| field.ty).collect();
            types.iter().all(|field| flatten(pool, *field, out))
        }
        Item::ArrayType { elem, len } => {
            // An array of four floats *is* an HFA — `float64[4]` and `struct { double a, b, c, d; }` are the
            // same thing to both ABIs, so the array has to flatten rather than being refused for being an
            // array. Bounded by the same five-scalar cut-off above.
            if *len > 4 {
                return false;
            }
            for _ in 0..*len {
                if !flatten(pool, *elem, out) {
                    return false;
                }
            }
            true
        }
        // A `string`, a view and a dynamic array are compiler-defined aggregates of words. Flattened
        // explicitly rather than by walking a field list, because they have no `Item` fields to walk.
        Item::StringType | Item::ViewType { .. } => {
            out.push(Scalar::Word);
            out.push(Scalar::Word);
            true
        }
        _ => false,
    }
}

/// The class a flattened scalar list implies.
fn classify_flattened(scalars: &[Scalar], layout: Layout, target: TargetLayout) -> Class {
    if scalars.is_empty() || scalars.len() > 4 {
        return Class::Memory;
    }
    // **HFA first**, because it is the case a size test would get wrong: a four-`double` `CGRect` is
    // thirty-two bytes and still travels in registers.
    if let Scalar::Float(kind) = scalars[0]
        && scalars.iter().all(|s| *s == Scalar::Float(kind))
    {
        return Class::Float {
            kind,
            count: u32::try_from(scalars.len()).unwrap_or(0),
        };
    }
    if scalars.iter().all(|s| *s == Scalar::Word) {
        let word = u64::from(target.pointer_size);
        let words = layout.size.div_ceil(word);
        if words <= 2 {
            // `max(1)` so a zero-sized aggregate — a struct with no fields — still occupies one register
            // rather than none. An empty struct is not passed at all in C++, and C has no empty struct, so
            // there is no convention to match; one word is the shape that cannot corrupt a later argument.
            return Class::Integer {
                words: u32::try_from(words.max(1)).unwrap_or(1),
            };
        }
        return Class::Memory;
    }
    Class::Memory
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::{DeclId, Field};
    use crate::pool::Pool;
    use jr_base::{FileId, Interner};

    const T: TargetLayout = TargetLayout::LP64;

    /// A struct type at declaration `index` whose fields are `types`.
    ///
    /// `index` distinguishes two structs in one test, since a `DeclId` is the struct's identity — nesting one
    /// inside another needs them to be different declarations, which is what caught the flattening bug this
    /// helper's `index` parameter exists for.
    fn struct_of(pool: &mut Pool, interner: &Interner, index: u32, types: &[PoolId]) -> PoolId {
        let decl = DeclId::new(FileId::from_usize(0), index);
        let ty = pool.struct_type(decl);
        let fields: Vec<Field> = types
            .iter()
            .enumerate()
            .map(|(at, field)| Field::new(interner.intern(&format!("f{at}")), *field))
            .collect();
        pool.set_struct_fields(decl, fields);
        ty
    }

    #[test]
    fn a_scalar_has_no_class() {
        let pool = Pool::new();
        assert_eq!(
            classify(&pool, T, PoolId::S64),
            Ok(None),
            "a scalar needs no classification, and answering one would invite routing scalars here"
        );
    }

    #[test]
    fn two_words_go_in_two_integer_registers() {
        let interner = Interner::new();
        let mut pool = Pool::new();
        let pair = struct_of(&mut pool, &interner, 0, &[PoolId::S64, PoolId::S64]);
        assert_eq!(
            classify(&pool, T, pair),
            Ok(Some(Class::Integer { words: 2 })),
            "`{{ s64, s64 }}` is sixteen bytes of words: two registers on both targets"
        );
    }

    /// The padded case, which a per-field register assignment would get wrong: the second register holds one
    /// meaningful byte and is still a whole register.
    #[test]
    fn a_padded_tail_still_occupies_a_whole_register() {
        let interner = Interner::new();
        let mut pool = Pool::new();
        let mixed = struct_of(&mut pool, &interner, 0, &[PoolId::S64, PoolId::U8]);
        assert_eq!(
            classify(&pool, T, mixed),
            Ok(Some(Class::Integer { words: 2 })),
            "sixteen bytes after padding is two registers, not one and a byte"
        );
    }

    #[test]
    fn one_word_goes_in_one_register() {
        let interner = Interner::new();
        let mut pool = Pool::new();
        let single = struct_of(&mut pool, &interner, 0, &[PoolId::S64]);
        assert_eq!(
            classify(&pool, T, single),
            Ok(Some(Class::Integer { words: 1 }))
        );
    }

    #[test]
    fn three_words_are_memory() {
        let interner = Interner::new();
        let mut pool = Pool::new();
        let three = struct_of(
            &mut pool,
            &interner,
            0,
            &[PoolId::S64, PoolId::S64, PoolId::S64],
        );
        assert_eq!(
            classify(&pool, T, three),
            Ok(Some(Class::Memory)),
            "past two words there is no register class this compiler implements"
        );
    }

    /// The case a size test gets wrong, and the reason this module exists rather than a byte count: a
    /// four-`float64` aggregate is thirty-two bytes and travels in four registers.
    #[test]
    fn a_four_double_aggregate_is_an_hfa_despite_being_thirty_two_bytes() {
        let interner = Interner::new();
        let mut pool = Pool::new();
        let f64_ty = pool.intern(Item::FloatType { bits: 64 });
        let rect = struct_of(&mut pool, &interner, 0, &[f64_ty, f64_ty, f64_ty, f64_ty]);
        assert_eq!(
            classify(&pool, T, rect),
            Ok(Some(Class::Float {
                kind: FloatKind::F64,
                count: 4
            })),
            "a `CGRect` is an HFA of four doubles; a size test would send it to memory"
        );
    }

    /// Nesting must not change the answer: a `CGRect` is two `CGPoint`s in the real headers.
    #[test]
    fn a_nested_float_aggregate_flattens_to_the_same_hfa() {
        let interner = Interner::new();
        let mut pool = Pool::new();
        let f64_ty = pool.intern(Item::FloatType { bits: 64 });
        let point = struct_of(&mut pool, &interner, 0, &[f64_ty, f64_ty]);
        let rect = struct_of(&mut pool, &interner, 1, &[point, point]);
        assert_eq!(
            classify(&pool, T, rect),
            Ok(Some(Class::Float {
                kind: FloatKind::F64,
                count: 4
            })),
            "`{{ CGPoint, CGPoint }}` and four bare doubles are the same thing to both ABIs"
        );
    }

    #[test]
    fn an_array_of_floats_is_an_hfa_too() {
        let interner = Interner::new();
        let mut pool = Pool::new();
        let _ = &interner;
        let f32_ty = pool.intern(Item::FloatType { bits: 32 });
        let array = pool.intern(Item::ArrayType {
            elem: f32_ty,
            len: 3,
        });
        assert_eq!(
            classify(&pool, T, array),
            Ok(Some(Class::Float {
                kind: FloatKind::F32,
                count: 3
            })),
            "`float32[3]` and a three-float struct are indistinguishable to a C ABI"
        );
    }

    #[test]
    fn five_floats_are_memory() {
        let interner = Interner::new();
        let mut pool = Pool::new();
        let f64_ty = pool.intern(Item::FloatType { bits: 64 });
        let five = struct_of(
            &mut pool,
            &interner,
            0,
            &[f64_ty, f64_ty, f64_ty, f64_ty, f64_ty],
        );
        assert_eq!(
            classify(&pool, T, five),
            Ok(Some(Class::Memory)),
            "an HFA holds at most four members"
        );
    }

    /// Mixed float and integer is where the two targets genuinely disagree, so it is refused rather than
    /// guessed — the decision this module's docs argue for.
    #[test]
    fn a_mixed_aggregate_is_memory() {
        let interner = Interner::new();
        let mut pool = Pool::new();
        let f64_ty = pool.intern(Item::FloatType { bits: 64 });
        let mixed = struct_of(&mut pool, &interner, 0, &[f64_ty, PoolId::S64]);
        assert_eq!(
            classify(&pool, T, mixed),
            Ok(Some(Class::Memory)),
            "System V splits this across two register files and AAPCS64 does not"
        );
    }

    /// Two float widths are not homogeneous, which is what the word means and what a lenient implementation
    /// would get wrong by taking the first member's width for all of them.
    #[test]
    fn two_float_widths_are_not_homogeneous() {
        let interner = Interner::new();
        let mut pool = Pool::new();
        let f32_ty = pool.intern(Item::FloatType { bits: 32 });
        let f64_ty = pool.intern(Item::FloatType { bits: 64 });
        let mixed = struct_of(&mut pool, &interner, 0, &[f32_ty, f64_ty]);
        assert_eq!(classify(&pool, T, mixed), Ok(Some(Class::Memory)));
    }

    /// A pointer is a word, which is what makes a `{ char*, long }` shape passable.
    #[test]
    fn a_pointer_counts_as_a_word() {
        let interner = Interner::new();
        let mut pool = Pool::new();
        let ptr = pool.pointer_to(PoolId::U8);
        let slice = struct_of(&mut pool, &interner, 0, &[ptr, PoolId::S64]);
        assert_eq!(
            classify(&pool, T, slice),
            Ok(Some(Class::Integer { words: 2 }))
        );
    }

    /// A `string` is that same two-word shape and must classify identically, because a C function taking a
    /// `{ char*, long }` by value is a real signature a caller would reach for `string` to describe.
    #[test]
    fn a_string_is_two_words() {
        let pool = Pool::new();
        assert_eq!(
            classify(&pool, T, PoolId::STRING),
            Ok(Some(Class::Integer { words: 2 }))
        );
    }
}
