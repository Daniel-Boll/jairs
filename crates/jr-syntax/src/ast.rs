//! Typed AST accessor layer over the lossless rowan CST.
//!
//! This module provides thin typed wrappers over [`SyntaxNode`] so that
//! downstream crates (`jr-hir`, the LSP, `jr-fmt`) can navigate the tree
//! without pattern-matching on raw [`SyntaxKind`]s.
//!
//! # Design
//!
//! Every AST node type implements [`AstNode`], which provides `can_cast`,
//! `cast`, and `syntax`. Accessors return `Option<T>` or iterators and are
//! *tolerant*: an incomplete tree from a failed parse returns `None` rather
//! than panicking.
//!
//! The private `ast_node!` macro generates the boilerplate for each node type.
//! Accessors are written by hand because they encode grammar knowledge that
//! cannot be derived from the kind alone.

use crate::kind::{SyntaxElement, SyntaxKind, SyntaxKind::*, SyntaxNode, SyntaxToken};

// ---------------------------------------------------------------------------
// AstNode trait
// ---------------------------------------------------------------------------

/// A typed wrapper over a [`SyntaxNode`].
///
/// Implementors correspond one-to-one with node [`SyntaxKind`]s.
pub trait AstNode: Sized {
    /// Returns `true` if `kind` is the kind this type wraps.
    fn can_cast(kind: SyntaxKind) -> bool;

    /// Attempts to cast a [`SyntaxNode`] to this type.
    ///
    /// Returns `None` if the node's kind does not match.
    fn cast(node: SyntaxNode) -> Option<Self>;

    /// The underlying [`SyntaxNode`].
    fn syntax(&self) -> &SyntaxNode;
}

// ---------------------------------------------------------------------------
// Macro for boilerplate
// ---------------------------------------------------------------------------

/// Generates the [`AstNode`] impl and a newtype struct for a single node kind.
macro_rules! ast_node {
    ($name:ident, $kind:ident) => {
        /// A typed wrapper for a [`
        #[doc = stringify!($kind)]
        /// `] node.
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name(SyntaxNode);

        impl AstNode for $name {
            fn can_cast(kind: SyntaxKind) -> bool {
                kind == $kind
            }

            fn cast(node: SyntaxNode) -> Option<Self> {
                if node.kind() == $kind {
                    Some(Self(node))
                } else {
                    None
                }
            }

            fn syntax(&self) -> &SyntaxNode {
                &self.0
            }
        }
    };
}

// ---------------------------------------------------------------------------
// Helper functions for navigating the tree
// ---------------------------------------------------------------------------

/// Returns the first child node of `kind`.
fn child_node<N: AstNode>(parent: &SyntaxNode) -> Option<N> {
    parent.children().find_map(N::cast)
}

/// Returns an iterator over all child nodes of type `N`.
fn child_nodes<'a, N: AstNode + 'a>(parent: &'a SyntaxNode) -> impl Iterator<Item = N> + 'a {
    parent.children().filter_map(N::cast)
}

/// Returns the first child token of `kind`.
fn child_token(parent: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxToken> {
    parent
        .children_with_tokens()
        .filter_map(SyntaxElement::into_token)
        .find(|t| t.kind() == kind)
}

/// Returns the nth child node of type `N` (0-based).
fn nth_child_node<N: AstNode>(parent: &SyntaxNode, n: usize) -> Option<N> {
    parent.children().filter_map(N::cast).nth(n)
}

/// Returns the operand expression inside the first child node of `kind`.
///
/// For a field's layout attributes (ADR-0144 §1), whose node holds the directive token and one
/// expression. Shared by both accessors so that `#align` and `#place` cannot come to disagree
/// about where their operand lives.
fn attr_value(parent: &SyntaxNode, kind: SyntaxKind) -> Option<Expr> {
    parent
        .children()
        .find(|node| node.kind() == kind)
        .and_then(|node| node.children().find_map(Expr::cast))
}

// ---------------------------------------------------------------------------
// Node types
// ---------------------------------------------------------------------------

ast_node!(SourceFile, SOURCE_FILE);
ast_node!(ConstDecl, CONST_DECL);
ast_node!(OperatorDecl, OPERATOR_DECL);
ast_node!(VarDecl, VAR_DECL);
ast_node!(ImportDecl, IMPORT_DECL);
ast_node!(RunDecl, RUN_DECL);
ast_node!(Name, NAME);
ast_node!(Proc, PROC);
ast_node!(ParamList, PARAM_LIST);
ast_node!(Param, PARAM);
ast_node!(RetType, RET_TYPE);
ast_node!(ForeignAttr, FOREIGN_ATTR);
ast_node!(NameType, NAME_TYPE);
ast_node!(PolyType, POLY_TYPE);
ast_node!(PointerType, POINTER_TYPE);
ast_node!(ArrayType, ARRAY_TYPE);
ast_node!(ViewType, VIEW_TYPE);
ast_node!(DynamicArrayType, DYNAMIC_ARRAY_TYPE);
ast_node!(ProcType, PROC_TYPE);
ast_node!(ProcTypeParams, PROC_TYPE_PARAMS);
ast_node!(TypeArguments, TYPE_ARGUMENTS);
ast_node!(StructTypeParams, STRUCT_TYPE_PARAMS);
ast_node!(StructType, STRUCT_TYPE);
ast_node!(UnionType, UNION_TYPE);
ast_node!(VariantType, VARIANT_TYPE);
ast_node!(EnumType, ENUM_TYPE);
ast_node!(MemberList, MEMBER_LIST);
ast_node!(Member, MEMBER);
ast_node!(FieldList, FIELD_LIST);
ast_node!(Field, FIELD);
ast_node!(Block, BLOCK);
ast_node!(DeclStmt, DECL_STMT);
ast_node!(ExprStmt, EXPR_STMT);
ast_node!(AssignStmt, ASSIGN_STMT);
ast_node!(IfStmt, IF_STMT);
ast_node!(ElseBranch, ELSE_BRANCH);
ast_node!(WhileStmt, WHILE_STMT);
ast_node!(ScopeDecl, SCOPE_DECL);
ast_node!(ContextExpr, CONTEXT_EXPR);
ast_node!(CCallAttr, C_CALL_ATTR);
ast_node!(NoAbcAttr, NO_ABC_ATTR);
ast_node!(NamedArg, NAMED_ARG);
ast_node!(Note, NOTE);
ast_node!(ForStmt, FOR_STMT);
ast_node!(RangeExpr, RANGE_EXPR);
ast_node!(DeferStmt, DEFER_STMT);
ast_node!(PushContextStmt, PUSH_CONTEXT_STMT);
ast_node!(CodeStmt, CODE_STMT);
ast_node!(SwitchStmt, SWITCH_STMT);
ast_node!(SwitchArm, SWITCH_ARM);
ast_node!(LoopLabel, LOOP_LABEL);
ast_node!(ReturnStmt, RETURN_STMT);
ast_node!(BreakStmt, BREAK_STMT);
ast_node!(ContinueStmt, CONTINUE_STMT);
ast_node!(LiteralExpr, LITERAL_EXPR);
ast_node!(NameExpr, NAME_EXPR);
ast_node!(BinaryExpr, BINARY_EXPR);
ast_node!(UnaryExpr, UNARY_EXPR);
ast_node!(ParenExpr, PAREN_EXPR);
ast_node!(CallExpr, CALL_EXPR);
ast_node!(ArgList, ARG_LIST);
ast_node!(FieldExpr, FIELD_EXPR);
ast_node!(IndexExpr, INDEX_EXPR);
ast_node!(SliceExpr, SLICE_EXPR);
ast_node!(DerefExpr, DEREF_EXPR);
ast_node!(UninitExpr, UNINIT_EXPR);
ast_node!(CastExpr, CAST_EXPR);
ast_node!(AutocastExpr, AUTOCAST_EXPR);
ast_node!(MemberExpr, MEMBER_EXPR);
ast_node!(RunExpr, RUN_EXPR);
ast_node!(DirectiveExpr, DIRECTIVE_EXPR);

// ---------------------------------------------------------------------------
// Item enum
// ---------------------------------------------------------------------------

/// Any top-level item in a source file.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Item {
    /// `name :: value`
    Const(ConstDecl),
    /// `operator + :: (…) -> T { … }` (ADR-0048 §1)
    Operator(OperatorDecl),
    /// `name := expr;` or `name: T = expr;`
    Var(VarDecl),
    /// `#import "module";`
    Import(ImportDecl),
    /// `#run expr;`
    Run(RunDecl),
}

impl AstNode for Item {
    fn can_cast(kind: SyntaxKind) -> bool {
        matches!(
            kind,
            CONST_DECL | OPERATOR_DECL | VAR_DECL | IMPORT_DECL | RUN_DECL
        )
    }

    fn cast(node: SyntaxNode) -> Option<Self> {
        match node.kind() {
            CONST_DECL => Some(Self::Const(ConstDecl(node))),
            OPERATOR_DECL => Some(Self::Operator(OperatorDecl(node))),
            VAR_DECL => Some(Self::Var(VarDecl(node))),
            IMPORT_DECL => Some(Self::Import(ImportDecl(node))),
            RUN_DECL => Some(Self::Run(RunDecl(node))),
            _ => None,
        }
    }

    fn syntax(&self) -> &SyntaxNode {
        match self {
            Self::Const(n) => n.syntax(),
            Self::Operator(n) => n.syntax(),
            Self::Var(n) => n.syntax(),
            Self::Import(n) => n.syntax(),
            Self::Run(n) => n.syntax(),
        }
    }
}

// ---------------------------------------------------------------------------
// Type enum
// ---------------------------------------------------------------------------

/// A type expression.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeExpr {
    /// `*T`
    Pointer(PointerType),
    /// `[N]T`
    Array(ArrayType),
    /// `[]T` (ADR-0044 §1)
    View(ViewType),
    /// `[..]T` — a growable dynamic array (ADR-0136)
    DynamicArray(DynamicArrayType),
    /// `(T, T) -> T` (ADR-0059 §3)
    Proc(ProcType),
    /// `Ident`
    Name(NameType),
    /// `$T` (ADR-0081 §1)
    Poly(PolyType),
    /// `struct { ... }`
    Struct(StructType),
    /// `union { ... }` (ADR-0045)
    Union(UnionType),
    /// `variant { … }` (ADR-0068 §1)
    Variant(VariantType),
    /// `enum { ... }`
    Enum(EnumType),
}

