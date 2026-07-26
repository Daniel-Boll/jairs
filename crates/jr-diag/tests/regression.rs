//! Regression tests for rendering defects.
//!
//! These exist because the failures they cover are the kind that look fine in a
//! snapshot written at the same time as the bug.

use jr_base::{SourceMap, Span};
use jr_diag::{Diagnostic, Diagnostics, Label, Renderer};

/// Every rendered line number must match the real line in the file.
///
/// `annotate-snippets` treats `line_start` as the line number of the FIRST line
/// of the source it is given. The renderer hands it the whole file, so
/// `line_start` must be 1. It previously passed the primary span's line, which
/// shifted every number by (line - 1): a diagnostic on line 1 rendered as line
/// 2, and one on line 40 rendered as line 79. That breaks human reading and
/// editor jump-to-error alike, and is invisible unless you count lines.
#[test]
fn rendered_line_numbers_match_the_source() {
    let text = "one\ntwo\nthree\nfour\nfive\n";
    let mut map = SourceMap::new();
    let file = map.add("lines.jr", text);

    // Point at each line in turn and assert the rendered header agrees.
    for (index, line_text) in text.lines().enumerate() {
        let expected_line = index + 1;
        let start = text.find(line_text).expect("line must be present") as u32;
        let span = Span::from_offsets(file, start, start + line_text.len() as u32);

        let diag = Diagnostic::error(span, "here").with_code("E9999");
        let rendered = Renderer::new().render(&map, &diag);

        assert!(
            rendered.contains(&format!("lines.jr:{expected_line}:1")),
            "line {expected_line} rendered wrong header:\n{rendered}"
        );
        // The gutter must label the offending line with its true number.
        assert!(
            rendered.contains(&format!("{expected_line} | {line_text}")),
            "line {expected_line} rendered wrong gutter:\n{rendered}"
        );
    }
}

/// A secondary label on an earlier line must also be numbered correctly, since
/// "first declared here" style diagnostics depend on it.
#[test]
fn secondary_label_line_numbers_are_correct() {
    let text = "dup :: 1;\ndup :: 2;\n";
    let mut map = SourceMap::new();
    let file = map.add("dup.jr", text);

    let first = Span::from_offsets(file, 0, 3);
    let second = Span::from_offsets(file, 10, 13);

    let diag = Diagnostic::error(second, "duplicate declaration of `dup`")
        .with_code("E0200")
        .with_label(Label::with_message(first, "`dup` first declared here"));

    let rendered = Renderer::new().render(&map, &diag);

    assert!(rendered.contains("dup.jr:2:1"), "header wrong:\n{rendered}");
    assert!(
        rendered.contains("1 | dup :: 1;"),
        "secondary must be labelled line 1:\n{rendered}"
    );
    assert!(
        rendered.contains("2 | dup :: 2;"),
        "primary must be labelled line 2:\n{rendered}"
    );
}

/// Rendering must not panic or mis-number when the span sits at end of file,
/// which is where "unexpected end of input" diagnostics land.
#[test]
fn span_at_end_of_file_renders() {
    let text = "a :: 1;";
    let mut map = SourceMap::new();
    let file = map.add("eof.jr", text);
    let end = text.len() as u32;

    let diag = Diagnostic::error(Span::from_offsets(file, end, end), "unexpected end of file");
    let rendered = Renderer::new().render(&map, &diag);
    assert!(rendered.contains("eof.jr:1:"), "got:\n{rendered}");
}

/// `render_all` output must be usable directly by the CLI: each diagnostic
/// separated, nothing swallowed.
#[test]
fn render_all_includes_every_diagnostic() {
    let text = "a\nb\nc\n";
    let mut map = SourceMap::new();
    let file = map.add("multi.jr", text);

    let mut diags = Diagnostics::new();
    for (i, code) in ["E0001", "E0002", "E0003"].iter().enumerate() {
        let start = (i * 2) as u32;
        diags.push(
            Diagnostic::error(
                Span::from_offsets(file, start, start + 1),
                format!("problem {i}"),
            )
            .with_code(code),
        );
    }

    let rendered = Renderer::new().render_all(&map, &diags);
    for i in 0..3 {
        assert!(
            rendered.contains(&format!("problem {i}")),
            "diagnostic {i} missing from render_all output:\n{rendered}"
        );
    }
    for (i, line) in ["1 | a", "2 | b", "3 | c"].iter().enumerate() {
        assert!(
            rendered.contains(line),
            "line {} mis-numbered in render_all:\n{rendered}",
            i + 1
        );
    }
}
