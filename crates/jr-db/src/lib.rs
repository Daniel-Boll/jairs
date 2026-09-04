//! The salsa query database — the single source of truth shared by the batch
//! driver and the language server (ADR-0007).
//!
//! # Architecture
//!
//! This crate is a thin **wiring layer**: it defines salsa inputs for file
//! contents and tracked queries that call the pure functions in `jr-syntax`
//! and `jr-hir`. No lexing, parsing, or lowering logic lives here.
//!
//! # Incrementality
//!
//! Updating a file's text via [`JairsDatabase::set_file_text`] starts a new
//! salsa revision. On the next query call, salsa re-runs only the queries
//! whose inputs changed. Files that were not edited are not re-parsed.
//!
//! # FileId stability
//!
//! Every [`jr_base::Span`] in the compiler is `(FileId, TextRange)`. The
//! [`jr_base::SourceMap`] already preserves [`jr_base::FileId`] when the same
//! path is re-added (see `SourceMap::add`). We mirror that invariant here: the
//! `SourceMap` inside the database is updated on every `set_file_text` call,
//! and the `FileId` for a given path never changes.
//!
//! The tradeoff: the `SourceMap` is stored in a `Mutex` inside the database
//! struct (outside salsa's tracking). Reads are cheap (shared lock), writes
//! happen only when a file is added or updated. This is correct because
//! `SourceMap` is not a salsa input — it is a side-channel that maps paths to
//! stable IDs, and its contents are always consistent with the salsa inputs.
//!
//! # Module resolution
//!
//! Module search paths are configured via [`JairsDatabase::set_module_search_paths`].
//! Module files are pre-loaded via [`JairsDatabase::load_module`] before
//! running resolution queries. The queries in [`module_loader`] implement
//! ADR-0014.
//!
//! # Semantic analysis
//!
//! The [`sema`] queries wrap `jr-sema`. They carry one extra piece of state that
//! salsa does not own: the interned type [`Pool`], reached through [`Db::pool`].
//! Its module docs argue why that is sound and what the alternative would have
//! cost.
//!
//! # The two queries here that exist only because an editor asked
//!
//! [`docs`] attaches doc-comment prose to declarations (ADR-0027 §2), and
//! [`imports`] answers which `#import`s nothing in a file uses (ADR-0031 §3). Both
//! are compiler-side rather than editor-side for the same reason: `jr check` reports
//! them too, and a fact computed in `jr-lsp` would be a fact the batch compiler
//! cannot see. [`workspace`] is the third of that kind, and the only one whose
//! answer comes from outside the database.

pub mod build;
pub mod consts;
pub mod docs;
pub mod imports;
pub mod mir;
pub mod module_loader;
mod queries;
pub mod run;
pub mod sema;
pub mod workspace;

// The salsa macro generates undocumented associated functions (new, field
// getters, field setters). We allow missing_docs for the module that contains
// the generated code rather than for the whole crate.
#[allow(missing_docs)]
mod input {
    use crate::OptLevel;
    use std::sync::Arc;

    /// A salsa input representing one source file.
    ///
    /// The `path` field is the canonical path string used as the file's identity.
    /// The `text` field is the current contents; updating it starts a new salsa
    /// revision and invalidates all queries that depend on this file.
    #[salsa::input]
    pub struct SourceFile {
        /// The file's path, used as a stable identity key.
        #[returns(clone)]
        pub path: Arc<str>,

        /// The current text of the file.
        #[returns(clone)]
        pub text: Arc<str>,
    }

