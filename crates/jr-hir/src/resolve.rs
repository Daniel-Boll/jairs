//! Name resolution for HIR.
//!
//! This module resolves name references within a single file. It operates on
//! the already-lowered [`FileHir`] and fills in the `res` field of every
//! `Expr::Name` node.
//!
//! ## Order independence at file level
//!
//! File-level items are collected first (the item tree is already built by
//! [`crate::lower_file`]), then name references are resolved. This means a
//! constant can refer to a name declared later in the file — declaration order
//! does not matter at file level (spec §02, ADR-0007).
//!
//! ## Order sensitivity inside bodies
//!
//! Inside a body, locals ARE order-sensitive: a local is visible only after
//! its declaration. This is handled during lowering in [`crate::lower`] by
//! the scope stack — a `Res::Local` is only set when the local has already
//! been declared in the current scope chain. After lowering, `Expr::Name`
//! nodes that resolved to a local already have `res = Res::Local(id)`.
//!
//! ## Diagnostics
//!
//! | Code  | Condition |
//! |-------|-----------|
//! | E0200 | Duplicate file-level declaration of the same name |
//! | E0201 | Unresolved name (not a local, param, file-level item, or import) |
//!
//! Note: E0200 (duplicate declaration) is detected here rather than in
//! lowering because we need to see all items before we can detect duplicates.
//! The item scope built during lowering uses last-write-wins; we detect
//! duplicates by scanning the item list for repeated names.

use jr_base::{Interner, Span, Symbol};
use jr_diag::{Diagnostic, Diagnostics, Label};
use rustc_hash::FxHashMap;

use crate::hir::{BodyId, Expr, ExprId, FileHir, ItemId, ItemKind, ItemScope, Res, Stmt, StmtId};

// ---------------------------------------------------------------------------
// Diagnostic codes
// ---------------------------------------------------------------------------

const E0200: &str = "E0200";
const E0201: &str = "E0201";

// ---------------------------------------------------------------------------
// ResolveMap
// ---------------------------------------------------------------------------

/// The result of name resolution: a map from `ExprId` to `Res`.
///
/// This is separate from the HIR so that resolution can be re-run without
/// mutating the HIR (important for incremental compilation via salsa).
///
/// For now, resolution results are stored here rather than mutated into the
/// HIR. Downstream passes look up resolution results via this map.
#[derive(Debug, Default)]
pub struct ResolveMap {
    /// Maps expression IDs (for `Expr::Name` nodes) to their resolution.
    ///
    /// Only `Expr::Name` nodes appear here; other expression kinds are absent.
    pub resolutions: FxHashMap<ExprId, Res>,
}

impl ResolveMap {
    /// Creates an empty resolve map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the resolution for an expression, if any.
    pub fn get(&self, id: ExprId) -> Option<Res> {
        self.resolutions.get(&id).copied()
    }

    /// Inserts a resolution.
    pub fn insert(&mut self, id: ExprId, res: Res) {
        self.resolutions.insert(id, res);
    }
}

// ---------------------------------------------------------------------------
// Resolution context
// ---------------------------------------------------------------------------

struct ResolveCtx<'a> {
    hir: &'a FileHir,
    interner: &'a Interner,
    /// Imported scopes: (module_name, scope)
    imports: &'a [(&'a str, &'a ItemScope)],
    diags: Diagnostics,
    map: ResolveMap,
}

impl<'a> ResolveCtx<'a> {
    fn new(
        hir: &'a FileHir,
        imports: &'a [(&'a str, &'a ItemScope)],
        interner: &'a Interner,
    ) -> Self {
        Self {
            hir,
            interner,
            imports,
            diags: Diagnostics::new(),
            map: ResolveMap::new(),
        }
    }

