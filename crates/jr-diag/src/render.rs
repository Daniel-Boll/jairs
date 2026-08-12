//! The [`Renderer`] that turns [`Diagnostic`]s into human-readable text.
//!
//! Uses `annotate-snippets` 0.12 to produce rustc-identical output.

use annotate_snippets::{AnnotationKind, Group, Level, Renderer as SnippetRenderer, Snippet};
use jr_base::SourceMap;

use crate::diagnostic::{Diagnostic, InstantiationFrame, Severity};
use crate::sink::Diagnostics;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration for the [`Renderer`].
#[derive(Debug, Clone)]
pub struct Config {
    /// Whether to emit ANSI colour codes. Default: `false` (deterministic for
    /// snapshot tests).
    pub colour: bool,
    /// Whether to render the instantiation backtrace. Default: `true`.
    pub show_backtrace: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            colour: false,
            show_backtrace: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Renderer
// ---------------------------------------------------------------------------

/// Renders [`Diagnostic`]s to text using `annotate-snippets`.
///
/// Construct with [`Renderer::new`] (plain, no colour) or configure via
/// [`Config`].
#[derive(Debug, Clone)]
pub struct Renderer {
    config: Config,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    /// Creates a renderer with default settings (no colour, backtrace on).
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: Config::default(),
        }
    }

    /// Creates a renderer from an explicit [`Config`].
    #[must_use]
    pub fn with_config(config: Config) -> Self {
        Self { config }
    }

    /// Renders a single [`Diagnostic`] to a `String`.
    ///
    /// Spans are resolved through `map`. Offsets past EOF are clamped by
    /// `jr-base`'s `SourceFile::line_col`, so this method never panics on
    /// out-of-range spans.
    #[must_use]
    pub fn render(&self, map: &SourceMap, diag: &Diagnostic) -> String {
        let renderer = self.make_snippet_renderer();
        // Materialise all string data that annotate-snippets will borrow.
        let data = DiagData::collect(map, diag);
        let groups = data.build_groups(diag);
        let output = renderer.render(groups.as_slice());
        // Append instantiation backtrace as plain text after the snippet output.
        if self.config.show_backtrace && !diag.backtrace.is_empty() {
            let mut s = output;
            // `annotate-snippets` does not end its output with a newline, so without this the first
            // frame was glued onto the caret line: "^^^^^  note: in instantiation of ...". Added here
            // rather than inside `render_backtrace`, which is also called where the separator differs.
            if !s.is_empty() && !s.ends_with('\n') {
                s.push('\n');
            }
            s.push_str(&render_backtrace(map, &diag.backtrace));
            s
        } else {
            output
        }
    }

    /// Renders all diagnostics in `diags` to a single `String`, separated by
    /// newlines.
    #[must_use]
    pub fn render_all(&self, map: &SourceMap, diags: &Diagnostics) -> String {
        diags
            .iter()
            .map(|d| self.render(map, d))
            .collect::<Vec<_>>()
            .join("\n")
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    fn make_snippet_renderer(&self) -> SnippetRenderer {
        if self.config.colour {
            SnippetRenderer::styled()
        } else {
            SnippetRenderer::plain()
        }
    }
}

// ---------------------------------------------------------------------------
// DiagData — owns all strings that annotate-snippets borrows
// ---------------------------------------------------------------------------

/// Owns all the string data (file names, source text) that `annotate-snippets`
/// will borrow during rendering. This sidesteps the lifetime issue where
/// `SourceFile::name()` returns a `Cow` that doesn't live long enough.
struct DiagData {
    /// (file_name, source_text, line_start_for_snippet)
    primary: FileData,
    /// Cross-file secondary labels, grouped by file.
    cross_file: Vec<FileData>,
    /// Notes/help messages.
    notes: Vec<(Severity, String)>,
}

struct FileData {
    name: String,
    source: String,
    line_start: usize,
    /// Annotations: (byte_start, byte_end, optional_label)
    annotations: Vec<(usize, usize, Option<String>)>,
    /// Whether the first annotation is Primary (vs Context).
    first_is_primary: bool,
}

