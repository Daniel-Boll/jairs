//! Core diagnostic types: [`Severity`], [`Label`], [`InstantiationFrame`], and [`Diagnostic`].

use jr_base::Span;

// ---------------------------------------------------------------------------
// Severity
// ---------------------------------------------------------------------------

/// The severity of a diagnostic.
///
/// Ordered so that `Error > Warning > Note > Help`, which lets callers use
/// `max()` to find the worst severity in a collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// A suggestion that may improve the code but is not required.
    Help,
    /// Informational annotation, not a problem.
    Note,
    /// A potential problem that does not prevent compilation.
    Warning,
    /// A hard error that prevents the program from being compiled.
    Error,
}

// ---------------------------------------------------------------------------
// Label
// ---------------------------------------------------------------------------

/// A span annotation: a source range with an optional explanatory message.
///
/// Labels are used for both the primary span of a [`Diagnostic`] and any
/// secondary spans that provide additional context.
#[derive(Debug, Clone)]
pub struct Label {
    /// The source span this label points at.
    pub span: Span,
    /// An optional message rendered under the caret.
    pub message: Option<String>,
}

impl Label {
    /// Creates a label with no message.
    #[must_use]
    pub fn new(span: Span) -> Self {
        Self {
            span,
            message: None,
        }
    }

    /// Creates a label with a message.
    #[must_use]
    pub fn with_message(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: Some(message.into()),
        }
    }
}

// ---------------------------------------------------------------------------
// InstantiationFrame
// ---------------------------------------------------------------------------

/// One frame of a polymorph instantiation backtrace.
///
/// When a generic procedure is instantiated with concrete type arguments, the
/// compiler records a chain of these frames so that errors inside the
/// instantiation can be traced back to the call site.
///
/// This type is defined now (in the vertical slice) even though polymorphs
/// land in wave W5, because retrofitting instantiation backtraces after the
/// fact is a known failure mode (see PLAN.md §5).
#[derive(Debug, Clone)]
pub struct InstantiationFrame {
    /// The call-site span that triggered this instantiation.
    pub span: Span,
    /// Human-readable description, e.g. `"in instantiation of sort($T = Entity)"`.
    pub description: String,
}

impl InstantiationFrame {
    /// Creates a new instantiation frame.
    #[must_use]
    pub fn new(span: Span, description: impl Into<String>) -> Self {
        Self {
            span,
            description: description.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Diagnostic
// ---------------------------------------------------------------------------

/// A complete diagnostic: a severity, headline message, primary span,
/// optional secondary spans, trailing notes/help lines, and an optional
/// instantiation backtrace.
///
/// Build diagnostics with the [`Diagnostic::error`] / [`Diagnostic::warning`]
/// constructors and the chainable builder methods.
///
/// # Example
///
/// ```
/// # use jr_base::{SourceMap, Span, FileId};
/// # use jr_diag::{Diagnostic, Label};
/// # let mut map = SourceMap::new();
/// # let fid = map.add("a.jr", "let x = 1;");
/// # let span = Span::from_offsets(fid, 4, 5);
/// let diag = Diagnostic::error(span, "type mismatch")
///     .with_code("E0001")
///     .with_note("expected `s64`, found `bool`");
/// ```
#[derive(Debug, Clone)]
pub struct Diagnostic {
    /// How severe this diagnostic is.
    pub severity: Severity,
    /// A stable error code like `"E0001"`, if any.
    pub code: Option<&'static str>,
    /// The headline message shown on the first line.
    pub message: String,
    /// The primary span — the main location the diagnostic points at.
    pub primary: Label,
    /// Additional spans providing context, possibly in other files.
    pub secondary: Vec<Label>,
    /// Trailing note and help lines.
    pub notes: Vec<(Severity, String)>,
    /// Instantiation backtrace frames, printed as a trailing chain.
    pub backtrace: Vec<InstantiationFrame>,
}

impl Diagnostic {
    /// Creates an error diagnostic.
    #[must_use]
    pub fn error(span: Span, message: impl Into<String>) -> Self {
        Self::new(Severity::Error, span, message)
    }

    /// Creates a warning diagnostic.
    #[must_use]
    pub fn warning(span: Span, message: impl Into<String>) -> Self {
        Self::new(Severity::Warning, span, message)
    }

    fn new(severity: Severity, span: Span, message: impl Into<String>) -> Self {
        Self {
            severity,
            code: None,
            message: message.into(),
            primary: Label::new(span),
            secondary: Vec::new(),
            notes: Vec::new(),
            backtrace: Vec::new(),
        }
    }

    /// Attaches a stable error code.
    #[must_use]
    pub fn with_code(mut self, code: &'static str) -> Self {
        self.code = Some(code);
        self
    }

    /// Adds a secondary label (additional context span).
    #[must_use]
    pub fn with_label(mut self, label: Label) -> Self {
        self.secondary.push(label);
        self
    }

    /// Adds a trailing note line.
    #[must_use]
    pub fn with_note(mut self, message: impl Into<String>) -> Self {
        self.notes.push((Severity::Note, message.into()));
        self
    }

    /// Adds a trailing help line.
    #[must_use]
    pub fn with_help(mut self, message: impl Into<String>) -> Self {
        self.notes.push((Severity::Help, message.into()));
        self
    }

    /// Adds an instantiation backtrace frame.
    #[must_use]
    pub fn with_frame(mut self, frame: InstantiationFrame) -> Self {
        self.backtrace.push(frame);
        self
    }
}
