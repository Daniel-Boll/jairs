//! Integration tests for `jr-db`.
//!
//! These tests verify:
//! 1. Database construction and basic file operations.
//! 2. Parse returning the expected tree for a small snippet.
//! 3. Diagnostics surfacing for broken input.
//! 4. `FileId` stability across edits.
//! 5. **Incrementality**: editing file A re-runs A's parse but NOT B's.
//! 6. Setting identical text does not re-run queries (salsa backdating).
//! 7. Corpus files parse without errors.
//! 8. Module resolution: search paths, file lookup, cycles, E0210.
//! 9. Semantic analysis: signatures, checking, and invalidation across imports.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use jr_db::{
    Db as _, InMemoryModules, JairsDatabase, ModuleSearchPaths, SourceFile, checked,
    file_diagnostics, file_exports, file_hir, file_signatures, imports_of, module_file,
    parse_diagnostics, parse_file, resolved,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Creates a database that counts `WillExecute` events for `parse_file`.
///
/// Returns the database and a shared counter. Each time salsa actually
/// re-executes `parse_file` (rather than returning a cached result), the
/// counter is incremented.
fn db_with_parse_counter() -> (JairsDatabase, Arc<AtomicUsize>) {
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();
    let db = JairsDatabase::with_event_callback(move |event| {
        if let salsa::EventKind::WillExecute { database_key } = event.kind {
            let name = format!("{database_key:?}");
            // Count executions of parse_file specifically.
            if name.contains("parse_file") {
                counter_clone.fetch_add(1, Ordering::SeqCst);
            }
        }
    });
    (db, counter)
}

// ---------------------------------------------------------------------------
// 1. Database construction
// ---------------------------------------------------------------------------

#[test]
fn database_constructs_with_default() {
    let _db = JairsDatabase::default();
}

#[test]
fn database_constructs_with_event_callback() {
    let _db = JairsDatabase::with_event_callback(|_event| {});
}

// ---------------------------------------------------------------------------
// 2. Adding and retrieving files
// ---------------------------------------------------------------------------

#[test]
fn add_file_returns_stable_file_id() {
    let mut db = JairsDatabase::default();
    let id1 = db.set_file_text("a.jr", "x := 1;");
    let id2 = db.set_file_text("a.jr", "x := 2;");
    assert_eq!(id1, id2, "FileId must be stable across edits");
}

#[test]
fn distinct_paths_get_distinct_file_ids() {
    let mut db = JairsDatabase::default();
    let a = db.set_file_text("a.jr", "");
    let b = db.set_file_text("b.jr", "");
    assert_ne!(a, b);
}

#[test]
fn source_file_lookup_by_path() {
    let mut db = JairsDatabase::default();
    db.set_file_text("hello.jr", "main :: () {}");
    let sf = db.source_file("hello.jr");
    assert!(sf.is_some(), "should find the file we just added");
    let sf = sf.unwrap();
    assert_eq!(sf.text(&db).as_ref(), "main :: () {}");
}

#[test]
fn file_id_lookup_by_path() {
    let mut db = JairsDatabase::default();
    let id = db.set_file_text("x.jr", "");
    assert_eq!(db.file_id("x.jr"), Some(id));
    assert_eq!(db.file_id("missing.jr"), None);
}

#[test]
fn source_map_reflects_added_files() {
    let mut db = JairsDatabase::default();
    db.set_file_text("a.jr", "");
    db.set_file_text("b.jr", "");
    let sm = db.source_map();
    assert_eq!(sm.len(), 2);
}

// ---------------------------------------------------------------------------
// 3. Parse returning the expected tree
// ---------------------------------------------------------------------------

#[test]
fn parse_empty_file_produces_source_file_node() {
    let mut db = JairsDatabase::default();
    let sf = add_file(&mut db, "empty.jr", "");
    let parse = parse_file(&db, sf);
    let root = parse.syntax();
    assert_eq!(
        format!("{:?}", root.kind()),
        "SOURCE_FILE",
        "root node must be SOURCE_FILE"
    );
    assert!(!parse.has_errors(), "empty file must parse without errors");
}

#[test]
fn parse_simple_constant_produces_const_decl() {
    let mut db = JairsDatabase::default();
    let sf = add_file(&mut db, "const.jr", "MAX :: 42;");
    let parse = parse_file(&db, sf);
    assert!(
        !parse.has_errors(),
        "simple constant must parse without errors"
    );
    let tree = jr_syntax::dump_tree(&parse.syntax());
    assert!(tree.contains("CONST_DECL"), "tree must contain CONST_DECL");
    assert!(
        tree.contains("INT_LITERAL"),
        "tree must contain INT_LITERAL"
    );
}

#[test]
fn parse_proc_produces_proc_node() {
    let mut db = JairsDatabase::default();
    let sf = add_file(
        &mut db,
        "proc.jr",
        "add :: (a: s64, b: s64) -> s64 { return a + b; }",
    );
    let parse = parse_file(&db, sf);
    assert!(!parse.has_errors());
    let tree = jr_syntax::dump_tree(&parse.syntax());
    assert!(tree.contains("PROC"), "tree must contain PROC");
    assert!(tree.contains("PARAM_LIST"), "tree must contain PARAM_LIST");
}

#[test]
fn parse_is_lossless() {
    let text = "add :: (a: s64, b: s64) -> s64 { return a + b; }";
    let mut db = JairsDatabase::default();
    let sf = add_file(&mut db, "lossless.jr", text);
    let parse = parse_file(&db, sf);
    let round_trip = parse.syntax().text().to_string();
    assert_eq!(round_trip, text, "parse must be lossless");
}

// ---------------------------------------------------------------------------
// 4. Diagnostics for broken input
// ---------------------------------------------------------------------------

