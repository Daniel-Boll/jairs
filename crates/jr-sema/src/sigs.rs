//! Signatures: everything about a file that another file is allowed to see.
//!
//! # Why signatures are a separate thing from a check
//!
//! ADR-0016 §5 requires that typing a call into an imported module read only
//! that module's *signatures*, and that computing signatures depend only on the
//! other file's HIR — never on the other file's full type-check. Two files that
//! import each other otherwise make the query graph cyclic, and
//! `tests/corpus/imports/valid/005-import-cycle-is-legal.jr` stops terminating.
//!
//! The consequence, stated so it is not rediscovered later: a procedure's
//! signature must be typeable **from syntax alone**. That holds in Jairs-0
//! because parameter and return types are always written out
//! (`docs/spec/02-declarations.md`), and it stops holding the day return-type
//! inference is added.

use jr_base::{FileId, Symbol};
use jr_hir::{ItemId, ProcId};
use jr_pool::{DeclId, Field, Pool, PoolId};
use rustc_hash::FxHashMap;

// ---------------------------------------------------------------------------
// SigKind
// ---------------------------------------------------------------------------

/// What kind of declaration a name came from.
///
/// The HIR's `Res::Item` is unclassified — it says "a file-level item" and
/// nothing more — so the checker has to look the item's kind up anyway. Carrying
/// it on the signature entry means that lookup happens once, in the phase that
/// already has the item in hand, rather than at every use site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SigKind {
    /// `name :: value` where the value is neither a procedure nor a struct.
    Const,
    /// `name := value` or `name: T` at file scope.
    Var,
    /// `name :: (params) -> T { … }`, or a `#foreign` declaration.
    Proc,
    /// `name :: struct { … }`.
    Struct,
    /// `name :: union { … }` (ADR-0045).
    ///
    /// Distinct from [`SigKind::Struct`] for the reason [`SigKind::Enum`] is: a diagnostic
    /// calling a union a struct would be wrong in a way the reader cannot correct, and the two
    /// differ in a way that matters to anyone reading it — a union's fields overlap.
    Union,
    /// `name :: variant { … }` (ADR-0068 §1).
    ///
    /// Distinct from [`SigKind::Union`] for the same reason that one is distinct from `Struct`: a
    /// variant carries a tag, so its size and its access cost differ, and a diagnostic that called it
    /// a union would be wrong about both.
    Variant,
    /// `operator + :: (…) -> T { … }` (ADR-0048 §1).
    ///
    /// Distinct from [`SigKind::Proc`] so a diagnostic can say "`+` is an operator, not a
    /// procedure" — an overload's name is the synthetic `"operator+"`, which no user wrote and
    /// which a "cannot find procedure" message would send them looking for.
    Operator,
    /// `name :: enum { … }` (ADR-0041).
    ///
    /// Distinct from [`SigKind::Struct`] because a diagnostic that says "`Colour` is a
    /// struct, not a procedure" would be wrong in a way the reader cannot correct.
    Enum,
}

// ---------------------------------------------------------------------------
// SigEntry
// ---------------------------------------------------------------------------

