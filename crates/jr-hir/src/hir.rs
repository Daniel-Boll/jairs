//! HIR data types: arenas, IDs, and the node kinds.
//!
//! All IDs are 32-bit newtypes (via [`jr_base::newtype_index!`]) so they are
//! `Copy`, `Eq`, `Hash`, and 4 bytes. `Option<Id>` is also 4 bytes thanks to
//! the niche in `NonZeroU32`.
//!
//! ## Arena choice
//!
//! We use plain `Vec<T>` indexed by the newtype ID. This is the simplest
//! possible arena: O(1) push and index, no fragmentation, and the ID is just
//! the index. We do not need deletion (HIR is immutable after lowering), so
//! `slotmap` or `id-arena` would add complexity without benefit.

use jr_base::{Interner, Span, Symbol};
use jr_pool::PoolId;

jr_base::newtype_index! {
    /// A file-level declaration.
    pub struct ItemId;
}

jr_base::newtype_index! {
    /// An expression node inside a body.
    pub struct ExprId;
}

jr_base::newtype_index! {
    /// A statement node inside a body.
    pub struct StmtId;
}

jr_base::newtype_index! {
    /// A syntactic type reference (not a resolved type).
    pub struct TypeRefId;
}

jr_base::newtype_index! {
    /// A local variable inside a body.
    pub struct LocalId;
}

jr_base::newtype_index! {
    /// A struct field.
    pub struct FieldId;
}

jr_base::newtype_index! {
    /// A procedure parameter.
    pub struct ParamId;
}

jr_base::newtype_index! {
    /// A procedure body.
    pub struct BodyId;
}

jr_base::newtype_index! {
    /// A procedure definition.
    pub struct ProcId;
}

jr_base::newtype_index! {
    /// A struct type definition.
    pub struct StructId;
}

jr_base::newtype_index! {
    /// An enum type definition.
    pub struct EnumId;
}

// ---------------------------------------------------------------------------
// Type references (syntactic, not resolved)
// ---------------------------------------------------------------------------

/// A syntactic type reference.
///
/// These are *not* resolved types — resolution happens in `jr-sema`. A
/// `TypeRef` is just the shape of the type as written in source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeRef {
    /// A named type, e.g. `s64`, `Point`, `bool`.
    Name(Symbol),
    /// A polymorphic type variable, `$T` (ADR-0081 §1).
    ///
    /// Distinct from [`TypeRef::Name`] because it **binds** a variable a call infers, rather than naming
    /// an existing type: sema treats `$T` as introducing `T` into the signature's scope, and a bare `T`
    /// elsewhere in the same signature as a use of it. Keeping them apart is what lets sema say which is
    /// meant instead of trying to resolve `$T` as a type that does not exist.
    Poly(Symbol),
    /// A pointer type `*T`.
    Pointer(TypeRefId),
    /// A fixed-size array type `[N]T` (ADR-0039 §3).
    Array {
        /// The element type.
        elem: TypeRefId,
        /// The literal length, when the length was written as one.
        ///
        /// `None` for `[COUNT]u8` and for any other non-literal — the length is *read*
        /// during lowering because that is where the literal token is, but it is
        /// **`jr-sema` that reports a bad one** (E0233). Lowering stays quiet.
        ///
        /// That split is not arbitrary. `jr-sema` has no constant evaluator (ADR-0018 §3
        /// puts const-eval in `jr-db` over the bytecode VM, downstream of type
        /// resolution), so sema cannot *compute* `COUNT` — but rejecting a type is a
        /// semantic judgement, and putting it in lowering made a well-formed program
        /// report a lowering error, which `tests/corpus/type-errors/` explicitly forbids
        /// its files from doing (ADR-0039 §3a).
        len: Option<u64>,
        /// The **name** the length was written as, when it was a bare name rather than a literal
        /// (ADR-0070 §1).
        ///
        /// Carried so that `jr-sema` can resolve it to a constant and read that constant's literal — a
        /// lookup needing no evaluation and therefore no dependency on `jr-db` or `jr-vm`, which is what
        /// makes `[N]s64` resolvable a sub-wave before `[2 + 2]s64` (ADR-0039 §3a is amended here rather
        /// than reversed).
        ///
        /// `None` when the length was a literal (then `len` is `Some`) or an expression that is neither —
        /// `[2 + 2]u8` names nothing to look up, and sema reports it.
        len_name: Option<Symbol>,
        /// Span of the length expression, for the diagnostic sema raises.
        ///
        /// Carried because a `TypeRef` has no span of its own (ADR-0013), so without this
        /// E0233 would have to point at the whole declaration.
        len_span: Span,
    },
    /// A vector type `#simd [N]T` — one machine register wide (ADR-0148 §1).
    ///
    /// The same four fields [`TypeRef::Array`] carries, and for the same reasons: the lane count may
    /// be a literal or a name that resolves to one, lowering reads it but does not judge it, and the
    /// span is carried because a `TypeRef` has none (ADR-0013). What differs is entirely in sema —
    /// the count must be one of six values rather than any positive integer, and arithmetic applies.
    ///
    /// Its own variant rather than an `Array` with a flag, because the *type* is different (ADR-0148
    /// §1): a flag would make `#simd [4]s32` and `[4]s32` the same `TypeRef`, so resolution would
    /// have to intern one of two pool items from a field rather than from the shape it is looking at.
    Vector {
        /// The element type.
        elem: TypeRefId,
        /// The literal lane count, when it was written as one.
        lanes: Option<u64>,
        /// The **name** the lane count was written as, when it was a bare name (ADR-0070 §1).
        lanes_name: Option<Symbol>,
        /// Span of the lane-count expression, for the diagnostic sema raises (E0285).
        lanes_span: Span,
    },
    /// A view type `[]T` (ADR-0044 §1).
    ///
    /// Its own variant rather than an [`TypeRef::Array`] with `len: None`, because that
    /// value already means "the length was written and was not a usable literal" (E0233).
    /// Sharing the variant would make a view and that error indistinguishable.
    View {
        /// The element type.
        elem: TypeRefId,
    },
    /// A dynamic-array type `[..]T` — a growable heap-backed sequence (ADR-0136).
    ///
    /// A compiler-known layout `{data: *T, count: s64, capacity: s64}` — 24 bytes on a 64-bit
    /// target — lifted to native syntax from `modules/List`. Its own variant rather than a
    /// [`TypeRef::View`] with an added capacity: the two types differ in *ownership* (a view
    /// borrows a run of elements; a dynamic array owns its heap block and its length is
    /// mutable), and merging them at the type level would make every view an owner-in-hiding.
    DynamicArray {
        /// The element type.
        elem: TypeRefId,
    },
    /// The result list of a procedure returning several values: `(s64, bool)` (ADR-0052 §1).
    ///
    /// Reachable **only** as a return type — ADR-0052 §4 keeps it unspellable elsewhere, so a
    /// consumer meeting one anywhere else has found a bug rather than a type to support.
    Results(Vec<TypeRefId>),
    /// A procedure-pointer type `(T, T) -> T` (ADR-0059 §3).
    ///
    /// Resolved by `jr-sema` to the **same** `Item::ProcType` a declared procedure has, so a
    /// procedure value passes to a parameter of this type by an ordinary type match. The `->` is
    /// what distinguishes this from [`TypeRef::Results`] in the source; here they are different
    /// variants and cannot be confused.
    Proc {
        /// The parameter types, in order.
        params: Vec<TypeRefId>,
        /// The return type, or `None` for a procedure type that returns `void`.
        ///
        /// `None` rather than a `TypeRef::Name("void")`, because `void` has no spelling — sema
        /// *rejects* the name `void` (ADR-0015 §3) — so there is no name to lower to. A missing
        /// return resolves to `PoolId::VOID` in sema, the same way a declared procedure's missing
        /// arrow does (`signature.rs`).
        ret: Option<TypeRefId>,
    },
    /// An inline struct type `struct { ... }`.
    Struct(StructId),
    /// An inline union type `union { ... }` (ADR-0045).
    ///
    /// Indexes the same arena `TypeRef::Struct` does — see `Struct::kind`.
    Union(StructId),
    /// An inline variant type `variant { ... }` (ADR-0068 §1).
    ///
    /// The same arena again, for the reason `Struct::kind` documents.
    Variant(StructId),
    /// An inline enum type `enum { ... }` (ADR-0041).
    Enum(EnumId),
    /// A parameterised type reference `Box(s64)` — a name applied to type arguments (ADR-0085 §3).
    ///
    /// Distinct from [`TypeRef::Name`] because it *applies* a type constructor rather than naming an
    /// existing type: sema resolves `name` to a parameterised declaration, resolves each argument to a
    /// type, and interns the instance. Keeping them apart is what lets an ordinary `Point` stay a
    /// `Name` — with no argument list to resolve — while `Box(s64)` carries its arguments.
    Apply {
        /// The type constructor's name — `Box` in `Box(s64)`.
        name: Symbol,
        /// The type arguments, in order — `[s64]` in `Box(s64)`.
        args: Vec<TypeRefId>,
    },
    /// A type that could not be lowered (error recovery).
    Error,
}

