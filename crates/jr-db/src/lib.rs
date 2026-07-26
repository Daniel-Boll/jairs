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

pub mod mir;
pub mod module_loader;
mod queries;
pub mod sema;

// The salsa macro generates undocumented associated functions (new, field
// getters, field setters). We allow missing_docs for the module that contains
// the generated code rather than for the whole crate.
#[allow(missing_docs)]
mod input {
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
}

pub use input::SourceFile;

pub use queries::{
    all_diagnostics, build_source_map, lex_file, line_index, parse_diagnostics, parse_file,
};

pub use module_loader::{
    InMemoryModules, ModuleLookupResult, ModuleName, ModuleSearchPaths, ResolveResult,
    file_diagnostics, file_exports, file_hir, frontend_diagnostics, imports_of, module_file,
    resolved,
};

pub use mir::{MirResult, dump_mir, file_mir};

pub use sema::{CheckResult, SignatureResult, checked, file_signatures};

use jr_base::{FileId, Interner, SourceMap};
use jr_pool::Pool;
use jr_syntax::LexOutput;
use salsa::Setter as _;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
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
    fn pool(&self) -> &std::sync::Mutex<Pool>;
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
    /// The shared interned types and compile-time values.
    ///
    /// Outside salsa for the same reason as `source_map`: it is an identity
    /// table, not an input. Every file analysed by this database interns into it,
    /// which is what makes a type from one file comparable with a type from
    /// another by id alone.
    pool: Arc<Mutex<Pool>>,
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
            pool: Arc::new(Mutex::new(Pool::new())),
        }
    }
}

impl JairsDatabase {
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
            pool: Arc::new(Mutex::new(Pool::new())),
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
            pool: Arc::new(Mutex::new(Pool::new())),
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
            pool: Arc::new(Mutex::new(Pool::new())),
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
                if let Some(found_path) = lookup.found {
                    if let Some(module_sf) = self.load_module(&found_path) {
                        loaded.push(found_path);
                        queue.push(module_sf);
                    }
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

    fn pool(&self) -> &Mutex<Pool> {
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
