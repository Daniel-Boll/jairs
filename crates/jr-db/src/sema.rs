//! Semantic analysis wired into the incremental database.
//!
//! # Query dependency graph
//!
//! ```text
//! file_hir(file) ─┬─────────────────────────────► file_signatures(file) ──► checked(file)
//!                 │                                    ▲        ▲               ▲
//! resolved(file) ─┴────────────────────────────────────┘        │               │
//!                                                               │               │
//! file_hir(import) + resolved(import) ──────────────────────────┘               │
//! file_signatures(import) ──────────────────────────────────────────────────────┘
//! ```
//!
//! ## Why this stays acyclic when modules import each other
//!
//! [`file_signatures`] takes the imported files' **HIR and resolution**, never
//! their signatures. That is ADR-0016 §5, and it is the same invariant one layer
//! down that keeps `file_exports` acyclic: every edge leads downhill towards
//! `file_hir` and `parse_file`, which depend on nothing but the file's text. So
//! `Cycle_A` and `Cycle_B` importing each other produces
//! `file_signatures(A) → resolved(B) → file_exports(A) → file_hir(A)` and stops.
//!
//! [`checked`] may read `file_signatures` of an import freely, because signatures
//! never call back into a check.
//!
//! ## The pool is a side-channel, and why that is sound
//!
//! Interned types live in [`Pool`], which is reached through [`Db::pool`] rather
//! than being a salsa input or a query result. Three properties make that safe:
//! interning is **append-only** (a `PoolId` handed out in one revision stays
//! valid in every later one), **idempotent** (interning the same type twice
//! yields the same id, so a re-run of a query produces the same result), and
//! **deterministic** given the same sequence of calls. Struct field lists are the
//! one mutable entry, and they are keyed on a declaration site, so re-analysing a
//! file overwrites exactly its own entries.
//!
//! This mirrors the `SourceMap`, which is already held in a `Mutex` outside
//! salsa's tracking for the same kind of reason. The alternative — a pool per
//! file, with signatures carrying a structural description that each importer
//! re-interns — was rejected because it makes two files' types incomparable by
//! id, which is the entire point of interning.
//!
//! The lock is never held across a nested query call: every query gathers what it
//! needs from other queries first, then locks.

use std::sync::Arc;

use jr_diag::Diagnostics;
use jr_hir::ResolveMap;
use jr_pool::Pool;
use jr_sema::{FileSignatures, ImportedFile, TypeMap};

use crate::{
    Db, SourceFile,
    module_loader::{ModuleSearchPaths, file_hir, imports_of, module_file, resolved},
};

// ---------------------------------------------------------------------------
// Query outputs
// ---------------------------------------------------------------------------

/// The signature-level view of a file, as stored in the database.
#[derive(Debug, Clone)]
pub struct SignatureResult {
    /// The signatures themselves: what an importer is allowed to see.
    pub signatures: Arc<FileSignatures>,
    /// The types of the file-level expressions the signature phase typed.
    pub types: Arc<TypeMap>,
    /// Diagnostics about this file's declarations.
    pub diagnostics: Arc<Diagnostics>,
}

/// Each polymorphic call and the instantiation it needs: the template procedure and the tuple of types
/// its variables bind to, in `poly_vars` order (ADR-0082 §1, ADR-0083 §1).
pub type Instantiations = rustc_hash::FxHashMap<
    (jr_hir::ExprScope, jr_hir::ExprId),
    (jr_hir::ProcId, Vec<jr_pool::PoolId>),
>;

/// Each comptime-value call and the argument *expressions* its `$N` parameters need (ADR-0088 §1).
///
/// Recorded by the checker, evaluated by `jr-db`'s `comptime_call_values` pre-pass. The value tuple is
/// the same shape as [`Instantiations`]' with `ExprId` in place of `PoolId`, because a value is not
/// known at check time.
pub type ComptimeCalls = rustc_hash::FxHashMap<
    (jr_hir::ExprScope, jr_hir::ExprId),
    (jr_hir::ProcId, Vec<jr_hir::ExprId>),
>;

/// Each comptime-value call and the tuple of interned argument *values* — the structural key an
/// instantiation is built for (ADR-0088 §3). Same shape as [`Instantiations`] once const-eval has run.
pub type ComptimeCallValues = rustc_hash::FxHashMap<
    (jr_hir::ExprScope, jr_hir::ExprId),
    (jr_hir::ProcId, Vec<jr_pool::PoolId>),
