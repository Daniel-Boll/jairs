//! Module resolution: search paths, file lookup, and the salsa queries that
//! wire `jr-hir` lowering and resolution into the incremental database.
//!
//! # Query dependency graph
//!
//! ```text
//! module_search_paths (salsa input)
//!        │
//!        ▼
//! module_file(name) ──────────────────────────────────────────────────────┐
//!                                                                          │
//! parse_file(file) ──► file_hir(file) ──► file_exports(file)             │
//!                              │                   ▲                      │
//!                              │                   │ (for each import)    │
//!                              ▼                   │                      │
//!                       imports_of(file)           │                      │
//!                              │                   │                      │
//!                              └──► module_file ───┘                      │
//!                                                                          │
//! file_hir(file) + file_exports(imports) ──► resolved(file)              │
//!                                                                          │
//! parse_file + file_hir + resolved ──► file_diagnostics(file)            │
//! ```
//!
//! ## Why there is no salsa cycle despite cyclic module imports
//!
//! The critical invariant is that **`file_exports` depends only on
//! `file_hir`**, which depends only on `parse_file`. It does NOT depend on
//! `resolved`. Therefore:
//!
//! - `resolved(A)` calls `file_exports(B)` to get B's exported names.
//! - `file_exports(B)` calls `file_hir(B)` — lowering only, no resolution.
//! - `file_hir(B)` calls `parse_file(B)` — pure syntax.
//!
//! Even if B imports A (a cycle in the module graph), `file_exports(B)` never
//! calls `resolved(A)` or `resolved(B)`. The salsa query graph is acyclic.
//!
//! ## Module loading and the filesystem seam
//!
//! Module files are loaded into the database **before** running resolution
//! queries. The [`crate::JairsDatabase::load_module`] method (called by the
//! batch driver and LSP) reads a module file from the filesystem (or an
//! in-memory map for tests) and registers it as a salsa [`SourceFile`] input.
//! The `resolved` query then looks up already-loaded files by path — it never
//! touches the filesystem itself.
//!
//! This separation keeps tracked queries pure (no filesystem I/O inside
//! salsa queries) and avoids the need for `&mut db` inside a tracked query.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use jr_base::{Interner, Span};
use jr_diag::{Diagnostic, Diagnostics};
use jr_hir::{FileHir, ItemKind, ItemScope, ResolveMap};

use crate::{Db, SourceFile};

// ---------------------------------------------------------------------------
// Diagnostic code
// ---------------------------------------------------------------------------

const E0210: &str = "E0210";

/// A body the compiler could not lower, in a file that otherwise checks clean (ADR-0047 §2).
///
/// The **first code in this project that reports a compiler limitation** rather than a program
/// error — E0231 was the first warning, and this is the first admission. A category worth having
/// exactly once, so it is one code raised from one place, meaning "this program is legal and
/// this compiler could not lower it".
///
/// It replaced a crash: a refused body that was actually *called* surfaced as
/// `internal compiler error: no routine for file 0 proc 0` on a program `jr check` had just
/// called clean.
const E0245: &str = "E0245";

// ---------------------------------------------------------------------------
// Module name type alias
// ---------------------------------------------------------------------------

/// A module name as it appears in `#import "Name"`.
///
/// This is the bare name string (e.g. `"Basic"`, `"Shapes"`), not a path.
pub type ModuleName = Arc<str>;

// ---------------------------------------------------------------------------
// ModuleSearchPaths — salsa input
// ---------------------------------------------------------------------------

// The salsa macro generates undocumented associated functions (new, field
// getters, field setters). We allow missing_docs for the input struct
// rather than for the whole module.
#[allow(missing_docs)]
mod search_paths_input {
    use std::{path::PathBuf, sync::Arc};

    /// The ordered list of directories to search for modules.
    ///
    /// This is a salsa input so that changing `--module-path` on the command line
    /// correctly invalidates all `module_file` queries and their dependents.
    ///
    /// **Why a salsa input?** Module search paths are configuration that comes
    /// from outside the source files. If they change (e.g. the user adds a new
    /// `--module-path`), every `module_file` lookup may return a different result.
    /// Making them a salsa input ensures that salsa tracks the dependency and
    /// re-runs affected queries automatically.
    #[salsa::input]
    pub struct ModuleSearchPaths {
        /// The ordered list of search directories.
        ///
        /// Each entry is an absolute (or workspace-relative) path to a directory
        /// that is searched for modules. Entries are tried in order; the first
        /// hit wins.
        #[returns(clone)]
        pub paths: Arc<[PathBuf]>,
    }
}

