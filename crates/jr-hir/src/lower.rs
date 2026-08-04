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
        ArrayType, AssignStmt, AstNode, BinaryExpr, Block, ConstDecl, ControlBody, DeclStmt,
        ElseBranch, EnumType, Expr as AstExpr, ForStmt, IfStmt, ImportDecl, Item as AstItem,
        LiteralExpr, Proc as AstProc, RunDecl, SourceFile, Stmt as AstStmt, StructType, TypeExpr,
        UnaryExpr, VarDecl, WhileStmt,
    },
};

use crate::hir::{
    AggregateKind, AssignOp, BinOp, Body, BodyId, ConstValue, Enum, EnumId, EnumMember, Expr,
    ExprId, Field, FileHir, ForIterable, ForeignInfo, InsertOperands, Item, ItemId, ItemKind,
    ItemScope, Literal, Local, LocalId, Param, ParamId, Proc, ProcId, Res, Stmt, StmtId, Struct,
    StructId, TypeRef, TypeRefId, UnOp,
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
/// `#insert` with an operand that is not a string literal (ADR-0072 §5).
const E0262: &str = "E0262";
/// The text of an `#insert` does not parse (ADR-0072 §3).
const E0263: &str = "E0263";
/// `#insert` expansion nested deeper than [`MAX_INSERT_DEPTH`] (ADR-0073 §3).
const E0264: &str = "E0264";

/// How deep `#insert` expansion may nest before it is refused (ADR-0073 §3).
///
/// A **bound with a diagnostic**, not a stack-overflow: a compiler that hangs or aborts on a program is
/// the one failure mode a compiler must never have, which is the argument `LayoutError::Recursive`
/// already makes for a recursive struct and `E0199` for parser nesting.
///
/// Sixteen, matching `jr-db`'s `MAX_ROUNDS` for constant evaluation, because both bound "how many times
/// may this feed itself" and a reader meeting one number twice has one thing to remember. Deliberately
/// far above any *written* nest that can exist: ADR-0072 §5 measured that escaping doubles the text per
/// level, so 16 levels of literal nesting is already ~32 KB of source and 40 would be ~10¹² bytes.
const MAX_INSERT_DEPTH: u32 = 16;

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
    /// Evaluated computed-`#insert` operands, threaded to each body (ADR-0073 §1, step 6).
    ///
    /// Empty in ordinary lowering, so a body behaves exactly as before; filled by the second lowering
    /// the operand pre-pass drives, where each pending insert's text is now known and expanded in place.
    operands: &'a InsertOperands,

    // File-level arenas
    items: Vec<Item>,
    scope: ItemScope,
    /// Whether declarations lowered from here on are exported (ADR-0054 §1).
    ///
    /// Starts `true`: export is the default, which is what keeps every existing file and all 126
    /// corpus files meaning exactly what they did — ADR-0014 §2 promised it and `modules/Basic`
    /// relies on it.
    exporting: bool,
    procs: Vec<Proc>,
    structs: Vec<Struct>,
    enums: Vec<Enum>,
    bodies: Vec<Body>,

    // Top-level expression arenas (for const values, var initialisers, #run)
    top_exprs: Vec<Expr>,
    top_expr_spans: Vec<Span>,
    top_type_refs: Vec<TypeRef>,
}

impl<'a> LowerCtx<'a> {
    fn new(file: FileId, interner: &'a Interner, operands: &'a InsertOperands) -> Self {
        Self {
            file,
            interner,
            operands,
            diags: Diagnostics::new(),
            items: Vec::new(),
            scope: ItemScope::new(),
            exporting: true,
            procs: Vec::new(),
            structs: Vec::new(),
            enums: Vec::new(),
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

    fn alloc_enum(&mut self, e: Enum) -> EnumId {
        let id = EnumId::from_usize(self.enums.len());
        self.enums.push(e);
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
                // `Box(s64)` — a name with type arguments is a parameterised reference (ADR-0085 §3).
                match n.arguments() {
                    Some(arguments) => {
                        let args: Vec<TypeRefId> = arguments
                            .args()
                            .map(|t| self.lower_type_expr_top(&t))
                            .collect();
                        self.alloc_top_type_ref(TypeRef::Apply { name: sym, args })
                    }
                    None => self.alloc_top_type_ref(TypeRef::Name(sym)),
                }
            }
            TypeExpr::Poly(poly) => {
                let sym = poly
                    .name_token()
                    .map(|t| self.intern(t.text()))
                    .unwrap_or_else(|| self.intern("<error>"));
                self.alloc_top_type_ref(TypeRef::Poly(sym))
            }
            TypeExpr::Pointer(p) => {
                let inner = if let Some(pointee) = p.pointee() {
                    self.lower_type_expr_top(&pointee)
                } else {
                    self.alloc_top_type_ref(TypeRef::Error)
                };
                self.alloc_top_type_ref(TypeRef::Pointer(inner))
            }
            TypeExpr::Array(a) => {
                let len_span = a.len().map_or_else(
                    || self.span_of_node(a.syntax()),
                    |e| self.span_of_node(e.syntax()),
                );
                let len = lower_array_len(a, len_span, self.interner, &mut self.diags);
                let elem = if let Some(e) = a.elem() {
                    self.lower_type_expr_top(&e)
                } else {
                    self.alloc_top_type_ref(TypeRef::Error)
                };
                self.alloc_top_type_ref(TypeRef::Array {
                    elem,
                    len,
                    len_name: lower_array_len_name(a, self.interner),
                    len_span,
                })
            }
            TypeExpr::View(v) => {
                let elem = if let Some(e) = v.elem() {
                    self.lower_type_expr_top(&e)
                } else {
                    self.alloc_top_type_ref(TypeRef::Error)
                };
                self.alloc_top_type_ref(TypeRef::View { elem })
            }
            TypeExpr::Proc(p) => {
                let params: Vec<TypeRefId> =
                    p.params().map(|t| self.lower_type_expr_top(&t)).collect();
                let ret = p.ret().map(|t| self.lower_type_expr_top(&t));
                self.alloc_top_type_ref(TypeRef::Proc { params, ret })
            }
            TypeExpr::Struct(s) => {
                let struct_id = self.lower_struct_type(s);
                self.alloc_top_type_ref(TypeRef::Struct(struct_id))
            }
            TypeExpr::Union(u) => {
                let union_id = self.lower_union_type(u);
                self.alloc_top_type_ref(TypeRef::Union(union_id))
            }
            TypeExpr::Variant(v) => {
                let variant_id = self.lower_variant_type(v);
                self.alloc_top_type_ref(TypeRef::Variant(variant_id))
            }
            TypeExpr::Enum(e) => {
                let enum_id = self.lower_enum_type(e);
                self.alloc_top_type_ref(TypeRef::Enum(enum_id))
            }
        }
    }

    // ---- struct types ------------------------------------------------------

    fn lower_struct_type(&mut self, s: &StructType) -> StructId {
        let span = self.span_of_node(s.syntax());
        // `struct($T) { … }` — the type parameters, empty for an ordinary struct (ADR-0085 §3).
        let mut poly_vars: Vec<Symbol> = Vec::new();
        if let Some(params) = s.params() {
            for var in params.vars() {
                if let Some(tok) = var.name_token() {
                    poly_vars.push(self.intern(tok.text()));
                }
            }
        }
        self.lower_fields_into_struct(span, s.field_list(), AggregateKind::Struct, poly_vars)
    }

    /// Lowers `union { i: s64; f: float64; }` (ADR-0045).
    ///
    /// Allocates into the **same arena** `lower_struct_type` does, with the kind set. That is
    /// not a shortcut: a `DeclId` is an index within its arena and does not record which arena
    /// (ADR-0041 §4a), and unions share `Pool::struct_fields` with structs — so two arenas would
    /// let a struct and a union at the same index overwrite each other's field lists. `variant`
    /// joined the same arena for the same reason (ADR-0068 §2).
    fn lower_union_type(&mut self, u: &jr_syntax::ast::UnionType) -> StructId {
        let span = self.span_of_node(u.syntax());
        self.lower_fields_into_struct(span, u.field_list(), AggregateKind::Union, Vec::new())
    }

    /// Lowers `variant { i: s64; f: float64; }` (ADR-0068 §1).
    ///
    /// The same arena and the same field loop as the other two forms; only the kind differs, which is
    /// what makes the tag a *layout* question (ADR-0068 §3) rather than a different HIR shape.
    fn lower_variant_type(&mut self, v: &jr_syntax::ast::VariantType) -> StructId {
        let span = self.span_of_node(v.syntax());
        self.lower_fields_into_struct(span, v.field_list(), AggregateKind::Variant, Vec::new())
    }

