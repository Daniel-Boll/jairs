//! The [`Pool`] itself: interning, the well-known prefix, and lookup.

use rustc_hash::FxHashMap;

use crate::item::{ContextKind, DeclId, EffectRow, EnumMember, Field, Item, PoolId, StrId};

// ---------------------------------------------------------------------------
// The well-known prefix
// ---------------------------------------------------------------------------

impl PoolId {
    /// `void`.
    pub const VOID: Self = Self::from_usize(0);
    /// `bool`.
    pub const BOOL: Self = Self::from_usize(1);
    /// `s64`.
    pub const S64: Self = Self::from_usize(2);
    /// `u8`.
    pub const U8: Self = Self::from_usize(3);
    /// `string`.
    pub const STRING: Self = Self::from_usize(4);
    /// The type of types.
    pub const TYPE: Self = Self::from_usize(5);
    /// The poison type used to keep analysis going after an error.
    pub const ERROR: Self = Self::from_usize(6);
    /// `*u8`.
    ///
    /// Pre-interned because both the `string` layout and the libc `write`
    /// signature are spelled in it (ADR-0004, `019-foreign.jr`), so it is
    /// reached before any user code mentions a pointer.
    pub const PTR_U8: Self = Self::from_usize(7);
    /// The single value of type `void`.
    pub const VOID_VALUE: Self = Self::from_usize(8);
    /// `true`.
    pub const TRUE: Self = Self::from_usize(9);
    /// `false`.
    pub const FALSE: Self = Self::from_usize(10);
    /// The type of a `#system_library` constant (ADR-0016 §3).
    pub const FOREIGN_LIBRARY: Self = Self::from_usize(11);
    /// `(s64) -> *u8` — an allocator's allocate half (ADR-0062 §2).
    ///
    /// Pre-interned for the same reason [`PoolId::PTR_U8`] is: `CONTEXT_FIELD_TYPES` is a
    /// `const &[PoolId]`, so a context field's type must be a well-known id — and this one is
    /// reached the moment any program mentions `context.allocator`.
    pub const ALLOC_FN: Self = Self::from_usize(12);
    /// `(*u8)` — an allocator's release half, returning `void` (ADR-0062 §2).
    pub const FREE_FN: Self = Self::from_usize(13);

    /// The number of well-known entries seeded by [`Pool::new`].
    pub const WELL_KNOWN_COUNT: usize = 14;
}

// ---------------------------------------------------------------------------
// Pool
// ---------------------------------------------------------------------------

/// Canonical identities for every type and every compile-time value.
///
/// Interning is idempotent: interning equal [`Item`]s always yields the same
/// [`PoolId`], so downstream code compares types by comparing 32-bit integers.
///
/// # Single-threaded, on purpose
///
/// The pool is used behind `&mut Pool`. It is not sharded and takes no locks.
/// Zig's `InternPool` was made thread-safe (per-thread append-only item lists
/// plus a sharded, lock-free-read map, with the owning thread ID packed into the
/// high bits of every index) and *measured a slowdown* before anything used it.
/// If wave W8's parallel analysis needs it, that is the proven shape to adopt;
/// until something needs it, it is complexity with no payer.
///
/// # No removal, and no garbage collection
///
/// There is deliberately no `remove`. `PoolId`s are indices into a `Vec`, so
/// removal cannot compact without invalidating every ID already handed out —
/// which is why Zig's pool marks removed entries and leaks the slot, and why
/// "garbage collection is currently vaporware" is the most-cited regret in its
/// own source. Because our IDs are opaque, the escape hatch if the pool ever
/// grows without bound across an editing session is a remap pass at an update
/// boundary: rebuild the pool and rewrite live IDs through an
/// old-ID-to-new-ID table. Nothing needs that yet.
#[derive(Debug, Clone)]
pub struct Pool {
    /// Every interned entry, indexed by [`PoolId`].
    items: Vec<Item>,
    /// Reverse map for de-duplication.
    dedupe: FxHashMap<Item, PoolId>,
    /// Interned string-value contents, indexed by [`StrId`].
    strings: Vec<String>,
    /// Reverse map for string de-duplication.
    string_dedupe: FxHashMap<String, StrId>,
    /// Resolved struct bodies, keyed by declaration rather than by [`PoolId`]
    /// because the body is not part of an ordinary struct's identity (ADR-0015 §1).
    struct_fields: FxHashMap<DeclId, Vec<Field>>,
    /// Resolved fields of a **parameterised** struct instance, keyed by the instance [`PoolId`]
    /// (ADR-0085 §2).
    ///
    /// Separate from `struct_fields` because the two instances `Box(s64)` and `Box(bool)` share one
    /// `DeclId` and must carry *different* field types — `value: s64` vs `value: bool` — which a
    /// `DeclId`-keyed map cannot hold. An ordinary struct has empty type arguments and stays in
    /// `struct_fields`, so its lookup is untouched; only an instance with arguments lands here. The
    /// dispatcher [`Pool::fields_of`] chooses between the two by whether the `Item` carries arguments.
    instance_fields: FxHashMap<PoolId, Vec<Field>>,
    /// Enum members, keyed by declaration site (ADR-0041 §4).
    enum_members: FxHashMap<DeclId, Vec<EnumMember>>,
}

