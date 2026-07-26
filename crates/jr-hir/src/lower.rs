//! Lowering from the typed AST to HIR.
//!
//! The entry point is [`lower_file`], which takes a parsed syntax tree and
//! produces a [`FileHir`] plus any diagnostics generated during lowering.
//!
//! ## Totality
//!
//! Lowering is **total**: a tree containing `ERROR` nodes still lowers,
//! producing `Expr::Error`/`Stmt::Error` and diagnostics. We never `unwrap()`
//! on AST accessors — the parser guarantees a tree, not a complete one.
//!
//! ## Stack overflow prevention
//!
//! The parser caps nesting at 256 levels, but we still guard against deep
//! recursion. Expression lowering is recursive but bounded by the parser's
//! nesting limit. Block lowering is iterative (no recursion per statement).

use jr_base::{FileId, Interner, Span, Symbol};
use jr_diag::{Diagnostic, Diagnostics};
use jr_syntax::{
    SyntaxKind,
    SyntaxKind::*,
    SyntaxNode, SyntaxToken,
    ast::{
        AssignStmt, AstNode, BinaryExpr, Block, ConstDecl, DeclStmt, ElseBranch, Expr as AstExpr,
        IfStmt, ImportDecl, Item as AstItem, LiteralExpr, Proc as AstProc, RunDecl, SourceFile,
        Stmt as AstStmt, StructType, TypeExpr, UnaryExpr, VarDecl, WhileStmt,
    },
};

use crate::hir::{
    AssignOp, BinOp, Body, BodyId, ConstValue, Expr, ExprId, Field, FileHir, ForeignInfo, Item,
    ItemId, ItemKind, ItemScope, Literal, Local, LocalId, Param, ParamId, Proc, ProcId, Res, Stmt,
    StmtId, Struct, StructId, TypeRef, TypeRefId, UnOp,
};

// ---------------------------------------------------------------------------
// Diagnostic codes
// ---------------------------------------------------------------------------

const E0203: &str = "E0203";
const E0205: &str = "E0205";
const E0206: &str = "E0206";
/// A declaration inside a procedure body.
const E0207: &str = "E0207";
/// `#import` somewhere other than file scope.
const E0208: &str = "E0208";
/// A directive used where it is not valid.
const E0209: &str = "E0209";

/// Directives that are legal in expression position.
///
/// Everything else is rejected. The parser is deliberately permissive -- it
/// lexes `#anything` as one token and parses `#name "arg"` as a generic
/// directive expression, so that adding a directive never requires a lexer or
/// grammar change (see `jr_syntax::lexer`). That permissiveness has to be paid
/// for here, or `main :: () { #import "Basic"; }` lowers silently and
/// `jr check` reports success on a program that makes no sense.
const DIRECTIVES_VALID_AS_EXPRESSIONS: &[&str] = &["system_library", "library"];

/// Directives that are only meaningful at file scope.
const FILE_SCOPE_ONLY_DIRECTIVES: &[&str] = &["import", "load"];

// ---------------------------------------------------------------------------
// Scope entry
// ---------------------------------------------------------------------------

/// What a name resolves to inside a body.
#[derive(Clone, Copy)]
enum ScopeEntry {
    Local(LocalId),
    Param(ParamId),
}

// ---------------------------------------------------------------------------
// Lowering context (shared for file + bodies)
// ---------------------------------------------------------------------------

/// The lowering context for a single file, including all bodies.
///
/// We use a single context for the whole file so that body arenas can be
/// allocated into the file-level `bodies` Vec directly.
struct LowerCtx<'a> {
    file: FileId,
    interner: &'a Interner,
    diags: Diagnostics,

    // File-level arenas
    items: Vec<Item>,
    scope: ItemScope,
    procs: Vec<Proc>,
    structs: Vec<Struct>,
    bodies: Vec<Body>,

    // Top-level expression arenas (for const values, var initialisers, #run)
    top_exprs: Vec<Expr>,
    top_expr_spans: Vec<Span>,
    top_type_refs: Vec<TypeRef>,
}

impl<'a> LowerCtx<'a> {
    fn new(file: FileId, interner: &'a Interner) -> Self {
        Self {
            file,
            interner,
            diags: Diagnostics::new(),
            items: Vec::new(),
            scope: ItemScope::new(),
            procs: Vec::new(),
            structs: Vec::new(),
            bodies: Vec::new(),
            top_exprs: Vec::new(),
            top_expr_spans: Vec::new(),
            top_type_refs: Vec::new(),
        }
    }

    fn span_of_node(&self, node: &SyntaxNode) -> Span {
        Span::new(self.file, node.text_range())
    }

    fn span_of_token(&self, tok: &SyntaxToken) -> Span {
        Span::new(self.file, tok.text_range())
    }

    fn intern(&self, text: &str) -> Symbol {
        self.interner.intern(text)
    }

    // ---- top-level arenas --------------------------------------------------

    fn alloc_top_expr(&mut self, expr: Expr, span: Span) -> ExprId {
        let id = ExprId::from_usize(self.top_exprs.len());
        self.top_exprs.push(expr);
        self.top_expr_spans.push(span);
        id
    }

    fn alloc_top_type_ref(&mut self, tr: TypeRef) -> TypeRefId {
        let id = TypeRefId::from_usize(self.top_type_refs.len());
        self.top_type_refs.push(tr);
        id
    }