// ---------------------------------------------------------------------------
// Literals
// ---------------------------------------------------------------------------

/// A literal value as it appears in source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Literal {
    /// An integer literal, **signed**: a leading `-` is folded in here during lowering
    /// (ADR-0038 §1).
    ///
    /// `radix` is 10, 16, 2, or 8.
    Int {
        /// The parsed value, sign included.
        ///
        /// `i128` because `u64::MAX` and `i64::MIN` must both be representable and no 64-bit
        /// integer type holds both — the same reason `jr_pool::IntKind`'s `min`, `max`,
        /// `decode` and `check` are already `i128`, so this makes the literal agree with the
        /// arithmetic rather than introducing a new width (ADR-0038 §2).
        ///
        /// This used to be a `u64` *magnitude*, with `-1` represented as `Neg` applied to
        /// `1`. That made the minimum of every signed type unwritable: `-128` was 128 tested
        /// against `s8`'s maximum of 127, and `jr_pool::int_negate` traps on negating it
        /// besides.
        value: i128,
        /// The radix (10, 16, 2, or 8).
        radix: u32,
        /// `true` if the literal fits no Jairs integer type at all.
        ///
        /// Computed against the signed value, so `-9223372036854775808` is **not** flagged:
        /// it is exactly `s64::MIN`, where the old `value > i64::MAX` test on the magnitude
        /// rejected it.
        overflowed: bool,
    },
    /// A floating-point literal (ADR-0040 §5).
    ///
    /// The value is held as an `f64` regardless of the eventual type, for the same reason
    /// ADR-0038 §2 made the integer literal an `i128`: the widest representation is the one
    /// that cannot lose information before the type is known. A `float32` context narrows it
    /// at interning time, and IEEE-754 says that narrowing rounds and saturates rather than
    /// failing — so unlike an integer literal, one that does not fit is **not** an error.
    Float {
        /// The parsed value, as raw IEEE-754 `f64` bits.
        ///
        /// **Bits rather than an `f64`**, because this enum derives `Eq` and `f64` does not
        /// implement it — `NaN != NaN`. That is the identical constraint `Item::FloatValue`
        /// records in `jr-pool`, arrived at from a different direction: here it is `Eq` on
        /// the HIR node, there it is the derived `Hash`/`Eq` that makes interning work.
        ///
        /// Two literals with the same text intern to the same bits, so structural equality
        /// on HIR still means what it should. `0.0` and `-0.0` are distinguishable, which is
        /// correct.
        bits: u64,
        /// `true` if the literal's text could not be parsed as a float at all.
        ///
        /// Distinct from "too large": `1e400` parses fine and *is* `inf`, which ADR-0040 §1
        /// makes a legitimate value. This flag is for text the lexer accepted and `f64`'s
        /// parser did not, which should be unreachable and is recorded rather than assumed
        /// away.
        malformed: bool,
    },
    /// A string literal with all escape sequences decoded.
    Str(String),
    /// `true` or `false`.
    Bool(bool),
    /// The null pointer, `null` (ADR-0060 §1).
    ///
    /// Carries no value: a null pointer is the bit pattern 0, and its *type* comes from context
    /// exactly as an integer literal's does (ADR-0016 §1). Held as a literal rather than a keyword
    /// expression of its own so it takes the context-typing path `check_literal` already has, and so
    /// `jr-fmt` finds it by the same route every other literal uses.
    Null,
}

// ---------------------------------------------------------------------------
// Operators
// ---------------------------------------------------------------------------

/// A binary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinOp {
    /// `+` (trapping)
    Add,
    /// `-` (trapping)
    Sub,
    /// `*` (trapping)
    Mul,
    /// `/`
    Div,
    /// `%`
    Rem,
    /// `+%` (wrapping, ADR-0002)
    WrapAdd,
    /// `-%` (wrapping, ADR-0002)
    WrapSub,
    /// `*%` (wrapping, ADR-0002)
    WrapMul,
    /// `&` — bitwise and (ADR-0042)
    BitAnd,
    /// `|` — bitwise or
    BitOr,
    /// `^` — bitwise xor
    BitXor,
    /// `<<` — left shift. Traps on a count outside the type's width (ADR-0042 §3)
    Shl,
    /// `>>` — right shift, **arithmetic** for a signed type and logical for an unsigned one
    /// (ADR-0042 §2). Traps on an out-of-range count
    Shr,
    /// `==`
    Eq,
    /// `!=`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `&&`
    And,
    /// `||`
    Or,
}

/// A unary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnOp {
    /// `-` (arithmetic negation, trapping)
    Neg,
    /// `!` (logical not)
    Not,
    /// Prefix `*` (address-of)
    AddrOf,
    /// `~` (bitwise complement, ADR-0042 §4)
    ///
    /// Distinct from [`UnOp::Not`]: `!` is the boolean negation and `~` is a bitwise one, so
    /// `~true` has no meaning and is refused.
    BitNot,
}

/// An assignment operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssignOp {
    /// `=`
    Assign,
    /// `+=` (trapping)
    AddAssign,
    /// `-=` (trapping)
    SubAssign,
    /// `*=` (trapping)
    MulAssign,
    /// `/=`
    DivAssign,
    /// `%=`
    RemAssign,
    /// `+%=` (wrapping)
    WrapAddAssign,
    /// `-%=` (wrapping)
    WrapSubAssign,
    /// `*%=` (wrapping)
    WrapMulAssign,
    /// `&=` (ADR-0042 §6)
    BitAndAssign,
    /// `|=`
    BitOrAssign,
    /// `^=`
    BitXorAssign,
    /// `<<=`
    ShlAssign,
    /// `>>=`
    ShrAssign,
}

// ---------------------------------------------------------------------------
// Name resolution result
// ---------------------------------------------------------------------------

