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

/// The effect row of a procedure type.
///
/// ADR-0008 reserved this slot in the vertical slice, before there was anything to put in it, on the
/// grounds that adding an effects system later would otherwise mean re-typing every signature in the
/// compiler. **It is no longer inert**: ADR-0151 puts `#must` here, which is the first and smallest
/// thing the slot was reserved for — "this call's result must be received" is an obligation on the
/// *call*, which is precisely what an effect is.
///
/// # Why `#must` belongs in the type rather than in a side table
///
/// A call may cross a module boundary, and the caller's file has no HIR for the callee's declaration.
/// Types are interned in one pool shared by every file (ADR-0015), so an obligation carried *by the
/// type* reaches an imported call with no plumbing at all — while a `ProcId`-keyed table would need
/// the same cross-file threading `imported_procs` exists to do, for one bit.
///
/// It also means the obligation survives a procedure being taken as a *value*: `f: (s64) -> (s64, bool)
/// #must` and a pointer to it have the same type, so calling through the pointer keeps the check.
/// A side table keyed on the declaration would have lost that.
///
/// # Why it is still not an effects system
///
/// One flag, not a row of named effects. ADR-0008's door stays open: adding named effects means
/// growing this struct, which is the change it was reserved to make cheap — and `must` being a field
/// rather than the whole struct is what keeps that true.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct EffectRow {
    /// `true` when a call's result must be received (ADR-0151 §1).
    ///
    /// **Part of procedure-type identity**, which is deliberate: a `#must` procedure and an otherwise
    /// identical one that is not are different types, so assigning one to a variable of the other's
    /// type is a mismatch rather than a silent loss of the obligation.
    pub must: bool,
}

// ---------------------------------------------------------------------------
// Struct fields
// ---------------------------------------------------------------------------

/// One member of an enum type (ADR-0041 §4).
///
/// A `(name, value)` pair rather than a [`Field`]: a field has a *type* and a member has a
/// *value*, so reusing `Field` would mean a `PoolId` that is always the backing type and a
/// name that lies about what it holds.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EnumMember {
    /// The member's name, interned by the shared [`jr_base::Interner`].
    pub name: Symbol,
    /// The member's value.
    ///
    /// `i64` rather than `PoolId`: every member of every enum has the same backing type
    /// (ADR-0041 §3), so interning each value would add a pool entry per member for no
    /// identity gain. The value is interned on demand where a `Colour.RED` expression needs
    /// one.
    pub value: i64,
}

impl EnumMember {
    /// Creates a member.
    #[must_use]
    pub const fn new(name: Symbol, value: i64) -> Self {
        Self { name, value }
    }
}

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
    /// `true` for `using base: Point;` — this field's own fields are reachable through the
    /// enclosing struct (ADR-0050 §1).
    ///
    /// Carried here, on the *layout* type, purely so that field **lookup** can follow it. It
    /// deliberately does not affect layout at all: an embedded base stays a real field at a real
    /// offset (ADR-0050 §4), which is what lets `using` be a resolution feature and leaves
    /// `field_offset` untouched.
    pub using: bool,
    /// `#align N` — this field's alignment is at least `N` bytes (ADR-0144 §3).
    ///
    /// A **minimum**, so the effective alignment is `max(natural, N)` and a value below the
    /// type's own is already satisfied rather than refused. `jr-sema` has checked that it is a
    /// power of two in range (E0282); this crate applies it.
    pub align: Option<u32>,
    /// `#place N` — this field sits at exactly byte `N` of its aggregate (ADR-0144 §4).
    ///
    /// **Two fields may share an offset**, deliberately: that is what makes an overlay
    /// expressible where a `union` cannot say that only *some* fields overlap. Nothing checks for
    /// overlap, and nothing requires `N` to satisfy the field's alignment — a placed field may be
    /// unaligned, which every engine handles because each computes its own addresses.
    pub place: Option<u64>,
}

impl Field {
    /// Creates a field with no layout attributes.
    #[must_use]
    pub const fn new(name: Symbol, ty: PoolId) -> Self {
        Self {
            name,
            ty,
            using: false,
            align: None,
            place: None,
        }
    }

    /// Creates a `using`-embedded field (ADR-0050 §1).
    #[must_use]
    pub const fn embedded(name: Symbol, ty: PoolId) -> Self {
        Self {
            name,
            ty,
            using: true,
            align: None,
            place: None,
        }
    }

