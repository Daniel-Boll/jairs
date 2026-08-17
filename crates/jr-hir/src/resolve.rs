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
//! ## Lookup order (spec §03, ADR-0014 §3, ADR-0050 §3)
//!
//! Innermost first: block locals → parameters → **fields promoted by `using`** →
//! **this file's own file-scope items** → imported scopes. A file-level
//! declaration silently shadows an imported name of the same name.
//!
//! ## `using` promotion (ADR-0050)
//!
//! `using p: Point` puts `Point`'s field names in scope, resolving to
//! [`Res::Promoted`] — a *path*, `p` then the field, rather than a single id.
//! Promotion sits **below** locals and parameters deliberately: a real binding
//! always wins, silently, exactly as a file-scope item shadows an import. The
//! rejected alternative is argued at ADR-0050 §3 — a promoted field shadowing a
//! local would mean adding a field to a struct silently changes what an
//! unrelated local name means in every procedure that `using`s it.
//!
//! Two `using`s promoting one name is an error **at the use site** (E0250), not
//! at the declaration, so overlapping embeds are harmless when only the
//! qualified forms are used. That is ADR-0014 §3's ambiguity rule reused rather
//! than a second one invented.
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
//! | E0250 | A `using` on a non-struct, or a name promoted by two `using`s |
//! | E0253 | A name an imported module declares but does not export (ADR-0054 §2) |
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
/// A `using` that cannot promote, or a promoted name two `using`s both supply (ADR-0050).
///
/// Four conditions, one code, each with its own note: `using` on a type that is not a struct,
/// `using` on a *union* (refused because untagged storage makes "which field is valid"
/// unanswerable — ADR-0050 §5), a name promoted by two bindings, and a `using` whose type could
/// not be resolved at all.
const E0250: &str = "E0250";
/// A use of a name an imported module declares but does not export (ADR-0054 §2).
///
/// Distinct from E0201 deliberately: the name really is absent from the imported scope, so
/// "unresolved" would be true — and its near-name suggestion would point the reader at a spelling
/// mistake when the actual answer is that the module's author put the declaration behind
/// `#scope_module`.
const E0253: &str = "E0253";

/// Whether a name is a compiler intrinsic, which has no declaration to resolve to.
///
/// `type_info` (ADR-0075 §2), `any_of` and `any_as` (ADR-0076), `has_note` and `note_value` (ADR-0099 §1), `noted_count` and `noted_name` (ADR-0100 §1), `noted_insert` (ADR-0101 §1), `size_of`, `typed` and `untyped` (ADR-0106 §1), `view` (ADR-0109 §1).
/// Listed here rather than in `jr-sema`'s
/// `Intrinsic` because this crate cannot depend on that one; the two lists must agree, and the corpus is
/// what says they do — a name withheld here but unrecognised there is an unresolved-name error that
/// reaches MIR, which refuses the body rather than miscompiling it.
fn is_intrinsic_name(name: &str) -> bool {
    matches!(
        name,
        "type_info"
            | "any_of"
            | "any_as"
            | "has_note"
            | "note_value"
            | "noted_count"
            | "noted_name"
            | "noted_insert"
            | "size_of"
            | "typed"
            | "untyped"
            | "view"
    )
}

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
    ///
    /// Clones rather than copies: `Res` gained a `Box` when `Res::Promoted` arrived (ADR-0050 §2),
    /// so this is a clone for a promoted name and a cheap bitwise copy for every other variant.
    pub fn get(&self, scope: ExprScope, id: ExprId) -> Option<Res> {
        self.resolutions.get(&(scope, id)).cloned()
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
    /// Names an imported module declares but does not export, and which module hid each
    /// (ADR-0054 §2).
    ///
    /// Consulted only when `by_name` misses, so it costs nothing on the common path — and it is what
    /// turns "unresolved name" into "not exported by `Shapes`", which is the difference between
    /// sending a reader after a typo and telling them the truth.
    hidden: FxHashMap<Symbol, &'a str>,
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
        let mut hidden: FxHashMap<Symbol, &'a str> = FxHashMap::default();
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
            // Names the module declares behind `#scope_module` (ADR-0054 §2). Recorded so a use of
            // one is reported as "not exported" rather than as an unresolved name — and *not*
            // inserted into `by_name`, because a hidden name genuinely does not resolve.
            for &sym in &scope.hidden {
                if hir.scope.get(sym).is_some() {
                    continue;
                }
                hidden.entry(sym).or_insert(mod_name);
            }
        }

        // Suppress unused-variable warning for interner when no names exist.
        let _ = interner;

        Self { by_name, hidden }
    }

    /// The module that declares `name` behind `#scope_module`, if one does (ADR-0054 §2).
    fn hidden_by(&self, name: Symbol) -> Option<&'a str> {
        self.hidden.get(&name).copied()
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