/// The result of resolving a name reference.
///
/// # Why this is not `Copy`
///
/// [`Res::Promoted`] carries a `Box<Res>`, because a promoted name resolves to a *path* —
/// `x` meaning `p.x` — and a self-referential enum cannot be `Copy`. ADR-0050 §2 chose that
/// over the two alternatives (rewriting the HIR during resolution, or a side map beside
/// `resolutions`) for one reason: adding a variant makes every exhaustive match over `Res` a
/// compile error, so no consumer can *silently* fail to handle a promoted name. The allocation
/// is per promoted resolution, not per lookup.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Res {
    /// A local variable.
    Local(LocalId),
    /// A procedure parameter.
    Param(ParamId),
    /// A file-level item.
    Item(ItemId),
    /// A name from an imported scope.
    ///
    /// The `ItemId` is the `#import` item in the current file; the `Symbol`
    /// is the name in the imported scope.
    Imported(ItemId, Symbol),
    /// A field reached through a `using` binding (ADR-0050 §2).
    ///
    /// `x` where `using p: Point` is in scope resolves to
    /// `Promoted { base: Local/Param(p), field: x }`. The base is itself a `Res` rather than a
    /// `LocalId`, so a name promoted through an *embedded* field is a chain — lowering walks it
    /// rather than assuming one level (ADR-0050 §4's transitivity).
    Promoted {
        /// What the fields were promoted from.
        base: Box<Res>,
        /// The field's name in the base's type.
        field: Symbol,
    },
    /// Resolution failed (unresolved name or error recovery).
    Error,
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

/// An expression node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    /// A literal value.
    Literal(
        /// The decoded value.
        Literal,
        /// Span of the literal token.
        Span,
    ),
    /// The implicit context, from the `context` keyword (ADR-0057 §1).
    ///
    /// Carries no name and no resolution: `context` is a keyword, so there is nothing to resolve and
    /// nothing for the `ResolveMap` to hold. That is deliberate — a `Res` entry would make it look
    /// like a name reference to anything reading that map, the same reason ADR-0049 §2 kept a loop
    /// label out of it.
    Context(Span),
    /// A name reference.
    ///
    /// `res` is filled in by the name-resolution pass. Before resolution it
    /// is `Res::Error`.
    Name {
        /// The interned name.
        name: Symbol,
        /// The span of the name token.
        span: Span,
        /// Resolution result (filled by [`resolve`](fn@crate::resolve)).
        res: Res,
    },
    /// `lhs op rhs`
    Binary {
        /// The operator.
        op: BinOp,
        /// Left operand.
        lhs: ExprId,
        /// Right operand.
        rhs: ExprId,
        /// Span of the whole expression.
        span: Span,
    },
    /// `op operand`
    Unary {
        /// The operator.
        op: UnOp,
        /// The operand.
        operand: ExprId,
        /// Span of the whole expression.
        span: Span,
    },
    /// `callee(args)`
    Call {
        /// The callee expression.
        callee: ExprId,
        /// The argument expressions, **in source order**.
        ///
        /// A named argument (ADR-0053 §1) appears here at the position it was *written*; the name is
        /// in `arg_names` at the same index. Sema reorders them into parameter order and records the
        /// result, so `jr-mir` never sees a name — one pass decides argument order.
        args: Vec<ExprId>,
        /// One entry per argument: `Some(name)` for `b = 2`, `None` for a positional one.
        ///
        /// A parallel `Vec` rather than a `Vec<(Option<Symbol>, ExprId)>` so that every existing
        /// consumer walking `args` keeps working unchanged — and an all-positional call carries a
        /// vector of `None`, which costs one allocation and keeps the two lists impossible to
        /// misalign.
        arg_names: Vec<Option<Symbol>>,
        /// Span of the whole call.
        span: Span,
    },
    /// `receiver.name`
    Field {
        /// The receiver expression.
        receiver: ExprId,
        /// The field name.
        name: Symbol,
        /// Span of the field name token.
        name_span: Span,
        /// Span of the whole expression.
        span: Span,
    },
    /// `a[]` — a view over the whole of `a` (ADR-0044 §2).
    ///
    /// A distinct expression rather than sugar for anything: it takes the *address* of its
    /// base, so `escape.rs` must treat it exactly as it treats [`UnOp::AddrOf`]. A promoted
    /// local has no address for a view's `data` word to point at, which would be a
    /// miscompile rather than a diagnostic — and it is the reason ADR-0044 §2 made the
    /// operator explicit instead of coercing an array implicitly.
    Slice {
        /// The expression being sliced.
        base: ExprId,
        /// Span of the whole expression.
        span: Span,
    },
    /// `base[index]` (ADR-0039 §5).
    Index {
        /// The expression being indexed.
        base: ExprId,
        /// The index expression.
        index: ExprId,
        /// Span of the index expression alone.
        ///
        /// Separate from `span` because an out-of-range or wrongly-typed index is a
        /// complaint about the index, not about the whole access — and pointing at
        /// `buf[i]` when the problem is `i` makes the reader look in the wrong place.
        index_span: Span,
        /// Span of the whole expression.
        span: Span,
    },
    /// `pointer.*`
    Deref(ExprId, Span),
    /// `---` (explicit non-initialisation)
    Uninit(Span),
    /// `cast(T, x)` — a conversion to an explicitly named type (ADR-0037 §2).
    Cast {
        /// The target type.
        ///
        /// A `TypeRefId` and therefore resolved in the arena the expression's
        /// [`ExprScope`](crate::ExprScope) selects — a cast inside a body indexes `Body::type_refs`, and one at
        /// file scope indexes `FileHir::type_refs`. Reading the wrong arena silently yields an
        /// unrelated type rather than failing, which is the trap `ExprScope` exists for.
        ty: TypeRefId,
        /// The operand being converted.
        operand: ExprId,
        /// Span of the whole `cast(T, x)`.
        span: Span,
    },
    /// `xx expr` — a conversion whose target type comes from the context (ADR-0046 §2).
    ///
    /// Deliberately **not** `Expr::Cast` with an `Option<TypeRefId>`: an optional target would
    /// make every existing consumer of `Cast` handle a case where the type is unknown, and the
    /// two differ in exactly the question ADR-0046 settles — where the type comes from.
    ///
    /// Carries no type at all, because there is no syntax for one. Sema supplies it from
    /// `expected` and refuses (E0242) when there is none; MIR then lowers this through the
    /// *same* path `cast` uses, since by then the target is simply the expression's type.
    Autocast {
        /// The operand being converted.
        operand: ExprId,
        /// Span of the whole `xx expr`.
        span: Span,
    },
    /// `.RED` — an enum member named without its type (ADR-0046 §3, ADR-0041 §2's plan).
    ///
    /// Resolves to **nothing** during lowering: finding the member needs to know which enum,
    /// and that comes from the context type, which only sema has. This is why it is not a
    /// `Res` on an `Expr::Name` — there is no scope in which a bare member is a name.
    Member {
        /// The member name.
        name: Symbol,
        /// Span of the name token, for the "no such member" diagnostic.
        name_span: Span,
        /// Span of the whole `.RED`.
        span: Span,
    },
    /// `#run expr`
    Run(ExprId, Span),
    /// A directive expression, e.g. `#system_library "c"`.
    Directive {
        /// The directive name (without `#`).
        name: Symbol,
        /// The optional string argument.
        arg: Option<String>,
        /// Span of the whole directive.
        span: Span,
    },
    /// Error recovery placeholder.
    Error(Span),
}

impl Expr {
    /// Returns the span of this expression.
    pub fn span(&self) -> Span {
        match self {
            Expr::Context(span) => *span,
            Expr::Literal(_, span) => *span,
            Expr::Name { span, .. } => *span,
            Expr::Binary { span, .. } => *span,
            Expr::Unary { span, .. } => *span,
            Expr::Call { span, .. } => *span,
            Expr::Field { span, .. } => *span,
            Expr::Index { span, .. } => *span,
            Expr::Slice { span, .. } => *span,
            Expr::Deref(_, span) => *span,
            Expr::Uninit(span) => *span,
            Expr::Cast { span, .. } => *span,
            Expr::Autocast { span, .. } => *span,
            Expr::Member { span, .. } => *span,
            Expr::Run(_, span) => *span,
            Expr::Directive { span, .. } => *span,
            Expr::Error(span) => *span,
        }
    }
}