impl Default for Pool {
    fn default() -> Self {
        Self::new()
    }
}

impl Pool {
    /// Creates a pool seeded with the well-known types and values.
    ///
    /// The seeding order is load-bearing: it is what makes the associated
    /// constants on [`PoolId`] correct. Do not reorder without updating them.
    #[must_use]
    pub fn new() -> Self {
        let mut pool = Self {
            items: Vec::new(),
            dedupe: FxHashMap::default(),
            strings: Vec::new(),
            string_dedupe: FxHashMap::default(),
            struct_fields: FxHashMap::default(),
            instance_fields: FxHashMap::default(),
            enum_members: FxHashMap::default(),
        };

        let void = pool.intern(Item::VoidType);
        let bool_ty = pool.intern(Item::BoolType);
        let s64 = pool.intern(Item::IntType {
            signed: true,
            bits: 64,
        });
        let u8_ty = pool.intern(Item::IntType {
            signed: false,
            bits: 8,
        });
        let string = pool.intern(Item::StringType);
        let type_ty = pool.intern(Item::TypeType);
        let error = pool.intern(Item::ErrorType);
        let ptr_u8 = pool.intern(Item::PointerType(u8_ty));
        let void_value = pool.intern(Item::VoidValue);
        let t = pool.intern(Item::BoolValue(true));
        let f = pool.intern(Item::BoolValue(false));
        let foreign_lib = pool.intern(Item::ForeignLibraryType);
        // The two halves of an allocator (ADR-0062 §2), `ContextKind::Jairs` because a proc-pointer
        // type always is (ADR-0059 §3) — which is what makes a `#foreign` allocator a *different*
        // type, and so refused (E0256) rather than silently accepted.
        let alloc_fn = pool.intern(Item::ProcType {
            params: vec![s64],
            ret: ptr_u8,
            context: ContextKind::Jairs,
            effects: EffectRow,
        });
        let free_fn = pool.intern(Item::ProcType {
            params: vec![ptr_u8],
            ret: void,
            context: ContextKind::Jairs,
            effects: EffectRow,
        });

        debug_assert_eq!(void, PoolId::VOID);
        debug_assert_eq!(bool_ty, PoolId::BOOL);
        debug_assert_eq!(s64, PoolId::S64);
        debug_assert_eq!(u8_ty, PoolId::U8);
        debug_assert_eq!(string, PoolId::STRING);
        debug_assert_eq!(type_ty, PoolId::TYPE);
        debug_assert_eq!(error, PoolId::ERROR);
        debug_assert_eq!(ptr_u8, PoolId::PTR_U8);
        debug_assert_eq!(void_value, PoolId::VOID_VALUE);
        debug_assert_eq!(t, PoolId::TRUE);
        debug_assert_eq!(f, PoolId::FALSE);
        debug_assert_eq!(foreign_lib, PoolId::FOREIGN_LIBRARY);
        debug_assert_eq!(alloc_fn, PoolId::ALLOC_FN);
        debug_assert_eq!(free_fn, PoolId::FREE_FN);
        debug_assert_eq!(pool.len(), PoolId::WELL_KNOWN_COUNT);

        pool
    }