impl AstNode for TypeExpr {
    fn can_cast(kind: SyntaxKind) -> bool {
        matches!(
            kind,
            POINTER_TYPE
                | ARRAY_TYPE
                | VIEW_TYPE
                | DYNAMIC_ARRAY_TYPE
                | PROC_TYPE
                | NAME_TYPE
                | POLY_TYPE
                | STRUCT_TYPE
                | UNION_TYPE
                | VARIANT_TYPE
                | ENUM_TYPE
        )
    }

    fn cast(node: SyntaxNode) -> Option<Self> {
        match node.kind() {
            POINTER_TYPE => Some(Self::Pointer(PointerType(node))),
            ARRAY_TYPE => Some(Self::Array(ArrayType(node))),
            VIEW_TYPE => Some(Self::View(ViewType(node))),
            DYNAMIC_ARRAY_TYPE => Some(Self::DynamicArray(DynamicArrayType(node))),
            PROC_TYPE => Some(Self::Proc(ProcType(node))),
            NAME_TYPE => Some(Self::Name(NameType(node))),
            POLY_TYPE => Some(Self::Poly(PolyType(node))),
            STRUCT_TYPE => Some(Self::Struct(StructType(node))),
            UNION_TYPE => Some(Self::Union(UnionType(node))),
            VARIANT_TYPE => Some(Self::Variant(VariantType(node))),
            ENUM_TYPE => Some(Self::Enum(EnumType(node))),
            _ => None,
        }
    }

    fn syntax(&self) -> &SyntaxNode {
        match self {
            Self::Pointer(n) => n.syntax(),
            Self::Array(n) => n.syntax(),
            Self::View(n) => n.syntax(),
            Self::DynamicArray(n) => n.syntax(),
            Self::Proc(n) => n.syntax(),
            Self::Name(n) => n.syntax(),
            Self::Poly(n) => n.syntax(),
            Self::Struct(n) => n.syntax(),
            Self::Union(n) => n.syntax(),
            Self::Variant(n) => n.syntax(),
            Self::Enum(n) => n.syntax(),
        }
    }
}

// ---------------------------------------------------------------------------
// Statement enum
// ---------------------------------------------------------------------------

/// A statement inside a block.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Stmt {
    /// `{ ... }`
    Block(Block),
    /// A declaration used as a statement.
    Decl(DeclStmt),
    /// `expr;`
    Expr(ExprStmt),
    /// `lhs = rhs;`
    Assign(AssignStmt),
    /// `if cond { ... }`
    If(IfStmt),
    /// `while cond { ... }`
    While(WhileStmt),
    /// `return expr;`
    Return(ReturnStmt),
    /// `break;` or `break label;`
    Break(BreakStmt),
    /// `continue;` or `continue label;`
    Continue(ContinueStmt),
    /// `for x: buf { … }` (ADR-0049 §1)
    For(ForStmt),
    /// `defer stmt;` (ADR-0049 §3)
    Defer(DeferStmt),
    /// `push_context { … }` (ADR-0063)
    PushContext(PushContextStmt),
    /// `#code { … }` (ADR-0080 §1) — unquoted source spliced into the enclosing scope.
    Code(CodeStmt),
    /// `switch e { case v; … }` (ADR-0067)
    Switch(SwitchStmt),
    /// `label: for …` or `label: while …` (ADR-0049 §2)
    Labelled(LoopLabel),
}