    fn alloc_item(&mut self, item: Item) -> ItemId {
        let id = ItemId::from_usize(self.items.len());
        if let Some(name) = item.name {
            self.scope.insert(name, id);
        }
        self.items.push(item);
        id
    }

    fn alloc_proc(&mut self, proc: Proc) -> ProcId {
        let id = ProcId::from_usize(self.procs.len());
        self.procs.push(proc);
        id
    }

    fn alloc_struct(&mut self, s: Struct) -> StructId {
        let id = StructId::from_usize(self.structs.len());
        self.structs.push(s);
        id
    }

    fn alloc_body(&mut self, body: Body) -> BodyId {
        let id = BodyId::from_usize(self.bodies.len());
        self.bodies.push(body);
        id
    }

    // ---- type references (top-level) ---------------------------------------

    fn lower_type_expr_top(&mut self, ty: &TypeExpr) -> TypeRefId {
        match ty {
            TypeExpr::Name(n) => {
                let sym = n
                    .name_token()
                    .map(|t| self.intern(t.text()))
                    .unwrap_or_else(|| self.intern("<error>"));
                self.alloc_top_type_ref(TypeRef::Name(sym))
            }
            TypeExpr::Pointer(p) => {
                let inner = if let Some(pointee) = p.pointee() {
                    self.lower_type_expr_top(&pointee)
                } else {
                    self.alloc_top_type_ref(TypeRef::Error)
                };
                self.alloc_top_type_ref(TypeRef::Pointer(inner))
            }
            TypeExpr::Struct(s) => {
                let struct_id = self.lower_struct_type(s);
                self.alloc_top_type_ref(TypeRef::Struct(struct_id))
            }
        }
    }

    // ---- struct types ------------------------------------------------------

    fn lower_struct_type(&mut self, s: &StructType) -> StructId {
        let span = self.span_of_node(s.syntax());
        let mut fields = Vec::new();

        if let Some(fl) = s.field_list() {
            for f in fl.fields() {
                let name_tok = f.name_token();
                let name = name_tok
                    .as_ref()
                    .map(|t| self.intern(t.text()))
                    .unwrap_or_else(|| self.intern("<error>"));
                let name_span = name_tok
                    .as_ref()
                    .map(|t| self.span_of_token(t))
                    .unwrap_or(span);
                let ty = f.ty().map(|t| self.lower_type_expr_top(&t));
                fields.push(Field {
                    name,
                    name_span,
                    ty,
                });
            }
        }

        self.alloc_struct(Struct {
            fields,
            span,
            type_refs: Vec::new(),
        })
    }

    // ---- procedures --------------------------------------------------------

    fn lower_proc(&mut self, ast_proc: &AstProc) -> ProcId {
        let span = self.span_of_node(ast_proc.syntax());

        // Parameters
        let mut params = Vec::new();
        if let Some(pl) = ast_proc.param_list() {
            for p in pl.params() {
                let name_tok = p.name_token();
                let name = name_tok
                    .as_ref()
                    .map(|t| self.intern(t.text()))
                    .unwrap_or_else(|| self.intern("<error>"));
                let name_span = name_tok
                    .as_ref()
                    .map(|t| self.span_of_token(t))
                    .unwrap_or(span);
                let ty = p.ty().map(|t| self.lower_type_expr_top(&t));
                params.push(Param {
                    name,
                    name_span,
                    ty,
                });
            }
        }

        // Return type
        let ret = ast_proc
            .ret_type()
            .and_then(|rt| rt.ty())
            .map(|t| self.lower_type_expr_top(&t));

        // Foreign attribute
        let foreign = ast_proc.foreign_attr().map(|fa| {
            let fa_span = self.span_of_node(fa.syntax());
            let library = fa.library_name().map(|t| self.intern(t.text()));
            let symbol = fa.symbol_name().map(|t| strip_quotes(t.text()));
            ForeignInfo {
                library,
                symbol,
                span: fa_span,
            }
        });

        // Body
        let body = ast_proc.body().map(|b| {
            let body = self.lower_body(&b, &params);
            self.alloc_body(body)
        });

        // Validate: must have body XOR foreign (or neither, which is an error)
        if body.is_none() && foreign.is_none() {
            let diag = Diagnostic::error(
                span,
                "procedure has neither a body nor a `#foreign` attribute",
            )
            .with_code(E0203);
            self.diags.push(diag);
        }

        self.alloc_proc(Proc {
            params,
            ret,
            body,
            foreign,
            span,
            type_refs: Vec::new(),
        })
    }

    // ---- bodies ------------------------------------------------------------

    fn lower_body(&mut self, block: &Block, params: &[Param]) -> Body {
        let mut bctx = BodyLowerCtx::new(self.file, self.interner);

        // Register parameters in the outermost scope
        for (i, param) in params.iter().enumerate() {
            let pid = ParamId::from_usize(i);
            bctx.params.push(param.clone());
            bctx.scope_stack
                .last_mut()
                .unwrap()
                .push((param.name, ScopeEntry::Param(pid)));
        }

        let root = bctx.lower_block(block);

        // Drain body diagnostics into the file diagnostics
        self.diags.extend(bctx.diags.into_vec());

        Body {
            exprs: bctx.exprs,
            expr_spans: bctx.expr_spans,
            stmts: bctx.stmts,
            locals: bctx.locals,
            type_refs: bctx.type_refs,
            root,
        }
    }

