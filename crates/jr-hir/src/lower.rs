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
        AssignStmt, AstNode, BinaryExpr, Block, ConstDecl, ControlBody, DeclStmt, ElseBranch,
        EnumType, Expr as AstExpr, ForStmt, IfStmt, ImportDecl, Item as AstItem, LiteralExpr,
        Proc as AstProc, RunDecl, SourceFile, Stmt as AstStmt, StructType, TypeExpr, UnaryExpr,
        VarDecl, WhileStmt,
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
/// A **computed** file-scope `#insert` generating anything but a library declaration (ADR-0184 §4).
///
/// The boundary is a phase order, not a policy: a literal insert expands during `file_hir` and can
/// generate anything, while a computed one expands only after const-eval — so a generated procedure has
/// no signature by the time one is wanted and a generated constant has no value. Both leaked internals
/// before this code existed ("called a procedure taking 2 arguments with 1", "a file-level item has no
/// value until jr-vm"), which is the family this project keeps turning into diagnostics.
///
/// Owned by `jr-hir`, continuing its `#insert` block (E0262–E0264), because it is judged in lowering
/// where the generated items are built.
const E0294: &str = "E0294";
/// A `return` that is not the last statement of a `#expand` macro body (ADR-0090 §2).
///
/// Raised in **lowering** rather than in sema, because it is a property of the macro's *text* — the splice
/// is built here, and it is here that "the rewrite would fall through" is knowable. Its number continues
/// `jr-hir`'s block (E0262-E0264 are `#insert`'s) rather than joining `jr-sema`'s.
const E0273: &str = "E0273";
/// `$$T` in **return** position, which cannot mean anything (ADR-0168 §1).
///
/// `$$` is `$` plus "and the argument is a compile-time constant" (ADR-0137 §1) — so it marks a *parameter*,
/// whose argument there is something to bake. A return has no argument, so the second `$` has nothing to say
/// and `-> $$T` is `-> $T` with a typo in front of it.
///
/// Refused in **lowering** for the reason E0276 is: the validity of a type *decoration* at a declaration site
/// is judged where the signature is built. Before this code the declaration lowered and the *call* died with
/// `internal compiler error: no routine for file 0 proc N` — the leaked-internal-error shape `AGENTS.md`
/// records as this project's most frequent, and this is its tenth instance.
const E0290: &str = "E0290";
/// `#bake_arguments` — a partial application, whose specialisation is not yet built (ADR-0096 §3).
///
/// `#bake_arguments add(a = 5)` produces a *specialised procedure* with some arguments built in. The surface
/// — the directive, its call-shaped operand, and the named-argument spelling reused from an ordinary call —
/// is this sub-wave; producing the specialised procedure is the next, and it reuses ADR-0088 §3's clone
/// wholesale (drop the baked parameters, substitute their values, remap the rest).
///
/// Refused here, in **lowering**, rather than left to fall through: before this code the declaration lowered
/// to a poisoned expression and the *caller* reported "the compiler could not lower `main` … this compiler
/// has a gap — please report it". That message is right for an unknown gap and wrong for a feature whose
/// absence is known and named, which is the whole difference this code makes.
const E0276: &str = "E0276";

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
const DIRECTIVES_VALID_AS_EXPRESSIONS: &[&str] = &[
    "system_library",
    "framework",
    "library",
    "compiler_library",
    "bake_arguments",
];

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
    /// A **nested** item declared inside a body — a local constant or a nested procedure
    /// (ADR-0134). The item is hoisted to the file's item arena but **not** added to
    /// `hir.scope`, so it is visible only through this scope frame. That is the "no capture,
    /// scoped-name" shape §7's table decided against a real closure.
    Item(ItemId),
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
    /// Every `#expand` macro's spliceable shape, collected before any body is lowered (ADR-0090 §2).
    ///
    /// Held here so each `BodyLowerCtx` this context creates can be handed it, the same way `operands` is.
    macros: MacroBodies,
    /// Every name an aliased `#import` binds, collected before any item is lowered (ADR-0179 §4).
    ///
    /// A pre-scan for the same reason [`MacroBodies`] needs one: a use of `Simp.foo` may precede the
    /// `Simp :: #import "Simp";` that binds `Simp` in source order, and lowering walks items once.
    import_aliases: ImportAliases,
    /// Predicate → guarded template's `$T` names, staged until the `FileHir` is built (ADR-0094 §1).
    predicate_vars: Vec<(ProcId, Vec<Symbol>)>,
    /// The span every node gets while a file-scope `#insert` is expanding (ADR-0184 §1).
    ///
    /// `BodyLowerCtx` has the same field for the same reason — see its docs, which argue at length why
    /// this is an override at the source rather than a rewrite afterwards.
    span_override: Option<Span>,
    /// How many file-scope `#insert` expansions enclose the item being lowered (ADR-0073 §3).
    ///
    /// Zero in ordinary source. A *literal* insert is bounded by its own text, so this exists for the
    /// **computed** case: a generated string can reproduce itself without growing, which is a quine, and
    /// nothing else would stop it.
    insert_depth: u32,

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
    fn new(
        file: FileId,
        interner: &'a Interner,
        operands: &'a InsertOperands,
        macros: MacroBodies,
        import_aliases: ImportAliases,
    ) -> Self {
        Self {
            file,
            interner,
            operands,
            import_aliases,
            macros,
            predicate_vars: Vec::new(),
            span_override: None,
            insert_depth: 0,
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
        // **Honours `span_override` for the same reason `BodyLowerCtx`'s does** (ADR-0072 §2): while a
        // file-scope `#insert` is expanding, a node's range is an offset into the *inserted string*, which
        // `jr-diag` would clamp onto unrelated bytes of the real file. Overriding at the source is the
        // only version that cannot be incomplete — a fix-up afterwards has to find every `Span` field,
        // and the first attempt at that missed `Expr::Name`'s own.
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
                    // `Window.Event` — a qualified type name (ADR-0179 §5). The module is the
                    // *first* `IDENT` of the same `NAME_TYPE` node; `sym` above already reads the
                    // last, so an unqualified name reaches the `Name` arm unchanged.
                    None => match n.module_token() {
                        Some(tok) => {
                            let module = self.intern(tok.text());
                            self.alloc_top_type_ref(TypeRef::Qualified { module, name: sym })
                        }
                        None => self.alloc_top_type_ref(TypeRef::Name(sym)),
                    },
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
            // `#simd [N]T` — the same shape as the array below, sharing its length helpers rather
            // than copying them (ADR-0148 §1). Placed before the array arm so the two read in the
            // order a reader meets them in the grammar.
            TypeExpr::Vector(v) => {
                let lanes_span = v.lanes().map_or_else(
                    || self.span_of_node(v.syntax()),
                    |e| self.span_of_node(e.syntax()),
                );
                let lanes = lower_array_len(v.lanes(), lanes_span, self.interner, &mut self.diags);
                let elem = if let Some(e) = v.elem() {
                    self.lower_type_expr_top(&e)
                } else {
                    self.alloc_top_type_ref(TypeRef::Error)
                };
                self.alloc_top_type_ref(TypeRef::Vector {
                    elem,
                    lanes,
                    lanes_name: lower_array_len_name(v.lanes(), self.interner),
                    lanes_span,
                })
            }
            TypeExpr::Array(a) => {
                let len_span = a.len().map_or_else(
                    || self.span_of_node(a.syntax()),
                    |e| self.span_of_node(e.syntax()),
                );
                let len = lower_array_len(a.len(), len_span, self.interner, &mut self.diags);
                let elem = if let Some(e) = a.elem() {
                    self.lower_type_expr_top(&e)
                } else {
                    self.alloc_top_type_ref(TypeRef::Error)
                };
                self.alloc_top_type_ref(TypeRef::Array {
                    elem,
                    len,
                    len_name: lower_array_len_name(a.len(), self.interner),
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
            TypeExpr::DynamicArray(d) => {
                let elem = if let Some(e) = d.elem() {
                    self.lower_type_expr_top(&e)
                } else {
                    self.alloc_top_type_ref(TypeRef::Error)
                };
                self.alloc_top_type_ref(TypeRef::DynamicArray { elem })
            }
            TypeExpr::Proc(p) => {
                let params: Vec<TypeRefId> =
                    p.params().map(|t| self.lower_type_expr_top(&t)).collect();
                let ret = p.ret().map(|t| self.lower_type_expr_top(&t));
                let c_call = p.is_c_call();
                self.alloc_top_type_ref(TypeRef::Proc {
                    params,
                    ret,
                    c_call,
                })
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
        // `#soa(N)` (ADR-0147 §1). Only a `struct` may carry it: a `union`'s fields all share one
        // offset and a `variant`'s are cases, so "one array per field" means nothing for either —
        // and the parser only admits the attribute on a `struct`, so there is nothing to refuse.
        let soa = s.soa_count().map(|e| self.lower_top_expr(&e));
        self.lower_fields_into_struct(span, s.field_list(), AggregateKind::Struct, poly_vars, soa)
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
        self.lower_fields_into_struct(span, u.field_list(), AggregateKind::Union, Vec::new(), None)
    }

    /// Lowers `variant { i: s64; f: float64; }` (ADR-0068 §1).
    ///
    /// The same arena and the same field loop as the other two forms; only the kind differs, which is
    /// what makes the tag a *layout* question (ADR-0068 §3) rather than a different HIR shape.
    fn lower_variant_type(&mut self, v: &jr_syntax::ast::VariantType) -> StructId {
        let span = self.span_of_node(v.syntax());
        self.lower_fields_into_struct(
            span,
            v.field_list(),
            AggregateKind::Variant,
            Vec::new(),
            None,
        )
    }

    /// The field loop all three aggregate forms share.
    fn lower_fields_into_struct(
        &mut self,
        span: jr_base::Span,
        field_list: Option<jr_syntax::ast::FieldList>,
        kind: AggregateKind,
        poly_vars: Vec<Symbol>,
        soa: Option<ExprId>,
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
                // The operands are lowered as ordinary expressions, so a name in one is resolved
                // and reported exactly as a name anywhere else is (ADR-0144 §2).
                let align = f.align_value().map(|e| self.lower_top_expr(&e));
                let place = f.place_value().map(|e| self.lower_top_expr(&e));
                fields.push(Field {
                    name,
                    name_span,
                    ty,
                    using: f.is_using(),
                    align,
                    place,
                });
            }
        }

        self.alloc_struct(Struct {
            kind,
            fields,
            poly_vars,
            span,
            type_refs: Vec::new(),
            soa,
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
        self.lower_proc_with_inherited(ast_proc, &[])
    }

    fn lower_proc_with_inherited(
        &mut self,
        ast_proc: &AstProc,
        inherited_items: &[(Symbol, ItemId)],
    ) -> ProcId {
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
                let ty_written = p.ty().map(|t| self.lower_type_expr_top(&t));
                // A variadic `args: ..T` parameter's HIR type is `[]T` — the caller packs the
                // trailing arguments into a stack view, and the callee sees an ordinary view
                // (ADR-0138 §1). Wrapping here means sema and MIR read `[]T` as if the user
                // wrote it; only call-side arg counting and the packing walk know the
                // parameter is variadic.
                let ty = if p.is_variadic() {
                    ty_written.map(|elem| self.alloc_top_type_ref(TypeRef::View { elem }))
                } else {
                    ty_written
                };
                // A default is a *top-level* expression, like a constant's value: it belongs to the
                // signature rather than to any body, so it goes in `FileHir::exprs`.
                let default = p.default_value().map(|e| self.lower_top_expr(&e));
                // Comptime is either an explicit `$N: T` mark on the parameter name (ADR-0087)
                // or a `$$T` mark on the parameter's *type* (ADR-0137). Both make the argument
                // a compile-time constant, so both feed one `Param::comptime` flag.
                let type_is_comptime_poly = p
                    .ty()
                    .and_then(|t| match t {
                        jr_syntax::ast::TypeExpr::Poly(pt) => Some(pt.is_comptime()),
                        _ => None,
                    })
                    .unwrap_or(false);
                params.push(Param {
                    name,
                    name_span,
                    ty,
                    using: p.is_using(),
                    comptime: p.is_comptime() || type_is_comptime_poly,
                    variadic: p.is_variadic(),
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

        // `$$T` in return position is refused rather than lowered (ADR-0168 §1). Checked here, after the
        // return type is built, so a `(s64, $$T)` result list is caught too — the walk is over every
        // `TypeExpr` the return position holds, not only a bare one.
        //
        // A `$$T` *parameter* is legal and common; this is only about the return.
        if let Some(rt) = ast_proc.ret_type() {
            let node = rt.syntax();
            let written: Vec<jr_syntax::ast::TypeExpr> = if let Some(list) = node
                .children()
                .find(|n| n.kind() == jr_syntax::SyntaxKind::RESULT_LIST)
            {
                list.children()
                    .filter_map(jr_syntax::ast::TypeExpr::cast)
                    .collect()
            } else {
                rt.ty().into_iter().collect()
            };
            for t in written {
                let jr_syntax::ast::TypeExpr::Poly(pt) = &t else {
                    continue;
                };
                if !pt.is_comptime() {
                    continue;
                }
                self.diags.push(
                    Diagnostic::error(
                        self.span_of_node(t.syntax()),
                        "`$$` cannot appear in a return type: the second `$` marks an argument as a \
                         compile-time constant, and a return has no argument",
                    )
                    .with_code(E0290)
                    .with_help("write `$T` — a return type is inferred from the call either way"),
                );
            }
        }

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

        // Body. **A `#expand` macro's body is not lowered here** (ADR-0090 §2): its statements exist only
        // to be *spliced* into a caller, where they are lowered in that caller's scope. Lowering it
        // standalone would resolve its names against the macro's own (empty) scope, so a macro that reads
        // the caller's locals — the whole point of `#expand` — would report them as unresolved. It also
        // emits no MIR, exactly as a `$T` template does not.
        // **A `#modify` block is lowered here, as its own synthetic procedure** (ADR-0094 §1): no
        // parameters, returning `bool`, body = the block. Lowering it at the *template* means it goes
        // through the same `lower_body` every procedure does — so it needs no text round-trip and no new
        // lowering entry point, which is what ADR-0093 §2 thought it would. Each instantiation appends a
        // *clone* of it with that instantiation's bindings, exactly as the instantiation is cloned.
        let modify_pred = ast_proc.modify_block().map(|block| {
            let bool_sym = self.intern("bool");
            let ret_id = self.alloc_top_type_ref(TypeRef::Name(bool_sym));
            let body = self.lower_body(&block, &[]);
            let body_id = self.alloc_body(body);
            let pred_span = self.span_of_node(block.syntax());
            let pred = self.alloc_proc(Proc {
                params: Vec::new(),
                c_call: false,
                no_abc: false,
                must: false,
                // A `#modify` predicate is synthetic and never `#foreign`, so it is never variadic.
                c_variadic: false,
                program_export: false,
                expand: false,
                modify: None,
                notes: Vec::new(),
                ret: Some(ret_id),
                body: Some(body_id),
                foreign: None,
                span: pred_span,
                type_refs: Vec::new(),
            });
            // A **synthetic, unexported** name, and deliberately **not** in `scope`: the signature phase
            // computes a signature only for a *named* item, but nothing should be able to call the
            // predicate by name — it is reached only through `Proc::modify`. The same arrangement an
            // instantiation gets (ADR-0082 §2).
            let synthetic = self.intern(&format!("$modify{}", pred.index()));
            self.items.push(Item {
                name: Some(synthetic),
                exported: false,
                nested: false,
                span: pred_span,
                name_span: pred_span,
                kind: ItemKind::Const {
                    value: ConstValue::Proc(pred),
                },
            });
            pred
        });
        // The guarded template's `$T` names, recorded against its predicate (ADR-0094 §1) so sema can
        // withhold `type_info(T)` inside it the way it does inside the template's own body — a predicate
        // has no `$T` of its own, but its body names the template's.
        if let Some(pred) = modify_pred {
            let vars = poly_var_names_of(&params, &self.top_type_refs);
            if !vars.is_empty() {
                self.predicate_vars.push((pred, vars));
            }
        }

        let is_macro = ast_proc.is_expand();
        let body = ast_proc.body().filter(|_| !is_macro).map(|b| {
            let body = self.lower_body_inner(&b, &params, inherited_items);
            self.alloc_body(body)
        });

        // Validate: must have body XOR foreign (or neither, which is an error). A macro is exempt: it has
        // a body in source, deliberately not lowered above.
        if body.is_none() && foreign.is_none() && !is_macro {
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
            must: ast_proc.is_must(),
            c_variadic: ast_proc.is_c_variadic(),
            program_export: ast_proc.is_program_export(),
            expand: ast_proc.is_expand(),
            modify: modify_pred,
            notes: ast_proc
                .notes()
                .filter_map(|note| {
                    let name = self.intern(note.name_token()?.text());
                    // The payload's quotes are stripped here, where the interner is, so every consumer sees
                    // the text rather than the literal.
                    let payload = note
                        .payload_token()
                        .map(|t| t.text().trim_matches('"').to_owned());
                    Some((name, payload))
                })
                .collect(),
            ret,
            body,
            foreign,
            span,
            type_refs: Vec::new(),
        })
    }

    // ---- bodies ------------------------------------------------------------

    fn lower_body(&mut self, block: &Block, params: &[Param]) -> Body {
        self.lower_body_inner(block, params, &[])
    }

    fn lower_body_inner(
        &mut self,
        block: &Block,
        params: &[Param],
        inherited_items: &[(Symbol, ItemId)],
    ) -> Body {
        // Nested-item hoisting (ADR-0134): every nested `X :: <value>;` declaration the body
        // encounters reserves a slot in the file's item arena counted from *this* point, and
        // then `LowerCtx::lower_body_inner` allocates those items in the same order after the body
        // finishes. The invariant that no other item is allocated between now and the drain
        // makes the predicted `ItemId` and the allocated one match by construction, which is
        // what lets the scope stack refer to items that do not yet exist.
        let first_pending_item = self.items.len();
        let mut bctx = BodyLowerCtx::new(
            self.file,
            self.interner,
            self.operands,
            &self.macros,
            &self.import_aliases,
            first_pending_item,
        );

        // Register inherited nested items (ADR-0134 §2). These are siblings of the current
        // procedure — nested items declared in the enclosing block — plus the current
        // procedure itself. They are pushed *before* the parameters so a parameter that
        // happens to share a name with a sibling shadows it (ordinary shadowing rules), and so
        // recursion through the current procedure's own name works: for a nested
        // `factorial :: (n: s64) -> s64 { … factorial(n - 1) … }` the enclosing block's drain
        // passes `factorial` as one of the inherited entries, so its own body can reach it.
        for (name, item_id) in inherited_items {
            bctx.scope_stack
                .last_mut()
                .unwrap()
                .push((*name, ScopeEntry::Item(*item_id)));
        }

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

        // Take the pending hoists out so the borrow ends before we start lowering them —
        // `lower_hoisted_const` reaches back into `self` and would clash with `bctx` still
        // holding fields we read here.
        let pending = std::mem::take(&mut bctx.pending_hoists);

        let body = Body {
            exprs: bctx.exprs,
            expr_spans: bctx.expr_spans,
            stmts: bctx.stmts,
            locals: bctx.locals,
            type_refs: bctx.type_refs,
            root,
        };

        // The drain. `self.items.len()` must still equal `first_pending_item` here — nothing
        // between construction of `bctx` and this point allocates a file-level item, because
        // `BodyLowerCtx` methods only touch body arenas. The assert is the invariant made
        // executable.
        assert_eq!(
            self.items.len(),
            first_pending_item,
            "an item was allocated during body lowering; the ADR-0134 hoist prediction is broken",
        );
        // Sibling inheritance: every nested proc drained here sees all *its own siblings*
        // (including itself, for recursion). Collected once from the pending list before we
        // start draining, since the drain adds items to `self.items` and this is the last
        // moment when each pending hoist's name is easy to reach.
        let siblings: Vec<(Symbol, ItemId)> = pending
            .iter()
            .filter_map(|h| {
                h.ast
                    .name()
                    .and_then(|n| n.text())
                    .map(|t| (t, h.predicted_id))
            })
            .map(|(t, id)| (self.intern(&t), id))
            .collect();
        for hoist in pending {
            let allocated = self.lower_hoisted_const(&hoist.ast, &siblings);
            assert_eq!(
                allocated, hoist.predicted_id,
                "ADR-0134: predicted item id did not match allocation order",
            );
        }

        body
    }

    /// Lowers a nested `X :: <value>;` declaration into the file's item arena (ADR-0134).
    ///
    /// Called by `lower_body_inner` on each pending hoist collected during body lowering. The item
    /// is allocated exactly as an ordinary file-scope constant is, **except that its name is
    /// not inserted into `hir.scope`** — visibility is via the enclosing body's scope stack
    /// only, plus the sibling-scope injection every nested proc's body receives (ADR-0134 §2).
    ///
    /// `siblings` are the (name, ItemId) pairs of *every* sibling nested item in the same
    /// enclosing block (including this one). They are injected into the nested body's outer
    /// scope so `factorial` can recurse and so `twice` can call its sibling `add`.
    fn lower_hoisted_const(&mut self, cd: &ConstDecl, siblings: &[(Symbol, ItemId)]) -> ItemId {
        self.lower_const_decl_with_inherited(cd, /* insert_in_scope */ false, siblings)
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

    /// The module alias a field access's receiver names, at file scope (ADR-0179 §4).
    ///
    /// No local can shadow an alias here — a file-scope initialiser has no locals — so this is
    /// [`field_receiver_alias`] with nothing added.
    fn qualified_receiver(&self, f: &jr_syntax::ast::FieldExpr) -> Option<Symbol> {
        field_receiver_alias(f, self.interner, &self.import_aliases)
    }

    fn lower_top_expr(&mut self, expr: &AstExpr) -> ExprId {
        let span = self.span_of_node(expr.syntax());
        match expr {
            // **A fixed array literal** (ADR-0194 §1). The element type is lowered as an *expression*,
            // which is what lets sema resolve it through `described_type` — the same route every
            // intrinsic's type argument takes.
            AstExpr::ArrayLiteral(al) => {
                let elem_ty = al
                    .element_type()
                    .map(|t| self.lower_top_expr(&t))
                    .unwrap_or_else(|| self.alloc_top_expr(Expr::Error(span), span));
                let elems: Vec<ExprId> = al.elements().map(|e| self.lower_top_expr(&e)).collect();
                self.alloc_top_expr(
                    Expr::ArrayLit {
                        elem_ty,
                        elems,
                        span,
                    },
                    span,
                )
            }
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
                        module: None,
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
                let (name, name_span) = f
                    .field_name()
                    .map(|t| (self.intern(t.text()), self.span_of_token(&t)))
                    .unwrap_or_else(|| (self.intern("<error>"), span));
                // **`Simp.foo` is a qualified name, not a field access** (ADR-0179 §4). At file scope
                // there are no locals, so an alias always wins here — the body path additionally
                // checks that no local shadows it.
                if let Some(alias) = self.qualified_receiver(f) {
                    return self.alloc_top_expr(
                        Expr::Name {
                            name,
                            module: Some(alias),
                            span,
                            res: Res::Error,
                        },
                        span,
                    );
                }
                let receiver = f
                    .object()
                    .map(|e| self.lower_top_expr(&e))
                    .unwrap_or_else(|| {
                        let err = Expr::Error(span);
                        self.alloc_top_expr(err, span)
                    });
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
            nested: false,
            name: Some(name),
            span,
            name_span,
            kind: ItemKind::Const {
                value: ConstValue::Operator(proc_id, op),
            },
        });
    }

    /// A poisoned top-level expression, for a constant whose value was refused (ADR-0097 §1).
    fn alloc_top_expr_error(&mut self, span: Span) -> ExprId {
        self.alloc_top_expr(Expr::Error(span), span)
    }

    /// Specialises a procedure by baking some of its arguments (ADR-0097 §1).
    ///
    /// `#bake_arguments add(a = 5)` produces a **clone of `add`** with the parameter `a` dropped from its
    /// list and `5` substituted for every use of `a` in its body — the same drop/substitute/remap
    /// `$N` instantiation does (ADR-0088 §3), so the result is an *ordinary* procedure that nothing
    /// downstream has to be taught about.
    ///
    /// `None` when the operand is not a call to a procedure declared in this file, or when a baked argument
    /// is not a literal — each reported here, because a specialisation that silently ignored an argument
    /// would produce a procedure that is not the one written.
    fn lower_bake_arguments(
        &mut self,
        call: &jr_syntax::ast::CallExpr,
        span: Span,
    ) -> Option<ProcId> {
        let Some(AstExpr::Name(callee)) = call.callee() else {
            self.diags.push(bake_needs_a_procedure(span));
            return None;
        };
        let name = self.intern(&callee.text()?);
        // The procedure to specialise must be declared in this file: the clone copies its *body*, and
        // another file's body is not in this HIR. A cross-file bake is deferred with the cross-file splice
        // (ADR-0091 §3's boundary), and named rather than silently mis-specialised.
        let target =
            self.scope
                .get(name)
                .and_then(|item| match &self.items.get(item.index())?.kind {
                    ItemKind::Const {
                        value: ConstValue::Proc(proc),
                    } => Some(*proc),
                    _ => None,
                });
        let Some(target) = target else {
            self.diags.push(bake_needs_a_procedure(span));
            return None;
        };

        // Which parameters are baked, and to what. A **named** argument names its parameter (ADR-0053 §1's
        // spelling, reused); a positional one bakes the parameter at its own index.
        let params = self.procs[target.index()].params.clone();
        let mut baked: Vec<Option<Literal>> = vec![None; params.len()];
        // The arg list's **children**, not `ArgList::args()`: a named argument is a `NAMED_ARG` node and
        // not an `Expr`, so that accessor would skip every one — the trap ADR-0053 §1 records.
        let args: Vec<SyntaxNode> = call
            .arg_list()
            .into_iter()
            .flat_map(|list| list.syntax().children().collect::<Vec<_>>())
            .filter(|n| {
                n.kind() == jr_syntax::SyntaxKind::NAMED_ARG
                    || jr_syntax::ast::Expr::cast(n.clone()).is_some()
            })
            .collect();
        for (index, arg) in args.iter().enumerate() {
            // A baked value must be a **literal** this pass can read: lowering has no evaluator (ADR-0018
            // §3), and ADR-0096 §2's const-eval route needs the value at *this* point, before any query
            // exists. So the narrower rule is taken and named, exactly as ADR-0039 §3a took it for an array
            // length before ADR-0070 widened it.
            let Some(lit) = literal_of(arg) else {
                self.diags.push(bake_needs_a_literal(span));
                return None;
            };
            let slot = match named_arg_name(arg) {
                Some(argname) => {
                    let sym = self.intern(&argname);
                    match params.iter().position(|p| p.name == sym) {
                        Some(i) => i,
                        None => {
                            self.diags.push(bake_needs_a_procedure(span));
                            return None;
                        }
                    }
                }
                None => index,
            };
            if slot >= baked.len() {
                self.diags.push(bake_needs_a_procedure(span));
                return None;
            }
            baked[slot] = Some(lit);
        }

        Some(self.clone_with_baked(target, &baked))
    }

    /// Clones `target` with the marked parameters dropped and their values substituted (ADR-0097 §1).
    ///
    /// The three steps are ADR-0088 §3's, applied here rather than in `instantiate.rs` because this runs
    /// during *lowering* (a baked procedure is a declaration, not an instantiation): drop the baked
    /// parameters, rewrite each `Res::Param` use of one into its literal, and remap the remaining indices so
    /// a kept parameter still resolves. Only the *body* is copied; the shared type refs are reused, since
    /// nothing rewrites them.
    fn clone_with_baked(&mut self, target: ProcId, baked: &[Option<Literal>]) -> ProcId {
        let template = self.procs[target.index()].clone();
        let mut keep: Vec<Option<u32>> = Vec::with_capacity(template.params.len());
        let mut next: u32 = 0;
        for (i, _) in template.params.iter().enumerate() {
            if baked.get(i).and_then(Option::as_ref).is_some() {
                keep.push(None);
            } else {
                keep.push(Some(next));
                next += 1;
            }
        }
        let params: Vec<Param> = template
            .params
            .iter()
            .enumerate()
            .filter(|(i, _)| keep[*i].is_some())
            .map(|(_, p)| p.clone())
            .collect();

        let body = template.body.map(|b| {
            let mut cloned = self.bodies[b.index()].clone();
            for index in 0..cloned.exprs.len() {
                if let Expr::Name {
                    name,
                    module,
                    span,
                    res,
                } = cloned.exprs[index].clone()
                    && let Res::Param(pid) = res
                {
                    let i = pid.index();
                    match keep.get(i).copied().flatten() {
                        // Dropped: substitute the baked literal.
                        None => {
                            if let Some(Some(lit)) = baked.get(i) {
                                cloned.exprs[index] = Expr::Literal(lit.clone(), span);
                            }
                        }
                        // Kept: remap its index, since earlier parameters may have been dropped.
                        Some(new_i) => {
                            cloned.exprs[index] = Expr::Name {
                                name,
                                module,
                                span,
                                res: Res::Param(ParamId::from_usize(new_i as usize)),
                            };
                        }
                    }
                }
            }
            let id = BodyId::from_usize(self.bodies.len());
            self.bodies.push(cloned);
            id
        });

        self.alloc_proc(Proc {
            params,
            c_call: template.c_call,
            no_abc: template.no_abc,
            // A baked clone inherits `#must` for the same reason it inherits the notes below: it *is*
            // that procedure, specialised, so its caller's obligation cannot differ.
            must: template.must,
            c_variadic: template.c_variadic,
            program_export: false,
            expand: false,
            modify: None,
            // A baked clone keeps its original's notes: it *is* that procedure, specialised.
            notes: template.notes.clone(),
            ret: template.ret,
            body,
            foreign: template.foreign.clone(),
            span: template.span,
            type_refs: Vec::new(),
        })
    }

    fn lower_const_decl(&mut self, cd: &ConstDecl) {
        // The file-scope entry point — see [`Self::lower_const_decl_with_inherited`] for the
        // parameters that make hoisted-nested constants (ADR-0134) share the same code path
        // without leaking their name into `hir.scope` and while carrying sibling scope.
        self.lower_const_decl_with_inherited(cd, /* insert_in_scope */ true, &[]);
    }

    fn lower_const_decl_with_inherited(
        &mut self,
        cd: &ConstDecl,
        insert_in_scope: bool,
        inherited_items: &[(Symbol, ItemId)],
    ) -> ItemId {
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

        // **Reserve the item slot up front** (ADR-0134). Nested items count `items.len()` at
        // the start of every body they enter to predict their own ItemIds. If the enclosing
        // item's slot is not allocated until *after* its value is lowered, a nested-in-nested
        // proc's body would see a count that does not include the outer nested item — so the
        // inner predictions would collide with the outer's actual position. Reserving first
        // gives every nested predictor the position count it is entitled to. The placeholder
        // kind is `ItemKind::Var { ty: None, init: None, uninit: false }` because it needs no
        // ExprId — a `ConstValue::Expr(err)` would spuriously allocate a top-level expression
        // and shift every top expr's index one, which is what
        // `resolve_map_does_not_collide_top_level_and_body_expression_ids` probes.
        let reserved_id = if insert_in_scope {
            self.alloc_item(Item {
                exported: self.exporting,
                nested: false,
                name,
                span,
                name_span,
                kind: ItemKind::Var {
                    ty: None,
                    init: None,
                    uninit: false,
                },
            })
        } else {
            let id = ItemId::from_usize(self.items.len());
            self.items.push(Item {
                exported: false,
                nested: true,
                name,
                span,
                name_span,
                kind: ItemKind::Var {
                    ty: None,
                    init: None,
                    uninit: false,
                },
            });
            id
        };

        // **`Simp :: #import "Simp";` is an import, not a constant** (ADR-0179 §1).
        //
        // The parser sees a constant declaration whose value is a directive expression, which
        // already parses — so recognition happens here, by the directive's *name*, exactly the way
        // `#bake_arguments` is recognised rather than given a grammar rule of its own.
        //
        // Gated on `insert_in_scope` because that is false for a hoisted nested constant
        // (ADR-0134): an `#import` inside a body stays E0208, and `check_directive_as_expression`
        // still reports it because this branch never runs there.
        if insert_in_scope
            && let Some(alias) = name
            && let Some(directive) = import_directive_value(cd)
        {
            let (path, path_span) = match directive.string_arg() {
                Some(tok) => (strip_quotes(tok.text()), self.span_of_token(&tok)),
                None => (String::new(), span),
            };
            // The alias is deliberately **not** left in `hir.scope`. A bare `Simp` is not a value,
            // and leaving it bound would resolve it to `Res::Item` of an import — which every
            // consumer downstream would then have to learn to refuse. An unresolved name is the
            // honest answer, and `Simp.thing` never asks: it lowers to a qualified `Expr::Name`.
            //
            // `Item::name` stays `Some`, which is what makes `check_duplicates` report an alias that
            // collides with a declaration — so the ambiguity needs no code of its own.
            self.scope.names.remove(&alias);
            self.items[reserved_id.index()].kind = ItemKind::Import {
                path,
                path_span,
                alias: Some(alias),
            };
            return reserved_id;
        }

        let kind = if let Some(proc) = cd.proc() {
            let proc_id = self.lower_proc_with_inherited(&proc, inherited_items);
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
        } else if let Some(baked) = bake_arguments_operand(cd) {
            // **`add_five :: #bake_arguments add(a = 5)` becomes a real procedure** (ADR-0097 §1): a clone of
            // the named one with the baked parameters dropped from its list and their values substituted into
            // its body. That is ADR-0088 §3's mechanism — the same drop/substitute/remap `$N` instantiation
            // does — so the specialised procedure is an *ordinary* one from here on, callable and lowerable
            // with nothing else taught about it.
            match self.lower_bake_arguments(&baked, span) {
                Some(proc_id) => ItemKind::Const {
                    value: ConstValue::Proc(proc_id),
                },
                // The refusal is already reported; a poisoned expression keeps the item shaped like a
                // constant rather than inventing a procedure that does not exist.
                None => {
                    let err = self.alloc_top_expr_error(span);
                    ItemKind::Const {
                        value: ConstValue::Expr {
                            expr: err,
                            ty: None,
                        },
                    }
                }
            }
        } else if let Some(expr) = cd.value_expr() {
            let expr_id = self.lower_top_expr(&expr);
            // **The annotation of a typed constant** (ADR-0190 §2). `cd.ty()` is `Some` only for
            // `name : T : value`, which the parser wraps as a `CONST_DECL` carrying a type child.
            let ty = cd.ty().map(|t| self.lower_type_expr_top(&t));
            ItemKind::Const {
                value: ConstValue::Expr { expr: expr_id, ty },
            }
        } else {
            let err_id = self.alloc_top_expr(Expr::Error(span), span);
            ItemKind::Const {
                value: ConstValue::Expr {
                    expr: err_id,
                    ty: None,
                },
            }
        };

        // Patch the reserved slot with the real `kind`. Every other field is already correct
        // from the reservation above.
        self.items[reserved_id.index()].kind = kind;
        reserved_id
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
            nested: false,
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
            nested: false,
            name: None,
            span,
            name_span: span,
            kind: ItemKind::Import {
                path,
                path_span,
                alias: None,
            },
        });
    }

    /// Lowers a file-scope `#insert`, whose text becomes **declarations** (ADR-0184 §1).
    ///
    /// The body-level twin is [`BodyLowerCtx::lower_insert`], and the two differ in exactly one way that
    /// matters: this one parses its text as a **source file** and allocates *items*, where that one parses
    /// statements. Everything else — the depth guard, the span override, the literal-versus-computed
    /// split, the pending state — is the same shape deliberately, so a reader who knows one knows both.
    fn lower_insert_decl(&mut self, idl: &jr_syntax::ast::InsertDecl) {
        let span = self.span_of_node(idl.syntax());

        // **Checked before the operand is looked at** (ADR-0073 §3), because what is bounded is the
        // recursion: an insert whose text contains an insert re-enters this function. A *literal* insert
        // is bounded by its own text, so reaching the limit almost always means a computed operand
        // reproduces itself — which a generated string can do without growing.
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
            return;
        }

        let Some(operand) = idl.operand() else {
            self.diags.push(
                Diagnostic::error(
                    span,
                    "`#insert` needs a string literal of Jairs declarations",
                )
                .with_code(E0262)
                .with_help("write the declarations inline, e.g. `#insert \"X :: 7;\";`"),
            );
            return;
        };

        // A **literal** operand expands now. Decoded rather than merely unquoted, through the same
        // function every string literal goes through, so `#insert "s :: \"hi\";"` inserts what it looks
        // like.
        if let AstExpr::Literal(literal) = &operand
            && let Some(token) = literal.syntax().first_token()
            && token.kind() == jr_syntax::SyntaxKind::STRING_LITERAL
        {
            let text = decode_string_impl(token.text(), span, &mut self.diags);
            self.expand_insert_items(&text, span, true);
            self.alloc_item(Item {
                exported: self.exporting,
                nested: false,
                name: None,
                span,
                name_span: span,
                kind: ItemKind::Insert {
                    operand: None,
                    span,
                },
            });
            return;
        }

        // A **computed** operand. If the pre-pass has already evaluated it — keyed by this directive's
        // span — expand it exactly as a literal, and record `operand: None`, which is what says
        // "evaluated, and this is the result" as opposed to "still waiting". An operand that evaluated to
        // the empty string therefore expands to zero declarations *legally*, where a pending one is
        // refused; ADR-0073 §1 draws the same line for statements, and it is the reason the two states
        // must not share a representation.
        if let Some(text) = self.operands.get(span) {
            let text = text.to_owned();
            self.expand_insert_items(&text, span, false);
            self.alloc_item(Item {
                exported: self.exporting,
                nested: false,
                name: None,
                span,
                name_span: span,
                kind: ItemKind::Insert {
                    operand: None,
                    span,
                },
            });
            return;
        }

        // Not yet evaluated: **pending**. The operand is lowered as an ordinary top-level expression so
        // it resolves and type-checks at its own span — `#insert nosuchname;` is an unresolved name and a
        // non-`string` operand is a type error, each reported where the reader wrote it rather than as a
        // blanket refusal here.
        let operand_id = self.lower_top_expr(&operand);
        self.alloc_item(Item {
            exported: self.exporting,
            nested: false,
            name: None,
            span,
            name_span: span,
            kind: ItemKind::Insert {
                operand: Some(operand_id),
                span,
            },
        });
    }

    /// Parses inserted `text` as a **source file** and lowers its items into this file's arena.
    ///
    /// The item counterpart of [`BodyLowerCtx::expand_insert_text`]. Every node produced takes the
    /// directive's span through `span_override`, for ADR-0072 §2's reason: the inner parse's spans are
    /// offsets into `text`, which `jr-diag` would clamp onto unrelated bytes of the real file.
    ///
    /// **Visibility is inherited, not reset.** An insert after `#scope_module` generates private
    /// declarations, because `self.exporting` is a *position in the file* (ADR-0054 §1) and a splice does
    /// not move that position. Saved and restored anyway, so a `#scope_module` *inside* the generated text
    /// affects only the generated text — otherwise one generated marker would silently privatise
    /// everything the rest of the real file declares.
    fn expand_insert_items(&mut self, text: &str, span: Span, from_literal: bool) {
        let parsed = jr_syntax::parse(text, self.file);
        for diag in parsed.diagnostics().iter() {
            // Re-pointed at the directive and re-worded, because the inner diagnostic's span is an offset
            // into `text` and would land on unrelated bytes of the real file (ADR-0072 §3). The offset is
            // carried in a note, which is the part a reader needs to find their mistake.
            let offset = u32::from(diag.primary.span.range.start());
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("inserted declarations do not parse: {}", diag.message),
                )
                .with_code(E0263)
                .with_note(format!("in inserted code, at offset {offset}"))
                .with_note(format!("the inserted text was: {text}")),
            );
        }

        let outer_override = self.span_override;
        self.span_override = Some(span);
        let outer_depth = self.insert_depth;
        self.insert_depth = outer_depth + 1;
        let outer_exporting = self.exporting;

        if let Some(source_file) = SourceFile::cast(parsed.syntax()) {
            // The same walk `lower_file_with_inserts` does, and for the same reason it walks *children*
            // rather than `items()`: a `SCOPE_DECL` is not an `Item` kind, so that accessor would skip a
            // visibility marker in the generated text (ADR-0054 §1).
            for child in source_file.syntax().children() {
                if let Some(scope) = jr_syntax::ast::ScopeDecl::cast(child.clone()) {
                    self.exporting = scope
                        .directive()
                        .map(|token| token.text() != "#scope_module")
                        .unwrap_or(true);
                    continue;
                }
                let Some(item) = AstItem::cast(child) else {
                    continue;
                };
                // **A computed operand may generate only a library declaration** (ADR-0184 §4), and the
                // boundary is a *phase order* rather than a policy. A **literal** insert expands during
                // `file_hir`, before signatures, before const-eval, before anything — so what it
                // generates is indistinguishable from what the file wrote, and every declaration works
                // (verified: a struct and a procedure from a literal insert run to 42). A **computed**
                // one cannot expand until its operand has been evaluated, which is *after* const-eval —
                // so a generated procedure has no signature by the time one is needed and a generated
                // constant has no value, and both surfaced as leaked internals: "called a procedure
                // taking 2 arguments with 1" and "a file-level item has no value until jr-vm".
                //
                // A `#system_library`/`#framework` declaration needs neither: `wanted()` excludes a
                // directive from const-eval by design, and its only consumer reads it from the pool. That
                // is why the per-OS library case — the one this whole wave exists for — works, and it is
                // the honest supported surface until the ordering is fixed.
                if !from_literal && !is_library_declaration(&item) {
                    let item_span = self.span_of_node(item.syntax());
                    self.diags.push(
                        Diagnostic::error(
                            item_span,
                            "a computed `#insert` at file scope may generate only a library declaration",
                        )
                        .with_code(E0294)
                        .with_note(
                            "a computed operand is evaluated after const-eval, so a generated procedure \
                             has no signature yet and a generated constant has no value",
                        )
                        .with_help(
                            "use a *literal* `#insert \"…\";` for other declarations, or select a per-OS \
                             *value* with `X :: #run pick();`",
                        ),
                    );
                    continue;
                }
                match item {
                    AstItem::Const(cd) => self.lower_const_decl(&cd),
                    AstItem::Operator(od) => self.lower_operator_decl(&od),
                    AstItem::Var(vd) => self.lower_var_decl_item(&vd),
                    AstItem::Import(id) => self.lower_import_decl(&id),
                    AstItem::Run(rd) => self.lower_run_decl(&rd),
                    // A nested insert, which is why the depth guard above exists.
                    AstItem::Insert(inner) => self.lower_insert_decl(&inner),
                }
            }
        }

        self.exporting = outer_exporting;
        self.insert_depth = outer_depth;
        self.span_override = outer_override;
    }

    fn lower_run_decl(&mut self, rd: &RunDecl) {
        let span = self.span_of_node(rd.syntax());
        let expr = rd
            .expr()
            .map(|e| self.lower_top_expr(&e))
            .unwrap_or_else(|| self.alloc_top_expr(Expr::Error(span), span));

        self.alloc_item(Item {
            exported: self.exporting,
            nested: false,
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
            instantiation_sites: Vec::new(),
            param_values: Vec::new(),
            modify_predicates: Vec::new(),
            predicate_vars: self.predicate_vars,
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
    /// Every `#expand` macro's spliceable shape, for a call to splice (ADR-0090 §2, sub-wave 7b).
    ///
    /// Threaded exactly as `operands` is, and empty for a file declaring no macro — so an ordinary
    /// program's lowering is unchanged.
    macros: &'a MacroBodies,
    /// Every name an aliased `#import` binds (ADR-0179 §4), threaded exactly as `macros` is.
    ///
    /// Consulted when a field access's receiver is a bare name: an alias no local shadows makes the
    /// whole access a *qualified name* rather than a field of a value.
    import_aliases: &'a ImportAliases,
    /// Names the next macro result local, so two macro calls in one body do not collide (ADR-0090 §2).
    macro_result_counter: u32,

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

    /// The `ItemId` `pending_hoists` will start numbering from (ADR-0134). Set from
    /// `LowerCtx::items.len()` when `BodyLowerCtx` is constructed. Guarded by the invariant
    /// that no other item is allocated between then and `LowerCtx::lower_body` draining these
    /// pending hoists — so a predicted `ItemId` is guaranteed to match the one allocated on
    /// drain.
    first_pending_item: usize,

    /// Nested `X :: <value>;` declarations discovered inside this body, in encounter order.
    /// Each was assigned an `ItemId` when it was seen — `ItemId::from_usize(first_pending_item
    /// + i)` — and registered as `ScopeEntry::Item(id)` in the enclosing scope. After the
    /// body finishes, `LowerCtx::lower_body` drains this list and lowers each one **in the
    /// same order**, producing the same `ItemId`s the body already resolved against.
    pending_hoists: Vec<PendingHoist>,
}

/// A nested `X :: <value>;` declaration discovered inside a body, waiting to be lowered into
/// the file's item arena after the body finishes (ADR-0134).
struct PendingHoist {
    /// The `AstItem::Const` node — carries all of name, value, spans.
    ast: jr_syntax::ast::ConstDecl,
    /// The `ItemId` this hoist was already promised in the body's scope stack. When
    /// `LowerCtx::lower_body` drains, `self.items.len()` must equal this at the point of
    /// drain — an assertion pins the invariant.
    predicted_id: ItemId,
}

impl<'a> BodyLowerCtx<'a> {
    fn new(
        file: FileId,
        interner: &'a Interner,
        operands: &'a InsertOperands,
        macros: &'a MacroBodies,
        import_aliases: &'a ImportAliases,
        first_pending_item: usize,
    ) -> Self {
        Self {
            file,
            interner,
            operands,
            macros,
            import_aliases,
            macro_result_counter: 0,
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
            first_pending_item,
            pending_hoists: Vec::new(),
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
                    // `Window.Event` — a qualified type name (ADR-0179 §5). The module is the
                    // *first* `IDENT` of the same `NAME_TYPE` node; `sym` above already reads the
                    // last, so an unqualified name reaches the `Name` arm unchanged.
                    None => match n.module_token() {
                        Some(tok) => {
                            let module = self.intern(tok.text());
                            self.alloc_type_ref(TypeRef::Qualified { module, name: sym })
                        }
                        None => self.alloc_type_ref(TypeRef::Name(sym)),
                    },
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
            // `#simd [N]T` — the same shape as the array below, sharing its length helpers rather
            // than copying them (ADR-0148 §1). Placed before the array arm so the two read in the
            // order a reader meets them in the grammar.
            TypeExpr::Vector(v) => {
                let lanes_span = v.lanes().map_or_else(
                    || self.span_of_node(v.syntax()),
                    |e| self.span_of_node(e.syntax()),
                );
                let lanes = lower_array_len(v.lanes(), lanes_span, self.interner, &mut self.diags);
                let elem = if let Some(e) = v.elem() {
                    self.lower_type_expr(&e)
                } else {
                    self.alloc_type_ref(TypeRef::Error)
                };
                self.alloc_type_ref(TypeRef::Vector {
                    elem,
                    lanes,
                    lanes_name: lower_array_len_name(v.lanes(), self.interner),
                    lanes_span,
                })
            }
            TypeExpr::Array(a) => {
                let len_span = a.len().map_or_else(
                    || self.span_of_node(a.syntax()),
                    |e| self.span_of_node(e.syntax()),
                );
                let len = lower_array_len(a.len(), len_span, self.interner, &mut self.diags);
                let elem = if let Some(e) = a.elem() {
                    self.lower_type_expr(&e)
                } else {
                    self.alloc_type_ref(TypeRef::Error)
                };
                self.alloc_type_ref(TypeRef::Array {
                    elem,
                    len,
                    len_name: lower_array_len_name(a.len(), self.interner),
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
            TypeExpr::DynamicArray(d) => {
                let elem = if let Some(e) = d.elem() {
                    self.lower_type_expr(&e)
                } else {
                    self.alloc_type_ref(TypeRef::Error)
                };
                self.alloc_type_ref(TypeRef::DynamicArray { elem })
            }
            TypeExpr::Proc(p) => {
                let params: Vec<TypeRefId> = p.params().map(|t| self.lower_type_expr(&t)).collect();
                let ret = p.ret().map(|t| self.lower_type_expr(&t));
                let c_call = p.is_c_call();
                self.alloc_type_ref(TypeRef::Proc {
                    params,
                    ret,
                    c_call,
                })
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
    /// Splices a macro call that appears in **expression** position, if this statement has one
    /// (ADR-0090 §2, sub-wave 7b).
    ///
    /// Returns `None` when the statement has no macro call in expression position — including the
    /// *statement*-position case (`f(x);` alone), which needs no result local and has its own arm.
    ///
    /// The generated text declares a result local, splices the macro (prelude + body with `return <e>;`
    /// rewritten to assign that local), then repeats the original statement with the call's text replaced
    /// by the local's name. Text rather than tree surgery because the splice *is* a text mechanism
    /// (ADR-0072 §1), and reusing it wholesale is what keeps one expansion path rather than two.
    ///
    /// **One macro call per statement** this wave: the first is spliced and a second would need its own
    /// result local threaded through the same rewrite. A second call is left to the ordinary path, which
    /// refuses it (E0272) rather than expanding only half — named rather than silent.
    fn try_splice_expression_macro(&mut self, stmt: &AstStmt, span: Span) -> Option<StmtId> {
        // A statement-position call has its own arm; skip it here so it is not double-handled.
        if let AstStmt::Expr(e) = stmt
            && let Some(AstExpr::Call(ref c)) = e.expr()
            && self.macro_called_by(c).is_some()
        {
            return None;
        }
        // Find the first macro call anywhere inside this statement's expressions.
        let (call, mac) = self.find_expression_macro_call(stmt.syntax())?;
        if macro_returns_early(&mac.text) {
            self.diags.push(early_return_in_macro(span));
            return Some(self.alloc_stmt(Stmt::Error(span)));
        }
        // A macro with no return type produces no value, so it cannot stand in an expression.
        if !mac.returns {
            self.diags.push(
                Diagnostic::error(
                    span,
                    "a `#expand` macro with no return type produces no value, so it cannot be used in \
                     an expression",
                )
                .with_code(E0273)
                .with_help("give the macro a `-> T` and a `return`, or call it as a statement"),
            );
            return Some(self.alloc_stmt(Stmt::Error(span)));
        }

        let result = format!("__macro_{}", self.macro_result_counter);
        self.macro_result_counter += 1;

        let call_text = call.syntax().text().to_string();
        let stmt_text = stmt.syntax().text().to_string();
        // The original statement with the call replaced by the result local. `replacen` so only the call
        // that was found is replaced — a second identical call keeps its own text and reaches the
        // ordinary path's refusal.
        let rewritten = stmt_text.replacen(&call_text, &result, 1);

        let mut text = String::new();
        // Seeded from the macro's own body so the local's type is the returned expression's, without this
        // pass having to resolve a type: `x := 0;` would fix it to `s64`.
        text.push_str(&format!("{result} := 0;\n"));
        let prelude_and_body = {
            let stmts_before = self.stmts.len();
            let _ = stmts_before;
            self.macro_splice_text(&mac, &call, Some(&result))
        };
        text.push_str(&prelude_and_body);
        text.push_str(&rewritten);
        text.push('\n');
        let stmts = self.expand_insert_text(&text, span);
        Some(self.alloc_stmt(Stmt::Insert {
            stmts,
            operand: None,
            span,
        }))
    }

    /// The first `#expand` macro call inside a statement's expression tree, if any (ADR-0090 §2).
    ///
    /// Walks the CST rather than the HIR, because this runs *before* the statement is lowered — the point
    /// is to rewrite it before it becomes a call node at all.
    fn find_expression_macro_call(
        &self,
        node: &SyntaxNode,
    ) -> Option<(jr_syntax::ast::CallExpr, MacroBody)> {
        for descendant in node.descendants() {
            if let Some(call) = jr_syntax::ast::CallExpr::cast(descendant.clone())
                && let Some(mac) = self.macro_called_by(&call)
            {
                return Some((call, mac));
            }
        }
        None
    }

    /// The text a macro call splices: a `name := arg;` prelude, then the body with its `return` rewritten
    /// (ADR-0090 §2).
    ///
    /// Split out from [`Self::splice_macro`] so the expression-position path can put the text *between* a
    /// result declaration and the rewritten statement, which needs the text rather than the lowered
    /// statements.
    fn macro_splice_text(
        &self,
        mac: &MacroBody,
        call: &jr_syntax::ast::CallExpr,
        result: Option<&str>,
    ) -> String {
        let args: Vec<String> = call
            .arg_list()
            .into_iter()
            .flat_map(|list| list.args().collect::<Vec<_>>())
            .map(|arg| arg.syntax().text().to_string())
            .collect();
        let mut text = String::new();
        for (param, arg) in mac.params.iter().zip(&args) {
            text.push_str(self.interner.resolve(*param));
            text.push_str(" := ");
            text.push_str(arg.trim());
            text.push_str(";\n");
        }
        text.push_str(&rewrite_macro_returns(&mac.text, result));
        text
    }

    /// The macro a call expression names, if it names one (ADR-0090 §2, sub-wave 7b).
    ///
    /// Recognised by the **callee's name text** against the pre-scanned macro map, because lowering runs
    /// before resolution: there is no `Res` yet to ask. A local shadowing a macro's name would therefore
    /// be mistaken for it — which is why a macro's name is looked up only when the callee is a bare name
    /// *and* nothing in scope binds it, the same "a real binding wins" order ADR-0050 §3 uses.
    fn macro_called_by(&self, call: &jr_syntax::ast::CallExpr) -> Option<MacroBody> {
        let Some(AstExpr::Name(name)) = call.callee() else {
            return None;
        };
        let token = name.name_token()?;
        let sym = self.interner.intern(token.text());
        // A local or parameter of that name wins, so a macro cannot capture an ordinary binding.
        if self.lookup_local(sym).is_some() {
            return None;
        }
        self.macros.get(&sym).cloned()
    }

    /// Builds the text a macro call splices, and lowers it in the enclosing scope (ADR-0090 §2).
    ///
    /// The generated text is a **prelude** binding each parameter to its argument's source text, then the
    /// macro's body. The prelude is what makes each argument evaluate **once**: substituting the argument
    /// text at every use of the parameter would re-evaluate a side-effecting argument per use, a wrong
    /// answer rather than a slow one.
    ///
    /// `result` names a local the body's `return <e>;` is rewritten to assign, so a macro works in
    /// expression position too; `None` for a void macro in statement position.
    fn splice_macro(
        &mut self,
        mac: &MacroBody,
        call: &jr_syntax::ast::CallExpr,
        result: Option<&str>,
        span: Span,
    ) -> Vec<StmtId> {
        let text = self.macro_splice_text(mac, call, result);
        self.expand_insert_text(&text, span)
    }

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
        // **A macro call in *expression* position is spliced ahead of the statement** (ADR-0090 §2,
        // sub-wave 7b). `exit(double(21))` becomes, as generated text:
        //
        //     __macro_0 := 0;                 // the result local, seeded so it has a type
        //     x := 21;  __macro_0 = x * 2;    // the prelude and the rewritten body
        //     exit(__macro_0);                // the original statement, call replaced
        //
        // handed to the same splice a `#insert` uses, so the whole thing lands in *this* scope and the
        // macro's body still sees the caller's locals. Checked before every other arm, because the
        // ordinary path would lower the call as a call — which is what E0272 refused when this did not
        // exist. A macro call in *statement* position needs no result local and is handled in its own arm
        // below; this one skips that case.
        if let Some(spliced) = self.try_splice_expression_macro(stmt, span) {
            return spliced;
        }
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
            // **A macro call in statement position splices** (ADR-0090 §2, sub-wave 7b): the macro's
            // statements land here, in the *enclosing* scope, so the body sees this body's locals. No
            // result local is needed — a call whose value nobody reads needs none — but a `return <e>;`
            // in the body still evaluates `<e>` for its effects rather than dropping it.
            AstStmt::Expr(e) if matches!(e.expr(), Some(AstExpr::Call(ref c)) if self.macro_called_by(c).is_some()) =>
            {
                let Some(AstExpr::Call(call)) = e.expr() else {
                    unreachable!("the guard just matched a call")
                };
                let mac = self
                    .macro_called_by(&call)
                    .expect("the guard just matched a macro");
                if macro_returns_early(&mac.text) {
                    self.diags.push(early_return_in_macro(span));
                    return self.alloc_stmt(Stmt::Error(span));
                }
                let stmts = self.splice_macro(&mac, &call, None, span);
                self.alloc_stmt(Stmt::Insert {
                    stmts,
                    operand: None,
                    span,
                })
            }
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
            // **Nested `X :: <value>;` declarations** — nested procedures and local constants,
            // ADR-0134. The body records a pending hoist and registers the name in its scope
            // stack; `LowerCtx::lower_body` drains the pending list after the body finishes and
            // allocates each item into the file's arena, in the same order the predictions were
            // made. That is the "no capture, file-scope proc with a scoped name" shape §7 of
            // PLAN.md decided against a real closure.
            AstItem::Const(cd) => {
                let name = cd
                    .name()
                    .as_ref()
                    .and_then(|n| n.text())
                    .map(|t| self.intern(&t));
                let predicted_id =
                    ItemId::from_usize(self.first_pending_item + self.pending_hoists.len());
                if let Some(name_sym) = name {
                    // Register in the current (innermost) scope. The frame is a Vec of
                    // (Symbol, ScopeEntry) pairs — `lookup_scope` walks it inside-out so the
                    // most recently pushed entry wins, giving ordinary shadow semantics.
                    self.scope_stack
                        .last_mut()
                        .unwrap()
                        .push((name_sym, ScopeEntry::Item(predicted_id)));
                }
                self.pending_hoists.push(PendingHoist {
                    ast: cd.clone(),
                    predicted_id,
                });
                self.alloc_stmt(Stmt::Item(predicted_id, span))
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
                        // **Not "arrives in wave W4".** W4 is complete: ADR-0069 shipped `#run`
                        // across files and inside a body, and `x := #run add(2, 3);` checks clean
                        // today. Whether this arm is still reachable at all is an open question —
                        // a bare `#run add(2, 3);` in a body also checks clean — so the note says
                        // what is owed rather than naming a wave that has shipped.
                        .with_note(
                            "a `#run` in expression position works (ADR-0069); a bare `#run` \
                             statement is owed its own decision about ordering its effects",
                        )
                        .with_help(
                            "use a file-scope `#run` or a `::` constant initialised with `#run`",
                        ),
                );
                self.alloc_stmt(Stmt::Error(span))
            }
            // **Unreachable, and listed rather than `_`-armed.** `INSERT_DECL` is built only by the
            // file-scope dispatcher (ADR-0184 §1); inside a body an `#insert` is a `DIRECTIVE_EXPR`
            // statement and takes the ADR-0072 path. Refused with the same message the other
            // file-scope-only items get, so if the grammar ever does route one here the answer is a
            // diagnostic rather than a silently dropped declaration.
            AstItem::Insert(_) => {
                self.diags.push(
                    Diagnostic::error(
                        span,
                        "a declaration-producing `#insert` is only allowed at file scope",
                    )
                    .with_code(E0208)
                    .with_note(
                        "inside a body, `#insert` splices *statements* into the enclosing scope \
                         (ADR-0072); at file scope it produces declarations",
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
                            ScopeEntry::Item(id) => Res::Item(id),
                        })
                        .unwrap_or(Res::Error);
                    let expr = self.alloc_expr(
                        Expr::Name {
                            name: *sym,
                            module: None,
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
        // **`_ = expr;` is a discard, not an assignment to a variable named `_`** (ADR-0151 §2).
        // Recognised here rather than in the parser, because `_` is a perfectly ordinary identifier
        // token and only its *position* makes it a hole — the same reasoning `lower_targets` uses for
        // a `_` inside a target list. Before this it resolved as a name and reported E0201.
        //
        // Only the plain `=` form: `_ += f()` would have to read `_` first, so it stays an ordinary
        // assignment and reports the unresolved name it genuinely is.
        if a.op_token()
            .is_none_or(|t| lower_assign_op(t.kind()) == AssignOp::Assign)
            && a.lhs().is_some_and(|e| is_underscore_name(&e))
        {
            let value = a
                .rhs()
                .map(|e| self.lower_expr(&e))
                .unwrap_or_else(|| self.alloc_expr(Expr::Error(span), span));
            return self.alloc_stmt(Stmt::Discard { value, span });
        }
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
        // Injected `it` / `it_index` on a nameless `for xs { … }` (ADR-0133). Ordinary
        // locals, so a body can shadow them by declaring `it := something_else`.
        //
        // `it_index` is now injected for **both** sequences and ranges (ADR-0135's follow-up
        // to ADR-0133 §2): MIR emits a per-iteration zero-based-index assignment for a range
        // with a named/injected index, so `for 0..5 { it_index }` gives 0,1,2,3,4 —
        // distinguishable from `it` when the range's start is non-zero.
        let named = f.value_name().is_some();
        let value = if named {
            self.bind_loop_local_by_name(f.value_name().as_ref(), span)
        } else {
            self.bind_loop_local_injected("it", span)
        };
        let index = if named {
            f.index_name()
                .map(|n| self.bind_loop_local_by_name(Some(&n), span))
        } else {
            Some(self.bind_loop_local_injected("it_index", span))
        };
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

    /// Allocates and binds one `for` loop variable named in the source.
    ///
    /// It has no annotation and no initialiser: its type comes from the iterable (`jr-sema`'s job)
    /// and its value from the loop (`jr-mir`'s). `uninit` is **false**, because a loop variable is
    /// assigned on every iteration that runs — marking it uninitialised would make the
    /// definite-assignment pass report a variable the loop guarantees.
    fn bind_loop_local_by_name(
        &mut self,
        name: Option<&jr_syntax::ast::Name>,
        span: Span,
    ) -> LocalId {
        let (sym, name_span) = match name {
            Some(n) => {
                let text = n.text().unwrap_or_else(|| String::from("<error>"));
                (self.intern(&text), self.span_of_node(n.syntax()))
            }
            None => (self.intern("<error>"), span),
        };
        self.alloc_and_define_loop_local(sym, name_span, span)
    }

    /// Allocates and binds an injected `it` or `it_index` on a nameless `for`.
    ///
    /// The span of the binding is the `for` keyword's span — a name that has no source
    /// location cannot point at one — and the injected locals are **ordinary**, so a body that
    /// declares `it := …` shadows them exactly as it would any local (ADR-0133 §1). That is
    /// the point: injection reads as a *default*, not a reservation of a keyword.
    fn bind_loop_local_injected(&mut self, name: &str, span: Span) -> LocalId {
        let sym = self.intern(name);
        self.alloc_and_define_loop_local(sym, span, span)
    }

    fn alloc_and_define_loop_local(&mut self, sym: Symbol, name_span: Span, span: Span) -> LocalId {
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
            // See `lower_top_expr`'s twin arm for why the element type is an expression.
            AstExpr::ArrayLiteral(al) => {
                let elem_ty = al
                    .element_type()
                    .map(|t| self.lower_expr(&t))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Error(span), span));
                let elems: Vec<ExprId> = al.elements().map(|e| self.lower_expr(&e)).collect();
                self.alloc_expr(
                    Expr::ArrayLit {
                        elem_ty,
                        elems,
                        span,
                    },
                    span,
                )
            }
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
                        ScopeEntry::Item(id) => Res::Item(id),
                    })
                    .unwrap_or(Res::Error);
                self.alloc_expr(
                    Expr::Name {
                        name,
                        module: None,
                        span,
                        res,
                    },
                    span,
                )
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
                let (name, name_span) = f
                    .field_name()
                    .map(|t| (self.intern(t.text()), self.span_of_token(&t)))
                    .unwrap_or_else(|| (self.intern("<error>"), span));
                // **`Simp.foo` is a qualified name, not a field access** (ADR-0179 §4) — unless a
                // local or a parameter of that name is in scope, in which case the binding wins and
                // this is an ordinary field of a value. That is ADR-0014 §3's rule, applied by
                // *where* the check sits rather than by a rule of its own.
                if let Some(alias) = field_receiver_alias(f, self.interner, self.import_aliases)
                    && self.lookup_local(alias).is_none()
                {
                    return self.alloc_expr(
                        Expr::Name {
                            name,
                            module: Some(alias),
                            span,
                            res: Res::Error,
                        },
                        span,
                    );
                }
                let receiver = f
                    .object()
                    .map(|e| self.lower_expr(&e))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Error(span), span));
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
/// Takes the length *expression* rather than the array node, so that `#simd [N]T`'s lane count reuses
/// this rather than copying it (ADR-0148 §1). A vector's count is the same question an array's is —
/// literal, or a name to resolve — and two implementations would be two chances to disagree about
/// which spellings are accepted.
fn lower_array_len_name(len: Option<AstExpr>, interner: &Interner) -> Option<Symbol> {
    let AstExpr::Name(name) = len? else {
        return None;
    };
    Some(interner.intern(name.name_token()?.text()))
}

/// Takes the length *expression*, for the reason [`lower_array_len_name`] above does.
fn lower_array_len(
    len: Option<AstExpr>,
    len_span: Span,
    interner: &Interner,
    diags: &mut Diagnostics,
) -> Option<u64> {
    let AstExpr::Literal(lit) = len? else {
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
                instantiation_sites: Vec::new(),
                param_values: Vec::new(),
                modify_predicates: Vec::new(),
                predicate_vars: Vec::new(),
            },
            diags,
        );
    };

    // Every `#expand` macro's spliceable shape, collected **before** any body is lowered (ADR-0090 §2):
    // a call needs the macro's text, and a call may precede the declaration in source order.
    let macros = collect_macro_bodies(&source_file, interner);
    // Every aliased `#import`'s name, for the same reason and by the same shape (ADR-0179 §4).
    let aliases = collect_import_aliases(&source_file, interner);
    let mut ctx = LowerCtx::new(file, interner, operands, macros, aliases);

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
            AstItem::Insert(idl) => ctx.lower_insert_decl(&idl),
        }
    }

    ctx.finish()
}

/// Whether a generated item is a **library declaration** — `gl :: #system_library "GL";` (ADR-0184 §4).
///
/// The one shape a *computed* file-scope `#insert` may produce, because it is the one that needs nothing
/// from a phase that has already run: `wanted()` excludes a directive from const-eval, and the library is
/// read from the pool by whoever links.
///
/// Recognised on the **AST**, before lowering, so a refused item is never allocated — an item in the arena
/// that later turns out to be unusable is the well-typed-placeholder shape AGENTS.md warns about.
fn is_library_declaration(item: &AstItem) -> bool {
    let AstItem::Const(decl) = item else {
        return false;
    };
    let Some(AstExpr::Directive(directive)) = decl.value_expr() else {
        return false;
    };
    let Some(token) = directive.directive_token() else {
        return false;
    };
    matches!(
        token.text().trim_start_matches('#'),
        "system_library" | "framework" | "compiler_library"
    )
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
/// The source text of every `#expand` macro in a file, by name (ADR-0090 §2, sub-wave 7b).
///
/// Collected in a pre-scan of the `SOURCE_FILE` before any body is lowered, and threaded to each
/// `BodyLowerCtx` exactly as [`InsertOperands`] is — `lower_file_with_inserts` already holds the whole AST
/// and already threads one such map, so this is the same proven shape rather than a new one.
///
/// Each entry is `(parameter names, body inner text, return type present)`. A call to the macro is lowered
/// by *generating text* — a `name := arg;` prelude then the body — and handing it to the splice, so the
/// text is what has to be reachable from a call site.
type MacroBodies = rustc_hash::FxHashMap<jr_base::Symbol, MacroBody>;

/// One `#expand` macro's spliceable shape (ADR-0090 §2).
#[derive(Debug, Clone)]
struct MacroBody {
    /// The parameter names, in order — bound to the call's arguments by a synthesized prelude.
    params: Vec<jr_base::Symbol>,
    /// The body's inner source text, braces excluded (as `#code` takes it, ADR-0080 §2).
    text: String,
    /// Whether the macro declares a return type, so a call in expression position needs a result local.
    returns: bool,
}

/// Collects every `#expand` macro's spliceable shape from a parsed file (ADR-0090 §2).
///
/// Walks the file's items rather than the whole tree: a macro is a file-level `name :: (…) #expand { … }`,
/// so nothing nested can be one. A macro with no body is skipped — the parser already reported it.
fn collect_macro_bodies(source_file: &SourceFile, interner: &Interner) -> MacroBodies {
    let mut out = MacroBodies::default();
    for item in source_file.items() {
        let jr_syntax::ast::Item::Const(decl) = item else {
            continue;
        };
        let Some(proc) = decl.proc() else { continue };
        if !proc.is_expand() {
            continue;
        }
        let Some(name_tok) = decl.name().and_then(|n| n.ident_token()) else {
            continue;
        };
        let Some(block) = proc.body() else { continue };
        let params: Vec<jr_base::Symbol> = proc
            .param_list()
            .into_iter()
            .flat_map(|pl| pl.params().collect::<Vec<_>>())
            .filter_map(|p| p.name_token().map(|t| interner.intern(t.text())))
            .collect();
        out.insert(
            interner.intern(name_tok.text()),
            MacroBody {
                params,
                text: block_inner_text(block.syntax()),
                returns: proc.ret_type().is_some(),
            },
        );
    }
    out
}

/// Every name an aliased `#import` binds in a file (ADR-0179 §4).
type ImportAliases = rustc_hash::FxHashSet<jr_base::Symbol>;

/// Collects the names the file's aliased `#import`s bind, before any item is lowered.
///
/// A pre-scan for [`collect_macro_bodies`]'s reason: `Simp.foo` may be written above the
/// `Simp :: #import "Simp";` that binds `Simp`, and lowering walks the file once.
///
/// Purely syntactic — a file-scope constant whose value is an `#import` directive — so it needs no
/// resolution and cannot disagree with what [`LowerCtx::lower_const_decl_with_inherited`] then
/// lowers that declaration to.
fn collect_import_aliases(source_file: &SourceFile, interner: &Interner) -> ImportAliases {
    let mut out = ImportAliases::default();
    for item in source_file.items() {
        let jr_syntax::ast::Item::Const(decl) = item else {
            continue;
        };
        if import_directive_value(&decl).is_none() {
            continue;
        }
        if let Some(tok) = decl.name().and_then(|n| n.ident_token()) {
            out.insert(interner.intern(tok.text()));
        }
    }
    out
}

/// The module alias a field access's receiver names, if it names one (ADR-0179 §4).
///
/// `Some(sym)` only when the receiver is a **bare name** an aliased `#import` binds. A receiver that
/// is anything else — a call, an index, a parenthesised expression — is a value, and a value's field
/// is a field. The caller in body position additionally checks that no local shadows the alias, so an
/// ordinary binding always wins, silently, exactly as ADR-0014 §3 has it for every other name.
fn field_receiver_alias(
    f: &jr_syntax::ast::FieldExpr,
    interner: &Interner,
    aliases: &ImportAliases,
) -> Option<Symbol> {
    if aliases.is_empty() {
        return None;
    }
    let AstExpr::Name(n) = f.object()? else {
        return None;
    };
    let sym = interner.intern(n.name_token()?.text());
    aliases.contains(&sym).then_some(sym)
}

/// Rewrites a macro body's `return <e>;` into an assignment to the splice's result local (ADR-0090 §2).
///
/// A macro has no frame of its own — its statements land in the caller's — so a `return` in it cannot mean
/// "return from the macro". This wave gives it the **weaker, well-defined** meaning: assign the value to
/// the local the call's value is read from. `return;` with no value becomes nothing at all.
///
/// **Only a `return` in tail position is handled**, which is the limit this wave accepts and names: a
/// `return` inside an `if` in a macro body would have to return from the *caller* (Jai's semantics), and
/// silently turning it into an assignment would fall through to the statements after it. `lower_call`
/// refuses that shape rather than miscompiling it.
/// A `return` that is not the last statement of a macro body (ADR-0090 §2).
///
/// Such a `return` must return from the **caller** — Jai's semantics — which this wave defers because it
/// changes what `return` means by provenance and interacts with `defer` (ADR-0049 §3). Refused rather than
/// rewritten into an assignment, which would fall through to the statements after it: a wrong answer.
fn early_return_in_macro(span: Span) -> Diagnostic {
    Diagnostic::error(
        span,
        "a `return` that is not the last statement of a `#expand` macro is not yet supported",
    )
    .with_code(E0273)
    .with_note(
        "a macro's statements are spliced into the caller, so an early `return` would have to return \
         from the *caller* — that arrives in a later sub-wave (ADR-0090 §2)",
    )
    .with_help("put the `return` last, or use an `if` that assigns and falls through")
}

fn rewrite_macro_returns(body: &str, result: Option<&str>) -> String {
    let mut out = String::with_capacity(body.len());
    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("return") {
            let value = rest.trim().trim_end_matches(';').trim();
            match (result, value.is_empty()) {
                // `return <e>;` in a value macro: assign to the result local.
                (Some(name), false) => {
                    out.push_str(name);
                    out.push_str(" = ");
                    out.push_str(value);
                    out.push_str(";\n");
                }
                // `return;` — nothing to carry, and no frame to leave.
                (_, true) => {}
                // A value returned by a macro whose call wants none: evaluate it for its effects, so a
                // call in statement position does not silently drop a side effect.
                (None, false) => {
                    out.push_str(value);
                    out.push_str(";\n");
                }
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Whether a macro body has a `return` anywhere but its **last** statement (ADR-0090 §2).
///
/// Such a `return` needs the caller-return semantics this wave defers, so `lower_call` refuses it rather
/// than rewriting it into an assignment that would fall through to the statements after it.
fn macro_returns_early(body: &str) -> bool {
    let lines: Vec<&str> = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    lines
        .iter()
        .enumerate()
        .any(|(i, l)| l.starts_with("return") && i + 1 != lines.len())
}

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

/// The call a `#bake_arguments` constant's value applies, if the value is one (ADR-0097 §1).
///
/// `add_five :: #bake_arguments add(a = 5);` — the directive's operand is a **call expression**, whose callee
/// names the procedure to specialise and whose arguments are the ones to bake. Recognised by the directive's
/// name, the way `insert_directive` recognises `#insert`, so no grammar rule distinguishes them.
fn bake_arguments_operand(cd: &ConstDecl) -> Option<jr_syntax::ast::CallExpr> {
    let AstExpr::Directive(directive) = cd.value_expr()? else {
        return None;
    };
    let token = directive.directive_token()?;
    if token.text().trim_start_matches('#') != "bake_arguments" {
        return None;
    }
    directive
        .syntax()
        .children()
        .find_map(jr_syntax::ast::CallExpr::cast)
}

/// The `#import` directive a constant declaration's value is, if it is one (ADR-0179 §1).
///
/// `Simp :: #import "Simp";` — recognised by the directive's name, like
/// [`bake_arguments_operand`] above, because the aliased form needs no grammar rule: a directive
/// expression is already a legal constant value as far as the parser is concerned.
fn import_directive_value(cd: &ConstDecl) -> Option<jr_syntax::ast::DirectiveExpr> {
    let AstExpr::Directive(directive) = cd.value_expr()? else {
        return None;
    };
    let token = directive.directive_token()?;
    (token.text().trim_start_matches('#') == "import").then_some(directive)
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
    // **`#bake_arguments` is refused by design, with its own code** (ADR-0096 §3). It is in the allowlist
    // above so the parser's call-shaped operand is not rejected as "not valid here" — the *surface* is
    // real — but its specialisation is the next sub-wave. Refused here rather than left to lower to a
    // poisoned expression, which made the *caller* report "the compiler could not lower `main` … this
    // compiler has a gap — please report it": right for an unknown gap, wrong for a named one.
    if name == "bake_arguments" {
        return Some(
            Diagnostic::error(
                span,
                "`#bake_arguments` is not yet supported",
            )
            .with_code(E0276)
            .with_note(
                "it produces a specialised procedure with some arguments built in; the specialisation \
                 arrives in the next sub-wave (ADR-0096)",
            )
            .with_help("call the procedure with all its arguments, or write a wrapper procedure"),
        );
    }
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

/// The `$T` variable names a parameter list introduces, read from the shared type-ref arena
/// (ADR-0094 §1).
///
/// Duplicates a little of `jr-sema`'s `collect_poly_vars`, deliberately: that one runs on resolved
/// signatures and this needs the answer during *lowering*, before any signature exists. Both read the same
/// `TypeRef::Poly`, so they cannot disagree about what a variable is.
fn poly_var_names_of(params: &[Param], arena: &[TypeRef]) -> Vec<Symbol> {
    fn walk(id: TypeRefId, arena: &[TypeRef], out: &mut Vec<Symbol>) {
        match arena.get(id.index()) {
            Some(TypeRef::Poly(sym)) => {
                if !out.contains(sym) {
                    out.push(*sym);
                }
            }
            Some(TypeRef::Pointer(inner)) => walk(*inner, arena, out),
            Some(
                TypeRef::Array { elem, .. }
                | TypeRef::View { elem }
                | TypeRef::DynamicArray { elem },
            ) => walk(*elem, arena, out),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for param in params {
        if let Some(id) = param.ty {
            walk(id, arena, &mut out);
        }
    }
    out
}

/// `#bake_arguments` whose operand does not name a procedure declared in this file (ADR-0097 §1).
fn bake_needs_a_procedure(span: Span) -> Diagnostic {
    Diagnostic::error(
        span,
        "`#bake_arguments` needs a call to a procedure declared in this file",
    )
    .with_code(E0276)
    .with_note(
        "the specialised procedure is a *clone* of the named one, and another file's body is not in this \
         tree — a cross-file bake is deferred with the cross-file splice (ADR-0091 §3)",
    )
    .with_help("move the procedure into this file, or write a wrapper")
}

/// A baked argument that is not a literal this pass can read (ADR-0097 §1).
fn bake_needs_a_literal(span: Span) -> Diagnostic {
    Diagnostic::error(
        span,
        "a `#bake_arguments` value must be a literal",
    )
    .with_code(E0276)
    .with_note(
        "lowering has no constant evaluator (ADR-0018 §3 puts it downstream), and the value is needed \
         *here*, where the specialised procedure is built — the same narrower rule an array length had \
         before ADR-0070 widened it",
    )
    .with_help("write the value as a literal, e.g. `#bake_arguments add(a = 5)`")
}

/// The literal an argument node carries, if it carries one (ADR-0097 §1).
///
/// Takes a **`SyntaxNode`** rather than an `ast::Expr`, because a named argument is its own node kind
/// (`NAMED_ARG`) and not an `Expr` variant — the same reason `lower_args` walks the arg list's children
/// rather than `ArgList::args()` (ADR-0053 §1, where filtering on `is_expr_kind` silently dropped every
/// named argument).
fn literal_of(node: &SyntaxNode) -> Option<Literal> {
    // A named argument wraps its value; unwrap one level, then read the literal.
    let value_node = if node.kind() == jr_syntax::SyntaxKind::NAMED_ARG {
        node.children()
            .find(|n| jr_syntax::ast::LiteralExpr::cast(n.clone()).is_some())?
    } else {
        node.clone()
    };
    let lit = jr_syntax::ast::LiteralExpr::cast(value_node)?;
    let token = lit.token()?;
    // Only an integer literal this wave, which is what a baked argument is in practice; a wider set follows
    // the same route once there is a reason to widen.
    token.text().parse::<i128>().ok().map(|value| Literal::Int {
        value,
        radix: 10,
        overflowed: false,
    })
}

/// The parameter name a named argument names, if the node is one (ADR-0053 §1's spelling).
fn named_arg_name(node: &SyntaxNode) -> Option<String> {
    if node.kind() != jr_syntax::SyntaxKind::NAMED_ARG {
        return None;
    }
    jr_syntax::ast::NamedArg::cast(node.clone())?.name()?.text()
}

/// Whether an expression is the bare name `_` — the discard hole (ADR-0151 §2).
///
/// A text comparison on the token, matching `lower_targets`'s test for the same thing, because `_` is
/// an ordinary identifier to the lexer and there is no `UNDERSCORE` kind to match on.
fn is_underscore_name(expr: &jr_syntax::ast::Expr) -> bool {
    matches!(expr, jr_syntax::ast::Expr::Name(n) if n
        .name_token()
        .is_some_and(|t| t.text() == "_"))
}