/// What one exported name means.
#[derive(Debug, Clone, Copy)]
pub struct SigEntry {
    /// The type of the name *used as a value*.
    ///
    /// For a name that denotes a type this is [`PoolId::TYPE`], because a type
    /// is a value of type `type` (ADR-0012). It is never `None`: an entry that
    /// could not be typed carries [`PoolId::ERROR`], so that consumers poison
    /// instead of branching.
    pub ty: PoolId,
    /// The type this name *denotes*, when it denotes one.
    ///
    /// `Some` exactly for [`SigKind::Struct`] and [`SigKind::Enum`] — the two nominal
    /// declarations. This is the field a type annotation reads; `ty` is the field an
    /// expression reads.
    pub type_value: Option<PoolId>,
    /// Which kind of declaration this was.
    pub kind: SigKind,
    /// The declaring item, for diagnostics that want to point at it.
    pub item: ItemId,
    /// The procedure, for [`SigKind::Proc`] and [`SigKind::Operator`]; `None` otherwise.
    ///
    /// # Why this field exists
    ///
    /// It is the missing link from an **imported name** to that module's [`ProcSig`] (ADR-0188 §2).
    /// [`FileSignatures::proc_sig`] is keyed by [`ProcId`], and an importer holds the exporting module's
    /// `FileSignatures` but not its HIR — so before this field there was no way to get from
    /// `Res::Imported(import, name)` to the callee's parameter defaults, and `callee_sig` returned
    /// `None` with a comment claiming the signatures were unavailable. They were available; only the
    /// index was missing.
    ///
    /// The consequence was that **a default argument silently did not apply across a module boundary**:
    /// `Simp.set_shader_for_color()` was "takes 1 argument, but 0 were supplied" while the same call in
    /// the same file worked. Jai's API leans on defaults heavily, so this blocked matching it.
    pub proc: Option<ProcId>,
}

impl SigEntry {
    /// Returns `true` if assigning to this name is legal — that is, if it is a
    /// variable rather than a constant, a procedure, or a type.
    #[must_use]
    pub fn is_assignable(&self) -> bool {
        match self.kind {
            SigKind::Var => true,
            SigKind::Const
            | SigKind::Proc
            | SigKind::Operator
            | SigKind::Struct
            | SigKind::Union
            | SigKind::Variant
            | SigKind::Enum => false,
        }
    }
}

// ---------------------------------------------------------------------------
// ProcSig
// ---------------------------------------------------------------------------

/// A procedure's resolved signature.
///
/// Held separately from [`SigEntry`] because checking a *body* needs the
/// parameter types individually, while checking a *call* needs only the
/// interned procedure type.
#[derive(Debug, Clone)]
pub struct ProcSig {
    /// The parameter types, in order. A parameter whose type could not be
    /// resolved is [`PoolId::ERROR`].
    pub params: Vec<PoolId>,
    /// The parameter *names*, parallel to `params` (ADR-0053 §1).
    ///
    /// Here rather than on `Item::ProcType` because that is the per-**type** record and this is the
    /// per-**procedure** one: two procedures with identical parameter and return types intern to one
    /// `ProcType` and genuinely have different parameter names, so putting names in the type would
    /// either break interning or lie about one of them.
    pub names: Vec<Symbol>,
    /// The default value of each parameter, parallel to `params` (ADR-0053 §2).
    ///
    /// An interned literal, or `None` for a parameter that must be supplied. Already a `PoolId`
    /// rather than an expression, because sema interned the literal when it resolved the signature —
    /// which is what keeps const-eval out of this entirely (ADR-0018 §3).
    pub defaults: Vec<Option<PoolId>>,
    /// The return type. [`PoolId::VOID`] when the source omitted the arrow —
    /// never `None`, per ADR-0015 §3.
    pub ret: PoolId,
    /// The polymorphic type-variable names this signature introduces, in first-seen order (ADR-0081 §1).
    ///
    /// Empty for an ordinary procedure. Non-empty means the signature is a **template**: its `params`
    /// and `ret` are not concrete (a `$T` position is [`PoolId::ERROR`] until a call instantiates it), so
    /// the body is not checked against them and a call is instantiated rather than checked directly. This
    /// sub-wave (ADR-0081) *recognises* the template and refuses a call pending the instantiation
    /// sub-wave; the field is what a consumer keys that decision on.
    pub poly_vars: Vec<Symbol>,
    /// Which parameters are `$N` — comptime-value polymorphic — parallel to `params` (ADR-0087 §1).
    ///
    /// All `false` for an ordinary procedure. Any `true` makes the signature a **template** the same
    /// way a non-empty `poly_vars` does, but for a different reason: a `$N` parameter's *type* is known
    /// (so the body checks), while its *value* is not until a call fixes it — so a call is instantiated
    /// per value rather than checked directly. This sub-wave (ADR-0087) recognises the template and
    /// refuses a call (E0271) pending the instantiation half; the field is what a consumer keys that on.
    pub comptime_params: Vec<bool>,
    /// Which parameters are variadic — parallel to `params` (ADR-0138 §1).
    ///
    /// At most one entry is `true`, and only the *last* parameter may be variadic. A caller
    /// with `variadic == true` on the last parameter packs its trailing arguments into a
    /// stack view of the parameter's element type — the sig's `params` entry for a variadic
    /// slot already holds the `[]T` type, since HIR wraps the written `..T` as `View { elem
    /// = T }` at lowering.
    pub variadic_params: Vec<bool>,
    /// Whether the procedure is `#must` — a call must receive its result (ADR-0151 §1).
    ///
    /// **In the signature rather than read from the HIR at each call site**, because a call may cross
    /// a module boundary and the caller's file has no HIR for the callee's declaration. Every other
    /// per-procedure fact a call site needs — the parameter types, the defaults, the comptime mask —
    /// travels the same way for the same reason, so an imported `#must` procedure needs no plumbing
    /// of its own.
    pub must: bool,
    /// The interned procedure type.
    pub ty: PoolId,
}