    /// The field loop all three aggregate forms share.
    fn lower_fields_into_struct(
        &mut self,
        span: jr_base::Span,
        field_list: Option<jr_syntax::ast::FieldList>,
        kind: AggregateKind,
        poly_vars: Vec<Symbol>,
    ) -> StructId {
        let mut fields = Vec::new();

        if let Some(fl) = field_list {
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
                    using: f.is_using(),
                });
            }
        }

        self.alloc_struct(Struct {
            kind,
            fields,
            poly_vars,
            span,
            type_refs: Vec::new(),
        })
    }

    /// Lowers `enum { RED; GREEN :: 10; }` (ADR-0041 §3).
    ///
    /// The member *values* are lowered as expressions and left unevaluated: auto-numbering
    /// and the continue-from-here rule are applied in `jr-sema`, where a constant can be
    /// read. Doing the arithmetic here would put it upstream of the only phase that can
    /// reject a bad member value, which is the mistake ADR-0039 §3a corrected for array
    /// lengths.
    fn lower_enum_type(&mut self, e: &EnumType) -> EnumId {
        let span = self.span_of_node(e.syntax());
        let mut members = Vec::new();

        if let Some(ml) = e.member_list() {
            for m in ml.members() {
                let name_tok = m.name_token();
                let name = name_tok
                    .as_ref()
                    .map(|t| self.intern(t.text()))
                    .unwrap_or_else(|| self.intern("<error>"));
                let name_span = name_tok
                    .as_ref()
                    .map(|t| self.span_of_token(t))
                    .unwrap_or(span);
                let value = m.value().map(|v| self.lower_top_expr(&v));
                members.push(EnumMember {
                    name,
                    name_span,
                    value,
                });
            }
        }

        self.alloc_enum(Enum {
            flags: e.is_flags(),
            members,
            span,
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
                // A default is a *top-level* expression, like a constant's value: it belongs to the
                // signature rather than to any body, so it goes in `FileHir::exprs`.
                let default = p.default_value().map(|e| self.lower_top_expr(&e));
                params.push(Param {
                    name,
                    name_span,
                    ty,
                    using: p.is_using(),
                    comptime: p.is_comptime(),
                    default,
                });
            }
        }

        // Return type. A `RESULT_LIST` is *not* a type node, so `rt.ty()` does not find one — the
        // list is checked for first (ADR-0052 §1), which is also what keeps `(s64, bool)` from
        // becoming a spellable type anywhere `TypeExpr` is accepted.
        let ret = ast_proc.ret_type().and_then(|rt| {
            let node = rt.syntax();
            if let Some(list) = node
                .children()
                .find(|n| n.kind() == jr_syntax::SyntaxKind::RESULT_LIST)
            {
                let elems: Vec<TypeRefId> = list
                    .children()
                    .filter_map(jr_syntax::ast::TypeExpr::cast)
                    .map(|t| self.lower_type_expr_top(&t))
                    .collect();
                return Some(self.alloc_top_type_ref(TypeRef::Results(elems)));
            }
            rt.ty().map(|t| self.lower_type_expr_top(&t))
        });

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
            c_call: ast_proc.is_c_call(),
            no_abc: ast_proc.is_no_abc(),
            ret,
            body,
            foreign,
            span,
            type_refs: Vec::new(),
        })
    }

    // ---- bodies ------------------------------------------------------------

    fn lower_body(&mut self, block: &Block, params: &[Param]) -> Body {
        let mut bctx = BodyLowerCtx::new(self.file, self.interner, self.operands);

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

    /// Lowers a **file-level** argument list, returning the values and their names (ADR-0053 §1).
    ///
    /// Walks the `ARG_LIST`'s children rather than `ArgList::args()`, because a `NAMED_ARG` is not an
    /// expression kind and that accessor skips it entirely — which would have silently dropped every
    /// named argument, the failure mode a kind-filtered walk always has.
    ///
    /// A near-identical `BodyLowerCtx::lower_args` exists for a body's own expression arena. The two
    /// are separate rather than sharing a flag because they lower into *different arenas* that both
    /// start at index 0, and passing the wrong one would resolve against a different expression —
    /// the hazard `Body::type_refs`' doc comment records for types and this repeats for expressions.
    fn lower_args(&mut self, list: &SyntaxNode) -> (Vec<ExprId>, Vec<Option<Symbol>>) {
        let mut args = Vec::new();
        let mut names = Vec::new();
        for child in list.children() {
            if let Some(named) = jr_syntax::ast::NamedArg::cast(child.clone()) {
                let span = self.span_of_node(named.syntax());
                let name = named
                    .name()
                    .and_then(|n| n.text())
                    .map(|t| self.intern(t.as_str()));
                let value = named
                    .value()
                    .map(|e| self.lower_top_expr(&e))
                    .unwrap_or_else(|| self.alloc_top_expr(Expr::Error(span), span));
                args.push(value);
                names.push(name);
                continue;
            }
            if let Some(expr) = AstExpr::cast(child) {
                args.push(self.lower_top_expr(&expr));
                names.push(None);
            }
        }
        (args, names)
    }

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
                // A `-` directly on an integer literal folds into it, so that a signed
                // minimum is expressible at all (ADR-0038 §1). Only a literal *directly*
                // under the `-`: `-x` and `-(128)` still lower to `Unary(Neg, ..)`, where
                // ADR-0002's trapping negation applies (§3).
                if op == UnOp::Neg
                    && let Some(AstExpr::Literal(lit)) = u.operand()
                    && let Some(folded) = negate_literal(&lower_literal_impl(
                        &lit,
                        span,
                        self.interner,
                        &mut self.diags,
                    ))
                {
                    return self.alloc_top_expr(Expr::Literal(folded, span), span);
                }
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
                let (args, arg_names) = match c.arg_list() {
                    Some(al) => self.lower_args(al.syntax()),
                    None => (Vec::new(), Vec::new()),
                };
                self.alloc_top_expr(
                    Expr::Call {
                        callee,
                        args,
                        arg_names,
                        span,
                    },
                    span,
                )
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
            AstExpr::Index(ix) => {
                let base = ix
                    .base()
                    .map(|e| self.lower_top_expr(&e))
                    .unwrap_or_else(|| {
                        let err = Expr::Error(span);
                        self.alloc_top_expr(err, span)
                    });
                let (index, index_span) = match ix.index() {
                    Some(e) => {
                        let s = self.span_of_node(e.syntax());
                        (self.lower_top_expr(&e), s)
                    }
                    // The parser reported the missing index (E0123); lowering keeps going
                    // with a poison expression rather than dropping the access, so the
                    // shape of the tree still matches what was written.
                    None => (self.alloc_top_expr(Expr::Error(span), span), span),
                };
                self.alloc_top_expr(
                    Expr::Index {
                        base,
                        index,
                        index_span,
                        span,
                    },
                    span,
                )
            }
            // A range is reachable **only** in a `for` header (ADR-0049 §1), so at file level it
            // is recovered syntax rather than an expression to lower. Refused rather than lowered
            // to its start, which would silently compile `X :: 0..4;` as `0`.
            AstExpr::Range(_) => self.alloc_top_expr(Expr::Error(span), span),
            AstExpr::Slice(sl) => {
                let base = sl
                    .base()
                    .map(|e| self.lower_top_expr(&e))
                    .unwrap_or_else(|| {
                        let err = Expr::Error(span);
                        self.alloc_top_expr(err, span)
                    });
                self.alloc_top_expr(Expr::Slice { base, span }, span)
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
            // `context` at file scope is refused by sema rather than here (there is no context
            // during const-eval), so lowering produces the node and sema says why.
            AstExpr::Context(_) => self.alloc_top_expr(Expr::Context(span), span),
            AstExpr::Uninit(_) => self.alloc_top_expr(Expr::Uninit(span), span),
            AstExpr::Cast(c) => {
                // Same refusal as the body case, and for the same reason.
                let Some(target) = c.target() else {
                    return self.alloc_top_expr(Expr::Error(span), span);
                };
                let Some(operand) = c.operand() else {
                    return self.alloc_top_expr(Expr::Error(span), span);
                };
                let ty = self.lower_type_expr_top(&target);
                let operand = self.lower_top_expr(&operand);
                self.alloc_top_expr(Expr::Cast { ty, operand, span }, span)
            }
            AstExpr::Autocast(a) => {
                // Same refusal shape as `cast`: no operand means `Expr::Error`, not an autocast
                // wrapped around a placeholder.
                let Some(operand) = a.operand() else {
                    return self.alloc_top_expr(Expr::Error(span), span);
                };
                let operand = self.lower_top_expr(&operand);
                self.alloc_top_expr(Expr::Autocast { operand, span }, span)
            }
            AstExpr::Member(m) => {
                let Some(token) = m.name_token() else {
                    return self.alloc_top_expr(Expr::Error(span), span);
                };
                let name = self.intern(token.text());
                let name_span = self.span_of_token(&token);
                self.alloc_top_expr(
                    Expr::Member {
                        name,
                        name_span,
                        span,
                    },
                    span,
                )
            }
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

    /// Lowers `operator + :: (…) -> T { … }` (ADR-0048 §1).
    ///
    /// The name is the **synthetic symbol** `"operator+"` — one token, no space — which is what
    /// makes the rest free: it lands in the same flat per-file name map every other constant does,
    /// so importing, shadowing and ADR-0014 §3's ambiguity reporting all work with no new
    /// mechanism. Nothing can collide with it, because a user cannot write that identifier.
    fn lower_operator_decl(&mut self, od: &jr_syntax::ast::OperatorDecl) {
        let span = self.span_of_node(od.syntax());
        let Some(token) = od.op_token() else {
            // The parser reported the missing operator (E0126). Dropping the declaration rather
            // than inventing one keeps a nonexistent `operator +` out of the name map.
            return;
        };
        let name_span = self.span_of_token(&token);
        let Some(op) = bin_op_of_token(token.kind()) else {
            // A token the parser accepted and `BinOp` has no variant for — `!` and `~` are unary,
            // and `&&`/`||` are control flow, so none of them is a `BinOp` at all. Dropped here
            // and reported by sema as a non-overloadable operator (ADR-0048 §2), which is where
            // the *reason* lives.
            return;
        };
        let Some(proc) = od.proc() else {
            // Reported by the parser (E0126) as a value that is not a procedure.
            return;
        };
        let proc_id = self.lower_proc(&proc);
        let name = self.intern(&format!("operator{}", token.text()));
        self.items.push(Item {
            exported: self.exporting,
            name: Some(name),
            span,
            name_span,
            kind: ItemKind::Const {
                value: ConstValue::Operator(proc_id, op),
            },
        });
    }

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
        } else if let Some(un) = cd.union_type() {
            let union_id = self.lower_union_type(&un);
            ItemKind::Const {
                value: ConstValue::Union(union_id),
            }
        } else if let Some(va) = cd.variant_type() {
            let variant_id = self.lower_variant_type(&va);
            ItemKind::Const {
                value: ConstValue::Variant(variant_id),
            }
        } else if let Some(en) = cd.enum_type() {
            let enum_id = self.lower_enum_type(&en);
            ItemKind::Const {
                value: ConstValue::Enum(enum_id),
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
            exported: self.exporting,
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
            exported: self.exporting,
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
            exported: self.exporting,
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
            exported: self.exporting,
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
            enums: self.enums,
            bodies: self.bodies,
            exprs: self.top_exprs,
            expr_spans: self.top_expr_spans,
            type_refs: self.top_type_refs,
            // No instantiations from ordinary lowering; the expansion pass in `jr-db` fills this on the
            // cloned tree (ADR-0082 §2).
            proc_bindings: Vec::new(),
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
    /// Evaluated computed-`#insert` operands (ADR-0073 §1, step 6), keyed by directive span.
    ///
    /// Empty in ordinary lowering. When the operand pre-pass has evaluated a `#insert`'s operand, its
    /// text is here, and `lower_insert` expands it in place exactly as the literal path does — otherwise
    /// the insert stays pending and `jr-mir` refuses the body.
    operands: &'a InsertOperands,

    exprs: Vec<Expr>,
    expr_spans: Vec<Span>,
    stmts: Vec<Stmt>,
    locals: Vec<Local>,
    type_refs: Vec<TypeRef>,
    params: Vec<Param>,

    /// Scope stack: each frame is a list of (name, entry) pairs.
    /// Innermost frame is last. Shadowing is allowed (spec §023).
    scope_stack: Vec<Vec<(Symbol, ScopeEntry)>>,

    /// The span every node gets while lowering an `#insert`, if one is in progress (ADR-0072 §2).
    ///
    /// Set to the directive's span for the duration, so `span_of_node` and `span_of_token` answer with
    /// it instead of the syntax node's own range — which for inserted text is an offset into the
    /// *string*, meaningless as a position in this file and silently **clamped** by `jr-diag` rather
    /// than rejected.
    ///
    /// Overriding the two span helpers rather than rewriting spans afterwards, and the difference is not
    /// stylistic: a `Span` lives in sixteen `Expr` fields, nineteen `Stmt` variants, `Local::name_span`
    /// and `Param::name_span`, and a rewriter would have to find all of them. The first attempt here
    /// rewrote the `expr_spans` arena and missed `Expr::Name`'s own `span` field — which is the one the
    /// *resolver* reads, so an unresolved name in inserted code reported against lines 1–2 of the file.
    /// Found by running, and it is exactly the clamping failure §2 was written to prevent. Catching it at
    /// the source is the only version that cannot be incomplete.
    span_override: Option<Span>,

    /// How many `#insert` expansions enclose the statement being lowered (ADR-0073 §3).
    ///
    /// Zero in ordinary source. Incremented for the duration of each nested expansion, so it is the
    /// *expansion depth* rather than a count of inserts: a file may contain any number of inserts, and
    /// each may expand to [`MAX_INSERT_DEPTH`].
    ///
    /// **A literal `#insert` cannot exhaust this and that is not a reason to omit it.** ADR-0072 §5
    /// established that escaping *doubles* the text at every level, so 18 levels of written nesting is
    /// 512 KB of source and the file is its own bound. A **computed** operand removes that bound
    /// entirely — a generated string can reproduce itself without growing, which is a quine — and
    /// ADR-0072 §5 named this sub-wave as the one that owes the guard. Adding it before the computed
    /// operand can produce one means the guard exists *when* the feature does, rather than after the
    /// first hang.
    insert_depth: u32,
}

impl<'a> BodyLowerCtx<'a> {
    fn new(file: FileId, interner: &'a Interner, operands: &'a InsertOperands) -> Self {
        Self {
            file,
            interner,
            operands,
            diags: Diagnostics::new(),
            exprs: Vec::new(),
            expr_spans: Vec::new(),
            stmts: Vec::new(),
            locals: Vec::new(),
            type_refs: Vec::new(),
            params: Vec::new(),
            scope_stack: vec![Vec::new()],
            span_override: None,
            insert_depth: 0,
        }
    }

    fn span_of_node(&self, node: &SyntaxNode) -> Span {
        self.span_override
            .unwrap_or_else(|| Span::new(self.file, node.text_range()))
    }

    fn span_of_token(&self, tok: &SyntaxToken) -> Span {
        self.span_override
            .unwrap_or_else(|| Span::new(self.file, tok.text_range()))
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
                // `Box(s64)` — a name with type arguments is a parameterised reference (ADR-0085 §3).
                match n.arguments() {
                    Some(arguments) => {
                        let args: Vec<TypeRefId> =
                            arguments.args().map(|t| self.lower_type_expr(&t)).collect();
                        self.alloc_type_ref(TypeRef::Apply { name: sym, args })
                    }
                    None => self.alloc_type_ref(TypeRef::Name(sym)),
                }
            }
            TypeExpr::Poly(poly) => {
                let sym = poly
                    .name_token()
                    .map(|t| self.intern(t.text()))
                    .unwrap_or_else(|| self.intern("<error>"));
                self.alloc_type_ref(TypeRef::Poly(sym))
            }
            TypeExpr::Pointer(p) => {
                let inner = if let Some(pointee) = p.pointee() {
                    self.lower_type_expr(&pointee)
                } else {
                    self.alloc_type_ref(TypeRef::Error)
                };
                self.alloc_type_ref(TypeRef::Pointer(inner))
            }
            TypeExpr::Array(a) => {
                let len_span = a.len().map_or_else(
                    || self.span_of_node(a.syntax()),
                    |e| self.span_of_node(e.syntax()),
                );
                let len = lower_array_len(a, len_span, self.interner, &mut self.diags);
                let elem = if let Some(e) = a.elem() {
                    self.lower_type_expr(&e)
                } else {
                    self.alloc_type_ref(TypeRef::Error)
                };
                self.alloc_type_ref(TypeRef::Array {
                    elem,
                    len,
                    len_name: lower_array_len_name(a, self.interner),
                    len_span,
                })
            }
            TypeExpr::View(v) => {
                let elem = if let Some(e) = v.elem() {
                    self.lower_type_expr(&e)
                } else {
                    self.alloc_type_ref(TypeRef::Error)
                };
                self.alloc_type_ref(TypeRef::View { elem })
            }
            TypeExpr::Proc(p) => {
                let params: Vec<TypeRefId> = p.params().map(|t| self.lower_type_expr(&t)).collect();
                let ret = p.ret().map(|t| self.lower_type_expr(&t));
                self.alloc_type_ref(TypeRef::Proc { params, ret })
            }
            TypeExpr::Struct(_) | TypeExpr::Union(_) | TypeExpr::Variant(_) | TypeExpr::Enum(_) => {
                // An inline aggregate type inside a body is unusual, and both arenas would
                // have to agree about where it lives; refused rather than half-lowered.
                self.alloc_type_ref(TypeRef::Error)
            }
        }
    }

    /// Lowers `#insert "…";` (ADR-0072).
    ///
    /// Four steps, and the order matters: decode the operand, refuse a non-literal, parse the text as a
    /// *statement list*, then lower those statements **into the current scope**.
    ///
    /// No scope is pushed. That is the feature: `#insert "n := 1;"` followed by `exit(n)` must resolve,
    /// which is why this produces a [`Stmt::Insert`] rather than a [`Stmt::Block`] — see that variant's
    /// docs for the two separate ways a block would have been wrong.
    ///
    /// Every statement's span is the directive's, per ADR-0072 §2: the spans the inner parse produced are
    /// offsets into the *inserted string*, and `jr-diag` clamps an out-of-range offset rather than
    /// rejecting it, so using one would silently underline source the user never wrote.
    /// Lowers `#code { … }` by splicing the body's own source text (ADR-0080 §1, §2).
    ///
    /// The **inner** text — between the braces, exclusive — because the braces are the `#code` syntax
    /// rather than part of the code: splicing them would produce a nested block, which is precisely the
    /// nested *name scope* ADR-0072 §1 says an insert must not create.
    ///
    /// Reaches the same [`Stmt::Insert`] a literal insert does, so the depth bound (E0264) and the
    /// pending-insert refusal in `jr-mir`'s `scan` apply unchanged — `#code` inherits every guarantee
    /// rather than needing its own.
    fn lower_code(&mut self, code: &jr_syntax::ast::CodeStmt, span: Span) -> StmtId {
        let Some(block) = code.block() else {
            // The parser already reported E0131 for a `#code` with no braces; refusing the statement here
            // keeps lowering from inventing an empty splice, which would be the well-typed-placeholder
            // shape AGENTS.md names.
            return self.alloc_stmt(Stmt::Error(span));
        };
        let text = block_inner_text(block.syntax());
        let stmts = self.expand_insert_text(&text, span);
        self.alloc_stmt(Stmt::Insert {
            stmts,
            operand: None,
            span,
        })
    }

    fn lower_insert(&mut self, directive: &jr_syntax::ast::DirectiveExpr, span: Span) -> StmtId {
        // **Checked before the operand is even looked at** (ADR-0073 §3), because the thing being
        // bounded is the recursion itself: `lower_stmt` calls this, which calls `lower_stmt`, and an
        // operand that reproduces itself would never reach a base case. Refused with a diagnostic
        // rather than allowed to exhaust the stack — a compiler must not abort on a program.
        if self.insert_depth >= MAX_INSERT_DEPTH {
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("`#insert` expanded more than {MAX_INSERT_DEPTH} levels deep"),
                )
                .with_code(E0264)
                .with_note(
                    "an insert whose text contains another expands recursively, and this one did not \
                     stop",
                )
                .with_help(
                    "a *literal* insert is bounded by its own text, so this usually means an insert's \
                     operand reproduces itself",
                ),
            );
            return self.alloc_stmt(Stmt::Error(span));
        }

        let Some(arg) = directive.string_arg() else {
            // No *literal* string operand. Two cases (ADR-0073 §1):
            //
            //   * a **computed** operand — `#insert S;` or `#insert #run mk();`. The operand is lowered
            //     as an ordinary expression and held in `Stmt::Insert { operand: Some(_), .. }`, so it
            //     **resolves and type-checks** like any expression: `#insert undefined;` is an
            //     unresolved-name error, and a non-`string` operand is a type error, each at the
            //     operand's own span rather than a blanket refusal. The operand pre-pass evaluates it to
            //     a string and re-lowers; until then `jr-mir`'s `scan` refuses the body (ADR-0073 §1,
            //     step 4) — which is why `stmts` is empty *and* `operand` is `Some`, a pending state
            //     distinct from an empty literal insert.
            //   * **nothing at all** — `#insert;` — the ADR-0072 §5 case, a hard error with no operand
            //     to lower.
            //
            // Neither lowers to zero statements *silently*: the pending insert is refused downstream by
            // `scan`, the well-typed-placeholder miscompile AGENTS.md names being the thing that refusal
            // exists to prevent.
            let operand = directive
                .syntax()
                .children()
                .find_map(jr_syntax::ast::Expr::cast);
            let Some(operand_expr) = operand else {
                self.diags.push(
                    Diagnostic::error(span, "`#insert` needs a string literal of Jairs source")
                        .with_code(E0262)
                        .with_help("write the code inline, e.g. `#insert \"x := 1;\";`"),
                );
                return self.alloc_stmt(Stmt::Error(span));
            };
            // The operand is real source in this file, so it lowers with no span override — unlike the
            // inserted text, whose spans are offsets into a string (ADR-0072 §2).
            let operand_id = self.lower_expr(&operand_expr);

            // If the pre-pass has evaluated this operand's text (keyed by the directive's span,
            // ADR-0073 step 6), expand it in place exactly as a literal and clear `operand` to `None`:
            // an expanded insert is *identical* to a literal one — statements in place, nothing left to
            // evaluate — and clearing the operand is what makes it so. This matters for the empty case:
            // `#insert EMPTY;` where `EMPTY` is `""` expands to **zero** statements, and if `operand`
            // stayed `Some` that would be indistinguishable from the *pending* state (`operand: Some`,
            // empty `stmts`) that `jr-mir` refuses. `operand: None` says "evaluated, and it was empty",
            // which is legal — the same rule a literal `#insert "";` follows (ADR-0072 §5).
            //
            // `operand_id` is dropped, which costs a little dump provenance and is worth it: the
            // alternative (a third field, "was expanded") is state that every match must thread through
            // to say what `None` already says.
            if let Some(text) = self.operands.get(span) {
                let text = text.to_owned();
                let _ = operand_id;
                let stmts = self.expand_insert_text(&text, span);
                return self.alloc_stmt(Stmt::Insert {
                    stmts,
                    operand: None,
                    span,
                });
            }
            // Not yet evaluated: *pending* — empty statements, `operand: Some`, refused by `jr-mir`'s
            // `scan` so it can never be mistaken for "insert nothing".
            return self.alloc_stmt(Stmt::Insert {
                stmts: Vec::new(),
                operand: Some(operand_id),
                span,
            });
        };

        // Decoded rather than merely unquoted, so `#insert "s := \"hi\";"` inserts what it looks like.
        // The same function every string literal goes through, which is what keeps one answer to "what
        // does this escape mean".
        let text = decode_string_impl(arg.text(), span, &mut self.diags);
        let stmts = self.expand_insert_text(&text, span);
        // A **literal** insert: its text is already lowered into `stmts`, so there is no operand
        // expression to evaluate (ADR-0073 §1).
        self.alloc_stmt(Stmt::Insert {
            stmts,
            operand: None,
            span,
        })
    }

    /// Parses inserted `text` as a statement list and lowers it **into the enclosing scope** (ADR-0072
    /// §1), returning the lowered statement ids. Shared by the literal path and the computed path once
    /// the operand pre-pass has evaluated the text — the two differ only in *where the string came from*,
    /// so the parse-and-lower is one function with one answer.
    ///
    /// `span` is the directive's, and every node produced takes it via `span_override` (ADR-0072 §2): the
    /// inner parse's spans are offsets into `text`, which `jr-diag` would clamp onto unrelated bytes of
    /// the real file.
    fn expand_insert_text(&mut self, text: &str, span: Span) -> Vec<StmtId> {
        let parsed = jr_syntax::parse_stmts(text, self.file);
        for diag in parsed.diagnostics().iter() {
            // Re-pointed at the directive and re-worded, because the inner diagnostic's span is an
            // offset into `text` and would land on unrelated bytes of the real file (ADR-0072 §3). The
            // offset is carried in a note, which is the part a reader needs to find their mistake.
            let offset = u32::from(diag.primary.span.range.start());
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("inserted code does not parse: {}", diag.message),
                )
                .with_code(E0263)
                .with_note(format!("in inserted code, at offset {offset}"))
                .with_note(format!("the inserted text was: {text}")),
            );
        }

        // **Every span produced below is the directive's** (ADR-0072 §2), set here rather than fixed up
        // afterwards — see `span_override`'s docs for why a fix-up cannot be made complete. Saved and
        // restored rather than cleared, so a nested lowering that is *not* an insert still works if one
        // ever reaches here.
        let outer_override = self.span_override;
        self.span_override = Some(span);

        // One level deeper for the duration, so an insert *inside* this text sees its true depth
        // (ADR-0073 §3). Saved and restored rather than reset, for the same reason `span_override` is:
        // this is a property of where lowering currently *is*, not of the file.
        let outer_depth = self.insert_depth;
        self.insert_depth = outer_depth + 1;

        // Lowered in the **enclosing** scope. `parse_stmts` roots the result in a `BLOCK`, so its
        // statements are reached through that node — but `lower_block` is deliberately not called on it,
        // since that would push a scope and hide every name the insert declares.
        let stmts = match jr_syntax::ast::Block::cast(parsed.syntax()) {
            Some(block) => block
                .stmts()
                .map(|inner| self.lower_stmt(&inner))
                .collect::<Vec<_>>(),
            // `parse_stmts` always roots a `BLOCK`, so this is unreachable; recorded rather than
            // `expect`ed, because a panic in lowering is the one failure a user cannot act on.
            None => Vec::new(),
        };

        self.insert_depth = outer_depth;
        self.span_override = outer_override;
        stmts
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
            // A `TARGET_LIST` child means this is a destructuring form (ADR-0052 §2) rather than an
            // ordinary declaration or assignment — the parser reuses `DECL_STMT`/`ASSIGN_STMT` for
            // both, so the *presence of the list* is what distinguishes them.
            AstStmt::Decl(d) if has_target_list(d.syntax()) => {
                self.lower_local_tuple(d.syntax(), span)
            }
            AstStmt::Decl(d) => self.lower_decl_stmt(d, span),
            // An `#insert "…";` statement is intercepted **before** the ordinary expression path
            // (ADR-0072 §1): its operand is source text to parse, not a value to lower. Recognised by
            // the directive's name, because the parser gives every `#name "arg"` the same generic node
            // — which is what lets a directive be added without a grammar change.
            AstStmt::Expr(e) if insert_directive(e).is_some() => {
                let directive = insert_directive(e).expect("the guard just matched");
                self.lower_insert(&directive, span)
            }
            // `#code { … }` splices its body's **source text**, through the same path a literal
            // `#insert "…"` takes (ADR-0080 §2). The body was already parsed as ordinary statements, so
            // its faults were reported where they are written; what is reused here is the *splice*, which
            // is what puts the statements in the **enclosing** scope rather than a nested one.
            //
            // Re-parsing the text rather than lowering the block we already have is deliberate: a
            // `Stmt::Block`'s statements go into a nested name scope (ADR-0072 §1's whole point is that an
            // insert's do not), and `Stmt::Insert` is the variant that carries "these statements belong to
            // the enclosing body". The cost is that the spans become the directive's (ADR-0080 §2), which
            // that section records as a debt shared with `#insert` rather than a surprise.
            AstStmt::Code(c) => self.lower_code(c, span),
            AstStmt::Expr(e) => {
                let expr_id = e
                    .expr()
                    .map(|ex| self.lower_expr(&ex))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Error(span), span));
                self.alloc_stmt(Stmt::Expr(expr_id, span))
            }
            AstStmt::Assign(a) if has_target_list(a.syntax()) => {
                self.lower_assign_tuple(a.syntax(), span)
            }
            AstStmt::Assign(a) => self.lower_assign_stmt(a, span),
            AstStmt::If(i) => self.lower_if_stmt(i, span),
            AstStmt::While(w) => self.lower_while_stmt(w, span, None),
            AstStmt::For(f) => self.lower_for_stmt(f, span, None),
            // The deferred statement is lowered *here*, once, and `jr-mir` emits it before every
            // terminator that leaves the scope (ADR-0049 §3). Lowering it once means the deferred
            // expression has one identity, so a `TypeMap` entry and a `ResolveMap` entry each exist
            // exactly once — which is what keeps the duplication in MIR rather than in HIR.
            AstStmt::Defer(d) => {
                let inner = d
                    .stmt()
                    .map(|b| self.lower_control_body(&b))
                    .unwrap_or_else(|| self.alloc_stmt(Stmt::Error(span)));
                self.alloc_stmt(Stmt::Defer(inner, span))
            }
            // `push_context { … }` (ADR-0063). The body is lowered as an ordinary block — its
            // statements resolve names and types exactly as they would outside the wrapper — and
            // `jr-mir` is where the context copy and the pointer swap live. Holding the block rather
            // than inlining its statements keeps the scope one identity, so the copy happens once.
            // `switch e { case v; … }` (ADR-0067). Each arm's value is lowered as an ordinary
            // expression — cases are values, not patterns (§2) — and its statements become a block, so
            // an arm's scope is a block's scope and needs no new rule.
            AstStmt::Switch(sw) => {
                let value = sw
                    .value()
                    .map(|e| self.lower_expr(&e))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Error(span), span));
                let arms = sw
                    .arms()
                    .map(|arm| {
                        let arm_span = self.span_of_node(arm.syntax());
                        // The `else` arm has no value, and that absence is the catch-all. A malformed
                        // `case ;` also has none, so `is_else` reads the *keyword* — treating a syntax
                        // error as a catch-all would make it silently exhaustive.
                        let value = if arm.is_else() {
                            None
                        } else {
                            Some(arm.value().map(|e| self.lower_expr(&e)).unwrap_or_else(|| {
                                self.alloc_expr(Expr::Error(arm_span), arm_span)
                            }))
                        };
                        let stmts: Vec<StmtId> =
                            arm.body().map(|stmt| self.lower_stmt(&stmt)).collect();
                        let body = self.alloc_stmt(Stmt::Block(stmts, arm_span));
                        crate::hir::SwitchArm {
                            value,
                            body,
                            span: arm_span,
                        }
                    })
                    .collect();
                self.alloc_stmt(Stmt::Switch { value, arms, span })
            }
            AstStmt::PushContext(p) => {
                let inner = p
                    .block()
                    .map(|b| self.lower_block(&b))
                    .unwrap_or_else(|| self.alloc_stmt(Stmt::Error(span)));
                self.alloc_stmt(Stmt::PushContext(inner, span))
            }
            // `label: for …` — the label is carried *on the loop* rather than in a wrapper
            // statement, so `jr-mir`'s loop stack has it without walking outward for a parent.
            AstStmt::Labelled(l) => {
                let label = l
                    .name()
                    .and_then(|n| n.text())
                    .map(|t| self.intern(t.as_str()));
                let Some(inner) = l.loop_stmt() else {
                    return self.alloc_stmt(Stmt::Error(span));
                };
                let inner_span = self.span_of_node(&inner);
                match jr_syntax::ast::Stmt::cast(inner) {
                    Some(AstStmt::For(f)) => self.lower_for_stmt(&f, inner_span, label),
                    Some(AstStmt::While(w)) => self.lower_while_stmt(&w, inner_span, label),
                    _ => self.alloc_stmt(Stmt::Error(span)),
                }
            }
            AstStmt::Return(r) => {
                // Several expressions means a multi-value return (ADR-0052 §1). Counted here rather
                // than given its own node kind, because one expression is the ordinary case and a
                // list of one would have to be unwrapped everywhere downstream.
                let exprs: Vec<ExprId> = r
                    .syntax()
                    .children()
                    .filter_map(jr_syntax::ast::Expr::cast)
                    .map(|e| self.lower_expr(&e))
                    .collect();
                match exprs.len() {
                    0 => self.alloc_stmt(Stmt::Return(None, span)),
                    1 => self.alloc_stmt(Stmt::Return(Some(exprs[0]), span)),
                    _ => self.alloc_stmt(Stmt::ReturnTuple(exprs, span)),
                }
            }
            AstStmt::Break(b) => {
                let label = b
                    .label()
                    .and_then(|n| n.text())
                    .map(|t| self.intern(t.as_str()));
                self.alloc_stmt(Stmt::Break(label, span))
            }
            AstStmt::Continue(c) => {
                let label = c
                    .label()
                    .and_then(|n| n.text())
                    .map(|t| self.intern(t.as_str()));
                self.alloc_stmt(Stmt::Continue(label, span))
            }
        }
    }

    fn lower_decl_stmt(&mut self, d: &DeclStmt, span: Span) -> StmtId {
        let Some(inner) = d.decl() else {
            return self.alloc_stmt(Stmt::Error(span));
        };

        match inner {
            // An `operator` declaration is file-level only: it names no scope and a body-local
            // one would be visible to nothing. The parser reaches this arm only for a nested
            // declaration statement, so refusing here is refusing a construct nobody can use.
            AstItem::Operator(od) => {
                let od_span = self.span_of_node(od.syntax());
                self.alloc_stmt(Stmt::Error(od_span))
            }
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
                    using: vd.is_using(),
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

    /// Lowers `q, ok := f();` (ADR-0052 §2).
    ///
    /// A target whose text is `_` becomes `None` — a **hole**, not a local — which is what keeps a
    /// discard out of the resolve map and out of the locals arena entirely (ADR-0052 §3). Recognised
    /// by text rather than by token kind, because `_` lexes as an ordinary identifier in Jairs and
    /// reserving it globally would break any program already using it as a name.
    fn lower_local_tuple(&mut self, node: &SyntaxNode, span: Span) -> StmtId {
        let targets = self.lower_targets(node);
        let call = self.lower_tuple_rhs(node, span);
        let mut locals = Vec::with_capacity(targets.len());
        for name in &targets {
            match name {
                Some((sym, name_span)) => {
                    let id = self.alloc_local(Local {
                        name: *sym,
                        name_span: *name_span,
                        // No annotation is possible in this form: each local's type *is* the
                        // matching result's, which sema fills in.
                        ty: None,
                        init: None,
                        uninit: false,
                        using: false,
                        span,
                    });
                    self.define_local(*sym, id);
                    locals.push(Some(id));
                }
                None => locals.push(None),
            }
        }
        self.alloc_stmt(Stmt::LocalTuple {
            targets: locals,
            call,
            span,
        })
    }

    /// Lowers `q, ok = f();` (ADR-0052 §2), whose targets are existing places.
    fn lower_assign_tuple(&mut self, node: &SyntaxNode, span: Span) -> StmtId {
        let targets = self.lower_targets(node);
        let call = self.lower_tuple_rhs(node, span);
        let mut places = Vec::with_capacity(targets.len());
        for name in &targets {
            match name {
                Some((sym, name_span)) => {
                    // A target is a *name expression*, so it goes through the ordinary expression
                    // path and gets an ordinary `Res` — which is what makes `is_place` and the
                    // assignability rule apply to it unchanged.
                    // The same conversion the ordinary name path does, so a destructuring target
                    // resolves exactly as `q = 1` would — one rule for what a name means.
                    let res = self
                        .lookup_local(*sym)
                        .map(|e| match e {
                            ScopeEntry::Local(id) => Res::Local(id),
                            ScopeEntry::Param(id) => Res::Param(id),
                        })
                        .unwrap_or(Res::Error);
                    let expr = self.alloc_expr(
                        Expr::Name {
                            name: *sym,
                            span: *name_span,
                            res,
                        },
                        *name_span,
                    );
                    places.push(Some(expr));
                }
                None => places.push(None),
            }
        }
        self.alloc_stmt(Stmt::AssignTuple {
            targets: places,
            call,
            span,
        })
    }

    /// The names in a `TARGET_LIST`, with `_` as `None`.
    fn lower_targets(&mut self, node: &SyntaxNode) -> Vec<Option<(Symbol, Span)>> {
        let Some(list) = node
            .children()
            .find(|n| n.kind() == jr_syntax::SyntaxKind::TARGET_LIST)
        else {
            return Vec::new();
        };
        list.children()
            .filter(|n| n.kind() == jr_syntax::SyntaxKind::NAME)
            .map(|name| {
                let text = name.text().to_string();
                let span = self.span_of_node(&name);
                if text.trim() == "_" {
                    None
                } else {
                    Some((self.intern(text.trim()), span))
                }
            })
            .collect()
    }

    /// The call on the right of a destructuring statement.
    fn lower_tuple_rhs(&mut self, node: &SyntaxNode, span: Span) -> ExprId {
        node.children()
            .find(|n| jr_syntax::ast::Expr::cast(n.clone()).is_some())
            .and_then(jr_syntax::ast::Expr::cast)
            .map(|e| self.lower_expr(&e))
            .unwrap_or_else(|| self.alloc_expr(Expr::Error(span), span))
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

    /// Lowers an `if`/`else`/`while` body, braced or not.
    ///
    /// An unbraced single statement is wrapped in a synthetic one-statement block.
    /// Two reasons: it keeps the invariant that `Stmt::If::then` and
    /// `Stmt::While::body` are always a `Stmt::Block`, which every consumer
    /// downstream already relies on; and it gives the statement its own scope, so
    /// `if c x := 1;` scopes `x` exactly as `if c { x := 1; }` does rather than
    /// leaking it into the enclosing block.
    fn lower_control_body(&mut self, body: &ControlBody) -> StmtId {
        match body {
            ControlBody::Block(block) => self.lower_block(block),
            ControlBody::Stmt(stmt) => {
                let span = self.span_of_node(stmt.syntax());
                self.push_scope();
                let inner = self.lower_stmt(stmt);
                self.pop_scope();
                self.alloc_stmt(Stmt::Block(vec![inner], span))
            }
        }
    }

    fn lower_if_stmt(&mut self, i: &IfStmt, span: Span) -> StmtId {
        let cond = i
            .condition()
            .map(|e| self.lower_expr(&e))
            .unwrap_or_else(|| self.alloc_expr(Expr::Error(span), span));
        let then = i
            .then_body()
            .map(|b| self.lower_control_body(&b))
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
        } else if let Some(else_body) = eb.else_body() {
            self.lower_control_body(&else_body)
        } else {
            self.alloc_stmt(Stmt::Error(span))
        }
    }

    /// Lowers `for x: iterable { … }` (ADR-0049 §1).
    ///
    /// The loop variables are **real locals**, so they obey the ordinary promotion rules and
    /// `x = 0` inside the body modifies a copy rather than the sequence — which follows from `x`
    /// being a local rather than needing a rule of its own.
    ///
    /// Order matters here: the iterable is lowered *before* the scope is pushed, so
    /// `for x: x` refers to an outer `x` rather than to itself.
    fn lower_for_stmt(&mut self, f: &ForStmt, span: Span, label: Option<Symbol>) -> StmtId {
        let iterable = self.lower_for_iterable(f, span);

        self.push_scope();
        let value = self.bind_loop_local(f.value_name().as_ref(), span);
        let index = f.index_name().map(|n| self.bind_loop_local(Some(&n), span));
        let body = f
            .body()
            .map(|b| self.lower_control_body(&b))
            .unwrap_or_else(|| self.alloc_stmt(Stmt::Error(span)));
        self.pop_scope();

        self.alloc_stmt(Stmt::For {
            value,
            index,
            iterable,
            reverse: f.is_reverse(),
            body,
            label,
            span,
        })
    }

    /// Allocates and binds one `for` loop variable.
    ///
    /// It has no annotation and no initialiser: its type comes from the iterable (`jr-sema`'s job)
    /// and its value from the loop (`jr-mir`'s). `uninit` is **false**, because a loop variable is
    /// assigned on every iteration that runs — marking it uninitialised would make the
    /// definite-assignment pass report a variable the loop guarantees.
    fn bind_loop_local(&mut self, name: Option<&jr_syntax::ast::Name>, span: Span) -> LocalId {
        let (sym, name_span) = match name {
            Some(n) => {
                let text = n.text().unwrap_or_else(|| String::from("<error>"));
                (self.intern(&text), self.span_of_node(n.syntax()))
            }
            None => (self.intern("<error>"), span),
        };
        let id = self.alloc_local(Local {
            name: sym,
            name_span,
            ty: None,
            init: None,
            uninit: false,
            // A `for`'s loop variable is never `using`: there is no syntax for it, and
            // ADR-0050 §6 leaves `using` in a `for` header deliberately absent.
            using: false,
            span,
        });
        self.define_local(sym, id);
        id
    }

    /// Lowers a `for` header's iterable, recognising `a..b` as a range (ADR-0049 §1).
    fn lower_for_iterable(&mut self, f: &ForStmt, span: Span) -> ForIterable {
        match f.iterable() {
            Some(jr_syntax::ast::Expr::Range(r)) => {
                let start = r
                    .start()
                    .map(|e| self.lower_expr(&e))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Error(span), span));
                let end = r
                    .end()
                    .map(|e| self.lower_expr(&e))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Error(span), span));
                ForIterable::Range { start, end }
            }
            Some(other) => ForIterable::Sequence(self.lower_expr(&other)),
            None => ForIterable::Sequence(self.alloc_expr(Expr::Error(span), span)),
        }
    }

    fn lower_while_stmt(&mut self, w: &WhileStmt, span: Span, label: Option<Symbol>) -> StmtId {
        let cond = w
            .condition()
            .map(|e| self.lower_expr(&e))
            .unwrap_or_else(|| self.alloc_expr(Expr::Error(span), span));
        let body = w
            .body()
            .map(|b| self.lower_control_body(&b))
            .unwrap_or_else(|| self.alloc_stmt(Stmt::Error(span)));
        self.alloc_stmt(Stmt::While {
            cond,
            body,
            label,
            span,
        })
    }

    /// Lowers a **body-level** argument list (ADR-0053 §1).
    ///
    /// See `LowerCtx::lower_args` for why this is a separate function rather than a shared one with a
    /// flag: the two arenas both start at index 0.
    fn lower_args(&mut self, list: &SyntaxNode) -> (Vec<ExprId>, Vec<Option<Symbol>>) {
        let mut args = Vec::new();
        let mut names = Vec::new();
        for child in list.children() {
            if let Some(named) = jr_syntax::ast::NamedArg::cast(child.clone()) {
                let span = self.span_of_node(named.syntax());
                let name = named
                    .name()
                    .and_then(|n| n.text())
                    .map(|t| self.intern(t.as_str()));
                let value = named
                    .value()
                    .map(|e| self.lower_expr(&e))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Error(span), span));
                args.push(value);
                names.push(name);
                continue;
            }
            if let Some(expr) = AstExpr::cast(child) {
                args.push(self.lower_expr(&expr));
                names.push(None);
            }
        }
        (args, names)
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
                // A `-` directly on an integer literal folds into it, so that a signed
                // minimum is expressible at all (ADR-0038 §1). Only a literal *directly*
                // under the `-`: `-x` and `-(128)` still lower to `Unary(Neg, ..)`, where
                // ADR-0002's trapping negation applies (§3).
                if op == UnOp::Neg
                    && let Some(AstExpr::Literal(lit)) = u.operand()
                    && let Some(folded) = negate_literal(&lower_literal_impl(
                        &lit,
                        span,
                        self.interner,
                        &mut self.diags,
                    ))
                {
                    return self.alloc_expr(Expr::Literal(folded, span), span);
                }
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
                let (args, arg_names) = match c.arg_list() {
                    Some(al) => self.lower_args(al.syntax()),
                    None => (Vec::new(), Vec::new()),
                };
                self.alloc_expr(
                    Expr::Call {
                        callee,
                        args,
                        arg_names,
                        span,
                    },
                    span,
                )
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
            AstExpr::Index(ix) => {
                let base = ix
                    .base()
                    .map(|e| self.lower_expr(&e))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Error(span), span));
                let (index, index_span) = match ix.index() {
                    Some(e) => {
                        let s = self.span_of_node(e.syntax());
                        (self.lower_expr(&e), s)
                    }
                    None => (self.alloc_expr(Expr::Error(span), span), span),
                };
                self.alloc_expr(
                    Expr::Index {
                        base,
                        index,
                        index_span,
                        span,
                    },
                    span,
                )
            }
            AstExpr::Range(_) => self.alloc_expr(Expr::Error(span), span),
            AstExpr::Slice(sl) => {
                let base = sl
                    .base()
                    .map(|e| self.lower_expr(&e))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Error(span), span));
                self.alloc_expr(Expr::Slice { base, span }, span)
            }
            AstExpr::Deref(d) => {
                let ptr = d
                    .pointer()
                    .map(|e| self.lower_expr(&e))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Error(span), span));
                self.alloc_expr(Expr::Deref(ptr, span), span)
            }
            AstExpr::Context(_) => self.alloc_expr(Expr::Context(span), span),
            AstExpr::Uninit(_) => self.alloc_expr(Expr::Uninit(span), span),
            AstExpr::Cast(c) => {
                // A `cast` missing its type or its operand lowers to `Expr::Error`, not to a
                // cast with a placeholder inside it. `TypeRef::Error` and a poison operand
                // both flow through sema silently (ADR-0016's poison rule), so a cast built
                // around one would type-check and then convert *something* — a well-typed
                // placeholder standing in for a construct that was never written, which is
                // this project's first named failure mode.
                let Some(target) = c.target() else {
                    return self.alloc_expr(Expr::Error(span), span);
                };
                let Some(operand) = c.operand() else {
                    return self.alloc_expr(Expr::Error(span), span);
                };
                let ty = self.lower_type_expr(&target);
                let operand = self.lower_expr(&operand);
                self.alloc_expr(Expr::Cast { ty, operand, span }, span)
            }
            AstExpr::Autocast(a) => {
                let Some(operand) = a.operand() else {
                    return self.alloc_expr(Expr::Error(span), span);
                };
                let operand = self.lower_expr(&operand);
                self.alloc_expr(Expr::Autocast { operand, span }, span)
            }
            AstExpr::Member(m) => {
                let Some(token) = m.name_token() else {
                    return self.alloc_expr(Expr::Error(span), span);
                };
                let name = self.intern(token.text());
                let name_span = self.span_of_token(&token);
                self.alloc_expr(
                    Expr::Member {
                        name,
                        name_span,
                        span,
                    },
                    span,
                )
            }
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

