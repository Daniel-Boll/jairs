//! The analysis context shared by both phases.
//!
//! # One context, two phases
//!
//! [`file_signatures`](crate::file_signatures) and [`check_file`](crate::check_file)
//! are the same machine in two configurations, not two implementations. They
//! share expression typing, type resolution, and diagnostic wording, and they
//! differ in exactly one thing: where a file-level name's type comes from. In
//! [`Mode::Signatures`] it is computed on demand, recursively, with a cycle
//! guard; in [`Mode::Check`] it is read out of the already-computed
//! [`FileSignatures`].
//!
//! The alternative — a separate, smaller typer for constant initialisers — was
//! tried on paper and rejected: two typers that agree today are two typers that
//! disagree later, and the disagreement would be silent, because a constant's
//! type is not written down anywhere a test could compare.
//!
//! # The arena trap
//!
//! Neither an [`ExprId`](jr_hir::ExprId) nor a [`TypeRefId`] is unique within a
//! file. `FileHir::exprs` and every `Body::exprs` are independent arenas that
//! start at 0, and the same is true of `FileHir::type_refs` and
//! `Body::type_refs`. Which arena an id indexes depends on *where it came from*,
//! not on the id. Every accessor here therefore takes an [`ExprScope`] saying
//! which arena is meant, and an index that falls outside that arena yields a
//! poison value rather than panicking or — worse — silently reading a different
//! node.
//!
//! `Proc::type_refs` and `Struct::type_refs` exist but are always empty:
//! `jr-hir`'s lowering puts parameter, return and field types in
//! `FileHir::type_refs`. Only a local's annotation lives in `Body::type_refs`.

use jr_base::{FileId, Interner, Span, Symbol};
use jr_diag::{Diagnostic, Diagnostics};
use jr_hir::{
    BodyId, ExprScope, FileHir, ItemId, ItemKind, LocalId, ResolveMap, StructId, TypeRef, TypeRefId,
};
use jr_pool::{DeclId, Item, Pool, PoolId};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::code::{E0211, E0212, E0213, E0214};
use crate::map::TypeMap;
use crate::sigs::{FileSignatures, SigEntry, SigKind};

// ---------------------------------------------------------------------------
// Mode
// ---------------------------------------------------------------------------

/// Which phase the context is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Computing this file's signatures. File-level names are typed on demand.
    Signatures,
    /// Checking this file's bodies. File-level names are already typed.
    Check,
}

// ---------------------------------------------------------------------------
// BodyEnv
// ---------------------------------------------------------------------------

/// The procedure context a body is being checked in.
///
/// Carries the parameter and return types by value rather than borrowing them
/// out of [`FileSignatures`], because the checker needs `&mut self` throughout
/// and a signature borrow would conflict with every diagnostic it pushes.
///
/// It exists at all because **`Body` has no back-pointer to its `Proc`**: a
/// `Res::Param(ParamId)` indexes `Proc::params`, and nothing in the body says
/// which `Proc` that is. The mapping is recovered by scanning `FileHir::procs`
/// for the proc whose `body` is this one.
pub(crate) struct BodyEnv {
    /// The body being checked.
    pub(crate) id: BodyId,
    /// The enclosing procedure's parameter types, in order.
    pub(crate) params: Vec<PoolId>,
    /// The enclosing procedure's return type.
    pub(crate) ret: PoolId,
}

// ---------------------------------------------------------------------------
// Ctx
// ---------------------------------------------------------------------------

/// The state shared by type resolution, signature computation, and checking.
pub(crate) struct Ctx<'a> {
    /// The file under analysis.
    pub(crate) hir: &'a FileHir,
    /// Its stable id, which is half of every nominal type's identity.
    pub(crate) file: FileId,
    /// Name resolution for this file.
    pub(crate) resolve: &'a ResolveMap,
    /// The shared string interner.
    pub(crate) interner: &'a Interner,
    /// The shared type and value pool.
    pub(crate) pool: &'a mut Pool,
    /// The signatures of each imported module, in source order.
    ///
    /// A `Vec` rather than a map because import order decides which module a
    /// flat-merged name comes from when reporting ambiguity, and iteration order
    /// over a hash map is not stable.
    pub(crate) imports: Vec<(&'a str, &'a FileSignatures)>,
    /// This file's signatures: under construction in [`Mode::Signatures`],
    /// already complete in [`Mode::Check`].
    pub(crate) sigs: FileSignatures,
    /// Which phase this is.
    pub(crate) mode: Mode,
    /// Diagnostics produced so far.
    pub(crate) diags: Diagnostics,
    /// Types learned so far.
    pub(crate) types: TypeMap,
    /// Items whose signature is currently being computed, innermost last.
    pub(crate) in_progress: Vec<ItemId>,
    /// Items whose signature is finished.
    pub(crate) finished: FxHashSet<ItemId>,
    /// The body being checked, if any.
    pub(crate) body: Option<BodyEnv>,
    /// The resolved type of every local seen so far.
    pub(crate) locals: FxHashMap<(BodyId, LocalId), PoolId>,
}

