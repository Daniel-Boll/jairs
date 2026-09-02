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

/// Each variadic call and the packing information MIR needs (ADR-0139 §2). Same shape as sema's
/// own `variadic_calls` field, re-exported through the query layer.
pub type VariadicCalls =
    rustc_hash::FxHashMap<(jr_hir::ExprScope, jr_hir::ExprId), jr_sema::VariadicCall>;

/// Each `#soa` field access, keyed on the index expression that is its receiver (ADR-0147 §2).
///
/// The value is the field's position, which is what `jr-mir` needs to build `Field(n)` then
/// `Index(i)` — the place order the HIR nests the other way round.
pub type SoaFields = rustc_hash::FxHashMap<(jr_hir::ExprScope, jr_hir::ExprId), u32>;

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
    /// Which atomic operation each `atomic_*` call performs, as a wire code (ADR-0176 §3).
    ///
    /// Rides beside `pointer_views` for the same reason: an intrinsic's callee resolves to nothing, so MIR
    /// cannot recognise the call and would otherwise have to compare interned names against a second copy
    /// of `resolve.rs`'s list.
    pub atomics: Arc<rustc_hash::FxHashMap<(jr_hir::ExprScope, jr_hir::ExprId), u8>>,
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
    /// Each variadic call's packing info (ADR-0139 §2), read by `jr-db`'s `mir` query and
    /// threaded into `ConstValues::set_variadic_call` so `call_rvalue` knows which trailing
    /// arguments to pack into a stack view. Empty for programs with no variadic calls.
    pub variadic_calls: Arc<VariadicCalls>,
    /// Each `#soa` field access, keyed on the index expression that is its receiver, holding the
    /// field's position (ADR-0147 §2). Read by the `mir` query and threaded into
    /// `ConstValues::set_soa_field`, so lowering builds the place in the order sema decided rather
    /// than recognising the pattern a second time. Empty for programs with no `#soa` accesses.
    pub soa_fields: Arc<SoaFields>,
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
    /// Each appended clone's body scope, paired with the template body it was cloned from (ADR-0120 §5).
    ///
    /// A clone copies its template's body arena wholesale, so every `ExprId` is shared and only the scope
    /// differs — which lets `file_mir` carry the template's `#run`, `typed`/`untyped` and `any_of` values
    /// across to the clone. Without it those calls had no value under the clone's scope and `scan` refused
    /// the body.
    pub body_scopes: Vec<(jr_hir::ExprScope, jr_hir::ExprScope)>,
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
    let values = crate::consts::file_consts(db, file, search_paths).values;
    instantiated_from(
        db,
        file,
        search_paths,
        file_hir(db, file),
        &base_check,
        Some(values),
    )
}

/// How many expansion rounds to attempt before refusing.
///
/// A bound rather than "until stable" for the reason [`crate::consts`]'s round limit is one: a bug in the
/// progress check should be a diagnosable stop rather than a hang. Eight is far past anything a written
/// program reaches, because a round only happens when a *new* structural key appeared.
const MAX_INSTANTIATION_ROUNDS: usize = 8;

/// Instantiation did not reach a fixed point (ADR-0120 §4).
///
/// Owned by this crate beside E0230 and E0271, because convergence is a property of the expansion loop,
/// which lives here. The alternative to refusing is lowering a call whose target was never appended —
/// which is exactly the `no routine for file N proc M` this ADR exists to remove, so a stop that names
/// itself is strictly better than running out of rounds quietly.
const E0280: &str = "E0280";

/// A polymorphic call site.
type CallSite = (jr_hir::ExprScope, jr_hir::ExprId);

/// The structural key an instantiation dedupes on: the template plus its bound types, or its baked
/// values (ADR-0005, ADR-0088 §3).
type CallKey = (jr_hir::ProcId, Vec<jr_pool::PoolId>);

/// One expansion round's output.
struct Expansion {
    /// The starting HIR with one appended procedure per distinct key.
    hir: Arc<jr_hir::FileHir>,
    /// Name resolution over it.
    resolve: Arc<ResolveMap>,
    /// Signatures over it, the appended procedures included.
    signatures: Arc<FileSignatures>,
    /// Its check.
    check: CheckResult,
    /// The resolve's and check's diagnostics.
    diagnostics: Diagnostics,
    /// The appended procedures, `new_ids[i]` for the i-th key.
    new_ids: Vec<jr_hir::ProcId>,
    /// Each clone's body scope paired with its template's (ADR-0120 §5).
    body_scopes: Vec<(jr_hir::ExprScope, jr_hir::ExprScope)>,
    /// Where the comptime-value keys start in [`Self::new_ids`].
    comptime_start: usize,
}

