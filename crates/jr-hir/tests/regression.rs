//! Regression tests for defects found by review rather than by the corpus.
//!
//! Each test here corresponds to a specific bug or footgun. They live in their
//! own file so it is obvious that removing one removes a guarantee.

use jr_base::{FileId, Interner};
use jr_hir::{Expr, Stmt, lower_file};
use jr_syntax::parse;

fn lower(src: &str) -> (jr_hir::FileHir, jr_diag::Diagnostics, Interner) {
    let interner = Interner::new();
    let parsed = parse(src, FileId::from_usize(0));
    let (hir, diags) = lower_file(&parsed, FileId::from_usize(0), &interner);
    (hir, diags, interner)
}

/// `Expr::span()` used to `unreachable!()` on the `Literal` variant, so any
/// consumer asking a literal for its span — which `jr-sema` will do constantly
/// when reporting type errors — would abort the compiler.
#[test]
fn every_expression_kind_can_report_its_span_without_panicking() {
    let src = r#"
lib :: #system_library "c";

main :: () {
    a := 1;
    b := "text";
    c := true;
    d := a + 1;
    e := -a;
    f := (a);
    g := *a;
    h := g.*;
    i := main;
    j := ---;
}
"#;
    let (hir, _diags, _interner) = lower(src);

    let mut seen = 0usize;
    for body in &hir.bodies {
        for expr in &body.exprs {
            // The assertion is simply that this does not panic.
            let span = expr.span();
            assert!(
                u32::from(span.end()) as usize <= src.len(),
                "span out of range for {expr:?}"
            );
            seen += 1;
        }
    }
    for expr in &hir.exprs {
        let _ = expr.span();
        seen += 1;
    }
    assert!(
        seen > 10,
        "expected to inspect many expressions, saw {seen}"
    );
}

/// Literal spans must be the literal's own span, not a parent's, or every
/// literal-related diagnostic would underline the wrong text.
#[test]
fn literal_spans_point_at_the_literal() {
    let src = "X :: 4096;";
    let (hir, _diags, _interner) = lower(src);

    let literal = hir
        .exprs
        .iter()
        .find(|e| matches!(e, Expr::Literal(..)))
        .expect("expected a literal");
    let span = literal.span();
    assert_eq!(
        &src[span.range], "4096",
        "a literal's span must cover exactly the literal token"
    );
}

/// Declarations inside a procedure body used to lower to a bare `Stmt::Error`
/// with NO diagnostic, silently deleting the declaration from the program.
/// Silent omission becomes a miscompile once codegen exists, so it must be
/// reported even though the feature is not implemented.
#[test]
fn declarations_inside_a_body_are_reported_not_silently_dropped() {
    let src = "outer :: () {\n    inner :: () {\n    }\n}\n";
    let (hir, diags, _interner) = lower(src);

    assert!(
        !diags.is_empty(),
        "a nested declaration must produce a diagnostic, not vanish"
    );
    assert!(
        diags.iter().any(|d| d.code == Some("E0207")),
        "expected E0207, got {:?}",
        diags.iter().map(|d| d.code).collect::<Vec<_>>()
    );

    // And it must still appear as an Error statement so the tree stays total.
    let has_error_stmt = hir
        .bodies
        .iter()
        .any(|b| b.stmts.iter().any(|s| matches!(s, Stmt::Error(_))));
    assert!(has_error_stmt, "expected an Error statement placeholder");
}

/// `#import` is a file-scope construct; inside a body it is a scope error, not
/// an unimplemented feature, so it gets its own code.
#[test]
fn import_inside_a_body_is_a_scope_error() {
    let src = "main :: () {\n    #import \"Basic\";\n}\n";
    let (_hir, diags, _interner) = lower(src);
    assert!(
        diags.iter().any(|d| d.code == Some("E0208")),
        "expected E0208 for a body-scoped #import, got {:?}",
        diags.iter().map(|d| d.code).collect::<Vec<_>>()
    );
}

/// Every diagnostic this crate emits must carry a code, so that users can look
/// it up and so that tests can assert on it precisely.
#[test]
fn all_diagnostics_carry_a_code() {
    let sources = [
        "outer :: () {\n    inner :: () {\n    }\n}\n",
        "main :: () {\n    #import \"Basic\";\n}\n",
        "X :: 99999999999999999999999;\n",
        "dup :: 1;\ndup :: 2;\n",
        "main :: () {\n    x := \"bad \\q\";\n}\n",
    ];
    for src in sources {
        let (_hir, diags, _interner) = lower(src);
        for diag in diags.iter() {
            assert!(
                diag.code.is_some(),
                "diagnostic without a code: {:?} (from {src:?})",
                diag.message
            );
        }
    }
}