// ---------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------

/// What a `for` loop iterates over (ADR-0049 §1).
///
/// Three shapes and no more: the compiler knows arrays, views and ranges, and there is no
/// user-extensible protocol until W5's macros can express one. A range is **only** representable
/// here — there is no `Range` type in the pool and no `..` operator in the expression grammar —
/// which is what keeps `0..n` from colliding with `[..]T`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForIterable {
    /// An array or a view. Which one is a *type* question, so `jr-sema` answers it.
    Sequence(ExprId),
    /// `a..b`, half-open: `0..n` runs `n` times.
    Range {
        /// The first value.
        start: ExprId,
        /// One past the last value.
        end: ExprId,
    },
}

/// A statement node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stmt {
    /// A block `{ stmts }`.
    Block(Vec<StmtId>, Span),
    /// A local variable declaration.
    Local(LocalId, Span),
    /// A nested item declaration (e.g. a nested procedure).
    Item(ItemId, Span),
    /// An expression statement.
    Expr(ExprId, Span),
    /// An assignment statement.
    Assign {
        /// Left-hand side.
        lhs: ExprId,
        /// The assignment operator.
        op: AssignOp,
        /// Right-hand side.
        rhs: ExprId,
        /// Span of the whole statement.
        span: Span,
    },
    /// `q, ok := f();` — declares several locals from one multi-result call (ADR-0052 §2).
    ///
    /// A separate variant rather than a generalised [`Stmt::Local`] because a *list* of targets is a
    /// different shape, and every exhaustive match over `Stmt` should be forced to consider it.
    LocalTuple {
        /// One entry per result position: `Some(local)` for a name, `None` for a `_` discard.
        ///
        /// A discard is `None` rather than a local nothing reads, which is what keeps `_` out of the
        /// resolve map entirely — it is a *hole* recognised positionally, not a binding (ADR-0052
        /// §3).
        targets: Vec<Option<LocalId>>,
        /// The call producing the results. Only a call is legal here; sema refuses anything else.
        call: ExprId,
        /// Span of the whole statement.
        span: Span,
    },
    /// `q, ok = f();` — assigns to several existing places (ADR-0052 §2).
    AssignTuple {
        /// One entry per result position: `Some(expr)` for a place, `None` for a `_` discard.
        targets: Vec<Option<ExprId>>,
        /// The call producing the results.
        call: ExprId,
        /// Span of the whole statement.
        span: Span,
    },
    /// `if cond { then } [else { else_ }]`
    If {
        /// The condition.
        cond: ExprId,
        /// The then-body (a block statement).
        then: StmtId,
        /// The optional else branch.
        else_: Option<StmtId>,
        /// Span of the whole if statement.
        span: Span,
    },
    /// `while cond { body }`
    While {
        /// The loop condition.
        cond: ExprId,
        /// The loop body (a block statement).
        body: StmtId,
        /// The label naming this loop, when one was written (ADR-0049 §2).
        label: Option<Symbol>,
        /// Span of the whole while statement.
        span: Span,
    },
    /// `return [expr];`
    Return(Option<ExprId>, Span),
    /// `return a, b;` — several values at once (ADR-0052 §1).
    ///
    /// A separate variant rather than `Return` with a list, so that every exhaustive match is forced
    /// to decide what a multi-value return means rather than silently handling only the first.
    ReturnTuple(Vec<ExprId>, Span),
    /// `break;` or `break label;` (ADR-0049 §2).
    ///
    /// The label is a [`Symbol`] and deliberately **not** resolved here or by `ResolveMap`: it
    /// names a *loop*, not a value, and putting it in the expression-name map would make
    /// `break outer` look like a name reference to anything reading that map. `jr-mir` resolves it
    /// against its own loop stack, which is the only place a loop's identity exists.
    Break(Option<Symbol>, Span),
    /// `continue;` or `continue label;` (ADR-0049 §2).
    Continue(Option<Symbol>, Span),
    /// `for x: iterable { … }` (ADR-0049 §1).
    For {
        /// The element variable — a real local, so it obeys the ordinary promotion rules.
        value: LocalId,
        /// The index variable, when the two-name form was written.
        ///
        /// `None` for `for x: buf`, where the induction variable is synthesised and unnameable.
        index: Option<LocalId>,
        /// What is being iterated.
        iterable: ForIterable,
        /// `true` for `for < x: buf` (ADR-0049 §1).
        reverse: bool,
        /// The loop body.
        body: StmtId,
        /// The label naming this loop, when one was written.
        label: Option<Symbol>,
        /// Span of the whole statement.
        span: Span,
    },
    /// `defer stmt;` (ADR-0049 §3).
    ///
    /// Holds the deferred statement unlowered-in-place: `jr-mir` emits it before *every* terminator
    /// that leaves the enclosing scope, so the same `StmtId` is lowered once per exit path.
    Defer(StmtId, Span),
    /// `push_context { … }` (ADR-0063) — a block whose statements run against a copy of the context.
    ///
    /// Holds the block statement. `jr-mir` copies the context into a fresh slot on entry, points
    /// `context` at the copy for the block's duration, and restores the outer pointer on exit — a
    /// lowering-time swap with no new MIR node. A separate variant rather than a flag on
    /// [`Stmt::Block`], so every exhaustive match decides what a context scope means.
    PushContext(StmtId, Span),
    /// `switch e { case v; … else; … }` (ADR-0067).
    ///
    /// The arms are values compared with `==`, not patterns (§2), so each carries an expression and the
    /// block it runs. The `else` arm's `value` is `None` — an absent value *is* the catch-all (§4), which
    /// is why no separate variant or flag distinguishes it.
    Switch {
        /// The value being matched.
        value: ExprId,
        /// The arms, in source order. Order matters: it is the order the comparisons are tried, and the
        /// order a duplicate-case diagnostic reports against.
        arms: Vec<SwitchArm>,
        /// Span of the whole statement.
        span: Span,
    },
    /// `#insert "…";` — statements parsed from a string literal (ADR-0072 §1).
    ///
    /// Holds the statements the inserted text lowered to, **in the enclosing scope**. Deliberately not a
    /// [`Stmt::Block`], which would be wrong twice over: `jr-mir` treats a block as a *defer scope*, so a
    /// `defer` inside an insert would run at the insert's end rather than the enclosing body's; and
    /// lowering pushes a *name* scope for a block, so a local the insert declares would be invisible
    /// afterwards — which is exactly the thing ADR-0072 §1 promises works (`#insert "n := 1;"` then
    /// `exit(n)`).
    ///
    /// Its own variant rather than a flag, so every exhaustive match decides what an insert means. The
    /// statements are already lowered: nothing downstream can tell they came from a string, and nothing
    /// downstream needs to — which is the evidence §1's "lowered where it is written" is the right model.
    Insert {
        /// The lowered statements, in order.
        ///
        /// **Empty while a computed operand is unexpanded** (ADR-0073): `#insert S;` whose operand is a
        /// constant has no statements until the operand pre-pass evaluates `S` to a string and lowering
        /// runs again. `operand` being `Some` is what distinguishes that pending state from a genuinely
        /// empty literal insert (`#insert "";`, `operand: None`, no statements) — and `jr-mir`'s `scan`
        /// refuses a body still holding a pending one, so empty `stmts` can never be mistaken for "insert
        /// nothing" (the well-typed-placeholder miscompile AGENTS.md names).
        stmts: Vec<StmtId>,
        /// The **computed** operand expression, when the insert has one (ADR-0073 §1).
        ///
        /// `None` for a literal `#insert "…";` — its text is parsed and lowered in place (ADR-0072), so
        /// there is no operand expression. `Some` for `#insert <expr>;`, holding the operand lowered as
        /// an ordinary expression so it *resolves and type-checks* like any other — which is how
        /// `#insert undefined;` becomes an unresolved-name error rather than a bare refusal. The operand
        /// pre-pass evaluates it to a string; until it does, `jr-mir`'s `scan` refuses the body.
        operand: Option<ExprId>,
        /// Span of the `#insert` directive — **shared by every statement in `stmts`** (ADR-0072 §2).
        ///
        /// The directive is where that code entered the program, and it is the only span for it that is
        /// in range: `jr-diag` *clamps* an out-of-range offset rather than rejecting it, so a synthesized
        /// span would silently underline source the user never wrote.
        span: Span,
    },
    /// Error recovery placeholder.
    Error(Span),
}

