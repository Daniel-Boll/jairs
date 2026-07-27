//! MIR wired into the incremental database, and the error gate ADR-0017 §4 requires.
//!
//! # Where this sits in the query graph
//!
//! ```text
//! file_hir(file) ─┬──► file_signatures(file) ──► checked(file) ──┬──► file_consts(file) ──┐
//! resolved(file) ─┘                                             │                        ├──► file_mir(file)
//! frontend_diagnostics(file) ───────────────────────────────────┴────────────────────────┘
//! imported_procs(file) ────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Why the gate is here and not in `jr-mir`
//!
//! ADR-0017 §4 refuses to lower a poisoned body, and `jr-mir` enforces that from
//! the inside for every error it can see — which means every error `jr-sema` left
//! as [`jr_pool::PoolId::ERROR`] in the `TypeMap`. Not every reported error
//! poisons a type: `x: u8 = 300;` raises E0204 and then type-checks as `u8`, so
//! the types alone cannot distinguish that body from a correct one.
//!
//! `jr-mir` is a pure function over HIR plus types and is handed no diagnostics,
//! so it cannot close that hole. This query can, because it is the one place that
//! has both. ADR-0017 §4 records this as the single respect in which the "require
//! the caller to check for errors first" option — rejected there as a *general*
//! policy, because a check every caller must remember is a check some caller will
//! forget — is still load-bearing. It is discharged once, here, rather than at
//! every consumer.
//!
//! # Why the gate is the whole file rather than one body
//!
//! ADR-0017 §3 makes a MIR body per procedure, and this query is per *file*, which
//! looks like a contradiction and is not: [`FileMir`] is a map from `ProcId` to an
//! independently lowered body, so the granularity of *lowering* is the procedure.
//! The query is per file because `jr-db`'s whole query surface is, and because
//! `ProcId` is an index into one file's HIR and so is not a salsa key on its own.
//! Splitting this into a per-body query needs an interned `(file, proc)` key, and
//! ADR-0017 §3 argues that the split worth making first is `mir_built` versus
//! `optimized_mir` — the one that keeps cross-body dependencies out of the
//! unoptimised query. Neither is needed until the inliner exists, and doing it now
//! would be a key type with no consumer.
//!
//! Uses `no_eq` for the same reason every other query here does: the result
//! carries a [`jr_diag`]-adjacent shape that is not `Eq`, and matching the
//! established pattern is worth more than a marginal invalidation win.

use std::sync::Arc;

use jr_base::{FileId, Symbol};
use jr_hir::{ConstValue, FileHir, ItemId, ItemKind, Res};
use jr_mir::{FileMir, ImportedProcs};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    Db, SourceFile,
    module_loader::{ModuleSearchPaths, file_hir, frontend_diagnostics, module_file, resolved},
    sema::checked,
};

// ---------------------------------------------------------------------------
// imported_procs — tracked query
// ---------------------------------------------------------------------------

/// Which procedure each imported name in a file refers to (ADR-0018 §5).
///
/// # Why this is a query and not something `jr-mir` works out
///
/// `jr_mir::Callee::Direct` names a `ProcRef` — a `(FileId, ProcId)` pair — and
/// producing one for `Res::Imported` needs the *other* file's declarations.
/// ADR-0016 §5 keeps one file's analysis off another file's analysis, so `jr-mir`
/// is deliberately never handed the means to look one up. This query is, because
/// `jr-db` is where cross-file lookup already lives.
///
/// # Why it depends only on the other file's HIR
///
/// It reads `file_hir` of the imported module and nothing else — not its
/// signatures, not its type check, and *not* its MIR. That is what keeps ADR-0017
/// §3's rule intact: the built-MIR query has no cross-body dependencies, so
/// editing a widely called leaf does not invalidate its callers' MIR. It is also
/// the same shape as `file_exports`, whose docs make the argument for why reading
/// only the other file's HIR is what stops an import cycle from diverging
/// (ADR-0014 §4).
///
/// A name that resolves to something other than a procedure is simply absent from
/// the result, and lowering refuses such a call. Distinguishing "not a procedure"
/// from "not found" would have no consumer.
#[salsa::tracked(returns(clone), no_eq)]
pub fn imported_procs(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
) -> Arc<ImportedProcs> {
    let hir = file_hir(db, file);
    let resolve = resolved(db, file, search_paths).map;

    // Every distinct imported name actually referred to, sorted so that the walk
    // is deterministic. The map itself is order-insensitive, but a deterministic
    // walk means a deterministic set of nested query calls.
    let mut pairs: Vec<(ItemId, Symbol)> = resolve
        .resolutions
        .values()
        .filter_map(|res| match res {
            Res::Imported(import, name) => Some((*import, *name)),
            Res::Local(_) | Res::Param(_) | Res::Item(_) | Res::Error => None,
        })
        .collect::<FxHashSet<_>>()
        .into_iter()
        .collect();
    pairs.sort_unstable();

    // One module lookup per `#import`, not per name: `modules/Basic` is consulted
    // once however many of its procedures a file calls.
    let mut modules: FxHashMap<ItemId, Option<(FileId, Arc<FileHir>)>> = FxHashMap::default();
    let mut out = ImportedProcs::new();

    for (import, name) in pairs {
        let target = modules
            .entry(import)
            .or_insert_with(|| import_target(db, &hir, search_paths, import))
            .clone();
        let Some((other_file, other_hir)) = target else {
            continue;
        };
        let Some(item) = other_hir.scope.get(name) else {
            continue;
        };
        let Some(item_data) = other_hir.items.get(item.index()) else {
            continue;
        };
        let ItemKind::Const {
            value: ConstValue::Proc(proc),
        } = &item_data.kind
        else {
            continue;
        };
        out.set_parts(import, name, other_file, *proc);
    }

    Arc::new(out)
}