    /// Build settings that change the code a back end receives (ADR-0058 §2).
    ///
    /// **Why a salsa input?** For the reason
    /// [`ModuleSearchPaths`](crate::ModuleSearchPaths) is one, stated in its own docs:
    /// configuration that comes from outside the source files must be an input, or
    /// salsa serves a memo computed under the old value. Toggling
    /// `--no-bounds-check` has to invalidate every query that reads MIR, and an
    /// input is what makes that automatic rather than remembered.
    ///
    /// **Why not a field on `ModuleSearchPaths`?** It would be one fewer thing to
    /// thread and wrong in both directions: changing a module path would invalidate
    /// MIR optimisation, and changing this would invalidate module lookup. Neither
    /// is visible until somebody measures (ADR-0058 §2).
    ///
    /// **Two fields.** ADR-0058 §6 deferred the optimisation level to W8 and said why:
    /// a surface designed around a single boolean would have to be redesigned. ADR-0142
    /// designed it, and it is a second field here rather than a parameter for the same
    /// invalidation reason.
    #[salsa::input]
    pub struct BuildConfig {
        /// Whether array indexing emits a bounds check.
        ///
        /// `true` is the default and what every command but `--no-bounds-check`
        /// passes. `false` makes `jr-mir`'s strip pass run, replacing every
        /// `Statement::BoundsCheck` with a `Nop` (ADR-0003, ADR-0058 §1).
        ///
        /// This is a *build* setting and is therefore separate from `#no_abc`, which
        /// is a property of a procedure and holds whatever the build says
        /// (ADR-0058 §3).
        pub bounds_checks: bool,

        /// How much the mid-end may rewrite a body before a back end sees it.
        ///
        /// [`OptLevel::Standard`] is the default and what every build did before
        /// ADR-0142; [`OptLevel::Off`] makes `optimized_file_mir` pass `file_mir`'s
        /// bodies through untouched.
        ///
        /// Separate from `bounds_checks` in both directions: `--no-bounds-check`
        /// changes what the program *means* and is honoured at every level, while a
        /// level may not change an answer at all (ADR-0142 §2, §3).
        pub opt_level: OptLevel,
    }
}

/// How much the mid-end may rewrite a body before a back end sees it (ADR-0142 §1).
///
/// An enum rather than a `u8`, so that every consumer's match is exhaustive and adding a
/// level is a compile error at each site that has to decide what the new level means. The
/// project's house style, and the reason it gives: it has caught real bugs.
///
/// **Two variants, because there are two behaviours.** ADR-0058 §6 warned against a
/// surface invented ahead of what is behind it, and a third level running the same four
/// passes would be a name with no content. A W8 sub-wave that adds a pass worth opting
/// into adds a variant here and a `--opt-level` value at the same time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum OptLevel {
    /// No mid-end pass runs: the back end receives exactly what `jr_mir::lower_file`
    /// produced, which is also the MIR that `jr check`'s diagnostics and the `mir` dump
    /// describe.
    ///
    /// What it is *for* is attribution: a wrong answer that survives `-O0` is not the
    /// mid-end's, and this project has had miscompiles on both sides of that line.
    Off,
    /// The pipeline: inline, forward stores, const-prop, DCE, to a bounded fixed point
    /// (ADR-0022 §3).
    ///
    /// The default, and what every build did before the level existed.
    #[default]
    Standard,
}

pub use input::{BuildConfig, SourceFile};

pub use queries::{
    all_diagnostics, build_source_map, lex_file, line_index, parse_diagnostics, parse_file,
};

pub use module_loader::{
    InMemoryModules, ModuleLookupResult, ModuleName, ModuleSearchPaths, ResolveResult,
    file_diagnostics, file_exports, file_hir, frontend_diagnostics, imports_of, module_file,
    resolved,
};

pub use build::{
    BackendChoice, BuildOutput, build_object, declared_build_output, declared_opt_level, entry_of,
};
// **Re-exported so the driver can read `BuildOutput::libraries` without depending on `jr-pool`**
// (ADR-0183 §1). `jr-cli` needs the link *kind* to translate it for `jr-link`, and adding a whole
// dependency for a two-variant enum would widen the driver's reach for nothing.
pub use consts::{ConstResult, file_consts};
pub use jr_codegen::LinkLibrary;
pub use jr_pool::LinkKind;
// Re-exported because `ConstResult::values` is an `Arc<ConstValues>` in this crate's
// public API: a consumer could not name the type it is handed without depending on
// `jr-mir` for nothing else, which is what `jr-lsp` would otherwise have had to do.
pub use docs::{FileDocs, file_docs};
pub use imports::{UnusedImport, UnusedImports, unused_imports};
pub use jr_mir::ConstValues;
pub use mir::{
    MirResult, dump_mir, dump_optimized_mir, file_mir, imported_procs, optimized_file_mir,
};
pub use run::{RunOutcome, main_of, reachable_files, run_main, run_main_with_host};
pub use workspace::{MAX_FILES, WorkspaceFileList, WorkspaceFiles, walk};

