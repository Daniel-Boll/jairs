//! The [`Diagnostics`] sink: collects, queries, and sorts diagnostics.

use crate::diagnostic::{Diagnostic, Severity};

/// A sink that collects [`Diagnostic`]s produced during compilation.
///
/// Diagnostics are stored in insertion order; call [`Diagnostics::sorted`] to
/// get them in deterministic source order.
#[derive(Debug, Clone, Default)]
pub struct Diagnostics {
    inner: Vec<Diagnostic>,
}

impl Diagnostics {
    /// Creates an empty sink.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a single diagnostic.
    pub fn push(&mut self, diag: Diagnostic) {
        self.inner.push(diag);
    }

    /// Adds all diagnostics from an iterator.
    pub fn extend(&mut self, iter: impl IntoIterator<Item = Diagnostic>) {
        self.inner.extend(iter);
    }

    /// Returns `true` if no diagnostics have been collected.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// The number of collected diagnostics.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns `true` if any collected diagnostic has [`Severity::Error`].
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.inner.iter().any(|d| d.severity == Severity::Error)
    }

    /// Iterates over diagnostics in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.inner.iter()
    }

    /// Consumes the sink and returns the diagnostics as a `Vec`.
    #[must_use]
    pub fn into_vec(self) -> Vec<Diagnostic> {
        self.inner
    }

    /// Returns diagnostics sorted by `(primary.span, severity)` so that
    /// output is deterministic regardless of the order analysis produced them.
    ///
    /// Severity is sorted descending (errors first) within the same span.
    #[must_use]
    pub fn sorted(&self) -> Vec<&Diagnostic> {
        let mut v: Vec<&Diagnostic> = self.inner.iter().collect();
        v.sort_by(|a, b| {
            a.primary
                .span
                .cmp(&b.primary.span)
                // Within the same span, errors before warnings before notes.
                .then_with(|| b.severity.cmp(&a.severity))
        });
        v
    }
}
