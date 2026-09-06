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
//! **The rules differ by target architecture**, which this module did not model until the x86-64 Linux CI
//! leg was read (ADR-0206). [`CAbi`] selects them:
//!
//! * **AAPCS64** (arm64) — a small integer aggregate in up to two general-purpose registers, or a
//!   *homogeneous floating aggregate* (HFA) of up to four members of one float type in up to four
//!   floating-point registers, **with no size limit**: a four-`float64` `CGRect` is thirty-two bytes and
//!   still travels in four registers.
//! * **System V** (x86-64) — **no HFA concept at all.** Classification is per *eightbyte*, and anything
//!   larger than sixteen bytes is `MEMORY`: passed by value on the stack, returned through a hidden
//!   pointer. Within sixteen bytes an eightbyte is SSE when everything in it is floating-point and
//!   INTEGER otherwise.
//!
//! # The claim this module used to make, and why it was wrong
//!
//! Its "Why an HFA is not size-limited" section said a `CGRect` travels in four floating-point registers
//! on **both** targets. **That is true of AAPCS64 and false of System V**, which has no homogeneous-aggregate
//! rule to be unlimited: sixteen bytes is the whole of it. So a 32-byte `Rect` at a `#foreign` boundary was
//! passed in four SSE registers on x86-64 where C expects it on the stack — **a silent miscompile**, with no
//! diagnostic, invisible on the arm64 machine every wave of this project has been developed on.
//!
//! It was found by `aggregates_cross_a_foreign_boundary_as_a_c_compiler_expects`, which links against a
//! `cc`-compiled shim precisely so a wrong answer cannot be self-consistent, once that test was finally run
//! on Linux. **Seventh in this project's family of hand-maintained claims with nothing enforcing them** — and
//! the most expensive, because the other six produced a diagnostic and this one produced wrong data.
//!
//! # Why `Stack` is a class and `Refused` is a different one
//!
//! ADR-0160 had one `Memory` variant meaning "this compiler will not pass it", and argued the case had to
//! stay refused because it covered *two* shapes with different right answers: a large composite, where an
//! indirect pass is correct, and a small mixed one, where the two ABIs disagree about which register file
//! each field uses. **The Linux run is what splits it**, exactly as that ADR said it would:
//!
//! * [`Class::Stack`] is System V's `MEMORY` for something over sixteen bytes. It has **one** correct
//!   answer, Cranelift models it as [`StructArgument`](https://docs.rs/cranelift-codegen), and it is now
//!   implemented rather than refused.
//! * [`Class::Refused`] is what is left: a mixed aggregate inside sixteen bytes, and a float aggregate whose
//!   members share an eightbyte. Both need a per-eightbyte register assignment this compiler's `Class`
//!   vocabulary cannot express, and an honest narrower rule beats a wrong wider one — the judgement
//!   ADR-0112 made about `sqrt` and ADR-0160 made here first.
//!
//! # Why mixed aggregates are refused
//!
//! System V on x86-64 classifies each eightbyte *independently* — `struct { double a; long b; }` puts `a` in
//! `xmm0` and `b` in `rdi`, in that order, interleaving two register files. AAPCS64 does not: the same struct
//! is not an HFA, is 16 bytes, and goes in `x0`/`x1`. So the two targets genuinely disagree about where a
//! mixed struct's fields live, and getting that right means implementing both classifications in full.
//!
use crate::item::Item;
use crate::layout::{Layout, TargetLayout, layout_of};
use crate::pool::Pool;
use crate::{FloatKind, PoolId};

/// Which platform's aggregate rules apply.
///
/// Separate from [`TargetLayout`] deliberately: that describes pointer *width*, which is the same LP64 on
/// both targets, and threading an architecture through `layout_of` would suggest a dependency the layout
/// computation does not have. Classification is the only thing that needs to know, so it is the only thing
/// asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CAbi {
    /// AAPCS64 — arm64. Homogeneous floating aggregates of up to four members, with no size limit.
    Aapcs64,
    /// System V — x86-64. Per-eightbyte classification, and `MEMORY` past sixteen bytes.
    SysV,
}