/// The [`BinOp`] a token spells, or `None` for one that is not a binary operator.
///
/// Shared by [`lower_bin_op`] and the `operator` declaration, so the two cannot disagree about
/// which token means which operator — the `op_token` trap from ADR-0042, where three
/// kind-filtered matchers each paired with a `_ =>` arm producing `Add`.
fn bin_op_of_token(kind: SyntaxKind) -> Option<BinOp> {
    Some(match kind {
        PLUS => BinOp::Add,
        MINUS => BinOp::Sub,
        STAR => BinOp::Mul,
        SLASH => BinOp::Div,
        PERCENT => BinOp::Rem,
        PLUS_PERCENT => BinOp::WrapAdd,
        MINUS_PERCENT => BinOp::WrapSub,
        STAR_PERCENT => BinOp::WrapMul,
        EQ_EQ => BinOp::Eq,
        BANG_EQ => BinOp::Ne,
        LT => BinOp::Lt,
        LT_EQ => BinOp::Le,
        GT => BinOp::Gt,
        GT_EQ => BinOp::Ge,
        AMP => BinOp::BitAnd,
        PIPE => BinOp::BitOr,
        CARET => BinOp::BitXor,
        SHL => BinOp::Shl,
        SHR => BinOp::Shr,
        AMP_AMP => BinOp::And,
        PIPE_PIPE => BinOp::Or,
        _ => return None,
    })
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
        Some(AMP) => BinOp::BitAnd,
        Some(PIPE) => BinOp::BitOr,
        Some(CARET) => BinOp::BitXor,
        Some(SHL) => BinOp::Shl,
        Some(SHR) => BinOp::Shr,
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
        Some(TILDE) => UnOp::BitNot,
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
        AMP_EQ => AssignOp::BitAndAssign,
        PIPE_EQ => AssignOp::BitOrAssign,
        CARET_EQ => AssignOp::BitXorAssign,
        SHL_EQ => AssignOp::ShlAssign,
        SHR_EQ => AssignOp::ShrAssign,
        _ => AssignOp::Assign, // error recovery
    }
}