/// The file an `#import` item resolves to, and its [`FileId`].
///
/// `None` when the module was not found, was not pre-loaded, or the item is not an
/// import at all. Every one of those is already reported elsewhere — E0210 by
/// `resolved` for a missing module — so this stays silent.
fn import_target(
    db: &dyn Db,
    hir: &FileHir,
    search_paths: ModuleSearchPaths,
    import: ItemId,
) -> Option<(FileId, Arc<FileHir>)> {
    let ItemKind::Import { path, .. } = &hir.items.get(import.index())?.kind else {
        return None;
    };
    let lookup = module_file(db, search_paths, Arc::from(path.as_str()));
    let found = lookup.found?;
    let module = db.source_file_for_path(found.to_string_lossy().as_ref())?;
    Some((
        crate::queries::resolve_file_id(db, module),
        file_hir(db, module),
    ))
}

// ---------------------------------------------------------------------------
// Query output
// ---------------------------------------------------------------------------

/// The MIR of one file, and whether it was produced at all.
#[derive(Debug, Clone)]
pub struct MirResult {
    /// One entry per procedure that has a body. Empty when [`Self::gated`].
    pub mir: Arc<FileMir>,
    /// Whether the file's diagnostics stopped lowering before it began.
    ///
    /// Distinguished from "the file has no procedures" deliberately: a consumer
    /// that cannot tell those apart would report an empty program as a correct
    /// one, which is the sort of silence that hides a build failure.
    pub gated: bool,
}

// ---------------------------------------------------------------------------
// mir — tracked query
// ---------------------------------------------------------------------------

/// Lowers every body in a file to typed SSA, unless the file has errors.
///
/// Uses `no_eq` to match the rest of this crate's queries.
#[salsa::tracked(returns(clone), no_eq)]
pub fn file_mir(db: &dyn Db, file: SourceFile, search_paths: ModuleSearchPaths) -> MirResult {
    // The gate first, so that a file with errors costs nothing beyond the
    // diagnostics that were going to be computed anyway.
    if frontend_diagnostics(db, file, search_paths).has_errors() {
        return MirResult {
            mir: Arc::new(FileMir::new()),
            gated: true,
        };
    }

    let hir = file_hir(db, file);
    let own_resolve = resolved(db, file, search_paths).map;
    let own = crate::sema::file_signatures(db, file, search_paths);
    let types = checked(db, file, search_paths).types;
    let imports = imported_procs(db, file, search_paths);
    let consts = crate::consts::file_consts(db, file, search_paths).values;
    let interner = db.interner();

    // Gather everything from other queries *before* locking the pool: the lock
    // must never be held across a nested query call.
    let mut pool = crate::sema::lock_pool(db);
    let lowered = jr_mir::lower_file(
        hir.as_ref(),
        own_resolve.as_ref(),
        types.as_ref(),
        own.signatures.as_ref(),
        consts.as_ref(),
        imports.as_ref(),
        interner,
        &mut pool,
    );
    drop(pool);

    MirResult {
        mir: Arc::new(lowered),
        gated: false,
    }
}

/// A textual dump of a file's MIR, for tests and for `jr` to print.
///
/// Not a tracked query: it is a rendering of one, and memoising a `String` nothing
/// compares would cost memory for no invalidation benefit.
#[must_use]
pub fn dump_mir(db: &dyn Db, file: SourceFile, search_paths: ModuleSearchPaths) -> String {
    let result = file_mir(db, file, search_paths);
    if result.gated {
        return String::from("gated: the file has errors\n");
    }
    let hir = file_hir(db, file);
    let signatures = crate::sema::file_signatures(db, file, search_paths).signatures;
    let interner = db.interner();
    let pool = crate::sema::lock_pool(db);
    jr_mir::dump_file(
        result.mir.as_ref(),
        hir.as_ref(),
        &pool,
        signatures.as_ref(),
        interner,
    )
}
