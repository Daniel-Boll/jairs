//! The salsa query database — the single source of truth shared by the batch
//! driver and the language server (ADR-0007).
//!
//! # Architecture
//!
//! This crate is a thin **wiring layer**: it defines salsa inputs for file
//! contents and tracked queries that call the pure functions in `jr-syntax`.
//! No lexing or parsing logic lives here; `jr-syntax` remains salsa-free.
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

mod queries;

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

use jr_base::{FileId, Interner, SourceMap};
use jr_syntax::LexOutput;
use salsa::Setter as _;
use std::sync::{Arc, Mutex};

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
}

// ---------------------------------------------------------------------------
// The concrete database
// ---------------------------------------------------------------------------

/// The concrete salsa database used by both the batch driver and the LSP.
///
/// Construct with [`JairsDatabase::default`] for normal use, or with
/// [`JairsDatabase::with_event_callback`] to observe query execution (useful
/// for tests and profiling).
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
}

impl Default for JairsDatabase {
    fn default() -> Self {
        Self {
            storage: salsa::Storage::default(),
            interner: Interner::new(),
            source_map: Arc::new(Mutex::new(SourceMap::new())),
            file_inputs: Arc::new(Mutex::new(rustc_hash::FxHashMap::default())),
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