impl AstNode for Stmt {
    fn can_cast(kind: SyntaxKind) -> bool {
        matches!(
            kind,
            BLOCK
                | DECL_STMT
                | EXPR_STMT
                | ASSIGN_STMT
                | IF_STMT
                | WHILE_STMT
                | RETURN_STMT
                | BREAK_STMT
                | CONTINUE_STMT
                | FOR_STMT
                | DEFER_STMT
                | PUSH_CONTEXT_STMT
                | SWITCH_STMT
                | LOOP_LABEL
        )
    }

    fn cast(node: SyntaxNode) -> Option<Self> {
        match node.kind() {
            BLOCK => Some(Self::Block(Block(node))),
            DECL_STMT => Some(Self::Decl(DeclStmt(node))),
            EXPR_STMT => Some(Self::Expr(ExprStmt(node))),
            ASSIGN_STMT => Some(Self::Assign(AssignStmt(node))),
            IF_STMT => Some(Self::If(IfStmt(node))),
            WHILE_STMT => Some(Self::While(WhileStmt(node))),
            RETURN_STMT => Some(Self::Return(ReturnStmt(node))),
            BREAK_STMT => Some(Self::Break(BreakStmt(node))),
            CONTINUE_STMT => Some(Self::Continue(ContinueStmt(node))),
            FOR_STMT => Some(Self::For(ForStmt(node))),
            DEFER_STMT => Some(Self::Defer(DeferStmt(node))),
            PUSH_CONTEXT_STMT => Some(Self::PushContext(PushContextStmt(node))),
            CODE_STMT => Some(Self::Code(CodeStmt(node))),
            SWITCH_STMT => Some(Self::Switch(SwitchStmt(node))),
            LOOP_LABEL => Some(Self::Labelled(LoopLabel(node))),
            _ => None,
        }
    }

    fn syntax(&self) -> &SyntaxNode {
        match self {
            Self::Block(n) => n.syntax(),
            Self::Decl(n) => n.syntax(),
            Self::Expr(n) => n.syntax(),
            Self::Assign(n) => n.syntax(),
            Self::If(n) => n.syntax(),
            Self::While(n) => n.syntax(),
            Self::Return(n) => n.syntax(),
            Self::Break(n) => n.syntax(),
            Self::Continue(n) => n.syntax(),
            Self::For(n) => n.syntax(),
            Self::Defer(n) => n.syntax(),
            Self::PushContext(n) => n.syntax(),
            Self::Code(n) => n.syntax(),
            Self::Switch(n) => n.syntax(),
            Self::Labelled(n) => n.syntax(),
        }
    }
}

// ---------------------------------------------------------------------------
// Expression enum
// ---------------------------------------------------------------------------

/// An expression.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Expr {
    /// An integer, string, or boolean literal.
    Literal(LiteralExpr),
    /// A name reference.
    Name(NameExpr),
    /// `a op b`
    Binary(BinaryExpr),
    /// `op a`
    Unary(UnaryExpr),
    /// `(a)`
    Paren(ParenExpr),
    /// `f(args)`
    Call(CallExpr),
    /// `a.b`
    Field(FieldExpr),
    /// `a[i]`
    Index(IndexExpr),
    /// `a[]` (ADR-0044 §2)
    Slice(SliceExpr),
    /// `context` (ADR-0057 §1)
    Context(ContextExpr),
    /// `a..b` — reachable only in a `for` header (ADR-0049 §1)
    Range(RangeExpr),
    /// `p.*`
    Deref(DerefExpr),
    /// `---`
    Uninit(UninitExpr),
    /// `cast(T, x)`
    Cast(CastExpr),
    /// `xx expr` (ADR-0046 §2)
    Autocast(AutocastExpr),
    /// `.RED` (ADR-0046 §3)
    Member(MemberExpr),
    /// `#run expr`
    Run(RunExpr),
    /// `#directive ...`
    Directive(DirectiveExpr),
}

impl AstNode for Expr {
    fn can_cast(kind: SyntaxKind) -> bool {
        matches!(
            kind,
            LITERAL_EXPR
                | NAME_EXPR
                | BINARY_EXPR
                | UNARY_EXPR
                | PAREN_EXPR
                | CALL_EXPR
                | FIELD_EXPR
                | INDEX_EXPR
                | SLICE_EXPR
                | CONTEXT_EXPR
                | RANGE_EXPR
                | DEREF_EXPR
                | UNINIT_EXPR
                | CAST_EXPR
                | AUTOCAST_EXPR
                | MEMBER_EXPR
                | RUN_EXPR
                | DIRECTIVE_EXPR
        )
    }

    fn cast(node: SyntaxNode) -> Option<Self> {
        match node.kind() {
            LITERAL_EXPR => Some(Self::Literal(LiteralExpr(node))),
            NAME_EXPR => Some(Self::Name(NameExpr(node))),
            BINARY_EXPR => Some(Self::Binary(BinaryExpr(node))),
            UNARY_EXPR => Some(Self::Unary(UnaryExpr(node))),
            PAREN_EXPR => Some(Self::Paren(ParenExpr(node))),
            CALL_EXPR => Some(Self::Call(CallExpr(node))),
            FIELD_EXPR => Some(Self::Field(FieldExpr(node))),
            INDEX_EXPR => Some(Self::Index(IndexExpr(node))),
            SLICE_EXPR => Some(Self::Slice(SliceExpr(node))),
            CONTEXT_EXPR => Some(Self::Context(ContextExpr(node))),
            RANGE_EXPR => Some(Self::Range(RangeExpr(node))),
            DEREF_EXPR => Some(Self::Deref(DerefExpr(node))),
            UNINIT_EXPR => Some(Self::Uninit(UninitExpr(node))),
            CAST_EXPR => Some(Self::Cast(CastExpr(node))),
            AUTOCAST_EXPR => Some(Self::Autocast(AutocastExpr(node))),
            MEMBER_EXPR => Some(Self::Member(MemberExpr(node))),
            RUN_EXPR => Some(Self::Run(RunExpr(node))),
            DIRECTIVE_EXPR => Some(Self::Directive(DirectiveExpr(node))),
            _ => None,
        }
    }

    fn syntax(&self) -> &SyntaxNode {
        match self {
            Self::Literal(n) => n.syntax(),
            Self::Name(n) => n.syntax(),
            Self::Binary(n) => n.syntax(),
            Self::Unary(n) => n.syntax(),
            Self::Paren(n) => n.syntax(),
            Self::Call(n) => n.syntax(),
            Self::Field(n) => n.syntax(),
            Self::Index(n) => n.syntax(),
            Self::Slice(n) => n.syntax(),
            Self::Context(n) => n.syntax(),
            Self::Range(n) => n.syntax(),
            Self::Deref(n) => n.syntax(),
            Self::Uninit(n) => n.syntax(),
            Self::Cast(n) => n.syntax(),
            Self::Autocast(n) => n.syntax(),
            Self::Member(n) => n.syntax(),
            Self::Run(n) => n.syntax(),
            Self::Directive(n) => n.syntax(),
        }
    }
}