pub use sema::{CheckResult, SignatureResult, checked, file_signatures};

use jr_base::{FileId, Interner, SourceMap};
use jr_pool::Pool;
use jr_syntax::LexOutput;
use salsa::Setter as _;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
};

// ---------------------------------------------------------------------------
// The database trait
// ---------------------------------------------------------------------------

/// The trait that all query functions are written against.
///
/// Downstream crates should depend on this trait rather than on
/// [`JairsDatabase`] directly, so that the concrete database type can be
/// swapped (e.g. for a test double) without recompiling every consumer.
#[salsa::db]
pub trait Db: salsa::Database {
    /// Returns the shared string interner.
    fn interner(&self) -> &Interner;

    /// Returns a snapshot of the current source map.
    ///
    /// The map is rebuilt from the salsa inputs on every call, so it always
    /// reflects the current file set. Callers that need a stable snapshot
    /// should clone the returned value.
    fn source_map(&self) -> SourceMap;

    /// Reads the contents of a module file from the filesystem (or an
    /// in-memory substitute for tests).
    ///
    /// Returns `Some(contents)` if the file exists and is readable, `None`
    /// otherwise. Implementations must not panic on filesystem errors.
    ///
    /// This is the seam that lets tests supply an in-memory filesystem
    /// instead of requiring real temporary directories. It is called by
    /// [`module_file`] to probe whether a candidate path exists.
    fn read_module_file(&self, path: &Path) -> Option<String>;

    /// Returns the [`SourceFile`] salsa input for a given path, if it has
    /// already been loaded into the database.
    ///
    /// This is used by [`resolved`] to look up already-loaded module files.
    /// Module files must be pre-loaded (via [`JairsDatabase::load_module`] or
    /// [`JairsDatabase::set_file_text`]) before running resolution queries.
    fn source_file_for_path(&self, path: &str) -> Option<SourceFile>;

    /// Returns the shared type and value pool.
    ///
    /// Held outside salsa's tracking, like the source map, and for a related
    /// reason: a `PoolId` is only meaningful relative to one pool, so every file
    /// analysed by one database must share one. Interning is append-only and
    /// idempotent, which is what makes mutating it inside a tracked query
    /// harmless — see the `sema` module docs for the full argument.
    ///
    /// The lock must never be held across a call into another query.
    fn pool(&self) -> &std::sync::RwLock<Pool>;