/// One name a `using` binding puts in scope, and where it came from.
///
/// Collected per body before its statements are walked. A `Vec` rather than a map because the
/// *ambiguity* rule needs to see every provider of a name, not just the last one (ADR-0050 §3):
/// two `using`s promoting `x` is an error at the use site, and reporting it requires both bases.
#[derive(Debug, Clone)]
struct Promotion {
    /// The promoted field's name.
    name: Symbol,
    /// The binding the field is reached through — a `Res::Local` or `Res::Param`.
    base: Res,
    /// Where the `using` was written, for the ambiguity diagnostic's note.
    span: Span,
}

struct ResolveCtx<'a> {
    hir: &'a FileHir,
    interner: &'a Interner,
    import_index: ImportIndex<'a>,
    diags: Diagnostics,
    map: ResolveMap,
    /// Names promoted into the scope currently being resolved (ADR-0050 §3).
    ///
    /// A stack, pushed as a `using` binding comes into scope and truncated when its block ends —
    /// so a `using` local promotes only from its declaration onward and only within its block,
    /// which is the same order-sensitivity ordinary locals have. Resolving the whole body against
    /// one flat set was the simpler option and it was rejected: it would make a promoted name
    /// visible *above* the `using` that introduces it, which is the only place in the language a
    /// name's visibility would not be position-dependent.
    ///
    /// A parameter's promotions are pushed once for the whole body, because a parameter is in
    /// scope throughout it.
    promotions: Vec<Promotion>,

    /// Whether the body being resolved holds a **pending** computed `#insert` (ADR-0073 §1).
    ///
    /// When it does, an unresolved name is withheld rather than reported: the insert has not been
    /// expanded yet, and the statements it will contribute may well be what declares that name —
    /// `#insert CODE;` followed by `exit(n)`, where `CODE` is `"n := 41;"`, is the feature working.
    ///
    /// **Why suppression rather than resolving against the expanded tree.** Expansion needs the operand's
    /// *value*, which needs const-eval, which runs downstream of resolution — so this pass genuinely
    /// cannot see the inserted statements. Reporting E0201 here would make every working computed insert
    /// an error. The expanded pass (`file_mir`'s branch, ADR-0073 step 6) resolves the real tree and its
    /// `ResolveMap` is what MIR uses, so a name that truly does not resolve is still caught there — and a
    /// body holding a pending insert is refused by `jr-mir`'s `scan` regardless, so nothing is silently
    /// accepted.
    ///
    /// Scoped to the body, not the file: a name unresolved in a body *without* an insert is reported as
    /// always.
    body_has_pending_insert: bool,
    /// Whether the name being resolved is the argument of a `type_info` call (ADR-0075 §2).
    ///
    /// Such a name denotes a **type**, and a builtin type name resolves to no declaration at all — the
    /// builtin names are ordinary identifiers rather than keywords — so E0201 would fire for
    /// `type_info(s64)`. `jr-sema` resolves it instead, through the same `resolve_type_name` a type
    /// annotation uses, which is what makes the two agree by construction.
    ///
    /// A flag rather than a set of expression ids because it is only ever read at the moment the argument
    /// is resolved, and the nesting is one level deep: `type_info`'s argument is a name, not a call.
    in_type_info_argument: bool,
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
            promotions: Vec::new(),
            body_has_pending_insert: false,
            in_type_info_argument: false,
        }
    }

    // -------------------------------------------------------------------
    // `using` promotion (ADR-0050)
    // -------------------------------------------------------------------

    /// The fields a `using` binding of type `ty` promotes, or `None` with a diagnostic raised.
    ///
    /// Only a **struct** has fields to promote. A union is refused on its own grounds (ADR-0050 §5):
    /// it is untagged, so exactly one field holds a valid value and nothing records which — and a
    /// promoted `f` gives the reader no syntactic clue a union is involved, where an explicit `u.f`
    /// does. That is a reason rather than an absence, which is what ADR-0045 §6 left open.
    fn using_fields(&mut self, ty: Option<crate::hir::TypeRefId>, span: Span) -> Vec<Symbol> {
        let Some(ty_id) = ty else {
            // The parser refuses a `using` with no type (E0128), so reaching here means the type
            // failed to lower. Already reported; stay quiet rather than doubling up.
            return Vec::new();
        };
        let Some(name) = self.type_ref_name_in(&self.hir.type_refs, ty_id) else {
            self.diags.push(
                Diagnostic::error(span, "a `using` declaration must name a struct type")
                    .with_code(E0250)
                    .with_label(Label::with_message(span, "this is not a named type"))
                    .with_note(
                        "`using` promotes a struct's fields into scope, so it needs a type that has fields",
                    ),
            );
            return Vec::new();
        };
        self.fields_of_named_struct(name, span)
    }

    /// The field names of the struct `name` denotes, with every refusal reported (ADR-0050 §5).
    ///
    /// Shared by the parameter and local paths so the two cannot disagree about what `using` may
    /// promote — the "teach the shared layer, not each consumer" rule this project keeps relearning.
    fn fields_of_named_struct(&mut self, name: Symbol, span: Span) -> Vec<Symbol> {
        // Only this file's own structs, deliberately: promoting an *imported* struct's fields needs
        // the other file's HIR, which this pass does not have (it receives `ItemScope`s, which map
        // names to ids and carry no field lists). Recorded as owed in ADR-0050's consequences
        // rather than silently resolving to nothing.
        let Some(item_id) = self.hir.scope.get(name) else {
            let text = self.interner.resolve(name);
            // A builtin type name and an imported struct both land here, and they want different
            // advice: one can never work, the other is a gap. Telling a user that `s64` "is not
            // supported yet" would promise a future where `using n: s64` means something.
            // Spelled out rather than asked of `jr-pool`: `jr-hir` does not depend on it and
            // should not start — the layering runs the other way. The cost is that a *new*
            // builtin type name has to be added here too, which is why the list is exhaustive
            // over the tower rather than a prefix test.
            let builtin = matches!(
                text,
                "s8" | "s16"
                    | "s32"
                    | "s64"
                    | "u8"
                    | "u16"
                    | "u32"
                    | "u64"
                    | "bool"
                    | "string"
                    | "float32"
                    | "float64"
                    | "void"
            );
            let mut diag =
                Diagnostic::error(span, format!("cannot `using` `{text}`: it is not a struct"))
                    .with_code(E0250)
                    .with_label(Label::with_message(span, "not a struct"));
            diag = if builtin {
                diag.with_note("`using` promotes a struct's fields, and a builtin type has none")
            } else {
                diag.with_note(
                    "`using` on a struct imported from another module is not supported yet",
                )
            };
            self.diags.push(diag);
            return Vec::new();
        };
        match &self.hir.items[item_id.index()].kind {
            ItemKind::Const {
                value: ConstValue::Struct(struct_id),
                ..
            } => self.hir.structs[struct_id.index()]
                .fields
                .iter()
                .map(|f| f.name)
                .collect(),
            ItemKind::Const {
                value: ConstValue::Union(_),
                ..
            } => {
                let text = self.interner.resolve(name);
                self.diags.push(
                    Diagnostic::error(span, format!("cannot `using` the union `{text}`"))
                        .with_code(E0250)
                        .with_label(Label::with_message(span, "a union's fields cannot be promoted"))
                        .with_note(
                            "a union is untagged, so exactly one field holds a valid value and nothing records which",
                        )
                        .with_note("write `u.field` instead, so the reader can see a union is involved"),
                );
                Vec::new()
            }
            ItemKind::Const { .. }
            | ItemKind::Import { .. }
            | ItemKind::Var { .. }
            | ItemKind::Run { .. } => {
                let text = self.interner.resolve(name);
                self.diags.push(
                    Diagnostic::error(span, format!("cannot `using` `{text}`: it is not a struct"))
                        .with_code(E0250)
                        .with_label(Label::with_message(span, "not a struct"))
                        .with_note(
                            "`using` promotes a struct's fields, so only a struct can be used",
                        ),
                );
                Vec::new()
            }
        }
    }

    /// [`Self::using_fields`] for a **local**, whose type annotation is in the body's arena.
    ///
    /// A separate entry point rather than a flag, because the arena to read is decided by *where
    /// the id came from* and nothing in a `TypeRefId` records which — `Body::type_refs`' own doc
    /// comment says so, and both arenas start at index 0. Reading the wrong one would silently
    /// resolve to a different type's fields, which is precisely the class of silent wrong answer
    /// `AGENTS.md` names.
    fn using_fields_in_body(
        &mut self,
        body: BodyId,
        ty: Option<crate::hir::TypeRefId>,
        span: Span,
    ) -> Vec<Symbol> {
        let Some(ty_id) = ty else {
            return Vec::new();
        };
        let Some(name) = self.type_ref_name_in(&self.hir.bodies[body.index()].type_refs, ty_id)
        else {
            self.diags.push(
                Diagnostic::error(span, "a `using` declaration must name a struct type")
                    .with_code(E0250)
                    .with_label(Label::with_message(span, "this is not a named type"))
                    .with_note(
                        "`using` promotes a struct's fields into scope, so it needs a type that has fields",
                    ),
            );
            return Vec::new();
        };
        self.fields_of_named_struct(name, span)
    }

    /// The name a `TypeRef` denotes, following pointers, in an explicit arena.
    ///
    /// The arena is a parameter rather than assumed, because a local's annotation lives in the
    /// body's `type_refs` and a parameter's in the file's, and both start at index 0 — so a
    /// `TypeRefId` alone cannot say which. Passing it in makes the choice visible at each call.
    ///
    /// Pointers are followed so that `using p: *Point` promotes too — the same auto-deref `a.b`
    /// already does (ADR-0050 §1), so the two spellings agree about what a field access means.
    fn type_ref_name_in(
        &self,
        arena: &[crate::hir::TypeRef],
        mut ty: crate::hir::TypeRefId,
    ) -> Option<Symbol> {
        loop {
            match arena.get(ty.index())? {
                crate::hir::TypeRef::Name(sym) => return Some(*sym),
                crate::hir::TypeRef::Pointer(inner) => ty = *inner,
                crate::hir::TypeRef::Array { .. }
                | crate::hir::TypeRef::View { .. }
                // A `using` of a results list is unreachable — ADR-0052 §4 keeps one out of every
                // position but a return type — so this names no struct.
                | crate::hir::TypeRef::Results(_)
                // A `using fn: (s64) -> s64` is refused: a procedure-pointer type has no fields to
                // promote, so it names no struct — the same answer as a view or an array.
                | crate::hir::TypeRef::Proc { .. }
                // A `using x: $T` promotes nothing: `$T` binds a type variable, and until a call
                // instantiates it there is no concrete type with fields — so it names no struct, the
                // same answer as a view or a procedure pointer (ADR-0081 §1).
                | crate::hir::TypeRef::Poly(_)
                // A `using b: Box(s64)` promoting a parameterised struct's fields is out of this
                // sub-wave's scope (ADR-0085 §5), so an `Apply` names no promotable struct here — the
                // same answer as a `$T`.
                | crate::hir::TypeRef::Apply { .. }
                | crate::hir::TypeRef::Struct(_)
                | crate::hir::TypeRef::Union(_)
                | crate::hir::TypeRef::Variant(_)
                | crate::hir::TypeRef::Enum(_)
                | crate::hir::TypeRef::Error => return None,
            }
        }
    }

    /// Looks a name up among the promotions currently in scope.
    ///
    /// Returns the innermost single match. Two providers at any depth is E0250 **at the use site**,
    /// which is ADR-0014 §3's ambiguity rule reused verbatim: overlapping promotions are harmless
    /// as long as the ambiguous name is never referenced.
    fn lookup_promotion(&mut self, name: Symbol, span: Span) -> Option<Res> {
        let matches: Vec<&Promotion> = self
            .promotions
            .iter()
            .rev()
            .filter(|p| p.name == name)
            .collect();
        let first = matches.first()?;
        // Innermost wins over an outer one — a `using` local shadows a `using` parameter, matching
        // how locals shadow parameters. Ambiguity is only among promotions at the *same* depth,
        // which is what equal spans of origin cannot distinguish, so the rule used here is: more
        // than one provider *anywhere* in scope for a name that is actually used is ambiguous.
        if matches.len() > 1 {
            let text = self.interner.resolve(name);
            let mut diag = Diagnostic::error(
                span,
                format!("`{text}` is promoted by more than one `using`"),
            )
            .with_code(E0250)
            .with_label(Label::with_message(
                span,
                "which one is meant is not decidable",
            ));
            for p in &matches {
                diag = diag.with_label(Label::with_message(p.span, "promoted here"));
            }
            diag = diag.with_note("write the qualified form, as in `a.x`, to say which is meant");
            self.diags.push(diag);
            return Some(Res::Error);
        }
        Some(Res::Promoted {
            base: Box::new(first.base.clone()),
            field: name,
        })
    }

    /// Whether a call's callee is a compiler intrinsic (ADR-0075 §2, ADR-0076 §1).
    ///
    /// By name, and only when the name resolves to nothing — matching `jr-sema`'s recogniser, so a
    /// program declaring its own `type_info` keeps it and the name is not reserved.
    fn callee_is_intrinsic(&self, scope: ExprScope, callee: ExprId) -> bool {
        let expr = match scope {
            ExprScope::TopLevel => self.hir.exprs.get(callee.index()),
            ExprScope::Body(body) => self
                .hir
                .bodies
                .get(body.index())
                .and_then(|b| b.exprs.get(callee.index())),
        };
        let Some(Expr::Name { name, .. }) = expr else {
            return false;
        };
        is_intrinsic_name(self.interner.resolve(*name)) && self.hir.scope.get(*name).is_none()
    }

    /// Resolve a name to a `Res`, checking file scope then imports.
    ///
    /// Emits E0201 (unresolved) or E0211 (ambiguous) as appropriate.
    /// Returns `Res::Error` on failure so callers can continue resolving.
    fn resolve_name(&mut self, name: Symbol, span: Span) -> Res {
        // 1. Fields promoted by a `using` in scope (ADR-0050 §3).
        //
        // Reaching this function at all means lowering did *not* bind the name to a local or a
        // parameter, so those two have already won — which is the "a real binding always wins,
        // silently" half of §3, enforced by where this check sits rather than by a rule.
        //
        // Promotion is checked *before* file items so that a promoted field beats a same-named
        // constant: the field is the nearer scope, matching how a local beats a file item.
        if let Some(res) = self.lookup_promotion(name, span) {
            return res;
        }

        // 2. Check file-level scope (shadows imports, ADR-0014 §3).
        if let Some(item_id) = self.hir.scope.get(name) {
            return Res::Item(item_id);
        }

        // 3. Check the import index.
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
                let name_text = self.interner.resolve(name);
                // **An imported module may declare this name and not export it** (ADR-0054 §2). The
                // name genuinely does not resolve, so E0201 would be *true* — and it would offer a
                // near-name suggestion and send the reader hunting a typo that is not there. The
                // difference between "you misspelled this" and "the author hid this" is the whole
                // value of the diagnostic.
                if let Some(module) = self.import_index.hidden_by(name) {
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            format!("`{name_text}` is not exported by `{module}`"),
                        )
                        .with_code(E0253)
                        .with_note("it is declared behind `#scope_module`")
                        .with_help("remove the `#scope_module`, or move the declaration above it"),
                    );
                    return Res::Error;
                }
                // **Withheld when a pending computed `#insert` is in this body** (ADR-0073 §1): the
                // insert's statements are not lowered yet and may be exactly what declares this name, so
                // E0201 here would make every working computed insert an error. See
                // `body_has_pending_insert` for why this pass cannot simply resolve the expanded tree, and
                // for what still catches a name that genuinely does not resolve.
                if self.body_has_pending_insert {
                    return Res::Error;
                }
                // **`type_info` is a compiler intrinsic and has no declaration to find** (ADR-0075 §2),
                // so an unresolved `type_info` is not a mistake — `jr-sema` recognises it by name and
                // types the call itself. Withheld here rather than pre-declared in scope because there
                // is nothing to declare: it takes a *type*, which no signature can express.
                //
                // This resolves to `Res::Error` exactly as before, which is also what sema's recogniser
                // tests for: a program that declares its own `type_info` resolves to *that*, so the
                // name is not reserved and such a program keeps working.
                if is_intrinsic_name(self.interner.resolve(name)) {
                    return Res::Error;
                }
                // **The argument of a `type_info` call names a type, not a value** (ADR-0075 §2), and a
                // *builtin* type name resolves to no declaration at all: the builtin names are ordinary
                // identifiers rather than keywords (`docs/spec/01-lexical.md`), so `type_info(s64)`
                // reported E0201 for a name that denotes a perfectly good type. `jr-sema`'s
                // `resolve_type_name` is what turns it into one, and it needs a pool this pass has not
                // got — so the refusal is withheld here and sema decides.
                //
                // Scoped to a `type_info` argument rather than to every unresolved builtin name, so that
                // `x := s64;` elsewhere keeps its error. That is the same asymmetry ADR-0071 §3 argued
                // for the type-position allowlist: a missed legal position is a visible false error, a
                // missed illegal one is silent.
                if self.in_type_info_argument {
                    return Res::Error;
                }
                // Not found anywhere.
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

    /// The `using` parameters of the procedure owning `body`, as promotions.
    ///
    /// A `Body` does not point back to its `Proc` — the arenas are independent — so this scans the
    /// procedure arena for the one whose `body` matches. Bodies are few per file and this runs once
    /// per body, so the quadratic shape is not worth a reverse index; if it ever is, the index
    /// belongs on `FileHir` where lowering can fill it in for free.
    fn param_promotions(&mut self, body: BodyId) -> Vec<Promotion> {
        let owner = self
            .hir
            .procs
            .iter()
            .position(|p| p.body == Some(body))
            .map(|i| self.hir.procs[i].clone());
        let Some(proc) = owner else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (index, param) in proc.params.iter().enumerate() {
            if !param.using {
                continue;
            }
            let base = Res::Param(crate::hir::ParamId::from_usize(index));
            for field in self.using_fields(param.ty, param.name_span) {
                out.push(Promotion {
                    name: field,
                    base: base.clone(),
                    span: param.name_span,
                });
            }
        }
        out
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
            // **A nested-hoisted item is exempt** — ADR-0134. A hoisted `X :: <value>` from
            // inside a body sits in `items` (so it gets checked, lowered, linked like any
            // other item) but its name is *not* in `hir.scope` because scoping is via the
            // enclosing body's scope stack. Two nested items sharing a name across different
            // enclosing procs are legal by construction, and the `nested` flag on `Item` is
            // what distinguishes them from a real user-visible duplicate.
            if item.nested {
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
                    self.map.insert(ExprScope::TopLevel, id, res.clone());
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
                let intrinsic = self.callee_is_intrinsic(ExprScope::TopLevel, callee);
                self.resolve_top_expr(callee);
                let outer = self.in_type_info_argument;
                // **Sticky through a nested call** (ADR-0119 §2): `size_of(Slot(s64, s64))` has a *call* as the
                // intrinsic's argument, and its own callee is not an intrinsic — so assigning `intrinsic` here
                // would clear the flag and `s64` inside it would be an unresolved name. `outer ||` keeps it set,
                // which is right because a type argument's arguments are types all the way down.
                self.in_type_info_argument = outer || intrinsic;
                for arg in args {
                    self.resolve_top_expr(arg);
                }
                self.in_type_info_argument = outer;
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
            // `context` is a *keyword*, so there is nothing to resolve — and deliberately no
            // `ResolveMap` entry, or it would look like a name reference to anything reading that map
            // (ADR-0057 §1, the reason ADR-0049 §2 kept a loop label out too).
            Expr::Context(_) => {}
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
        // Whether *this* body holds an unexpanded computed `#insert` (ADR-0073 §1). Computed once per
        // body rather than asked per name, and scoped to the body so a name unresolved in a body without
        // an insert is reported exactly as before. Set before resolution starts, because the insert may
        // appear *after* the name that reads what it declares — `#insert CODE;` then `exit(n)` is the
        // ordinary shape, but a statement order the other way round must behave the same.
        self.body_has_pending_insert = self.hir.bodies[body_id.index()].stmts.iter().any(
            |stmt| matches!(stmt, Stmt::Insert { operand: Some(_), stmts, .. } if stmts.is_empty()),
        );
        // A parameter is in scope for the whole body, so its promotions are pushed once around it.
        // The stack is cleared rather than truncated because bodies do not nest.
        self.promotions.clear();
        self.promotions = self.param_promotions(body_id);
        self.resolve_body_stmt(body_id, root);
        self.promotions.clear();
        self.body_has_pending_insert = false;
    }

    fn resolve_body_stmt(&mut self, body_id: BodyId, stmt_id: StmtId) {
        // Clone to avoid borrow issues
        let stmt = self.hir.bodies[body_id.index()].stmts[stmt_id.index()].clone();
        match stmt {
            Stmt::Block(stmts, _) => {
                // A `using` local promotes only until its block ends, so the stack is truncated
                // back to its depth on the way out. That is what makes promotion order-sensitive
                // in the same way an ordinary local is.
                let depth = self.promotions.len();
                for sid in stmts {
                    self.resolve_body_stmt(body_id, sid);
                }
                self.promotions.truncate(depth);
            }
            Stmt::Local(local_id, _) => {
                let local = self.hir.bodies[body_id.index()].locals[local_id.index()].clone();
                if let Some(init) = local.init {
                    self.resolve_body_expr(body_id, init);
                }
                // Pushed *after* the initialiser is resolved, so `using p: Point = p;` does not
                // see its own promotions — the same reason a local is not in scope in its own
                // initialiser.
                if local.using {
                    // A local's type annotation lives in the *body's* arena, not the file's
                    // (see `Body::type_refs`), so this cannot share `using_fields` with the
                    // parameter path — which reads `FileHir::type_refs`. Two arenas that both
                    // start at 0 is exactly the hazard `Body::type_refs`' doc comment warns about.
                    let fields = self.using_fields_in_body(body_id, local.ty, local.name_span);
                    let base = Res::Local(local_id);
                    for field in fields {
                        self.promotions.push(Promotion {
                            name: field,
                            base: base.clone(),
                            span: local.name_span,
                        });
                    }
                }
            }
            // The call is resolved; the *targets* are locals lowering already bound, and a `_`
            // discard resolves to nothing at all — which is the whole point of making it a hole
            // rather than a binding (ADR-0052 §3).
            Stmt::LocalTuple { call, .. } => {
                self.resolve_body_expr(body_id, call);
            }
            // Here the targets *are* expressions — places being assigned to — so each present one
            // is resolved. A `None` is a discard and has nothing to resolve.
            Stmt::AssignTuple { targets, call, .. } => {
                for target in targets.into_iter().flatten() {
                    self.resolve_body_expr(body_id, target);
                }
                self.resolve_body_expr(body_id, call);
            }
            // Each returned expression is resolved; there is no target list here, so nothing else.
            Stmt::ReturnTuple(exprs, _) => {
                for expr in exprs {
                    self.resolve_body_expr(body_id, expr);
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
            // An `#insert`'s statements resolve **as if written here**, which is the whole model
            // (ADR-0072 §1): no scope is pushed, so a name the insert declares is visible afterwards and
            // a name from the enclosing body is visible inside. Nothing here can tell they came from a
            // string, which is the evidence lowering put them in the right place.
            Stmt::Insert { stmts, operand, .. } => {
                // A **computed** operand resolves like any expression (ADR-0073 §1), so
                // `#insert undefined;` reports an unresolved name against the operand, not a bare
                // refusal. `None` for a literal insert, whose statements are resolved below instead.
                if let Some(op) = operand {
                    self.resolve_body_expr(body_id, op);
                }
                for inner in stmts {
                    self.resolve_body_stmt(body_id, inner);
                }
            }
            // A `push_context` block resolves like any block: its `context` reads and calls bind to
            // the same names as outside the wrapper (ADR-0063). The copy that isolates them is a
            // `jr-mir` concern, invisible to resolution.
            Stmt::PushContext(inner, _) => self.resolve_body_stmt(body_id, inner),
            // A `switch`'s arms resolve like any block, and each arm's *value* like any expression —
            // cases are values, not patterns (ADR-0067 §2), so a bare `.RED` needs nothing special here:
            // `jr-sema` supplies its enum from the scrutinee's type.
            Stmt::Switch { value, arms, .. } => {
                self.resolve_body_expr(body_id, value);
                for arm in arms {
                    if let Some(case) = arm.value {
                        self.resolve_body_expr(body_id, case);
                    }
                    self.resolve_body_stmt(body_id, arm.body);
                }
            }
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
                let intrinsic = self.callee_is_intrinsic(ExprScope::Body(body_id), callee);
                self.resolve_body_expr(body_id, callee);
                // Only the *arguments* are in the intrinsic's type position; the callee is not, and it
                // is resolved above with the flag still at its outer value.
                let outer = self.in_type_info_argument;
                // **Sticky through a nested call** (ADR-0119 §2): `size_of(Slot(s64, s64))` has a *call* as the
                // intrinsic's argument, and that call's own callee is not an intrinsic — so assigning `intrinsic`
                // would clear the flag and `s64` inside it would be an unresolved name. `outer ||` keeps it set,
                // which is right because a type argument's own arguments are types all the way down.
                self.in_type_info_argument = outer || intrinsic;
                for arg in args {
                    self.resolve_body_expr(body_id, arg);
                }
                self.in_type_info_argument = outer;
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
            // Nothing to resolve; see the top-level arm above.
            Expr::Context(_) => {}
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