    /// Interns an item, returning its canonical [`PoolId`].
    ///
    /// Idempotent: interning an equal item again returns the same ID without
    /// growing the pool.
    ///
    /// # Panics
    /// Panics if the pool exceeds `PoolId::MAX` entries.
    pub fn intern(&mut self, item: Item) -> PoolId {
        if let Some(&existing) = self.dedupe.get(&item) {
            return existing;
        }
        let id = PoolId::from_usize(self.items.len());
        self.items.push(item.clone());
        self.dedupe.insert(item, id);
        id
    }

    /// Looks an item up **without interning it**, for a consumer holding `&Pool`.
    ///
    /// This exists because both back ends need the type a `Projection::ViewData` lands on —
    /// `*T` for the view's element `T` — and neither has `&mut Pool` to intern one. Returning
    /// `None` rather than fabricating a pointer type is what keeps the failure visible: a
    /// consumer that guessed `*u8` would index with the wrong stride and produce wrong
    /// addresses rather than an error.
    ///
    /// In practice the answer is always `Some` for a well-formed body, because `jr-mir`'s
    /// lowering interns `*T` while building the view. The `Option` is the honest shape for a
    /// lookup, not a hedge against that.
    #[must_use]
    pub fn find(&self, item: &Item) -> Option<PoolId> {
        self.dedupe.get(item).copied()
    }

    /// Returns the item an ID names.
    ///
    /// # Panics
    /// Panics if `id` did not come from this pool.
    #[must_use]
    pub fn item(&self, id: PoolId) -> &Item {
        self.items
            .get(id.index())
            .expect("PoolId came from a different pool")
    }

    /// Returns the number of interned items.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Returns `true` if the pool holds nothing.
    ///
    /// Never true for a pool from [`Pool::new`], which seeds the well-known
    /// prefix; present because clippy requires it alongside `len`.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Returns `true` if `id` names a type rather than a value.
    #[must_use]
    pub fn is_type(&self, id: PoolId) -> bool {
        self.item(id).is_type()
    }

    /// Returns the type of anything in the pool.
    ///
    /// Total: every entry has a type, including the types themselves — a type's
    /// type is [`PoolId::TYPE`]. This totality is why `void` is a real type
    /// rather than an absence (ADR-0015 §3).
    ///
    /// The match is exhaustive by variant rather than falling back on
    /// [`Item::is_type`], so that adding an item kind is a compile error here
    /// instead of an `unreachable!` reached halfway through type checking.
    ///
    /// # Panics
    /// Panics if `id` did not come from this pool.
    #[must_use]
    pub fn type_of(&self, id: PoolId) -> PoolId {
        match self.item(id) {
            // Every type is a value of type `type`.
            Item::VoidType
            | Item::BoolType
            | Item::IntType { .. }
            | Item::FloatType { .. }
            | Item::StringType
            | Item::TypeType
            | Item::ErrorType
            | Item::ForeignLibraryType
            | Item::PointerType(_)
            | Item::ArrayType { .. }
            | Item::ViewType { .. }
            | Item::ResultsType { .. }
            | Item::ContextType
            | Item::EnumType { .. }
            | Item::StructType { .. }
            | Item::UnionType { .. }
            | Item::VariantType { .. }
            | Item::ProcType { .. } => PoolId::TYPE,

            Item::VoidValue => PoolId::VOID,
            Item::BoolValue(_) => PoolId::BOOL,
            Item::StrValue(_) => PoolId::STRING,
            Item::TypeValue(_) => PoolId::TYPE,
            Item::ForeignLibraryValue(_) => PoolId::FOREIGN_LIBRARY,
            // These carry their own type, because one shape can have many. An aggregate constant is here
            // for exactly that reason: two struct types with identically-typed fields have the same
            // element list, so without a `ty` in the key they would intern to one id (ADR-0074 §1).
            Item::IntValue { ty, .. }
            | Item::FloatValue { ty, .. }
            | Item::ProcValue { ty, .. }
            | Item::AggregateValue { ty, .. } => *ty,
        }
    }

