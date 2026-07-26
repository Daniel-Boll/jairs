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
    /// An inline struct type `struct { ... }`.
    Struct(StructId),
    /// A type that could not be lowered (error recovery).
    Error,
}

// ---------------------------------------------------------------------------
// Literals
// ---------------------------------------------------------------------------

/// A literal value as it appears in source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Literal {
    /// An integer literal.
    ///
    /// `value` is the parsed value (clamped to `u64::MAX` on overflow, with
    /// `overflowed` set). `radix` is 10, 16, 2, or 8.
    Int {
        /// The parsed integer value.
        value: u64,
        /// The radix (10, 16, 2, or 8).
        radix: u32,
        /// `true` if the literal value exceeded `i64::MAX` (s64 range).
        overflowed: bool,
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
    /// `pointer.*`
    Deref(ExprId, Span),
    /// `---` (explicit non-initialisation)
    Uninit(Span),
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
            Expr::Deref(_, span) => *span,
            Expr::Uninit(span) => *span,
            Expr::Run(_, span) => *span,
            Expr::Directive { span, .. } => *span,
            Expr::Error(span) => *span,
        }
    }
}

// ---------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------

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
        /// Span of the whole while statement.
        span: Span,
    },
    /// `return [expr];`
    Return(Option<ExprId>, Span),
    /// `break;`
    Break(Span),
    /// `continue;`
    Continue(Span),
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
    /// Type references used in the signature (shared with the body's arena
    /// when a body exists, or stored here for foreign procs).
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
    /// The fields.
    pub fields: Vec<Field>,
    /// Span of the whole struct.
    pub span: Span,
    /// Type references used in field types.
    pub type_refs: Vec<TypeRef>,
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