/// Every `$T` call site and its key, in a **deterministic** order.
///
/// `FxHashMap` iteration is not stable and the appended `ProcId`s must be reproducible across runs, since
/// a snapshot depends on them. Sorted by call site.
fn type_call_sites(check: &CheckResult) -> Vec<(CallSite, CallKey)> {
    let mut calls: Vec<(CallSite, CallKey)> = check
        .instantiations
        .iter()
        .map(|(&call, target)| (call, target.clone()))
        .collect();
    calls.sort_by_key(|(call, _)| (scope_ord(call.0), call.1.index()));
    calls
}

/// Every `$N` call site and its key, keyed on values read from `values` (ADR-0088 §3).
///
/// An argument whose value is missing means the pre-pass refused it (E0271, a diagnostic already) — or
/// that the call site is one `values` was not computed for, which is the case for a clone's body. Either
/// way the call is skipped rather than keyed with a hole, and the caller's redirect for it is absent,
/// which `scan` then refuses.
fn comptime_call_sites(
    check: &CheckResult,
    values: &jr_mir::ConstValues,
) -> Vec<(CallSite, CallKey)> {
    let mut calls: Vec<(CallSite, CallKey)> = Vec::new();
    for (call, (template, args)) in check.comptime_calls.iter() {
        let mut resolved = Vec::with_capacity(args.len());
        let mut all_present = true;
        for arg in args {
            match values.run(call.0, *arg) {
                Some(value) => resolved.push(value),
                None => {
                    all_present = false;
                    break;
                }
            }
        }
        if all_present {
            calls.push((*call, (*template, resolved)));
        }
    }
    calls.sort_by_key(|(call, _)| (scope_ord(call.0), call.1.index()));
    calls
}

