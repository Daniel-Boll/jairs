//! Interface tests for semantic extraction.

use jr_base::{TextRange, TextSize};
use jr_db::{JairsDatabase, SourceFile};
use jr_refactor::{ExtractRequest, ExtractionKind, Refusal, SourceChange, extract};

fn program(source: &str) -> (JairsDatabase, jr_db::ModuleCatalog, SourceFile) {
    let mut db = JairsDatabase::default();
    let catalog = db.set_module_search_paths(Vec::new());
    let path = "/jr-refactor-test/main.jr";
    db.set_file_text(path, source);
    let file = db.source_file(path).expect("the source was just installed");
    (db, catalog, file)
}

fn range_of(source: &str, needle: &str) -> TextRange {
    let start = source.find(needle).expect("selection exists");
    TextRange::new(
        TextSize::from(u32::try_from(start).expect("small test source")),
        TextSize::from(u32::try_from(start + needle.len()).expect("small test source")),
    )
}

fn apply(source: &str, change: &SourceChange) -> String {
    let mut result = source.to_owned();
    let mut edits = change.edits.clone();
    edits.sort_by_key(|edit| edit.range.start());
    for edit in edits.into_iter().rev() {
        result.replace_range(
            usize::from(edit.range.start())..usize::from(edit.range.end()),
            &edit.replacement,
        );
    }
    result
}

fn candidate(
    source: &str,
    selection: &str,
    kind: ExtractionKind,
) -> (JairsDatabase, jr_db::ModuleCatalog, SourceFile, String) {
    let (db, catalog, file) = program(source);
    let report = extract(
        &db,
        file,
        catalog,
        ExtractRequest::all(range_of(source, selection)),
    );
    let action = report
        .candidates
        .iter()
        .find(|candidate| candidate.kind == kind)
        .unwrap_or_else(|| panic!("missing {kind:?} candidate: {:?}", report.refusals));
    let transformed = apply(source, &action.change);
    (db, catalog, file, transformed)
}

fn assert_checks(
    mut db: JairsDatabase,
    catalog: jr_db::ModuleCatalog,
    file: SourceFile,
    transformed: &str,
) {
    let path = file.path(&db);
    db.set_file_text(path, transformed);
    let diagnostics = jr_db::file_diagnostics(&db, file, catalog);
    assert!(
        !diagnostics.has_errors(),
        "generated source must check:\n{transformed}\n\n{diagnostics:?}"
    );
}

#[test]
fn expression_extraction_inserts_a_local_immediately_before_the_statement() {
    let source = "f :: () -> s64 {\n  return 1 + 2;\n}\n";
    let (db, catalog, file, transformed) = candidate(source, "1 + 2", ExtractionKind::Local);
    assert_eq!(
        transformed,
        "f :: () -> s64 {\n  extracted := 1 + 2;\n  return extracted;\n}\n"
    );
    assert_checks(db, catalog, file, &transformed);
}

#[test]
fn statement_extraction_preserves_capture_storage_and_an_escaping_local() {
    let source = "f :: (n: s64) -> s64 {\n  answer := n + 1;\n  return answer;\n}\n";
    let (db, catalog, file, transformed) =
        candidate(source, "answer := n + 1;", ExtractionKind::Procedure);
    assert!(transformed.contains("__extract_capture_0.*"));
    assert!(transformed.contains("__extract_output_0.* ="));
    assert!(transformed.contains("answer: s64;"));
    assert_checks(db, catalog, file, &transformed);
}

#[test]
fn outward_return_is_dispatched_by_the_caller() {
    let source = "f :: (n: s64) -> s64 {\n  if n > 0 {\n    return n;\n  }\n  return 0;\n}\n";
    let selection = "if n > 0 {\n    return n;\n  }";
    let (db, catalog, file, transformed) = candidate(source, selection, ExtractionKind::Procedure);
    assert!(transformed.contains("return 1;"));
    assert!(transformed.contains("if __extract_flow == 1"));
    assert!(transformed.contains("return __extract_return_0;"));
    assert_checks(db, catalog, file, &transformed);
}

#[test]
fn outward_break_and_continue_are_dispatched_inside_the_original_loop() {
    let source = "f :: () {\n  n := 0;\n  while n < 3 {\n    n += 1;\n    if n == 1 {\n      continue;\n    }\n    break;\n  }\n}\n";
    let selection = "if n == 1 {\n      continue;\n    }\n    break;";
    let (db, catalog, file, transformed) = candidate(source, selection, ExtractionKind::Procedure);
    assert!(transformed.contains("continue;"));
    assert!(transformed.contains("break;"));
    assert!(transformed.matches("if __extract_flow ==").count() == 2);
    assert_checks(db, catalog, file, &transformed);
}

#[test]
fn a_defer_that_would_outlive_the_selection_is_refused() {
    let source = "f :: () {\n  n := 0;\n  defer n += 1;\n  n += 2;\n}\n";
    let (db, catalog, file) = program(source);
    let report = extract(
        &db,
        file,
        catalog,
        ExtractRequest::all(range_of(source, "defer n += 1;")),
    );
    assert!(
        report
            .refusals
            .contains(&(ExtractionKind::Procedure, Refusal::DeferLifetimeWouldChange))
    );
}