/// Reads the literal length of an `[N]T`, or `None` if it was not one (ADR-0039 §3a).
///
/// **Emits no diagnostic.** A bad length is reported by `jr-sema` as E0233, not here, and
/// the reason is a contract rather than a preference: `tests/corpus/type-errors/` requires
/// its files to lex, parse, lower and resolve *cleanly* and be rejected by sema alone, so
/// a lowering error would make `[COUNT]u8` untestable in the directory where every other
/// rejected type lives.
///
/// This function's job is narrower than it looks: it reaches the literal *token*, which
/// only the AST has. Whether the resulting length is acceptable is sema's call.
///
/// Shared by the top-level and body type-lowering paths so that `[COUNT]u8` behaves
/// identically wherever it is written; the two paths have separate arenas and had already
/// drifted once for pointers.
/// Reads the literal length of an `[N]T`, or `None` if it was not one (ADR-0039 §3a).
///
/// **Emits no diagnostic.** A bad length is reported by `jr-sema` as E0233, not here, and
/// the reason is a contract rather than a preference: `tests/corpus/type-errors/` requires
/// its files to lex, parse, lower and resolve *cleanly* and be rejected by sema alone, so
/// a lowering error would make `[COUNT]u8` untestable in the directory where every other
/// rejected type lives.
///
/// This function's job is narrower than it looks: it reaches the literal *token*, which
/// only the AST has. Whether the resulting length is acceptable is sema's call.
///
/// Shared by the top-level and body type-lowering paths so that `[COUNT]u8` behaves
/// identically wherever it is written; the two paths have separate arenas and had already
/// drifted once for pointers.
/// The bare **name** an array length was written as, if it was one (ADR-0070 §1).
///
/// `None` for a literal (which `lower_array_len` reads) and for anything else — `[2 + 2]u8` names nothing
/// to look up, so sema reports it rather than this guessing.
///
/// Lowering only *reads* the name; whether it resolves to a usable constant is a semantic judgement and
/// therefore sema's, which is the same split ADR-0039 §3a drew for the literal.
fn lower_array_len_name(ty: &ArrayType, interner: &Interner) -> Option<Symbol> {
    let AstExpr::Name(name) = ty.len()? else {
        return None;
    };
    Some(interner.intern(name.name_token()?.text()))
}