    /// A **shared** handle on the pool, with the poison recovered.
    ///
    /// Provided rather than left to each consumer, because `jr-lsp` hand-rolled
    /// `db.pool().lock().unwrap_or_else(|e| e.into_inner())` at four sites — a second copy of a
    /// discipline `run.rs`'s module docs already describe as deliberately centralised. Two copies of
    /// one rule is exactly what this project calls two chances to disagree, and the `RwLock`
    /// conversion (ADR-0149 §1) turned that duplication into four compile errors, which is the good
    /// outcome.
    ///
    /// A **read** guard, because every consumer outside `jr-db` only reads: interning happens in
    /// sema and const-eval, which are inside. A reader taking this while another thread interns
    /// simply waits, and several readers share.
    fn read_pool(&self) -> std::sync::RwLockReadGuard<'_, Pool> {
        match self.pool().read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

// ---------------------------------------------------------------------------
// The concrete database
// ---------------------------------------------------------------------------

/// The concrete salsa database used by both the batch driver and the LSP.
///
/// Construct with [`JairsDatabase::default`] for normal use,
/// [`JairsDatabase::with_event_callback`] to observe query execution (useful
/// for tests and profiling), or [`JairsDatabase::with_in_memory_modules`] to
/// supply an in-memory module filesystem for tests.
#[salsa::db]
pub struct JairsDatabase {
    storage: salsa::Storage<Self>,
    interner: Interner,
    /// Maps path strings to stable [`FileId`]s and stores line-index data.
    /// Kept outside salsa because `FileId` stability is a property of the
    /// path→ID mapping, not of the file contents.
    source_map: Arc<Mutex<SourceMap>>,
    /// Maps path strings to their salsa [`SourceFile`] inputs.
    file_inputs: Arc<Mutex<rustc_hash::FxHashMap<Arc<str>, SourceFile>>>,
    /// Optional in-memory module filesystem for tests.
    ///
    /// When `Some`, `read_module_file` reads from this map instead of the
    /// real filesystem. When `None`, the real filesystem is used.
    in_memory_modules: Option<Arc<InMemoryModules>>,
    /// The current module search paths salsa input, if set.
    module_search_paths: Arc<Mutex<Option<ModuleSearchPaths>>>,
    /// The current workspace file list salsa input, if set (ADR-0029 §2).
    ///
    /// Held here for the same reason as `module_search_paths`: an input has to be created
    /// once and then updated, or every refresh would make a new input and orphan the
    /// dependencies of the old one.
    workspace_files: Arc<Mutex<Option<workspace::WorkspaceFiles>>>,
    /// The current build settings salsa input, if set (ADR-0058 §2).
    ///
    /// Held for the same reason the two above are, and created lazily for one more: most
    /// commands never read it. `jr check` and `jr fmt` have no build settings to apply,
    /// and an input created eagerly would be one every database carried whether or not a
    /// query could reach it.
    build_config: Arc<Mutex<Option<BuildConfig>>>,
    /// The shared interned types and compile-time values.
    ///
    /// Outside salsa for the same reason as `source_map`: it is an identity
    /// table, not an input. Every file analysed by this database interns into it,
    /// which is what makes a type from one file comparable with a type from
    /// another by id alone.
    pool: Arc<RwLock<Pool>>,
}

impl Default for JairsDatabase {
    fn default() -> Self {
        Self {
            storage: salsa::Storage::default(),
            interner: Interner::new(),
            source_map: Arc::new(Mutex::new(SourceMap::new())),
            file_inputs: Arc::new(Mutex::new(rustc_hash::FxHashMap::default())),
            in_memory_modules: None,
            module_search_paths: Arc::new(Mutex::new(None)),
            workspace_files: Arc::new(Mutex::new(None)),
            build_config: Arc::new(Mutex::new(None)),
            pool: Arc::new(RwLock::new(Pool::new())),
        }
    }
}

impl JairsDatabase {
    /// A second handle on the same database, for a reader on another thread.
    ///
    /// Every field is either an `Arc` or salsa's own `Storage`, which is `Clone`, so
    /// this is a field-wise clone and costs nothing. That is not luck: the `Interner`
    /// has been an `Arc<ThreadedRodeo>` since `jr-base` was written "because parsing
    /// and semantic analysis are intended to run in parallel", and `source_map`,
    /// `file_inputs` and `pool` are behind mutexes for identity reasons of their own.
    ///
    /// # The obligation this carries (ADR-0024 §2)
    ///
    /// salsa cancels readers when a writer wants the next revision, and a writer
    /// **blocks until the snapshot count drops back to one**. So a snapshot held
    /// across requests does not merely waste work — it stalls the next edit. Take one
    /// per request and drop it when the request finishes or unwinds.
    #[must_use]
    pub fn snapshot(&self) -> Self {
        Self {
            storage: self.storage.clone(),
            interner: self.interner.clone(),
            source_map: Arc::clone(&self.source_map),
            file_inputs: Arc::clone(&self.file_inputs),
            in_memory_modules: self.in_memory_modules.clone(),
            module_search_paths: Arc::clone(&self.module_search_paths),
            workspace_files: Arc::clone(&self.workspace_files),
            // Shared, not reset. A snapshot that made a fresh `None` here would silently
            // read checks-on while the database it came from had them off — and the LSP is
            // the only snapshot caller, so the divergence would be invisible until
            // something in an editor depended on the setting.
            build_config: Arc::clone(&self.build_config),
            pool: Arc::clone(&self.pool),
        }
    }

