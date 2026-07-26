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

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use jr_db::{Db as _, JairsDatabase, SourceFile, parse_diagnostics, parse_file};

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