>;

/// The result of type-checking a file.
#[derive(Debug, Clone)]
pub struct CheckResult {
    /// Every expression and local type the checker learned, including the
    /// file-level ones from the signature phase.
    pub types: Arc<TypeMap>,
    /// Diagnostics from checking bodies, `#run` items, and foreign bindings.
    pub diagnostics: Arc<Diagnostics>,
    /// Modules a type annotation inside a body named a type from.
    ///
    /// Carried through from `jr-sema` so that [`crate::unused_imports`] can see a local's
    /// annotation, which `ResolveMap` cannot (ADR-0031 §2). The signature phase's half of
    /// the same answer lives on `FileSignatures`.
    pub type_name_imports: Arc<[String]>,
    /// Which overload each operator expression resolved to (ADR-0048 §5).
    ///
    /// Carried through from `jr-sema` so that `jr-mir` can lower the call without re-running
    /// resolution — the same reason `types` is carried rather than recomputed.
    pub operator_calls: Arc<jr_mir::OperatorCalls>,
    /// The positional argument list of every call using a named argument or a default
    /// (ADR-0053 §1).
    pub filled_args: Arc<jr_mir::FilledArgs>,
    /// Each `typed`/`untyped` call and the pointer type it produces (ADR-0106 §1).
    ///
    /// Real code rather than a fold, so it rides beside `any_calls` rather than in `folded_calls`: retyping a
    /// pointer is a store-then-load through a slot, and lowering needs the target type to build the slot.
    pub pointer_views:
        Arc<rustc_hash::FxHashMap<(jr_hir::ExprScope, jr_hir::ExprId), jr_pool::PoolId>>,
    /// Calls `jr-sema` already folded to a value — `has_note`, `note_value` (ADR-0099 §2).
    ///
    /// Carried through rather than recomputed because the answer lives in the HIR's `Proc::notes`, which
    /// sema is holding when it checks the call; `file_consts` copies each entry straight into
    /// `ConstValues` through the same `set_run` channel a `#run` uses, so `jr-mir` reads it with the one
    /// mechanism it has for "this call is a constant" rather than a second one.
    pub folded_calls:
        Arc<rustc_hash::FxHashMap<(jr_hir::ExprScope, jr_hir::ExprId), jr_pool::PoolId>>,
    /// The same folded values keyed by **span** (ADR-0101 §3), so `file_mir` can find them in an *expanded*
    /// tree whose `ExprId`s were renumbered by a splice.
    pub folded_call_spans: Arc<rustc_hash::FxHashMap<jr_base::Span, jr_pool::PoolId>>,
    /// The type each `type_info(T)` call describes (ADR-0075 §2).
    ///
    /// Carried through from `jr-sema` because a *type* is not an operand — nothing in the expression
    /// tree holds a `PoolId` — so const-eval could not recover the argument by looking at the call.
    /// `file_consts` turns each entry into the `Type_Info` constant the call folds to.
    pub type_info_calls:
        Arc<rustc_hash::FxHashMap<(jr_hir::ExprScope, jr_hir::ExprId), jr_pool::PoolId>>,
    /// Which `Any` operation each `any_of`/`any_as` call is, and the type it concerns (ADR-0076).
    ///
    /// Carried through from `jr-sema` beside [`CheckResult::type_info_calls`]. Separate because these
    /// lower to real code rather than folding to a constant — `file_consts` turns each into an
    /// [`jr_mir::AnyLowering`].
    pub any_calls: Arc<
        rustc_hash::FxHashMap<
            (jr_hir::ExprScope, jr_hir::ExprId),
            (jr_sema::AnyOp, jr_pool::PoolId),
        >,
    >,
    /// Each polymorphic call and the instantiation it needs: `(proc, bound type)` (ADR-0082 §1).
    ///
    /// Read by `file_mir`'s expansion pass to append a substituted procedure per distinct key and rewrite
    /// the call to target it. Empty for a file with no polymorphic calls.
    pub instantiations: Arc<Instantiations>,
    /// Each comptime-value call and the argument expressions its `$N` parameters need (ADR-0088 §1):
    /// `(proc, [arg ExprId per comptime parameter])`.
    ///
    /// Read by `file_consts` (which evaluates each argument via a `Wanted::ComptimeArg`) and by
    /// `instantiated()` (which keys an instantiation on the tuple of resulting values and appends a clone
    /// with those values baked in). Empty for a program with no comptime-value calls.
    pub comptime_calls: Arc<ComptimeCalls>,
}

