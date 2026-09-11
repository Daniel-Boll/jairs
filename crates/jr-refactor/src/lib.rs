//! Semantic source refactorings.
//!
//! This crate is deliberately protocol-neutral. It consumes the compiler's lossless CST,
//! resolved HIR and inferred types, then returns byte edits against the exact source snapshot it
//! analysed. The `jr-lsp` crate is the adapter that converts those edits to negotiated LSP
//! positions.
//!
//! [ADR-0246](../../../docs/adr/0246-semantic-extraction-and-editor-highlighting.md) is the module's
//! interface contract.

mod extract;

use jr_base::TextRange;

pub use extract::extract;

/// Which extraction a candidate performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionKind {
    /// Introduce a local for one selected expression.
    Local,
    /// Move selected statements into a nested procedure.
    Procedure,
}

/// The extraction kinds requested by a caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtractKinds(u8);

impl ExtractKinds {
    /// Request both local and procedure extraction.
    pub const ALL: Self = Self(0b11);
    /// Request only local extraction.
    pub const LOCAL: Self = Self(0b01);
    /// Request only procedure extraction.
    pub const PROCEDURE: Self = Self(0b10);

    const fn contains(self, kind: ExtractionKind) -> bool {
        let bit = match kind {
            ExtractionKind::Local => 0b01,
            ExtractionKind::Procedure => 0b10,
        };
        self.0 & bit != 0
    }
}

/// One extraction request against one immutable source snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractRequest {
    /// The selected byte range.
    pub selection: TextRange,
    /// Which actions to attempt.
    pub kinds: ExtractKinds,
    /// One indentation unit, usually two spaces or one tab.
    pub indent_unit: String,
}

impl ExtractRequest {
    /// Requests every extraction kind with a two-space indentation unit.
    #[must_use]
    pub fn all(selection: TextRange) -> Self {
        Self {
            selection,
            kinds: ExtractKinds::ALL,
            indent_unit: String::from("  "),
        }
    }
}

/// One source edit against the analysed snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceEdit {
    /// Bytes to replace.
    pub range: TextRange,
    /// Replacement text.
    pub replacement: String,
}

/// A complete atomic source transformation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceChange {
    /// Sorted, non-overlapping edits.
    pub edits: Vec<SourceEdit>,
}

/// One immediately applicable extraction candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extraction {
    /// Editor-facing action title.
    pub title: String,
    /// The extraction family.
    pub kind: ExtractionKind,
    /// Exact byte edits.
    pub change: SourceChange,
}

/// Why one requested extraction kind was not offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The selection is empty or does not exactly identify supported source.
    InvalidSelection,
    /// Current compiler errors make a semantic rewrite unreliable.
    ExistingErrors,
    /// The selected expression cannot be moved without changing evaluation.
    UnsafeEvaluationOrder,
    /// The statements are not consecutive children of one block.
    NonContiguousStatements,
    /// A required source type cannot be spelled in this file.
    UnspellableType,
    /// Moving a defer would change its lexical lifetime.
    DeferLifetimeWouldChange,
    /// Generated or expanded source has no stable editable identity.
    GeneratedSource,
    /// The selected construct has no semantics-preserving extraction yet.
    UnsupportedConstruct,
}

/// Candidates and honest refusals for one request.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExtractionReport {
    /// Applicable actions.
    pub candidates: Vec<Extraction>,
    /// One refusal per requested kind that was not applicable.
    pub refusals: Vec<(ExtractionKind, Refusal)>,
}