    // -----------------------------------------------------------------------
    // Type constructors
    // -----------------------------------------------------------------------

    /// Interns `*pointee`.
    pub fn pointer_to(&mut self, pointee: PoolId) -> PoolId {
        self.intern(Item::PointerType(pointee))
    }

    /// Interns a floating-point value from its raw bits.
    ///
    /// Bits rather than an `f64` for the reason [`Item::FloatValue`] records: this pool's key
    /// derives `Hash` and `Eq`, and `f64` has neither.
    pub fn float_value(&mut self, ty: PoolId, bits: u64) -> PoolId {
        self.intern(Item::FloatValue { ty, bits })
    }

    /// Interns `[len]elem` (ADR-0039 §3).
    pub fn array_of(&mut self, elem: PoolId, len: u64) -> PoolId {
        self.intern(Item::ArrayType { elem, len })
    }

    /// Interns `[]elem` (ADR-0044 §1).
    ///
    /// No length, which is the whole difference from [`Pool::array_of`]: a view's length is
    /// runtime data, so `[]s64` is one type however many elements any particular view has.
    pub fn view_of(&mut self, elem: PoolId) -> PoolId {
        self.intern(Item::ViewType { elem })
    }

    /// Interns the implicit context's struct type (ADR-0057 §1).
    ///
    /// Compiler-declared and structural, so there is one `Context` type across every file — which is
    /// what makes a context passed from one module usable in another without translation, the same
    /// property ADR-0018 §2's shared pool gives every other type.
    pub fn context_type(&mut self) -> PoolId {
        self.intern(Item::ContextType)
    }

    /// The already-interned context type's id, without interning one (ADR-0057 §5).
    ///
    /// A method on `&self` so the native back end can call it during `declare`, where it holds the
    /// pool by shared reference. `None` before sema interns the type.
    #[must_use]
    pub fn context_type_id(&self) -> Option<PoolId> {
        self.find(&Item::ContextType)
    }

    /// The already-interned context type, without interning one (ADR-0057 §5).
    ///
    /// A read-only lookup, because the caller that needs it — `run_main`, creating `main`'s context —
    /// holds the pool by shared reference and re-locking to intern deadlocked. Sema interns the type
    /// long before, so `None` means no procedure in the program receives a context.
    #[must_use]
    pub fn find_context(pool: &Self) -> Option<PoolId> {
        pool.find(&Item::ContextType)
    }

    /// A pointer to the context, which is how it is actually passed (ADR-0057 §2).
    ///
    /// By pointer rather than by value so that a callee's writes are visible to *its* callees — "set
    /// the allocator, then call" is the whole point of a context, and a copy would make that silently
    /// not work. It is also one machine word however many fields the struct grows, which matters
    /// because every Jairs call carries it.
    pub fn context_pointer(&mut self) -> PoolId {
        let context = self.context_type();
        self.pointer_to(context)
    }

    /// The index of a context field by name, or `None` (ADR-0057 §1).
    #[must_use]
    pub fn context_field(name: &str) -> Option<u32> {
        crate::layout::CONTEXT_FIELD_NAMES
            .iter()
            .position(|candidate| *candidate == name)
            .and_then(|index| u32::try_from(index).ok())
    }

    /// The type of a context field by index (ADR-0057 §1).
    #[must_use]
    pub fn context_field_type(index: u32) -> Option<PoolId> {
        crate::layout::CONTEXT_FIELD_TYPES
            .get(index as usize)
            .copied()
    }