    /// Resolve a name to a `Res`, checking file scope and imports.
    fn resolve_name(&self, name: Symbol) -> Res {
        // Check file-level scope first
        if let Some(item_id) = self.hir.scope.get(name) {
            return Res::Item(item_id);
        }

        // Check imported scopes
        for (mod_name, scope) in self.imports {
            if scope.get(name).is_some() {
                // Find the import item in the current file
                for (i, item) in self.hir.items.iter().enumerate() {
                    if let ItemKind::Import { path, .. } = &item.kind {
                        if path == mod_name {
                            let import_id = ItemId::from_usize(i);
                            return Res::Imported(import_id, name);
                        }
                    }
                }
            }
        }

        Res::Error
    }

    /// Resolve all name expressions in the file.
    fn resolve_all(&mut self) {
        // Check for duplicate file-level declarations
        self.check_duplicates();

        // Resolve top-level expressions
        let n_exprs = self.hir.exprs.len();
        for i in 0..n_exprs {
            let id = ExprId::from_usize(i);
            self.resolve_top_expr(id);
        }

        // Resolve expressions inside bodies
        let n_bodies = self.hir.bodies.len();
        for i in 0..n_bodies {
            let body_id = BodyId::from_usize(i);
            self.resolve_body(body_id);
        }
    }

    fn check_duplicates(&mut self) {
        let mut seen: FxHashMap<Symbol, (ItemId, Span)> = FxHashMap::default();
        for (i, item) in self.hir.items.iter().enumerate() {
            let Some(name) = item.name else { continue };
            let item_id = ItemId::from_usize(i);
            if let Some((_orig_id, orig_span)) = seen.get(&name) {
                let name_text = self.interner.resolve(name);
                let diag = Diagnostic::error(
                    item.name_span,
                    format!("duplicate declaration of `{name_text}`"),
                )
                .with_code(E0200)
                .with_label(Label::with_message(
                    *orig_span,
                    format!("`{name_text}` first declared here"),
                ));
                self.diags.push(diag);
            } else {
                seen.insert(name, (item_id, item.name_span));
            }
        }
    }

    fn resolve_top_expr(&mut self, id: ExprId) {
        // We need to clone to avoid borrow issues
        let expr = self.hir.exprs[id.index()].clone();
        match &expr {
            Expr::Name { name, span, res } => {
                if matches!(res, Res::Error) {
                    let resolved = self.resolve_name(*name);
                    if matches!(resolved, Res::Error) {
                        let name_text = self.interner.resolve(*name);
                        let diag =
                            Diagnostic::error(*span, format!("unresolved name `{name_text}`"))
                                .with_code(E0201);
                        self.diags.push(diag);
                    }
                    self.map.insert(id, resolved);
                } else {
                    self.map.insert(id, *res);
                }
            }
            Expr::Binary { lhs, rhs, .. } => {
                let (lhs, rhs) = (*lhs, *rhs);
                self.resolve_top_expr(lhs);
                self.resolve_top_expr(rhs);
            }
            Expr::Unary { operand, .. } => {
                let operand = *operand;
                self.resolve_top_expr(operand);
            }
            Expr::Call { callee, args, .. } => {
                let callee = *callee;
                let args = args.clone();
                self.resolve_top_expr(callee);
                for arg in args {
                    self.resolve_top_expr(arg);
                }
            }
            Expr::Field { receiver, .. } => {
                let receiver = *receiver;
                self.resolve_top_expr(receiver);
            }
            Expr::Deref(ptr, _) => {
                let ptr = *ptr;
                self.resolve_top_expr(ptr);
            }
            Expr::Run(inner, _) => {
                let inner = *inner;
                self.resolve_top_expr(inner);
            }
            Expr::Literal(..) | Expr::Uninit(_) | Expr::Directive { .. } | Expr::Error(_) => {}
        }
    }

    fn resolve_body(&mut self, body_id: BodyId) {
        let root = self.hir.bodies[body_id.index()].root;
        self.resolve_body_stmt(body_id, root);
    }