impl ProcSig {
    /// Whether this signature is a **template** — polymorphic over a type or a comptime value
    /// (ADR-0081, ADR-0087) — so it has no concrete instance until a call instantiates it.
    #[must_use]
    pub fn is_template(&self) -> bool {
        !self.poly_vars.is_empty() || self.comptime_params.iter().any(|&c| c)
    }
}

// ---------------------------------------------------------------------------
// FileSignatures
// ---------------------------------------------------------------------------

/// The signature-level view of one file.
///
/// Construct with [`file_signatures`](crate::file_signatures). Read by the check
/// phase of the same file and by the check phase of every file that imports it.
#[derive(Debug, Clone, Default)]
pub struct FileSignatures {
    /// Every named file-level declaration.
    names: FxHashMap<jr_base::Symbol, SigEntry>,
    /// Each procedure's resolved signature, keyed by its HIR id.
    procs: FxHashMap<ProcId, ProcSig>,
    /// The file these signatures belong to.
    ///
    /// Needed because an imported *overload* must become a `ProcRef` at lowering time, and a
    /// `ProcId` alone indexes whichever file's arena the reader happens to hold — the same reason
    /// ADR-0018 §5 widened `Callee::Direct` to carry a `FileId`.
    ///
    /// `None` for a `FileSignatures::new()` that nothing has populated, which is what the
    /// standalone unit tests build.
    file: Option<FileId>,
    /// Operator overloads declared in this file, keyed on the operator and both operand types
    /// (ADR-0048 §4).
    ///
    /// A map rather than a scan, and keyed on the *exact* pair because resolution requires an
    /// exact match: no conversion, no promotion, no ranking. `Vec2 * float64` and
    /// `float64 * Vec2` are therefore two entries, which is the cost ADR-0048 §4 accepts to
    /// avoid becoming C++.
    ///
    /// Carried on `FileSignatures` rather than in a side table so that an overload crosses a
    /// module boundary the way every other declaration does — `record_in` is what an importer
    /// calls, and nothing new had to learn about overloads.
    operators: FxHashMap<(jr_hir::BinOp, PoolId, PoolId), ProcId>,
    /// The resolved field list of every struct declared in this file.
    ///
    /// Kept here as well as in the pool so that the pool dependency is
    /// explicit: a consumer calls [`FileSignatures::record_in`] rather than
    /// relying on some earlier phase having happened to intern them.
    struct_bodies: Vec<(DeclId, Vec<Field>)>,
    /// Display names for the nominal types declared in this file.
    ///
    /// The pool keys struct types on a [`DeclId`], which is deliberately just a
    /// file and an index — it cannot render `Rect`. Diagnostics need to, so the
    /// name is recorded on the side.
    type_names: FxHashMap<PoolId, String>,
    /// The resolved library of each `#foreign` procedure declared in this file.
    ///
    /// The [`PoolId`] names an [`jr_pool::Item::ForeignLibraryValue`]; read the string back
    /// with [`Pool::foreign_library_name`]. Absent for a procedure that is not
    /// `#foreign`, that named no library, or whose library operand did not resolve
    /// to a `#system_library` declaration — a consumer then knows the answer is
    /// unavailable rather than guessing which library was meant.
    ///
    /// ADR-0019 §4 is why this exists. `ForeignInfo::library` names the *constant*
    /// (`libc`), not the library (`"c"`), and getting from one to the other used to
    /// be done independently by this crate for E0225 and by `jr-vm` to make a call.
    /// The native back end is the third consumer, which is the trigger ADR-0018 §4
    /// set for interning the answer once. Recorded here, alongside `struct_bodies`
    /// and for the same reason: the resolution happens in the phase that already
    /// walks these declarations, and every later consumer reads it instead of
    /// repeating it.
    foreign_libraries: FxHashMap<ProcId, PoolId>,
    /// The module each *type* name in this file was resolved from, when it came
    /// from an import.
    ///
    /// ADR-0031 §2 is why this exists, and it is worth stating what goes wrong
    /// without it. `ResolveMap` covers `Expr::Name` and **only** `Expr::Name`; a
    /// type annotation is a `TypeRef::Name`, resolved by this crate's
    /// `Ctx::resolve_type_name` and recorded nowhere. So a consumer
    /// asking "is this import used" from the resolve map alone answers *no* for
    /// `#import "Shapes"` in a file whose only use of `Shapes` is `r: Rect` —
    /// which `tests/corpus/imports/valid/001-import-directory-module.jr` is.
    ///
    /// Re-deriving the answer outside this crate would mean a second copy of
    /// ADR-0014 §3's shadowing order, and a divergence would surface as a
    /// warning telling the user to delete an import their program needs.
    /// Recorded here for the same reason `foreign_libraries` is: the phase that
    /// already resolved it is the only one that knows.
    ///
    /// Keyed by module name because that is what an `#import` item carries; a
    /// name resolved to a builtin or to this file is deliberately absent rather
    /// than recorded as "not an import".
    type_name_imports: FxHashMap<jr_base::Symbol, String>,
    /// The names this file declares as `#expand` **macros** (ADR-0091 §3).
    ///
    /// Carried on the signatures because an *importer* needs it: a macro is spliced from its own file's
    /// source text, which does not cross a module boundary, so a cross-file call must be refused rather
    /// than reaching the VM as "no routine for file N proc M". An importer has this file's signatures and
    /// not its HIR, which is why the fact lives here.
    macro_names: rustc_hash::FxHashSet<Symbol>,
    /// The names this file declares as **templates** — a `$T` or `$N` procedure (ADR-0104 §2).
    ///
    /// Here for the reason `macro_names` is, one level over: cross-file *instantiation* is deferred
    /// (ADR-0082 §5), so a cross-file call must be refused rather than reaching an engine as "no routine for
    /// file N proc M" — and an importer has these signatures rather than this file's HIR. Separate from
    /// `macro_names` because the two refusals differ: a macro cannot be *spliced* across a file, a template
    /// cannot be *instantiated* across one, and a reader hitting either should be told which.
    template_names: rustc_hash::FxHashSet<Symbol>,
}