#[test]
fn broken_input_produces_diagnostics() {
    let mut db = JairsDatabase::default();
    let sf = add_file(&mut db, "broken.jr", "x :="); // missing RHS
    let diags = parse_diagnostics(&db, sf);
    assert!(!diags.is_empty(), "broken input must produce diagnostics");
    assert!(diags.has_errors(), "broken input must produce errors");
}

#[test]
fn valid_input_produces_no_diagnostics() {
    let mut db = JairsDatabase::default();
    let sf = add_file(&mut db, "valid.jr", "x := 1;");
    let diags = parse_diagnostics(&db, sf);
    assert!(diags.is_empty(), "valid input must produce no diagnostics");
}

#[test]
fn parse_diagnostics_is_separate_from_parse_tree() {
    // Verify that parse_diagnostics does not require the caller to hold
    // a reference to the parse tree.
    let mut db = JairsDatabase::default();
    let sf = add_file(&mut db, "sep.jr", "x :=");
    let diags = parse_diagnostics(&db, sf);
    assert!(diags.has_errors());
    // We can also get the tree independently.
    let parse = parse_file(&db, sf);
    assert!(parse.has_errors());
}

// ---------------------------------------------------------------------------
// 5. FileId stability across edits
// ---------------------------------------------------------------------------

#[test]
fn file_id_stable_across_text_edit() {
    let mut db = JairsDatabase::default();
    let id_before = db.set_file_text("stable.jr", "x := 1;");
    let id_after = db.set_file_text("stable.jr", "x := 999;");
    assert_eq!(
        id_before, id_after,
        "FileId must not change when file text is updated"
    );
}

#[test]
fn file_id_stable_across_multiple_edits() {
    let mut db = JairsDatabase::default();
    let id0 = db.set_file_text("multi.jr", "a := 1;");
    let id1 = db.set_file_text("multi.jr", "b := 2;");
    let id2 = db.set_file_text("multi.jr", "c := 3;");
    assert_eq!(id0, id1);
    assert_eq!(id1, id2);
}

// ---------------------------------------------------------------------------
// 6. Incrementality: editing A re-runs A but NOT B
// ---------------------------------------------------------------------------

/// This is the critical test. We use salsa's `WillExecute` event to count
/// actual re-executions of `parse_file`. After editing file A:
/// - A's parse count must increase by 1.
/// - B's parse count must NOT increase.
#[test]
fn incrementality_editing_a_does_not_reparse_b() {
    let (mut db, counter) = db_with_parse_counter();

    // Add two files.
    let sf_a = add_file(&mut db, "a.jr", "x := 1;");
    let sf_b = add_file(&mut db, "b.jr", "y := 2;");

    // Query both files to populate the cache.
    let _ = parse_file(&db, sf_a);
    let _ = parse_file(&db, sf_b);

    // Record how many parse_file executions happened so far.
    let count_after_initial = counter.load(Ordering::SeqCst);
    assert!(
        count_after_initial >= 2,
        "both files must have been parsed initially (got {count_after_initial})"
    );

    // Edit only file A.
    db.set_file_text("a.jr", "x := 999;");

    // Query both files again.
    let _ = parse_file(&db, sf_a);
    let _ = parse_file(&db, sf_b);

    let count_after_edit = counter.load(Ordering::SeqCst);
    let new_executions = count_after_edit - count_after_initial;

    // Exactly one new execution: A was re-parsed, B was not.
    assert_eq!(
        new_executions, 1,
        "editing A must re-run parse_file exactly once (for A), not {new_executions} times. \
         B must NOT be re-parsed."
    );
}

/// Verify that querying the same file twice without any edit does not
/// re-execute the query.
#[test]
fn no_reexecution_without_edit() {
    let (mut db, counter) = db_with_parse_counter();
    let sf = add_file(&mut db, "cached.jr", "x := 1;");

    let _ = parse_file(&db, sf);
    let count_after_first = counter.load(Ordering::SeqCst);

    let _ = parse_file(&db, sf);
    let count_after_second = counter.load(Ordering::SeqCst);

    assert_eq!(
        count_after_first, count_after_second,
        "querying the same file twice without an edit must not re-execute parse_file"
    );
}

// ---------------------------------------------------------------------------
// 7. Identical-text update
// ---------------------------------------------------------------------------

/// Setting a file's text to the same value still starts a new salsa revision
/// (salsa does not compare old and new values). This test documents that
/// behaviour: the query WILL re-run after a set, even with identical text.
///
/// If salsa ever adds value-comparison for inputs (it currently does not),
/// this test would need to be updated.
#[test]
fn identical_text_update_documents_salsa_behaviour() {
    let (mut db, counter) = db_with_parse_counter();
    let sf = add_file(&mut db, "same.jr", "x := 1;");

    let _ = parse_file(&db, sf);
    let count_before = counter.load(Ordering::SeqCst);

    // Set the same text again.
    db.set_file_text("same.jr", "x := 1;");
    let _ = parse_file(&db, sf);
    let count_after = counter.load(Ordering::SeqCst);

    // Salsa does not compare input values, so the query re-runs.
    // This is the documented behaviour; if it changes, update this test.
    assert_eq!(
        count_after - count_before,
        1,
        "salsa re-runs the query after any set_text call, even with identical text"
    );
}

// ---------------------------------------------------------------------------
// 8. Line index
// ---------------------------------------------------------------------------

#[test]
fn line_index_single_line() {
    let mut db = JairsDatabase::default();
    let sf = add_file(&mut db, "oneline.jr", "x := 1;");
    let idx = jr_db::line_index(&db, sf);
    assert_eq!(idx.line_starts, vec![0u32]);
    let lc = idx.line_col(0);
    assert_eq!(lc.line, 1);
    assert_eq!(lc.col, 1);
}