impl CAbi {
    /// The rules for the machine this compiler is running on.
    ///
    /// **The host's, because the target is the host.** This project cross-compiles nowhere: there is no
    /// `--target`, and `jr-link` shells out to the host `cc` (ADR-0019 §2). The day a target flag exists,
    /// this is the call site that must take it as an argument instead — which is why it is a named
    /// constructor rather than a `Default`, so a reader can find every place that assumes the host.
    ///
    /// # Panics
    /// Never. An architecture this compiler has no rules for is a compile error rather than a run-time one:
    /// the `cfg` below has no fallback arm, so a third target fails to build here instead of silently
    /// inheriting a convention that does not apply to it. That is the same reasoning as the exhaustive-match
    /// rule in `AGENTS.md`, applied to a platform instead of an enum.
    #[must_use]
    pub const fn host() -> Self {
        #[cfg(target_arch = "aarch64")]
        {
            Self::Aapcs64
        }
        #[cfg(target_arch = "x86_64")]
        {
            Self::SysV
        }
    }
}

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
    /// By value **on the stack** — System V's `MEMORY` class for an aggregate over sixteen bytes.
    ///
    /// The half of ADR-0160's `Memory` that has exactly one correct answer, split out once the x86-64 run
    /// existed to justify it (ADR-0206). A caller passes the aggregate's address and the ABI implementation
    /// copies the bytes: Cranelift models this as `ArgumentPurpose::StructArgument`, and a return of this
    /// class uses the struct-return pointer this compiler already emits for its own aggregate returns.
    ///
    /// **Never produced for AAPCS64.** A large non-HFA composite is passed by reference there, which is a
    /// *different* convention — and Cranelift's arm64 back end rejects `StructArgument` outright, so a
    /// classification that produced this on arm64 would be a panic rather than a wrong answer.
    Stack {
        /// The size in bytes, rounded up to a whole number of eightbytes.
        ///
        /// Rounded here rather than at the call site because Cranelift asserts `size % 8 == 0` when it
        /// lowers a `StructArgument`, and an assertion inside a dependency is a worse diagnostic than a
        /// field whose type says it is already aligned.
        size: u32,
    },
    /// Not classified by this compiler, and refused with a diagnostic.
    ///
    /// What is left of ADR-0160's `Memory` after [`Class::Stack`] took the decidable half: a mixed aggregate
    /// **inside** sixteen bytes, where System V interleaves two register files and AAPCS64 does not, and a
    /// float aggregate whose members *share* an eightbyte — `struct { float a; float b; }` is one SSE
    /// eightbyte holding both, which this `Class` cannot express without a packed vector type.
    ///
    /// Refused rather than approximated: a struct in the wrong register is a silent wrong answer, which is
    /// the failure this module exists to prevent and the one it committed for eleven waves on x86-64.
    Refused,
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
    abi: CAbi,
    ty: PoolId,
) -> Result<Option<Class>, crate::layout::LayoutError> {
    if !is_aggregate(pool, ty) {
        return Ok(None);
    }
    let layout = layout_of(pool, target, ty)?;
    let mut scalars = Vec::new();
    let flattened = flatten(pool, ty, &mut scalars);
    // **The size rule comes before the flatten result on System V**, because `MEMORY` does not care what is
    // inside: a union, a forty-byte struct and a five-member float aggregate all go to the stack there, and
    // `flatten` deliberately gives up on all three (it bails past five scalars and on any overlapping type).
    // Asking "did it flatten" first would refuse shapes whose answer is not in doubt.
    if abi == CAbi::SysV && layout.size > 16 {
        return Ok(Some(Class::Stack {
            size: eightbytes(layout.size),
        }));
    }
    if !flattened {
        return Ok(Some(Class::Refused));
    }
    Ok(Some(classify_flattened(&scalars, layout, target, abi)))
}