    fn resolve_body_stmt(&mut self, body_id: BodyId, stmt_id: StmtId) {
        // Clone to avoid borrow issues
        let stmt = self.hir.bodies[body_id.index()].stmts[stmt_id.index()].clone();
        match stmt {
            Stmt::Block(stmts, _) => {
                for sid in stmts {
                    self.resolve_body_stmt(body_id, sid);
                }
            }
            Stmt::Local(local_id, _) => {
                let local = self.hir.bodies[body_id.index()].locals[local_id.index()].clone();
                if let Some(init) = local.init {
                    self.resolve_body_expr(body_id, init);
                }
            }
            Stmt::Item(_, _) => {}
            Stmt::Expr(expr_id, _) => {
                self.resolve_body_expr(body_id, expr_id);
            }
            Stmt::Assign { lhs, rhs, .. } => {
                self.resolve_body_expr(body_id, lhs);
                self.resolve_body_expr(body_id, rhs);
            }
            Stmt::If {
                cond, then, else_, ..
            } => {
                self.resolve_body_expr(body_id, cond);
                self.resolve_body_stmt(body_id, then);
                if let Some(else_stmt) = else_ {
                    self.resolve_body_stmt(body_id, else_stmt);
                }
            }
            Stmt::While { cond, body, .. } => {
                self.resolve_body_expr(body_id, cond);
                self.resolve_body_stmt(body_id, body);
            }
            Stmt::Return(expr, _) => {
                if let Some(e) = expr {
                    self.resolve_body_expr(body_id, e);
                }
            }
            Stmt::Break(_) | Stmt::Continue(_) | Stmt::Error(_) => {}
        }
    }

    fn resolve_body_expr(&mut self, body_id: BodyId, expr_id: ExprId) {
        let expr = self.hir.bodies[body_id.index()].exprs[expr_id.index()].clone();
        match expr {
            Expr::Name { name, span, res } => {
                // If already resolved to a local/param during lowering, keep it.
                // Otherwise try file-level resolution.
                let final_res = if !matches!(res, Res::Error) {
                    res
                } else {
                    let resolved = self.resolve_name(name);
                    if matches!(resolved, Res::Error) {
                        let name_text = self.interner.resolve(name);
                        let diag =
                            Diagnostic::error(span, format!("unresolved name `{name_text}`"))
                                .with_code(E0201);
                        self.diags.push(diag);
                    }
                    resolved
                };
                self.map.insert(expr_id, final_res);
            }
            Expr::Binary { lhs, rhs, .. } => {
                self.resolve_body_expr(body_id, lhs);
                self.resolve_body_expr(body_id, rhs);
            }
            Expr::Unary { operand, .. } => {
                self.resolve_body_expr(body_id, operand);
            }
            Expr::Call { callee, args, .. } => {
                self.resolve_body_expr(body_id, callee);
                for arg in args {
                    self.resolve_body_expr(body_id, arg);
                }
            }
            Expr::Field { receiver, .. } => {
                self.resolve_body_expr(body_id, receiver);
            }
            Expr::Deref(ptr, _) => {
                self.resolve_body_expr(body_id, ptr);
            }
            Expr::Run(inner, _) => {
                self.resolve_body_expr(body_id, inner);
            }
            Expr::Literal(..) | Expr::Uninit(_) | Expr::Directive { .. } | Expr::Error(_) => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Resolves name references in a lowered file HIR.
///
/// This is a pure function: it takes the already-lowered [`FileHir`] and
/// returns a [`ResolveMap`] mapping expression IDs to their resolutions,
/// plus any diagnostics.
///
/// `imports` is a slice of `(module_name, scope)` pairs for modules that
/// have been imported via `#import`. Pass an empty slice if no imports have
/// been resolved yet; unresolved names will be reported as errors.
///
/// ## Order independence
///
/// File-level items are resolved in any order — a constant may refer to a
/// name declared later in the file. Inside bodies, locals are order-sensitive
/// (already handled during lowering).
pub fn resolve(
    file: &FileHir,
    imports: &[(&str, &ItemScope)],
    interner: &Interner,
) -> (ResolveMap, Diagnostics) {
    let mut ctx = ResolveCtx::new(file, imports, interner);
    ctx.resolve_all();
    (ctx.map, ctx.diags)
}