/// One arm of a [`Stmt::Switch`] (ADR-0067 §1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchArm {
    /// The value this arm matches, or `None` for the `else` catch-all (ADR-0067 §4).
    ///
    /// A *value*, not a pattern: it is compared with `==`, which is why a bare `.RED` works here
    /// unchanged — the scrutinee's type is the expected type it resolves against (§2).
    pub value: Option<ExprId>,
    /// The statements this arm runs, as a block.
    pub body: StmtId,
    /// Span of the arm's header, for a diagnostic that points at one arm.
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Local variables
// ---------------------------------------------------------------------------

/// A local variable declaration inside a body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Local {
    /// The variable name.
    pub name: Symbol,
    /// Span of the name token.
    pub name_span: Span,
    /// The explicit type annotation, if present.
    pub ty: Option<TypeRefId>,
    /// The initialiser expression, if present.
    pub init: Option<ExprId>,
    /// `true` if the initialiser is `---` (explicit non-initialisation).
    pub uninit: bool,
    /// `true` for `using q: Point;` — the type's fields resolve unqualified after this point
    /// (ADR-0050 §1). Always has an explicit `ty`, because promotion needs the field list and
    /// the parser refuses the inferred form (E0128).
    pub using: bool,
    /// Span of the whole declaration.
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Bodies
// ---------------------------------------------------------------------------

/// A procedure body: the arenas for all nodes inside one procedure.
#[derive(Debug, Clone)]
pub struct Body {
    /// All expressions in this body.
    pub exprs: Vec<Expr>,
    /// Spans for each expression (parallel to `exprs`).
    pub expr_spans: Vec<Span>,
    /// All statements in this body.
    pub stmts: Vec<Stmt>,
    /// All local variables in this body.
    pub locals: Vec<Local>,
    /// All type references in this body.
    ///
    /// Only a local's annotation lives here. Parameter, return and field types
    /// are in [`FileHir::type_refs`](crate::FileHir::type_refs), and the two
    /// arenas both start at index 0 — so a `TypeRefId` says nothing about which
    /// one it belongs to. Which arena to use follows from where the id came from.
    pub type_refs: Vec<TypeRef>,
    /// The root statement (always a `Stmt::Block`).
    pub root: StmtId,
}

impl Body {
    /// Returns the span of an expression.
    pub fn expr_span(&self, id: ExprId) -> Span {
        self.expr_spans[id.index()]
    }

    /// Returns the expression for an ID.
    pub fn expr(&self, id: ExprId) -> &Expr {
        &self.exprs[id.index()]
    }

    /// Returns the statement for an ID.
    pub fn stmt(&self, id: StmtId) -> &Stmt {
        &self.stmts[id.index()]
    }

    /// Returns the local for an ID.
    pub fn local(&self, id: LocalId) -> &Local {
        &self.locals[id.index()]
    }

    /// Returns the type reference for an ID.
    pub fn type_ref(&self, id: TypeRefId) -> &TypeRef {
        &self.type_refs[id.index()]
    }
}

// ---------------------------------------------------------------------------
// Procedures
// ---------------------------------------------------------------------------

/// A procedure definition.
///
/// A procedure has either a `body` or `foreign` info, never both and never
/// neither. If the source gave neither, `body` is `None`, `foreign` is `None`,
/// and a diagnostic (E0203) is emitted.
#[derive(Debug, Clone)]
pub struct Proc {
    /// The parameters.
    pub params: Vec<Param>,
    /// `true` for a `#c_call` procedure, which receives no implicit context (ADR-0057 §3).
    ///
    /// Read by `jr-sema` to set `ContextKind`, which ADR-0001 put in every procedure type in the
    /// slice precisely so this wave would not have to re-type anything. Every `#foreign` procedure is
    /// implicitly `#c_call`, and sema decides that from `foreign` rather than from this flag — so
    /// writing both is redundant rather than contradictory.
    pub c_call: bool,
    /// `true` for a `#no_abc` procedure, whose indexing emits no bounds check (ADR-0058 §3).
    ///
    /// Read by `jr-mir`, which is where the check is emitted, so this flag is the whole
    /// representation of the opt-out — no `Projection`, `Expr` or `Statement` carries it.
    ///
    /// **That is why it is on the procedure**, and ADR-0058 §3 amends ADR-0003 to say so: the
    /// original decision put the opt-out at an individual index, which would have meant threading a
    /// flag from here to `Projection::Index` through every pass and both back ends. A flag some of
    /// those consumers ignored would be a bounds check silently restored or silently dropped.
    pub no_abc: bool,
    /// `true` for a `#expand` procedure — a **macro** (ADR-0090 §1).
    ///
    /// A call to one is **spliced**: the macro's statements are lowered into the *caller's* scope rather
    /// than the call being emitted, which is why this is a flag on the declaration rather than anything
    /// on the call. Deliberately **unhygienic**, matching Jai and matching `#insert`'s existing splice
    /// (ADR-0072 §1): the body sees and may modify the caller's locals, which is what makes a macro
    /// useful for a custom loop. A macro therefore emits no MIR of its own, exactly as a `$T` template
    /// does not (ADR-0090 §3).
    pub expand: bool,
    /// The `#modify { … }` predicate's **source text**, if this procedure has one (ADR-0093 §1).
    ///
    /// A compile-time predicate over an instantiation: `false` refuses the call. Held as *text* for the
    /// reason a macro's body is (ADR-0091 §1) — it is evaluated per instantiation, against that
    /// instantiation's bindings, by generating a body from it; lowering it once against the *template*
    /// would resolve `T` where nothing binds it.
    /// The `ProcId` of the **lowered** `#modify` predicate, if this procedure has one (ADR-0094 §1).
    ///
    /// The block is lowered at the *template* as an ordinary synthetic procedure — no parameters, returning
    /// `bool` — so it goes through the same body lowering every procedure does. Each instantiation then
    /// appends a **clone** of it with that instantiation's bindings, exactly as the instantiation itself is
    /// cloned (ADR-0082 §2), which is why evaluating it needs no new machinery: the clone is an ordinary
    /// procedure and `file_consts` already evaluates one as a `#run`-shaped target.
    ///
    /// Replaces the source text ADR-0093 §1 carried: text was the right shape when the block had to be
    /// re-lowered per instantiation, and lowering it once at the template makes it unnecessary.
    pub modify: Option<ProcId>,
    /// The `@note`s this procedure carries, in source order (ADR-0098 §1).
    ///
    /// Metadata a **metaprogram** reads — `@deprecated`, `@requires "x"` — not an instruction to the
    /// compiler, which is why it is a list of `(name, payload)` beside the directive flags rather than one
    /// of them. Empty for a declaration with none, which is every existing program.
    ///
    /// Carried on the declaration because that is what a note is attached to (ADR-0098 §2): allowing one on
    /// an arbitrary expression would raise "what does a note on `a + b` mean", which nothing needs.
    pub notes: Vec<(Symbol, Option<String>)>,
    /// The return type, if present.
    pub ret: Option<TypeRefId>,
    /// The body, if this is not a foreign procedure.
    pub body: Option<BodyId>,
    /// Foreign binding info, if this is a `#foreign` procedure.
    pub foreign: Option<ForeignInfo>,
    /// Span of the whole procedure.
    pub span: Span,
    /// **Always empty.** Kept for a future lowering that gives a procedure its
    /// own type-reference arena.
    ///
    /// Parameter and return types live in
    /// [`FileHir::type_refs`](crate::FileHir::type_refs), because lowering
    /// allocates them there. A `TypeRefId` taken from `Param::ty` or `Proc::ret`
    /// must be resolved against that arena, never against this one — indexing the
    /// wrong arena reads an unrelated node rather than failing.
    pub type_refs: Vec<TypeRef>,
}