    /// Interns the results aggregate of a procedure returning several values (ADR-0052 §1).
    ///
    /// **A one-element list normalises to the element itself**, so `-> (T)` and `-> T` are the same
    /// type and there is no 1-tuple whose behaviour would have to be explained. An *empty* list
    /// normalises to `void` for the same reason: `-> ()` is a procedure returning nothing, which
    /// ADR-0015 §3 already spells `PoolId::VOID`.
    ///
    /// Structural rather than nominal, so `(s64, bool)` written in two files interns once — see
    /// [`Item::ResultsType`] for why an anonymous type cannot key on a `DeclId`.
    pub fn results_type(&mut self, elems: Vec<PoolId>) -> PoolId {
        match elems.len() {
            0 => PoolId::VOID,
            1 => elems[0],
            _ => self.intern(Item::ResultsType { elems }),
        }
    }

    /// The element types of a results aggregate, or `None` for any other type.
    ///
    /// The one place a consumer asks "does this procedure return several values, and which". Sema's
    /// arity check and MIR's destructuring both read it, so neither counts results for itself.
    #[must_use]
    pub fn results_elems(&self, id: PoolId) -> Option<&[PoolId]> {
        match self.item(id) {
            Item::ResultsType { elems } => Some(elems),
            _ => None,
        }
    }

    /// Interns the nominal enum type declared at `decl` (ADR-0041 §4).
    ///
    /// The member list is not required and not part of the key, for the same reason a
    /// struct's fields are not (ADR-0015 §1): a member's value is a constant expression that
    /// resolution may have to evaluate, and the type must have an ID before that starts.
    pub fn enum_type(&mut self, decl: DeclId, flags: bool) -> PoolId {
        self.intern(Item::EnumType { decl, flags })
    }

    /// Records the resolved members of the enum declared at `decl`.
    pub fn set_enum_members(&mut self, decl: DeclId, members: Vec<EnumMember>) {
        self.enum_members.insert(decl, members);
    }

    /// Returns the resolved members of the enum declared at `decl`, if recorded yet.
    #[must_use]
    pub fn enum_members(&self, decl: DeclId) -> Option<&[EnumMember]> {
        self.enum_members.get(&decl).map(Vec::as_slice)
    }

    /// Interns the nominal struct type declared at `decl`.
    ///
    /// The field list is not required, and not part of the key. Calling this
    /// twice for the same `decl` yields the same ID; calling it for two
    /// different `decl`s yields two different IDs even if their fields match
    /// (ADR-0015 §1).
    ///
    /// Splitting identity from the body is what lets a struct refer to itself
    /// through a pointer — `Node :: struct { next: *Node; }` needs `Node` to
    /// already have an ID while its own fields are still being lowered.
    pub fn struct_type(&mut self, decl: DeclId) -> PoolId {
        self.intern(Item::StructType {
            decl,
            args: Vec::new(),
        })
    }

    /// Interns a parameterised struct instance — `Box(s64)` (ADR-0085 §1).
    ///
    /// Distinct from [`Pool::struct_type`] only in carrying `args`: `Box(s64)` and `Box(bool)` share
    /// one `decl` and are two `Item`s, so the interner gives them two `PoolId`s the way it does
    /// `[2]s64` and `[3]s64`. An empty `args` is exactly [`Pool::struct_type`], so this never mints a
    /// second ID for an ordinary struct.
    pub fn struct_instance(&mut self, decl: DeclId, args: Vec<PoolId>) -> PoolId {
        self.intern(Item::StructType { decl, args })
    }

    /// Interns the nominal union type declared at `decl` (ADR-0045 §4).
    ///
    /// Its fields go in the *same* side table a struct's do — [`Pool::set_struct_fields`] —
    /// because the field list is the same data. Only the layout differs.
    pub fn union_type(&mut self, decl: DeclId) -> PoolId {
        self.intern(Item::UnionType {
            decl,
            args: Vec::new(),
        })
    }