fn lower_array_len(
    ty: &ArrayType,
    len_span: Span,
    interner: &Interner,
    diags: &mut Diagnostics,
) -> Option<u64> {
    let AstExpr::Literal(lit) = ty.len()? else {
        return None;
    };
    // The literal is a signed `i128` since ADR-0038, so a negative length and one too
    // large for a `u64` are both visible here rather than after a lossy conversion. Both
    // yield `None` and sema explains which.
    match lower_literal_impl(&lit, len_span, interner, diags) {
        Literal::Int { value, .. } => u64::try_from(value).ok(),
        // `[1.5]u8` is not an array length. Sema reports it as E0233 like any other
        // non-integer-literal length; there is deliberately no float-specific message,
        // because "an array length must be an integer literal" already says it.
        Literal::Float { .. } | Literal::Bool(_) | Literal::Str(_) | Literal::Null => None,
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
        NULL_KW => Literal::Null,
        STRING_LITERAL => {
            let raw = tok.text();
            let decoded = decode_string_impl(raw, span, diags);
            Literal::Str(decoded)
        }
        INT_LITERAL => {
            let raw = tok.text();
            parse_int_literal_impl(raw)
        }
        FLOAT_LITERAL => {
            // Underscores are digit separators here too, so `1_000.5` parses.
            let cleaned: String = tok.text().chars().filter(|&c| c != '_').collect();
            match cleaned.parse::<f64>() {
                // `1e400` parses to `inf` rather than failing, and ADR-0040 §1 makes that a
                // legitimate value — so there is no overflow check and no diagnostic.
                Ok(value) => Literal::Float {
                    bits: value.to_bits(),
                    malformed: false,
                },
                // Unreachable in practice: the lexer only produces `FLOAT_LITERAL` for text
                // `f64`'s parser accepts. Recorded rather than assumed away, and *not* a
                // diagnostic, because a diagnostic nothing can trigger is untestable.
                Err(_) => Literal::Float {
                    bits: 0,
                    malformed: true,
                },
            }
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

    // Parsed as `u64` because that is the widest *magnitude* the source can spell — a
    // literal has no sign of its own; `negate_literal` applies one afterwards (ADR-0038 §1).
    match u64::from_str_radix(digits, radix) {
        Ok(value) => Literal::Int {
            value: i128::from(value),
            radix,
            // `u64::MAX` is a legal `u64`, so the widest *positive* literal that fits nothing
            // is one past it — which `u64::from_str_radix` cannot produce, so this is `false`
            // for every value that parses. Kept as a field because the `Err` arm below does
            // set it, and because a negated literal can also overflow (see `negate_literal`).
            overflowed: false,
        },
        // Too large for even `u64`. Clamped rather than rejected here, because lowering does
        // not diagnose: `overflowed` is what makes sema reject it for every integer type.
        Err(_) => Literal::Int {
            value: i128::from(u64::MAX),
            radix,
            overflowed: true,
        },
    }
}

/// Folds a leading `-` into an integer literal (ADR-0038 §1).
///
/// `None` when the literal is not an integer, which leaves the caller to lower an ordinary
/// `Unary(Neg, …)`.
///
/// # Why this exists rather than a flag in sema
///
/// Because the minimum of a two's-complement type is not the negation of anything the type can
/// hold. `-128` as an `s8` needs the sign to be *part of* the literal: negating 128 in `s8`
/// overflows, so `jr_pool::int_negate` traps on it, and teaching only the fit check to accept
/// it would move the failure from compile time to run time (ADR-0038 §1's rejected
/// alternative).
fn negate_literal(literal: &Literal) -> Option<Literal> {
    let Literal::Int {
        value,
        radix,
        overflowed,
    } = literal
    else {
        return None;
    };
    let negated = value.checked_neg()?;
    Some(Literal::Int {
        value: negated,
        radix: *radix,
        // A magnitude too large to parse stays too large once negated. Recomputed rather than
        // copied so that the field means the same thing on both sides of the fold.
        overflowed: *overflowed,
    })
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
    // The ordinary entry point lowers with **no** evaluated inserts — every computed `#insert` stays
    // pending, exactly as before this feature existed. The operand pre-pass calls
    // [`lower_file_with_inserts`] instead once it has the strings (ADR-0073 §1, step 6).
    lower_file_with_inserts(parse, file, interner, &InsertOperands::new())
}

/// Lowers a parsed tree into HIR, expanding each computed `#insert` whose operand `operands` has
/// evaluated (ADR-0073 §1, step 6).
///
/// Identical to [`lower_file`] except that a pending computed insert whose directive span appears in
/// `operands` is expanded in place — its evaluated text parsed and lowered like a literal — rather than
/// left pending for `jr-mir` to refuse. Passing an empty map is exactly [`lower_file`], which is why the
/// two share every line below.
pub fn lower_file_with_inserts(
    parse: &jr_syntax::Parse,
    file: FileId,
    interner: &Interner,
    operands: &InsertOperands,
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
                enums: Vec::new(),
                bodies: Vec::new(),
                exprs: Vec::new(),
                expr_spans: Vec::new(),
                type_refs: Vec::new(),
                proc_bindings: Vec::new(),
            },
            diags,
        );
    };

    let mut ctx = LowerCtx::new(file, interner, operands);

    // Walked as **children** rather than through `source_file.items()`, because a `SCOPE_DECL` is
    // not an `Item` kind and that accessor would skip it — so every visibility marker would be
    // invisible and every declaration would stay exported (ADR-0054 §1). The same kind-filtered
    // walk that dropped named arguments one wave earlier.
    for child in source_file.syntax().children() {
        // A marker changes the visibility of everything *after* it, so it is applied to the context
        // as the walk passes it. That is what makes this a position in the file rather than a
        // property of a declaration.
        if let Some(scope) = jr_syntax::ast::ScopeDecl::cast(child.clone()) {
            let exported = scope
                .directive()
                .map(|token| token.text() != "#scope_module")
                .unwrap_or(true);
            ctx.exporting = exported;
            continue;
        }
        let Some(item) = AstItem::cast(child) else {
            continue;
        };
        match item {
            AstItem::Const(cd) => ctx.lower_const_decl(&cd),
            AstItem::Operator(od) => ctx.lower_operator_decl(&od),
            AstItem::Var(vd) => ctx.lower_var_decl_item(&vd),
            AstItem::Import(id) => ctx.lower_import_decl(&id),
            AstItem::Run(rd) => ctx.lower_run_decl(&rd),
        }
    }

    ctx.finish()
}

