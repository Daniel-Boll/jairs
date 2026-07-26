//! The pool's identity types and the [`Item`] key.
//!
//! # One index space for types *and* values
//!
//! [`PoolId`] indexes types and compile-time values alike. This is deliberate,
//! and it is forced by two decisions taken elsewhere: ADR-0005 keys polymorph
//! instantiation on the tuple of interned comptime-argument IDs, and wave W4
//! makes `Type` a first-class comptime value. A type argument therefore *is* an
//! interned value, and the two cannot live in separate index spaces without a
//! conversion that would have to be total and unchecked anyway.
//!
//! The cost is real and worth naming: `jr-base`'s newtype-index convention
//! advertises that it "makes it a compile error to use a `TypeId` where a
//! `ValueId` was meant", and a single `PoolId` gives that up. We buy uniform
//! keying with it. [`Pool::is_type`](crate::Pool::is_type) is the runtime check
//! that replaces the lost compile-time one.
//!
//! # Why `Item` is an ordinary enum
//!
//! Zig's `InternPool` encodes every entry as an 8-bit tag plus a `u32` payload
//! that is either inline or an offset into an untyped `extra: []u32` soup. That
//! buys 4-byte handles, cache-friendly tag scans, and a `memcpy`-serialisable
//! pool. It was considered and not copied: in Rust it costs match exhaustiveness
//! and derived `Hash`/`Eq`, replacing them with a hand-written encoder and
//! decoder per variant — and Zig's own experience is that the decode function
//! (`indexToKey`) becomes a profile hotspot. We have no measurement saying the
//! enum's width hurts, so we follow the house convention already argued for in
//! `jr-hir`'s HIR arenas: a `Vec<Item>` indexed by a newtype ID.

use jr_base::{FileId, Symbol};

jr_base::newtype_index! {
    /// A type or compile-time value interned in the [`Pool`](crate::Pool).
    ///
    /// Equality of `PoolId`s *is* equality of the things they name, which is the
    /// whole point of interning: comparing two types is a 32-bit integer
    /// compare, not a structural walk.
    pub struct PoolId;
}

jr_base::newtype_index! {
    /// An interned string *value*.
    ///
    /// Distinct from [`jr_base::Symbol`], which interns *identifiers*. String
    /// literal payloads are data, not names: they are arbitrarily large, and
    /// they are never resolved back for display in a diagnostic headline.
    /// Keeping them out of the global symbol table keeps that table about
    /// names.
    pub struct StrId;
}

// ---------------------------------------------------------------------------
// Declaration identity
// ---------------------------------------------------------------------------

/// The identity of a declaration site, used to key *nominal* types.
///
/// Per ADR-0015, struct types are nominal: two separately-declared structs with
/// identical field lists are different types. A nominal type is therefore keyed
/// on where it was declared rather than on its shape, and this is that "where".
///
/// It is deliberately opaque to the pool. The caller (`jr-sema`) builds one from
/// a [`FileId`] and the declaration's index within that file, which keeps
/// `jr-pool` independent of `jr-hir` — the pool does not need to know that a
/// struct declaration is spelled `StructId` upstream.
///
/// # Stability
///
/// ADR-0015 records the cost this imposes: a file-plus-index identity moves when
/// a declaration is inserted above it, and an unstable declaration identity does
/// not fail loudly — it silently splits one type in two, or merges two into one.
/// The slice tolerates this because it re-analyses whole files. Making it stable
/// under unrelated edits is what the deferred `AstIdMap` (ADR-0013) is for.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeclId {
    /// The file the declaration appears in.
    pub file: FileId,
    /// The declaration's index within that file.
    pub index: u32,
}

impl DeclId {
    /// Creates a declaration identity.
    #[must_use]
    pub const fn new(file: FileId, index: u32) -> Self {
        Self { file, index }
    }
}

impl core::fmt::Debug for DeclId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "DeclId({}:{})", self.file.index(), self.index)
    }
}

// ---------------------------------------------------------------------------
// Procedure type components
// ---------------------------------------------------------------------------

