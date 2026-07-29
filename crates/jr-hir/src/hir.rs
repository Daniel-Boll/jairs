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
        /// Span of the length expression, for the diagnostic sema raises.
        ///
        /// Carried because a `TypeRef` has no span of its own (ADR-0013), so without this
        /// E0233 would have to point at the whole declaration.
        len_span: Span,
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
    /// An inline struct type `struct { ... }`.
    Struct(StructId),
    /// An inline union type `union { ... }` (ADR-0045).
    ///
    /// Indexes the same arena `TypeRef::Struct` does — see `Struct::is_union`.
    Union(StructId),
    /// An inline enum type `enum { ... }` (ADR-0041).
    Enum(EnumId),
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
        /// The argument expressions.
        args: Vec<ExprId>,
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
    /// Error recovery placeholder.
    Error(Span),
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
#[derive(Debug, Clone)]
pub struct Struct {
    /// Whether this is a `union` rather than a `struct` (ADR-0045 §4).
    ///
    /// A field on the shared node rather than a second arena, and that is **load-bearing**: a
    /// `DeclId` is `(file, index-within-its-arena)` and says nothing about *which* arena
    /// (ADR-0041 §4a). A separate `unions: Vec<Union>` would make a struct at index 0 and a
    /// union at index 0 the same `DeclId`, and they share `Pool::struct_fields` — so the two
    /// field lists would silently overwrite each other. One arena makes that unrepresentable.
    pub is_union: bool,
    /// The fields.
    pub fields: Vec<Field>,
    /// Span of the whole struct.
    pub span: Span,
    /// **Always empty**, for the same reason as
    /// [`Proc::type_refs`](crate::Proc::type_refs): field types are allocated in
    /// [`FileHir::type_refs`](crate::FileHir::type_refs).
    pub type_refs: Vec<TypeRef>,
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
    /// because a consumer deciding what to intern needs to know which — and `Struct::is_union`
    /// would make that a second lookup rather than a match.
    Union(StructId),
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
}

// ---------------------------------------------------------------------------
// The whole-file HIR
// ---------------------------------------------------------------------------

/// The complete HIR for one source file.
///
/// Owns all arenas. After lowering, call [`resolve`](fn@crate::resolve) to fill in name
/// resolution results.
#[derive(Debug)]
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
    /// **Wave W1 temporary over-share:** everything at file scope is currently
    /// exported. `#scope_file`, `#scope_module`, and `#scope_export` are lexed
    /// but not yet implemented (wave W2). Until W2 lands, this method returns
    /// the full file scope, which means modules have no encapsulation. This is
    /// a known and deliberate temporary state, recorded in ADR-0014 §2.
    pub fn export_scope(&self) -> &ItemScope {
        &self.scope
    }
}