#[test]
fn line_index_multi_line() {
    let mut db = JairsDatabase::default();
    let sf = add_file(&mut db, "multi.jr", "a := 1;\nb := 2;\n");
    let idx = jr_db::line_index(&db, sf);
    assert_eq!(idx.line_starts, vec![0u32, 8, 16]);
    let lc = idx.line_col(8);
    assert_eq!(lc.line, 2);
    assert_eq!(lc.col, 1);
}

// ---------------------------------------------------------------------------
// 9. Corpus files
// ---------------------------------------------------------------------------

/// Parses every file in `tests/corpus/valid/` and asserts no errors.
///
/// The corpus is the shared truth between the compiler and tree-sitter; it
/// must always parse cleanly.
#[test]
fn corpus_valid_files_parse_without_errors() {
    let corpus_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/valid");

    let mut db = JairsDatabase::default();
    let mut failures = Vec::new();

    let entries = std::fs::read_dir(&corpus_dir).expect("corpus/valid directory must exist");

    for entry in entries {
        let entry = entry.expect("directory entry");
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jr") {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let sf = add_file(&mut db, &name, &text);
        let diags = parse_diagnostics(&db, sf);
        if diags.has_errors() {
            let msgs: Vec<_> = diags.iter().map(|d| d.message.clone()).collect();
            failures.push(format!("{name}: {msgs:?}"));
        }
    }

    assert!(
        failures.is_empty(),
        "corpus/valid files must parse without errors:\n{}",
        failures.join("\n")
    );
}