// ---------------------------------------------------------------------------
// Imported module lookup
// ---------------------------------------------------------------------------

/// Resolves a file's `#import`s to the already-loaded module files.
///
/// Missing modules are skipped silently: `resolved` already reported them as
/// E0210, and reporting them again from sema would double every message.
/// A self-import is skipped too (ADR-0014 §6).
fn imported_module_files(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
) -> Vec<(Arc<str>, SourceFile)> {
    let own_path = file.path(db);
    let mut found = Vec::new();
    for name in imports_of(db, file).iter() {
        let lookup = module_file(db, search_paths, name.clone());
        let Some(path) = lookup.found else { continue };
        let path = path.to_string_lossy();
        if path.as_ref() == own_path.as_ref() {
            continue;
        }
        if let Some(module) = db.source_file_for_path(path.as_ref()) {
            found.push((name.clone(), module));
        }
    }
    found
}

/// One imported module's inputs to the signature phase.
///
/// Exists to own the `Arc`s while the borrowed [`ImportedFile`] view over them is
/// built: `jr-sema` takes references, and the values have to outlive the call.
struct ImportedInputs {
    /// The module name as written in `#import "Name"`.
    name: Arc<str>,
    /// Its stable file id, which nominal type identity depends on.
    file: jr_base::FileId,
    /// Its HIR.
    hir: Arc<jr_hir::FileHir>,
    /// Its name resolution.
    resolve: Arc<ResolveMap>,
}

// ---------------------------------------------------------------------------
// file_signatures — tracked query
// ---------------------------------------------------------------------------

/// Computes a file's signatures: the types of everything it declares.
///
/// Depends on the imported files' HIR and resolution, never on their signatures
/// or checks (ADR-0016 §5).
///
/// Uses `no_eq` because [`Diagnostics`] is not `Eq`.
#[salsa::tracked(returns(clone), no_eq)]
pub fn file_signatures(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
) -> SignatureResult {
    let hir = file_hir(db, file);
    let file_id = crate::queries::resolve_file_id(db, file);
    let own_resolve = resolved(db, file, search_paths).map;
    let interner = db.interner();

    // Gather everything from other queries *before* locking the pool.
    let imported: Vec<ImportedInputs> = imported_module_files(db, file, search_paths)
        .into_iter()
        .map(|(name, module)| ImportedInputs {
            name,
            file: crate::queries::resolve_file_id(db, module),
            hir: file_hir(db, module),
            resolve: resolved(db, module, search_paths).map,
        })
        .collect();
    let imports: Vec<ImportedFile<'_>> = imported
        .iter()
        .map(|input| ImportedFile {
            name: input.name.as_ref(),
            file: input.file,
            hir: input.hir.as_ref(),
            resolve: input.resolve.as_ref(),
        })
        .collect();

    let mut pool = lock_pool(db);
    let output = jr_sema::file_signatures(
        hir.as_ref(),
        file_id,
        own_resolve.as_ref(),
        &imports,
        &mut pool,
        interner,
    );

    SignatureResult {
        signatures: Arc::new(output.signatures),
        types: Arc::new(output.types),
        diagnostics: Arc::new(output.diagnostics),
    }
}

// ---------------------------------------------------------------------------
// checked — tracked query
// ---------------------------------------------------------------------------

/// Type-checks a file's bodies against its own and its imports' signatures.
///
/// Uses `no_eq` because [`Diagnostics`] is not `Eq`.
#[salsa::tracked(returns(clone), no_eq)]
pub fn checked(db: &dyn Db, file: SourceFile, search_paths: ModuleSearchPaths) -> CheckResult {
    let hir = file_hir(db, file);
    let file_id = crate::queries::resolve_file_id(db, file);
    let own_resolve = resolved(db, file, search_paths).map;
    let own = file_signatures(db, file, search_paths);
    let interner = db.interner();

    let modules = imported_module_files(db, file, search_paths);
    let imported: Vec<(Arc<str>, SignatureResult)> = modules
        .into_iter()
        .map(|(name, module)| (name, file_signatures(db, module, search_paths)))
        .collect();
    let imports: Vec<(&str, &FileSignatures)> = imported
        .iter()
        .map(|(name, sigs)| (name.as_ref(), sigs.signatures.as_ref()))
        .collect();

    // **The imported HIRs the check phase needs** (ADR-0117 §1): a *parameterised* imported struct's fields are
    // resolved by the importer, under the arguments it supplies, and that needs the field `TypeRef` tree in the
    // declaring file's arena. `file_signatures` has always taken these through `ImportedFile`; this is the same
    // values, for the same reason, one phase later.
    let imported_module_hirs: Vec<(jr_base::FileId, Arc<jr_hir::FileHir>)> =
        imported_module_files(db, file, search_paths)
            .into_iter()
            .map(|(_, module)| {
                (
                    crate::queries::resolve_file_id(db, module),
                    file_hir(db, module),
                )
            })
            .collect();
    let imported_hirs: Vec<(jr_base::FileId, &jr_hir::FileHir)> = imported_module_hirs
        .iter()
        .map(|(id, hir)| (*id, hir.as_ref()))
        .collect();

    let mut pool = lock_pool(db);
    let output = jr_sema::check_file(
        hir.as_ref(),
        file_id,
        own_resolve.as_ref(),
        own.signatures.as_ref(),
        &imports,
        &imported_hirs,
        &mut pool,
        interner,
    );
    drop(pool);

    translate_check_output(output, own.types.as_ref())
}