impl<'a> Ctx<'a> {
    /// Creates a context.
    pub(crate) fn new(
        hir: &'a FileHir,
        file: FileId,
        resolve: &'a ResolveMap,
        interner: &'a Interner,
        pool: &'a mut Pool,
        imports: Vec<(&'a str, &'a FileSignatures)>,
        mode: Mode,
    ) -> Self {
        Self {
            hir,
            file,
            resolve,
            interner,
            pool,
            imports,
            sigs: FileSignatures::new(),
            mode,
            diags: Diagnostics::new(),
            types: TypeMap::new(),
            in_progress: Vec::new(),
            finished: FxHashSet::default(),
            body: None,
            locals: FxHashMap::default(),
        }
    }

    // -----------------------------------------------------------------------
    // Arena access
    // -----------------------------------------------------------------------

    /// Returns the type reference `id` names in the arena `scope` selects.
    ///
    /// An index outside that arena yields [`TypeRef::Error`], which poisons
    /// quietly rather than reading a node from the wrong arena.
    pub(crate) fn type_ref(&self, scope: ExprScope, id: TypeRefId) -> TypeRef {
        let hir = self.hir;
        let arena = match scope {
            ExprScope::TopLevel => &hir.type_refs,
            ExprScope::Body(body) => &hir.body(body).type_refs,
        };
        arena.get(id.index()).cloned().unwrap_or(TypeRef::Error)
    }

    // -----------------------------------------------------------------------
    // Type resolution
    // -----------------------------------------------------------------------

    /// Resolves a syntactic type reference to an interned type.
    ///
    /// `span` is the span of the *declaration* the annotation belongs to, because
    /// a [`TypeRef`] carries no span of its own.
    pub(crate) fn resolve_type(&mut self, scope: ExprScope, id: TypeRefId, span: Span) -> PoolId {
        match self.type_ref(scope, id) {
            TypeRef::Error => PoolId::ERROR,
            TypeRef::Name(sym) => self.resolve_type_name(sym, span),
            TypeRef::Pointer(inner) => {
                let pointee = self.resolve_type(scope, inner, span);
                // `*<unknown>` is not a more useful type than `<unknown>`, and
                // keeping the poison flat means one comparison recognises it.
                if pointee == PoolId::ERROR {
                    PoolId::ERROR
                } else {
                    self.pool.pointer_to(pointee)
                }
            }
            TypeRef::Struct(sid) => {
                let ty = self.struct_type(sid);
                self.resolve_struct_body(sid, ty, span);
                ty
            }
        }
    }

    /// Resolves a named type: builtins, then this file, then imports.
    ///
    /// The order is the same one name resolution uses for expressions
    /// (ADR-0014 §3): a file-level declaration silently shadows an imported name.
    pub(crate) fn resolve_type_name(&mut self, sym: Symbol, span: Span) -> PoolId {
        let interner = self.interner;
        // Builtin type names are ordinary identifiers, not keywords
        // (`docs/spec/01-lexical.md`), so they are matched here by text rather
        // than recognised by the lexer.
        match interner.resolve(sym) {
            "s64" => return PoolId::S64,
            "u8" => return PoolId::U8,
            "bool" => return PoolId::BOOL,
            "string" => return PoolId::STRING,
            _ => {}
        }

        if let Some(item) = self.hir.scope.get(sym) {
            // A struct's *identity* is registered before any field type is
            // resolved (ADR-0015 §1 makes identity the declaration site, not the
            // fields), so a struct that points at itself — or at a struct that
            // points back — resolves here without re-entering signature
            // computation and tripping the constant-cycle guard.
            if let Some(entry) = self.sigs.lookup(sym) {
                if let Some(ty) = entry.type_value {
                    return ty;
                }
            }
            let Some(entry) = self.entry_for_item(item) else {
                return PoolId::ERROR;
            };
            return match entry.type_value {
                Some(ty) => ty,
                None => {
                    self.not_a_type(sym, entry.kind, span);
                    PoolId::ERROR
                }
            };
        }

        let providers: Vec<(&str, SigEntry)> = self
            .imports
            .iter()
            .filter_map(|(name, sigs)| sigs.lookup(sym).map(|entry| (*name, entry)))
            .collect();

        match providers.as_slice() {
            [] => {
                self.unknown_type(sym, span);
                PoolId::ERROR
            }
            [(_, entry)] => match entry.type_value {
                Some(ty) => ty,
                None => {
                    self.not_a_type(sym, entry.kind, span);
                    PoolId::ERROR
                }
            },
            many => {
                let name = interner.resolve(sym);
                let modules: Vec<String> = many.iter().map(|(m, _)| format!("`{m}`")).collect();
                self.diags.push(
                    Diagnostic::error(
                        span,
                        format!(
                            "ambiguous type name `{name}`: provided by multiple imported modules: {}",
                            modules.join(", ")
                        ),
                    )
                    .with_code(E0211)
                    .with_help("declare the type in this file to shadow both, or rename one"),
                );
                PoolId::ERROR
            }
        }
    }