    /// Creates a database that calls `callback` for every salsa event.
    ///
    /// The most useful event for testing is [`salsa::EventKind::WillExecute`],
    /// which fires whenever a tracked query actually re-runs (as opposed to
    /// returning a cached result).
    pub fn with_event_callback(callback: impl Fn(salsa::Event) + Send + Sync + 'static) -> Self {
        Self {
            storage: salsa::Storage::new(Some(Box::new(callback))),
            interner: Interner::new(),
            source_map: Arc::new(Mutex::new(SourceMap::new())),
            file_inputs: Arc::new(Mutex::new(rustc_hash::FxHashMap::default())),
            in_memory_modules: None,
            module_search_paths: Arc::new(Mutex::new(None)),
            workspace_files: Arc::new(Mutex::new(None)),
            build_config: Arc::new(Mutex::new(None)),
            pool: Arc::new(RwLock::new(Pool::new())),
        }
    }

    /// Creates a database with an in-memory module filesystem.
    ///
    /// Use this in tests to avoid requiring real temporary directories.
    /// The `modules` map is consulted by `read_module_file` instead of the
    /// real filesystem.
    pub fn with_in_memory_modules(modules: InMemoryModules) -> Self {
        Self {
            storage: salsa::Storage::default(),
            interner: Interner::new(),
            source_map: Arc::new(Mutex::new(SourceMap::new())),
            file_inputs: Arc::new(Mutex::new(rustc_hash::FxHashMap::default())),
            in_memory_modules: Some(Arc::new(modules)),
            module_search_paths: Arc::new(Mutex::new(None)),
            workspace_files: Arc::new(Mutex::new(None)),
            build_config: Arc::new(Mutex::new(None)),
            pool: Arc::new(RwLock::new(Pool::new())),
        }
    }

    /// Creates a database with an event callback AND an in-memory module filesystem.
    ///
    /// Combines [`Self::with_event_callback`] and [`Self::with_in_memory_modules`]
    /// for tests that need both incrementality measurement and in-memory modules.
    pub fn with_event_callback_and_modules(
        callback: impl Fn(salsa::Event) + Send + Sync + 'static,
        modules: InMemoryModules,
    ) -> Self {
        Self {
            storage: salsa::Storage::new(Some(Box::new(callback))),
            interner: Interner::new(),
            source_map: Arc::new(Mutex::new(SourceMap::new())),
            file_inputs: Arc::new(Mutex::new(rustc_hash::FxHashMap::default())),
            in_memory_modules: Some(Arc::new(modules)),
            module_search_paths: Arc::new(Mutex::new(None)),
            workspace_files: Arc::new(Mutex::new(None)),
            build_config: Arc::new(Mutex::new(None)),
            pool: Arc::new(RwLock::new(Pool::new())),
        }
    }

    /// Adds a new file or updates an existing file's text.
    ///
    /// Returns the stable [`FileId`] for the file. The ID is assigned on the
    /// first call for a given path and never changes, even across edits.
    ///
    /// Calling this with the same `text` as the current contents still starts
    /// a new salsa revision (salsa does not compare old and new values before
    /// recording a change). If you need to avoid spurious invalidation, check
    /// whether the text actually changed before calling.
    pub fn set_file_text(
        &mut self,
        path: impl Into<Arc<str>>,
        text: impl Into<Arc<str>>,
    ) -> FileId {
        let path: Arc<str> = path.into();
        let text: Arc<str> = text.into();

        // Update the SourceMap (outside salsa) to keep FileId stable.
        let file_id = {
            let mut sm = self.source_map.lock().expect("source_map lock poisoned");
            sm.add(path.as_ref(), text.as_ref())
        };

        // Check whether we already have a salsa input for this path.
        // We must drop the lock before calling salsa setters (which need &mut self).
        let existing = {
            let inputs = self.file_inputs.lock().expect("file_inputs lock poisoned");
            inputs.get(&path).copied()
        };

        if let Some(existing) = existing {
            existing.set_text(self).to(text);
        } else {
            let input = SourceFile::new(self, path.clone(), text);
            let mut inputs = self.file_inputs.lock().expect("file_inputs lock poisoned");
            inputs.insert(path, input);
        }

        file_id
    }