/// The `#insert` directive an expression statement *is*, if it is one (ADR-0072 §1).
///
/// Matched on the directive's **name**, because the parser gives every `#name "arg"` the same generic
/// `DIRECTIVE_EXPR` node — the permissiveness that lets a directive be added without a grammar or lexer
/// change, and which `DIRECTIVES_VALID_AS_EXPRESSIONS` documents the cost of.
///
/// `None` for anything else, including a nested `#insert` inside a larger expression: `x := #insert "…"`
/// is not an insert, it is an `#insert` in value position, and it reaches the ordinary directive path
/// where E0209 refuses it. That is deliberate — an insert produces *statements*, so it has no value.
/// The source text **inside** a block's braces, exclusive (ADR-0080 §1).
///
/// The braces belong to the `#code` syntax rather than to the code, so splicing them would wrap the
/// statements in a nested block — and a block is a nested *name scope*, which is exactly what ADR-0072 §1
/// says an insert must not create. Taking the inner text keeps `#code { n := 1; }` equivalent to
/// `#insert "n := 1;"`, which is what makes the next line able to read `n`.
///
/// Works because the CST is lossless: every token and trivium is present, so a node's text is the original
/// source. That is the property ADR-0080 relies on to need no new representation at all.
fn block_inner_text(block: &jr_syntax::SyntaxNode) -> String {
    let text = block.text().to_string();
    // The braces are the first and last non-trivia characters of a `BLOCK`, so trimming them off the
    // trimmed text is exact — and it avoids walking children, which would need `rowan` as a direct
    // dependency this crate deliberately does not have.
    let trimmed = text.trim();
    let inner = trimmed
        .strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
        .unwrap_or(trimmed);
    inner.to_owned()
}

fn insert_directive(stmt: &jr_syntax::ast::ExprStmt) -> Option<jr_syntax::ast::DirectiveExpr> {
    let AstExpr::Directive(directive) = stmt.expr()? else {
        return None;
    };
    let token = directive.directive_token()?;
    (token.text().trim_start_matches('#') == "insert").then_some(directive)
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

/// Whether a statement node carries a destructuring target list (ADR-0052 §2).
///
/// The parser reuses `DECL_STMT` and `ASSIGN_STMT` for both the ordinary and the destructuring
/// forms, so this presence test is what tells them apart — rather than a third node kind, which
/// would have made every consumer of those two kinds learn about a variant that behaves the same
/// everywhere except in lowering.
fn has_target_list(node: &SyntaxNode) -> bool {
    node.children()
        .any(|n| n.kind() == jr_syntax::SyntaxKind::TARGET_LIST)
}