/// Whether a procedure receives the implicit `context` parameter.
///
/// This is **part of a procedure type's identity**, not a decoration on it:
/// ADR-0001 requires that the type system be able to distinguish a
/// context-taking procedure from a `#c_call` one, so that a function pointer of
/// one kind can never be used where the other is wanted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ContextKind {
    /// An ordinary Jairs procedure, which receives the implicit context as a
    /// hidden trailing parameter (ADR-0001).
    Jairs,
    /// A `#c_call` procedure, which does not. Every `#foreign` procedure is
    /// implicitly `#c_call`.
    CCall,
}

/// The effect row of a procedure type. Inert.
///
/// This is defined now (in the vertical slice) even though there is no effects
/// system, because ADR-0008 requires the slot to exist from the start: adding an
/// effects system later would otherwise mean re-typing every signature in the
/// compiler. It is a zero-sized placeholder that participates in procedure-type
/// identity trivially — all effect rows are currently equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct EffectRow;

// ---------------------------------------------------------------------------
// Struct fields
// ---------------------------------------------------------------------------

/// One field of a struct type.
///
/// Fields are *not* part of a struct type's key (ADR-0015 makes struct identity
/// nominal); they are its resolved body. See
/// [`Pool::set_struct_fields`](crate::Pool::set_struct_fields).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Field {
    /// The field's name, interned by the shared [`jr_base::Interner`].
    pub name: Symbol,
    /// The field's type.
    pub ty: PoolId,
}

impl Field {
    /// Creates a field.
    #[must_use]
    pub const fn new(name: Symbol, ty: PoolId) -> Self {
        Self { name, ty }
    }
}

// ---------------------------------------------------------------------------
// Item
// ---------------------------------------------------------------------------

/// One interned entry: either a type or a compile-time value.
///
/// `Item` is simultaneously the storage and the interning key — there is no
/// separate packed representation, so `Hash` and `Eq` are derived and are
/// exactly the identity relation ADR-0015 specifies. Two `Item`s that compare
/// equal name the same type or value, and interning either yields the same
/// [`PoolId`].
///
/// Composite items hold [`PoolId`]s for their children rather than nested
/// `Item`s. Structural equality is therefore *shallow* and still exact: the
/// children are already interned, so comparing their IDs is deep equality by
/// induction.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Item {
    // ---- types ------------------------------------------------------------
    /// `void` — the type of a procedure that returns nothing (ADR-0015 §3).
    ///
    /// Zero-sized, and a real type rather than an absence, so that a procedure
    /// type's return field is total.
    VoidType,
    /// `bool`.
    BoolType,
    /// An integer type, e.g. `s64` (signed, 64) or `u8` (unsigned, 8).
    IntType {
        /// `true` for `sN`, `false` for `uN`.
        signed: bool,
        /// Width in bits.
        bits: u16,
    },
    /// `string` — a distinct builtin type whose *layout* is
    /// `{data: *u8, count: s64}` (ADR-0004, ADR-0015 §2).
    ///
    /// Deliberately not the struct type of that shape: a user-written
    /// `struct { data: *u8; count: s64; }` is a different type and never
    /// coerces to this one.
    StringType,
    /// The type of a type — the type of every [`Item::TypeValue`].
    ///
    /// Present in the slice because `type_of` must be total over values, and a
    /// type used as a value needs a type. Wave W4 makes this user-visible.
    TypeType,
    /// A type that could not be determined, used to keep analysis going after an
    /// error.
    ///
    /// The HIR already has `TypeRef::Error` and `Expr::Error` for recovery, so
    /// the pool needs a poison type to map them onto. It is equal only to
    /// itself, and never coerces.
    ErrorType,
    /// `*T`.
    ///
    /// Pointers are **structural** (ADR-0015 §4): `*T` interns to one ID for a
    /// given `T`, and nests, so `**T` is this variant applied twice.
    PointerType(
        /// The pointee type.
        PoolId,
    ),
    /// A nominal struct type, keyed on its declaration site (ADR-0015 §1).
    ///
    /// The field list is *not* in the key. It is stored separately and may be
    /// filled in after the type has an ID — see
    /// [`Pool::set_struct_fields`](crate::Pool::set_struct_fields).
    StructType {
        /// Where the struct was declared. This alone is the identity.
        decl: DeclId,
    },
    /// A procedure type.
    ///
    /// Identity is the parameter types, the return type, the context kind
    /// (ADR-0001) and the effect row (ADR-0008) — see ADR-0015 §4.
    ProcType {
        /// The parameter types, in order.
        params: Vec<PoolId>,
        /// The return type. Always present; [`Item::VoidType`] when the source
        /// omitted the arrow.
        ret: PoolId,
        /// Whether the procedure takes the implicit context.
        context: ContextKind,
        /// The inert effect row.
        effects: EffectRow,
    },

    // ---- values -----------------------------------------------------------
    /// The single value of type `void`.
    VoidValue,
    /// A boolean compile-time value.
    BoolValue(
        /// The value.
        bool,
    ),
    /// An integer compile-time value.
    ///
    /// The same bit pattern at two different types interns to two different
    /// values, which is why the type is part of the key.
    IntValue {
        /// The value's type.
        ty: PoolId,
        /// The value, as raw bits.
        bits: u64,
    },
    /// A string compile-time value.
    StrValue(
        /// The interned, already-escape-decoded contents.
        StrId,
    ),
    /// A type used as a compile-time value (ADR-0012, wave W4).
    TypeValue(
        /// The type this value names.
        PoolId,
    ),
    /// A procedure used as a compile-time value.
    ///
    /// Procedures are constants (ADR-0012), so a procedure is a value whose type
    /// is a [`Item::ProcType`]. Two declarations with identical signatures are
    /// different *values* — hence the [`DeclId`] — while still sharing one type.
    ProcValue {
        /// The procedure's type.
        ty: PoolId,
        /// Where the procedure was declared.
        decl: DeclId,
    },
}

