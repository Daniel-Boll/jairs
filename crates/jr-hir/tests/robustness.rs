//! Robustness tests: deep nesting must not overflow the stack.
//!
//! The parser caps nesting at 256 levels, but we still verify that lowering
//! a deeply nested tree does not overflow the stack. We run the test on an
//! explicitly spawned thread with a 1 MiB stack, exactly as
//! `crates/jr-syntax/tests/robustness.rs` does.

use jr_base::{FileId, Interner};
use jr_hir::lower_file;
use jr_syntax::parse;

fn file() -> FileId {
    FileId::from_usize(0)
}

/// Run a closure on a thread with a 1 MiB stack.
fn with_small_stack<F: FnOnce() + Send + 'static>(f: F) {
    std::thread::Builder::new()
        .stack_size(1024 * 1024) // 1 MiB
        .spawn(f)
        .expect("failed to spawn thread")
        .join()
        .expect("thread panicked");
}

#[test]
fn deeply_nested_binary_expr_does_not_overflow() {
    // Build a deeply nested binary expression: 1 + 1 + 1 + ... (256 levels)
    let mut expr = "1".to_owned();
    for _ in 0..255 {
        expr = format!("{expr} + 1");
    }
    let source = format!("X :: {expr};");

    with_small_stack(move || {
        let interner = Interner::new();
        let f = file();
        let parsed = parse(&source, f);
        let (hir, diags) = lower_file(&parsed, f, &interner);
        // Should not panic; diagnostics may or may not be present
        let _ = (hir, diags);
    });
}

#[test]
fn deeply_nested_block_does_not_overflow() {
    // Build deeply nested blocks: { { { ... } } }
    let depth = 200;
    let mut body = "x := 1;".to_owned();
    for _ in 0..depth {
        body = format!("{{ {body} }}");
    }
    let source = format!("main :: () {body}");

    with_small_stack(move || {
        let interner = Interner::new();
        let f = file();
        let parsed = parse(&source, f);
        let (hir, diags) = lower_file(&parsed, f, &interner);
        let _ = (hir, diags);
    });
}

#[test]
fn deeply_nested_field_access_does_not_overflow() {
    // Build a deeply nested field access: a.b.c.d...
    let depth = 200;
    let mut expr = "a".to_owned();
    for _ in 0..depth {
        expr = format!("{expr}.field");
    }
    let source = format!("X :: {expr};");

    with_small_stack(move || {
        let interner = Interner::new();
        let f = file();
        let parsed = parse(&source, f);
        let (hir, diags) = lower_file(&parsed, f, &interner);
        let _ = (hir, diags);
    });
}

#[test]
fn many_items_do_not_overflow() {
    // 1000 top-level constants
    let mut source = String::new();
    for i in 0..1000 {
        source.push_str(&format!("C{i} :: {i};\n"));
    }

    with_small_stack(move || {
        let interner = Interner::new();
        let f = file();
        let parsed = parse(&source, f);
        let (hir, diags) = lower_file(&parsed, f, &interner);
        assert!(diags.is_empty());
        assert_eq!(hir.items.len(), 1000);
    });
}