/// Parses every file in `tests/corpus/invalid/` and asserts that each
/// produces at least one diagnostic.
#[test]
fn corpus_invalid_files_produce_diagnostics() {
    let corpus_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/invalid");

    let mut db = JairsDatabase::default();
    let mut failures = Vec::new();

    let entries = std::fs::read_dir(&corpus_dir).expect("corpus/invalid directory must exist");

    for entry in entries {
        let entry = entry.expect("directory entry");
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jr") {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let sf = add_file(&mut db, &name, &text);
        let diags = parse_diagnostics(&db, sf);
        if !diags.has_errors() {
            failures.push(name);
        }
    }

    assert!(
        failures.is_empty(),
        "corpus/invalid files must produce at least one error:\n{}",
        failures.join("\n")
    );
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn add_file(db: &mut JairsDatabase, path: &str, text: &str) -> SourceFile {
    db.set_file_text(path, text);
    db.source_file(path)
        .expect("file must exist after set_file_text")
}

/// Joins a diagnostic's note and help lines into one string, for assertions
/// about content that is carried in the notes rather than the headline.
fn notes_text(diag: &jr_diag::Diagnostic) -> String {
    diag.notes
        .iter()
        .map(|(_, text)| text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// Incrementality, strengthened
// ---------------------------------------------------------------------------

/// Counting re-executions alone is not sufficient: a query that re-runs but
/// hands back a stale value would satisfy the count assertion perfectly. This
/// checks both halves at once — the right queries re-ran, AND the values are
/// actually correct afterwards.
#[test]
fn incremental_edit_yields_fresh_value_for_a_and_intact_value_for_b() {
    let (mut db, counter) = db_with_parse_counter();

    let sf_a = add_file(&mut db, "a.jr", "x := 1;");
    let sf_b = add_file(&mut db, "b.jr", "y := 2;");

    let text_a_before = parse_file(&db, sf_a).syntax().text().to_string();
    let text_b_before = parse_file(&db, sf_b).syntax().text().to_string();
    assert_eq!(text_a_before, "x := 1;");
    assert_eq!(text_b_before, "y := 2;");

    let baseline = counter.load(Ordering::SeqCst);

    db.set_file_text("a.jr", "x := 999;");

    let text_a_after = parse_file(&db, sf_a).syntax().text().to_string();
    let text_b_after = parse_file(&db, sf_b).syntax().text().to_string();

    // Freshness: A reflects the edit, B is untouched.
    assert_eq!(
        text_a_after, "x := 999;",
        "A's cached tree was served stale after an edit"
    );
    assert_eq!(
        text_b_after, text_b_before,
        "B changed even though it was never edited"
    );

    // Efficiency: exactly one re-execution, for A.
    assert_eq!(
        counter.load(Ordering::SeqCst) - baseline,
        1,
        "expected exactly one re-parse (A); B must be served from cache"
    );
}

/// Adding a third file must not invalidate the two already parsed. Otherwise
/// opening a file in an editor would re-analyse the whole project.
#[test]
fn adding_a_file_does_not_invalidate_existing_ones() {
    let (mut db, counter) = db_with_parse_counter();

    let sf_a = add_file(&mut db, "a.jr", "x := 1;");
    let sf_b = add_file(&mut db, "b.jr", "y := 2;");
    let _ = parse_file(&db, sf_a);
    let _ = parse_file(&db, sf_b);

    let baseline = counter.load(Ordering::SeqCst);

    let sf_c = add_file(&mut db, "c.jr", "z := 3;");
    let _ = parse_file(&db, sf_c);
    let _ = parse_file(&db, sf_a);
    let _ = parse_file(&db, sf_b);

    assert_eq!(
        counter.load(Ordering::SeqCst) - baseline,
        1,
        "adding a file must parse only that file"
    );
}

// ---------------------------------------------------------------------------
// Module loader helpers
// ---------------------------------------------------------------------------

/// Builds an in-memory module database with the corpus modules pre-loaded.
///
/// The search path is a single virtual directory `/modules`. Module files are
/// stored as `/modules/<Name>/module.jr` (directory form) or
/// `/modules/<Name>.jr` (single-file form).
fn make_module_db_with_corpus() -> (JairsDatabase, ModuleSearchPaths) {
    let mut modules = InMemoryModules::new();

    // Shapes — directory form
    modules.add(
        PathBuf::from("/modules/Shapes/module.jr"),
        r#"Rect :: struct {
    w: s64;
    h: s64;
}

area :: (r: Rect) -> s64 {
    return r.w * r.h;
}

SHAPES_VERSION :: 1;
"#,
    );

    // Colors — single-file form
    modules.add(
        PathBuf::from("/modules/Colors.jr"),
        r#"BLACK :: 0;
WHITE :: 255;

blend :: (a: s64, b: s64) -> s64 {
    return (a + b) / 2;
}
"#,
    );

    // Palette — single-file form
    modules.add(
        PathBuf::from("/modules/Palette.jr"),
        r#"blend :: (a: s64, b: s64) -> s64 {
    return a;
}

PALETTE_SIZE :: 16;
"#,
    );

    // Cycle_A — directory form
    modules.add(
        PathBuf::from("/modules/Cycle_A/module.jr"),
        r#"#import "Cycle_B";

A_VALUE :: 1;

a_calls_b :: () -> s64 {
    return b_value();
}
"#,
    );

    // Cycle_B — directory form
    modules.add(
        PathBuf::from("/modules/Cycle_B/module.jr"),
        r#"#import "Cycle_A";

b_value :: () -> s64 {
    return A_VALUE;
}
"#,
    );

    let mut db = JairsDatabase::with_in_memory_modules(modules);
    let sp = db.set_module_search_paths(vec![PathBuf::from("/modules")]);
    (db, sp)
}

/// Loads a file and all its transitive module imports into the database.
fn load_with_modules(db: &mut JairsDatabase, path: &str, text: &str) -> SourceFile {
    let sf = add_file(db, path, text);
    db.load_modules_transitively(sf);
    sf
}

// ---------------------------------------------------------------------------
// 10. module_file query
// ---------------------------------------------------------------------------

#[test]
fn module_file_finds_directory_form() {
    let (db, sp) = make_module_db_with_corpus();
    let result = module_file(&db, sp, Arc::from("Shapes"));
    assert!(
        result.found.is_some(),
        "Shapes/module.jr must be found; searched: {:?}",
        result.searched
    );
    let found = result.found.unwrap();
    assert!(
        found.ends_with("Shapes/module.jr"),
        "must find directory form first, got: {found:?}"
    );
}

#[test]
fn module_file_finds_single_file_form() {
    let (db, sp) = make_module_db_with_corpus();
    let result = module_file(&db, sp, Arc::from("Colors"));
    assert!(
        result.found.is_some(),
        "Colors.jr must be found; searched: {:?}",
        result.searched
    );
    let found = result.found.unwrap();
    assert!(
        found.ends_with("Colors.jr"),
        "must find single-file form, got: {found:?}"
    );
}

#[test]
fn module_file_not_found_lists_searched_paths() {
    let (db, sp) = make_module_db_with_corpus();
    let result = module_file(&db, sp, Arc::from("No_Such_Module"));
    assert!(result.found.is_none(), "No_Such_Module must not be found");
    // Must have searched both forms.
    assert!(
        result.searched.len() >= 2,
        "must have searched at least 2 paths, got: {:?}",
        result.searched
    );
    // Directory form must be tried before single-file form.
    let dir_idx = result
        .searched
        .iter()
        .position(|p| p.ends_with("No_Such_Module/module.jr"))
        .expect("directory form must be in searched list");
    let file_idx = result
        .searched
        .iter()
        .position(|p| p.ends_with("No_Such_Module.jr"))
        .expect("single-file form must be in searched list");
    assert!(
        dir_idx < file_idx,
        "directory form must be tried before single-file form"
    );
}

// ---------------------------------------------------------------------------
// 11. file_hir and file_exports
// ---------------------------------------------------------------------------

#[test]
fn file_hir_lowers_simple_file() {
    let mut db = JairsDatabase::default();
    let sf = add_file(&mut db, "simple.jr", "X :: 42;");
    let hir = file_hir(&db, sf);
    assert_eq!(hir.items.len(), 1, "must have one item");
}

#[test]
fn file_exports_contains_declared_names() {
    let mut db = JairsDatabase::default();
    let sf = add_file(&mut db, "exports.jr", "X :: 42;\nY :: 99;");
    let exports = file_exports(&db, sf);
    // The scope should contain X and Y.
    assert_eq!(
        exports.names.len(),
        2,
        "must export both X and Y, got: {:?}",
        exports.names.keys().collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// 12. imports_of
// ---------------------------------------------------------------------------

#[test]
fn imports_of_returns_module_names() {
    let mut db = JairsDatabase::default();
    let sf = add_file(
        &mut db,
        "importer.jr",
        r#"#import "Colors";
#import "Shapes";
main :: () {}
"#,
    );
    let names = imports_of(&db, sf);
    assert_eq!(names.len(), 2);
    assert!(names.iter().any(|n| n.as_ref() == "Colors"));
    assert!(names.iter().any(|n| n.as_ref() == "Shapes"));
}

#[test]
fn imports_of_deduplicates() {
    let mut db = JairsDatabase::default();
    let sf = add_file(
        &mut db,
        "dup.jr",
        r#"#import "Colors";
#import "Colors";
main :: () {}
"#,
    );
    let names = imports_of(&db, sf);
    assert_eq!(names.len(), 1, "duplicate imports must be deduplicated");
}

// ---------------------------------------------------------------------------
// 13. resolved — valid imports
// ---------------------------------------------------------------------------

#[test]
fn resolved_directory_module_no_diagnostics() {
    let (mut db, sp) = make_module_db_with_corpus();
    let sf = load_with_modules(
        &mut db,
        "test.jr",
        r#"#import "Shapes";
main :: () {
    r: Rect;
    r.w = 3;
    r.h = 4;
    a := area(r);
    v := SHAPES_VERSION;
}
"#,
    );
    let result = resolved(&db, sf, sp);
    let diags = &result.diagnostics;
    let errors: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == jr_diag::Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "directory module import must resolve without errors; got: {errors:?}"
    );
}

#[test]
fn resolved_single_file_module_no_diagnostics() {
    let (mut db, sp) = make_module_db_with_corpus();
    let sf = load_with_modules(
        &mut db,
        "test.jr",
        r#"#import "Colors";
main :: () {
    mid := blend(BLACK, WHITE);
}
"#,
    );
    let result = resolved(&db, sf, sp);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.severity == jr_diag::Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "single-file module import must resolve without errors; got: {errors:?}"
    );
}

// ---------------------------------------------------------------------------
// 14. E0210 — module not found
// ---------------------------------------------------------------------------

#[test]
fn e0210_module_not_found_with_searched_paths() {
    let (mut db, sp) = make_module_db_with_corpus();
    let sf = add_file(
        &mut db,
        "missing.jr",
        r#"#import "No_Such_Module";
main :: () {}
"#,
    );
    let result = resolved(&db, sf, sp);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.code == Some("E0210"))
        .collect();
    assert_eq!(errors.len(), 1, "must produce exactly one E0210");
    let msg = &errors[0].message;
    assert!(
        msg.contains("No_Such_Module"),
        "E0210 must name the missing module; got: {msg:?}"
    );
    // The searched paths live in the notes, one per line -- a multi-line
    // headline renders with every continuation line indented under the
    // message, which is unreadable with more than a couple of paths.
    let notes = notes_text(errors[0]);
    assert!(
        notes.contains("searched"),
        "E0210 must list searched paths; got notes: {notes:?}"
    );
    // Must list at least one path.
    assert!(
        notes.contains("/modules/"),
        "E0210 must include the search directory; got notes: {notes:?}"
    );
}