    /// Interns the nominal variant type declared at `decl` (ADR-0068 §1).
    ///
    /// Its cases go in the *same* side table a struct's fields do, because a case list is a field
    /// list — what differs is the layout (a leading tag, §3) and the check on a read (§4).
    pub fn variant_type(&mut self, decl: DeclId) -> PoolId {
        self.intern(Item::VariantType {
            decl,
            args: Vec::new(),
        })
    }

    /// Records the resolved fields of the struct declared at `decl`.
    ///
    /// Replaces any fields already recorded, so re-analysing a file is safe.
    pub fn set_struct_fields(&mut self, decl: DeclId, fields: Vec<Field>) {
        self.struct_fields.insert(decl, fields);
    }

    /// Returns the resolved fields of the struct declared at `decl`, if they
    /// have been recorded yet.
    #[must_use]
    pub fn struct_fields(&self, decl: DeclId) -> Option<&[Field]> {
        self.struct_fields.get(&decl).map(Vec::as_slice)
    }

    /// Records the substituted fields of a parameterised struct **instance** (ADR-0085 §2).
    ///
    /// `instance` is the `PoolId` of a [`Item::StructType`] with non-empty arguments — `Box(s64)`.
    /// The fields are the declaration's, resolved under the type-argument bindings, so `Box(s64)`
    /// records `value: s64` and `Box(bool)` records `value: bool` from the one declaration.
    pub fn set_instance_fields(&mut self, instance: PoolId, fields: Vec<Field>) {
        self.instance_fields.insert(instance, fields);
    }

    /// The fields of any struct/union/variant type, by its `PoolId` — the one lookup a consumer
    /// holding a type should use (ADR-0085 §2).
    ///
    /// Dispatches on whether the type carries type arguments: a parameterised instance reads the
    /// instance-keyed map (its fields are substituted), and an ordinary struct reads the
    /// `DeclId`-keyed map exactly as before. A consumer that extracts `decl` and calls
    /// [`Pool::struct_fields`] directly gets the *unsubstituted* template — correct for an ordinary
    /// struct, wrong for an instance — so every field-reading site is being moved to this.
    ///
    /// Returns `None` for a type that is not a struct/union/variant, or whose fields are not
    /// recorded yet.
    #[must_use]
    pub fn fields_of(&self, ty: PoolId) -> Option<&[Field]> {
        match self.item(ty) {
            Item::StructType { decl, args }
            | Item::UnionType { decl, args }
            | Item::VariantType { decl, args } => {
                if args.is_empty() {
                    self.struct_fields.get(decl).map(Vec::as_slice)
                } else {
                    self.instance_fields.get(&ty).map(Vec::as_slice)
                }
            }
            _ => None,
        }
    }

    /// Interns a procedure type.
    ///
    /// `ret` is always a real type; pass [`PoolId::VOID`] when the source
    /// omitted the return arrow.
    pub fn proc_type(&mut self, params: Vec<PoolId>, ret: PoolId, context: ContextKind) -> PoolId {
        self.intern(Item::ProcType {
            params,
            ret,
            context,
            effects: EffectRow,
        })
    }

    // -----------------------------------------------------------------------
    // Value constructors
    // -----------------------------------------------------------------------

    /// Interns an integer value of type `ty`.
    ///
    /// `bits` is the value as the HIR produced it. The pool does not check that
    /// it fits `ty` and does not record that it did not: whether a literal fits
    /// its *contextual* type is a `jr-sema` question (ADR-0016 §1), because the
    /// literal has no type of its own until a context gives it one. By the time
    /// a value is interned that question has been answered.
    pub fn int_value(&mut self, ty: PoolId, bits: u64) -> PoolId {
        self.intern(Item::IntValue { ty, bits })
    }

    /// Interns a boolean value.
    ///
    /// Both booleans are in the well-known prefix, so this never grows the pool.
    pub fn bool_value(&mut self, value: bool) -> PoolId {
        self.intern(Item::BoolValue(value))
    }

    /// Interns a string value from its already-escape-decoded contents.
    pub fn str_value(&mut self, contents: &str) -> PoolId {
        let str_id = self.intern_str(contents);
        self.intern(Item::StrValue(str_id))
    }