// ---------------------------------------------------------------------------
// Accessors
// ---------------------------------------------------------------------------

impl SourceFile {
    /// All top-level items.
    pub fn items(&self) -> impl Iterator<Item = Item> + '_ {
        child_nodes::<Item>(&self.0)
    }
}

impl ConstDecl {
    /// The name being bound.
    pub fn name(&self) -> Option<Name> {
        child_node(&self.0)
    }

    /// The value (a proc, struct type, or expression).
    ///
    /// Returns the first child that is an expression, proc, or struct type.
    pub fn value_expr(&self) -> Option<Expr> {
        child_node(&self.0)
    }

    /// The proc value, if this constant is a procedure.
    pub fn proc(&self) -> Option<Proc> {
        child_node(&self.0)
    }

    /// The struct type value, if this constant is a struct type.
    pub fn struct_type(&self) -> Option<StructType> {
        child_node(&self.0)
    }

    /// The `union { … }` value, if this declaration is a union type (ADR-0045).
    pub fn union_type(&self) -> Option<UnionType> {
        child_node(&self.0)
    }

    /// The variant value, if this constant is a variant type (ADR-0068 §1).
    pub fn variant_type(&self) -> Option<VariantType> {
        child_node(&self.0)
    }

    /// The enum value, if this constant is an enum type (ADR-0041).
    pub fn enum_type(&self) -> Option<EnumType> {
        child_node(&self.0)
    }
}

impl VarDecl {
    /// The name being declared.
    pub fn name(&self) -> Option<Name> {
        child_node(&self.0)
    }

    /// The explicit type annotation, if present.
    pub fn ty(&self) -> Option<TypeExpr> {
        child_node(&self.0)
    }

    /// The initialiser expression, if present.
    pub fn initializer(&self) -> Option<Expr> {
        child_node(&self.0)
    }

    /// Whether this declaration is `using`, promoting its type's fields (ADR-0050 §1).
    pub fn is_using(&self) -> bool {
        child_token(&self.0, USING_KW).is_some()
    }
}

impl ImportDecl {
    /// The module path string literal.
    pub fn path(&self) -> Option<SyntaxToken> {
        child_token(&self.0, STRING_LITERAL)
    }
}

impl RunDecl {
    /// The expression to run at compile time.
    pub fn expr(&self) -> Option<Expr> {
        child_node(&self.0)
    }
}

impl Name {
    /// The identifier token.
    pub fn ident_token(&self) -> Option<SyntaxToken> {
        child_token(&self.0, IDENT)
    }

    /// The text of the name as an owned string.
    pub fn text(&self) -> Option<String> {
        self.ident_token().map(|t| t.text().to_owned())
    }
}

impl Proc {
    /// The parameter list.
    pub fn param_list(&self) -> Option<ParamList> {
        child_node(&self.0)
    }

    /// The return type, if present.
    pub fn ret_type(&self) -> Option<RetType> {
        child_node(&self.0)
    }

    /// The body block, if present.
    pub fn body(&self) -> Option<Block> {
        child_node(&self.0)
    }

    /// The `#foreign` attribute, if present.
    pub fn foreign_attr(&self) -> Option<ForeignAttr> {
        child_node(&self.0)
    }

    /// Returns `true` if this is a foreign procedure.
    pub fn is_foreign(&self) -> bool {
        self.foreign_attr().is_some()
    }
}

impl ParamList {
    /// All parameters.
    pub fn params(&self) -> impl Iterator<Item = Param> + '_ {
        child_nodes(&self.0)
    }
}

impl Param {
    /// The parameter name token.
    pub fn name_token(&self) -> Option<SyntaxToken> {
        child_token(&self.0, IDENT)
    }

    /// The parameter type.
    pub fn ty(&self) -> Option<TypeExpr> {
        child_node(&self.0)
    }

    /// Whether this parameter is `using`, promoting its type's fields (ADR-0050 §1).
    pub fn is_using(&self) -> bool {
        child_token(&self.0, USING_KW).is_some()
    }

    /// Whether this parameter is `$N` — comptime-value polymorphic (ADR-0087 §1).
    ///
    /// The `$` precedes the *name*, so it is a `DOLLAR` token child of the `PARAM` node — distinct
    /// from a `$T` in type position, which is a `POLY_TYPE` node inside the parameter's type.
    pub fn is_comptime(&self) -> bool {
        child_token(&self.0, DOLLAR).is_some()
    }

    /// Whether this parameter is `..T` — variadic (ADR-0138 §1). The `..` precedes the *type*
    /// (after the `:`), and it means "the caller's trailing arguments are collected into a
    /// `[]T` view".
    pub fn is_variadic(&self) -> bool {
        child_token(&self.0, DOT_DOT).is_some()
    }

    /// The default value, if the parameter has one (ADR-0053 §2).
    pub fn default_value(&self) -> Option<Expr> {
        child_node(&self.0)
    }
}

impl Proc {
    /// Whether this procedure is `#c_call`, opting out of the implicit context (ADR-0057 §3).
    pub fn is_c_call(&self) -> bool {
        self.0.children().any(|n| n.kind() == C_CALL_ATTR)
    }

    /// Whether this procedure is `#no_abc`, suppressing its bounds checks (ADR-0058 §3).
    ///
    /// A *procedure*-level question, which is what ADR-0058 §3 amends ADR-0003 to make it: the
    /// directive is on the header, so `Proc` is the only node that has to answer.
    pub fn is_no_abc(&self) -> bool {
        self.0.children().any(|n| n.kind() == NO_ABC_ATTR)
    }

    /// Whether this procedure is `#expand` — a **macro**, spliced into the caller's scope rather than
    /// called (ADR-0090 §1).
    pub fn is_expand(&self) -> bool {
        self.0.children().any(|n| n.kind() == EXPAND_ATTR)
    }

    /// The `@note`s on this procedure, in source order (ADR-0098 §1).
    pub fn notes(&self) -> impl Iterator<Item = Note> + '_ {
        self.0.children().filter_map(Note::cast)
    }

    /// The `#modify { … }` predicate block, if this procedure has one (ADR-0093 §1).
    ///
    /// A compile-time predicate over an instantiation: it returns a `bool`, and `false` refuses the call.
    pub fn modify_block(&self) -> Option<Block> {
        self.0
            .children()
            .find(|n| n.kind() == MODIFY_ATTR)
            .and_then(|attr| attr.children().find_map(Block::cast))
    }
}

impl ScopeDecl {
    /// The directive token, `#scope_module` or `#scope_export` (ADR-0054 §1).
    pub fn directive(&self) -> Option<SyntaxToken> {
        child_token(&self.0, DIRECTIVE)
    }
}