impl FileSignatures {
    /// Records that `name` is a `#expand` macro this file declares (ADR-0091 §3).
    pub fn insert_macro(&mut self, name: Symbol) {
        self.macro_names.insert(name);
    }

    /// Whether `name` is a `#expand` macro this file declares.
    #[must_use]
    pub fn is_macro(&self, name: Symbol) -> bool {
        self.macro_names.contains(&name)
    }

    /// Records that `name` is a **template** this file declares — a `$T` or `$N` procedure (ADR-0104 §2).
    ///
    /// Shaped exactly like [`Self::insert_macro`], and for the same reason: cross-file instantiation is
    /// deferred (ADR-0082 §5), so an importing file has to be able to *recognise* an imported template in
    /// order to refuse the call — and a name-keyed set is what one file has of another's declarations.
    pub fn insert_template(&mut self, name: Symbol) {
        self.template_names.insert(name);
    }

    /// Whether `name` is a template this file declares.
    #[must_use]
    pub fn is_template_name(&self, name: Symbol) -> bool {
        self.template_names.contains(&name)
    }
    /// Creates an empty set of signatures.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Looks up a name.
    #[must_use]
    pub fn lookup(&self, name: jr_base::Symbol) -> Option<SigEntry> {
        self.names.get(&name).copied()
    }

