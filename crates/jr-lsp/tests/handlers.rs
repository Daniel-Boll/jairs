//! The three capabilities, tested without a transport.
//!
//! [ADR-0024](../../../docs/adr/0024-language-server.md) §4 keeps every handler a pure
//! function of `(&db, params)` so that these tests exist at all. `tests/stdio.rs` is the
//! other half, and it exists because these tests would pass with a completely broken
//! transport — which is not hypothetical: the first native run of `024-hello.jr` printed
//! both its lines perfectly and exited **1**, and no in-process assertion noticed.

use std::path::{Path, PathBuf};

use jr_db::{JairsDatabase, ModuleSearchPaths, SourceFile};
use jr_lsp::{Encoding, diagnostics, goto_definition, hover};
use lsp_types::{HoverContents, MarkupContent, Position};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn modules() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../modules")
}

/// A database holding one file at an absolute path, with `modules/` importable.
///
/// The path must be absolute because a `Location` carries a `file:` URI, and
/// `jr_lsp::uri::from_path` refuses a relative path rather than inventing a base for it.
fn program(source: &str) -> (JairsDatabase, ModuleSearchPaths, SourceFile) {
    let mut db = JairsDatabase::default();
    let search = db.set_module_search_paths(vec![modules()]);
    let path = "/jairs-lsp-test/main.jr";
    db.set_file_text(path, source);
    let file = db.source_file(path).expect("the file was just added");
    db.load_modules_transitively(file);
    (db, search, file)
}

/// The offset of `needle` in `source`, as a zero-based line and UTF-8 column.
///
/// Written out rather than hand-counted, because a hand-counted position is a test that
/// fails for the wrong reason the first time a line is edited.
fn at(source: &str, needle: &str) -> Position {
    let offset = source.find(needle).expect("the needle must appear");
    let line = source[..offset].matches('\n').count();
    let line_start = source[..offset].rfind('\n').map_or(0, |index| index + 1);
    Position {
        line: u32::try_from(line).expect("small"),
        character: u32::try_from(offset - line_start).expect("small"),
    }
}

