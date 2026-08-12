//! The [`Diagnostics`] sink: collects, queries, and sorts diagnostics.

use crate::diagnostic::{Diagnostic, InstantiationFrame, Severity};

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

    /// Attaches `frames` to every diagnostic pushed at or after `watermark`.
    ///
    /// # Why a watermark rather than mutable iteration
    ///
    /// An instantiation's diagnostics are produced by the ordinary checker, at hundreds of
    /// `push` sites that know nothing about polymorphism. Threading a frame to each would
    /// touch every one of them and be forgotten at the next; recording [`Diagnostics::len`]
    /// before a body is checked and stamping everything added since is one call at the one
    /// place that knows the body *is* an instantiation.
    ///
    /// A public `iter_mut` would do this too, and would widen the sink's API to every
    /// consumer so one caller could stamp a field — the trade ADR-0123 refused when it
    /// declined a `pub const CODES` for a test's convenience. This method says what it is
    /// for, so nothing else can quietly depend on mutating a collected diagnostic.
    ///
    /// Frames already present are kept and the new ones appended after them, so an inner
    /// instantiation's frame stays innermost. A `watermark` past the end is not an error: a
    /// body that produced no diagnostics stamps nothing.
    pub fn attach_frames_since(&mut self, watermark: usize, frames: &[InstantiationFrame]) {
        if frames.is_empty() {
            return;
        }
        for diag in self.inner.iter_mut().skip(watermark) {
            diag.backtrace.extend(frames.iter().cloned());
        }
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