    /// Returns a procedure's resolved signature.
    #[must_use]
    pub fn proc_sig(&self, proc: ProcId) -> Option<&ProcSig> {
        self.procs.get(&proc)
    }

    /// The file these signatures were computed for, if it has been recorded.
    ///
    /// Returns file 0 when absent rather than panicking: an unpopulated
    /// `FileSignatures` declares no overloads, so nothing can ask this and get a wrong answer that
    /// matters.
    #[must_use]
    pub fn file(&self) -> FileId {
        self.file.unwrap_or_else(|| FileId::from_usize(0))
    }

    /// Records which file these signatures describe.
    pub(crate) fn set_file(&mut self, file: FileId) {
        self.file = Some(file);
    }

    /// The overload for `op` on this exact pair of operand types, if this file declares one
    /// (ADR-0048 §4).
    #[must_use]
    pub fn operator(&self, op: jr_hir::BinOp, lhs: PoolId, rhs: PoolId) -> Option<ProcId> {
        self.operators.get(&(op, lhs, rhs)).copied()
    }

    /// Records an overload, or reports the [`ProcId`] already registered for that key.
    ///
    /// `Err` is a **genuine duplicate**: the same operator on the same operand pair. That is the
    /// only real collision, and it has to be caught here because `jr-hir`'s name scan deliberately
    /// exempts overloads — one operator has many, all interning to one synthetic name, so the name
    /// map cannot tell a second overload from a redefinition (ADR-0048 §1).
    ///
    /// # Errors
    /// The previously-registered procedure, so the caller can point at both.
    pub(crate) fn insert_operator(
        &mut self,
        op: jr_hir::BinOp,
        lhs: PoolId,
        rhs: PoolId,
        proc: ProcId,
    ) -> Result<(), ProcId> {
        match self.operators.insert((op, lhs, rhs), proc) {
            None => Ok(()),
            Some(previous) => {
                // Keep the *first* declaration, matching the name map's shadowing story: a
                // later duplicate is the error, so the earlier one stays usable and only one
                // diagnostic is produced.
                self.operators.insert((op, lhs, rhs), previous);
                Err(previous)
            }
        }
    }

    /// Whether this file declares any overload at all.
    ///
    /// Lets the operator path skip the lookup entirely for the overwhelmingly common file that
    /// declares none, so builtin arithmetic pays nothing for the feature existing.
    #[must_use]
    pub fn has_operators(&self) -> bool {
        !self.operators.is_empty()
    }

    /// Returns the display name of a nominal type declared in this file.
    #[must_use]
    pub fn type_name(&self, ty: PoolId) -> Option<&str> {
        self.type_names.get(&ty).map(String::as_str)
    }

    /// Returns the interned library of a `#foreign` procedure.
    ///
    /// The result names an [`jr_pool::Item::ForeignLibraryValue`]; pass it to
    /// [`Pool::foreign_library_name`] for the string.
    ///
    /// `None` when the procedure is not `#foreign`, declared no library, or named
    /// something that is not a `#system_library` declaration — the last case being
    /// an E0225 the check phase reports. A consumer must treat `None` as *the
    /// library is unknown* and refuse, rather than defaulting to a likely one:
    /// guessing produces a link against a library the source never named.
    #[must_use]
    pub fn foreign_library(&self, proc: ProcId) -> Option<PoolId> {
        self.foreign_libraries.get(&proc).copied()
    }