/// `size` rounded up to a whole number of eightbytes, as a `u32`.
///
/// Saturating rather than wrapping: an aggregate larger than four gibibytes cannot be passed by any
/// convention, and a wrapped size would describe a *small* stack copy for a huge struct — which corrupts the
/// stack silently instead of failing.
fn eightbytes(size: u64) -> u32 {
    u32::try_from(size.div_ceil(8).saturating_mul(8)).unwrap_or(u32::MAX)
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

/// The class a flattened scalar list implies, under `abi`'s rules.
fn classify_flattened(
    scalars: &[Scalar],
    layout: Layout,
    target: TargetLayout,
    abi: CAbi,
) -> Class {
    if scalars.is_empty() || scalars.len() > 4 {
        return Class::Refused;
    }
    if let Scalar::Float(kind) = scalars[0]
        && scalars.iter().all(|s| *s == Scalar::Float(kind))
    {
        let count = u32::try_from(scalars.len()).unwrap_or(0);
        return match abi {
            // **AAPCS64's HFA, with no size limit**: a four-`float64` `CGRect` is thirty-two bytes and
            // travels in four `v` registers. The limit is four *members*, not sixteen bytes, which is the
            // whole reason this is a classification rather than a size check.
            CAbi::Aapcs64 => Class::Float { kind, count },
            // **System V has no HFA rule.** Anything past sixteen bytes already went to the stack above, so
            // what reaches here fits — and it is in registers only when each member owns its own eightbyte.
            // A `float64` is eight bytes, so up to two of them qualify. Two `float32`s **share** one SSE
            // eightbyte and would need a packed `<2 x float>`, which `Class` cannot say: emitting two
            // separate `f32` parameters would put them in `xmm0` and `xmm1` where C reads one register.
            // That is the wrong-register failure this module exists to prevent, so it is refused.
            // In registers exactly when **every member owns its own eightbyte**, which is what makes one
            // `Class::Float` member equal one SSE register. A `float64` is eight bytes so it always does; a
            // `float32` does only when it is alone, because two of them pack into one. The size rule above
            // has already sent anything past sixteen bytes to the stack, so `count` cannot exceed two here
            // for a `float64` — the bound is restated rather than assumed, since it is load-bearing.
            CAbi::SysV if (count == 1 || kind.bits == 64) && count <= 2 => {
                Class::Float { kind, count }
            }
            CAbi::SysV => Class::Refused,
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
        // Past two words: `MEMORY` on System V, which the size rule in [`classify`] already answered, so
        // only AAPCS64 reaches here. A large non-HFA composite is passed **by reference** there — a
        // different convention from a stack copy, and one no engine implements — so it stays refused, which
        // is exactly what it was before this split.
        return Class::Refused;
    }
    Class::Refused
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
            classify(&pool, T, CAbi::Aapcs64, PoolId::S64),
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
            classify(&pool, T, CAbi::Aapcs64, pair),
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
            classify(&pool, T, CAbi::Aapcs64, mixed),
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
            classify(&pool, T, CAbi::Aapcs64, single),
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
            classify(&pool, T, CAbi::Aapcs64, three),
            Ok(Some(Class::Refused)),
            "past two words AAPCS64 passes by reference, which no engine here implements"
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
            classify(&pool, T, CAbi::Aapcs64, rect),
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
            classify(&pool, T, CAbi::Aapcs64, rect),
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
            classify(&pool, T, CAbi::Aapcs64, array),
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
            classify(&pool, T, CAbi::Aapcs64, five),
            Ok(Some(Class::Refused)),
            "an HFA holds at most four members, on the target that has HFAs at all"
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
            classify(&pool, T, CAbi::Aapcs64, mixed),
            Ok(Some(Class::Refused)),
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
        assert_eq!(
            classify(&pool, T, CAbi::Aapcs64, mixed),
            Ok(Some(Class::Refused))
        );
    }

    /// **The defect this whole ADR exists for**: System V has no HFA rule, so a thirty-two-byte aggregate
    /// is `MEMORY` even though every member is a `float64`.
    ///
    /// The AAPCS64 twin of this assertion sits ten lines above and expects four floating-point registers.
    /// The two together are the point: one classification, two targets, and a wave of silent miscompiles
    /// because only the first was ever written.
    #[test]
    fn a_four_double_aggregate_goes_to_the_stack_under_system_v() {
        let interner = Interner::new();
        let mut pool = Pool::new();
        let f64_ty = pool.intern(Item::FloatType { bits: 64 });
        let point = struct_of(&mut pool, &interner, 0, &[f64_ty, f64_ty]);
        let rect = struct_of(&mut pool, &interner, 1, &[point, point]);
        assert_eq!(
            classify(&pool, T, CAbi::SysV, rect),
            Ok(Some(Class::Stack { size: 32 })),
            "System V sends anything past sixteen bytes to memory; there is no HFA to be unlimited"
        );
    }

    /// Two `float64`s fit sixteen bytes as one eightbyte each, so System V *does* use two SSE registers.
    ///
    /// The boundary case, asserted because it is the one a reader would assume goes to the stack along with
    /// its four-member sibling. Sixteen bytes is the line, and this is on the register side of it.
    #[test]
    fn two_doubles_stay_in_registers_under_system_v() {
        let interner = Interner::new();
        let mut pool = Pool::new();
        let f64_ty = pool.intern(Item::FloatType { bits: 64 });
        let point = struct_of(&mut pool, &interner, 0, &[f64_ty, f64_ty]);
        assert_eq!(
            classify(&pool, T, CAbi::SysV, point),
            Ok(Some(Class::Float {
                kind: FloatKind { bits: 64 },
                count: 2
            })),
            "one eightbyte each, so `xmm0` and `xmm1` — the same answer AAPCS64 gives by another route"
        );
    }

    /// Two `float32`s **share** an eightbyte, which `Class` cannot express — so System V refuses.
    ///
    /// The case that makes [`Class::Refused`] still necessary after [`Class::Stack`] took the large half.
    /// Passing them as two `f32` parameters would use `xmm0` and `xmm1`; C reads both from `xmm0`. A stack
    /// copy would be wrong too — the struct is eight bytes and belongs in a register. Neither available
    /// answer is right, which is precisely when this compiler refuses.
    #[test]
    fn two_floats_sharing_an_eightbyte_are_refused_under_system_v() {
        let interner = Interner::new();
        let mut pool = Pool::new();
        let f32_ty = pool.intern(Item::FloatType { bits: 32 });
        let pair = struct_of(&mut pool, &interner, 0, &[f32_ty, f32_ty]);
        assert_eq!(
            classify(&pool, T, CAbi::SysV, pair),
            Ok(Some(Class::Refused)),
            "both floats live in `xmm0`; two `f32` parameters would use two registers"
        );
        assert_eq!(
            classify(&pool, T, CAbi::Aapcs64, pair),
            Ok(Some(Class::Float {
                kind: FloatKind { bits: 32 },
                count: 2
            })),
            "AAPCS64 gives each member its own `s` register, so the same struct is fine there"
        );
    }

    /// A single `float32` owns its eightbyte, so it is in a register on both.
    #[test]
    fn one_float_is_a_register_under_both_abis() {
        let interner = Interner::new();
        let mut pool = Pool::new();
        let f32_ty = pool.intern(Item::FloatType { bits: 32 });
        let one = struct_of(&mut pool, &interner, 0, &[f32_ty]);
        for abi in [CAbi::Aapcs64, CAbi::SysV] {
            assert_eq!(
                classify(&pool, T, abi, one),
                Ok(Some(Class::Float {
                    kind: FloatKind { bits: 32 },
                    count: 1
                })),
                "a lone float is unambiguous: {abi:?}"
            );
        }
    }

    /// Three words are twenty-four bytes: the stack under System V, refused under AAPCS64.
    ///
    /// The all-integer counterpart to the `Rect` case, and it shows the split is about *size* rather than
    /// about floats — the shape ADR-0160's single `Memory` variant could not distinguish.
    #[test]
    fn three_words_go_to_the_stack_under_system_v() {
        let interner = Interner::new();
        let mut pool = Pool::new();
        let s64 = pool.intern(Item::IntType {
            bits: 64,
            signed: true,
        });
        let three = struct_of(&mut pool, &interner, 0, &[s64, s64, s64]);
        assert_eq!(
            classify(&pool, T, CAbi::SysV, three),
            Ok(Some(Class::Stack { size: 24 }))
        );
        assert_eq!(
            classify(&pool, T, CAbi::Aapcs64, three),
            Ok(Some(Class::Refused)),
            "AAPCS64 passes it by reference, a convention no engine here implements"
        );
    }

    /// A size that is not a whole number of eightbytes is rounded up, because Cranelift asserts on it.
    ///
    /// `{ s64, s64, u8 }` is seventeen bytes of payload and twenty-four after padding — but a shape that
    /// padded to, say, twenty would still have to be described as twenty-four, and getting that wrong is an
    /// assertion inside a dependency rather than a diagnostic of ours.
    #[test]
    fn a_stack_size_is_a_whole_number_of_eightbytes() {
        assert_eq!(eightbytes(17), 24);
        assert_eq!(eightbytes(24), 24);
        assert_eq!(eightbytes(1), 8);
        assert_eq!(eightbytes(0), 0);
    }

    /// A pointer is a word, which is what makes a `{ char*, long }` shape passable.
    #[test]
    fn a_pointer_counts_as_a_word() {
        let interner = Interner::new();
        let mut pool = Pool::new();
        let ptr = pool.pointer_to(PoolId::U8);
        let slice = struct_of(&mut pool, &interner, 0, &[ptr, PoolId::S64]);
        assert_eq!(
            classify(&pool, T, CAbi::Aapcs64, slice),
            Ok(Some(Class::Integer { words: 2 }))
        );
    }

    /// A `string` is that same two-word shape and must classify identically, because a C function taking a
    /// `{ char*, long }` by value is a real signature a caller would reach for `string` to describe.
    #[test]
    fn a_string_is_two_words() {
        let pool = Pool::new();
        assert_eq!(
            classify(&pool, T, CAbi::Aapcs64, PoolId::STRING),
            Ok(Some(Class::Integer { words: 2 }))
        );
    }
}