    /// Interns the nominal type of the struct declared at `sid`.
    ///
    /// Identity is the declaration site (ADR-0015 §1), so this is safe to call
    /// before the field list is known — which is what lets a struct hold a
    /// pointer to itself.
    pub(crate) fn struct_type(&mut self, sid: StructId) -> PoolId {
        let decl = DeclId::new(self.file, sid.as_u32());
        self.pool.struct_type(decl)
    }

    /// Resolves and records the field list of the struct declared at `sid`.
    pub(crate) fn resolve_struct_body(&mut self, sid: StructId, ty: PoolId, span: Span) {
        let hir = self.hir;
        let fields = hir.struct_def(sid).fields.clone();
        let mut resolved = Vec::with_capacity(fields.len());
        for field in &fields {
            let field_ty = match field.ty {
                Some(id) => self.resolve_type(ExprScope::TopLevel, id, field.name_span),
                None => PoolId::ERROR,
            };
            resolved.push(jr_pool::Field::new(field.name, field_ty));
        }
        let decl = DeclId::new(self.file, sid.as_u32());
        self.pool.set_struct_fields(decl, resolved.clone());
        self.sigs.insert_struct_body(decl, resolved);
        // `span` is unused for well-formed input; keeping the parameter means a
        // future per-field diagnostic has somewhere to point without changing
        // every caller.
        let _ = (ty, span);
    }

    // -----------------------------------------------------------------------
    // The name environment
    // -----------------------------------------------------------------------

    /// Returns the signature of a file-level item.
    ///
    /// In [`Mode::Signatures`] this computes it on demand; in [`Mode::Check`] it
    /// is a lookup. `None` means the item has no name (a bare `#import` or
    /// `#run`) or its signature failed, and callers must poison rather than
    /// report.
    pub(crate) fn entry_for_item(&mut self, item: ItemId) -> Option<SigEntry> {
        match self.mode {
            Mode::Signatures => self.item_signature(item),
            Mode::Check => {
                let name = self.hir.item(item).name?;
                self.sigs.lookup(name)
            }
        }
    }

    /// Returns the signature of a name reached through an `#import`.
    ///
    /// `import` is the `#import` item in *this* file — that is what
    /// `Res::Imported` names — so the module has to be recovered from its path
    /// string before the name can be looked up.
    pub(crate) fn entry_for_import(&mut self, import: ItemId, sym: Symbol) -> Option<SigEntry> {
        let hir = self.hir;
        let ItemKind::Import { path, .. } = &hir.item(import).kind else {
            return None;
        };
        self.imports
            .iter()
            .find(|(name, _)| *name == path.as_str())
            .and_then(|(_, sigs)| sigs.lookup(sym))
    }

    // -----------------------------------------------------------------------
    // Type predicates and rendering
    // -----------------------------------------------------------------------

    /// Returns `(signed, bits)` if `ty` is an integer type.
    pub(crate) fn int_info(&self, ty: PoolId) -> Option<(bool, u16)> {
        match self.pool.item(ty) {
            Item::IntType { signed, bits } => Some((*signed, *bits)),
            _ => None,
        }
    }

    /// Returns the pointee if `ty` is a pointer.
    pub(crate) fn pointee(&self, ty: PoolId) -> Option<PoolId> {
        match self.pool.item(ty) {
            Item::PointerType(inner) => Some(*inner),
            _ => None,
        }
    }