/// Resolves and checks an **already-expanded** HIR, for `file_mir`'s computed-`#insert` branch
/// (ADR-0073 §1, step 6).
///
/// The inserted statements are nodes the ordinary [`resolved`] and [`checked`] never saw, so MIR cannot
/// be lowered from the expanded HIR with the unexpanded maps — it needs its own resolve and check over
/// *that* tree. Not a salsa query, because it is parameterised by an HIR rather than by a file: both
/// `jr_hir::resolve` and `jr_sema::check_file` take an explicit `&FileHir`, which is what makes this a
/// function call rather than a parallel query stack.
///
/// **Signatures are reused, not recomputed.** `#insert` is body-scoped (ADR-0072 §5), so expansion cannot
/// change a file's items — which is the same fact that keeps `imports_of`, `file_exports` and
/// `file_signatures` off the expanded path entirely, and with them the import cycle (ADR-0054 §3).
///
/// **Returns this pass's diagnostics**, which the caller surfaces via `MirResult`. They are the only
/// diagnostics that can exist for the expanded tree: the unexpanded resolve *withholds* unresolved-name
/// errors in a body holding a pending insert (it cannot know what the insert will declare, ADR-0073 §1),
/// so without reporting these a misspelling inside such a body would reach the user as "the compiler could
/// not lower `main`" — an internal-sounding message for an ordinary typo.
pub(crate) fn checked_expanded(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
    expanded: &jr_hir::FileHir,
) -> (Arc<jr_hir::ResolveMap>, CheckResult, Diagnostics) {
    let file_id = crate::queries::resolve_file_id(db, file);
    let own = file_signatures(db, file, search_paths);
    let interner = db.interner();

    // The same import scopes `resolved` gathers, over the expanded tree.
    let modules = imported_module_files(db, file, search_paths);
    let export_scopes: Vec<(Arc<str>, Arc<jr_hir::ItemScope>)> = modules
        .iter()
        .map(|(name, module)| {
            (
                name.clone(),
                crate::module_loader::file_exports(db, *module),
            )
        })
        .collect();
    let scope_refs: Vec<(&str, &jr_hir::ItemScope)> = export_scopes
        .iter()
        .map(|(name, scope)| (name.as_ref(), scope.as_ref()))
        .collect();
    let (resolve_map, resolve_diags) = jr_hir::resolve(expanded, &scope_refs, interner);

    let imported: Vec<(Arc<str>, SignatureResult)> = modules
        .into_iter()
        .map(|(name, module)| (name, file_signatures(db, module, search_paths)))
        .collect();
    let imports: Vec<(&str, &FileSignatures)> = imported
        .iter()
        .map(|(name, sigs)| (name.as_ref(), sigs.signatures.as_ref()))
        .collect();

    // **The imported HIRs the check phase needs** (ADR-0117 §1): a *parameterised* imported struct's fields are
    // resolved by the importer, under the arguments it supplies, and that needs the field `TypeRef` tree in the
    // declaring file's arena. `file_signatures` has always taken these through `ImportedFile`; this is the same
    // values, for the same reason, one phase later.
    let imported_module_hirs: Vec<(jr_base::FileId, Arc<jr_hir::FileHir>)> =
        imported_module_files(db, file, search_paths)
            .into_iter()
            .map(|(_, module)| {
                (
                    crate::queries::resolve_file_id(db, module),
                    file_hir(db, module),
                )
            })
            .collect();
    let imported_hirs: Vec<(jr_base::FileId, &jr_hir::FileHir)> = imported_module_hirs
        .iter()
        .map(|(id, hir)| (*id, hir.as_ref()))
        .collect();

    let mut pool = lock_pool(db);
    let output = jr_sema::check_file(
        expanded,
        file_id,
        &resolve_map,
        own.signatures.as_ref(),
        &imports,
        &imported_hirs,
        &mut pool,
        interner,
    );
    drop(pool);

    // The resolve's and the check's diagnostics together: the expanded tree is the only place either can
    // see the inserted statements, so both are the caller's to report.
    let mut diags = resolve_diags;
    let result = translate_check_output(output, own.types.as_ref());
    diags.extend(result.diagnostics.iter().cloned());

    (Arc::new(resolve_map), result, diags)
}