impl DiagData {
    fn collect(map: &SourceMap, diag: &Diagnostic) -> Self {
        let primary_file = map.file(diag.primary.span.file);
        let primary_source = primary_file.text();
        let primary_len = primary_source.len();

        let primary_start = (u32::from(diag.primary.span.start()) as usize).min(primary_len);
        let primary_end = (u32::from(diag.primary.span.end()) as usize).min(primary_len);
        // `annotate-snippets` interprets `line_start` as the line number of the
        // FIRST line of the source it is handed. We hand it the whole file, so
        // that is line 1. Passing the primary span's line here instead shifts
        // every rendered line number by (line - 1), which silently breaks both
        // human reading and editor jump-to-error.
        let primary_line = 1usize;
        debug_assert!(
            primary_file.line_col(diag.primary.span.start()).line >= 1,
            "line numbers are 1-based"
        );

        let mut primary_annotations: Vec<(usize, usize, Option<String>)> =
            vec![(primary_start, primary_end, diag.primary.message.clone())];

        // Same-file secondary labels.
        for sec in &diag.secondary {
            if sec.span.file == diag.primary.span.file {
                let s = (u32::from(sec.span.start()) as usize).min(primary_len);
                let e = (u32::from(sec.span.end()) as usize).min(primary_len);
                primary_annotations.push((s, e, sec.message.clone()));
            }
        }

        let primary = FileData {
            name: primary_file.name().into_owned(),
            source: primary_source.to_owned(),
            line_start: primary_line,
            annotations: primary_annotations,
            first_is_primary: true,
        };

        // Cross-file secondary labels.
        let mut cross_file_map: Vec<(jr_base::FileId, FileData)> = Vec::new();
        for sec in &diag.secondary {
            if sec.span.file != diag.primary.span.file {
                let file = map.file(sec.span.file);
                let source = file.text();
                let src_len = source.len();
                let s = (u32::from(sec.span.start()) as usize).min(src_len);
                let e = (u32::from(sec.span.end()) as usize).min(src_len);
                let ann = (s, e, sec.message.clone());

                if let Some(pos) = cross_file_map
                    .iter()
                    .position(|(fid, _)| *fid == sec.span.file)
                {
                    cross_file_map[pos].1.annotations.push(ann);
                } else {
                    // Whole-file source, so the first line is line 1. See the
                    // comment on `primary_line` above.
                    let line_start = 1usize;
                    cross_file_map.push((
                        sec.span.file,
                        FileData {
                            name: file.name().into_owned(),
                            source: source.to_owned(),
                            line_start,
                            annotations: vec![ann],
                            first_is_primary: false,
                        },
                    ));
                }
            }
        }

        let cross_file = cross_file_map.into_iter().map(|(_, fd)| fd).collect();

        Self {
            primary,
            cross_file,
            notes: diag.notes.clone(),
        }
    }

