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
//! ## Lookup order (spec §03, ADR-0014 §3)
//!
//! Innermost first: block locals → parameters → **this file's own file-scope
//! items** → imported scopes. A file-level declaration silently shadows an
//! imported name of the same name.
//!
//! ## Import semantics (ADR-0014 §2)
//!
//! Imported names merge in flat: after `#import "Shapes";`, `Rect` and `area`
//! resolve directly with no `Shapes.` qualification. The resolution is
//! `Res::Imported(import_item_id, name)` where `import_item_id` is the
//! `#import` item in the *importing* file.
//!
//! ## Ambiguity (ADR-0014 §3)
//!
//! If two or more **distinct** imported modules provide the same name and that
//! name is used, the use is E0211. Importing the same module twice is
//! idempotent (ADR-0014 §6): duplicates are deduplicated by module name before
//! the ambiguity check.
//!
//! ## Diagnostics
//!
//! | Code  | Condition |
//! |-------|-----------|
//! | E0200 | Duplicate file-level declaration of the same name |
//! | E0201 | Unresolved name (not a local, param, file-level item, or import) |
//! | E0211 | Ambiguous name provided by two or more imported modules |
//!
//! Note: E0200 (duplicate declaration) is detected here rather than in
//! lowering because we need to see all items before we can detect duplicates.
//! The item scope built during lowering uses last-write-wins; we detect
//! duplicates by scanning the item list for repeated names.
//!
//! E0210 (module not found) is owned by `jr-db`, not this crate.

use jr_base::{Interner, Span, Symbol};
use jr_diag::{Diagnostic, Diagnostics, Label};
use rustc_hash::FxHashMap;

use crate::hir::{
    BodyId, ConstValue, Expr, ExprId, FileHir, ForIterable, ItemId, ItemKind, ItemScope, Res, Stmt,
    StmtId,
};

// ---------------------------------------------------------------------------
// Diagnostic codes
// ---------------------------------------------------------------------------

const E0200: &str = "E0200";
const E0201: &str = "E0201";
/// Ambiguous name provided by two or more imported modules.
const E0211: &str = "E0211";

// ---------------------------------------------------------------------------
// ResolveMap
// ---------------------------------------------------------------------------

/// Which expression arena an [`ExprId`] indexes.
///
/// This exists because `ExprId`s are **not** unique across a file.
/// [`FileHir::exprs`](crate::FileHir::exprs) and every [`Body::exprs`](crate::Body::exprs) are independent arenas that all
/// start at index 0, so an `ExprId` alone does not say what it refers to. A map
/// keyed on `ExprId` alone silently collides: the last writer wins, and a
/// top-level constant's name reference ends up resolved to whatever local
/// happened to share its index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExprScope {
    /// [`FileHir::exprs`](crate::FileHir::exprs) — constant values, variable initialisers, top-level
    /// `#run`.
    TopLevel,
    /// The [`Body::exprs`](crate::Body::exprs) arena of one procedure body.
    Body(BodyId),
}

/// The result of name resolution: a map from expression to [`Res`].
///
/// This is separate from the HIR so that resolution can be re-run without
/// mutating the HIR (important for incremental compilation via salsa).
///
/// Keys are `(ExprScope, ExprId)` rather than a bare `ExprId`; see
/// [`ExprScope`] for why a bare `ExprId` is not a unique key.
#[derive(Debug, Default)]
pub struct ResolveMap {
    /// Maps `(arena, expression ID)` for `Expr::Name` nodes to their
    /// resolution.
    ///
    /// Only `Expr::Name` nodes appear here; other expression kinds are absent.
    pub resolutions: FxHashMap<(ExprScope, ExprId), Res>,
}

impl ResolveMap {
    /// Creates an empty resolve map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the resolution for an expression in a given arena, if any.
    pub fn get(&self, scope: ExprScope, id: ExprId) -> Option<Res> {
        self.resolutions.get(&(scope, id)).copied()
    }

    /// Returns the resolution for a top-level expression, if any.
    ///
    /// Convenience for [`ExprScope::TopLevel`].
    pub fn get_top(&self, id: ExprId) -> Option<Res> {
        self.get(ExprScope::TopLevel, id)
    }