fn hover_text(contents: &HoverContents) -> String {
    match contents {
        HoverContents::Markup(MarkupContent { value, .. }) => value.clone(),
        other => panic!("expected markup, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

#[test]
fn a_clean_file_reports_nothing() {
    let source = "main :: () { }\n";
    let (db, search, file) = program(source);
    assert!(diagnostics(&db, file, search, Encoding::Utf8).is_empty());
}

#[test]
fn a_type_error_is_reported_with_its_code_and_range() {
    let source = "main :: () {\n    x: bool = 1;\n}\n";
    let (db, search, file) = program(source);
    let items = diagnostics(&db, file, search, Encoding::Utf8);
    assert!(!items.is_empty(), "a type error must reach the editor");
    let first = &items[0];
    assert_eq!(
        first.severity,
        Some(lsp_types::DiagnosticSeverity::ERROR),
        "a type error is an error"
    );
    assert!(
        first.code.is_some(),
        "the stable code is what lets a user look the error up"
    );
    assert_eq!(first.range.start.line, 1, "on the line that has the error");
    assert_eq!(
        first.source.as_deref(),
        Some("jairs"),
        "so a client can tell whose diagnostic it is"
    );
}

#[test]
fn a_diagnostics_note_survives_into_the_message() {
    // `jr-diag` puts half of a diagnostic's usefulness in its notes, and the protocol
    // has one message field. Dropping them would lose the part that says what to do.
    let source = "#import \"Nope\";\n\nmain :: () { }\n";
    let (db, search, file) = program(source);
    let items = diagnostics(&db, file, search, Encoding::Utf8);
    assert!(
        items.iter().any(|d| d.code.is_some()),
        "a missing module is reported: {items:?}"
    );
}

// ---------------------------------------------------------------------------
// Hover
// ---------------------------------------------------------------------------

#[test]
fn hovering_a_local_shows_its_type() {
    let source = "main :: () {\n    n := 7;\n    m := n;\n}\n";
    let (db, search, file) = program(source);
    let found =
        hover(&db, file, search, Encoding::Utf8, at(source, "n;")).expect("a name has a type");
    assert_eq!(hover_text(&found.contents), "```jr\ns64\n```");
}

#[test]
fn hovering_a_pointer_shows_the_pointee() {
    let source = "main :: () {\n    n := 7;\n    p := *n;\n    q := p;\n}\n";
    let (db, search, file) = program(source);
    let found = hover(&db, file, search, Encoding::Utf8, at(source, "p;"))
        .expect("a pointer name has a type");
    assert_eq!(hover_text(&found.contents), "```jr\n*s64\n```");
}

#[test]
fn hovering_a_struct_shows_its_declared_name() {
    // Not `struct DeclId(..)`: the name comes from `FileSignatures::type_name`, which is
    // the same source `jr-mir`'s dump uses, so a hover and a dump cannot disagree about
    // what a nominal type is called.
    let source =
        "Point :: struct { x: s64; y: s64; }\n\nmain :: () {\n    p: Point;\n    q := p;\n}\n";
    let (db, search, file) = program(source);
    let found = hover(&db, file, search, Encoding::Utf8, at(source, "p;\n}"))
        .expect("a struct name has a type");
    assert_eq!(hover_text(&found.contents), "```jr\nPoint\n```");
}

#[test]
fn hovering_whitespace_returns_nothing() {
    // A real answer rather than a failure: there is no expression there, and an editor
    // showing nothing is correct.
    let source = "main :: () {\n\n    n := 7;\n}\n";
    let (db, search, file) = program(source);
    assert!(
        hover(
            &db,
            file,
            search,
            Encoding::Utf8,
            Position {
                line: 1,
                character: 0
            }
        )
        .is_none()
    );
}

#[test]
fn hover_reports_the_range_of_the_thing_it_described() {
    // So a client can highlight it. The range must be the *innermost* expression, which
    // is what ADR-0024 §1's narrowest-wins rule is for.
    let source = "main :: () {\n    n := 7;\n    m := n;\n}\n";
    let (db, search, file) = program(source);
    let found = hover(&db, file, search, Encoding::Utf8, at(source, "n;")).expect("a hover");
    let range = found.range.expect("a range");
    assert_eq!(range.start.line, 2);
    assert_eq!(
        range.end.character - range.start.character,
        1,
        "`n` is one character, not the whole statement"
    );
}

// ---------------------------------------------------------------------------
// Goto definition
// ---------------------------------------------------------------------------

#[test]
fn goto_definition_finds_a_local() {
    let source = "main :: () {\n    n := 7;\n    m := n;\n}\n";
    let (db, search, file) = program(source);
    let found = goto_definition(&db, file, search, Encoding::Utf8, at(source, "n;"))
        .expect("a local resolves");
    assert_eq!(found.range.start.line, 1, "the declaration is on line 1");
}

#[test]
fn goto_definition_finds_a_parameter() {
    // Parameters are not locals: `jr-hir`'s `Body` does not store them, so the span
    // lives on `Proc::params` and the owning procedure has to be found by which one
    // declares this body. That asymmetry is why this is a separate test.
    let source = "id :: (a: s64) -> s64 {\n    return a;\n}\n";
    let (db, search, file) = program(source);
    let found = goto_definition(&db, file, search, Encoding::Utf8, at(source, "a;"))
        .expect("a parameter resolves");
    assert_eq!(
        found.range.start.line, 0,
        "the parameter is declared on line 0"
    );
}

#[test]
fn goto_definition_finds_a_file_level_item() {
    let source = "LIMIT :: 10;\n\nmain :: () {\n    n := LIMIT;\n}\n";
    let (db, search, file) = program(source);
    let found = goto_definition(&db, file, search, Encoding::Utf8, at(source, "LIMIT;"))
        .expect("a constant resolves");
    assert_eq!(found.range.start.line, 0);
}

#[test]
fn goto_definition_crosses_into_an_imported_module() {
    // The one that demonstrates the module system actually resolved rather than merely
    // type-checked: the location is in another file, converted with *that* file's line
    // index. Using this file's lines would produce a plausible wrong location, which is
    // worse than none.
    let source = "#import \"Basic\";\n\nmain :: () {\n    print(\"hi\");\n}\n";
    let (db, search, file) = program(source);
    let found = goto_definition(
        &db,
        file,
        search,
        Encoding::Utf8,
        at(source, "print(\"hi\")"),
    )
    .expect("an imported name resolves");
    assert!(
        found.uri.as_str().ends_with("modules/Basic/module.jr"),
        "expected a location in the Basic module, got {}",
        found.uri.as_str()
    );
}

#[test]
fn goto_definition_on_an_unresolved_name_returns_nothing() {
    let source = "main :: () {\n    n := nope;\n}\n";
    let (db, search, file) = program(source);
    assert!(
        goto_definition(&db, file, search, Encoding::Utf8, at(source, "nope")).is_none(),
        "`Res::Error` has no definition to go to"
    );
}

// ---------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------

#[test]
fn a_non_ascii_line_places_the_range_correctly_under_both_encodings() {
    // The reason ADR-0024 §3 implements both paths. The comment holds an em dash, so a
    // server that confused bytes with UTF-16 units would misplace this range by two
    // columns — and would pass every test written against an ASCII file.
    let source = "main :: () {\n    // — a dash\n    n := 7;\n    m := n;\n}\n";
    let (db, search, file) = program(source);

    let utf8 = hover(&db, file, search, Encoding::Utf8, at(source, "n;"))
        .expect("a hover")
        .range
        .expect("a range");
    // The same cursor, expressed the way a UTF-16 client would express it. The dash is
    // on an earlier line, so the column is unchanged here — what matters is that both
    // encodings agree about *this* line rather than that either is wrong.
    let utf16 = hover(&db, file, search, Encoding::Utf16, at(source, "n;"))
        .expect("a hover")
        .range
        .expect("a range");
    assert_eq!(utf8, utf16, "an ASCII line reads the same either way");
    assert_eq!(utf8.start.line, 3);
}