    fn build_groups<'s>(&'s self, diag: &'s Diagnostic) -> Vec<Group<'s>> {
        let mut groups: Vec<Group<'s>> = Vec::new();

        // --- Primary group ---------------------------------------------------
        let as_level = severity_to_level(diag.severity);
        let mut title = as_level.primary_title(diag.message.as_str());
        if let Some(code) = diag.code {
            title = title.id(code);
        }

        let mut primary_snippet = Snippet::source(self.primary.source.as_str())
            .line_start(self.primary.line_start)
            .path(self.primary.name.as_str());

        for (i, (start, end, msg)) in self.primary.annotations.iter().enumerate() {
            let kind = if i == 0 && self.primary.first_is_primary {
                AnnotationKind::Primary
            } else {
                AnnotationKind::Context
            };
            let ann = if let Some(m) = msg {
                kind.span(*start..*end).label(m.as_str())
            } else {
                kind.span(*start..*end)
            };
            primary_snippet = primary_snippet.annotation(ann);
        }

        let mut primary_group = title.element(primary_snippet);

        // Add notes/help as Message elements.
        for (sev, msg) in &self.notes {
            let note_level = severity_to_level(*sev).no_name();
            primary_group = primary_group.element(note_level.message(msg.as_str()));
        }

        groups.push(primary_group);

        // --- Cross-file secondary groups ------------------------------------
        for file_data in &self.cross_file {
            let mut snippet = Snippet::source(file_data.source.as_str())
                .line_start(file_data.line_start)
                .path(file_data.name.as_str());

            for (start, end, msg) in &file_data.annotations {
                let ann = if let Some(m) = msg {
                    AnnotationKind::Context.span(*start..*end).label(m.as_str())
                } else {
                    AnnotationKind::Context.span(*start..*end)
                };
                snippet = snippet.annotation(ann);
            }

            let group = Level::NOTE
                .secondary_title("referenced here")
                .element(snippet);
            groups.push(group);
        }

        groups
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn severity_to_level(sev: Severity) -> Level<'static> {
    match sev {
        Severity::Error => Level::ERROR,
        Severity::Warning => Level::WARNING,
        Severity::Note => Level::NOTE,
        Severity::Help => Level::HELP,
    }
}

/// Renders the instantiation backtrace as plain text appended after the
/// `annotate-snippets` output.
fn render_backtrace(map: &SourceMap, frames: &[InstantiationFrame]) -> String {
    let mut out = String::new();
    for frame in frames {
        let file = map.file(frame.span.file);
        let lc = file.line_col(frame.span.start());
        out.push_str(&format!(
            "  note: {} ({}:{}:{})\n",
            frame.description,
            file.name(),
            lc.line,
            lc.col
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use jr_base::{SourceMap, Span};

    use crate::diagnostic::{Diagnostic, InstantiationFrame, Label, Severity};
    use crate::sink::Diagnostics;

    use super::*;

    fn renderer() -> Renderer {
        Renderer::new()
    }

    fn map_with(path: &str, text: &str) -> (SourceMap, jr_base::FileId) {
        let mut map = SourceMap::new();
        let id = map.add(path, text);
        (map, id)
    }

    // -----------------------------------------------------------------------
    // Severity ordering
    // -----------------------------------------------------------------------

    #[test]
    fn severity_ordering() {
        assert!(Severity::Error > Severity::Warning);
        assert!(Severity::Warning > Severity::Note);
        assert!(Severity::Note > Severity::Help);
    }

    // -----------------------------------------------------------------------
    // Builder chaining
    // -----------------------------------------------------------------------

    #[test]
    fn builder_chaining() {
        let (_, fid) = map_with("a.jr", "let x = 1;");
        let span = Span::from_offsets(fid, 4, 5);
        let diag = Diagnostic::error(span, "type mismatch")
            .with_code("E0001")
            .with_note("expected `s64`")
            .with_help("try casting");
        assert_eq!(diag.severity, Severity::Error);
        assert_eq!(diag.code, Some("E0001"));
        assert_eq!(diag.notes.len(), 2);
        assert_eq!(diag.notes[0].0, Severity::Note);
        assert_eq!(diag.notes[1].0, Severity::Help);
    }

    // -----------------------------------------------------------------------
    // Diagnostics sink
    // -----------------------------------------------------------------------

    #[test]
    fn has_errors_false_when_only_warnings() {
        let (_, fid) = map_with("a.jr", "x");
        let span = Span::from_offsets(fid, 0, 1);
        let mut diags = Diagnostics::new();
        diags.push(Diagnostic::warning(span, "unused variable"));
        assert!(!diags.has_errors());
    }

    #[test]
    fn has_errors_true_when_error_present() {
        let (_, fid) = map_with("a.jr", "x");
        let span = Span::from_offsets(fid, 0, 1);
        let mut diags = Diagnostics::new();
        diags.push(Diagnostic::warning(span, "unused"));
        diags.push(Diagnostic::error(span, "undefined"));
        assert!(diags.has_errors());
    }

    // -----------------------------------------------------------------------
    // sorted() determinism
    // -----------------------------------------------------------------------

    #[test]
    fn sorted_determinism() {
        let (_, fid) = map_with("a.jr", "abcdef");
        let span_b = Span::from_offsets(fid, 2, 3);
        let span_a = Span::from_offsets(fid, 0, 1);
        let mut diags = Diagnostics::new();
        // Push in reverse order.
        diags.push(Diagnostic::error(span_b, "second"));
        diags.push(Diagnostic::error(span_a, "first"));
        let sorted = diags.sorted();
        assert_eq!(sorted[0].message, "first");
        assert_eq!(sorted[1].message, "second");
    }

    #[test]
    fn sorted_errors_before_warnings_at_same_span() {
        let (_, fid) = map_with("a.jr", "x");
        let span = Span::from_offsets(fid, 0, 1);
        let mut diags = Diagnostics::new();
        diags.push(Diagnostic::warning(span, "warn"));
        diags.push(Diagnostic::error(span, "err"));
        let sorted = diags.sorted();
        assert_eq!(sorted[0].severity, Severity::Error);
        assert_eq!(sorted[1].severity, Severity::Warning);
    }

    // -----------------------------------------------------------------------
    // Rendering: single-span error
    // -----------------------------------------------------------------------

    #[test]
    fn render_single_span_error() {
        let (map, fid) = map_with("a.jr", "let x = true;\n");
        let span = Span::from_offsets(fid, 4, 5);
        let diag = Diagnostic::error(span, "type mismatch").with_code("E0001");
        let out = renderer().render(&map, &diag);
        assert!(out.contains("error"), "output: {out}");
        assert!(out.contains("type mismatch"), "output: {out}");
        assert!(out.contains("E0001"), "output: {out}");
        assert!(out.contains("a.jr"), "output: {out}");
    }

    // -----------------------------------------------------------------------
    // Rendering: notes and help
    // -----------------------------------------------------------------------

    #[test]
    fn render_with_notes_and_help() {
        let (map, fid) = map_with("a.jr", "let x = true;\n");
        let span = Span::from_offsets(fid, 4, 5);
        let diag = Diagnostic::error(span, "type mismatch")
            .with_note("expected `s64`, found `bool`")
            .with_help("try casting with `cast(x)`");
        let out = renderer().render(&map, &diag);
        assert!(out.contains("expected `s64`"), "output: {out}");
        assert!(out.contains("try casting"), "output: {out}");
    }

    // -----------------------------------------------------------------------
    // Rendering: secondary label in a DIFFERENT file
    // -----------------------------------------------------------------------

    #[test]
    fn render_cross_file_secondary_label() {
        let mut map = SourceMap::new();
        let fid_a = map.add("a.jr", "let x = foo();\n");
        let fid_b = map.add("b.jr", "foo :: () -> bool { return true; }\n");

        let primary_span = Span::from_offsets(fid_a, 8, 13); // "foo()"
        let secondary_span = Span::from_offsets(fid_b, 0, 3); // "foo"

        let diag = Diagnostic::error(primary_span, "type mismatch")
            .with_label(Label::with_message(secondary_span, "defined here"));

        let out = renderer().render(&map, &diag);
        assert!(out.contains("a.jr"), "output: {out}");
        assert!(out.contains("b.jr"), "output: {out}");
        assert!(out.contains("defined here"), "output: {out}");
    }

    // -----------------------------------------------------------------------
    // Rendering: instantiation backtrace
    // -----------------------------------------------------------------------

    #[test]
    fn render_instantiation_backtrace() {
        let (map, fid) = map_with("a.jr", "sort(entities);\n");
        let call_span = Span::from_offsets(fid, 0, 4);
        let diag = Diagnostic::error(call_span, "no matching overload").with_frame(
            InstantiationFrame::new(call_span, "in instantiation of `sort($T = Entity)`"),
        );
        let out = renderer().render(&map, &diag);
        assert!(out.contains("in instantiation of"), "output: {out}");
        assert!(out.contains("sort($T = Entity)"), "output: {out}");
    }

    // -----------------------------------------------------------------------
    // Rendering: span at EOF (empty span, offset == text length)
    // -----------------------------------------------------------------------

    #[test]
    fn render_eof_span_no_panic() {
        let text = "let x = 1;";
        let (map, fid) = map_with("a.jr", text);
        let eof = Span::empty_at(fid, jr_base::TextSize::from(text.len() as u32));
        let diag = Diagnostic::error(eof, "unexpected end of file");
        let out = renderer().render(&map, &diag);
        assert!(out.contains("unexpected end of file"), "output: {out}");
    }

    // -----------------------------------------------------------------------
    // Rendering: multi-byte UTF-8 source line
    // -----------------------------------------------------------------------

    #[test]
    fn render_multibyte_utf8_no_panic() {
        // 'é' is 2 bytes; the span points at the ASCII 'l' after it.
        let text = "héllo world\n";
        let (map, fid) = map_with("utf8.jr", text);
        // Byte offset 3 is the 'l' after 'é' (h=0, é=1..3, l=3).
        let span = Span::from_offsets(fid, 3, 4);
        let diag = Diagnostic::error(span, "unexpected character");
        let out = renderer().render(&map, &diag);
        assert!(out.contains("unexpected character"), "output: {out}");
    }

    // -----------------------------------------------------------------------
    // Snapshot: full multi-file + backtrace rendering
    // -----------------------------------------------------------------------

    #[test]
    fn snapshot_multi_file_with_backtrace() {
        let mut map = SourceMap::new();
        let fid_a = map.add("main.jr", "result := sort(entities);\n");
        let fid_b = map.add("sort.jr", "sort :: ($T: Type) -> []T { return []; }\n");

        let call_span = Span::from_offsets(fid_a, 10, 14); // "sort"
        let def_span = Span::from_offsets(fid_b, 0, 4); // "sort"

        let diag = Diagnostic::error(call_span, "type mismatch in generic call")
            .with_code("E0042")
            .with_label(Label::with_message(def_span, "generic defined here"))
            .with_note("expected `[]Entity`, found `[]s64`")
            .with_help("check the type argument")
            .with_frame(InstantiationFrame::new(
                call_span,
                "in instantiation of `sort($T = Entity)`",
            ));

        let out = renderer().render(&map, &diag);

        assert!(out.contains("error"), "missing 'error': {out}");
        assert!(out.contains("E0042"), "missing code: {out}");
        assert!(out.contains("type mismatch"), "missing message: {out}");
        assert!(out.contains("main.jr"), "missing primary file: {out}");
        assert!(out.contains("sort.jr"), "missing secondary file: {out}");
        assert!(
            out.contains("generic defined here"),
            "missing secondary label: {out}"
        );
        assert!(out.contains("expected `[]Entity`"), "missing note: {out}");
        assert!(
            out.contains("check the type argument"),
            "missing help: {out}"
        );
        assert!(
            out.contains("in instantiation of"),
            "missing backtrace: {out}"
        );

        eprintln!("--- multi-file + backtrace output ---\n{out}\n---");
    }

    // -----------------------------------------------------------------------
    // render_all
    // -----------------------------------------------------------------------

    #[test]
    fn render_all_joins_diagnostics() {
        let (map, fid) = map_with("a.jr", "let x = 1;\nlet y = 2;\n");
        let span1 = Span::from_offsets(fid, 4, 5);
        let span2 = Span::from_offsets(fid, 15, 16);
        let mut diags = Diagnostics::new();
        diags.push(Diagnostic::error(span1, "first error"));
        diags.push(Diagnostic::warning(span2, "second warning"));
        let out = renderer().render_all(&map, &diags);
        assert!(out.contains("first error"), "output: {out}");
        assert!(out.contains("second warning"), "output: {out}");
    }
}
