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
use jr_mir::{Callees, FileMir, ImportedProcs};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    BuildConfig, Db, SourceFile,
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
            // A promoted name resolves through a local or a parameter, so it never names an
            // import — and ADR-0050 §5 leaves `using` on an *imported* struct unsupported, so
            // there is no cross-file promotion to collect here yet.
            Res::Local(_) | Res::Param(_) | Res::Item(_) | Res::Promoted { .. } | Res::Error => {
                None
            }
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
        // Whether the *callee* takes a context is decided in *its* file's HIR (ADR-0057 §3), which is
        // the only place it is available — the importing file cannot recompute it. Carried across
        // now, or a call to an imported `#foreign` procedure would be handed a context it does not
        // take.
        let callee = other_hir.procs.get(proc.index());
        let receives_context = callee.is_some_and(|p| !(p.c_call || p.foreign.is_some()));
        out.set_full(
            import,
            name,
            jr_mir::ProcRef::new(other_file, *proc),
            receives_context,
        );
    }

    Arc::new(out)
}

/// The value of every imported **constant** a file reads (ADR-0055 §1).
///
/// The parallel of [`imported_procs`], deliberately built the same way from the same
/// `(ItemId, Symbol)` pairs: ADR-0018 §5 established the "resolve across files in `jr-db`, hand
/// `jr-mir` a flat map" shape, and a second mechanism for the same job would be a second thing to
/// keep correct.
///
/// **Why this does not cycle** (ADR-0055 §3): `file_consts(B)` depends on `file_signatures(B)` and
/// `file_hir(B)` — *not* on `checked(B)`, because ADR-0018 §3 put const-eval downstream of signatures
/// — and on nothing in the importing file. So an edge from A's lowering to B's const-eval has no path
/// back, and two modules importing each other is fine for the reason ADR-0014 §4 makes cycles legal.
///
/// The same `search_paths` are passed through, so a module's constants cannot depend on who imported
/// it — the action at a distance ADR-0014 §3 objects to throughout.
#[salsa::tracked(returns(clone))]
pub fn imported_values(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
) -> Arc<jr_mir::ImportedValues> {
    let hir = file_hir(db, file);
    let resolve = resolved(db, file, search_paths).map;

    let mut pairs: Vec<(ItemId, Symbol)> = resolve
        .resolutions
        .values()
        .filter_map(|res| match res {
            Res::Imported(import, name) => Some((*import, *name)),
            Res::Local(_) | Res::Param(_) | Res::Item(_) | Res::Promoted { .. } | Res::Error => {
                None
            }
        })
        .collect::<FxHashSet<_>>()
        .into_iter()
        .collect();
    pairs.sort_unstable();

    // One `file_consts` call per imported module rather than per name, matching `imported_procs`'
    // one-module-lookup-per-import.
    let mut evaluated: FxHashMap<ItemId, Option<EvaluatedModule>> = FxHashMap::default();
    let mut out = jr_mir::ImportedValues::new();

    for (import, name) in pairs {
        let entry = evaluated
            .entry(import)
            .or_insert_with(|| {
                // `import_target` yields the `FileHir` but not the `SourceFile` that `file_consts`
                // needs, so the module is looked up once more here. One extra `module_file` call per
                // *import*, memoised by salsa and by the `evaluated` map — not per name.
                let (_, other_hir) = import_target(db, &hir, search_paths, import)?;
                let path = module_path_of(&hir, import)?;
                let found = module_file(db, search_paths, path).found?;
                let other = db.source_file_for_path(found.to_string_lossy().as_ref())?;
                Some((
                    other_hir,
                    crate::consts::file_consts(db, other, search_paths).values,
                ))
            })
            .clone();
        let Some((other_hir, values)) = entry else {
            continue;
        };
        let Some(item) = other_hir.scope.get(name) else {
            continue;
        };
        // A *procedure* crosses as a `ProcRef` through `imported_procs`, not as a value — so only a
        // constant with an evaluated value lands here.
        if let Some(value) = values.item(item) {
            out.set(import, name, value);
        }
    }

    Arc::new(out)
}

/// One imported module's HIR and its evaluated constants, memoised per `#import` item.
///
/// A named type because the tuple is complex enough that clippy asks for one, and the name says what
/// the pair is *for*: the HIR resolves a name to an `ItemId`, and the values map that id to a value.
type EvaluatedModule = (Arc<FileHir>, Arc<jr_mir::ConstValues>);