impl NamedArg {
    /// The parameter name being named.
    pub fn name(&self) -> Option<Name> {
        child_node(&self.0)
    }

    /// The value.
    pub fn value(&self) -> Option<Expr> {
        child_node(&self.0)
    }
}

impl RetType {
    /// The return type expression.
    pub fn ty(&self) -> Option<TypeExpr> {
        child_node(&self.0)
    }
}

impl ForeignAttr {
    /// The library name identifier.
    pub fn library_name(&self) -> Option<SyntaxToken> {
        child_token(&self.0, IDENT)
    }

    /// The optional symbol name string literal.
    pub fn symbol_name(&self) -> Option<SyntaxToken> {
        child_token(&self.0, STRING_LITERAL)
    }
}

impl PolyType {
    /// The identifier token naming the polymorphic type variable — the `T` in `$T` (ADR-0081 §1).
    pub fn name_token(&self) -> Option<SyntaxToken> {
        child_token(&self.0, IDENT)
    }

    /// `true` for `$$T` — a comptime-required polymorphic parameter (ADR-0137). Distinguished
    /// from `$T` by the presence of a *second* `$` token as a child of the POLY_TYPE node.
    pub fn is_comptime(&self) -> bool {
        self.0
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| t.kind() == DOLLAR)
            .count()
            > 1
    }
}

impl NameType {
    /// The identifier token naming the type.
    pub fn name_token(&self) -> Option<SyntaxToken> {
        child_token(&self.0, IDENT)
    }

    /// The text of the type name as an owned string.
    pub fn text(&self) -> Option<String> {
        self.name_token().map(|t| t.text().to_owned())
    }

    /// The type arguments of a parameterised reference — `(s64)` in `Box(s64)` (ADR-0085 §3).
    ///
    /// `None` for an ordinary name, which has no argument list.
    pub fn arguments(&self) -> Option<TypeArguments> {
        child_node(&self.0)
    }
}

impl TypeArguments {
    /// The argument types, in order — `s64` in `Box(s64)` (ADR-0085 §3).
    pub fn args(&self) -> impl Iterator<Item = TypeExpr> + '_ {
        child_nodes(&self.0)
    }
}

impl StructTypeParams {
    /// The type-variable parameters, in order — `$T` in `struct($T) { … }` (ADR-0085 §3).
    pub fn vars(&self) -> impl Iterator<Item = PolyType> + '_ {
        child_nodes(&self.0)
    }
}

impl PointerType {
    /// The pointee type.
    pub fn pointee(&self) -> Option<TypeExpr> {
        child_node(&self.0)
    }
}

impl ArrayType {
    /// The length expression, `N` in `[N]T`.
    ///
    /// An expression rather than a literal because `[COUNT]u8` must parse; whether it is
    /// a compile-time constant is `jr-sema`'s question (ADR-0039 §3).
    pub fn len(&self) -> Option<Expr> {
        child_node(&self.0)
    }

    /// The element type, `T` in `[N]T`.
    pub fn elem(&self) -> Option<TypeExpr> {
        child_node(&self.0)
    }
}

impl ViewType {
    /// The element type, `T` in `[]T`.
    ///
    /// There is deliberately no `len()`: a view's length is runtime data, which is the whole
    /// difference from [`ArrayType`] (ADR-0044 §1).
    pub fn elem(&self) -> Option<TypeExpr> {
        child_node(&self.0)
    }
}
impl DynamicArrayType {
    /// The element type, `T` in `[..]T` (ADR-0136).
    pub fn elem(&self) -> Option<TypeExpr> {
        child_node(&self.0)
    }
}
impl ProcType {
    /// The parameter types, in order — `s64, bool` in `(s64, bool) -> T`.
    pub fn params(&self) -> impl Iterator<Item = TypeExpr> + '_ {
        self.0
            .children()
            .find(|n| n.kind() == PROC_TYPE_PARAMS)
            .into_iter()
            .flat_map(|list| list.children().filter_map(TypeExpr::cast))
    }

    /// The return type, `T` in `(…) -> T`.
    ///
    /// The one type child that is **not** inside the `PROC_TYPE_PARAMS` node — which is why the
    /// parameters live in their own node rather than as flat children: otherwise the last parameter
    /// and the return type would be indistinguishable (ADR-0059 §3).
    pub fn ret(&self) -> Option<TypeExpr> {
        self.0
            .children()
            .filter(|n| n.kind() != PROC_TYPE_PARAMS)
            .find_map(TypeExpr::cast)
    }
}
impl AutocastExpr {
    /// The operand being converted.
    pub fn operand(&self) -> Option<Expr> {
        child_node(&self.0)
    }
}
impl MemberExpr {
    /// The member name token, `RED` in `.RED`.
    ///
    /// There is deliberately no `receiver()`: the absence of one *is* the form (ADR-0046 §3).
    pub fn name_token(&self) -> Option<SyntaxToken> {
        child_token(&self.0, IDENT)
    }
}
impl SliceExpr {
    /// The expression being sliced, `a` in `a[]`.
    pub fn base(&self) -> Option<Expr> {
        child_node(&self.0)
    }
}
impl IndexExpr {
    /// The expression being indexed, `a` in `a[i]`.
    pub fn base(&self) -> Option<Expr> {
        child_node(&self.0)
    }

    /// The index, `i` in `a[i]`.
    ///
    /// The second `Expr` child: the base is first, because the postfix parser opens the
    /// node at the base's checkpoint.
    pub fn index(&self) -> Option<Expr> {
        self.0.children().filter_map(Expr::cast).nth(1)
    }
}

impl StructType {
    /// The field list.
    pub fn field_list(&self) -> Option<FieldList> {
        child_node(&self.0)
    }

    /// The type parameters of a parameterised struct — `($T)` in `struct($T) { … }` (ADR-0085 §3).
    ///
    /// `None` for an ordinary `struct { … }`.
    pub fn params(&self) -> Option<StructTypeParams> {
        child_node(&self.0)
    }
}
impl ForStmt {
    /// Whether the loop is reversed — the `<` after `for` (ADR-0049 §1).
    pub fn is_reverse(&self) -> bool {
        child_token(&self.0, LT).is_some()
    }

    /// The element variable, `x` in `for x: buf`.
    pub fn value_name(&self) -> Option<Name> {
        child_node(&self.0)
    }

    /// The index variable, `i` in `for x, i: buf` — absent in the one-name form.
    pub fn index_name(&self) -> Option<Name> {
        nth_child_node(&self.0, 1)
    }

    /// The thing being iterated: an expression, or a [`RangeExpr`].
    pub fn iterable(&self) -> Option<Expr> {
        child_node(&self.0)
    }