pub use search_paths_input::ModuleSearchPaths;

// ---------------------------------------------------------------------------
// ModuleLookupResult
// ---------------------------------------------------------------------------

/// The result of looking up a module by name.
///
/// Carries both the resolved file path (if found) and the list of paths that
/// were searched, so that E0210 can list every location tried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleLookupResult {
    /// The resolved file path, if the module was found.
    pub found: Option<PathBuf>,
    /// Every path that was probed, in search order.
    ///
    /// This is always populated (even on success) so that diagnostics can
    /// show the full search list.
    pub searched: Vec<PathBuf>,
}

// ---------------------------------------------------------------------------
// ResolveResult
// ---------------------------------------------------------------------------

/// The result of resolving a file against its imports' export scopes.
///
/// Bundles the [`ResolveMap`] (name→resolution) and any diagnostics produced
/// during resolution (E0201 unresolved names, E0200 duplicates, E0210 missing
/// modules).
#[derive(Debug, Clone)]
pub struct ResolveResult {
    /// The name-resolution map for this file.
    pub map: Arc<ResolveMap>,
    /// Diagnostics from resolution (E0200, E0201, E0210, …).
    pub diagnostics: Arc<Diagnostics>,
}

// ---------------------------------------------------------------------------
// module_file — tracked query
// ---------------------------------------------------------------------------

/// Looks up a module by name and returns the path to its entry file, if found.
///
/// Search order (ADR-0014 §1):
/// 1. For each search path, in order:
///    a. `<path>/<Name>/module.jr` — directory form (tried first)
///    b. `<path>/<Name>.jr` — single-file form
/// 2. First hit wins.
///
/// The importing file's own directory is deliberately **not** searched.
///
/// Returns a [`ModuleLookupResult`] that carries both the found path (if any)
/// and the full list of paths that were probed, so that E0210 can list them.
#[salsa::tracked(returns(clone))]
pub fn module_file(
    db: &dyn Db,
    search_paths: ModuleSearchPaths,
    name: ModuleName,
) -> ModuleLookupResult {
    let paths = search_paths.paths(db);
    let mut searched = Vec::new();

    for dir in paths.iter() {
        // Try directory form first: <dir>/<Name>/module.jr
        let dir_form = dir.join(name.as_ref()).join("module.jr");
        searched.push(dir_form.clone());
        if db.read_module_file(&dir_form).is_some() {
            return ModuleLookupResult {
                found: Some(dir_form),
                searched,
            };
        }

        // Try single-file form: <dir>/<Name>.jr
        let file_form = dir.join(format!("{}.jr", name.as_ref()));
        searched.push(file_form.clone());
        if db.read_module_file(&file_form).is_some() {
            return ModuleLookupResult {
                found: Some(file_form),
                searched,
            };
        }
    }

    ModuleLookupResult {
        found: None,
        searched,
    }
}

// ---------------------------------------------------------------------------
// file_hir — tracked query
// ---------------------------------------------------------------------------

/// Lowers a source file to HIR.
///
/// This is a thin salsa wrapper around [`jr_hir::lower_file`]. It depends
/// only on `parse_file` (the syntax tree) and the interner — never on
/// resolution or other files.
///
/// Uses `no_eq` because [`FileHir`] does not implement [`PartialEq`].
#[salsa::tracked(returns(clone), no_eq)]
pub fn file_hir(db: &dyn Db, file: SourceFile) -> Arc<FileHir> {
    let parse = crate::parse_file(db, file);
    let file_id = crate::queries::resolve_file_id(db, file);
    let interner = db.interner();
    let (hir, _diags) = jr_hir::lower_file(&parse, file_id, interner);
    Arc::new(hir)
}

// ---------------------------------------------------------------------------
// file_exports — tracked query
// ---------------------------------------------------------------------------