/// The module path an `#import` item names, for [`imported_values`].
fn module_path_of(hir: &FileHir, import: ItemId) -> Option<Arc<str>> {
    let ItemKind::Import { path, .. } = &hir.items.get(import.index())?.kind else {
        return None;
    };
    Some(Arc::from(path.as_str()))
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
    /// Diagnostics from resolving and checking the **expanded** tree, when a computed `#insert` was
    /// expanded (ADR-0073 §1, step 6). Empty otherwise.
    ///
    /// Carried here rather than reported by `frontend_diagnostics` because expansion needs
    /// `insert_operands`, which the frontend gate runs *before* — so these are the diagnostics only the
    /// post-expansion tree can produce, and `file_diagnostics` (which already depends on this query) is
    /// where they surface. Without this a misspelled name inside a body holding a computed insert reached
    /// the user as "the compiler could not lower `main`", an internal-sounding message for an ordinary
    /// typo: the unexpanded resolve withholds E0201 there (it cannot know what the insert declares), so
    /// the expanded resolve is the only pass that can report it.
    pub expanded_diagnostics: Arc<jr_diag::Diagnostics>,
    /// The HIR the `mir` was lowered from, and its signatures — the **expanded** ones when a polymorphic
    /// instantiation added procedures (ADR-0082 §2), else the base file's.
    ///
    /// Carried because instantiation makes `mir` have *more procedures* than the base HIR, so a consumer
    /// that paired `mir` with `file_hir(db, file)` — as `jr-vm`'s `add_file` and the native build both do
    /// — would find no declaration for an appended `ProcId`. The `#insert` expansion did not need this
    /// (an insert adds no procedures, so the counts matched and a base-HIR pairing worked); instantiation
    /// is the first expansion that does, which is why this field arrives with it.
    pub hir: Arc<FileHir>,
    /// The signatures matching [`Self::hir`] — expanded when instantiation added procedures.
    pub signatures: Arc<jr_sema::FileSignatures>,
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
            // Nothing was expanded: the gate ran before the operand pre-pass.
            expanded_diagnostics: Arc::new(jr_diag::Diagnostics::new()),
            hir: file_hir(db, file),
            signatures: crate::sema::file_signatures(db, file, search_paths).signatures,
        };
    }

    // **Computed `#insert`s, expanded** (ADR-0073 §1, step 6). When the operand pre-pass has evaluated an
    // operand's text, the file is re-lowered with it — and the inserted statements are nodes the ordinary
    // `resolved`/`checked` never saw, so they need a resolve and a check over *that* tree. Empty for every
    // file with no computed insert, which takes the ordinary path below at zero cost.
    //
    // Acyclic: `insert_operands` reaches `file_consts` → `frontend_diagnostics`, which is mir-free (only
    // `file_diagnostics` calls this query), so nothing here loops back.
    let operands = crate::consts::insert_operands(db, file, search_paths);
    #[allow(clippy::type_complexity)]
    let expanded: Option<(
        Arc<FileHir>,
        Arc<jr_hir::ResolveMap>,
        crate::sema::CheckResult,
        jr_diag::Diagnostics,
    )> = if operands.is_empty() {
        None
    } else {
        let parse = crate::parse_file(db, file);
        let file_id = crate::queries::resolve_file_id(db, file);
        let interner = db.interner();
        let (tree, _diags) =
            jr_hir::lower_file_with_inserts(&parse, file_id, interner, operands.as_ref());
        let tree = Arc::new(tree);
        let (resolve_map, check, diags) =
            crate::sema::checked_expanded(db, file, search_paths, tree.as_ref());
        Some((tree, resolve_map, check, diags))
    };

    // **Polymorphic instantiations, expanded** (ADR-0082 §2). When a file has polymorphic calls, the HIR
    // gains one appended procedure per distinct instantiation, and signatures/resolve/check are recomputed
    // over that expanded tree — unlike the `#insert` branch, which reuses signatures because it adds no
    // items (§3). `None` for a file with no polymorphic calls, which takes the ordinary path.
    //
    // The two expansions do not currently compose: a computed `#insert` that introduces a polymorphic call
    // is out of scope (ADR-0082 §5's spirit), so this runs only when there was no `#insert` expansion.
    let instantiated = if expanded.is_some() {
        None
    } else {
        crate::sema::instantiated(db, file, search_paths)
    };

    let hir = match (&expanded, &instantiated) {
        (Some((tree, _, _, _)), _) => tree.clone(),
        (_, Some(inst)) => inst.hir.clone(),
        _ => file_hir(db, file),
    };
    let own_resolve = match (&expanded, &instantiated) {
        (Some((_, resolve_map, _, _)), _) => resolve_map.clone(),
        (_, Some(inst)) => inst.resolve.clone(),
        _ => resolved(db, file, search_paths).map,
    };
    let base_sigs = crate::sema::file_signatures(db, file, search_paths);
    let own_signatures = match &instantiated {
        Some(inst) => inst.signatures.clone(),
        None => base_sigs.signatures.clone(),
    };
    let checked_file = match (&expanded, &instantiated) {
        (Some((_, _, check, _)), _) => check.clone(),
        (_, Some(inst)) => inst.check.clone(),
        _ => checked(db, file, search_paths),
    };
    let types = checked_file.types;
    let operators = checked_file.operator_calls;
    let filled = checked_file.filled_args;
    let imports = imported_procs(db, file, search_paths);
    let imported_constants = imported_values(db, file, search_paths);
    // The const values, plus the call→instantiation redirects (ADR-0082): `call_rvalue` consults these to
    // target the appended procedure rather than the template.
    let consts = {
        let base = crate::consts::file_consts(db, file, search_paths).values;
        // **A folded value keyed by `ExprId` is stale once a body expands** (ADR-0101 §3). `file_consts`
        // records `folded_calls` against the *unexpanded* tree, and an expansion renumbers every id after
        // the splice — so in the expanded tree those ids name *different* expressions, and a second folded
        // `#insert` in one body left a `string` value sitting on an arithmetic operand. The failure was a
        // verifier panic (`mixed operand types`), not a diagnostic, because the value is well-typed *for the
        // expression it was computed for*: the "well-typed placeholder" family AGENTS.md names, in its
        // sharpest form yet — the placeholder is a genuine value from the same program.
        //
        // Fixed by **clearing and re-recording from the expanded check**, which is the only pass that saw
        // the ids MIR will use. Clearing matters as much as re-recording: a stale entry the expanded check
        // does not replace is exactly the wrong value at a live id.
        let base = match &expanded {
            Some((_, _, check, _)) => {
                let mut values = (*base).clone();
                for (scope, expr) in checked(db, file, search_paths).folded_calls.keys() {
                    values.clear_run(*scope, *expr);
                }
                for ((scope, expr), value) in check.folded_calls.iter() {
                    values.set_run(*scope, *expr, *value);
                }
                Arc::new(values)
            }
            None => base,
        };
        match &instantiated {
            Some(inst) => {
                let mut values = (*base).clone();
                for (call, target) in &inst.redirects {
                    values.set_instantiation(call.0, call.1, *target);
                }
                // Each comptime call's argument-drop mask, so `call_rvalue` passes only the non-comptime
                // arguments to the instantiation whose parameter list has already had the `$N`s dropped
                // (ADR-0088 §3).
                for (call, mask) in &inst.comptime_masks {
                    values.set_comptime_arg_mask(call.0, call.1, mask.clone());
                }
                // **The instantiation's own `type_info(T)` calls fold here** (ADR-0092 §1). `file_consts`
                // folded the *base* check's, where a template's `T` had no binding and the call was
                // withheld — so an instantiation's `type_info(T)` had no value and `scan` refused the body,
                // surfacing as "no routine for file 0 proc 2". Folded against the instantiation's check,
                // which is where `T` is bound, with the same `type_info_value` `file_consts` uses so the
                // two cannot disagree about what a `Type_Info` is.
                // The same re-recording for an instantiation's tree (ADR-0101 §3), for the same reason: an
                // appended procedure's `folded_calls` are keyed in *its* arena.
                for ((scope, expr), value) in inst.check.folded_calls.iter() {
                    values.set_run(*scope, *expr, *value);
                }
                if !inst.check.type_info_calls.is_empty() {
                    let base_sigs_for_ti = crate::sema::file_signatures(db, file, search_paths);
                    let module_sigs: Vec<Arc<jr_sema::FileSignatures>> =
                        crate::sema::imported_signatures(db, file, search_paths);
                    let mut pool = crate::sema::lock_pool(db);
                    let interner = db.interner();
                    let mut all_sigs: Vec<&jr_sema::FileSignatures> = vec![
                        inst.signatures.as_ref(),
                        base_sigs_for_ti.signatures.as_ref(),
                    ];
                    all_sigs.extend(module_sigs.iter().map(AsRef::as_ref));
                    for ((scope, expr), described) in inst.check.type_info_calls.iter() {
                        if let Ok(value) = crate::consts::type_info_value(
                            &mut pool, interner, &all_sigs, *described,
                        ) {
                            values.set_run(*scope, *expr, value);
                        }
                    }
                }
                Arc::new(values)
            }
            None => base,
        }
    };
    let interner = db.interner();

    // Gather everything from other queries *before* locking the pool: the lock
    // must never be held across a nested query call.
    let mut pool = crate::sema::lock_pool(db);
    let lowered = jr_mir::lower_file(
        hir.as_ref(),
        own_resolve.as_ref(),
        types.as_ref(),
        own_signatures.as_ref(),
        consts.as_ref(),
        imports.as_ref(),
        imported_constants.as_ref(),
        operators.as_ref(),
        filled.as_ref(),
        interner,
        &mut pool,
    );
    drop(pool);

    // **Each instantiation's `#modify` predicate runs here** (ADR-0095 §1), the one place with everything
    // it needs: the *expanded* tree, its MIR (just lowered above), and the VM. `instantiated()` runs before
    // this and `file_consts` evaluates the *unexpanded* tree, which is why ADR-0094 §3 could not put it in
    // either. A predicate answering `false` refuses its guarded instantiation with E0275 — a rejection the
    // author asked for, so its message names the predicate rather than reading like a compiler fault.
    let modify_rejections =
        evaluate_modify_predicates(db, file, hir.as_ref(), &lowered, &own_signatures);

    // The expanded tree's own resolve/check diagnostics — from a computed `#insert` (ADR-0073) or an
    // instantiation (ADR-0082), whichever expanded. Only this pass can produce them.
    let mut expanded_diagnostics = expanded
        .map(|(_, _, _, diags)| diags)
        .or_else(|| instantiated.map(|inst| inst.diagnostics))
        .unwrap_or_default();
    // A rejected instantiation is reported through the same channel the expansion's own diagnostics take,
    // so `file_diagnostics` picks it up with no new plumbing (ADR-0095 §1).
    for diag in modify_rejections {
        expanded_diagnostics.push(diag);
    }

    MirResult {
        mir: Arc::new(lowered),
        gated: false,
        expanded_diagnostics: Arc::new(expanded_diagnostics),
        // The HIR and signatures the MIR was lowered from — expanded when instantiation added procedures
        // (ADR-0082 §2), so a consumer pairing MIR with them finds every appended procedure.
        hir,
        signatures: own_signatures,
    }
}