    /// Returns the resolution for an expression inside a body, if any.
    ///
    /// Convenience for [`ExprScope::Body`].
    pub fn get_in_body(&self, body: BodyId, id: ExprId) -> Option<Res> {
        self.get(ExprScope::Body(body), id)
    }

    /// Inserts a resolution.
    pub fn insert(&mut self, scope: ExprScope, id: ExprId, res: Res) {
        self.resolutions.insert((scope, id), res);
    }
}

// ---------------------------------------------------------------------------
// Import index
// ---------------------------------------------------------------------------

/// A pre-built index of the imports in the current file.
///
/// For each name that appears in at least one imported scope, records the
/// list of distinct modules that provide it. "Distinct" means distinct by
/// module name (the `&str` key in the `imports` slice): importing the same
/// module twice is idempotent (ADR-0014 §6).
///
/// Each entry is `(module_name, import_item_id)` where `import_item_id` is
/// the `#import` item in the *importing* file.
struct ImportIndex<'a> {
    /// Maps name → list of (module_name, import_item_id) for distinct modules.
    ///
    /// If a name maps to exactly one entry, it is unambiguous. If it maps to
    /// two or more, it is ambiguous (E0211 at the use site).
    by_name: FxHashMap<Symbol, Vec<(&'a str, ItemId)>>,
}

/// The result of looking up a name in the import index.
///
/// - `Ok((import_id, name))` — exactly one module provides this name.
/// - `Err(providers)` — two or more distinct modules provide this name
///   (ambiguous; E0211 should be emitted at the use site).
type ImportLookup<'a> = Result<(ItemId, Symbol), Vec<(&'a str, ItemId)>>;

impl<'a> ImportIndex<'a> {
    /// Builds the index from the `imports` slice and the file's item list.
    ///
    /// `imports` is `(module_name, scope)` pairs. The module name is the
    /// canonical key used for deduplication: two entries with the same name
    /// are the same module (ADR-0014 §6).
    fn build(hir: &FileHir, imports: &'a [(&'a str, &'a ItemScope)], interner: &Interner) -> Self {
        // Deduplicate imports by module name: keep only the first occurrence
        // of each module name. This implements ADR-0014 §6 (duplicate import
        // is idempotent).
        let mut seen_modules: FxHashMap<&str, ()> = FxHashMap::default();
        let mut deduped: Vec<(&str, &ItemScope, ItemId)> = Vec::new();

        for (mod_name, scope) in imports {
            if seen_modules.contains_key(mod_name) {
                // Same module imported again — skip.
                continue;
            }
            seen_modules.insert(mod_name, ());

            // Find the first `#import` item in the file whose path matches
            // this module name.
            let import_item_id = hir.items.iter().enumerate().find_map(|(i, item)| {
                if let ItemKind::Import { path, .. } = &item.kind
                    && path == mod_name
                {
                    return Some(ItemId::from_usize(i));
                }
                None
            });

            if let Some(import_id) = import_item_id {
                deduped.push((mod_name, scope, import_id));
            }
            // If no matching #import item is found (shouldn't happen in
            // well-formed input), skip silently — the caller is responsible
            // for passing consistent data.
        }

        // Build the by-name index.
        let mut by_name: FxHashMap<Symbol, Vec<(&'a str, ItemId)>> = FxHashMap::default();
        for (mod_name, scope, import_id) in &deduped {
            for &sym in scope.names.keys() {
                // Skip names that are shadowed by a file-level declaration.
                // We check this here so the index only contains names that
                // are actually reachable via imports.
                if hir.scope.get(sym).is_some() {
                    continue;
                }
                by_name.entry(sym).or_default().push((mod_name, *import_id));
            }
        }

        // Suppress unused-variable warning for interner when no names exist.
        let _ = interner;

        Self { by_name }
    }

    /// Looks up a name in the import index.
    ///
    /// Returns:
    /// - `None` if the name is not provided by any import.
    /// - `Some(Ok((import_id, name)))` if exactly one module provides it.
    /// - `Some(Err(providers))` if two or more distinct modules provide it
    ///   (ambiguous; E0211 should be emitted at the use site).
    fn lookup(&self, name: Symbol) -> Option<ImportLookup<'a>> {
        let providers = self.by_name.get(&name)?;
        match providers.as_slice() {
            [] => None,
            [(_, import_id)] => Some(Ok((*import_id, name))),
            _ => Some(Err(providers.clone())),
        }
    }
}

