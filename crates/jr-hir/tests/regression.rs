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
/// ADR-0134 flipped this: nested `X :: <value>` is now supported, so the
/// declaration is **hoisted** into the file's item arena and represented as
/// `Stmt::Item(item_id, span)`. No diagnostic, no `Stmt::Error` — because it
/// is not an error any more. This test still exists so a regression that
/// re-drops the declaration silently is caught.
#[test]
fn nested_declarations_are_hoisted_not_silently_dropped() {
    let src = "outer :: () {\n    inner :: () {\n    }\n}\n";
    let (hir, diags, _interner) = lower(src);

    // No E0207 any more — ADR-0134 lifted it.
    assert!(
        !diags.iter().any(|d| d.code == Some("E0207")),
        "ADR-0134 lifted E0207 for nested constants; got: {:?}",
        diags.iter().map(|d| d.code).collect::<Vec<_>>()
    );

    // The nested `inner` is now an ordinary hoisted item, reachable in `outer`'s
    // body via `Stmt::Item(item_id, span)`. A `Stmt::Error` placeholder would
    // mean silent dropping, which is precisely what this test used to catch.
    let has_item_stmt = hir
        .bodies
        .iter()
        .any(|b| b.stmts.iter().any(|s| matches!(s, Stmt::Item(_, _))));
    assert!(
        has_item_stmt,
        "expected a Stmt::Item placeholder for the hoisted `inner`; \
         a Stmt::Error here would mean silent dropping"
    );

    // Two procs — `outer` and the hoisted `inner`.
    let proc_items = hir
        .items
        .iter()
        .filter(|it| {
            matches!(
                it.kind,
                jr_hir::ItemKind::Const {
                    value: jr_hir::ConstValue::Proc(_),
                }
            )
        })
        .count();
    assert_eq!(
        proc_items, 2,
        "expected two proc items (outer + hoisted inner), got {proc_items}"
    );
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

/// `ExprId`s are not unique across a file: `FileHir::exprs` and every
/// `Body::exprs` are independent arenas that all start at index 0. `ResolveMap`
/// was originally keyed on a bare `ExprId`, so the two arenas collided and the
/// last writer won — a top-level constant's name reference came back resolved to
/// whatever local happened to share its index.
///
/// This is the shape that caught it: `LIMIT :: GREETING` is top-level expression
/// 1, and `z := y` makes `y` body expression 1. Before the fix, looking up the
/// top-level one returned `Res::Local`.
///
/// Nothing consumed the map at the time, so this was latent rather than visibly
/// broken; `jr-sema` is the first pass that would have depended on it.
#[test]
fn resolve_map_does_not_collide_top_level_and_body_expression_ids() {
    let src = "GREETING :: \"x\";\nLIMIT :: GREETING;\nmain :: () {\n    y := 1;\n    z := y;\n}\n";
    let (hir, diags, interner) = lower(src);
    assert!(!diags.has_errors(), "probe source must lower cleanly");

    let (map, resolve_diags) = jr_hir::resolve(&hir, &[], &interner);
    assert!(
        !resolve_diags.has_errors(),
        "probe source must resolve cleanly"
    );

    // The top-level `GREETING` reference, in FileHir::exprs.
    let top_idx = hir
        .exprs
        .iter()
        .position(|e| matches!(e, Expr::Name { .. }))
        .expect("`LIMIT :: GREETING` lowers to a top-level Name expression");

    // The `y` reference inside main's body, which shares that index.
    let body_id = hir
        .bodies
        .iter()
        .enumerate()
        .map(|(i, _)| jr_hir::BodyId::from_usize(i))
        .next()
        .expect("main has a body");
    let body_idx = hir
        .body(body_id)
        .exprs
        .iter()
        .position(|e| matches!(e, Expr::Name { .. }))
        .expect("`z := y` lowers to a Name expression in the body");
    assert_eq!(
        top_idx, body_idx,
        "this test is only meaningful while the two arenas collide at one index"
    );

    let top = map.get_top(jr_hir::ExprId::from_usize(top_idx));
    let in_body = map.get_in_body(body_id, jr_hir::ExprId::from_usize(body_idx));

    assert!(
        matches!(top, Some(jr_hir::Res::Item(_))),
        "the top-level `GREETING` must resolve to a file-scope item, got {top:?}"
    );
    assert!(
        matches!(in_body, Some(jr_hir::Res::Local(_))),
        "the body's `y` must resolve to a local, got {in_body:?}"
    );
}

/// `Parser::parse_body` accepts either a braced block or a single unbraced
/// statement, but the typed-AST accessors were declared `Option<Block>`, so a
/// braceless body was invisible to them. Lowering then fell through to its error
/// branch and produced `Stmt::Error` — with **no diagnostic** from the lexer, the
/// parser or lowering — silently discarding the body.
///
/// `tests/corpus/valid/010-if-else.jr` documents `if n > 0 return n;` as legal and
/// contains it, so `jr check` reported that file clean while the `return` was
/// gone. `jr-mir`'s poison gate (ADR-0017 §4) is what surfaced it: MIR refused the
/// body rather than emitting one that ignored the `return`.
///
/// One test per shape the grammar allows, because the three accessors were three
/// separate instances of the same mistake.
#[test]
fn a_braceless_control_flow_body_is_not_silently_discarded() {
    for (label, src) in [
        (
            "if",
            "f :: (n: s64) -> s64 {\n    if n > 0 return n;\n    return 0;\n}\n",
        ),
        (
            "else",
            "f :: (n: s64) -> s64 {\n    if n > 0 { return n; } else return 0;\n}\n",
        ),
        (
            "while",
            "f :: () {\n    i := 0;\n    while i < 3 i = i + 1;\n}\n",
        ),
    ] {
        let (hir, diags, _interner) = lower(src);
        assert!(
            diags.is_empty(),
            "a braceless `{label}` body is legal, so lowering must not complain: {diags:?}"
        );
        let body = hir.bodies.first().expect("the procedure has a body");
        assert!(
            !body.stmts.iter().any(|stmt| matches!(stmt, Stmt::Error(_))),
            "a braceless `{label}` body must survive lowering, not become Stmt::Error"
        );
    }
}

/// A braceless body gets its own scope, exactly as a braced one does.
///
/// Lowering wraps the single statement in a synthetic one-statement block, which
/// is what keeps `Stmt::If::then` always a `Stmt::Block` for every consumer
/// downstream. If it leaked the declaration into the enclosing scope instead,
/// `if c x := 1;` would make `x` visible afterwards.
#[test]
fn a_braceless_body_scopes_its_declaration_like_a_braced_one() {
    let (hir, _diags, _interner) = lower("f :: (c: bool) {\n    if c x := 1;\n    y := 2;\n}\n");
    let body = hir.bodies.first().expect("the procedure has a body");
    let then_is_a_block = body.stmts.iter().any(|stmt| match stmt {
        Stmt::If { then, .. } => {
            matches!(body.stmt(*then), Stmt::Block(inner, _) if inner.len() == 1)
        }
        _ => false,
    });
    assert!(
        then_is_a_block,
        "the braceless body must be wrapped in a one-statement block"
    );
}