/// Returns the export scope of a file: the set of names it makes available
/// to importers.
///
/// **Depends only on `file_hir`** — never on `resolved` or any other file.
/// This is the key invariant that prevents salsa cycles when modules import
/// each other: `resolved(A)` can call `file_exports(B)` without
/// `file_exports(B)` calling back into `resolved(A)`.
///
/// **Filters out what `#scope_module` hides** (ADR-0054 §3). Export is the default, so a file with
/// no visibility marker exports everything exactly as it did before — which ADR-0014 §2 promised and
/// the whole corpus relies on.
///
/// The filter reads `Item::exported`, which lowering computed from this file's own source order. That
/// is what keeps this query dependent on `file_hir` alone: a visibility rule needing *resolution* —
/// an export list naming identifiers, say — could reach into another file and reintroduce the cycle
/// this query's shape exists to prevent.
///
/// The declaring file's own `hir.scope` is untouched, so a hidden name resolves and answers hover
/// inside its own file. "Module-private" means invisible to importers, not invisible everywhere.
///
/// Uses `no_eq` because [`ItemScope`] does not implement [`PartialEq`].
#[salsa::tracked(returns(clone), no_eq)]
pub fn file_exports(db: &dyn Db, file: SourceFile) -> Arc<ItemScope> {
    let hir = file_hir(db, file);
    // **`FileHir::export_scope` owns the rule**, and this query only caches it. Duplicating the
    // filter here would be two answers to "what does this module export", and whichever a consumer
    // happened to call would decide whether it saw encapsulation at all (ADR-0054 §3).
    Arc::new(hir.export_scope())
}

// ---------------------------------------------------------------------------
// imports_of — tracked query
// ---------------------------------------------------------------------------

/// Returns the list of module names imported by a file.
///
/// Scans the file's HIR for `ItemKind::Import` items and collects the path
/// strings. Duplicate imports are deduplicated (ADR-0014 §6: importing the
/// same module twice is idempotent).
///
/// Self-imports (a file importing itself by name) are included here; the
/// caller is responsible for ignoring them.
#[salsa::tracked(returns(clone))]
pub fn imports_of(db: &dyn Db, file: SourceFile) -> Arc<[ModuleName]> {
    let hir = file_hir(db, file);
    let mut seen = rustc_hash::FxHashSet::default();
    let mut names: Vec<ModuleName> = Vec::new();
    for item in &hir.items {
        if let ItemKind::Import { path, .. } = &item.kind {
            let name: ModuleName = Arc::from(path.as_str());
            if seen.insert(path.clone()) {
                names.push(name);
            }
        }
    }
    names.into()
}

// ---------------------------------------------------------------------------
// resolved — tracked query
// ---------------------------------------------------------------------------