/// A procedure parameter.
#[derive(Debug, Clone)]
pub struct Param {
    /// The parameter name.
    pub name: Symbol,
    /// Span of the name token.
    pub name_span: Span,
    /// The parameter type.
    pub ty: Option<TypeRefId>,
    /// `true` for `using p: Point` — the type's fields resolve unqualified in the body
    /// (ADR-0050 §1).
    pub using: bool,
    /// `true` for `$N: s64` — a comptime-value parameter, polymorphic over a compile-time-known
    /// value (ADR-0087 §1).
    ///
    /// The value-side counterpart of a `$T` type parameter's mark. Its *type* is ordinary and fully
    /// known (`s64`), so unlike `$T` the body type-checks at template time; only its *value* varies,
    /// and a procedure with one is a **template** with no concrete signature until instantiation.
    pub comptime: bool,
    /// `true` for `args: ..T` — a variadic parameter (ADR-0138 §1). The parameter's type is
    /// `[]T`; the caller's trailing arguments are packed into a stack-allocated array and a
    /// view of that array is passed. Must be the **last** parameter, and there is at most one.
    pub variadic: bool,
    /// The default value, for `b: s64 = 10` (ADR-0053 §2).
    ///
    /// Lowered as an ordinary expression so the tree stays faithful; **sema refuses anything but a
    /// literal**, with a message saying why — the value must be one because const-eval runs
    /// downstream of signatures (ADR-0018 §3), so a signature cannot depend on a computed constant.
    pub default: Option<ExprId>,
}

/// Foreign procedure binding information.
#[derive(Debug, Clone)]
pub struct ForeignInfo {
    /// The library constant name (e.g. `libc`).
    pub library: Option<Symbol>,
    /// The external symbol name string (e.g. `"write"`).
    pub symbol: Option<String>,
    /// Span of the `#foreign` attribute.
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

/// A struct type definition.
/// Which aggregate form a [`Struct`] node is (ADR-0068 §2).
///
/// All three share one arena, for the reason [`Struct::kind`] documents. They differ in *semantics*
/// rather than in a detail, which is why each has its own keyword rather than an attribute on one form
/// (ADR-0068 §1, following ADR-0043 §1's precedent for `enum_flags`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregateKind {
    /// `struct { … }` — fields at increasing offsets.
    Struct,
    /// `union { … }` — every field at offset 0, **untagged**, so reading a field other than the one
    /// last written reinterprets bits (ADR-0045 §1). Costs nothing and checks nothing.
    Union,
    /// `variant { … }` — a leading tag plus the union of the cases (ADR-0068 §3).
    ///
    /// A write sets the tag and a read checks it, so reading the wrong field traps rather than
    /// reinterpreting. Bigger than the equivalent `union` by the tag, and that size cost is the
    /// choice a program makes by writing this form (ADR-0045 §1's surviving objection).
    Variant,
}

/// A struct, union or variant type definition.
///
/// One node for all three forms, distinguished by [`Struct::kind`] — see that field for why the
/// sharing is load-bearing rather than a convenience.
#[derive(Debug, Clone)]
pub struct Struct {
    /// Which of the three aggregate forms this is (ADR-0045 §4, ADR-0068 §2).
    ///
    /// A field on the shared node rather than a second arena, and that is **load-bearing**: a
    /// `DeclId` is `(file, index-within-its-arena)` and says nothing about *which* arena
    /// (ADR-0041 §4a). A separate `unions: Vec<Union>` would make a struct at index 0 and a
    /// union at index 0 the same `DeclId`, and they share `Pool::struct_fields` — so the two
    /// field lists would silently overwrite each other. One arena makes that unrepresentable,
    /// which is why `variant` joined the same arena rather than getting a third.
    ///
    /// An **enum rather than the `is_union: bool` it replaced** (ADR-0068 §2): three forms do not
    /// fit in a bool, and two bools would admit the nonsense "union and variant". Every reader
    /// becomes an exhaustive match, so a fourth form is a compile error at each site that must
    /// decide rather than a `false` silently meaning "struct".
    pub kind: AggregateKind,
    /// The fields.
    pub fields: Vec<Field>,
    /// The type parameters of a parameterised struct — `[T]` for `struct($T) { … }` (ADR-0085 §3).
    ///
    /// **Empty for an ordinary struct**, which is not parameterised. A non-empty list makes this a
    /// type constructor: sema does not intern it as a type directly, but instantiates it per
    /// [`TypeRef::Apply`] reference, binding these variables to the arguments — the type-side mirror
    /// of a polymorphic procedure's [`Proc`] poly variables.
    pub poly_vars: Vec<Symbol>,
    /// Span of the whole struct.
    pub span: Span,
    /// **Always empty**, for the same reason as
    /// [`Proc::type_refs`](crate::Proc::type_refs): field types are allocated in
    /// [`FileHir::type_refs`](crate::FileHir::type_refs).
    pub type_refs: Vec<TypeRef>,
    /// The `#soa(N)` count expression, when the struct carries the attribute (ADR-0147 §1).
    ///
    /// An [`ExprId`] for the reason a field's `#align` operand is one: whether it is a usable count
    /// is a semantic judgement, so `jr-hir` records the expression and `jr-sema` reads it — the same
    /// split an array length uses (ADR-0070 §1).
    ///
    /// When present, `jr-sema` wraps **every** field's type in `[N]T` while resolving the body, so
    /// nothing downstream of resolution sees anything but an ordinary struct of arrays.
    pub soa: Option<ExprId>,
}

/// An enum type definition.
#[derive(Debug, Clone)]
pub struct Enum {
    /// `true` for `enum_flags`, which numbers by powers of two (ADR-0043 §2).
    ///
    /// One field rather than two `ConstValue` variants: the two forms differ *only* in the
    /// numbering rule and which operators apply, and separating them here would duplicate
    /// everything they share — the member list, the namespace, the nominal identity.
    pub flags: bool,
    /// The members, in declaration order.
    ///
    /// Order is load-bearing: auto-numbering counts from 0 in this order (ADR-0041 §3).
    pub members: Vec<EnumMember>,
    /// Span of the whole `enum { … }`.
    pub span: Span,
}

/// One enum member.
#[derive(Debug, Clone)]
pub struct EnumMember {
    /// The member's name.
    pub name: Symbol,
    /// Span of the name token, for a diagnostic that points at the member.
    pub name_span: Span,
    /// The explicit value, when the member was written `NAME :: value`.
    ///
    /// `None` means auto-numbered — one past the previous member, or 0 for the first. The
    /// *value* is resolved in `jr-sema`, not here, for the same reason an array length's
    /// diagnostic lives there (ADR-0039 §3a): `jr-hir` can reach the literal token but
    /// rejecting a bad one is a semantic judgement.
    pub value: Option<ExprId>,
}

