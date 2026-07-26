//! Tracked query functions: the salsa-wired calls into `jr-syntax`.
//!
//! Each function here is a thin wrapper that calls a pure function from
//! `jr-syntax` and wraps the result in an `Arc` so salsa can store and
//! compare it. No parsing or lexing logic lives here.
//!
//! # Return-value bounds
//!
//! Salsa's `#[tracked]` functions require return values to implement `Clone`.
//! By default they also require `PartialEq` for backdating (if the result
//! hasn't changed, dependents are not invalidated). We use `no_eq` on the
//! three queries whose return types do not implement `PartialEq`:
//!
//! - [`lex_file`]: `Arc<LexOutput>` — `LexOutput` contains `Diagnostics`
//!   which is not `Eq`.
//! - [`parse_file`]: `Arc<Parse>` — `Parse` contains a `GreenNode` and
//!   `Diagnostics`, neither of which is `Eq`.
//! - [`parse_diagnostics`]: `Arc<Diagnostics>` — same reason.
//!
//! `no_eq` means salsa will always propagate invalidation when the input
//! changes, even if the output is semantically identical. This is conservative
//! but correct. [`line_index`] returns a `LineIndex` that *does* implement
//! `PartialEq`, so it gets full backdating.

use std::sync::Arc;

use jr_base::FileId;
use jr_diag::Diagnostics;

use crate::{ArcDiagnostics, ArcLexOutput, ArcParse, Db, LineIndex, SourceFile};

// ---------------------------------------------------------------------------
// lex_file
// ---------------------------------------------------------------------------

/// Lexes the source file and returns all tokens plus any lex-time diagnostics.
///
/// This is a separate query from [`parse_file`] so that consumers needing only
/// the token stream (e.g. a syntax highlighter) do not pay for parsing.
///
/// Uses `no_eq` because [`jr_syntax::LexOutput`] does not implement
/// [`PartialEq`].
#[salsa::tracked(returns(clone), no_eq)]
pub fn lex_file(db: &dyn Db, file: SourceFile) -> ArcLexOutput {
    let text = file.text(db);
    let file_id = resolve_file_id(db, file);
    Arc::new(jr_syntax::lex(text.as_ref(), file_id))
}

// ---------------------------------------------------------------------------
// parse_file
// ---------------------------------------------------------------------------

/// Parses the source file and returns the lossless CST.
///
/// The returned [`jr_syntax::Parse`] contains the green tree and any
/// parse-time diagnostics (including lex diagnostics, since the parser calls
/// the lexer internally).
///
/// Uses `no_eq` because [`jr_syntax::Parse`] does not implement [`PartialEq`].
#[salsa::tracked(returns(clone), no_eq)]
pub fn parse_file(db: &dyn Db, file: SourceFile) -> ArcParse {
    let text = file.text(db);
    let file_id = resolve_file_id(db, file);
    Arc::new(jr_syntax::parse(text.as_ref(), file_id))
}

// ---------------------------------------------------------------------------
// parse_diagnostics
// ---------------------------------------------------------------------------

/// Returns only the diagnostics from parsing, without the CST.
///
/// This is a separate query from [`parse_file`] so that a consumer wanting
/// only error messages (e.g. `jr check`) does not depend on the tree and is
/// not invalidated when the tree changes but the diagnostics do not.
///
/// Uses `no_eq` because [`jr_diag::Diagnostics`] does not implement
/// [`PartialEq`].
#[salsa::tracked(returns(clone), no_eq)]
pub fn parse_diagnostics(db: &dyn Db, file: SourceFile) -> ArcDiagnostics {
    // We call parse_file so that the parse result is shared; the diagnostics
    // are extracted from the already-memoized parse.
    let parse = parse_file(db, file);
    Arc::new(parse.diagnostics().clone())
}

// ---------------------------------------------------------------------------
// line_index
// ---------------------------------------------------------------------------

/// Builds a line index for the file: a sorted list of byte offsets at which
/// each line starts.
///
/// This is a separate query so that span→line/col conversion does not depend
/// on the parse tree. The LSP needs this for every hover/goto request.
///
/// [`LineIndex`] implements [`PartialEq`], so this query gets full backdating:
/// if the line structure of a file does not change (e.g. only a character on
/// an existing line was edited), dependents are not invalidated.
#[salsa::tracked(returns(clone))]
pub fn line_index(db: &dyn Db, file: SourceFile) -> LineIndex {
    let text = file.text(db);
    let mut starts = vec![0u32];
    starts.extend(
        text.bytes()
            .enumerate()
            .filter(|&(_, b)| b == b'\n')
            .map(|(i, _)| i as u32 + 1),
    );
    LineIndex {
        line_starts: starts,
    }
}

// ---------------------------------------------------------------------------
// Helper: resolve FileId from the SourceMap
// ---------------------------------------------------------------------------

/// Resolves the [`FileId`] for a salsa [`SourceFile`] input.
///
/// The `SourceMap` is the authoritative source of `FileId`s. We look up the
/// path in the map; if it is not there yet (which should not happen in normal
/// use, since `set_file_text` always updates the map before creating the
/// salsa input), we fall back to `FileId::from_usize(0)` rather than
/// panicking, because panicking inside a tracked query would poison the memo.
pub(crate) fn resolve_file_id(db: &dyn Db, file: SourceFile) -> FileId {
    let path = file.path(db);
    let sm = db.source_map();
    sm.file_id(path.as_ref())
        .unwrap_or_else(|| FileId::from_usize(0))
}

// ---------------------------------------------------------------------------
// Convenience: build a SourceMap for the current file set
// ---------------------------------------------------------------------------

/// Builds a [`jr_base::SourceMap`] reflecting all files currently in the
/// database.
///
/// This is a convenience for callers that need to render diagnostics via
/// [`jr_diag::Renderer`], which requires a `SourceMap` to resolve spans.
///
/// The returned map is a snapshot; it does not update when files change.
#[must_use]
pub fn build_source_map(db: &dyn Db) -> jr_base::SourceMap {
    db.source_map()
}

/// Collects all diagnostics from all files currently in the database.
///
/// Useful for batch-mode `jr check`: call this after adding all files to get
/// a single sorted list of every diagnostic.
#[must_use]
pub fn all_diagnostics(db: &dyn Db, files: &[SourceFile]) -> Diagnostics {
    let mut all = Diagnostics::new();
    for &file in files {
        let diags = parse_diagnostics(db, file);
        all.extend(diags.iter().cloned());
    }
    all
}