    /// Interns an aggregate compile-time value from its element values (ADR-0074 §1).
    ///
    /// `elements` are in declaration order for a struct and index order for an array; each is itself an
    /// interned value, so a nested aggregate needs no special case. `ty` is part of the key, because two
    /// struct types with identically-typed fields would otherwise intern to one id — see
    /// [`Item::AggregateValue`].
    ///
    /// Deliberately takes the *values* rather than a byte image: the pool is target-independent, and a
    /// byte image is not (ADR-0074 §1).
    pub fn aggregate_value(&mut self, ty: PoolId, elements: Vec<PoolId>) -> PoolId {
        self.intern(Item::AggregateValue { ty, elements })
    }

    /// Interns a type as a compile-time value (ADR-0012, wave W4).
    ///
    /// # Panics
    /// Panics if `ty` names a value rather than a type.
    pub fn type_value(&mut self, ty: PoolId) -> PoolId {
        assert!(
            self.is_type(ty),
            "type_value requires a type, got {:?}",
            self.item(ty)
        );
        self.intern(Item::TypeValue(ty))
    }

    /// Interns a procedure as a compile-time value (ADR-0012).
    pub fn proc_value(&mut self, ty: PoolId, decl: DeclId) -> PoolId {
        self.intern(Item::ProcValue { ty, decl })
    }

    /// Interns a foreign library value, e.g. the `"c"` of `#system_library "c"`.
    ///
    /// Its type is always [`PoolId::FOREIGN_LIBRARY`] (ADR-0016 §3).
    pub fn foreign_library_value(&mut self, name: &str) -> PoolId {
        let str_id = self.intern_str(name);
        self.intern(Item::ForeignLibraryValue(str_id))
    }

    /// Reads a foreign library value back out, e.g. `"c"`.
    ///
    /// `None` when `id` names anything else, so a caller that was handed the wrong
    /// [`PoolId`] gets nothing rather than a plausible-looking string.
    ///
    /// This exists so that no consumer matches on
    /// [`Item::ForeignLibraryValue`] itself. ADR-0019 §4 makes the pool the *one*
    /// place a `#foreign` library is resolved — sema records the answer here and
    /// the VM and the native back end read it — and three call sites each
    /// destructuring the item would reintroduce, in a smaller way, exactly the
    /// divergence that consolidating the resolution was meant to end.
    #[must_use]
    pub fn foreign_library_name(&self, id: PoolId) -> Option<&str> {
        // Matched exhaustively, like `type_of` above, so that adding an item kind
        // is a compile error here rather than silently falling into `None`.
        match self.item(id) {
            Item::ForeignLibraryValue(str_id) => Some(self.resolve_str(*str_id)),
            Item::VoidType
            | Item::BoolType
            | Item::IntType { .. }
            | Item::FloatType { .. }
            | Item::StringType
            | Item::TypeType
            | Item::ErrorType
            | Item::ForeignLibraryType
            | Item::PointerType(_)
            | Item::ResultsType { .. }
            | Item::ContextType
            | Item::ArrayType { .. }
            | Item::ViewType { .. }
            | Item::EnumType { .. }
            | Item::StructType { .. }
            | Item::UnionType { .. }
            | Item::VariantType { .. }
            | Item::ProcType { .. }
            | Item::VoidValue
            | Item::BoolValue(_)
            | Item::IntValue { .. }
            | Item::FloatValue { .. }
            | Item::StrValue(_)
            | Item::TypeValue(_)
            | Item::ProcValue { .. }
            // An aggregate constant names no library (ADR-0074 §1).
            | Item::AggregateValue { .. } => None,
        }
    }

    // -----------------------------------------------------------------------
    // String values
    // -----------------------------------------------------------------------

    /// Interns string-value contents, returning a [`StrId`].
    ///
    /// # Panics
    /// Panics if the string table exceeds `StrId::MAX` entries.
    pub fn intern_str(&mut self, contents: &str) -> StrId {
        if let Some(&existing) = self.string_dedupe.get(contents) {
            return existing;
        }
        let id = StrId::from_usize(self.strings.len());
        self.strings.push(contents.to_owned());
        self.string_dedupe.insert(contents.to_owned(), id);
        id
    }