/// Resolves a file against its imports' export scopes.
///
/// For each module name imported by the file:
/// 1. Look up the module file via `module_file`.
/// 2. If found, look it up in the database (it must already be loaded via
///    [`Db::source_file_for_path`]) and get its export scope via `file_exports`.
/// 3. If not found, emit E0210 with the full list of searched paths.
///
/// Then call [`jr_hir::resolve`] with the collected import scopes.
///
/// **Module files must be pre-loaded** (via [`Db::load_module`] or
/// [`JairsDatabase::set_file_text`]) before calling this query. The query
/// itself does not touch the filesystem.
///
/// Uses `no_eq` because [`ResolveResult`] contains [`Diagnostics`] which is
/// not `Eq`.
#[salsa::tracked(returns(clone), no_eq)]
pub fn resolved(db: &dyn Db, file: SourceFile, search_paths: ModuleSearchPaths) -> ResolveResult {
    let hir = file_hir(db, file);
    let import_names = imports_of(db, file);
    let interner = db.interner();

    // The file's own path (for self-import detection).
    let own_path = file.path(db);

    let mut diags = Diagnostics::new();
    // Collect (module_name, ItemScope) pairs for resolution.
    // We keep the scopes alive in a Vec so the slices remain valid.
    let mut import_scopes: Vec<(String, Arc<ItemScope>)> = Vec::new();

    for name in import_names.iter() {
        let lookup = module_file(db, search_paths, name.clone());

        if let Some(ref found_path) = lookup.found {
            // Self-import: a file importing itself is a no-op (ADR-0014 §6).
            let found_path_str = found_path.to_string_lossy();
            if found_path_str.as_ref() == own_path.as_ref() {
                continue;
            }

            // Look up the already-loaded SourceFile for this path.
            // Module files must be pre-loaded by the caller.
            if let Some(module_sf) = db.source_file_for_path(found_path_str.as_ref()) {
                let exports = file_exports(db, module_sf);
                import_scopes.push((name.to_string(), exports));
            }
            // If source_file_for_path returns None, the module file was found
            // on disk but not yet loaded into the database. This should not
            // happen if the caller pre-loads all modules correctly.
        } else {
            // Module not found — emit E0210 with the full search path list.
            let import_span = find_import_span(&hir, name.as_ref());

            // The searched paths go in NOTES, one per path -- not in the
            // message. A multi-line message renders with every continuation
            // line indented to align under the headline, which is unreadable
            // once there are more than a couple of search paths.
            let mut diag = Diagnostic::error(import_span, format!("module `{name}` not found"))
                .with_code(E0210)
                .with_note(if lookup.searched.len() == 1 {
                    "searched 1 location:".to_owned()
                } else {
                    format!("searched {} locations:", lookup.searched.len())
                });
            for path in &lookup.searched {
                diag = diag.with_note(format!("  {}", path.display()));
            }
            diag = diag.with_help(
                "add the module's directory with `--module-path <DIR>`, or check the spelling",
            );
            diags.push(diag);
        }
    }

    // Build the slice of (&str, &ItemScope) for jr_hir::resolve.
    let imports_for_resolve: Vec<(&str, &ItemScope)> = import_scopes
        .iter()
        .map(|(name, scope)| (name.as_str(), scope.as_ref()))
        .collect();

    let (resolve_map, resolve_diags) = jr_hir::resolve(&hir, &imports_for_resolve, interner);
    diags.extend(resolve_diags.iter().cloned());

    ResolveResult {
        map: Arc::new(resolve_map),
        diagnostics: Arc::new(diags),
    }
}

// ---------------------------------------------------------------------------
// frontend_diagnostics — everything before MIR
// ---------------------------------------------------------------------------

/// Collects the diagnostics of every phase up to and including type checking.
///
/// **No phase gates any later one.** A file that does not parse is still lowered,
/// resolved, and type-checked, because an editor wants whatever information is
/// available rather than the first error alone. That is what makes poison
/// propagation `jr-sema`'s obligation: without it every parse error would arrive
/// as an invented type error too.
///
/// # Why this is separate from [`file_diagnostics`]
///
/// It exists so that [`crate::file_mir`] has something to gate on. ADR-0017 §4
/// requires that nothing ask for the MIR of a file with errors, and MIR also
/// *produces* diagnostics of its own (E0227–E0229) that belong in
/// `file_diagnostics`. If MIR gated on `file_diagnostics` and `file_diagnostics`
/// included MIR's, the two queries would form a cycle. Splitting the frontend out
/// breaks it, and the split is the honest one anyway: the gate's question is "did
/// anything before MIR fail", not "did anything at all fail".
///
/// Uses `no_eq` because [`Diagnostics`] is not `Eq`.
#[salsa::tracked(returns(clone), no_eq)]
pub fn frontend_diagnostics(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
) -> Arc<Diagnostics> {
    let mut all = Diagnostics::new();

    // Parse diagnostics (lex + parse errors).
    let parse_diags = crate::parse_diagnostics(db, file);
    all.extend(parse_diags.iter().cloned());

    // Lower diagnostics (E0203–E0209).
    let parse = crate::parse_file(db, file);
    let file_id = crate::queries::resolve_file_id(db, file);
    let interner = db.interner();
    let (_hir, lower_diags) = jr_hir::lower_file(&parse, file_id, interner);
    all.extend(lower_diags.iter().cloned());

    // Resolution diagnostics (E0200, E0201, E0210, E0211).
    let resolve_result = resolved(db, file, search_paths);
    all.extend(resolve_result.diagnostics.iter().cloned());

    // Declaration typing (E0204, E0212–E0214, E0226) and body checking
    // (E0214–E0225). Both phases run: signatures own the file's declarations and
    // the check owns its bodies, so neither subsumes the other.
    let signatures = crate::sema::file_signatures(db, file, search_paths);
    all.extend(signatures.diagnostics.iter().cloned());
    let checked = crate::sema::checked(db, file, search_paths);
    all.extend(checked.diagnostics.iter().cloned());

    Arc::new(all)
}