    /// Returns this field with its layout attributes set (ADR-0144).
    #[must_use]
    pub const fn placed(mut self, align: Option<u32>, place: Option<u64>) -> Self {
        self.align = align;
        self.place = place;
        self
    }

    /// The alignment this field is laid out at, given its type's own.
    ///
    /// One function rather than a `max` written at each site, because the layout fold and the
    /// field-offset walk must agree exactly: two spellings of "how aligned is this field" is how
    /// one of them comes to be wrong by a factor of two, silently.
    #[must_use]
    pub fn effective_align(&self, natural: u32) -> u32 {
        natural.max(self.align.unwrap_or(1))
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
    /// `float32` or `float64` (ADR-0040 §2).
    ///
    /// **Structural**, like [`Item::IntType`]: the width is the whole identity, and there is
    /// no signedness field because IEEE-754 has one signed representation and no unsigned
    /// counterpart.
    FloatType {
        /// Width in bits: 32 or 64.
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
    /// The type of a `#system_library` constant (ADR-0016 §3).
    ///
    /// An opaque handle: there is exactly one such type, and each distinct
    /// library is a [`Item::ForeignLibraryValue`] of it. Giving these a real
    /// type is what lets `#foreign libc "write"` check that its library operand
    /// actually is a library instead of leaving the whole FFI boundary untyped
    /// (ADR-0006).
    ForeignLibraryType,
    /// `*T`.
    ///
    /// Pointers are **structural** (ADR-0015 §4): `*T` interns to one ID for a
    /// given `T`, and nests, so `**T` is this variant applied twice.
    PointerType(
        /// The pointee type.
        PoolId,
    ),
    /// `[N]T` — a fixed-size array (ADR-0039 §3).
    ///
    /// **Structural**, like [`Item::PointerType`] (ADR-0015 §4): `[4]s64` interns to
    /// one ID however many files write it, and the length is part of the key, so
    /// `[4]s64` and `[5]s64` are different types. Nests, so `[2][3]u8` is this
    /// variant applied twice.
    ArrayType {
        /// The element type.
        elem: PoolId,
        /// The number of elements.
        ///
        /// In the key: a length outside the type identity would make `[4]s64` and
        /// `[5]s64` the same type and push the length into sema, which is how a
        /// language ends up unable to say what a value's type is.
        len: u64,
    },
    /// `[]T` — a view over a run of elements (ADR-0044 §1).
    ///
    /// **Structural**, like [`Item::PointerType`] and [`Item::ArrayType`] (ADR-0015 §4), and
    /// it nests: `[][]s64` is this variant applied twice. There is no length in the key
    /// because a view's length is *runtime* data — that is the whole difference from
    /// [`Item::ArrayType`], whose `len` is part of its identity.
    ///
    /// The layout is `{data: *T, count: s64}`, the same two words [`Item::StringType`] has
    /// (ADR-0004). The layouts are shared and the identities are not: `string` is UTF-8 by
    /// convention and a `[]u8` is bytes, so merging them would make every byte run printable
    /// and every string indexable as a number (ADR-0044 §1).
    ViewType {
        /// The element type.
        elem: PoolId,
    },
    /// `#simd [N]T` — a vector, one machine register wide (ADR-0148 §1).
    ///
    /// **Structural**, like [`Item::ArrayType`], and the lane count is in the key for the same
    /// reason a length is. The layout is *identical* to `[N]T`'s: `lanes * size_of(elem)` bytes,
    /// contiguous. What differs is everything else — the representation is a register rather than
    /// memory, and an arithmetic operator applies.
    ///
    /// # Why this is its own variant when an `#soa` struct is not
    ///
    /// ADR-0147 §1 refused an `Item::SoaType` because an `#soa` struct answers every question the
    /// same way an ordinary struct does, so five crates' matches would have grown an arm saying
    /// "the same as a struct". A vector answers three differently: its representation, which
    /// operators apply, and whether its element count is chosen or fixed by its width. A new
    /// variant earns its arms exactly when the arms differ (ADR-0148 §1).
    ///
    /// # Why the width is fixed
    ///
    /// `lanes * size_of(elem)` is **exactly 16 bytes**, enforced in `jr-sema` (E0285), because a
    /// vector operation compiles at exactly that width and nowhere else — `iadd` on an `I64X4`
    /// fails with `Unexpected SSA-value type`, which is what probing found before this was
    /// designed (ADR-0148's context). Nothing here re-checks it: by the time a `PoolId` names this
    /// variant, sema has already refused every other width.
    VectorType {
        /// The element type: a numeric scalar, never a pointer or an aggregate.
        elem: PoolId,
        /// The number of lanes.
        ///
        /// In the key, like [`Item::ArrayType`]'s `len`: `[[4]]s32` and `[[2]]s64` are the same
        /// sixteen bytes and different types, and a use site cannot be asked which it has.
        lanes: u64,
    },
    /// `[..]T` — a growable dynamic array (ADR-0136).
    ///
    /// **Structural**, like [`Item::ViewType`] and [`Item::PointerType`]: the element is the whole
    /// identity, so `[..]s64` interns to one `PoolId` however many files write it. The layout is
    /// `{data: *T, count: s64, capacity: s64}` — three words instead of `ViewType`'s two — because a
    /// dynamic array owns its heap block and knows how much it has, while a view merely borrows a run
    /// (ADR-0136 §1). Kept separate from `Item::StructType` for the same reason `Item::StringType` is
    /// (ADR-0004): a user-written struct with the same three fields is a *different* type and never
    /// coerces to this one — dynamic arrays are indexable and views are not-writable-through, and
    /// merging the identities would blur both properties.
    DynamicArrayType {
        /// The element type.
        elem: PoolId,
    },
    /// The implicit context's struct type (ADR-0057 §1).
    ///
    /// **Compiler-declared**, so it has no `DeclId` from any file — which is why it is its own
    /// variant rather than an `Item::StructType`, whose nominal identity *is* a declaration site
    /// (ADR-0015 §1). The same reasoning ADR-0052 §1 used for a results aggregate: a type with no
    /// declaration cannot key on one.
    ///
    /// Its fields are fixed by the compiler rather than held in the struct side table, because there
    /// is no `DeclId` to key that table on. One field today — `allocator: s64`, a placeholder a
    /// program can read and write so the ABI is observable, deliberately *not* an allocator protocol
    /// (ADR-0057 §1).
    ContextType,
    /// The several results of a procedure returning more than one (ADR-0052 §1).
    ///
    /// **Structural**, keyed on the element list, because it has no declaration site: `(s64, bool)`
    /// written in two files is one type. That is the opposite choice from [`Item::StructType`],
    /// whose nominal `DeclId` exists so two structurally identical structs stay distinct
    /// (ADR-0015 §1) — and the reason it is right here is that there is nothing to be distinct
    /// *from*, since a results list is anonymous by construction.
    ///
    /// **Its layout is a struct's**, computed by delegating to the same function rather than
    /// repeating it: a duplicated offset computation would be a silent wrong answer rather than a
    /// crash, and both engines read offsets from `jr-pool` for exactly that reason (ADR-0018 §2).
    ///
    /// Deliberately **not** a general tuple. ADR-0052 §4 keeps it unspellable as a variable's,
    /// parameter's or field's type: it is a transport that comes into being at a `return` and is
    /// destructured at the call, and making it storable would raise every tuple question — literals,
    /// equality, indexing — that ADR-0052 §1 declined to answer.
    ResultsType {
        /// The result types, in declaration order. Always at least two: interning normalises a
        /// one-element list to the element itself, so there is no 1-tuple to explain.
        elems: Vec<PoolId>,
    },
    /// A nominal enum type, keyed on its declaration site (ADR-0041 §4).
    ///
    /// Structurally identical to [`Item::StructType`], and nominal for the same reason: a
    /// bare integer must not be passable where an enum belongs, which is the only thing an
    /// enum buys over its backing type.
    ///
    /// The member list is *not* in the key. It is stored separately and may be filled in
    /// after the type has an ID — see [`Pool::set_enum_members`](crate::Pool::set_enum_members).
    EnumType {
        /// Where the enum was declared. This alone is the identity.
        decl: DeclId,
        /// `true` for `enum_flags` (ADR-0043 §2).
        ///
        /// Redundant *as identity* — two enums at one `DeclId` cannot differ in it — and
        /// carried anyway so that any consumer holding a `PoolId` can answer "is this a flags
        /// enum" without a side-table lookup. Sema needs that answer at every operator site,
        /// and a lookup per site is how the two would eventually disagree.
        flags: bool,
    },
    /// A nominal struct type, keyed on its declaration site (ADR-0015 §1).
    ///
    /// The field list is *not* in the key. It is stored separately and may be
    /// filled in after the type has an ID — see
    /// [`Pool::set_struct_fields`](crate::Pool::set_struct_fields).
    StructType {
        /// Where the struct was declared.
        decl: DeclId,
        /// The type arguments of a parameterised struct — `[s64]` for `Box(s64)` (ADR-0085 §1).
        ///
        /// **Empty for an ordinary `struct { … }`**, which interns exactly as it did before this
        /// field existed: an empty `Vec` changes no key, so every non-parameterised struct keeps
        /// its old `PoolId`. `Box(s64)` and `Box(bool)` share one `decl` and are told apart *here* —
        /// two distinct `Item`s, two distinct `PoolId`s — the same way [`Item::ArrayType`]
        /// distinguishes `[2]s64` from `[3]s64`. The field side table keys on the instance `PoolId`
        /// (ADR-0085 §2), so the two instances carry two field lists, substituted per argument.
        args: Vec<PoolId>,
    },
    /// A nominal union type, keyed on its declaration site (ADR-0045 §4).
    ///
    /// Structurally identical to [`Item::StructType`] and nominal for the same reason, and it
    /// shares the *same* field side table — [`Pool::set_struct_fields`](crate::Pool::set_struct_fields)
    /// keys on the instance and knows nothing about which kind of declaration it was, so the fields
    /// are the same data.
    ///
    /// A **separate variant** rather than a `union: bool` on `StructType`, because the two
    /// differ in *layout*: every field of a union sits at offset 0 and the size is the largest
    /// field's. A boolean would let a site that forgot to check compute struct offsets for a
    /// union and produce wrong addresses silently; a variant makes every offset-computing site
    /// a compile error until it handles both.
    UnionType {
        /// Where the union was declared.
        decl: DeclId,
        /// The type arguments of a parameterised union — empty for an ordinary `union` (ADR-0085 §1).
        args: Vec<PoolId>,
    },
    /// A `variant { … }` type, nominal for the reason [`Item::StructType`] and [`Item::UnionType`]
    /// are: two variants with identical cases in two files are two types (ADR-0068 §1).
    ///
    /// A **separate variant** rather than a flag on `UnionType`, because the two have different
    /// layouts (a variant carries a leading tag, §3) and different access semantics (a read is
    /// checked, §4) — so a consumer that treated them alike would be wrong about both size and cost.
    VariantType {
        /// The declaration site that gives this variant its identity.
        decl: DeclId,
        /// The type arguments of a parameterised variant — empty for an ordinary `variant`
        /// (ADR-0085 §1).
        args: Vec<PoolId>,
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
        /// The effect row (ADR-0151 §1 — no longer inert; it carries `#must`).
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
    /// A floating-point compile-time value.
    ///
    /// The value is stored as **bits**, not as an `f64`, because this enum derives `Hash`
    /// and `Eq` and `f64` has neither: `NaN != NaN` breaks `Eq`, and `0.0 == -0.0` with
    /// different bit patterns breaks the `Hash`/`Eq` contract (ADR-0040's Consequences).
    ///
    /// The consequence is that `0.0` and `-0.0` are **distinct pool entries**, which is
    /// correct rather than a compromise: they are distinguishable values, and
    /// `1.0/0.0` against `1.0/-0.0` proves it.
    FloatValue {
        /// The value's type.
        ty: PoolId,
        /// The value, as raw IEEE-754 bits. A `float32`'s are its low 32.
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
    /// A specific foreign library, e.g. `#system_library "c"` (ADR-0016 §3).
    ///
    /// Keyed on the library name, so two constants naming the same library are
    /// the same value.
    ForeignLibraryValue(
        /// The library name, as written.
        StrId,
    ),
    /// A struct or fixed-array compile-time value: its **element values, in order** (ADR-0074 §1).
    ///
    /// The first *recursive* value variant — an element may itself be an aggregate, and a [`PoolId`]
    /// already expresses that, because interning is recursive by construction. So `[2]P` interns as an
    /// `AggregateValue` of two `AggregateValue`s, with no bespoke tree to maintain.
    ///
    /// **Deliberately not the byte image**, though `jr-db`'s `reduce` already has one from the VM. The
    /// pool is **target-independent** — `layout_of(pool, target, ty)` takes a [`crate::TargetLayout`] and
    /// the pool holds none — so a byte image would put a target fact inside the shared pool: the VM writes
    /// it at offsets for *one* target, and a cross-compile would read plausible wrong values rather than
    /// fail. Every target in the slice being `LP64` is exactly why that would go unnoticed. Interning the
    /// values instead leaves `field_offset(pool, target, …)` to produce bytes at the point that knows which
    /// target is meant (ADR-0074 §1).
    ///
    /// `string` is **not** one of these: it interns as [`Item::StrValue`], because its contents are its
    /// identity and its runtime form is a pointer, which has no compile-time value (ADR-0074 §2). A union
    /// is not one either — untagged storage makes "which field is valid" unanswerable (§4).
    ///
    /// **Carries its type**, for the reason [`Item::IntValue`] does: one shape can have many types. Two
    /// distinct struct types with identically-typed fields would otherwise produce the same element list
    /// and *intern to one id* — so `type_of` could not answer, and a constant of one type would silently
    /// stand in for the other. Interning is by the whole key, so the type is part of the identity.
    AggregateValue {
        /// The aggregate's type: a struct or fixed-array type.
        ty: PoolId,
        /// The element values, in declaration order for a struct and index order for an array.
        elements: Vec<PoolId>,
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
            | Self::FloatType { .. }
            | Self::StringType
            | Self::TypeType
            | Self::ErrorType
            | Self::ForeignLibraryType
            // A results list is a type, so it answers `type` like every other (ADR-0052 §1).
            | Self::ResultsType { .. }
            // So is the context's struct type (ADR-0057 §1).
            | Self::ContextType
            | Self::PointerType(_)
            | Self::ArrayType { .. }
            | Self::VectorType { .. }
            | Self::ViewType { .. }
            | Self::DynamicArrayType { .. }
            | Self::EnumType { .. }
            | Self::StructType { .. }
            | Self::UnionType { .. }
            | Self::VariantType { .. }
            | Self::ProcType { .. } => true,

            Self::VoidValue
            | Self::BoolValue(_)
            | Self::IntValue { .. }
            | Self::FloatValue { .. }
            | Self::StrValue(_)
            | Self::TypeValue(_)
            | Self::ProcValue { .. }
            | Self::ForeignLibraryValue(_)
            // An aggregate constant is a value (ADR-0074 §1), whatever its elements are.
            | Self::AggregateValue { .. } => false,
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

    /// The effect row was zero-sized while it was inert, and this test asserted that.
    ///
    /// **That premise is now false by design** (ADR-0151 §1): the row carries `#must`, which is the
    /// first and smallest thing ADR-0008 reserved the slot for. Asserting emptiness would now be
    /// asserting that the slot is still unused — so the test asserts what actually matters instead,
    /// which is that the row participates in procedure-type **identity**.
    ///
    /// That is the load-bearing property. If two procedure types differing only in `#must` interned to
    /// the same `PoolId`, assigning a `#must` procedure to a variable of the unmarked type would
    /// silently drop the obligation — the exact silent-loss shape this project keeps finding.
    #[test]
    fn the_effect_row_is_part_of_procedure_type_identity() {
        let mut pool = crate::Pool::new();
        let plain = pool.proc_type(vec![PoolId::S64], PoolId::S64, ContextKind::Jairs);
        let must = pool.proc_type_with(
            vec![PoolId::S64],
            PoolId::S64,
            ContextKind::Jairs,
            EffectRow { must: true },
        );
        assert_ne!(
            plain, must,
            "a `#must` procedure type must not intern to the same id as an unmarked one, or the \
             obligation would be lost by assignment"
        );

        // And interning is still idempotent with the row present, which is what keeps two calls to the
        // same `#must` procedure comparable by id.
        let again = pool.proc_type_with(
            vec![PoolId::S64],
            PoolId::S64,
            ContextKind::Jairs,
            EffectRow { must: true },
        );
        assert_eq!(must, again);
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