/// A struct field.
#[derive(Debug, Clone)]
pub struct Field {
    /// The field name.
    pub name: Symbol,
    /// Span of the name token.
    pub name_span: Span,
    /// The field type.
    pub ty: Option<TypeRefId>,
    /// `true` for `using base: Point;` — the field's own type's fields become reachable
    /// through the enclosing struct (ADR-0050 §1).
    ///
    /// The field stays a **real field** at a real offset, so `jr-pool` needs nothing: `using`
    /// is purely a resolution feature (ADR-0050 §4).
    pub using: bool,
    /// The `#align N` operand, when the field carries one (ADR-0144 §3).
    ///
    /// An [`ExprId`] rather than a number: the operand may be an integer literal or a name that
    /// resolves to a literal-valued constant, and deciding which is a semantic judgement — the
    /// same split an array length uses (ADR-0070 §1), so `jr-hir` records the expression and
    /// `jr-sema` reads it.
    pub align: Option<ExprId>,
    /// The `#place N` operand, when the field carries one (ADR-0144 §4).
    pub place: Option<ExprId>,
}

// ---------------------------------------------------------------------------
// Items
// ---------------------------------------------------------------------------

/// The kind of a file-level item.
#[derive(Debug, Clone)]
pub enum ItemKind {
    /// `name :: value` — a compile-time constant.
    Const {
        /// The constant value.
        value: ConstValue,
    },
    /// `name := value` or `name: T [= value]` — a variable.
    Var {
        /// The explicit type annotation, if present.
        ty: Option<TypeRefId>,
        /// The initialiser expression, if present.
        init: Option<ExprId>,
        /// `true` if the initialiser is `---`.
        uninit: bool,
    },
    /// `#import "path";`
    Import {
        /// The module path string (without quotes).
        path: String,
        /// Span of the path string literal.
        path_span: Span,
    },
    /// A top-level `#run expr;`
    Run {
        /// The expression to run at compile time.
        expr: ExprId,
    },
}

/// The value of a compile-time constant.
#[derive(Debug, Clone)]
pub enum ConstValue {
    /// A procedure: `name :: (params) -> T { body }`.
    Proc(ProcId),
    /// A struct type: `name :: struct { fields }`.
    Struct(StructId),
    /// A union type: `name :: union { fields }` (ADR-0045).
    ///
    /// Its own `ConstValue` variant even though it indexes the *same* arena `Struct` does,
    /// because a consumer deciding what to intern needs to know which — and `Struct::kind`
    /// would make that a second lookup rather than a match.
    Union(StructId),
    /// A variant type: `name :: variant { fields }` (ADR-0068 §1).
    ///
    /// Its own variant for the reason `Union` has one: what a consumer interns differs, and a match
    /// says so where a field lookup would not.
    Variant(StructId),
    /// An operator overload: `operator + :: (a: T, b: T) -> T { … }` (ADR-0048 §1).
    ///
    /// A `ProcId` like [`ConstValue::Proc`], because an overload **is** an ordinary procedure —
    /// same arena, same signature resolution, same lowering, same inliner eligibility. The
    /// operator is carried alongside so that sema can check ADR-0048 §2's permitted set and
    /// register the overload under the operator rather than under a name a user could write.
    Operator(ProcId, BinOp),
    /// An enum type: `name :: enum { members }` (ADR-0041).
    Enum(EnumId),
    /// An arbitrary expression: `name :: expr`.
    Expr(ExprId),
}

/// A file-level item.
#[derive(Debug, Clone)]
pub struct Item {
    /// The declared name, if any (top-level `#run` has no name).
    pub name: Option<Symbol>,
    /// Whether an importing file can see this declaration (ADR-0054 §1).
    ///
    /// `true` unless a `#scope_module` marker precedes it with no `#scope_export` in between.
    /// Computed during lowering by walking items in source order, so it is a function of **this
    /// file's own HIR** — which is what lets `jr-db`'s `file_exports` stay dependent on `file_hir`
    /// alone and never cycle when two modules import each other (ADR-0054 §3).
    ///
    /// The declaring file's own scope is *never* filtered by this, so a hidden name resolves,
    /// type-checks and answers hover inside its own file exactly as before.
    pub exported: bool,
    /// **A nested-hoisted item** (ADR-0134): `X :: <value>;` written inside a procedure body.
    /// The item lives in `items` — so it is checked, lowered and linked like any other — but
    /// its name is **not** in `hir.scope`. Visibility is via the enclosing body's scope stack
    /// plus the sibling-scope injection every nested proc's body receives. Two nested items
    /// sharing a name across different enclosing procs are legal by construction, which is
    /// what this flag lets `check_duplicates` distinguish from a real user-visible collision.
    pub nested: bool,
    /// Span of the whole item.
    pub span: Span,
    /// Span of the name token, if any.
    pub name_span: Span,
    /// The item kind.
    pub kind: ItemKind,
}

// ---------------------------------------------------------------------------
// Item scope (for name resolution)
// ---------------------------------------------------------------------------

/// A flat name→item mapping for one file, used during name resolution.
///
/// Callers pass slices of `(&str, &ItemScope)` to [`resolve`](fn@crate::resolve) to
/// provide the scopes of imported modules.
#[derive(Debug, Clone, Default)]
pub struct ItemScope {
    /// Maps interned name → item ID.
    pub names: rustc_hash::FxHashMap<Symbol, ItemId>,
    /// Names the file declares but does **not** export (ADR-0054 §2).
    ///
    /// Empty for a file's own scope, and populated only in the *exported* scope a module hands to an
    /// importer — so a failed lookup can tell "this module has no such name" from "this module hides
    /// it", and report the second as E0253 rather than as an unresolved name with a spelling
    /// suggestion the reader cannot act on.
    ///
    /// Carried here rather than passed alongside because the two travel together everywhere: a
    /// consumer holding a scope can always ask, and there is no way to pass one without the other.
    pub hidden: rustc_hash::FxHashSet<Symbol>,
}

impl ItemScope {
    /// Creates an empty scope.
    pub fn new() -> Self {
        Self::default()
    }

    /// Looks up a name.
    pub fn get(&self, name: Symbol) -> Option<ItemId> {
        self.names.get(&name).copied()
    }

    /// Inserts a name→item mapping.
    pub fn insert(&mut self, name: Symbol, id: ItemId) {
        self.names.insert(name, id);
    }

    /// Records that this scope's file declares `name` but does not export it (ADR-0054 §2).
    pub fn hide(&mut self, name: Symbol) {
        self.hidden.insert(name);
    }

    /// Whether `name` is declared by this scope's file but hidden from importers.
    #[must_use]
    pub fn is_hidden(&self, name: Symbol) -> bool {
        self.hidden.contains(&name)
    }
}

// ---------------------------------------------------------------------------
// The whole-file HIR
// ---------------------------------------------------------------------------

/// The evaluated text of every computed `#insert` operand in one file (ADR-0073 §1).
///
/// Produced by `jr-db`'s `insert_operands` pre-pass — which evaluates each operand against the
/// *unexpanded* HIR — and consumed by a second lowering that fills each pending [`Stmt::Insert`]'s
/// statements. Empty for a file with no computed insert, which is every file today, so ordinary lowering
/// passes `InsertOperands::default()` and behaves exactly as before.
///
/// **Keyed by the directive's [`Span`], not by an `ExprId` or `StmtId`** — and that is load-bearing, not
/// incidental. Expanding one insert adds statements and expressions to the body, so a *later* insert's
/// operand id differs between the pass that computed the value and the pass that consumes it; keying by
/// id would attach the wrong text to the wrong insert, a miscompile that type-checks. A `Span` comes from
/// source, so it is invariant across both lowerings.
#[derive(Debug, Clone, Default)]
pub struct InsertOperands {
    by_span: std::collections::HashMap<Span, String>,
}

impl InsertOperands {
    /// An empty map: no computed operand has a value, so every one stays pending and `jr-mir` refuses it.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the evaluated text of the operand whose directive has this span.
    pub fn set(&mut self, directive: Span, text: String) {
        self.by_span.insert(directive, text);
    }