// ---------------------------------------------------------------------------
// 15. Cycles are legal (ADR-0014 §4)
// ---------------------------------------------------------------------------

#[test]
fn import_cycle_is_legal() {
    let (mut db, sp) = make_module_db_with_corpus();
    // Load Cycle_A (which imports Cycle_B which imports Cycle_A).
    let sf = load_with_modules(
        &mut db,
        "test.jr",
        r#"#import "Cycle_A";
main :: () {
    x := a_calls_b();
    y := A_VALUE;
}
"#,
    );
    // This must not panic or infinite-loop.
    let result = resolved(&db, sf, sp);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.severity == jr_diag::Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "import cycle must be legal (ADR-0014 §4); got errors: {errors:?}"
    );
}

#[test]
fn self_import_terminates() {
    let (mut db, sp) = make_module_db_with_corpus();
    // A file that imports itself by name. We need to set up the search path
    // to point to a directory containing this file.
    // For simplicity, use a module name that doesn't exist — the self-import
    // detection is based on path matching, so we test it via the corpus cycle.
    // The real self-import test: load Cycle_A which imports Cycle_B which
    // imports Cycle_A — this exercises the cycle termination.
    let sf = load_with_modules(
        &mut db,
        "test.jr",
        r#"#import "Cycle_A";
main :: () {}
"#,
    );
    // Must terminate (not infinite-loop).
    let _result = resolved(&db, sf, sp);
}

// ---------------------------------------------------------------------------
// 16. Duplicate import is idempotent (ADR-0014 §6)
// ---------------------------------------------------------------------------

#[test]
fn duplicate_import_is_idempotent() {
    let (mut db, sp) = make_module_db_with_corpus();
    let sf = load_with_modules(
        &mut db,
        "test.jr",
        r#"#import "Colors";
#import "Colors";
main :: () {
    x := blend(BLACK, WHITE);
}
"#,
    );
    let result = resolved(&db, sf, sp);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.severity == jr_diag::Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "duplicate import must be idempotent; got: {errors:?}"
    );
}

// ---------------------------------------------------------------------------
// 17. Local shadows import (ADR-0014 §3)
// ---------------------------------------------------------------------------

#[test]
fn local_shadows_import_no_error() {
    let (mut db, sp) = make_module_db_with_corpus();
    let sf = load_with_modules(
        &mut db,
        "test.jr",
        r#"#import "Colors";

blend :: (a: s64, b: s64) -> s64 {
    return a - b;
}

main :: () {
    x := blend(10, 4);
}
"#,
    );
    let result = resolved(&db, sf, sp);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.severity == jr_diag::Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "local declaration must shadow import without error; got: {errors:?}"
    );
}

// ---------------------------------------------------------------------------
// 18. file_diagnostics
// ---------------------------------------------------------------------------

#[test]
fn file_diagnostics_no_errors_for_valid_import() {
    let (mut db, sp) = make_module_db_with_corpus();
    let sf = load_with_modules(
        &mut db,
        "test.jr",
        r#"#import "Colors";
main :: () {
    x := blend(BLACK, WHITE);
}
"#,
    );
    let diags = file_diagnostics(&db, sf, sp);
    let errors: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == jr_diag::Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "valid import must produce no errors; got: {errors:?}"
    );
}

#[test]
fn file_diagnostics_e0210_for_missing_module() {
    let (mut db, sp) = make_module_db_with_corpus();
    let sf = add_file(
        &mut db,
        "missing.jr",
        r#"#import "No_Such_Module";
main :: () {}
"#,
    );
    let diags = file_diagnostics(&db, sf, sp);
    let e0210: Vec<_> = diags.iter().filter(|d| d.code == Some("E0210")).collect();
    assert_eq!(e0210.len(), 1, "must produce exactly one E0210");
}

// ---------------------------------------------------------------------------
// 19. Corpus imports/valid — all must check cleanly
// ---------------------------------------------------------------------------