    /// The loop body: a braced block, or a single unbraced statement.
    pub fn body(&self) -> Option<ControlBody> {
        control_body(&self.0)
    }
}
impl RangeExpr {
    /// The start of the range, `a` in `a..b`.
    pub fn start(&self) -> Option<Expr> {
        child_node(&self.0)
    }

    /// The end of the range, `b` in `a..b` — excluded, since the range is half-open.
    pub fn end(&self) -> Option<Expr> {
        nth_child_node(&self.0, 1)
    }
}
impl DeferStmt {
    /// The deferred statement.
    ///
    /// Reuses [`ControlBody`], since `defer { … }` and `defer f();` are the same two shapes an
    /// `if` body has — and reusing it means a consumer handles both without a second convention.
    pub fn stmt(&self) -> Option<ControlBody> {
        control_body(&self.0)
    }
}
impl PushContextStmt {
    /// The block whose statements run against a copy of the context (ADR-0063).
    ///
    /// A `Block` rather than a [`ControlBody`]: the parser requires braces (a braceless context swap
    /// reads as a mistake), so there is only ever the one shape and no need for the two-shape helper
    /// `defer` and `if` share.
    pub fn block(&self) -> Option<Block> {
        self.0.children().find_map(Block::cast)
    }
}
impl CodeStmt {
    /// The braced body whose **source text** is spliced (ADR-0080 §1, §2).
    ///
    /// A `Block`, parsed as ordinary statements so its faults are reported where they are written. What
    /// lowering actually uses is the block's own *text* — the CST is lossless, so it is recoverable — which
    /// is what makes `#code` reuse `#insert`'s path with no new representation.
    pub fn block(&self) -> Option<Block> {
        self.0.children().find_map(Block::cast)
    }
}
impl SwitchStmt {
    /// The value being matched (ADR-0067 §1).
    ///
    /// The *first* expression child, because an arm's `case` value is also an expression child of the
    /// switch's subtree — but an arm's lives under a `SWITCH_ARM`, so `children()` at this level sees
    /// only the scrutinee.
    pub fn value(&self) -> Option<Expr> {
        child_node(&self.0)
    }

    /// The arms, in source order.
    pub fn arms(&self) -> impl Iterator<Item = SwitchArm> + '_ {
        child_nodes(&self.0)
    }
}
impl SwitchArm {
    /// The value this arm matches, or `None` for the `else` arm (ADR-0067 §4).
    ///
    /// An absent value *is* the catch-all — that is what distinguishes `else;` from `case v;` without a
    /// second node kind — so a consumer asks this rather than looking for a keyword.
    pub fn value(&self) -> Option<Expr> {
        child_node(&self.0)
    }

    /// Whether this is the `else` arm.
    ///
    /// Reads the keyword rather than inferring it from a missing value, because a malformed
    /// `case ;` also has no value and is *not* a catch-all — treating it as one would make a syntax
    /// error silently exhaustive.
    pub fn is_else(&self) -> bool {
        child_token(&self.0, ELSE_KW).is_some()
    }

    /// The statements this arm runs.
    pub fn body(&self) -> impl Iterator<Item = Stmt> + '_ {
        child_nodes(&self.0)
    }
}
impl LoopLabel {
    /// The label name.
    pub fn name(&self) -> Option<Name> {
        child_node(&self.0)
    }

    /// The loop the label names — a `FOR_STMT` or a `WHILE_STMT`.
    pub fn loop_stmt(&self) -> Option<SyntaxNode> {
        self.0
            .children()
            .find(|n| matches!(n.kind(), FOR_STMT | WHILE_STMT))
    }
}
impl OperatorDecl {
    /// The operator token, `+` in `operator + :: …`.
    ///
    /// Found by *kind* rather than by position: the first token after the keyword is the operator,
    /// but a malformed declaration may have none and a positional search would then pick the `::`.
    pub fn op_token(&self) -> Option<SyntaxToken> {
        self.0
            .children_with_tokens()
            .filter_map(SyntaxElement::into_token)
            .find(|t| {
                matches!(
                    t.kind(),
                    PLUS | MINUS
                        | STAR
                        | SLASH
                        | PERCENT
                        | EQ_EQ
                        | BANG_EQ
                        | LT
                        | LT_EQ
                        | GT
                        | GT_EQ
                        | PLUS_PERCENT
                        | MINUS_PERCENT
                        | STAR_PERCENT
                        | AMP
                        | PIPE
                        | CARET
                        | TILDE
                        | SHL
                        | SHR
                        | AMP_AMP
                        | PIPE_PIPE
                        | BANG
                )
            })
    }

    /// The procedure that implements the overload.
    pub fn proc(&self) -> Option<Proc> {
        child_node(&self.0)
    }
}
impl UnionType {
    /// The field list.
    ///
    /// The *same* `FieldList` node a struct has, because a union's fields are a struct's
    /// fields — only the layout differs (ADR-0045 §5).
    pub fn field_list(&self) -> Option<FieldList> {
        child_node(&self.0)
    }
}
impl VariantType {
    /// The field list — a variant's *cases*.
    ///
    /// The same `FieldList` node the other two forms have, because a case is written like a field.
    /// What differs is the layout (a leading tag, ADR-0068 §3) and the check on a read (§4), neither of
    /// which is visible in the syntax.
    pub fn field_list(&self) -> Option<FieldList> {
        child_node(&self.0)
    }
}

impl EnumType {
    /// Whether this was declared `enum_flags` rather than `enum` (ADR-0043 §1).
    ///
    /// Read from the keyword token rather than from a second node kind, so that every consumer
    /// which handles an enum handles both forms and only the ones that *care* about the
    /// difference ask.
    pub fn is_flags(&self) -> bool {
        self.0
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .any(|t| t.kind() == FLAGS_KW)
    }

    /// The member list.
    pub fn member_list(&self) -> Option<MemberList> {
        child_node(&self.0)
    }
}

impl MemberList {
    /// All members, in declaration order.
    ///
    /// Order is load-bearing: auto-numbering counts from 0 in this order, and an explicit
    /// value makes later members continue from it (ADR-0041 §3).
    pub fn members(&self) -> impl Iterator<Item = Member> + '_ {
        self.0.children().filter_map(Member::cast)
    }
}

impl Member {
    /// The member's name token.
    pub fn name_token(&self) -> Option<SyntaxToken> {
        self.0
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == IDENT)
    }

    /// The explicit value, if the member was written `NAME :: value`.
    pub fn value(&self) -> Option<Expr> {
        child_node(&self.0)
    }
}

impl FieldList {
    /// All fields.
    pub fn fields(&self) -> impl Iterator<Item = Field> + '_ {
        child_nodes(&self.0)
    }
}

impl Field {
    /// The field name token.
    pub fn name_token(&self) -> Option<SyntaxToken> {
        child_token(&self.0, IDENT)
    }

    /// The field type.
    pub fn ty(&self) -> Option<TypeExpr> {
        child_node(&self.0)
    }