/// Expands `start_hir` from base each round with the **whole** accumulated key list, iterating until no
/// new key appears (ADR-0120 §2).
///
/// One round is not enough, and that was the defect: an instantiation's body is a **clone** with its own
/// `BodyId` and its own `ExprId`s, so a polymorphic call inside a template's body is a call site the base
/// tree's redirect map cannot name. Redirects are therefore built from the **final** check, which is the
/// only one that has seen every clone body.
///
/// Rebuilding from `start_hir` each round rather than appending incrementally keeps `new_ids[i]` paired
/// with `keys[i]`, so the appended `ProcId`s stay a function of the key list alone.
/// Builds the backtrace frame for one instantiation, or `None` when there is no site to name.
///
/// # Why the span comes from the *call*, not the template
///
/// A diagnostic inside an instantiation already points at the template's source — code the reader may
/// never have opened, and which is correct for every other instantiation of it. The one thing that
/// locates *their* mistake is the call that demanded these bindings, so that is the span a frame
/// carries (the argument ADR-0043 made for keeping a diagnostic's own span, one level out).
///
/// A missing site yields `None` rather than a frame pointing somewhere plausible. A backtrace naming the
/// wrong line is worse than no backtrace, because a reader trusts it and stops looking.
fn instantiation_site(
    hir: &jr_hir::FileHir,
    interner: &jr_base::Interner,
    sigs: &SignatureResult,
    pool: &jr_pool::Pool,
    template: jr_hir::ProcId,
    bindings: &[(jr_base::Symbol, jr_pool::PoolId)],
    site: Option<CallSite>,
) -> Option<jr_hir::InstantiationSite> {
    let (scope, expr) = site?;
    // The demanding expression's span, read from the arena the scope names. A body id that is somehow
    // absent answers `None`, which costs the backtrace rather than panicking inside a query.
    let span = match scope {
        jr_hir::ExprScope::TopLevel => hir.expr_spans.get(expr.index()).copied(),
        jr_hir::ExprScope::Body(body) => hir
            .bodies
            .get(body.index())
            .and_then(|b| b.expr_spans.get(expr.index()).copied()),
    }?;

    // A `Proc` carries no name — the name lives on the `Item` that declares it (a procedure value can
    // be anonymous), so the template's item is what to look up. An instantiation of something unnamed
    // answers `None`: with no name to print, a frame would say "in instantiation of ``".
    let name = hir.items.iter().find_map(|item| match item.kind {
        jr_hir::ItemKind::Const {
            value: jr_hir::ConstValue::Proc(p) | jr_hir::ConstValue::Operator(p, _),
        } if p == template => item.name,
        _ => None,
    })?;
    let name = interner.resolve(name);
    let description = if bindings.is_empty() {
        format!("in instantiation of `{name}`")
    } else {
        let bound = bindings
            .iter()
            .map(|(var, ty)| {
                let text = binding_type_text(sigs, pool, ty);
                format!("${} = {text}", interner.resolve(*var))
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("in instantiation of `{name}({bound})`")
    };

    Some(jr_hir::InstantiationSite {
        frame: jr_diag::InstantiationFrame::new(span, description),
        // `TopLevel` is recorded as `None`: a constant initialiser has no enclosing procedure, so the
        // chain ends there rather than looking for one.
        called_from: match scope {
            jr_hir::ExprScope::TopLevel => None,
            jr_hir::ExprScope::Body(_) => Some(scope),
        },
    })
}

/// A bound type rendered for a backtrace frame.
///
/// Prefers the file's own signatures, which know a declared type's source name, and falls back to the
/// scalar builtins. Anything else answers `?` rather than a half-built spelling like `*` — the same call
/// ADR-0075 §3 made for `Type_Info`, where a composite falls back to its kind instead of a name that
/// looks real and is not.
fn binding_type_text(sigs: &SignatureResult, pool: &jr_pool::Pool, ty: &jr_pool::PoolId) -> String {
    if let Some(name) = sigs.signatures.type_name(*ty) {
        return name.to_owned();
    }
    // The signatures know a *declared* type's source name and nothing about a builtin, which has no
    // declaration — so `$T = bool` rendered as `$T = ?` until this arm existed. Exhaustive over the
    // scalars deliberately; a composite falls through to `?` rather than to a half-built spelling like
    // `*`, the same call ADR-0075 §3 made for `Type_Info`.
    match *pool.item(*ty) {
        jr_pool::Item::VoidType => "void".to_owned(),
        jr_pool::Item::BoolType => "bool".to_owned(),
        jr_pool::Item::IntType { signed, bits } => {
            format!("{}{bits}", if signed { 's' } else { 'u' })
        }
        jr_pool::Item::FloatType { bits } => format!("float{bits}"),
        jr_pool::Item::StringType => "string".to_owned(),
        _ => String::from("?"),
    }
}

pub(crate) fn instantiated_from(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
    start_hir: Arc<jr_hir::FileHir>,
    start_check: &CheckResult,
    comptime_values: Option<Arc<jr_mir::ConstValues>>,
) -> Option<Instantiated> {
    if start_check.instantiations.is_empty() && start_check.comptime_calls.is_empty() {
        return None;
    }
    let file_id = crate::queries::resolve_file_id(db, file);

    let mut keys: Vec<CallKey> = Vec::new();
    let mut comptime_keys: Vec<CallKey> = Vec::new();
    // One representative call site per **distinct** key, parallel to `keys` (ADR-0128 §2). The first
    // site to demand a key is the one recorded: a second call with the same bound types reuses the same
    // clone, so there is one body and it can carry only one backtrace. Naming the first demand is
    // deterministic, which a snapshot depends on.
    let mut key_sites: Vec<CallSite> = Vec::new();
    let mut comptime_key_sites: Vec<CallSite> = Vec::new();
    let mut expansion: Option<Expansion> = None;
    let mut converged = false;

    // Harvest from the caller's check first, then from each round's own.
    let mut harvest: CheckResult = start_check.clone();
    for _ in 0..MAX_INSTANTIATION_ROUNDS {
        let mut fresh = false;
        for (site, key) in type_call_sites(&harvest) {
            if !keys.contains(&key) {
                keys.push(key);
                key_sites.push(site);
                fresh = true;
            }
        }
        if let Some(values) = comptime_values.as_deref() {
            for (site, key) in comptime_call_sites(&harvest, values) {
                if !comptime_keys.contains(&key) {
                    comptime_keys.push(key);
                    comptime_key_sites.push(site);
                    fresh = true;
                }
            }
        }
        if !fresh {
            converged = true;
            break;
        }
        let built = expand_round(
            db,
            file,
            search_paths,
            &start_hir,
            &keys,
            &comptime_keys,
            &key_sites,
            &comptime_key_sites,
        );
        harvest = built.check.clone();
        expansion = Some(built);
    }

    let expansion = expansion?;
    let base_sigs = file_signatures(db, file, search_paths);

    // **Redirects from the final check**, which is the fix: every call site in the tree MIR will lower,
    // clone bodies included, mapped to the procedure its key was appended as.
    let mut redirects: Vec<(CallSite, jr_mir::ProcRef)> = Vec::new();
    for (call, key) in type_call_sites(&expansion.check) {
        if let Some(index) = keys.iter().position(|k| *k == key) {
            redirects.push((
                call,
                jr_mir::ProcRef::new(file_id, expansion.new_ids[index]),
            ));
        }
    }
    let mut comptime_masks: Vec<(CallSite, Vec<bool>)> = Vec::new();
    if let Some(values) = comptime_values.as_deref() {
        for (call, key) in comptime_call_sites(&expansion.check, values) {
            let Some(index) = comptime_keys.iter().position(|k| *k == key) else {
                continue;
            };
            redirects.push((
                call,
                jr_mir::ProcRef::new(file_id, expansion.new_ids[expansion.comptime_start + index]),
            ));
            // The template's `comptime_params` flags exactly, because the checker preserved source order.
            let mask = base_sigs
                .signatures
                .proc_sig(key.0)
                .map(|sig| sig.comptime_params.clone())
                .unwrap_or_default();
            comptime_masks.push((call, mask));
        }
    }

    let mut diagnostics = expansion.diagnostics;
    if !converged {
        // Every span here would be arbitrary — the family is the file's, not one call's — so the file's
        // start is used, which `jr-diag` renders as the first line rather than clamping.
        diagnostics.push(
            jr_diag::Diagnostic::error(
                jr_base::Span::from_offsets(file_id, 0, 0),
                format!(
                    "instantiation did not settle after {MAX_INSTANTIATION_ROUNDS} rounds: this file \
                     produces an unbounded family of instantiations"
                ),
            )
            .with_code(E0280)
            .with_help(
                "a polymorphic procedure instantiating itself at a new type each round cannot \
                 terminate; give the recursion a concrete type",
            ),
        );
    }

    Some(Instantiated {
        hir: expansion.hir,
        resolve: expansion.resolve,
        signatures: expansion.signatures,
        check: expansion.check,
        diagnostics,
        redirects,
        comptime_masks,
        body_scopes: expansion.body_scopes,
    })
}

/// One round: append a procedure per key and recompute resolve, signatures and the check.
fn expand_round(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
    start_hir: &jr_hir::FileHir,
    keys: &[CallKey],
    comptime_keys: &[CallKey],
    key_sites: &[CallSite],
    comptime_key_sites: &[CallSite],
) -> Expansion {
    let file_id = crate::queries::resolve_file_id(db, file);
    let interner = db.interner();
    let base_sigs_for_vars = file_signatures(db, file, search_paths);

    // Append one procedure per distinct key. Each key's bound types are paired with the template's type
    // variables — both in `poly_vars` order (ADR-0083 §1, §2), so the i-th bound type is the i-th
    // variable's. The variable names come from the base file's signatures, which is where the template's
    // `poly_vars` lives.
    let mut hir = start_hir.clone();
    // Held only while the backtrace descriptions are rendered, and dropped before
    // `expand_instantiations` takes its own lock: the pool mutex is **not reentrant**, so the two must
    // not overlap (the ordering `run_main` also observes).
    let pool_for_names = crate::sema::read_pool(db);
    let mut instantiations: Vec<jr_hir::Instantiation> = keys
        .iter()
        .enumerate()
        .map(|(n, (template, bound_types))| {
            let vars = base_sigs_for_vars
                .signatures
                .proc_sig(*template)
                .map(|sig| sig.poly_vars.clone())
                .unwrap_or_default();
            let bindings: Vec<(jr_base::Symbol, jr_pool::PoolId)> =
                vars.into_iter().zip(bound_types.iter().copied()).collect();
            let site = instantiation_site(
                start_hir,
                interner,
                &base_sigs_for_vars,
                &pool_for_names,
                *template,
                &bindings,
                key_sites.get(n).copied(),
            );
            jr_hir::Instantiation {
                template: *template,
                bindings,
                // A `$T` instantiation has no comptime-value bakings — that path is comptime-value's
                // (ADR-0088 §3); this vector is empty, which the appender reads as "keep every parameter".
                comptime_values: Vec::new(),
                site,
            }
        })
        .collect();
    // Then one procedure per distinct comptime-value key (ADR-0088 §3). Each key's values are paired
    // with the template's `comptime_params` flags — a value slots into a `Some` at the parameter's
    // position, `None` at a runtime parameter's, and the appender drops the `Some` params and bakes
    // their literals.
    let comptime_start = instantiations.len();
    for (n, (template, values)) in comptime_keys.iter().enumerate() {
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
            // A `$N` instantiation has no type bindings, so its description names the baked
            // parameters' template rather than a `$T = …` list (ADR-0128 §2).
            site: instantiation_site(
                start_hir,
                interner,
                &base_sigs_for_vars,
                &pool_for_names,
                *template,
                &[],
                comptime_key_sites.get(n).copied(),
            ),
        });
    }
    drop(pool_for_names);
    let pool_for_expand = crate::sema::lock_pool(db);
    let new_ids =
        jr_hir::expand_instantiations(&mut hir, interner, &pool_for_expand, &instantiations);
    drop(pool_for_expand);

    // Pair each clone's body scope with its template's (ADR-0120 §5). Both are read *after* the append, so
    // the clone's `BodyId` exists; a template with no body (a `#foreign` one cannot be polymorphic, but the
    // field is an `Option`) contributes no pair, which reads as "nothing to carry across".
    let body_scopes: Vec<(jr_hir::ExprScope, jr_hir::ExprScope)> = instantiations
        .iter()
        .zip(&new_ids)
        .filter_map(|(inst, &new_id)| {
            let from = hir.proc(inst.template).body?;
            let to = hir.proc(new_id).body?;
            Some((jr_hir::ExprScope::Body(from), jr_hir::ExprScope::Body(to)))
        })
        .collect();
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

    Expansion {
        hir,
        resolve: resolve_map,
        signatures,
        check,
        diagnostics,
        new_ids,
        comptime_start,
        body_scopes,
    }
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
        atomics: Arc::new(output.atomics),
        folded_calls: Arc::new(output.folded_calls),
        folded_call_spans: Arc::new(output.folded_call_spans),
        type_info_calls: Arc::new(output.type_info_calls),
        any_calls: Arc::new(output.any_calls),
        instantiations: Arc::new(output.instantiations),
        comptime_calls: Arc::new(output.comptime_calls),
        variadic_calls: Arc::new(output.variadic_calls),
        soa_fields: Arc::new(output.soa_fields),
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
/// An **exclusive** handle on the pool, with a poisoned lock recovered.
///
/// # Why this is the write half of a `RwLock` rather than a `Mutex`
///
/// The pool is append-only and idempotent, so *reading* it needs no exclusion at all (ADR-0149 §1).
/// Splitting the two makes which sites intern a fact the type system carries: `let pool` versus
/// `let mut pool` already told a reader, and now it tells the compiler.
///
/// **This did not make anything measurably faster**, and that is recorded rather than glossed:
/// `jr check`'s parallel speedup is the same before and after, because check's pool use is dominated
/// by interning — a write. What it bought is the eight hand-rolled
/// `pool().lock().unwrap_or_else(|e| e.into_inner())` sites in `jr-lsp` collapsing into
/// [`Db::read_pool`], which is the duplication `run.rs`'s module docs already warned about.
///
/// # The obligation
///
/// A `std::sync::RwLock` is **not reentrant and not upgradable**: a thread holding either guard that
/// asks for another deadlocks. That is the same rule the `Mutex` carried — never hold a pool guard
/// across a nested query call — and `run.rs`'s comment records the hang that taught it.
pub(crate) fn lock_pool(db: &dyn Db) -> std::sync::RwLockWriteGuard<'_, Pool> {
    match db.pool().write() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// A **shared** handle on the pool, for a site that only reads it (ADR-0149 §1).
///
/// Six sites inside this crate take this rather than [`lock_pool`], and Rust identified all six: they
/// were already spelled `let pool` rather than `let mut pool`, so the read/write split was a fact the
/// code stated and the type did not.
pub(crate) fn read_pool(db: &dyn Db) -> std::sync::RwLockReadGuard<'_, Pool> {
    db.read_pool()
}
