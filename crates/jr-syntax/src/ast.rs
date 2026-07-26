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

// ---------------------------------------------------------------------------
// Node types
// ---------------------------------------------------------------------------

ast_node!(SourceFile, SOURCE_FILE);
ast_node!(ConstDecl, CONST_DECL);
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
ast_node!(PointerType, POINTER_TYPE);
ast_node!(StructType, STRUCT_TYPE);
ast_node!(FieldList, FIELD_LIST);
ast_node!(Field, FIELD);
ast_node!(Block, BLOCK);
ast_node!(DeclStmt, DECL_STMT);
ast_node!(ExprStmt, EXPR_STMT);
ast_node!(AssignStmt, ASSIGN_STMT);
ast_node!(IfStmt, IF_STMT);
ast_node!(ElseBranch, ELSE_BRANCH);
ast_node!(WhileStmt, WHILE_STMT);
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
ast_node!(DerefExpr, DEREF_EXPR);
ast_node!(UninitExpr, UNINIT_EXPR);
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
    /// `name := expr;` or `name: T = expr;`
    Var(VarDecl),
    /// `#import "module";`
    Import(ImportDecl),
    /// `#run expr;`
    Run(RunDecl),
}

impl AstNode for Item {
    fn can_cast(kind: SyntaxKind) -> bool {
        matches!(kind, CONST_DECL | VAR_DECL | IMPORT_DECL | RUN_DECL)
    }

    fn cast(node: SyntaxNode) -> Option<Self> {
        match node.kind() {
            CONST_DECL => Some(Self::Const(ConstDecl(node))),
            VAR_DECL => Some(Self::Var(VarDecl(node))),
            IMPORT_DECL => Some(Self::Import(ImportDecl(node))),
            RUN_DECL => Some(Self::Run(RunDecl(node))),
            _ => None,
        }
    }

    fn syntax(&self) -> &SyntaxNode {
        match self {
            Self::Const(n) => n.syntax(),
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
    /// `Ident`
    Name(NameType),
    /// `struct { ... }`
    Struct(StructType),
}

impl AstNode for TypeExpr {
    fn can_cast(kind: SyntaxKind) -> bool {
        matches!(kind, POINTER_TYPE | NAME_TYPE | STRUCT_TYPE)
    }

    fn cast(node: SyntaxNode) -> Option<Self> {
        match node.kind() {
            POINTER_TYPE => Some(Self::Pointer(PointerType(node))),
            NAME_TYPE => Some(Self::Name(NameType(node))),
            STRUCT_TYPE => Some(Self::Struct(StructType(node))),
            _ => None,
        }
    }

    fn syntax(&self) -> &SyntaxNode {
        match self {
            Self::Pointer(n) => n.syntax(),
            Self::Name(n) => n.syntax(),
            Self::Struct(n) => n.syntax(),
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
    /// `break;`
    Break(BreakStmt),
    /// `continue;`
    Continue(ContinueStmt),
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
    /// `p.*`
    Deref(DerefExpr),
    /// `---`
    Uninit(UninitExpr),
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
                | DEREF_EXPR
                | UNINIT_EXPR
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
            DEREF_EXPR => Some(Self::Deref(DerefExpr(node))),
            UNINIT_EXPR => Some(Self::Uninit(UninitExpr(node))),
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
            Self::Deref(n) => n.syntax(),
            Self::Uninit(n) => n.syntax(),
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

impl NameType {
    /// The identifier token naming the type.
    pub fn name_token(&self) -> Option<SyntaxToken> {
        child_token(&self.0, IDENT)
    }

    /// The text of the type name as an owned string.
    pub fn text(&self) -> Option<String> {
        self.name_token().map(|t| t.text().to_owned())
    }
}

impl PointerType {
    /// The pointee type.
    pub fn pointee(&self) -> Option<TypeExpr> {
        child_node(&self.0)
    }
}

impl StructType {
    /// The field list.
    pub fn field_list(&self) -> Option<FieldList> {
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
                )
            })
    }

    /// The right-hand side.
    pub fn rhs(&self) -> Option<Expr> {
        nth_child_node(&self.0, 1)
    }
}

impl IfStmt {
    /// The condition expression.
    pub fn condition(&self) -> Option<Expr> {
        child_node(&self.0)
    }

    /// The then-body (block or single statement).
    pub fn then_body(&self) -> Option<Block> {
        child_node(&self.0)
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

    /// The else block, if this is a plain `else { ... }`.
    pub fn else_block(&self) -> Option<Block> {
        child_node(&self.0)
    }
}

impl WhileStmt {
    /// The loop condition.
    pub fn condition(&self) -> Option<Expr> {
        child_node(&self.0)
    }

    /// The loop body.
    pub fn body(&self) -> Option<Block> {
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
                    INT_LITERAL | FLOAT_LITERAL | STRING_LITERAL | TRUE_KW | FALSE_KW
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
            .find(|t| matches!(t.kind(), MINUS | BANG | STAR))
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