/// The expanded HIR for a file whose polymorphic calls need instantiating, plus its recomputed
/// resolve/check and the call→instantiation redirects (ADR-0082 §2, §3).
///
/// Unlike [`checked_expanded`] (which reuses the unexpanded signatures because `#insert` adds no items),
/// this **recomputes signatures over the expanded tree**: an instantiation *is* a new procedure, so its
/// signature does not exist in the base file's. That is the one structural difference between the two
/// expansions (ADR-0082 §3).
pub(crate) struct Instantiated {
    /// The base HIR with one appended procedure per distinct instantiation.
    pub hir: Arc<jr_hir::FileHir>,
    /// Name resolution over the expanded tree.
    pub resolve: Arc<ResolveMap>,
    /// Signatures over the expanded tree (the appended procedures included).
    pub signatures: Arc<FileSignatures>,
    /// The check of the expanded tree.
    pub check: CheckResult,
    /// The resolve's and check's diagnostics.
    pub diagnostics: Diagnostics,
    /// Each polymorphic call and the `ProcRef` of the procedure it was instantiated to.
    pub redirects: Vec<((jr_hir::ExprScope, jr_hir::ExprId), jr_mir::ProcRef)>,
    /// Per redirected **comptime-value** call, which source-order argument positions to drop at the call
    /// site (ADR-0088 §3). Empty for a `$T`-only file. The instantiation's parameter list is *shorter*
    /// than the source call's argument list — the `$N` params were baked in — so the call must pass only
    /// the non-comptime arguments.
    pub comptime_masks: Vec<((jr_hir::ExprScope, jr_hir::ExprId), Vec<bool>)>,
}