    /// Renders a type the way a diagnostic should spell it.
    pub(crate) fn describe(&self, ty: PoolId) -> String {
        match self.pool.item(ty) {
            Item::VoidType => "void".to_owned(),
            Item::BoolType => "bool".to_owned(),
            Item::IntType { signed, bits } => {
                format!("{}{bits}", if *signed { 's' } else { 'u' })
            }
            Item::StringType => "string".to_owned(),
            Item::TypeType => "type".to_owned(),
            // Never spelled as a real type: poison exists so that one error does
            // not become five, and naming it would invite the user to look for a
            // type called `<unknown>`.
            Item::ErrorType => "<unknown>".to_owned(),
            Item::ForeignLibraryType => "foreign library".to_owned(),
            Item::PointerType(inner) => format!("*{}", self.describe(*inner)),
            Item::StructType { .. } => self
                .type_name(ty)
                .map_or_else(|| "struct".to_owned(), str::to_owned),
            Item::ProcType { params, ret, .. } => {
                let rendered: Vec<String> = params.iter().map(|p| self.describe(*p)).collect();
                format!("({}) -> {}", rendered.join(", "), self.describe(*ret))
            }
            // A value is never what a "type" diagnostic means to name, but
            // `describe` must be total, so fall through to the value's type.
            Item::VoidValue
            | Item::BoolValue(_)
            | Item::IntValue { .. }
            | Item::StrValue(_)
            | Item::TypeValue(_)
            | Item::ProcValue { .. }
            | Item::ForeignLibraryValue(_) => self.describe(self.pool.type_of(ty)),
        }
    }

    /// Returns the source-level name of a nominal type, if one is known.
    fn type_name(&self, ty: PoolId) -> Option<&str> {
        self.sigs
            .type_name(ty)
            .or_else(|| self.imports.iter().find_map(|(_, s)| s.type_name(ty)))
    }

    // -----------------------------------------------------------------------
    // Diagnostics
    // -----------------------------------------------------------------------

    /// Requires `actual` where `expected` was wanted, reporting a mismatch.
    ///
    /// Returns the type the caller should carry on with: the expected type on a
    /// mismatch, so that one wrong expression does not produce an error at every
    /// enclosing node.
    ///
    /// Poison propagates silently in both directions. This is not politeness —
    /// `file_diagnostics` does not gate later phases on earlier ones, so without
    /// it every parse error would arrive here as a second, invented type error.
    pub(crate) fn expect(
        &mut self,
        expected: Option<PoolId>,
        actual: PoolId,
        span: Span,
    ) -> PoolId {
        let Some(want) = expected else { return actual };
        if want == PoolId::ERROR {
            return actual;
        }
        if actual == PoolId::ERROR {
            return PoolId::ERROR;
        }
        if want == actual {
            return actual;
        }
        let (want_text, actual_text) = (self.describe(want), self.describe(actual));
        self.diags.push(
            Diagnostic::error(
                span,
                format!("mismatched types: expected `{want_text}`, found `{actual_text}`"),
            )
            .with_code(E0214),
        );
        want
    }

    /// Reports a type annotation that names nothing.
    fn unknown_type(&mut self, sym: Symbol, span: Span) {
        let interner = self.interner;
        let name = interner.resolve(sym);
        let mut diag =
            Diagnostic::error(span, format!("unknown type name `{name}`")).with_code(E0212);
        // `void` is a real type (ADR-0015 §3) that has no spelling, so the
        // obvious guess deserves the obvious answer.
        if name == "void" {
            diag = diag
                .with_note("`void` is not a type name in Jairs")
                .with_help("a procedure that returns nothing omits the `->` entirely");
        } else {
            diag = diag.with_note("the builtin types are `s64`, `u8`, `bool` and `string`");
        }
        self.diags.push(diag);
    }

    /// Reports a type annotation that names something which is not a type.
    fn not_a_type(&mut self, sym: Symbol, kind: SigKind, span: Span) {
        let interner = self.interner;
        let name = interner.resolve(sym);
        let what = match kind {
            SigKind::Const => "a constant",
            SigKind::Var => "a variable",
            SigKind::Proc => "a procedure",
            // Unreachable while `type_value` is `Some` for exactly this kind,
            // but spelled out so that adding a kind is a compile error.
            SigKind::Struct => "a type",
        };
        self.diags.push(
            Diagnostic::error(span, format!("`{name}` is {what}, not a type")).with_code(E0213),
        );
    }
}