    /// Whether this field is `using`-embedded, promoting its type's fields (ADR-0050 §1).
    pub fn is_using(&self) -> bool {
        child_token(&self.0, USING_KW).is_some()
    }

    /// The `#align N` operand, if the field carries one (ADR-0144 §3).
    ///
    /// The *expression*, not a number: it may be an integer literal or a name that resolves to
    /// a literal-valued constant, and deciding which is `jr-sema`'s judgement rather than the
    /// syntax's — the same split an array length uses (ADR-0070).
    pub fn align_value(&self) -> Option<Expr> {
        attr_value(&self.0, ALIGN_ATTR)
    }

    /// The `#place N` operand, if the field carries one (ADR-0144 §4).
    pub fn place_value(&self) -> Option<Expr> {
        attr_value(&self.0, PLACE_ATTR)
    }
}

impl Block {
    /// All statements in the block.
    pub fn stmts(&self) -> impl Iterator<Item = Stmt> + '_ {
        child_nodes(&self.0)
    }
}

impl DeclStmt {
    /// The inner declaration.
    pub fn decl(&self) -> Option<Item> {
        child_node(&self.0)
    }
}

impl ExprStmt {
    /// The expression.
    pub fn expr(&self) -> Option<Expr> {
        child_node(&self.0)
    }
}

impl AssignStmt {
    /// The left-hand side.
    pub fn lhs(&self) -> Option<Expr> {
        nth_child_node(&self.0, 0)
    }

    /// The assignment operator token.
    pub fn op_token(&self) -> Option<SyntaxToken> {
        self.0
            .children_with_tokens()
            .filter_map(SyntaxElement::into_token)
            .find(|t| {
                matches!(
                    t.kind(),
                    EQ | PLUS_EQ
                        | MINUS_EQ
                        | STAR_EQ
                        | SLASH_EQ
                        | PERCENT_EQ
                        | PLUS_PERCENT_EQ
                        | MINUS_PERCENT_EQ
                        | STAR_PERCENT_EQ
                        // As above: without these `flags |= FLAG` becomes `flags = FLAG`,
                        // because `lower_assign_op` recovers to `AssignOp::Assign`.
                        | AMP_EQ
                        | PIPE_EQ
                        | CARET_EQ
                        | SHL_EQ
                        | SHR_EQ
                )
            })
    }

    /// The right-hand side.
    pub fn rhs(&self) -> Option<Expr> {
        nth_child_node(&self.0, 1)
    }
}

// ---------------------------------------------------------------------------
// Control-flow bodies
// ---------------------------------------------------------------------------

/// The body of an `if`, `else`, or `while`.
///
/// The grammar accepts two shapes here, and `Parser::parse_body` is written for
/// both: a braced [`Block`], or a single unbraced [`Stmt`]. Accessors typed
/// `Option<Block>` could only see the first, so the second was invisible to every
/// consumer of the typed AST — and `jr-hir` then lowered it to `Stmt::Error` with
/// no diagnostic, silently discarding the body of
/// `tests/corpus/valid/010-if-else.jr`'s `if n > 0 return n;`. Making the two
/// shapes an enum is what stops a consumer from being able to forget one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ControlBody {
    /// A braced block: `if c { ... }`.
    Block(Block),
    /// A single statement without braces: `if c return n;`.
    Stmt(Stmt),
}

impl ControlBody {
    /// The underlying syntax node.
    #[must_use]
    pub fn syntax(&self) -> &SyntaxNode {
        match self {
            Self::Block(block) => block.syntax(),
            Self::Stmt(stmt) => stmt.syntax(),
        }
    }
}

/// The control-flow body of `parent`, whichever shape it took.
///
/// A [`Block`] is itself a [`Stmt`], so the braced case must be tried first or
/// every braced body would come back as the single-statement case.
fn control_body(parent: &SyntaxNode) -> Option<ControlBody> {
    if let Some(block) = child_node::<Block>(parent) {
        return Some(ControlBody::Block(block));
    }
    child_node::<Stmt>(parent).map(ControlBody::Stmt)
}

impl IfStmt {
    /// The condition expression.
    pub fn condition(&self) -> Option<Expr> {
        child_node(&self.0)
    }

    /// The then-body: a braced block, or a single unbraced statement.
    pub fn then_body(&self) -> Option<ControlBody> {
        control_body(&self.0)
    }

    /// The else branch, if present.
    pub fn else_branch(&self) -> Option<ElseBranch> {
        child_node(&self.0)
    }
}

impl ElseBranch {
    /// The else-if statement, if this is an `else if`.
    pub fn else_if(&self) -> Option<IfStmt> {
        child_node(&self.0)
    }

    /// The else-body, if this is a plain `else`: a block, or one statement.
    ///
    /// Returns `None` for an `else if`, so that this and [`Self::else_if`] are
    /// disjoint. Without that guard they would overlap, because an `IF_STMT` is
    /// itself a [`Stmt`] and so would satisfy the single-statement case.
    pub fn else_body(&self) -> Option<ControlBody> {
        if self.else_if().is_some() {
            return None;
        }
        control_body(&self.0)
    }
}

impl WhileStmt {
    /// The loop condition.
    pub fn condition(&self) -> Option<Expr> {
        child_node(&self.0)
    }

    /// The loop body: a braced block, or a single unbraced statement.
    pub fn body(&self) -> Option<ControlBody> {
        control_body(&self.0)
    }
}

impl BreakStmt {
    /// The label this `break` names, if any (ADR-0049 §2).
    ///
    /// `None` for a bare `break;`, which still means the innermost loop.
    pub fn label(&self) -> Option<Name> {
        child_node(&self.0)
    }
}

impl ContinueStmt {
    /// The label this `continue` names, if any (ADR-0049 §2).
    pub fn label(&self) -> Option<Name> {
        child_node(&self.0)
    }
}

impl ReturnStmt {
    /// The return value, if present.
    pub fn expr(&self) -> Option<Expr> {
        child_node(&self.0)
    }
}

impl LiteralExpr {
    /// The literal token.
    pub fn token(&self) -> Option<SyntaxToken> {
        self.0
            .children_with_tokens()
            .filter_map(SyntaxElement::into_token)
            .find(|t| {
                matches!(
                    t.kind(),
                    INT_LITERAL | FLOAT_LITERAL | STRING_LITERAL | TRUE_KW | FALSE_KW | NULL_KW
                )
            })
    }

    /// The kind of literal.
    pub fn kind(&self) -> Option<SyntaxKind> {
        self.token().map(|t| t.kind())
    }
}

impl NameExpr {
    /// The identifier token.
    pub fn name_token(&self) -> Option<SyntaxToken> {
        child_token(&self.0, IDENT)
    }

    /// The text of the name as an owned string.
    pub fn text(&self) -> Option<String> {
        self.name_token().map(|t| t.text().to_owned())
    }
}

impl BinaryExpr {
    /// The left operand.
    pub fn lhs(&self) -> Option<Expr> {
        nth_child_node(&self.0, 0)
    }