/// Builds the expanded HIR for a file's instantiations and re-checks it (ADR-0082 §2, §3).
///
/// `None` when the file has no polymorphic calls, so the caller takes the ordinary path at no cost. When
/// it does: the calls are de-duplicated by structural key (ADR-0005 — `(template, bound type)`), one
/// procedure is appended per distinct key, signatures/resolve/check are recomputed over the expanded
/// tree, and each call's redirect to its instantiation's `ProcRef` is returned for the const map.
pub(crate) fn instantiated(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
) -> Option<Instantiated> {
    let base_check = checked(db, file, search_paths);
    if base_check.instantiations.is_empty() && base_check.comptime_calls.is_empty() {
        return None;
    }
    let file_id = crate::queries::resolve_file_id(db, file);
    let interner = db.interner();

    // Every `$T` call in a **deterministic** order, because `FxHashMap` iteration is not stable and the
    // appended `ProcId`s must be reproducible across runs (a snapshot depends on it). Sorted by call
    // site.
    type Call = (
        (jr_hir::ExprScope, jr_hir::ExprId),
        (jr_hir::ProcId, Vec<jr_pool::PoolId>),
    );
    let mut calls: Vec<Call> = base_check
        .instantiations
        .iter()
        .map(|(&call, target)| (call, target.clone()))
        .collect();
    calls.sort_by_key(|(call, _)| (scope_ord(call.0), call.1.index()));

    // De-duplicate `$T` calls by the structural key (ADR-0005): the `(template, bound type)` tuple. The
    // first distinct key seen in sorted order is appended first, so `keys[i]` ↔ the i-th appended
    // procedure.
    let mut keys: Vec<(jr_hir::ProcId, Vec<jr_pool::PoolId>)> = Vec::new();
    for (_, key) in &calls {
        if !keys.contains(key) {
            keys.push(key.clone());
        }
    }

    // **Comptime-value calls** (ADR-0088 §3). For each, look up each argument's evaluated value in
    // `file_consts` — which the pre-pass populated via `Wanted::ComptimeArg` — and build a structural
    // key on those values. Any argument whose value is missing means the pre-pass refused it (E0271),
    // which is a diagnostic already; the call is skipped here so the instantiation is not built with a
    // hole. Deterministic order, same discipline as the `$T` calls above.
    type ComptimeCall = (
        (jr_hir::ExprScope, jr_hir::ExprId),
        (jr_hir::ProcId, Vec<jr_pool::PoolId>),
    );
    let const_values = crate::consts::file_consts(db, file, search_paths).values;
    let mut comptime_calls_vec: Vec<ComptimeCall> = Vec::new();
    let base_sigs_for_vars = file_signatures(db, file, search_paths);
    for (call_key, (template, args)) in base_check.comptime_calls.iter() {
        let mut values = Vec::with_capacity(args.len());
        let mut all_present = true;
        for arg in args {
            match const_values.run(call_key.0, *arg) {
                Some(v) => values.push(v),
                None => {
                    all_present = false;
                    break;
                }
            }
        }
        if all_present {
            comptime_calls_vec.push((*call_key, (*template, values)));
        }
    }
    comptime_calls_vec.sort_by_key(|(call, _)| (scope_ord(call.0), call.1.index()));

    // De-duplicate `$N` calls the same way: `(template, [value PoolIds])`.
    let mut comptime_keys: Vec<(jr_hir::ProcId, Vec<jr_pool::PoolId>)> = Vec::new();
    for (_, key) in &comptime_calls_vec {
        if !comptime_keys.contains(key) {
            comptime_keys.push(key.clone());
        }
    }

    // Append one procedure per distinct key. Each key's bound types are paired with the template's type
    // variables — both in `poly_vars` order (ADR-0083 §1, §2), so the i-th bound type is the i-th
    // variable's. The variable names come from the base file's signatures, which is where the template's
    // `poly_vars` lives.
    let mut hir = (*file_hir(db, file)).clone();
    let mut instantiations: Vec<jr_hir::Instantiation> = keys
        .iter()
        .map(|(template, bound_types)| {
            let vars = base_sigs_for_vars
                .signatures
                .proc_sig(*template)
                .map(|sig| sig.poly_vars.clone())
                .unwrap_or_default();
            let bindings = vars.into_iter().zip(bound_types.iter().copied()).collect();
            jr_hir::Instantiation {
                template: *template,
                bindings,
                // A `$T` instantiation has no comptime-value bakings — that path is comptime-value's
                // (ADR-0088 §3); this vector is empty, which the appender reads as "keep every parameter".
                comptime_values: Vec::new(),
            }
        })
        .collect();
    // Then one procedure per distinct comptime-value key (ADR-0088 §3). Each key's values are paired
    // with the template's `comptime_params` flags — a value slots into a `Some` at the parameter's
    // position, `None` at a runtime parameter's, and the appender drops the `Some` params and bakes
    // their literals.
    let comptime_start = instantiations.len();
    for (template, values) in &comptime_keys {
        let sig = base_sigs_for_vars.signatures.proc_sig(*template);
        let comptime_flags = sig.map(|s| s.comptime_params.clone()).unwrap_or_default();
        let mut value_iter = values.iter().copied();
        let comptime_values: Vec<Option<jr_pool::PoolId>> = comptime_flags
            .iter()
            .map(|&is_comptime| if is_comptime { value_iter.next() } else { None })
            .collect();
        instantiations.push(jr_hir::Instantiation {
            template: *template,
            bindings: Vec::new(),
            comptime_values,
        });
    }
    let pool_for_expand = crate::sema::lock_pool(db);
    let new_ids =
        jr_hir::expand_instantiations(&mut hir, interner, &pool_for_expand, &instantiations);
    drop(pool_for_expand);
    let hir = Arc::new(hir);

    // Recompute resolve and signatures over the expanded tree.
    let modules = imported_module_files(db, file, search_paths);
    let export_scopes: Vec<(Arc<str>, Arc<jr_hir::ItemScope>)> = modules
        .iter()
        .map(|(name, module)| {
            (
                name.clone(),
                crate::module_loader::file_exports(db, *module),
            )
        })
        .collect();
    let scope_refs: Vec<(&str, &jr_hir::ItemScope)> = export_scopes
        .iter()
        .map(|(name, scope)| (name.as_ref(), scope.as_ref()))
        .collect();
    let (resolve_map, resolve_diags) = jr_hir::resolve(hir.as_ref(), &scope_refs, interner);
    let resolve_map = Arc::new(resolve_map);

    let sig_inputs: Vec<ImportedInputs> = imported_module_files(db, file, search_paths)
        .into_iter()
        .map(|(name, module)| ImportedInputs {
            name,
            file: crate::queries::resolve_file_id(db, module),
            hir: file_hir(db, module),
            resolve: resolved(db, module, search_paths).map,
        })
        .collect();
    let sig_imports: Vec<ImportedFile<'_>> = sig_inputs
        .iter()
        .map(|input| ImportedFile {
            name: input.name.as_ref(),
            file: input.file,
            hir: input.hir.as_ref(),
            resolve: input.resolve.as_ref(),
        })
        .collect();

    let mut pool = lock_pool(db);
    let sig_output = jr_sema::file_signatures(
        hir.as_ref(),
        file_id,
        resolve_map.as_ref(),
        &sig_imports,
        &mut pool,
        interner,
    );
    let signatures = Arc::new(sig_output.signatures);

    // Check the expanded tree against the recomputed signatures.
    let check_imported: Vec<(Arc<str>, SignatureResult)> = modules
        .into_iter()
        .map(|(name, module)| (name, file_signatures(db, module, search_paths)))
        .collect();
    let check_imports: Vec<(&str, &FileSignatures)> = check_imported
        .iter()
        .map(|(name, sigs)| (name.as_ref(), sigs.signatures.as_ref()))
        .collect();
    // The imported HIRs, as the ordinary check path takes them (ADR-0117 §1). Gathered here too so that an
    // *expanded* tree resolves an imported parameterised struct exactly as the unexpanded one does — a
    // difference between the two would be a construct that worked until something in the file expanded.
    let check_imported_hirs_owned: Vec<(jr_base::FileId, Arc<jr_hir::FileHir>)> =
        imported_module_files(db, file, search_paths)
            .into_iter()
            .map(|(_, module)| {
                (
                    crate::queries::resolve_file_id(db, module),
                    file_hir(db, module),
                )
            })
            .collect();
    let check_imported_hirs: Vec<(jr_base::FileId, &jr_hir::FileHir)> = check_imported_hirs_owned
        .iter()
        .map(|(id, hir)| (*id, hir.as_ref()))
        .collect();
    let output = jr_sema::check_file(
        hir.as_ref(),
        file_id,
        resolve_map.as_ref(),
        signatures.as_ref(),
        &check_imports,
        &check_imported_hirs,
        &mut pool,
        interner,
    );
    drop(pool);

    let mut diagnostics = resolve_diags;
    diagnostics.extend(sig_output.diagnostics.iter().cloned());
    let check = translate_check_output(output, &sig_output.types);
    diagnostics.extend(check.diagnostics.iter().cloned());

    // Map each call to the `ProcRef` of the procedure appended for its key. The `$T` calls are indexed
    // into the first `keys.len()` entries of `new_ids`; the `$N` calls into the tail, offset by
    // `comptime_start`. Both share the redirects vector because MIR's `call_rvalue` reads one map.
    let mut redirects: Vec<((jr_hir::ExprScope, jr_hir::ExprId), jr_mir::ProcRef)> = calls
        .iter()
        .map(|(call, key)| {
            let index = keys
                .iter()
                .position(|k| k == key)
                .expect("key was collected above");
            (*call, jr_mir::ProcRef::new(file_id, new_ids[index]))
        })
        .collect();
    for (call, key) in &comptime_calls_vec {
        let index = comptime_keys
            .iter()
            .position(|k| k == key)
            .expect("comptime key was collected above");
        redirects.push((
            *call,
            jr_mir::ProcRef::new(file_id, new_ids[comptime_start + index]),
        ));
    }

    // For each comptime call, record its argument-drop mask — one boolean per source-order argument,
    // `true` for a `$N` argument to drop. The mask is the template's `comptime_params` flags exactly,
    // because the checker preserved source order (an ordinary call's arguments correspond 1:1 to the
    // template's parameters).
    let comptime_masks: Vec<((jr_hir::ExprScope, jr_hir::ExprId), Vec<bool>)> = comptime_calls_vec
        .iter()
        .map(|(call, (template, _))| {
            let mask = base_sigs_for_vars
                .signatures
                .proc_sig(*template)
                .map(|sig| sig.comptime_params.clone())
                .unwrap_or_default();
            (*call, mask)
        })
        .collect();

    Some(Instantiated {
        hir,
        resolve: resolve_map,
        signatures,
        check,
        diagnostics,
        redirects,
        comptime_masks,
    })
}