#[test]
fn corpus_imports_valid_check_cleanly() {
    let corpus_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/imports/valid");
    let modules_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/modules");

    let mut db = JairsDatabase::default();
    let sp = db.set_module_search_paths(vec![modules_dir]);

    let mut failures = Vec::new();

    let entries = std::fs::read_dir(&corpus_dir).expect("corpus/imports/valid must exist");
    for entry in entries {
        let entry = entry.expect("directory entry");
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jr") {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let sf = add_file(&mut db, &name, &text);
        db.load_modules_transitively(sf);
        let diags = file_diagnostics(&db, sf, sp);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == jr_diag::Severity::Error)
            .collect();
        if !errors.is_empty() {
            let msgs: Vec<_> = errors
                .iter()
                .map(|d| format!("[{}] {}", d.code.unwrap_or("?"), d.message))
                .collect();
            failures.push(format!("{name}: {msgs:?}"));
        }
    }

    assert!(
        failures.is_empty(),
        "corpus/imports/valid files must check without errors:\n{}",
        failures.join("\n")
    );
}

// ---------------------------------------------------------------------------
// 20. Corpus imports/invalid/001 — E0210 with searched paths
// ---------------------------------------------------------------------------

#[test]
fn corpus_imports_invalid_001_produces_e0210() {
    let corpus_file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/imports/invalid/001-module-not-found.jr");
    let modules_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/modules");

    let text = std::fs::read_to_string(&corpus_file).expect("001-module-not-found.jr must exist");

    let mut db = JairsDatabase::default();
    let sp = db.set_module_search_paths(vec![modules_dir.clone()]);
    let sf = add_file(&mut db, "001-module-not-found.jr", &text);

    let diags = file_diagnostics(&db, sf, sp);
    let e0210: Vec<_> = diags.iter().filter(|d| d.code == Some("E0210")).collect();

    assert_eq!(e0210.len(), 1, "must produce exactly one E0210");

    let msg = &e0210[0].message;
    assert!(
        msg.contains("No_Such_Module"),
        "E0210 must name the missing module; got: {msg:?}"
    );
    let notes = notes_text(e0210[0]);
    assert!(
        notes.contains("searched"),
        "E0210 must say 'searched'; got notes: {notes:?}"
    );
    // The searched paths must include the modules directory.
    let modules_dir_str = modules_dir.to_string_lossy();
    assert!(
        notes.contains(modules_dir_str.as_ref()),
        "E0210 must list the modules directory ({modules_dir_str}); got notes: {notes:?}"
    );
}

// ---------------------------------------------------------------------------
// 21. Incrementality: editing a module invalidates its importers
// ---------------------------------------------------------------------------

/// Editing a module file must invalidate `file_diagnostics` for files that
/// import it, but NOT for unrelated files.
///
/// We measure this with the `WillExecute` event counter, counting re-runs of
/// `file_hir` (which is the first query that depends on the module's text).
#[test]
fn editing_module_invalidates_importers_not_unrelated() {
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();

    let mut modules = InMemoryModules::new();
    modules.add(PathBuf::from("/modules/MyMod.jr"), "MY_CONST :: 1;\n");

    let mut db = JairsDatabase::with_event_callback_and_modules(
        move |event| {
            if let salsa::EventKind::WillExecute { database_key } = event.kind {
                let name = format!("{database_key:?}");
                if name.contains("file_hir") {
                    counter_clone.fetch_add(1, Ordering::SeqCst);
                }
            }
        },
        modules,
    );

    let _sp = db.set_module_search_paths(vec![PathBuf::from("/modules")]);

    // importer.jr imports MyMod; unrelated.jr does not.
    let sf_importer = load_with_modules(
        &mut db,
        "importer.jr",
        "#import \"MyMod\";\nmain :: () { x := MY_CONST; }\n",
    );
    let sf_unrelated = add_file(&mut db, "unrelated.jr", "Y :: 99;\n");

    // Warm up the cache.
    let _ = file_hir(&db, sf_importer);
    let _ = file_hir(&db, sf_unrelated);
    // Also warm up the module file.
    let mymod_path = PathBuf::from("/modules/MyMod.jr");
    let mymod_sf = db
        .source_file_for_path(&mymod_path.to_string_lossy())
        .unwrap();
    let _ = file_hir(&db, mymod_sf);

    let baseline = counter.load(Ordering::SeqCst);

    // Edit the module file.
    db.set_file_text("/modules/MyMod.jr", "MY_CONST :: 2;\n");

    // Re-query all three files.
    let _ = file_hir(&db, sf_importer);
    let _ = file_hir(&db, sf_unrelated);
    let _ = file_hir(&db, mymod_sf);

    let new_executions = counter.load(Ordering::SeqCst) - baseline;

    // MyMod was edited → its file_hir re-runs (1).
    // importer.jr depends on MyMod's exports via resolved(), but file_hir
    // for importer.jr depends only on parse_file(importer.jr), which didn't
    // change. So file_hir(importer.jr) does NOT re-run.
    // unrelated.jr: no dependency on MyMod → does NOT re-run.
    // Total: exactly 1 re-execution (for MyMod itself).
    assert_eq!(
        new_executions, 1,
        "editing MyMod must re-run file_hir exactly once (for MyMod); \
         importer and unrelated must be served from cache. Got {new_executions} re-executions."
    );
}

// ---------------------------------------------------------------------------
// Invalidation actually propagating through the module graph
// ---------------------------------------------------------------------------

/// Builds a database counting `WillExecute` events whose key mentions `needle`.
fn db_counting(
    needle: &'static str,
    modules: InMemoryModules,
) -> (JairsDatabase, Arc<AtomicUsize>) {
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();
    let db = JairsDatabase::with_event_callback_and_modules(
        move |event| {
            if let salsa::EventKind::WillExecute { database_key } = event.kind
                && format!("{database_key:?}").contains(needle)
            {
                counter_clone.fetch_add(1, Ordering::SeqCst);
            }
        },
        modules,
    );
    (db, counter)
}