impl Item {
    /// Returns `true` if this item is a type rather than a value.
    ///
    /// This is the runtime check that stands in for the compile-time one a
    /// separate `TypeId` newtype would have given us; see the module docs.
    ///
    /// Written as an exhaustive match rather than a `matches!` on the type
    /// variants alone, so that adding a variant is a compile error here instead
    /// of silently defaulting to "value".
    #[must_use]
    pub const fn is_type(&self) -> bool {
        match self {
            Self::VoidType
            | Self::BoolType
            | Self::IntType { .. }
            | Self::StringType
            | Self::TypeType
            | Self::ErrorType
            | Self::PointerType(_)
            | Self::StructType { .. }
            | Self::ProcType { .. } => true,

            Self::VoidValue
            | Self::BoolValue(_)
            | Self::IntValue { .. }
            | Self::StrValue(_)
            | Self::TypeValue(_)
            | Self::ProcValue { .. } => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_id_option_is_niche_optimised() {
        assert_eq!(
            size_of::<Option<PoolId>>(),
            size_of::<PoolId>(),
            "Option<PoolId> must stay 4 bytes or every IR node that holds a type bloats"
        );
    }

    #[test]
    fn ids_are_four_bytes() {
        assert_eq!(size_of::<PoolId>(), 4);
        assert_eq!(size_of::<StrId>(), 4);
    }

    #[test]
    fn decl_id_is_eight_bytes() {
        // FileId is a 4-byte newtype index and `index` is a u32, so a DeclId
        // should not need padding.
        assert_eq!(size_of::<DeclId>(), 8);
    }

    #[test]
    fn effect_row_is_zero_sized() {
        assert_eq!(
            size_of::<EffectRow>(),
            0,
            "the reserved effect row must cost nothing while it is inert"
        );
    }

    #[test]
    fn is_type_partitions_every_variant() {
        // Every variant must answer, and types and values must not overlap.
        let ty = Item::BoolType;
        let value = Item::BoolValue(true);
        assert!(ty.is_type());
        assert!(!value.is_type());
    }

    #[test]
    fn decl_id_debug_is_readable() {
        let decl = DeclId::new(FileId::from_usize(2), 7);
        assert_eq!(format!("{decl:?}"), "DeclId(2:7)");
    }
}
