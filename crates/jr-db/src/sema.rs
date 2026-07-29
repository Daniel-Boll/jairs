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

    let mut pool = lock_pool(db);
    let output = jr_sema::check_file(
        hir.as_ref(),
        file_id,
        own_resolve.as_ref(),
        own.signatures.as_ref(),
        &imports,
        &mut pool,
        interner,
    );
    drop(pool);

    // One map for the whole file: the signature phase typed the declarations,
    // this phase typed the bodies, and neither typed the other's expressions.
    let mut types = output.types;
    types.absorb(own.types.as_ref());

    // Translated from `jr-sema`'s `(FileId, ProcId)` pairs into `jr-mir`'s `ProcRef`, which is
    // the one place that mapping belongs: `jr-sema` must not depend on `jr-mir`, and `jr-mir`
    // must not re-resolve.
    let mut operator_calls = jr_mir::OperatorCalls::new();
    for ((scope, expr), (target_file, proc)) in &output.operator_calls {
        operator_calls.set(*scope, *expr, jr_mir::ProcRef::new(*target_file, *proc));
    }

    CheckResult {
        types: Arc::new(types),
        diagnostics: Arc::new(output.diagnostics),
        type_name_imports: Arc::from(output.type_name_imports),
        operator_calls: Arc::new(operator_calls),
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