/// A textual dump of a file's MIR, for tests and for `jr` to print.
///
/// Not a tracked query: it is a rendering of one, and memoising a `String` nothing
/// compares would cost memory for no invalidation benefit.
#[must_use]
pub fn dump_mir(db: &dyn Db, file: SourceFile, search_paths: ModuleSearchPaths) -> String {
    render(db, file_mir(db, file, search_paths))
}

/// A textual dump of a file's MIR *after* inlining.
///
/// Separate from [`dump_mir`] rather than a flag on it, because the two describe
/// different things and a test should have to say which it means: `dump_mir` shows
/// the program the user wrote, and this shows the program the back end receives.
#[must_use]
pub fn dump_optimized_mir(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
    config: BuildConfig,
) -> String {
    render(db, optimized_file_mir(db, file, search_paths, config))
}

fn render(db: &dyn Db, result: MirResult) -> String {
    if result.gated {
        return String::from("gated: the file has errors\n");
    }
    // The **expanded** HIR and signatures the MIR was lowered from (ADR-0082 §2), not the base file's —
    // an instantiation added procedures the base HIR does not have, and pairing base HIR with expanded
    // MIR would dump a procedure with no declaration. The result now carries what this used to recompute.
    let hir = result.hir;
    let signatures = result.signatures;
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

// ---------------------------------------------------------------------------
// optimized_file_mir — tracked query
// ---------------------------------------------------------------------------

/// A file's MIR with every eligible call inlined (ADR-0021 §1).
///
/// This is the staged half ADR-0017 §3 described and deferred: [`file_mir`] stays
/// the unstaged query with no cross-body dependencies, and this one — which reads
/// the MIR of every module the file imports — is what `jr run` and `jr build`
/// consume. Diagnostics and [`dump_mir`] deliberately stay on built MIR, so the
/// dump, the corpus snapshots and the editor keep describing the program the
/// programmer wrote.
///
/// The invalidation cost is real and is stated in ADR-0021 §1: editing
/// `modules/Basic` invalidates this query for every importer wholesale. ADR-0017 §5
/// accepted fan-in invalidation as inherent to inlining; the coarseness is what a
/// per-body key would fix later.
///
/// # Why this cannot be what comptime runs
///
/// `file_consts` calls `jr_mir::lower_file` directly — its own docs explain that
/// calling `file_mir` from there would be a salsa cycle — so comptime is strictly
/// *upstream* of this query and could not consume it even if that were wanted.
/// ADR-0021 §2 is the consequence: the bodies comptime executes are frozen here, so
/// both engines run the same MIR for every one of them, and `PLAN.md` §3.1's
/// invariant holds structurally rather than by trusting the inliner.
///
/// Uses `no_eq` to match the rest of this crate's queries.
#[salsa::tracked(returns(clone), no_eq)]
pub fn optimized_file_mir(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
    config: BuildConfig,
) -> MirResult {
    let built = file_mir(db, file, search_paths);
    if built.gated {
        return built;
    }

    let hir = file_hir(db, file);
    let file_id = crate::queries::resolve_file_id(db, file);
    let resolve = resolved(db, file, search_paths).map;
    let imports = imported_procs(db, file, search_paths);
    let frozen = frozen_procs(
        file_id,
        &built.mir,
        &jr_mir::const_callees(hir.as_ref(), file_id, resolve.as_ref(), imports.as_ref()),
    );

    // Every module this file imports, because `imported_procs` only ever resolves a
    // callee in a *direct* import, so a transitive walk would read MIR no
    // `Callee::Direct` in this file can name.
    let modules: Vec<Arc<FileMir>> = imported_modules(db, &hir, search_paths)
        .into_iter()
        .map(|module| file_mir(db, module, search_paths))
        .filter(|result| !result.gated)
        .map(|result| result.mir)
        .collect();

    // Every query result gathered before the pool is locked: the lock must never be
    // held across a nested query call. Mutable because const-prop interns the values
    // it folds — ADR-0015 keys an integer value on its type, so `4 + 5` at `s64` is a
    // pool entry that may not exist yet.
    let mut pool = crate::sema::lock_pool(db);

    let mut callees = Callees::new();
    for module in &modules {
        for (_, body) in module.iter() {
            if let Ok(body) = body {
                callees.insert(body);
            }
        }
    }
    // The file's own bodies are candidates too — `024-hello.jr`'s `add` is one — and
    // they are taken from *built* MIR, so a splice never copies a partially inlined
    // body. With a leaf-only rule there is nothing to cascade anyway; taking them
    // from the built map means that stays true if the rule is ever relaxed.
    for (_, body) in built.mir.iter() {
        if let Ok(body) = body {
            callees.insert(body);
        }
    }

    let mut out = FileMir::new();
    for (proc, body) in built.mir.iter() {
        match body {
            Ok(body) if !frozen.contains(&proc) => {
                let mut body = body.clone();
                // One call, because ADR-0022 §3 gives `jr-mir` the pass order and
                // leaves this query only the decision above: *which* bodies may be
                // rewritten. A future pass is a change in one crate, and it cannot
                // accidentally be appended on the wrong side of the frozen check.
                // The strip pass runs **before** the pipeline and **once** (ADR-0058 §1). Not
                // inside the loop, because a body never grows a new `BoundsCheck`, so a second
                // scan could only find nothing — and not after, because a stripped body has
                // fewer statements for const-prop and DCE to work with, which is the point.
                if !config.bounds_checks(db) {
                    jr_mir::strip_bounds_checks(&mut body);
                }
                jr_mir::optimize(&mut body, &callees, &mut pool);
                out.push(proc, Ok(body));
            }
            // A frozen body and a refused one are both passed through unchanged, for
            // different reasons: ADR-0021 §2 for the first, and there being nothing
            // to optimise for the second.
            Ok(body) => out.push(proc, Ok(body.clone())),
            Err(poisoned) => out.push(proc, Err(*poisoned)),
        }
    }
    drop(pool);

    MirResult {
        mir: Arc::new(out),
        gated: false,
        // The optimiser consumes already-expanded MIR, so expansion diagnostics were reported by the
        // built-MIR query this one reads.
        expanded_diagnostics: Arc::new(jr_diag::Diagnostics::new()),
        // Carried through from the built result: the optimiser rewrites bodies, not the procedure set, so
        // the expanded HIR and signatures still match (ADR-0082 §2).
        hir: built.hir.clone(),
        signatures: built.signatures.clone(),
    }
}

/// The procedures in `file` that compile-time evaluation could execute.
///
/// ADR-0021 §2's frozen set: the inliner must leave every one of these
/// byte-identical to its built form, because `file_consts` runs them from its own
/// lowering and `PLAN.md` §3.1 requires both engines to execute the same MIR.
///
/// # Why same-file calls only
///
/// Because comptime cannot follow a cross-file call today: `file_consts` lowers only
/// its own file's HIR, so a `Callee::Direct` naming another file has no body in the
/// map it hands the VM and evaluation fails with E0230. Every body comptime can
/// reach is therefore in this file, reached through this file's calls.
///
/// **This is the one place ADR-0021 §2's soundness rests on that accident.** A `#run`
/// in another file calling into this one would need a set this per-file query cannot
/// compute — salsa has no reverse dependencies. `tests/optimized_mir.rs` pins the
/// refusal so that enabling a cross-file `#run` fails there rather than shipping a
/// comptime/runtime divergence.
fn frozen_procs(
    file: FileId,
    mir: &FileMir,
    roots: &[jr_mir::ProcRef],
) -> FxHashSet<jr_hir::ProcId> {
    let mut frozen = FxHashSet::default();
    let mut queue: Vec<jr_hir::ProcId> = roots
        .iter()
        .filter(|root| root.file == file)
        .map(|root| root.proc)
        .collect();
    while let Some(proc) = queue.pop() {
        if !frozen.insert(proc) {
            continue;
        }
        let Some(Ok(body)) = mir.get(proc) else {
            continue;
        };
        for callee in same_file_callees(body, file) {
            if !frozen.contains(&callee) {
                queue.push(callee);
            }
        }
    }
    frozen
}

/// Every procedure in this file that `body` calls directly.
fn same_file_callees(body: &jr_mir::MirBody, file: FileId) -> Vec<jr_hir::ProcId> {
    let mut out = Vec::new();
    let mut note = |rvalue: &jr_mir::Rvalue| {
        if let jr_mir::Rvalue::Call {
            callee: jr_mir::Callee::Direct(target),
            args: _,
        } = rvalue
            && target.file == file
        {
            out.push(target.proc);
        }
    };
    for block in body.blocks() {
        for stmt in &block.stmts {
            match stmt {
                jr_mir::Statement::Assign { rvalue, .. }
                | jr_mir::Statement::Discard { rvalue, .. } => note(rvalue),
                // None of these can contain a call, so none contributes a callee.
                jr_mir::Statement::Store { .. }
                | jr_mir::Statement::Zero { .. }
                | jr_mir::Statement::BoundsCheck { .. }
                | jr_mir::Statement::TagCheck { .. }
                | jr_mir::Statement::Nop => {}
            }
        }
    }
    out
}

/// The already-loaded files this file imports directly.
///
/// A self-import is skipped for the same reason `run::reachable_files` skips one:
/// ADR-0014 §6 makes it a no-op, and reading a file's own MIR from its own optimized
/// query would be a salsa cycle rather than merely redundant.
fn imported_modules(
    db: &dyn Db,
    hir: &FileHir,
    search_paths: ModuleSearchPaths,
) -> Vec<crate::SourceFile> {
    let mut out: Vec<crate::SourceFile> = Vec::new();
    for item in &hir.items {
        let ItemKind::Import { path, .. } = &item.kind else {
            continue;
        };
        let lookup = module_file(db, search_paths, Arc::from(path.as_str()));
        let Some(found) = lookup.found else { continue };
        let Some(module) = db.source_file_for_path(found.to_string_lossy().as_ref()) else {
            continue;
        };
        if !out.contains(&module) {
            out.push(module);
        }
    }
    out
}

/// A `#modify` predicate that answered `false` refuses its instantiation (ADR-0095 §1).
///
/// This code exists because a rejection is a *feature*, not a compiler fault: the author wrote a predicate
/// precisely so some instantiations would be refused, so the message names the guard rather than reading
/// like an internal error.
const E0275: &str = "E0275";

/// Runs each instantiation's `#modify` predicate, returning a diagnostic per rejection (ADR-0095 §1).
///
/// Called from [`file_mir`] because that is the only place with all three things a predicate needs: the
/// **expanded** tree (where the predicate clone lives), that tree's MIR, and the VM. `instantiated()` runs
/// before the MIR exists and `file_consts` evaluates the *unexpanded* tree, which is why ADR-0094 §3 could
/// not put this in either.
///
/// A predicate that fails to *run* — a trap, an unsupported operation — is **not** a rejection: it is left
/// to the ordinary refusal path rather than silently rejecting the instantiation, because "the guard could
/// not be evaluated" and "the guard said no" are different findings and only the second is the author's
/// intent.
fn evaluate_modify_predicates(
    db: &dyn Db,
    file: SourceFile,
    hir: &jr_hir::FileHir,
    mir: &jr_mir::FileMir,
    signatures: &jr_sema::FileSignatures,
) -> Vec<jr_diag::Diagnostic> {
    if hir.modify_predicates.is_empty() {
        return Vec::new();
    }
    let file_id = crate::queries::resolve_file_id(db, file);
    let mut out: Vec<jr_diag::Diagnostic> = Vec::new();
    let pool = crate::sema::lock_pool(db);
    let mut program = jr_vm::comptime_program();
    if jr_vm::add_file(&mut program, file_id, hir, mir, signatures, &pool).is_err() {
        // The tree could not be loaded into the comptime program at all. Not a rejection — see this
        // function's docs — so the instantiation stands and any real problem is reported elsewhere.
        return Vec::new();
    }
    let pairs: Vec<(jr_hir::ProcId, jr_hir::ProcId)> = hir.modify_predicates.clone();
    // **The context's layout, read before the VM borrows the pool** (the same order `run_main` uses, and for
    // the same reason: the mutex is not reentrant, so locking twice deadlocks). A predicate is an ordinary
    // Jairs procedure, so it takes the hidden context parameter every one does (ADR-0057 §4) — calling it
    // with no arguments gave "called a procedure taking 1 arguments with 0", found by running.
    let context_layout = jr_pool::Pool::find_context(&pool)
        .and_then(|ctx| jr_pool::layout_of(&pool, jr_pool::TargetLayout::LP64, ctx).ok());
    {
        let Ok(mut vm) = jr_vm::Vm::new(&program, &pool, jr_vm::Mode::Comptime) else {
            return Vec::new();
        };
        for (guarded, predicate) in &pairs {
            let target = jr_mir::ProcRef::new(file_id, *predicate);
            let args = match context_layout {
                Some(layout) => match vm.new_context(layout.size, layout.align) {
                    Ok(ctx) => vec![ctx],
                    // No context could be allocated: not a rejection (see this function's docs).
                    Err(_) => continue,
                },
                None => Vec::new(),
            };
            // `false` — the author's guard rejects this instantiation. Any other outcome (`true`, or a
            // predicate that failed to run) leaves it standing: see this function's docs for why a failure
            // to evaluate is deliberately not a rejection.
            if let Ok(jr_vm::Value::Scalar(0)) = vm.call(target, args) {
                let span = hir.proc(*guarded).span;
                out.push(
                    jr_diag::Diagnostic::error(
                        span,
                        "this instantiation is rejected by the procedure's `#modify` predicate",
                    )
                    .with_code(E0275)
                    .with_note(
                        "the predicate ran at compile time and returned `false`, which refuses the \
                             call that produced this instantiation (ADR-0095)",
                    ),
                );
            }
        }
    }
    out
}