    // ---- top-level expressions ---------------------------------------------

    fn lower_top_expr(&mut self, expr: &AstExpr) -> ExprId {
        let span = self.span_of_node(expr.syntax());
        match expr {
            AstExpr::Literal(lit) => {
                let literal = lower_literal_impl(lit, span, self.interner, &mut self.diags);
                self.alloc_top_expr(Expr::Literal(literal, span), span)
            }
            AstExpr::Name(n) => {
                let name = n
                    .name_token()
                    .map(|t| self.intern(t.text()))
                    .unwrap_or_else(|| self.intern("<error>"));
                self.alloc_top_expr(
                    Expr::Name {
                        name,
                        span,
                        res: Res::Error,
                    },
                    span,
                )
            }
            AstExpr::Binary(b) => {
                let op = lower_bin_op(b);
                let lhs = b.lhs().map(|e| self.lower_top_expr(&e)).unwrap_or_else(|| {
                    let err = Expr::Error(span);
                    self.alloc_top_expr(err, span)
                });
                let rhs = b.rhs().map(|e| self.lower_top_expr(&e)).unwrap_or_else(|| {
                    let err = Expr::Error(span);
                    self.alloc_top_expr(err, span)
                });
                self.alloc_top_expr(Expr::Binary { op, lhs, rhs, span }, span)
            }
            AstExpr::Unary(u) => {
                let op = lower_un_op(u);
                let operand = u
                    .operand()
                    .map(|e| self.lower_top_expr(&e))
                    .unwrap_or_else(|| {
                        let err = Expr::Error(span);
                        self.alloc_top_expr(err, span)
                    });
                self.alloc_top_expr(Expr::Unary { op, operand, span }, span)
            }
            AstExpr::Paren(p) => {
                // Drop PAREN_EXPR
                p.expr()
                    .map(|e| self.lower_top_expr(&e))
                    .unwrap_or_else(|| {
                        let err = Expr::Error(span);
                        self.alloc_top_expr(err, span)
                    })
            }
            AstExpr::Call(c) => {
                let callee = c
                    .callee()
                    .map(|e| self.lower_top_expr(&e))
                    .unwrap_or_else(|| {
                        let err = Expr::Error(span);
                        self.alloc_top_expr(err, span)
                    });
                let args = c
                    .arg_list()
                    .map(|al| {
                        al.args()
                            .map(|a| self.lower_top_expr(&a))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                self.alloc_top_expr(Expr::Call { callee, args, span }, span)
            }
            AstExpr::Field(f) => {
                let receiver = f
                    .object()
                    .map(|e| self.lower_top_expr(&e))
                    .unwrap_or_else(|| {
                        let err = Expr::Error(span);
                        self.alloc_top_expr(err, span)
                    });
                let (name, name_span) = f
                    .field_name()
                    .map(|t| (self.intern(t.text()), self.span_of_token(&t)))
                    .unwrap_or_else(|| (self.intern("<error>"), span));
                self.alloc_top_expr(
                    Expr::Field {
                        receiver,
                        name,
                        name_span,
                        span,
                    },
                    span,
                )
            }
            AstExpr::Deref(d) => {
                let ptr = d
                    .pointer()
                    .map(|e| self.lower_top_expr(&e))
                    .unwrap_or_else(|| {
                        let err = Expr::Error(span);
                        self.alloc_top_expr(err, span)
                    });
                self.alloc_top_expr(Expr::Deref(ptr, span), span)
            }
            AstExpr::Uninit(_) => self.alloc_top_expr(Expr::Uninit(span), span),
            AstExpr::Run(r) => {
                let inner = r
                    .expr()
                    .map(|e| self.lower_top_expr(&e))
                    .unwrap_or_else(|| {
                        let err = Expr::Error(span);
                        self.alloc_top_expr(err, span)
                    });
                self.alloc_top_expr(Expr::Run(inner, span), span)
            }
            AstExpr::Directive(d) => {
                let name = d
                    .directive_token()
                    .map(|t| self.intern(t.text().trim_start_matches('#')))
                    .unwrap_or_else(|| self.intern("<error>"));
                let arg = d.string_arg().map(|t| strip_quotes(t.text()));
                if let Some(diag) = check_directive_as_expression(self.interner.resolve(name), span)
                {
                    self.diags.push(diag);
                }
                self.alloc_top_expr(Expr::Directive { name, arg, span }, span)
            }
        }
    }

    // ---- items -------------------------------------------------------------

    fn lower_const_decl(&mut self, cd: &ConstDecl) {
        let span = self.span_of_node(cd.syntax());
        let name_node = cd.name();
        let name = name_node
            .as_ref()
            .and_then(|n| n.text())
            .map(|t| self.intern(&t));
        let name_span = name_node
            .as_ref()
            .map(|n| self.span_of_node(n.syntax()))
            .unwrap_or(span);

        let kind = if let Some(proc) = cd.proc() {
            let proc_id = self.lower_proc(&proc);
            ItemKind::Const {
                value: ConstValue::Proc(proc_id),
            }
        } else if let Some(st) = cd.struct_type() {
            let struct_id = self.lower_struct_type(&st);
            ItemKind::Const {
                value: ConstValue::Struct(struct_id),
            }
        } else if let Some(expr) = cd.value_expr() {
            let expr_id = self.lower_top_expr(&expr);
            ItemKind::Const {
                value: ConstValue::Expr(expr_id),
            }
        } else {
            let err_id = self.alloc_top_expr(Expr::Error(span), span);
            ItemKind::Const {
                value: ConstValue::Expr(err_id),
            }
        };

        self.alloc_item(Item {
            name,
            span,
            name_span,
            kind,
        });
    }

    fn lower_var_decl_item(&mut self, vd: &VarDecl) {
        let span = self.span_of_node(vd.syntax());
        let name_node = vd.name();
        let name = name_node
            .as_ref()
            .and_then(|n| n.text())
            .map(|t| self.intern(&t));
        let name_span = name_node
            .as_ref()
            .map(|n| self.span_of_node(n.syntax()))
            .unwrap_or(span);

        let ty = vd.ty().map(|t| self.lower_type_expr_top(&t));
        let (init, uninit) = match vd.initializer() {
            Some(AstExpr::Uninit(_)) => (None, true),
            Some(e) => (Some(self.lower_top_expr(&e)), false),
            None => (None, false),
        };

        self.alloc_item(Item {
            name,
            span,
            name_span,
            kind: ItemKind::Var { ty, init, uninit },
        });
    }

    fn lower_import_decl(&mut self, id: &ImportDecl) {
        let span = self.span_of_node(id.syntax());
        let (path, path_span) = if let Some(tok) = id.path() {
            (strip_quotes(tok.text()), self.span_of_token(&tok))
        } else {
            (String::new(), span)
        };

        self.alloc_item(Item {
            name: None,
            span,
            name_span: span,
            kind: ItemKind::Import { path, path_span },
        });
    }

    fn lower_run_decl(&mut self, rd: &RunDecl) {
        let span = self.span_of_node(rd.syntax());
        let expr = rd
            .expr()
            .map(|e| self.lower_top_expr(&e))
            .unwrap_or_else(|| self.alloc_top_expr(Expr::Error(span), span));

        self.alloc_item(Item {
            name: None,
            span,
            name_span: span,
            kind: ItemKind::Run { expr },
        });
    }

    fn finish(self) -> (FileHir, Diagnostics) {
        let hir = FileHir {
            items: self.items,
            scope: self.scope,
            procs: self.procs,
            structs: self.structs,
            bodies: self.bodies,
            exprs: self.top_exprs,
            expr_spans: self.top_expr_spans,
            type_refs: self.top_type_refs,
        };
        (hir, self.diags)
    }
}

// ---------------------------------------------------------------------------
// Body lowering context
// ---------------------------------------------------------------------------

/// Lowering context for a single procedure body.
struct BodyLowerCtx<'a> {
    file: FileId,
    interner: &'a Interner,
    diags: Diagnostics,

    exprs: Vec<Expr>,
    expr_spans: Vec<Span>,
    stmts: Vec<Stmt>,
    locals: Vec<Local>,
    type_refs: Vec<TypeRef>,
    params: Vec<Param>,

    /// Scope stack: each frame is a list of (name, entry) pairs.
    /// Innermost frame is last. Shadowing is allowed (spec §023).
    scope_stack: Vec<Vec<(Symbol, ScopeEntry)>>,
}

impl<'a> BodyLowerCtx<'a> {
    fn new(file: FileId, interner: &'a Interner) -> Self {
        Self {
            file,
            interner,
            diags: Diagnostics::new(),
            exprs: Vec::new(),
            expr_spans: Vec::new(),
            stmts: Vec::new(),
            locals: Vec::new(),
            type_refs: Vec::new(),
            params: Vec::new(),
            scope_stack: vec![Vec::new()],
        }
    }