/// Every imported module's signatures, for a consumer that needs to look a library type up (ADR-0092 §1).
///
/// Exists so `file_mir` can fold an instantiation's `type_info(T)` with the same signature set
/// `file_consts` uses — `Type_Info` is declared in `Basic`, so the own file's signatures alone would not
/// find it, the failure ADR-0075 §2 records finding by running.
pub(crate) fn imported_signatures(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
) -> Vec<Arc<FileSignatures>> {
    imported_module_files(db, file, search_paths)
        .into_iter()
        .map(|(_, module)| file_signatures(db, module, search_paths).signatures)
        .collect()
}

/// A total order over `ExprScope` for deterministic sorting.
fn scope_ord(scope: jr_hir::ExprScope) -> (u8, u32) {
    match scope {
        jr_hir::ExprScope::TopLevel => (0, 0),
        jr_hir::ExprScope::Body(body) => (1, body.index() as u32),
    }
}

/// Turns `jr-sema`'s [`jr_sema::CheckOutput`] into this crate's [`CheckResult`].
///
/// Shared by [`checked`] and by `file_mir`'s **expanded** branch, which re-checks a file whose computed
/// `#insert`s have been expanded (ADR-0073 §1, step 6). One function rather than two copies, because the
/// translations below are the single place `jr-sema`'s vocabulary becomes `jr-mir`'s — and two copies
/// would be two chances for the expanded path to disagree with the ordinary one about what a call means.
fn translate_check_output(
    output: jr_sema::CheckOutput,
    declaration_types: &jr_sema::TypeMap,
) -> CheckResult {
    // One map for the whole file: the signature phase typed the declarations,
    // this phase typed the bodies, and neither typed the other's expressions.
    let mut types = output.types;
    types.absorb(declaration_types);

    // Translated from `jr-sema`'s `(FileId, ProcId)` pairs into `jr-mir`'s `ProcRef`, which is
    // the one place that mapping belongs: `jr-sema` must not depend on `jr-mir`, and `jr-mir`
    // must not re-resolve.
    let mut operator_calls = jr_mir::OperatorCalls::new();
    for ((scope, expr), (target_file, proc)) in &output.operator_calls {
        operator_calls.set(*scope, *expr, jr_mir::ProcRef::new(*target_file, *proc));
    }

    // Translated for the same reason `operator_calls` is: the `ArgSlot`/`FilledArg` pair keeps
    // `jr-sema` and `jr-mir` independent of each other, and this is the one place the mapping lives.
    let mut filled_args = jr_mir::FilledArgs::new();
    for ((scope, expr), slots) in &output.filled_calls {
        let translated: Vec<jr_mir::FilledArg> = slots
            .iter()
            .map(|slot| match slot {
                jr_sema::ArgSlot::Given(expr) => jr_mir::FilledArg::Expr(*expr),
                jr_sema::ArgSlot::Default(value) => jr_mir::FilledArg::Default(*value),
            })
            .collect();
        filled_args.set(*scope, *expr, translated);
    }

    CheckResult {
        types: Arc::new(types),
        diagnostics: Arc::new(output.diagnostics),
        type_name_imports: Arc::from(output.type_name_imports),
        operator_calls: Arc::new(operator_calls),
        filled_args: Arc::new(filled_args),
        pointer_views: Arc::new(output.pointer_views),
        folded_calls: Arc::new(output.folded_calls),
        folded_call_spans: Arc::new(output.folded_call_spans),
        type_info_calls: Arc::new(output.type_info_calls),
        any_calls: Arc::new(output.any_calls),
        instantiations: Arc::new(output.instantiations),
        comptime_calls: Arc::new(output.comptime_calls),
    }
}

// ---------------------------------------------------------------------------
// Pool access
// ---------------------------------------------------------------------------

/// Locks the shared pool, recovering from a poisoned lock.
///
/// A poisoned pool means another thread panicked while interning. The pool is
/// append-only and every entry is fully written before its id is handed out, so
/// the worst a panic can leave behind is a value nobody references — recovering
/// is sound, and refusing to would turn one panic into a dead database.
pub(crate) fn lock_pool(db: &dyn Db) -> std::sync::MutexGuard<'_, Pool> {
    match db.pool().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}
