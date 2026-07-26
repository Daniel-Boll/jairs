//! Integration tests: lower every corpus file and check diagnostics.
//!
//! Valid corpus files must lower with zero HIR lowering diagnostics.
//! (Resolution diagnostics are not checked here because the corpus test
//! cannot provide real import scopes — `print` and `print_int` come from
//! `#import "Basic"` which is not available in the test environment.)
//!
//! Invalid corpus files must lower without panicking.

use std::path::Path;

use jr_base::{FileId, Interner};
use jr_hir::{lower_file, resolve};
use jr_syntax::parse;

#[test]
fn valid_corpus_files_lower_with_zero_lowering_diagnostics() {
    let corpus_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/valid");

    let mut failures = Vec::new();

    let mut entries: Vec<_> = std::fs::read_dir(&corpus_dir)
        .unwrap_or_else(|e| panic!("failed to read corpus dir: {e}"))
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("jr"))
        .collect();
    entries.sort_by_key(|e| e.path());

    for entry in entries {
        let path = entry.path();
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        let interner = Interner::new();
        let file = FileId::from_usize(0);
        let parsed = parse(&source, file);

        // Lowering diagnostics only (not resolution — we can't provide imports).
        let (_hir, diags) = lower_file(&parsed, file, &interner);

        if !diags.is_empty() {
            let msgs: Vec<String> = diags.iter().map(|d| d.message.clone()).collect();
            failures.push(format!(
                "{}: {} lowering diagnostic(s): {}",
                path.file_name().unwrap().to_string_lossy(),
                diags.len(),
                msgs.join("; ")
            ));
        }
    }

    if !failures.is_empty() {
        panic!(
            "Valid corpus files with lowering diagnostics:\n{}",
            failures.join("\n")
        );
    }
}

#[test]
fn invalid_corpus_files_do_not_panic() {
    let corpus_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/invalid");

    let mut entries: Vec<_> = std::fs::read_dir(&corpus_dir)
        .unwrap_or_else(|e| panic!("failed to read corpus dir: {e}"))
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("jr"))
        .collect();
    entries.sort_by_key(|e| e.path());

    for entry in entries {
        let path = entry.path();
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        let interner = Interner::new();
        let file = FileId::from_usize(0);
        let parsed = parse(&source, file);
        // Must not panic:
        let (hir, mut diags) = lower_file(&parsed, file, &interner);
        let (_, resolve_diags) = resolve(&hir, &[], &interner);
        diags.extend(resolve_diags.into_vec());
        // We don't assert anything about the number of diagnostics for invalid files.
        let _ = diags;
    }
}

/// Test that resolution with no imports reports unresolved imported names.
#[test]
fn resolution_without_imports_reports_unresolved_names() {
    let source = r#"
#import "Basic";
main :: () {
    print("hello");
}
"#;
    let interner = Interner::new();
    let file = FileId::from_usize(0);
    let parsed = parse(source, file);
    let (hir, lower_diags) = lower_file(&parsed, file, &interner);
    assert!(
        lower_diags.is_empty(),
        "unexpected lowering diagnostics: {:?}",
        lower_diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );

    // With no imports, `print` should be unresolved
    let (_, resolve_diags) = resolve(&hir, &[], &interner);
    assert!(
        resolve_diags.iter().any(|d| d.message.contains("print")),
        "expected unresolved `print` diagnostic, got: {:?}",
        resolve_diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}