    /// Returns the salsa [`SourceFile`] input for `path`, if it has been added.
    pub fn source_file(&self, path: &str) -> Option<SourceFile> {
        let inputs = self.file_inputs.lock().expect("file_inputs lock poisoned");
        inputs.get(path).copied()
    }

    /// Returns the [`FileId`] for `path`, if it has been added.
    pub fn file_id(&self, path: &str) -> Option<FileId> {
        let sm = self.source_map.lock().expect("source_map lock poisoned");
        sm.file_id(path)
    }

    /// Sets the module search paths and returns the salsa input.
    ///
    /// Call this before running any module-resolution queries. The paths are
    /// tried in order; the first match wins (ADR-0014 §1).
    ///
    /// Changing the search paths starts a new salsa revision and invalidates
    /// all `module_file` queries and their dependents.
    pub fn set_module_search_paths(&mut self, paths: Vec<PathBuf>) -> ModuleSearchPaths {
        let paths: Arc<[PathBuf]> = paths.into();
        let existing = {
            let guard = self
                .module_search_paths
                .lock()
                .expect("module_search_paths lock poisoned");
            *guard
        };
        if let Some(existing) = existing {
            existing.set_paths(self).to(paths);
            existing
        } else {
            let sp = ModuleSearchPaths::new(self, paths);
            let mut guard = self
                .module_search_paths
                .lock()
                .expect("module_search_paths lock poisoned");
            *guard = Some(sp);
            sp
        }
    }

    /// Sets the build settings and returns the salsa input.
    ///
    /// Changing them starts a new salsa revision and invalidates `optimized_file_mir` and
    /// its dependents, which is the entire reason ADR-0058 §2 made this an input rather
    /// than a parameter the caller remembers to pass consistently.
    ///
    /// Updates the existing input rather than making a new one, for the reason
    /// `workspace_files` records: a fresh input each time would orphan the dependencies
    /// of the old one, so nothing would be invalidated and the change would appear to
    /// have no effect.
    pub fn set_build_config(&mut self, bounds_checks: bool, opt_level: OptLevel) -> BuildConfig {
        let existing = {
            let guard = self
                .build_config
                .lock()
                .expect("build_config lock poisoned");
            *guard
        };
        if let Some(existing) = existing {
            existing.set_bounds_checks(self).to(bounds_checks);
            existing.set_opt_level(self).to(opt_level);
            existing
        } else {
            let config = BuildConfig::new(self, bounds_checks, opt_level);
            let mut guard = self
                .build_config
                .lock()
                .expect("build_config lock poisoned");
            *guard = Some(config);
            config
        }
    }

    /// The build settings, creating them with bounds checks **on** and the mid-end
    /// **on** if unset.
    ///
    /// The default is checks-on and optimised, which is what every consumer that does not
    /// care should get: an editor, a test harness and `jr check` all want the program as
    /// written. ADR-0058 §2 is explicit that only `jr run` and `jr build` take the
    /// bounds-check flag, and ADR-0142 §1 gives the optimisation level the same rule.
    pub fn build_config(&mut self) -> BuildConfig {
        let existing = {
            let guard = self
                .build_config
                .lock()
                .expect("build_config lock poisoned");
            *guard
        };
        existing.unwrap_or_else(|| self.set_build_config(true, OptLevel::Standard))
    }

    /// Walks `roots` and records the result as the workspace file list.
    ///
    /// The walk happens **here**, outside any query, which is ADR-0029 §2's whole point: a
    /// directory listing is untracked I/O, so it belongs on the input side of the database
    /// rather than inside a query that salsa would then believe it could cache.
    ///
    /// Call it again to refresh — on a file-watcher notification, or on `didSave` for a
    /// client that cannot watch. Updating the existing input is what lets salsa invalidate
    /// exactly the queries that consulted the old list.
    pub fn set_workspace_roots(&mut self, roots: &[PathBuf]) -> workspace::WorkspaceFiles {
        self.set_workspace_files(Arc::new(workspace::walk(roots)))
    }