    /// Resolves an interned string value back to its contents.
    ///
    /// # Panics
    /// Panics if `id` did not come from this pool.
    #[must_use]
    pub fn resolve_str(&self, id: StrId) -> &str {
        self.strings
            .get(id.index())
            .expect("StrId came from a different pool")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_known_prefix_is_seeded_at_the_expected_indices() {
        let pool = Pool::new();
        assert_eq!(pool.len(), PoolId::WELL_KNOWN_COUNT);
        assert_eq!(*pool.item(PoolId::VOID), Item::VoidType);
        assert_eq!(*pool.item(PoolId::BOOL), Item::BoolType);
        assert_eq!(
            *pool.item(PoolId::S64),
            Item::IntType {
                signed: true,
                bits: 64
            }
        );
        assert_eq!(
            *pool.item(PoolId::U8),
            Item::IntType {
                signed: false,
                bits: 8
            }
        );
        assert_eq!(*pool.item(PoolId::STRING), Item::StringType);
        assert_eq!(*pool.item(PoolId::TYPE), Item::TypeType);
        assert_eq!(*pool.item(PoolId::ERROR), Item::ErrorType);
        assert_eq!(*pool.item(PoolId::PTR_U8), Item::PointerType(PoolId::U8));
        assert_eq!(*pool.item(PoolId::VOID_VALUE), Item::VoidValue);
        assert_eq!(*pool.item(PoolId::TRUE), Item::BoolValue(true));
        assert_eq!(*pool.item(PoolId::FALSE), Item::BoolValue(false));
        assert_eq!(
            *pool.item(PoolId::FOREIGN_LIBRARY),
            Item::ForeignLibraryType
        );
    }

    #[test]
    fn foreign_libraries_dedupe_by_name() {
        let mut pool = Pool::new();
        let libc = pool.foreign_library_value("c");
        assert_eq!(libc, pool.foreign_library_value("c"));
        assert_ne!(libc, pool.foreign_library_value("m"));
        assert_eq!(pool.type_of(libc), PoolId::FOREIGN_LIBRARY);
        assert!(!pool.is_type(libc));
        assert!(pool.is_type(PoolId::FOREIGN_LIBRARY));
    }

    #[test]
    fn interning_a_well_known_item_reuses_its_id() {
        let mut pool = Pool::new();
        let before = pool.len();
        assert_eq!(pool.intern(Item::BoolType), PoolId::BOOL);
        assert_eq!(pool.pointer_to(PoolId::U8), PoolId::PTR_U8);
        assert_eq!(pool.bool_value(true), PoolId::TRUE);
        assert_eq!(pool.len(), before, "well-known items must not be re-added");
    }

    #[test]
    fn interning_is_idempotent() {
        let mut pool = Pool::new();
        let a = pool.pointer_to(PoolId::S64);
        let b = pool.pointer_to(PoolId::S64);
        assert_eq!(a, b);
        assert_eq!(pool.len(), PoolId::WELL_KNOWN_COUNT + 1);
    }

    #[test]
    fn string_values_dedupe() {
        let mut pool = Pool::new();
        let a = pool.str_value("hello");
        let b = pool.str_value("hello");
        let c = pool.str_value("goodbye");
        assert_eq!(a, b);
        assert_ne!(a, c);
        let Item::StrValue(sid) = *pool.item(a) else {
            panic!("expected a string value");
        };
        assert_eq!(pool.resolve_str(sid), "hello");
    }

    #[test]
    #[should_panic(expected = "type_value requires a type")]
    fn type_value_rejects_a_value() {
        let mut pool = Pool::new();
        let _ = pool.type_value(PoolId::TRUE);
    }

    #[test]
    fn is_empty_is_false_for_a_seeded_pool() {
        assert!(!Pool::new().is_empty());
    }
}