    /// The operator token.
    pub fn op_token(&self) -> Option<SyntaxToken> {
        self.0
            .children_with_tokens()
            .filter_map(SyntaxElement::into_token)
            .find(|t| {
                matches!(
                    t.kind(),
                    PIPE_PIPE
                        | AMP_AMP
                        // The bitwise operators (ADR-0042). Omitting them here made
                        // `6 & 3` evaluate to 9: `op_token` returned `None`, and
                        // `lower_bin_op`'s `_ => BinOp::Add` recovery arm turned every
                        // bitwise operator into an addition — in *both* engines, silently,
                        // with no diagnostic anywhere. A well-typed placeholder standing in
                        // for a missing case, which is this project's named failure mode.
                        | AMP
                        | PIPE
                        | CARET
                        | SHL
                        | SHR
                        | EQ_EQ
                        | BANG_EQ
                        | LT
                        | LT_EQ
                        | GT
                        | GT_EQ
                        | PLUS
                        | MINUS
                        | PLUS_PERCENT
                        | MINUS_PERCENT
                        | STAR
                        | SLASH
                        | PERCENT
                        | STAR_PERCENT
                )
            })
    }

    /// The right operand.
    pub fn rhs(&self) -> Option<Expr> {
        nth_child_node(&self.0, 1)
    }
}

impl UnaryExpr {
    /// The operator token.
    pub fn op_token(&self) -> Option<SyntaxToken> {
        self.0
            .children_with_tokens()
            .filter_map(SyntaxElement::into_token)
            // `TILDE` for `~` (ADR-0042 §4). The *third* kind-filtered `op_token` this wave
            // had to extend: without it `~0` recovered to `UnOp::Neg` and evaluated to `-0`,
            // which is 0 — a plausible answer, in both engines, with no diagnostic.
            .find(|t| matches!(t.kind(), MINUS | BANG | STAR | TILDE))
    }

    /// The operand.
    pub fn operand(&self) -> Option<Expr> {
        child_node(&self.0)
    }
}

impl ParenExpr {
    /// The inner expression.
    pub fn expr(&self) -> Option<Expr> {
        child_node(&self.0)
    }
}

impl CallExpr {
    /// The callee expression.
    pub fn callee(&self) -> Option<Expr> {
        child_node(&self.0)
    }

    /// The argument list.
    pub fn arg_list(&self) -> Option<ArgList> {
        child_node(&self.0)
    }
}

impl ArgList {
    /// All argument expressions.
    pub fn args(&self) -> impl Iterator<Item = Expr> + '_ {
        child_nodes(&self.0)
    }
}

impl FieldExpr {
    /// The object expression.
    pub fn object(&self) -> Option<Expr> {
        child_node(&self.0)
    }

    /// The field name token.
    pub fn field_name(&self) -> Option<SyntaxToken> {
        child_token(&self.0, IDENT)
    }
}

impl DerefExpr {
    /// The pointer expression being dereferenced.
    pub fn pointer(&self) -> Option<Expr> {
        child_node(&self.0)
    }
}

impl CastExpr {
    /// The target type: the `T` of `cast(T, x)`.
    pub fn target(&self) -> Option<TypeExpr> {
        child_node(&self.0)
    }

    /// The operand: the `x` of `cast(T, x)`.
    ///
    /// `child_node` finds the first `Expr` child, and the target is a `Type` rather than an
    /// `Expr`, so the two cannot be confused even though the type is written first.
    pub fn operand(&self) -> Option<Expr> {
        child_node(&self.0)
    }
}

impl RunExpr {
    /// The expression to evaluate at compile time.
    pub fn expr(&self) -> Option<Expr> {
        child_node(&self.0)
    }
}

impl DirectiveExpr {
    /// The directive token.
    pub fn directive_token(&self) -> Option<SyntaxToken> {
        child_token(&self.0, DIRECTIVE)
    }

    /// The optional string argument.
    pub fn string_arg(&self) -> Option<SyntaxToken> {
        child_token(&self.0, STRING_LITERAL)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

impl Note {
    /// The note's name — the `deprecated` of `@deprecated` (ADR-0098 §1).
    pub fn name_token(&self) -> Option<SyntaxToken> {
        child_token(&self.0, IDENT)
    }

    /// The note's optional string payload — the `"x"` of `@requires "x"`.
    ///
    /// Returned with its quotes still on, as every other string accessor in this file does, so one decoder
    /// handles them all.
    pub fn payload_token(&self) -> Option<SyntaxToken> {
        child_token(&self.0, STRING_LITERAL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use jr_base::FileId;

    fn file() -> FileId {
        FileId::from_usize(0)
    }

    #[test]
    fn source_file_items() {
        let p = parse("MAX :: 42;\nMSG :: \"hi\";", file());
        let sf = SourceFile::cast(p.syntax()).expect("source file");
        assert_eq!(sf.items().count(), 2);
    }

    #[test]
    fn const_decl_name() {
        let p = parse("MAX :: 42;", file());
        let sf = SourceFile::cast(p.syntax()).unwrap();
        let item = sf.items().next().unwrap();
        let Item::Const(cd) = item else {
            panic!("expected const decl")
        };
        assert_eq!(cd.name().and_then(|n| n.text()), Some("MAX".to_owned()));
    }

    #[test]
    fn proc_params() {
        let p = parse("add :: (a: s64, b: s64) -> s64 { return a + b; }", file());
        let sf = SourceFile::cast(p.syntax()).unwrap();
        let item = sf.items().next().unwrap();
        let Item::Const(cd) = item else {
            panic!("expected const decl")
        };
        let proc = cd.proc().expect("proc");
        let params: Vec<_> = proc.param_list().expect("param list").params().collect();
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn struct_fields() {
        let p = parse("Point :: struct { x: s64; y: s64; }", file());
        let sf = SourceFile::cast(p.syntax()).unwrap();
        let item = sf.items().next().unwrap();
        let Item::Const(cd) = item else {
            panic!("expected const decl")
        };
        let st = cd.struct_type().expect("struct type");
        let fields: Vec<_> = st.field_list().expect("field list").fields().collect();
        assert_eq!(fields.len(), 2);
    }

    #[test]
    fn import_path() {
        let p = parse(r#"#import "Basic";"#, file());
        let sf = SourceFile::cast(p.syntax()).unwrap();
        let item = sf.items().next().unwrap();
        let Item::Import(id) = item else {
            panic!("expected import")
        };
        let tok = id.path().expect("path token");
        assert_eq!(tok.text(), r#""Basic""#);
    }

    #[test]
    fn incomplete_tree_does_not_panic() {
        // A parse with errors must not panic when navigating the AST.
        let p = parse("broken :: ;", file());
        let sf = SourceFile::cast(p.syntax()).unwrap();
        for item in sf.items() {
            let _ = item.syntax().kind();
        }
    }
}