    fn span_of_node(&self, node: &SyntaxNode) -> Span {
        Span::new(self.file, node.text_range())
    }

    fn span_of_token(&self, tok: &SyntaxToken) -> Span {
        Span::new(self.file, tok.text_range())
    }

    fn intern(&self, text: &str) -> Symbol {
        self.interner.intern(text)
    }

    fn alloc_expr(&mut self, expr: Expr, span: Span) -> ExprId {
        let id = ExprId::from_usize(self.exprs.len());
        self.exprs.push(expr);
        self.expr_spans.push(span);
        id
    }

    fn alloc_stmt(&mut self, stmt: Stmt) -> StmtId {
        let id = StmtId::from_usize(self.stmts.len());
        self.stmts.push(stmt);
        id
    }

    fn alloc_local(&mut self, local: Local) -> LocalId {
        let id = LocalId::from_usize(self.locals.len());
        self.locals.push(local);
        id
    }

    fn alloc_type_ref(&mut self, tr: TypeRef) -> TypeRefId {
        let id = TypeRefId::from_usize(self.type_refs.len());
        self.type_refs.push(tr);
        id
    }

    fn push_scope(&mut self) {
        self.scope_stack.push(Vec::new());
    }

    fn pop_scope(&mut self) {
        self.scope_stack.pop();
    }

    fn define_local(&mut self, name: Symbol, id: LocalId) {
        if let Some(frame) = self.scope_stack.last_mut() {
            frame.push((name, ScopeEntry::Local(id)));
        }
    }