/// Editing a module must invalidate its importers' RESOLUTION.
///
/// `editing_module_invalidates_importers_not_unrelated` counts `file_hir`, which
/// correctly does *not* re-run for the importer — the importer's own text did not
/// change. That proves lowering is properly isolated, but it says nothing about
/// whether the importer is re-resolved, which is the property that actually
/// matters: if `resolved(importer)` were served from cache after its dependency
/// changed, the compiler would report stale diagnostics about a module that no
/// longer looks like that.
///
/// So: `resolved(importer)` MUST re-run, and `resolved(unrelated)` must NOT.
#[test]
fn editing_a_module_re_resolves_importers_and_only_importers() {
    let mut modules = InMemoryModules::new();
    modules.add(PathBuf::from("/modules/MyMod.jr"), "MY_CONST :: 1;\n");

    let (mut db, counter) = db_counting("resolved", modules);
    let sp = db.set_module_search_paths(vec![PathBuf::from("/modules")]);

    let importer = load_with_modules(
        &mut db,
        "importer.jr",
        "#import \"MyMod\";\nmain :: () { x := MY_CONST; }\n",
    );
    let unrelated = add_file(&mut db, "unrelated.jr", "Y :: 99;\n");

    // Warm the cache and confirm we start from a clean resolution.
    assert!(
        resolved(&db, importer, sp).diagnostics.is_empty(),
        "importer must resolve cleanly before the edit"
    );
    let _ = resolved(&db, unrelated, sp);

    let baseline = counter.load(Ordering::SeqCst);

    // Edit the module that `importer.jr` depends on.
    db.set_file_text("/modules/MyMod.jr", "MY_CONST :: 2;\n");

    let _ = resolved(&db, importer, sp);
    let _ = resolved(&db, unrelated, sp);

    let reruns = counter.load(Ordering::SeqCst) - baseline;
    assert_eq!(
        reruns, 1,
        "expected exactly one re-resolution (the importer); \
         0 means the importer was served stale after its dependency changed, \
         2 means the unrelated file was invalidated needlessly. Got {reruns}."
    );
}

/// The other half of the same property: if a module stops exporting a name, the
/// importer must start reporting it as unresolved. This is the observable
/// consequence of the invalidation above, checked on values rather than on
/// execution counts — a query can re-run and still hand back a stale result.
#[test]
fn removing_an_export_makes_importers_report_it_unresolved() {
    let mut modules = InMemoryModules::new();
    modules.add(PathBuf::from("/modules/MyMod.jr"), "MY_CONST :: 1;\n");

    let (mut db, _counter) = db_counting("resolved", modules);
    let sp = db.set_module_search_paths(vec![PathBuf::from("/modules")]);

    let importer = load_with_modules(
        &mut db,
        "importer.jr",
        "#import \"MyMod\";\nmain :: () { x := MY_CONST; }\n",
    );

    assert!(
        resolved(&db, importer, sp).diagnostics.is_empty(),
        "MY_CONST must resolve while the module exports it"
    );

    // The module no longer exports MY_CONST.
    db.set_file_text("/modules/MyMod.jr", "SOMETHING_ELSE :: 1;\n");

    let after = resolved(&db, importer, sp);
    assert!(
        after.diagnostics.iter().any(|d| d.code == Some("E0201")),
        "removing the export must make the importer report E0201, got {:?}",
        after
            .diagnostics
            .iter()
            .map(|d| (d.code, d.message.clone()))
            .collect::<Vec<_>>()
    );
}

/// And the inverse: adding the export back must clear the error, proving the
/// cache is not sticky in the other direction either.
#[test]
fn restoring_an_export_clears_the_importers_error() {
    let mut modules = InMemoryModules::new();
    modules.add(PathBuf::from("/modules/MyMod.jr"), "WRONG :: 1;\n");

    let (mut db, _counter) = db_counting("resolved", modules);
    let sp = db.set_module_search_paths(vec![PathBuf::from("/modules")]);

    let importer = load_with_modules(
        &mut db,
        "importer.jr",
        "#import \"MyMod\";\nmain :: () { x := MY_CONST; }\n",
    );

    assert!(
        resolved(&db, importer, sp)
            .diagnostics
            .iter()
            .any(|d| d.code == Some("E0201")),
        "MY_CONST must be unresolved before the module exports it"
    );

    db.set_file_text("/modules/MyMod.jr", "MY_CONST :: 7;\n");

    assert!(
        resolved(&db, importer, sp).diagnostics.is_empty(),
        "adding the export back must clear the importer's error"
    );
}

// ---------------------------------------------------------------------------
// 15. Semantic analysis
// ---------------------------------------------------------------------------

/// Type errors must reach `file_diagnostics`, or `jr check` never sees them.
#[test]
fn file_diagnostics_reports_type_errors() {
    let (db, sp) = make_module_db_with_corpus();
    let mut db = db;
    let file = add_file(&mut db, "a.jr", "main :: () {\n    g: u8 = 300;\n}\n");
    let diags = file_diagnostics(&db, file, sp);
    assert!(
        diags.iter().any(|d| d.code == Some("E0204")),
        "expected E0204, got {:?}",
        diags.iter().map(|d| d.code).collect::<Vec<_>>()
    );
}

/// A well-formed program that uses an imported struct and procedure must check
/// cleanly — which requires the importer and the module to agree on one `PoolId`
/// for `Rect`, and therefore requires them to share a pool.
#[test]
fn checking_a_call_into_an_imported_module_succeeds() {
    let (mut db, sp) = make_module_db_with_corpus();
    let importer = load_with_modules(
        &mut db,
        "importer.jr",
        "#import \"Shapes\";\n\nmain :: () {\n    r: Rect;\n    r.w = 3;\n    r.h = 4;\n    a := area(r);\n}\n",
    );
    let diags = file_diagnostics(&db, importer, sp);
    assert!(
        diags.is_empty(),
        "the importer must check cleanly, got {:?}",
        diags
            .iter()
            .map(|d| (d.code, d.message.clone()))
            .collect::<Vec<_>>()
    );
}