    /// Every module this file resolved a *type* name from, in no particular order.    /// Every module this file resolved a *type* name from, in no particular order.
    ///
    /// The other half of what "is this import used" needs, the first half being
    /// `ResolveMap`'s `Res::Imported` — which covers `Expr::Name` and **only**
    /// `Expr::Name`, so a type annotation naming an imported struct is invisible to
    /// it. ADR-0031 §2 has the failure that motivated recording this: without it,
    /// a file whose only use of an import is `r: Rect` gets a warning telling the
    /// user to delete an import their program needs.
    pub fn modules_used_in_type_position(&self) -> impl Iterator<Item = &str> {
        self.type_name_imports.values().map(String::as_str)
    }

    /// The number of named declarations. Non-zero for any file that declares
    /// anything, which is what makes an empty result distinguishable from a
    /// failed one.
    #[must_use]
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Returns `true` if the file declares no names at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Records this file's struct bodies **and its nominal types' declared names** in `pool`.
    ///
    /// Idempotent, and safe to call for a file whose fields are already
    /// recorded: [`Pool::set_struct_fields`] replaces rather than appends.
    /// Call it for the file being checked *and* for every file it imports,
    /// before any field access is typed.
    ///
    /// # Why the names travel with the fields (ADR-0200 §1)
    ///
    /// `type_names` is keyed by [`PoolId`] and scoped to **this file**, so an importer's map has no
    /// entry for an imported struct and every renderer fell through to a debug spelling — a hover on
    /// a `Window` local read `window: structDeclId(1:1)`. The pool is the one place every file's
    /// declarations meet, and this call already runs for the file being checked *and* every file it
    /// imports, which is exactly the coverage the name needs.
    ///
    /// The [`DeclId`] is derived here rather than carried through `insert_type_name`, because
    /// [`Pool::nominal_decl`] already answers it and the five call sites recording a name hold a
    /// `PoolId`. One derivation beats five extra arguments that could each pass the wrong one.
    pub fn record_in(&self, pool: &mut Pool) {
        for (decl, fields) in &self.struct_bodies {
            pool.set_struct_fields(*decl, fields.clone());
        }
        for (ty, name) in &self.type_names {
            // `None` for a structural type. A name recorded against one would be meaningless, and
            // `nominal_decl` refuses rather than inventing a declaration.
            if let Some(decl) = pool.nominal_decl(*ty) {
                pool.set_decl_name(decl, name.clone());
            }
        }
    }

    // -----------------------------------------------------------------------
    // Construction (crate-internal)
    // -----------------------------------------------------------------------

    /// Records a named declaration.
    pub(crate) fn insert(&mut self, name: jr_base::Symbol, entry: SigEntry) {
        self.names.insert(name, entry);
    }

    /// Records a procedure's resolved signature.
    pub(crate) fn insert_proc(&mut self, proc: ProcId, sig: ProcSig) {
        self.procs.insert(proc, sig);
    }

    /// Records a struct's resolved field list.
    pub(crate) fn insert_struct_body(&mut self, decl: DeclId, fields: Vec<Field>) {
        self.struct_bodies.push((decl, fields));
    }

    /// Records the display name of a nominal type.
    pub(crate) fn insert_type_name(&mut self, ty: PoolId, name: String) {
        self.type_names.insert(ty, name);
    }

    /// Records a `#foreign` procedure's resolved library.
    pub(crate) fn insert_foreign_library(&mut self, proc: ProcId, library: PoolId) {
        self.foreign_libraries.insert(proc, library);
    }

    /// Records that a type name resolved to a declaration in an imported module.
    pub(crate) fn insert_type_name_import(&mut self, name: jr_base::Symbol, module: &str) {
        self.type_name_imports.insert(name, module.to_owned());
    }

    /// The names this file declares, for a "did you mean" suggestion.
    ///
    /// Returns symbols rather than strings because the caller has the interner and
    /// this crate's diagnostics resolve names through it anyway.
    pub(crate) fn declared_names(&self) -> impl Iterator<Item = jr_base::Symbol> {
        self.names.keys().copied()
    }
}