    /// The evaluated text for the `#insert` at `directive`, if the pre-pass computed one.
    #[must_use]
    pub fn get(&self, directive: Span) -> Option<&str> {
        self.by_span.get(&directive).map(String::as_str)
    }

    /// Whether nothing has been evaluated — the ordinary case, lowered without expansion.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_span.is_empty()
    }
}

/// The complete HIR for one source file.
///
/// Owns all arenas. After lowering, call [`resolve`](fn@crate::resolve) to fill in name
/// resolution results.
///
/// `Clone` because the instantiation pass builds an *expanded* copy with appended procedures
/// (ADR-0082 §2); an ordinary compile never clones one.
#[derive(Debug, Clone)]
pub struct FileHir {
    /// All file-level items, in source order.
    pub items: Vec<Item>,
    /// The name→item index for this file.
    pub scope: ItemScope,
    /// All procedure definitions.
    pub procs: Vec<Proc>,
    /// All struct definitions.
    pub structs: Vec<Struct>,
    /// All enum definitions.
    pub enums: Vec<Enum>,
    /// All procedure bodies.
    pub bodies: Vec<Body>,
    /// Top-level expressions (for `ItemKind::Const { value: Expr }` and
    /// `ItemKind::Var` initialisers and `ItemKind::Run`).
    pub exprs: Vec<Expr>,
    /// Spans for top-level expressions (parallel to `exprs`).
    pub expr_spans: Vec<Span>,
    /// Top-level type references (for `ItemKind::Var` type annotations).
    pub type_refs: Vec<TypeRef>,
    /// Per-procedure polymorphic bindings, for instantiated procedures appended to an expanded HIR
    /// (ADR-0082 §2).
    ///
    /// Empty for an ordinary file. An **instantiation** — a clone of a `$T` procedure appended to `procs`
    /// — maps its `ProcId` to `(variable, concrete type)`, so the signature and check phases resolve its
    /// `$T`/`T` to the concrete type via the `type_bindings` map. Carried on the HIR rather than threaded
    /// through `check_file`'s parameters because only the expanded tree ever has entries, and every other
    /// caller would pass an empty map.
    pub proc_bindings: Vec<(ProcId, Symbol, PoolId)>,
    /// Where each appended instantiation was demanded, for a diagnostic's backtrace (ADR-0128).
    ///
    /// Empty for an ordinary file, exactly as [`FileHir::proc_bindings`] is, and carried on the HIR for
    /// the same reason: only the expanded tree ever has entries, so threading it through `check_file`'s
    /// parameters would make every other caller pass an empty vector.
    ///
    /// `jr-sema` reads this to stamp every diagnostic produced while checking an instantiation's body,
    /// which is what turns "`bool` does not support `+`" — reported against a template a user may never
    /// have opened — into that plus "in instantiation of `add($T = bool)`" at the call they wrote.
    pub instantiation_sites: Vec<(ProcId, crate::instantiate::InstantiationSite)>,
    /// Per-procedure **comptime-value** bindings, for instantiated procedures (ADR-0089 §1).
    ///
    /// The value-side counterpart of [`FileHir::proc_bindings`]: an instantiation of a `$N` template maps
    /// its `ProcId` to `(parameter name, baked value)`, so `jr-sema` can resolve an array length that
    /// *names* that parameter — `buf: [N]s64` inside the instantiation — by reading the value the
    /// const-eval pre-pass already produced (ADR-0088 §2).
    ///
    /// **No evaluation happens in sema because of this**, which is why ADR-0039 §3a's constraint still
    /// holds: the value arrives through the HIR, already interned, exactly as a bound `$T` does.
    pub param_values: Vec<(ProcId, Symbol, PoolId)>,
    /// Each instantiation's cloned `#modify` predicate: `(instantiation, predicate clone)` (ADR-0094 §2).
    ///
    /// Empty for an ordinary file. `jr-db` evaluates each predicate clone as a `#run`-shaped target and
    /// refuses the guarded instantiation when it answers `false` — so the pairing is what says *which*
    /// instantiation a `false` rejects.
    pub modify_predicates: Vec<(ProcId, ProcId)>,
    /// Every `#modify` predicate procedure and the **type-variable names** its guarded template introduces
    /// (ADR-0094 §1).
    ///
    /// A predicate is a synthetic no-parameter procedure, so it has no `$T` of its own — but its body says
    /// `type_info(T)`, where `T` is the *guarded template's* variable. Without this, checking the
    /// template's predicate reported E0261 "needs a type", because sema had no way to know `T` was a
    /// variable awaiting a binding rather than a name that resolves to nothing. Sema withholds on these
    /// exactly as it withholds inside the template itself (ADR-0092 §1).
    pub predicate_vars: Vec<(ProcId, Vec<Symbol>)>,
}

impl FileHir {
    /// Returns the item for an ID.
    pub fn item(&self, id: ItemId) -> &Item {
        &self.items[id.index()]
    }

    /// Returns the procedure for an ID.
    pub fn proc(&self, id: ProcId) -> &Proc {
        &self.procs[id.index()]
    }

    /// Returns the struct for an ID.
    pub fn struct_def(&self, id: StructId) -> &Struct {
        &self.structs[id.index()]
    }

    /// Returns the enum definition for an ID.
    pub fn enum_def(&self, id: EnumId) -> &Enum {
        &self.enums[id.index()]
    }

    /// Returns the body for an ID.
    pub fn body(&self, id: BodyId) -> &Body {
        &self.bodies[id.index()]
    }

    /// Returns the top-level expression for an ID.
    pub fn expr(&self, id: ExprId) -> &Expr {
        &self.exprs[id.index()]
    }

    /// Returns the span of a top-level expression.
    pub fn expr_span(&self, id: ExprId) -> Span {
        self.expr_spans[id.index()]
    }

    /// Returns the top-level type reference for an ID.
    pub fn type_ref(&self, id: TypeRefId) -> &TypeRef {
        &self.type_refs[id.index()]
    }

    /// Resolves a name in the file scope.
    pub fn resolve_name(&self, name: Symbol) -> Option<ItemId> {
        self.scope.get(name)
    }

    /// Returns the interner-resolved text of a symbol, for diagnostics.
    pub fn symbol_text<'a>(&self, sym: Symbol, interner: &'a Interner) -> &'a str {
        interner.resolve(sym)
    }

    /// Returns the export scope for this file.
    ///
    /// The export scope is the set of names this file makes available to
    /// importers. Pass this to [`resolve`](fn@crate::resolve) as part of the `imports`
    /// slice when resolving a file that imports this one.
    ///
    /// **Honours `#scope_module`** (ADR-0054 §1). Export is the default, so a file with no marker
    /// exports everything exactly as it did before — which is what ADR-0014 §2 promised and the whole
    /// corpus relies on. A hidden name is *recorded* in the returned scope's `hidden` set rather than
    /// merely omitted, so a use of it reports E0253 "not exported" instead of "unresolved name".
    ///
    /// This used to return the full file scope with a doc comment calling it a "W1 temporary
    /// over-share". Returning it unfiltered while `jr-db`'s `file_exports` filtered would have been
    /// **two answers to one question** — and the one a test harness happened to call would decide
    /// whether the test saw encapsulation. So this filters too, by the same rule.
    #[must_use]
    pub fn export_scope(&self) -> ItemScope {
        let mut exported = ItemScope::new();
        for (name, item) in &self.scope.names {
            if self
                .items
                .get(item.index())
                .is_some_and(|declaration| declaration.exported)
            {
                exported.insert(*name, *item);
            } else {
                exported.hide(*name);
            }
        }
        exported
    }
}