// ---------------------------------------------------------------------------
// file_diagnostics — all diagnostics for one file
// ---------------------------------------------------------------------------

/// Collects all diagnostics for a single file: the frontend's, plus MIR's.
///
/// Diagnostics are returned in source order (the [`Diagnostics`] sink sorts
/// them by span).
///
/// Compile-time evaluation contributes E0230, and MIR the three that need a
/// control-flow graph — definite assignment,
/// missing `return`, and a `break` outside a loop (E0227–E0229). They are absent
/// when [`crate::file_mir`] gated the file, which is correct: a file with a real
/// error should not also be told that a body it could not analyse might not return.
///
/// Uses `no_eq` because [`Diagnostics`] is not `Eq`.
#[salsa::tracked(returns(clone), no_eq)]
pub fn file_diagnostics(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
) -> Arc<Diagnostics> {
    let mut all = Diagnostics::new();
    all.extend(frontend_diagnostics(db, file, search_paths).iter().cloned());

    // E0230: a `#run` or a constant that could not be evaluated. Reported here rather
    // than in `frontend_diagnostics` because MIR's gate reads that query, and a
    // constant with no value is precisely a thing MIR must still be asked to lower —
    // it refuses the bodies that needed the value, which is the correct outcome.
    let consts = crate::consts::file_consts(db, file, search_paths);
    all.extend(consts.diagnostics.iter().cloned());

    // E0231: an import nothing in the file uses. A warning rather than an error, and here
    // rather than in `frontend_diagnostics` for the same reason E0230 is: MIR's gate reads
    // that query, and an unused import must not stop a file being lowered.
    let unused = crate::imports::unused_imports(db, file, search_paths);
    all.extend(unused.diagnostics().into_vec());

    let mir = crate::mir::file_mir(db, file, search_paths);
    // Diagnostics only the **expanded** tree can produce (ADR-0073 §1): the unexpanded resolve withholds
    // unresolved-name errors in a body holding a pending computed `#insert`, because it cannot know what
    // the insert declares. Reported here rather than in `frontend_diagnostics` because expansion needs
    // `insert_operands`, which that gate runs before — and this query already depends on `file_mir`, so it
    // adds no edge. Without this a misspelling in such a body surfaced as "the compiler could not lower
    // `main`", an internal-sounding message for an ordinary typo.
    all.extend(mir.expanded_diagnostics.iter().cloned());
    if !mir.gated {
        let hir = file_hir(db, file);
        let interner = db.interner();
        let cfg = jr_mir::file_diagnostics(hir.as_ref(), mir.mir.as_ref(), interner);
        all.extend(cfg.into_vec());
        all.extend(refused_bodies(hir.as_ref(), mir.mir.as_ref(), interner).into_vec());
    }

    Arc::new(all)
}