    /// Look up a name in the current scope stack (innermost first).
    fn lookup_local(&self, name: Symbol) -> Option<ScopeEntry> {
        for frame in self.scope_stack.iter().rev() {
            for (n, entry) in frame.iter().rev() {
                if *n == name {
                    return Some(*entry);
                }
            }
        }
        None
    }

    fn lower_type_expr(&mut self, ty: &TypeExpr) -> TypeRefId {
        match ty {
            TypeExpr::Name(n) => {
                let sym = n
                    .name_token()
                    .map(|t| self.intern(t.text()))
                    .unwrap_or_else(|| self.intern("<error>"));
                self.alloc_type_ref(TypeRef::Name(sym))
            }
            TypeExpr::Pointer(p) => {
                let inner = if let Some(pointee) = p.pointee() {
                    self.lower_type_expr(&pointee)
                } else {
                    self.alloc_type_ref(TypeRef::Error)
                };
                self.alloc_type_ref(TypeRef::Pointer(inner))
            }
            TypeExpr::Struct(_) => {
                // Inline struct types inside bodies are unusual
                self.alloc_type_ref(TypeRef::Error)
            }
        }
    }

    fn lower_block(&mut self, block: &Block) -> StmtId {
        let span = self.span_of_node(block.syntax());
        self.push_scope();
        let mut stmts = Vec::new();
        for ast_stmt in block.stmts() {
            let sid = self.lower_stmt(&ast_stmt);
            stmts.push(sid);
        }
        self.pop_scope();
        self.alloc_stmt(Stmt::Block(stmts, span))
    }