// ---------------------------------------------------------------------------
// Resolution context
// ---------------------------------------------------------------------------

struct ResolveCtx<'a> {
    hir: &'a FileHir,
    interner: &'a Interner,
    import_index: ImportIndex<'a>,
    diags: Diagnostics,
    map: ResolveMap,
}

impl<'a> ResolveCtx<'a> {
    fn new(
        hir: &'a FileHir,
        imports: &'a [(&'a str, &'a ItemScope)],
        interner: &'a Interner,
    ) -> Self {
        let import_index = ImportIndex::build(hir, imports, interner);
        Self {
            hir,
            interner,
            import_index,
            diags: Diagnostics::new(),
            map: ResolveMap::new(),
        }
    }

    /// Resolve a name to a `Res`, checking file scope then imports.
    ///
    /// Emits E0201 (unresolved) or E0211 (ambiguous) as appropriate.
    /// Returns `Res::Error` on failure so callers can continue resolving.
    fn resolve_name(&mut self, name: Symbol, span: Span) -> Res {
        // 1. Check file-level scope first (shadows imports, ADR-0014 §3).
        if let Some(item_id) = self.hir.scope.get(name) {
            return Res::Item(item_id);
        }

        // 2. Check the import index.
        match self.import_index.lookup(name) {
            Some(Ok((import_id, sym))) => Res::Imported(import_id, sym),
            Some(Err(providers)) => {
                // Ambiguous: two or more distinct modules provide this name.
                let name_text = self.interner.resolve(name);
                let module_list: Vec<String> =
                    providers.iter().map(|(m, _)| format!("`{m}`")).collect();
                let modules_str = module_list.join(", ");
                let mut diag = Diagnostic::error(
                    span,
                    format!(
                        "ambiguous name `{name_text}`: provided by multiple imported modules: {modules_str}"
                    ),
                )
                .with_code(E0211);

                // Add secondary labels pointing at each #import item.
                for (mod_name, import_id) in &providers {
                    let import_item = self.hir.item(*import_id);
                    diag = diag.with_label(Label::with_message(
                        import_item.span,
                        format!("`{name_text}` also provided by `{mod_name}` here"),
                    ));
                }

                self.diags.push(diag);
                Res::Error
            }
            None => {
                // Not found anywhere.
                let name_text = self.interner.resolve(name);
                let diag = Diagnostic::error(span, format!("unresolved name `{name_text}`"))
                    .with_code(E0201);
                self.diags.push(diag);
                Res::Error
            }
        }
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
            // **An operator overload is exempt**, because one operator legitimately has many
            // overloads: `operator * :: (Vec2, s64)` and `operator * :: (s64, Vec2)` are two
            // declarations that must coexist, and both intern to the synthetic name `operator*`
            // (ADR-0048 §1).
            //
            // Their real key is `(operator, lhs, rhs)`, and a *genuine* duplicate — the same
            // operator on the same operand pair — is reported by `jr-sema` where that key exists.
            // This scan is about names a user wrote, and nobody wrote `operator*`.
            if matches!(
                item.kind,
                ItemKind::Const {
                    value: ConstValue::Operator(_, _)
                }
            ) {
                continue;
            }
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
                    let (name, span) = (*name, *span);
                    let resolved = self.resolve_name(name, span);
                    self.map.insert(ExprScope::TopLevel, id, resolved);
                } else {
                    self.map.insert(ExprScope::TopLevel, id, *res);
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
            // Both sides are ordinary expressions: `a[i]` resolves `a` and `i` the same way
            // any other operand is resolved. There is no third thing to look up — an index
            // is not a name in a scope the way a *field* is.
            Expr::Index { base, index, .. } => {
                let (base, index) = (*base, *index);
                self.resolve_top_expr(base);
                self.resolve_top_expr(index);
            }
            Expr::Slice { base, .. } => {
                let base = *base;
                self.resolve_top_expr(base);
            }
            Expr::Deref(ptr, _) => {
                let ptr = *ptr;
                self.resolve_top_expr(ptr);
            }
            // The *operand* is resolved; the target type is not. A `TypeRef::Name` is
            // resolved by `jr-sema`'s `resolve_type_name`, never by this map — which is the
            // asymmetry ADR-0031 §2 had to work around for unused imports, restated here so
            // it is not mistaken for an omission.
            Expr::Cast { operand, .. } => {
                let operand = *operand;
                self.resolve_top_expr(operand);
            }
            // The operand is resolved; there is no target type to resolve, which is the whole
            // of `xx` (ADR-0046 §2).
            Expr::Autocast { operand, .. } => {
                let operand = *operand;
                self.resolve_top_expr(operand);
            }
            // A bare `.RED` names no *scope*, so this map has nothing to say about it: the
            // member is found in an enum sema picks from the context type (ADR-0046 §3). Left
            // unresolved deliberately rather than resolved to `Res::Error`, which would read as
            // a failed lookup.
            Expr::Member { .. } => {}
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
            // A `for`'s iterable is resolved; its loop *variables* are locals that lowering
            // already bound, and its label names a loop rather than a value (ADR-0049 §2), so
            // there is nothing here for this map to say about either.
            Stmt::For { iterable, body, .. } => {
                match iterable {
                    ForIterable::Sequence(e) => self.resolve_body_expr(body_id, e),
                    ForIterable::Range { start, end } => {
                        self.resolve_body_expr(body_id, start);
                        self.resolve_body_expr(body_id, end);
                    }
                }
                self.resolve_body_stmt(body_id, body);
            }
            // The deferred statement is resolved once, where it was written — `jr-mir` duplicates
            // its *lowering*, not its identity (ADR-0049 §3).
            Stmt::Defer(inner, _) => self.resolve_body_stmt(body_id, inner),
            Stmt::Break(_, _) | Stmt::Continue(_, _) | Stmt::Error(_) => {}
        }
    }

    fn resolve_body_expr(&mut self, body_id: BodyId, expr_id: ExprId) {
        let expr = self.hir.bodies[body_id.index()].exprs[expr_id.index()].clone();
        match expr {
            Expr::Name { name, span, res } => {
                // If already resolved to a local/param during lowering, keep it.
                // Otherwise try file-level and import resolution.
                let final_res = if !matches!(res, Res::Error) {
                    res
                } else {
                    self.resolve_name(name, span)
                };
                self.map
                    .insert(ExprScope::Body(body_id), expr_id, final_res);
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
            Expr::Index { base, index, .. } => {
                self.resolve_body_expr(body_id, base);
                self.resolve_body_expr(body_id, index);
            }
            Expr::Slice { base, .. } => {
                self.resolve_body_expr(body_id, base);
            }
            Expr::Deref(ptr, _) => {
                self.resolve_body_expr(body_id, ptr);
            }
            Expr::Cast { operand, .. } => {
                self.resolve_body_expr(body_id, operand);
            }
            Expr::Autocast { operand, .. } => {
                self.resolve_body_expr(body_id, operand);
            }
            Expr::Member { .. } => {}
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
/// have been imported via `#import`. The module name must match the string
/// in the `#import` directive (e.g. `"Colors"` for `#import "Colors";`).
/// Pass an empty slice if no imports have been resolved yet; unresolved
/// names will be reported as E0201 errors.
///
/// ## Lookup order (ADR-0014 §3, spec §03)
///
/// 1. Block locals (already resolved during lowering)
/// 2. Parameters (already resolved during lowering)
/// 3. File-scope items (silently shadow imported names of the same name)
/// 4. Imported scopes (flat merge, ADR-0014 §2)
///
/// ## Duplicate imports (ADR-0014 §6)
///
/// Importing the same module twice is idempotent. Entries in `imports` with
/// the same module name are deduplicated before the ambiguity check.
///
/// ## Ambiguity (ADR-0014 §3)
///
/// If two or more **distinct** modules provide the same name and that name
/// is used, E0211 is emitted at the use site. Importing two overlapping
/// modules is harmless if the ambiguous name is never referenced.
///
/// ## Cycles (ADR-0014 §4)
///
/// Cycles are legal. Since this function receives already-built scopes
/// rather than loading modules itself, there is no recursion and cycles
/// are naturally handled.
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