/// The same call with the wrong argument type must fail, so that the previous
/// test cannot pass by having poisoned everything.
#[test]
fn checking_catches_a_bad_argument_to_an_imported_procedure() {
    let (mut db, sp) = make_module_db_with_corpus();
    let importer = load_with_modules(
        &mut db,
        "importer.jr",
        "#import \"Shapes\";\n\nmain :: () {\n    a := area(1);\n}\n",
    );
    let diags = file_diagnostics(&db, importer, sp);
    assert!(
        diags.iter().any(|d| d.code == Some("E0214")),
        "expected a type mismatch, got {:?}",
        diags.iter().map(|d| d.code).collect::<Vec<_>>()
    );
}

/// Two modules that import each other must type-check. If signatures depended on
/// another file's *check*, this would be a salsa cycle rather than a test.
#[test]
fn a_module_cycle_type_checks_through_the_database() {
    let (mut db, sp) = make_module_db_with_corpus();
    let importer = load_with_modules(
        &mut db,
        "importer.jr",
        "#import \"Cycle_A\";\n\nmain :: () {\n    x := a_calls_b();\n    y := A_VALUE;\n}\n",
    );
    let diags = file_diagnostics(&db, importer, sp);
    // No *errors*. The body reads `A_VALUE` — an imported constant, which `jr-mir` still refuses
    // to lower — so E0245 warns about it (ADR-0047 §2). That warning is the honest state of
    // affairs and asserting `is_empty` would conflate it with a type error, which is the
    // distinction this test is about.
    assert!(
        !diags.has_errors(),
        "an import cycle is legal and must type-check, got {:?}",
        diags
            .iter()
            .map(|d| (d.code, d.message.clone()))
            .collect::<Vec<_>>()
    );

    // And each module in the cycle must check on its own terms too.
    for path in ["/modules/Cycle_A/module.jr", "/modules/Cycle_B/module.jr"] {
        let module = db.source_file(path).expect("module must be loaded");
        assert!(
            checked(&db, module, sp).diagnostics.is_empty(),
            "{path} must type-check"
        );
    }
}

/// Editing a module's declaration must change what its importer thinks the name
/// means. Signatures are cached, so this is the test that the cache invalidates.
#[test]
fn changing_a_modules_declaration_retypes_the_importer() {
    let mut modules = InMemoryModules::new();
    modules.add(PathBuf::from("/modules/MyMod.jr"), "MY_CONST :: 1;\n");

    let mut db = JairsDatabase::with_in_memory_modules(modules);
    let sp = db.set_module_search_paths(vec![PathBuf::from("/modules")]);

    let importer = load_with_modules(
        &mut db,
        "importer.jr",
        "#import \"MyMod\";\n\nmain :: () {\n    flag: bool = MY_CONST;\n}\n",
    );

    assert!(
        file_diagnostics(&db, importer, sp)
            .iter()
            .any(|d| d.code == Some("E0214")),
        "an `s64` constant must not satisfy a `bool` annotation"
    );

    db.set_file_text("/modules/MyMod.jr", "MY_CONST :: false;\n");

    let after = file_diagnostics(&db, importer, sp);
    // Errors only, for the reason above: the imported constant still draws an E0245 warning.
    assert!(
        !after.has_errors(),
        "changing the constant's type must clear the importer's error, got {:?}",
        after
            .iter()
            .map(|d| (d.code, d.message.clone()))
            .collect::<Vec<_>>()
    );
}

/// Editing an importer must not recompute the module's signatures. Otherwise
/// typing in one file re-analyses every module it imports, which is the cost
/// salsa exists to avoid.
#[test]
fn editing_an_importer_does_not_recompute_a_modules_signatures() {
    let mut modules = InMemoryModules::new();
    modules.add(PathBuf::from("/modules/MyMod.jr"), "MY_CONST :: 1;\n");

    let (mut db, counter) = db_counting("file_signatures", modules);
    let sp = db.set_module_search_paths(vec![PathBuf::from("/modules")]);

    let importer = load_with_modules(
        &mut db,
        "importer.jr",
        "#import \"MyMod\";\n\nmain :: () {\n    x := MY_CONST;\n}\n",
    );
    let _ = file_diagnostics(&db, importer, sp);
    let module = db
        .source_file("/modules/MyMod.jr")
        .expect("module must be loaded");
    let _ = file_signatures(&db, module, sp);

    let baseline = counter.load(Ordering::SeqCst);

    db.set_file_text(
        "importer.jr",
        "#import \"MyMod\";\n\nmain :: () {\n    x := MY_CONST;\n    y := MY_CONST;\n}\n",
    );
    let _ = file_diagnostics(&db, importer, sp);
    let after_importer_edit = counter.load(Ordering::SeqCst) - baseline;

    // The importer's own signatures must be recomputed; the module's must not.
    assert_eq!(
        after_importer_edit, 1,
        "expected exactly one signature recomputation (the importer's)"
    );
}

/// The type map must survive the query boundary: a checked file knows the types
/// of its expressions, not merely whether it had errors.
#[test]
fn checking_records_the_types_it_learned() {
    let (mut db, sp) = make_module_db_with_corpus();
    let file = add_file(
        &mut db,
        "a.jr",
        "main :: () {\n    n := 1;\n    flag := true;\n}\n",
    );
    let result = checked(&db, file, sp);
    assert!(
        result.types.expr_count() >= 2,
        "the checker must record the types it computed"
    );
    assert!(result.types.local_count() >= 2);
}