    fn lower_stmt(&mut self, stmt: &AstStmt) -> StmtId {
        let span = self.span_of_node(stmt.syntax());
        match stmt {
            AstStmt::Block(b) => self.lower_block(b),
            AstStmt::Decl(d) => self.lower_decl_stmt(d, span),
            AstStmt::Expr(e) => {
                let expr_id = e
                    .expr()
                    .map(|ex| self.lower_expr(&ex))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Error(span), span));
                self.alloc_stmt(Stmt::Expr(expr_id, span))
            }
            AstStmt::Assign(a) => self.lower_assign_stmt(a, span),
            AstStmt::If(i) => self.lower_if_stmt(i, span),
            AstStmt::While(w) => self.lower_while_stmt(w, span),
            AstStmt::Return(r) => {
                let expr = r.expr().map(|e| self.lower_expr(&e));
                self.alloc_stmt(Stmt::Return(expr, span))
            }
            AstStmt::Break(_) => self.alloc_stmt(Stmt::Break(span)),
            AstStmt::Continue(_) => self.alloc_stmt(Stmt::Continue(span)),
        }
    }

    fn lower_decl_stmt(&mut self, d: &DeclStmt, span: Span) -> StmtId {
        let Some(inner) = d.decl() else {
            return self.alloc_stmt(Stmt::Error(span));
        };

        match inner {
            AstItem::Var(vd) => {
                let vd_span = self.span_of_node(vd.syntax());
                let name_node = vd.name();
                let name = name_node
                    .as_ref()
                    .and_then(|n| n.text())
                    .map(|t| self.intern(&t))
                    .unwrap_or_else(|| self.intern("<error>"));
                let name_span = name_node
                    .as_ref()
                    .map(|n| self.span_of_node(n.syntax()))
                    .unwrap_or(vd_span);

                let ty = vd.ty().map(|t| self.lower_type_expr(&t));
                let (init, uninit) = match vd.initializer() {
                    Some(AstExpr::Uninit(_)) => (None, true),
                    Some(e) => (Some(self.lower_expr(&e)), false),
                    None => (None, false),
                };

                let local = Local {
                    name,
                    name_span,
                    ty,
                    init,
                    uninit,
                    span: vd_span,
                };
                let local_id = self.alloc_local(local);
                self.define_local(name, local_id);
                self.alloc_stmt(Stmt::Local(local_id, vd_span))
            }
            // Declarations inside a procedure body.
            //
            // The parser accepts these because a block may contain a
            // declaration statement, but they are NOT part of the Jairs-0
            // subset, and `BodyLowerCtx` has no access to the file-level item
            // arena to lower them into. What matters is that we say so: an
            // earlier version emitted a bare `Stmt::Error` with no diagnostic,
            // which silently dropped the declaration from the program. Once
            // codegen exists that is a miscompile, not an inconvenience.
            AstItem::Const(_) => {
                self.diags.push(
                    Diagnostic::error(
                        span,
                        "declarations inside a procedure body are not supported yet",
                    )
                    .with_code(E0207)
                    .with_note("nested procedures and local constants arrive in wave W2")
                    .with_help("move the declaration to file scope"),
                );
                self.alloc_stmt(Stmt::Error(span))
            }
            AstItem::Import(_) => {
                self.diags.push(
                    Diagnostic::error(span, "`#import` is only allowed at file scope")
                        .with_code(E0208)
                        .with_help("move the `#import` to the top of the file"),
                );
                self.alloc_stmt(Stmt::Error(span))
            }
            AstItem::Run(_) => {
                self.diags.push(
                    Diagnostic::error(span, "`#run` as a statement is not supported yet")
                        .with_code(E0207)
                        .with_note("compile-time execution inside a body arrives in wave W4")
                        .with_help(
                            "use a file-scope `#run` or a `::` constant initialised with `#run`",
                        ),
                );
                self.alloc_stmt(Stmt::Error(span))
            }
        }
    }

    fn lower_assign_stmt(&mut self, a: &AssignStmt, span: Span) -> StmtId {
        let lhs = a
            .lhs()
            .map(|e| self.lower_expr(&e))
            .unwrap_or_else(|| self.alloc_expr(Expr::Error(span), span));
        let rhs = a
            .rhs()
            .map(|e| self.lower_expr(&e))
            .unwrap_or_else(|| self.alloc_expr(Expr::Error(span), span));
        let op = a
            .op_token()
            .map(|t| lower_assign_op(t.kind()))
            .unwrap_or(AssignOp::Assign);
        self.alloc_stmt(Stmt::Assign { lhs, op, rhs, span })
    }

    fn lower_if_stmt(&mut self, i: &IfStmt, span: Span) -> StmtId {
        let cond = i
            .condition()
            .map(|e| self.lower_expr(&e))
            .unwrap_or_else(|| self.alloc_expr(Expr::Error(span), span));
        let then = i
            .then_body()
            .map(|b| self.lower_block(&b))
            .unwrap_or_else(|| self.alloc_stmt(Stmt::Error(span)));
        let else_ = i.else_branch().map(|eb| self.lower_else_branch(&eb, span));
        self.alloc_stmt(Stmt::If {
            cond,
            then,
            else_,
            span,
        })
    }

    fn lower_else_branch(&mut self, eb: &ElseBranch, span: Span) -> StmtId {
        if let Some(else_if) = eb.else_if() {
            let else_span = self.span_of_node(eb.syntax());
            self.lower_if_stmt(&else_if, else_span)
        } else if let Some(else_block) = eb.else_block() {
            self.lower_block(&else_block)
        } else {
            self.alloc_stmt(Stmt::Error(span))
        }
    }

    fn lower_while_stmt(&mut self, w: &WhileStmt, span: Span) -> StmtId {
        let cond = w
            .condition()
            .map(|e| self.lower_expr(&e))
            .unwrap_or_else(|| self.alloc_expr(Expr::Error(span), span));
        let body = w
            .body()
            .map(|b| self.lower_block(&b))
            .unwrap_or_else(|| self.alloc_stmt(Stmt::Error(span)));
        self.alloc_stmt(Stmt::While { cond, body, span })
    }

    fn lower_expr(&mut self, expr: &AstExpr) -> ExprId {
        let span = self.span_of_node(expr.syntax());
        match expr {
            AstExpr::Literal(lit) => {
                let literal = lower_literal_impl(lit, span, self.interner, &mut self.diags);
                self.alloc_expr(Expr::Literal(literal, span), span)
            }
            AstExpr::Name(n) => {
                let name = n
                    .name_token()
                    .map(|t| self.intern(t.text()))
                    .unwrap_or_else(|| self.intern("<error>"));
                // Pre-fill local/param resolution; file-level items resolved later.
                let res = self
                    .lookup_local(name)
                    .map(|e| match e {
                        ScopeEntry::Local(id) => Res::Local(id),
                        ScopeEntry::Param(id) => Res::Param(id),
                    })
                    .unwrap_or(Res::Error);
                self.alloc_expr(Expr::Name { name, span, res }, span)
            }
            AstExpr::Binary(b) => {
                let op = lower_bin_op(b);
                let lhs = b
                    .lhs()
                    .map(|e| self.lower_expr(&e))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Error(span), span));
                let rhs = b
                    .rhs()
                    .map(|e| self.lower_expr(&e))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Error(span), span));
                self.alloc_expr(Expr::Binary { op, lhs, rhs, span }, span)
            }
            AstExpr::Unary(u) => {
                let op = lower_un_op(u);
                let operand = u
                    .operand()
                    .map(|e| self.lower_expr(&e))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Error(span), span));
                self.alloc_expr(Expr::Unary { op, operand, span }, span)
            }
            AstExpr::Paren(p) => {
                // Drop PAREN_EXPR: parentheses exist only to shape the tree.
                p.expr()
                    .map(|e| self.lower_expr(&e))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Error(span), span))
            }
            AstExpr::Call(c) => {
                let callee = c
                    .callee()
                    .map(|e| self.lower_expr(&e))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Error(span), span));
                let args = c
                    .arg_list()
                    .map(|al| al.args().map(|a| self.lower_expr(&a)).collect::<Vec<_>>())
                    .unwrap_or_default();
                self.alloc_expr(Expr::Call { callee, args, span }, span)
            }
            AstExpr::Field(f) => {
                let receiver = f
                    .object()
                    .map(|e| self.lower_expr(&e))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Error(span), span));
                let (name, name_span) = f
                    .field_name()
                    .map(|t| (self.intern(t.text()), self.span_of_token(&t)))
                    .unwrap_or_else(|| (self.intern("<error>"), span));
                self.alloc_expr(
                    Expr::Field {
                        receiver,
                        name,
                        name_span,
                        span,
                    },
                    span,
                )
            }
            AstExpr::Deref(d) => {
                let ptr = d
                    .pointer()
                    .map(|e| self.lower_expr(&e))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Error(span), span));
                self.alloc_expr(Expr::Deref(ptr, span), span)
            }
            AstExpr::Uninit(_) => self.alloc_expr(Expr::Uninit(span), span),
            AstExpr::Run(r) => {
                let inner = r
                    .expr()
                    .map(|e| self.lower_expr(&e))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Error(span), span));
                self.alloc_expr(Expr::Run(inner, span), span)
            }
            AstExpr::Directive(d) => {
                let name = d
                    .directive_token()
                    .map(|t| self.intern(t.text().trim_start_matches('#')))
                    .unwrap_or_else(|| self.intern("<error>"));
                let arg = d.string_arg().map(|t| strip_quotes(t.text()));
                if let Some(diag) = check_directive_as_expression(self.interner.resolve(name), span)
                {
                    self.diags.push(diag);
                }
                self.alloc_expr(Expr::Directive { name, arg, span }, span)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Free functions (shared between LowerCtx and BodyLowerCtx)
// ---------------------------------------------------------------------------

fn strip_quotes(s: &str) -> String {
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        s[1..s.len() - 1].to_owned()
    } else {
        s.to_owned()
    }
}