/// E0245: a body the compiler could not lower, in a file that otherwise checks clean.
///
/// **This exists because the alternative was a crash.** A refused body is skipped when the
/// program is assembled — correct, and verified: one nobody calls costs nothing. But a *called*
/// one reached the interpreter's own lookup and produced `internal compiler error: no routine
/// for file 0 proc 0` on a program `jr check` had just called clean. No user can act on that
/// (ADR-0047 §2).
///
/// Reported *here*, in `file_diagnostics`, rather than at the entry point: every consumer —
/// `jr check`, `jr run`, `jr build`, the LSP — then sees it through the one path they already
/// share, so none of them can be the one that still crashes. That is the asymmetry ADR-0047
/// exists to remove, and gating only `run` would have reintroduced it in `build`.
///
/// Only reached when `mir.gated` is false, which means the file has no *errors*: a body refused
/// because an earlier phase reported the cause is not reported twice, which is ADR-0017 §4's
/// silence preserved.
fn refused_bodies(hir: &FileHir, mir: &jr_mir::FileMir, interner: &Interner) -> Diagnostics {
    let mut diags = Diagnostics::new();
    for (proc, outcome) in mir.iter() {
        let Err(reason) = outcome else {
            continue;
        };
        let Some(data) = hir.procs.get(proc.index()) else {
            continue;
        };
        // A `#foreign` procedure has no body to lower and is never refused; guard anyway, so
        // that a future refusal shape cannot make one look broken.
        if data.body.is_none() {
            continue;
        }
        // The name is on the *item* that declares the procedure, not on the `Proc`:
        // procedures are constants (ADR-0012), so a `Proc` carries no name of its own. Found
        // the same way `main_of` finds `main`, rather than by a second convention.
        let declaration = hir.items.iter().find(|item| {
            matches!(
                &item.kind,
                ItemKind::Const {
                    value: jr_hir::ConstValue::Proc(p)
                } if *p == proc
            )
        });
        let name = declaration
            .and_then(|item| item.name)
            .map(|sym| interner.resolve(sym).to_owned())
            .unwrap_or_else(|| String::from("<anonymous>"));
        // The declaration's *name* span, so the diagnostic points at the procedure rather than
        // at its whole body. Falls back to the body's span, which is still inside the right
        // procedure.
        let span = declaration.map_or(data.span, |item| item.name_span);
        // The reason is a short compiler-facing string (`Poisoned::Here`), deliberately not
        // user-facing prose (ADR-0017 §4). It goes in a *note* rather than the headline, so the
        // headline says what happened and the note says what the compiler was doing.
        let detail = match reason {
            jr_mir::Poisoned::Here(what) => (*what).to_owned(),
            jr_mir::Poisoned::Transitive(other) => {
                format!("a body it depends on could not be lowered ({other:?})")
            }
        };
        // A **warning**, not an error, and the severity is the whole design. A refused body
        // that nobody calls genuinely does not stop the program — verified: one sitting beside a
        // working `main` runs and exits normally, and six files in
        // `tests/corpus/imports/valid/` have been in exactly that state since they were written
        // (each reads an imported constant, which `jr-mir` still refuses). Making this an error
        // would reject programs that work today.
        //
        // What must *not* be a warning is a refused body that is actually **run**. `run_main`
        // therefore checks the entry point itself and fails hard, so the ICE cannot come back
        // through the door this severity opens (ADR-0047 §2).
        diags.push(
            Diagnostic::warning(
                span,
                format!("the compiler could not lower the body of `{name}`"),
            )
            .with_code(E0245)
            .with_note(format!("the lowering step reported: {detail}"))
            .with_note(
                "this program is legal and this compiler has a gap — it is not a mistake in \
                 your code",
            )
            .with_note("calling it is an error; leaving it uncalled is not")
            .with_help("please report it, with this file, at the project's issue tracker"),
        );
    }
    diags
}

// ---------------------------------------------------------------------------
// Helper: find the span of an #import for a given module name
// ---------------------------------------------------------------------------

/// Finds the span of the `#import "name"` path string for the given module
/// name in the file's HIR.
///
/// Returns the path string's span if found, or the file's zero span as a
/// fallback (should not happen in practice).
fn find_import_span(hir: &FileHir, name: &str) -> Span {
    for item in &hir.items {
        if let ItemKind::Import { path, path_span } = &item.kind
            && path == name
        {
            return *path_span;
        }
    }
    // Fallback: use the first item's span, or a zero span.
    hir.items.first().map(|i| i.span).unwrap_or_else(|| {
        Span::new(
            jr_base::FileId::from_usize(0),
            jr_base::TextRange::default(),
        )
    })
}

// ---------------------------------------------------------------------------
// In-memory filesystem for tests
// ---------------------------------------------------------------------------

/// An in-memory module filesystem for use in tests.
///
/// Maps absolute path strings to file contents. Pass this to
/// [`crate::JairsDatabase::with_in_memory_modules`] to avoid requiring real
/// temporary directories in tests.
#[derive(Debug, Clone, Default)]
pub struct InMemoryModules {
    files: BTreeMap<PathBuf, String>,
}

impl InMemoryModules {
    /// Creates an empty in-memory module filesystem.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a file to the in-memory filesystem.
    pub fn add(&mut self, path: impl Into<PathBuf>, content: impl Into<String>) {
        self.files.insert(path.into(), content.into());
    }

    /// Returns the content of a file, if it exists.
    pub fn get(&self, path: &Path) -> Option<&str> {
        self.files.get(path).map(|s| s.as_str())
    }
}