    /// Records an already-computed file list.
    ///
    /// Separate from [`Self::set_workspace_roots`] so that a test can supply a list without
    /// touching a filesystem, and so that a caller which already walked (a watcher handler
    /// with a delta) need not walk again.
    pub fn set_workspace_files(
        &mut self,
        list: Arc<workspace::WorkspaceFileList>,
    ) -> workspace::WorkspaceFiles {
        let existing = {
            let guard = self
                .workspace_files
                .lock()
                .expect("workspace_files lock poisoned");
            *guard
        };
        if let Some(existing) = existing {
            existing.set_list(self).to(list);
            existing
        } else {
            let files = workspace::WorkspaceFiles::new(self, list);
            let mut guard = self
                .workspace_files
                .lock()
                .expect("workspace_files lock poisoned");
            *guard = Some(files);
            files
        }
    }

    /// Reads every workspace file that is not yet in the database.
    ///
    /// ADR-0029 §3: discovery yields *paths*, and a path is not a `SourceFile`. Anything
    /// that must see the whole workspace — a rename, a reference search, a symbol query —
    /// has to call this first, or it will scan only the handful of files an editor happens
    /// to have opened and report a confident wrong answer.
    ///
    /// **This is where the cost the ADR promised lands**: the first call reads and later
    /// queries parse every file in the workspace. Returns how many files it read, so a
    /// caller can log or test that.
    ///
    /// A file that cannot be read is skipped. An unreadable file is not a reason to refuse
    /// a rename — it is a reason for it not to be in the list of files a rename touches,
    /// which is what skipping achieves.
    pub fn load_workspace_files(&mut self) -> usize {
        let Some(files) = self.workspace_files() else {
            return 0;
        };
        let list = files.list(self);
        let mut read = 0;
        for path in list.files.iter() {
            let key = path.to_string_lossy();
            if self.source_file(key.as_ref()).is_some() {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(path) {
                self.set_file_text(key.as_ref(), text.as_str());
                read += 1;
            }
        }
        read
    }

    /// The workspace file list, if one has been set.
    ///
    /// `None` means discovery has not run — which a consumer must distinguish from an
    /// empty workspace, because "I do not know" and "there are no other files" lead to
    /// different answers for a rename.
    #[must_use]
    pub fn workspace_files(&self) -> Option<workspace::WorkspaceFiles> {
        *self
            .workspace_files
            .lock()
            .expect("workspace_files lock poisoned")
    }

    /// Returns the current [`ModuleSearchPaths`] salsa input, if set.
    pub fn module_search_paths_input(&self) -> Option<ModuleSearchPaths> {
        let guard = self
            .module_search_paths
            .lock()
            .expect("module_search_paths lock poisoned");
        *guard
    }

    /// Loads a module file into the database from the filesystem (or the
    /// in-memory map if configured).
    ///
    /// Returns the [`SourceFile`] salsa input for the file, or `None` if the
    /// file could not be read.
    ///
    /// This is the `&mut self` entry point for pre-loading module files before
    /// running resolution queries. The batch driver and LSP call this to
    /// ensure all transitively imported modules are in the database before
    /// calling [`resolved`] or [`file_diagnostics`].
    pub fn load_module(&mut self, path: &Path) -> Option<SourceFile> {
        let path_str: Arc<str> = path.to_string_lossy().into_owned().into();

        // Check if already loaded.
        {
            let inputs = self.file_inputs.lock().expect("file_inputs lock poisoned");
            if let Some(&sf) = inputs.get(&path_str) {
                return Some(sf);
            }
        }

        // Read the file content.
        let content = if let Some(ref mem) = self.in_memory_modules {
            mem.get(path)?.to_owned()
        } else {
            std::fs::read_to_string(path).ok()?
        };

        let text: Arc<str> = content.into();

        // Update the SourceMap.
        {
            let mut sm = self.source_map.lock().expect("source_map lock poisoned");
            sm.add(path_str.as_ref(), text.as_ref());
        }

        // Create the salsa input.
        let sf = SourceFile::new(self, path_str.clone(), text);
        {
            let mut inputs = self.file_inputs.lock().expect("file_inputs lock poisoned");
            inputs.insert(path_str, sf);
        }

        Some(sf)
    }

    /// Loads all modules transitively imported by a file, recursively.
    ///
    /// This is a convenience method for the batch driver: call it after
    /// setting up search paths and loading the root file(s) to ensure all
    /// transitively imported modules are in the database before running
    /// resolution queries.
    ///
    /// Cycles are handled correctly: a module that has already been loaded
    /// (or is currently being loaded) is not loaded again.
    ///
    /// Returns the set of all module paths that were loaded.
    pub fn load_modules_transitively(&mut self, root: SourceFile) -> Vec<PathBuf> {
        let search_paths = match self.module_search_paths_input() {
            Some(sp) => sp,
            None => return Vec::new(),
        };

        let mut loaded = Vec::new();
        let mut visited: rustc_hash::FxHashSet<String> = rustc_hash::FxHashSet::default();
        let mut queue: Vec<SourceFile> = vec![root];

        while let Some(file) = queue.pop() {
            let import_names = imports_of(self, file);
            for name in import_names.iter() {
                if !visited.insert(name.to_string()) {
                    continue; // Already processed this module name.
                }

                let lookup = module_file(self, search_paths, name.clone());
                if let Some(found_path) = lookup.found
                    && let Some(module_sf) = self.load_module(&found_path)
                {
                    loaded.push(found_path);
                    queue.push(module_sf);
                }
            }
        }

        loaded
    }
}

#[salsa::db]
impl Db for JairsDatabase {
    fn interner(&self) -> &Interner {
        &self.interner
    }