fn lower_bin_op(b: &BinaryExpr) -> BinOp {
    match b.op_token().map(|t| t.kind()) {
        Some(PLUS) => BinOp::Add,
        Some(MINUS) => BinOp::Sub,
        Some(STAR) => BinOp::Mul,
        Some(SLASH) => BinOp::Div,
        Some(PERCENT) => BinOp::Rem,
        Some(PLUS_PERCENT) => BinOp::WrapAdd,
        Some(MINUS_PERCENT) => BinOp::WrapSub,
        Some(STAR_PERCENT) => BinOp::WrapMul,
        Some(EQ_EQ) => BinOp::Eq,
        Some(BANG_EQ) => BinOp::Ne,
        Some(LT) => BinOp::Lt,
        Some(LT_EQ) => BinOp::Le,
        Some(GT) => BinOp::Gt,
        Some(GT_EQ) => BinOp::Ge,
        Some(AMP_AMP) => BinOp::And,
        Some(PIPE_PIPE) => BinOp::Or,
        _ => BinOp::Add, // error recovery
    }
}

fn lower_un_op(u: &UnaryExpr) -> UnOp {
    match u.op_token().map(|t| t.kind()) {
        Some(MINUS) => UnOp::Neg,
        Some(BANG) => UnOp::Not,
        Some(STAR) => UnOp::AddrOf,
        _ => UnOp::Neg, // error recovery
    }
}

fn lower_assign_op(kind: SyntaxKind) -> AssignOp {
    match kind {
        EQ => AssignOp::Assign,
        PLUS_EQ => AssignOp::AddAssign,
        MINUS_EQ => AssignOp::SubAssign,
        STAR_EQ => AssignOp::MulAssign,
        SLASH_EQ => AssignOp::DivAssign,
        PERCENT_EQ => AssignOp::RemAssign,
        PLUS_PERCENT_EQ => AssignOp::WrapAddAssign,
        MINUS_PERCENT_EQ => AssignOp::WrapSubAssign,
        STAR_PERCENT_EQ => AssignOp::WrapMulAssign,
        _ => AssignOp::Assign, // error recovery
    }
}

/// Lower a literal expression, emitting diagnostics for invalid values.
fn lower_literal_impl(
    lit: &LiteralExpr,
    span: Span,
    _interner: &Interner,
    diags: &mut Diagnostics,
) -> Literal {
    let Some(tok) = lit.token() else {
        return Literal::Bool(false);
    };
    match tok.kind() {
        TRUE_KW => Literal::Bool(true),
        FALSE_KW => Literal::Bool(false),
        STRING_LITERAL => {
            let raw = tok.text();
            let decoded = decode_string_impl(raw, span, diags);
            Literal::Str(decoded)
        }
        INT_LITERAL => {
            let raw = tok.text();
            parse_int_literal_impl(raw)
        }
        _ => Literal::Bool(false),
    }
}

/// Decode a string literal, processing escape sequences.
fn decode_string_impl(raw: &str, span: Span, diags: &mut Diagnostics) -> String {
    let inner = if raw.starts_with('"') && raw.ends_with('"') && raw.len() >= 2 {
        &raw[1..raw.len() - 1]
    } else {
        raw
    };

    let mut result = String::with_capacity(inner.len());
    let mut chars = inner.char_indices();

    while let Some((_, c)) = chars.next() {
        if c != '\\' {
            result.push(c);
            continue;
        }

        match chars.next() {
            Some((_, 'n')) => result.push('\n'),
            Some((_, 'r')) => result.push('\r'),
            Some((_, 't')) => result.push('\t'),
            Some((_, '0')) => result.push('\0'),
            Some((_, '\\')) => result.push('\\'),
            Some((_, '"')) => result.push('"'),
            Some((_, 'u')) => {
                let mut hex = String::with_capacity(4);
                for _ in 0..4 {
                    if let Some((_, hc)) = chars.next() {
                        hex.push(hc);
                    }
                }
                if hex.len() == 4 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                    if let Ok(code) = u32::from_str_radix(&hex, 16) {
                        if let Some(ch) = char::from_u32(code) {
                            result.push(ch);
                        } else {
                            diags.push(
                                Diagnostic::error(
                                    span,
                                    format!("invalid unicode scalar value U+{hex}"),
                                )
                                .with_code(E0206),
                            );
                            result.push('\u{FFFD}');
                        }
                    } else {
                        diags.push(
                            Diagnostic::error(span, format!("invalid unicode escape `\\u{hex}`"))
                                .with_code(E0206),
                        );
                        result.push('\u{FFFD}');
                    }
                } else {
                    diags.push(
                        Diagnostic::error(
                            span,
                            "invalid unicode escape: expected exactly 4 hex digits",
                        )
                        .with_code(E0206),
                    );
                    result.push('\u{FFFD}');
                }
            }
            Some((_, other)) => {
                diags.push(
                    Diagnostic::error(span, format!("unknown string escape `\\{other}`"))
                        .with_code(E0205),
                );
                result.push(other);
            }
            None => {}
        }
    }

    result
}

