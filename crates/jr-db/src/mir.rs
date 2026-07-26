//! MIR wired into the incremental database, and the error gate ADR-0017 §4 requires.
//!
//! # Where this sits in the query graph
//!
//! ```text
//! file_hir(file) ─┬──► file_signatures(file) ──► checked(file) ──┐
//! resolved(file) ─┘                                             ├──► file_mir(file)
//! frontend_diagnostics(file) ──────────────────────────────────┘
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

use jr_mir::FileMir;

use crate::{
    Db, SourceFile,
    module_loader::{ModuleSearchPaths, file_hir, frontend_diagnostics, resolved},
    sema::checked,
};

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
    let interner = db.interner();

    // Gather everything from other queries *before* locking the pool: the lock
    // must never be held across a nested query call.
    let mut pool = crate::sema::lock_pool(db);
    let lowered = jr_mir::lower_file(
        hir.as_ref(),
        own_resolve.as_ref(),
        types.as_ref(),
        own.signatures.as_ref(),
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