    fn source_map(&self) -> SourceMap {
        self.source_map
            .lock()
            .expect("source_map lock poisoned")
            .clone()
    }

    fn read_module_file(&self, path: &Path) -> Option<String> {
        if let Some(ref mem) = self.in_memory_modules {
            mem.get(path).map(|s| s.to_owned())
        } else {
            std::fs::read_to_string(path).ok()
        }
    }

    fn source_file_for_path(&self, path: &str) -> Option<SourceFile> {
        let inputs = self.file_inputs.lock().expect("file_inputs lock poisoned");
        inputs.get(path).copied()
    }

    fn pool(&self) -> &RwLock<Pool> {
        &self.pool
    }
}

#[salsa::db]
impl salsa::Database for JairsDatabase {}

// ---------------------------------------------------------------------------
// Re-export the output types callers need
// ---------------------------------------------------------------------------

/// The result of lexing a file, wrapped in an [`Arc`] for cheap cloning.
///
/// Salsa requires return values to be `Clone`. We wrap [`LexOutput`] in
/// `Arc` because `LexOutput` itself is not `Eq` (it contains `Diagnostics`
/// which are not `Eq`). The queries use `no_eq` to disable backdating.
pub type ArcLexOutput = Arc<LexOutput>;

/// The result of parsing a file, wrapped in an [`Arc`] for cheap cloning.
///
/// [`jr_syntax::Parse`] contains a `rowan::GreenNode` (reference-counted) and
/// `Diagnostics`. Neither is `Eq`, so we wrap in `Arc` and use `no_eq`.
pub type ArcParse = Arc<jr_syntax::Parse>;

/// A set of diagnostics, wrapped in an [`Arc`] for cheap cloning.
pub type ArcDiagnostics = Arc<jr_diag::Diagnostics>;

/// A line index for a file: maps byte offsets to line/column positions.
///
/// This is a thin wrapper around the data already in [`jr_base::SourceFile`],
/// extracted into its own query so that consumers needing only line/column
/// information do not depend on the parse tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineIndex {
    /// Byte offset of the start of each line. Always begins with 0.
    pub line_starts: Vec<u32>,
}

impl LineIndex {
    /// Converts a byte offset to a 1-based line and column.
    ///
    /// Offsets past the end of the file clamp to the final position.
    ///
    /// Note: the column returned here is byte-based, not character-based.
    /// For character-based columns, use [`jr_base::SourceFile::line_col`].
    #[must_use]
    pub fn line_col(&self, offset: u32) -> jr_base::LineCol {
        let line_index = self.line_starts.partition_point(|&start| start <= offset) - 1;
        let line_start = self.line_starts[line_index];
        jr_base::LineCol {
            line: line_index as u32 + 1,
            col: offset - line_start + 1,
        }
    }
}