/// Parse an integer literal into its magnitude and radix.
///
/// # No diagnostic here
///
/// Whether a literal *fits* is not a lowering question. Under ADR-0016 §1 an
/// integer literal has no intrinsic type — it takes the type of its context — and
/// lowering does not know the context. This function used to test every literal
/// against `s64` and report E0204, which silently accepted `x: u8 = 300;` and
/// worded the error after a type the literal may not have. The check lives in
/// `jr-sema`, which knows the contextual type; `overflowed` is recorded here only
/// as a note that the value did not fit `s64`, for consumers that want it.
fn parse_int_literal_impl(raw: &str) -> Literal {
    // Remove underscores (digit separators)
    let cleaned: String = raw.chars().filter(|&c| c != '_').collect();

    let (radix, digits): (u32, &str) = if cleaned.starts_with("0x") || cleaned.starts_with("0X") {
        (16, &cleaned[2..])
    } else if cleaned.starts_with("0b") || cleaned.starts_with("0B") {
        (2, &cleaned[2..])
    } else if cleaned.starts_with("0o") || cleaned.starts_with("0O") {
        (8, &cleaned[2..])
    } else {
        (10, &cleaned)
    };

    if digits.is_empty() {
        return Literal::Int {
            value: 0,
            radix,
            overflowed: false,
        };
    }

    match u64::from_str_radix(digits, radix) {
        Ok(value) => Literal::Int {
            value,
            radix,
            overflowed: value > i64::MAX as u64,
        },
        // Too large for even `u64`. Clamping keeps the value monotone, so the
        // fit check in sema rejects it for every integer type there is.
        Err(_) => Literal::Int {
            value: u64::MAX,
            radix,
            overflowed: true,
        },
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Lowers a parsed syntax tree into HIR for one file.
///
/// This is the main entry point for the HIR crate. It is a pure function:
/// no filesystem access, no salsa, no cross-file loading.
///
/// The returned [`FileHir`] contains the item tree and all arenas. Name
/// resolution (filling in `Res` fields on `Expr::Name` nodes) is a separate
/// step; call [`resolve`](fn@crate::resolve) afterwards.
///
/// # Panics
///
/// Never panics. A tree with `ERROR` nodes produces `Expr::Error`/`Stmt::Error`
/// and diagnostics, but does not panic.
pub fn lower_file(
    parse: &jr_syntax::Parse,
    file: FileId,
    interner: &Interner,
) -> (FileHir, Diagnostics) {
    let syntax = parse.syntax();
    let Some(source_file) = SourceFile::cast(syntax) else {
        let mut diags = Diagnostics::new();
        let span = Span::new(file, jr_base::TextRange::default());
        diags.push(Diagnostic::error(
            span,
            "internal error: root is not a SOURCE_FILE",
        ));
        return (
            FileHir {
                items: Vec::new(),
                scope: ItemScope::new(),
                procs: Vec::new(),
                structs: Vec::new(),
                bodies: Vec::new(),
                exprs: Vec::new(),
                expr_spans: Vec::new(),
                type_refs: Vec::new(),
            },
            diags,
        );
    };

    let mut ctx = LowerCtx::new(file, interner);

    for item in source_file.items() {
        match item {
            AstItem::Const(cd) => ctx.lower_const_decl(&cd),
            AstItem::Var(vd) => ctx.lower_var_decl_item(&vd),
            AstItem::Import(id) => ctx.lower_import_decl(&id),
            AstItem::Run(rd) => ctx.lower_run_decl(&rd),
        }
    }

    ctx.finish()
}

/// Rejects directives that are not valid in expression position.
///
/// Returns the diagnostic to emit, if any. Kept as a free function because both
/// the file-level and body-level lowering contexts need it.
fn check_directive_as_expression(name: &str, span: Span) -> Option<Diagnostic> {
    if DIRECTIVES_VALID_AS_EXPRESSIONS.contains(&name) {
        return None;
    }
    if FILE_SCOPE_ONLY_DIRECTIVES.contains(&name) {
        return Some(
            Diagnostic::error(span, format!("`#{name}` is only allowed at file scope"))
                .with_code(E0208)
                .with_help(format!("move the `#{name}` to the top of the file")),
        );
    }
    Some(
        Diagnostic::error(span, format!("`#{name}` is not valid here"))
            .with_code(E0209)
            .with_note("only `#run` and `#system_library` may appear in an expression")
            .with_help("if this directive should be supported, it needs a grammar rule of its own"),
    )
}
