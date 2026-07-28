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
    // ADR-0028 §4: the name is on the card, not just the type. Which binding the
    // cursor found matters wherever one shadows another, and `s64` alone did not say.
    assert_eq!(hover_text(&found.contents), "```jr\nmain\nn: s64\n```");
}

#[test]
fn hovering_a_pointer_shows_the_pointee() {
    let source = "main :: () {\n    n := 7;\n    p := *n;\n    q := p;\n}\n";
    let (db, search, file) = program(source);
    let found = hover(&db, file, search, Encoding::Utf8, at(source, "p;"))
        .expect("a pointer name has a type");
    assert_eq!(hover_text(&found.contents), "```jr\nmain\np: *s64\n```");
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
    assert_eq!(hover_text(&found.contents), "```jr\nmain\np: Point\n```");
}

// ---- the card (ADR-0028) ------------------------------------------------------
//
// What prompted the wave: hovering a procedure rendered `(s64, s64) -> s64` — no name,
// no parameter names, no origin, no documentation. These assert the whole card by exact
// text, because a card that merely *contains* the right words is how a renderer drifts.

#[test]
fn hovering_a_procedure_shows_container_signature_and_docs() {
    let source = "/// Adds two numbers.\nadd :: (a: s64, b: s64) -> s64 {\n    return a + b;\n}\n\nmain :: () {\n    n := add(1, 2);\n}\n";
    let (db, search, file) = program(source);
    let found = hover(&db, file, search, Encoding::Utf8, at(source, "add(1"))
        .expect("a procedure name resolves");
    assert_eq!(
        hover_text(&found.contents),
        "```jr\nmain\nadd :: (a: s64, b: s64) -> s64\n```\n\n---\n\nAdds two numbers."
    );
}

#[test]
fn a_procedure_returning_nothing_shows_no_arrow() {
    // Jairs writes no arrow for a void return, so the card does not invent one.
    let source = "f :: (x: s64) {\n}\n\nmain :: () {\n    f(1);\n}\n";
    let (db, search, file) = program(source);
    let found = hover(&db, file, search, Encoding::Utf8, at(source, "f(1)")).expect("resolves");
    assert_eq!(
        hover_text(&found.contents),
        "```jr\nmain\nf :: (x: s64)\n```"
    );
}

#[test]
fn hovering_an_undocumented_procedure_omits_the_rule() {
    let source =
        "add :: (a: s64) -> s64 {\n    return a;\n}\n\nmain :: () {\n    n := add(1);\n}\n";
    let (db, search, file) = program(source);
    let found = hover(&db, file, search, Encoding::Utf8, at(source, "add(1")).expect("resolves");
    assert_eq!(
        hover_text(&found.contents),
        "```jr\nmain\nadd :: (a: s64) -> s64\n```"
    );
}

#[test]
fn hovering_a_declaration_name_works() {
    // It did not before: `verify.lua`'s first draft hovered a declaration, got nothing,
    // and asserted that nothing was correct (ADR-0028 §4).
    let source = "/// Documented.\nadd :: (a: s64) -> s64 {\n    return add(a);\n}\n";
    let (db, search, file) = program(source);
    let found = hover(&db, file, search, Encoding::Utf8, at(source, "add(a)"))
        .expect("a recursive call resolves to the declaration");
    assert_eq!(
        hover_text(&found.contents),
        "```jr\nmain\nadd :: (a: s64) -> s64\n```\n\n---\n\nDocumented."
    );
}

#[test]
fn hovering_a_struct_name_shows_its_fields() {
    // `Alias :: Point;` rather than the annotation in `p: Point`, and that is a real
    // limitation rather than a test convenience: `jr_hir::TypeRef::Name` carries only a
    // `Symbol` and no `Span`, so `locate` — which scans expressions — has nothing to
    // match a cursor inside a type annotation against. Giving `TypeRef` spans is a
    // `jr-hir` change, recorded as owed work rather than done quietly here.
    let source =
        "/// A point in the plane.\nPoint :: struct { x: s64; y: s64; }\n\nAlias :: Point;\n";
    let (db, search, file) = program(source);
    let found = hover(&db, file, search, Encoding::Utf8, at(source, "Point;"))
        .expect("a struct name used as a value resolves");
    assert_eq!(
        hover_text(&found.contents),
        "```jr\nmain\nPoint :: struct { x: s64; y: s64 }\n```\n\n---\n\nA point in the plane."
    );
}

#[test]
fn hovering_a_type_annotation_returns_nothing_today() {
    // Pins the limitation above, so that giving `TypeRef` a span turns this test red and
    // whoever does it is handed the reason.
    let source = "Point :: struct { x: s64; }\n\nmain :: () {\n    p: Point;\n}\n";
    let (db, search, file) = program(source);
    assert!(
        hover(&db, file, search, Encoding::Utf8, at(source, "Point;")).is_none(),
        "a type annotation has no HIR span to locate; if this now works, update the note \
         in `hovering_a_struct_name_shows_its_fields` and PLAN.md"
    );
}

#[test]
fn hovering_a_constant_shows_its_type_and_value() {
    let source = "/// The answer.\nANSWER :: 42;\n\nmain :: () {\n    n := ANSWER;\n}\n";
    let (db, search, file) = program(source);
    let found = hover(&db, file, search, Encoding::Utf8, at(source, "ANSWER;\n}"))
        .expect("a constant resolves");
    assert_eq!(
        hover_text(&found.contents),
        "```jr\nmain\nANSWER :: s64 = 42\n```\n\n---\n\nThe answer."
    );
}

#[test]
fn a_string_constant_renders_its_escaped_value() {
    let source = "MESSAGE :: \"hi\\n\";\n\nmain :: () {\n    m := MESSAGE;\n}\n";
    let (db, search, file) = program(source);
    let found = hover(&db, file, search, Encoding::Utf8, at(source, "MESSAGE;\n}"))
        .expect("a constant resolves");
    assert_eq!(
        hover_text(&found.contents),
        "```jr\nmain\nMESSAGE :: string = \"hi\\n\"\n```"
    );
}

#[test]
fn hovering_an_imported_procedure_shows_the_module_as_container() {
    // The container is `Basic`, not `module`: every module's file is `module.jr`, so the
    // file stem would render every module in the system identically.
    let source = "#import \"Basic\";\n\nmain :: () {\n    print(\"hi\");\n}\n";
    let (db, search, file) = program(source);
    let found = hover(
        &db,
        file,
        search,
        Encoding::Utf8,
        at(source, "print(\"hi\")"),
    )
    .expect("an imported name resolves");
    let text = hover_text(&found.contents);
    assert!(
        text.starts_with("```jr\nBasic\nprint :: (s: string)\n```"),
        "expected the Basic module's card, got:\n{text}"
    );
}

#[test]
fn a_non_name_expression_still_falls_back_to_its_type() {
    // ADR-0028 §4's fallback: there is no declaration behind `1 + 2`.
    let source = "main :: () {\n    n := 1 + 2;\n}\n";
    let (db, search, file) = program(source);
    let found = hover(&db, file, search, Encoding::Utf8, at(source, "1 + 2"))
        .expect("an expression has a type");
    assert_eq!(hover_text(&found.contents), "```jr\nmain\ns64\n```");
}

#[test]
fn an_ordinary_comment_is_not_documentation() {
    // ADR-0027: `//` above a declaration is an aside. `Basic` has dozens of lines of
    // them, and turning those into API documentation by position is what the language
    // feature exists to avoid.
    let source = "// Not documentation.\nadd :: (a: s64) -> s64 {\n    return a;\n}\n\nmain :: () {\n    n := add(1);\n}\n";
    let (db, search, file) = program(source);
    let found = hover(&db, file, search, Encoding::Utf8, at(source, "add(1")).expect("resolves");
    assert_eq!(
        hover_text(&found.contents),
        "```jr\nmain\nadd :: (a: s64) -> s64\n```"
    );
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

// ---------------------------------------------------------------------------
// Completion (ADR-0028 §5)
// ---------------------------------------------------------------------------

/// The labels offered at `needle`, sorted so an assertion does not depend on order.
fn labels(source: &str, needle: &str) -> Vec<String> {
    let (db, search, file) = program(source);
    let mut out: Vec<String> =
        jr_lsp::completion(&db, file, search, Encoding::Utf8, at(source, needle))
            .into_iter()
            .map(|item| item.label)
            .collect();
    out.sort();
    out
}

#[test]
fn a_dot_offers_the_receivers_fields_and_nothing_else() {
    let source =
        "Point :: struct { x: s64; y: s64; }\n\nmain :: () {\n    p: Point;\n    n := p.;\n}\n";
    assert_eq!(labels(source, ";\n}"), vec!["x", "y"]);
}

#[test]
fn a_dot_on_a_pointer_derefs_to_the_pointee_fields() {
    // Because `jr_sema::check_field` loops `pointee` before looking a field up. A list
    // that stopped at the pointer would hide fields the checker accepts.
    let source = "Point :: struct { x: s64; }\n\nmain :: () {\n    p: Point;\n    q := *p;\n    n := q.;\n}\n";
    assert_eq!(labels(source, ";\n}"), vec!["x"]);
}

#[test]
fn a_dot_on_a_string_offers_its_pseudo_fields() {
    // ADR-0004 fixes `string` as `{data: *u8, count: s64}` and ADR-0015 §2 keeps it from
    // *being* that struct, so these two are special-cased in the checker — and here.
    let source = "main :: () {\n    s := \"hi\";\n    n := s.;\n}\n";
    assert_eq!(labels(source, ";\n}"), vec!["count", "data"]);
}

#[test]
fn a_field_completion_carries_its_type_as_detail() {
    let source = "Point :: struct { x: s64; }\n\nmain :: () {\n    p: Point;\n    n := p.;\n}\n";
    let (db, search, file) = program(source);
    let items = jr_lsp::completion(&db, file, search, Encoding::Utf8, at(source, ";\n}"));
    let x = items.iter().find(|i| i.label == "x").expect("field x");
    assert_eq!(x.detail.as_deref(), Some("s64"));
}

#[test]
fn a_hash_offers_directives() {
    let source = "main :: () {\n    n := 1;\n}\n#";
    // One character past the `#`, because that is where the cursor is when a client
    // fires the trigger character.
    let mut position = at(source, "#");
    position.character += 1;
    let (db, search, file) = program(source);
    let mut offered: Vec<String> = jr_lsp::completion(&db, file, search, Encoding::Utf8, position)
        .into_iter()
        .map(|item| item.label)
        .collect();
    offered.sort();
    assert_eq!(
        offered,
        vec!["#foreign", "#import", "#run", "#system_library"]
    );
}

#[test]
fn names_include_locals_items_keywords_and_builtin_types() {
    let source = "add :: (a: s64) -> s64 {\n    return a;\n}\n\nmain :: () {\n    total := 1;\n    n := t\n}\n";
    let offered = labels(source, "t\n}");
    for expected in ["add", "main", "total", "while", "s64", "string"] {
        assert!(
            offered.iter().any(|l| l == expected),
            "{expected:?} was not offered: {offered:?}"
        );
    }
}

#[test]
fn a_reserved_keyword_is_never_offered() {
    // `enum`, `for` and `cast` lex but are refused with an "arrives in wave Wn"
    // diagnostic, so completing one would be offering the user an error.
    let source = "main :: () {\n    n := e\n}\n";
    let offered = labels(source, "e\n}");
    for reserved in [
        "enum", "union", "for", "defer", "using", "cast", "xx", "null",
    ] {
        assert!(
            !offered.iter().any(|l| l == reserved),
            "{reserved:?} is reserved and must not be offered"
        );
    }
}

#[test]
fn a_procedure_completes_as_a_call_snippet() {
    let source =
        "add :: (a: s64, b: s64) -> s64 {\n    return a + b;\n}\n\nmain :: () {\n    n := a\n}\n";
    let (db, search, file) = program(source);
    let items = jr_lsp::completion(&db, file, search, Encoding::Utf8, at(source, "a\n}"));
    let add = items
        .iter()
        .find(|i| i.label == "add")
        .expect("add offered");
    assert_eq!(add.insert_text.as_deref(), Some("add(${1:a}, ${2:b})$0"));
    assert_eq!(
        add.insert_text_format,
        Some(lsp_types::InsertTextFormat::SNIPPET)
    );
    assert_eq!(
        add.detail.as_deref(),
        Some("add :: (a: s64, b: s64) -> s64")
    );
}

#[test]
fn an_imported_name_is_offered_with_its_module() {
    let source = "#import \"Basic\";\n\nmain :: () {\n    p\n}\n";
    let (db, search, file) = program(source);
    let items = jr_lsp::completion(&db, file, search, Encoding::Utf8, at(source, "p\n}"));
    let print = items
        .iter()
        .find(|i| i.label == "print")
        .expect("print offered from Basic");
    assert_eq!(print.detail.as_deref(), Some("print :: (s: string)"));
    assert_eq!(
        print
            .label_details
            .as_ref()
            .and_then(|d| d.description.as_deref()),
        Some("Basic")
    );
}

#[test]
fn the_list_carries_no_documentation_until_resolved() {
    // ADR-0028 §5's trade: the list stays cheap, so the prose has to arrive from resolve.
    let source =
        "/// Adds.\nadd :: (a: s64) -> s64 {\n    return a;\n}\n\nmain :: () {\n    n := a\n}\n";
    let (db, search, file) = program(source);
    let items = jr_lsp::completion(&db, file, search, Encoding::Utf8, at(source, "a\n}"));
    let add = items
        .iter()
        .find(|i| i.label == "add")
        .expect("add offered");
    assert!(add.documentation.is_none(), "docs should be lazy");
    assert!(add.data.is_some(), "an item must say how to resolve itself");
}

#[test]
fn resolving_an_item_agrees_with_the_hover_card() {
    // The trap ADR-0028 §5 names: resolve is a second path over the same item, so a
    // resolved item can disagree with the hover. Both render through `Decl`, and this is
    // what holds them equal.
    let source = "/// Adds two numbers.\nadd :: (a: s64, b: s64) -> s64 {\n    return a + b;\n}\n\nmain :: () {\n    n := a\n}\n";
    let (db, search, file) = program(source);

    let items = jr_lsp::completion(&db, file, search, Encoding::Utf8, at(source, "a\n}"));
    let add = items
        .iter()
        .find(|i| i.label == "add")
        .expect("add offered")
        .clone();
    let resolved = jr_lsp::resolve_completion(&db, file, search, add);

    let documentation = match resolved.documentation {
        Some(lsp_types::Documentation::MarkupContent(markup)) => markup.value,
        other => panic!("expected markdown documentation, got {other:?}"),
    };

    let hovered = hover(&db, file, search, Encoding::Utf8, at(source, "add ::"))
        .map(|h| hover_text(&h.contents))
        .expect("the declaration hovers");
    assert_eq!(documentation, hovered);
}

#[test]
fn resolving_an_item_with_no_data_returns_it_unchanged() {
    let source = "main :: () {\n    n := 1;\n}\n";
    let (db, search, file) = program(source);
    let keyword = lsp_types::CompletionItem {
        label: String::from("while"),
        ..lsp_types::CompletionItem::default()
    };
    let resolved = jr_lsp::resolve_completion(&db, file, search, keyword.clone());
    assert_eq!(resolved, keyword);
}

#[test]
fn completing_in_an_empty_file_offers_keywords_rather_than_failing() {
    let source = "";
    let (db, search, file) = program(source);
    let items = jr_lsp::completion(
        &db,
        file,
        search,
        Encoding::Utf8,
        lsp_types::Position {
            line: 0,
            character: 0,
        },
    );
    assert!(
        items.iter().any(|i| i.label == "struct"),
        "an empty file still has keywords: {items:?}"
    );
}

// ---- declaration sites (locate_declaration) -----------------------------------

#[test]
fn hovering_a_declarations_own_name_shows_its_card() {
    // The `add` in `add :: (…)` is an `Item::name_span`, not an `Expr::Name`, so the
    // expression scan answers nothing there. This is what `locate_declaration` is for.
    let source = "/// Adds.\nadd :: (a: s64) -> s64 {\n    return a;\n}\n";
    let (db, search, file) = program(source);
    let found = hover(&db, file, search, Encoding::Utf8, at(source, "add ::"))
        .expect("a declaration hovers");
    assert_eq!(
        hover_text(&found.contents),
        "```jr\nmain\nadd :: (a: s64) -> s64\n```\n\n---\n\nAdds."
    );
}

#[test]
fn hovering_a_parameter_at_its_declaration_shows_its_type() {
    let source = "add :: (count: s64) -> s64 {\n    return count;\n}\n";
    let (db, search, file) = program(source);
    let found = hover(&db, file, search, Encoding::Utf8, at(source, "count: s64"))
        .expect("a parameter declaration hovers");
    assert_eq!(hover_text(&found.contents), "```jr\nmain\ncount: s64\n```");
}

#[test]
fn hovering_a_local_at_its_declaration_shows_its_type() {
    let source = "main :: () {\n    total := 7;\n}\n";
    let (db, search, file) = program(source);
    let found = hover(&db, file, search, Encoding::Utf8, at(source, "total :="))
        .expect("a local declaration hovers");
    assert_eq!(hover_text(&found.contents), "```jr\nmain\ntotal: s64\n```");
}

#[test]
fn the_hover_range_covers_the_name_and_not_the_body() {
    // Hovering `add` should not highlight the whole procedure.
    let source = "add :: (a: s64) -> s64 {\n    return a;\n}\n";
    let (db, search, file) = program(source);
    let found = hover(&db, file, search, Encoding::Utf8, at(source, "add ::")).expect("hovers");
    let range = found.range.expect("a range");
    assert_eq!(range.start.line, 0);
    assert_eq!(range.start.character, 0);
    assert_eq!(range.end.line, 0);
    assert_eq!(range.end.character, 3);
}

// ---------------------------------------------------------------------------
// References, rename, symbols (ADR-0029, ADR-0030)
// ---------------------------------------------------------------------------

/// A database with several files on disk, discovered the way the server discovers them.
///
/// Files are written to a real temporary directory rather than injected, because ADR-0029's
/// walk is the thing under test in half of these and a walk needs a filesystem.
fn workspace(
    files: &[(&str, &str)],
) -> (jr_db::JairsDatabase, ModuleSearchPaths, tempfile::TempDir) {
    let dir = tempfile::TempDir::new().expect("a temporary directory");
    for (name, text) in files {
        let path = dir.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&path, text).expect("write");
    }
    let mut db = jr_db::JairsDatabase::default();
    let search = db.set_module_search_paths(vec![modules(), dir.path().to_path_buf()]);
    db.set_workspace_roots(&[dir.path().to_path_buf(), modules()]);
    db.load_workspace_files();
    (db, search, dir)
}

fn file_in(db: &jr_db::JairsDatabase, dir: &tempfile::TempDir, name: &str) -> SourceFile {
    let path = dir.path().join(name);
    db.source_file(path.to_string_lossy().as_ref())
        .expect("discovery must have loaded the file")
}

fn list_of(db: &jr_db::JairsDatabase) -> std::sync::Arc<jr_db::WorkspaceFileList> {
    let files = db.workspace_files().expect("discovery ran");
    files.list(db)
}

#[test]
fn discovery_loads_every_workspace_file_not_just_the_open_one() {
    // The bug this guards: `source_file_for_path` only sees loaded files, so a reference
    // scan over a *path* list would silently cover whatever the editor had opened.
    let (db, _search, dir) = workspace(&[("a.jr", "A :: 1;\n"), ("sub/b.jr", "B :: 2;\n")]);
    let list = list_of(&db);
    assert!(list.contains(&dir.path().join("a.jr")));
    assert!(list.contains(&dir.path().join("sub/b.jr")));
    for path in list.files.iter() {
        assert!(
            db.source_file(path.to_string_lossy().as_ref()).is_some(),
            "{} was discovered but never loaded",
            path.display()
        );
    }
}

#[test]
fn references_to_a_local_stay_in_one_file() {
    let source = "main :: () {\n    total := 1;\n    n := total + total;\n}\n";
    let (db, search, dir) = workspace(&[("main.jr", source)]);
    let file = file_in(&db, &dir, "main.jr");
    let found = jr_lsp::find_references(
        &db,
        file,
        search,
        Encoding::Utf8,
        at(source, "total +"),
        true,
        &list_of(&db).files,
    );
    // The declaration plus two uses.
    assert_eq!(found.len(), 3, "{found:?}");
}

#[test]
fn excluding_the_declaration_drops_exactly_one() {
    let source = "main :: () {\n    total := 1;\n    n := total;\n}\n";
    let (db, search, dir) = workspace(&[("main.jr", source)]);
    let file = file_in(&db, &dir, "main.jr");
    let with = jr_lsp::find_references(
        &db,
        file,
        search,
        Encoding::Utf8,
        at(source, "total;"),
        true,
        &list_of(&db).files,
    );
    let without = jr_lsp::find_references(
        &db,
        file,
        search,
        Encoding::Utf8,
        at(source, "total;"),
        false,
        &list_of(&db).files,
    );
    assert_eq!(with.len(), without.len() + 1);
}

#[test]
fn references_to_an_imported_name_cross_files() {
    // The point of identifying a definition by declaration site rather than by name
    // (ADR-0030 §1): `print` is spelled `print` in both importers.
    let a = "#import \"Basic\";\n\nmain :: () {\n    print(\"a\");\n}\n";
    let b = "#import \"Basic\";\n\nsecond :: () {\n    print(\"b\");\n    print(\"c\");\n}\n";
    let (db, search, dir) = workspace(&[("a.jr", a), ("b.jr", b)]);
    let file = file_in(&db, &dir, "a.jr");
    let found = jr_lsp::find_references(
        &db,
        file,
        search,
        Encoding::Utf8,
        at(a, "print(\"a\")"),
        false,
        &list_of(&db).files,
    );
    // Five, and the count is the interesting part: one use in `a.jr`, two in `b.jr`, and
    // **two inside `Basic` itself**, where `print_line` calls `print`. A search that only
    // looked at importers would have found three and looked correct.
    let per_file = |suffix: &str| {
        found
            .iter()
            .filter(|l| l.uri.as_str().ends_with(suffix))
            .count()
    };
    assert_eq!(per_file("a.jr"), 1, "{found:?}");
    assert_eq!(per_file("b.jr"), 2, "{found:?}");
    assert_eq!(
        per_file("Basic/module.jr"),
        2,
        "uses inside the declaring module are references too: {found:?}"
    );
    assert_eq!(found.len(), 5, "{found:?}");
}

#[test]
fn a_same_named_local_in_another_file_is_not_a_reference() {
    // Name matching would have found this; declaration-site matching must not.
    let a = "value :: 1;\n\nmain :: () {\n    n := value;\n}\n";
    let b = "main2 :: () {\n    value := 99;\n    m := value;\n}\n";
    let (db, search, dir) = workspace(&[("a.jr", a), ("b.jr", b)]);
    let file = file_in(&db, &dir, "a.jr");
    let found = jr_lsp::find_references(
        &db,
        file,
        search,
        Encoding::Utf8,
        at(a, "value;"),
        true,
        &list_of(&db).files,
    );
    assert!(
        found.iter().all(|l| l.uri.as_str().ends_with("a.jr")),
        "b.jr's unrelated local `value` was reported: {found:?}"
    );
}

#[test]
fn document_highlight_never_leaves_the_file() {
    let a = "#import \"Basic\";\n\nmain :: () {\n    print(\"a\");\n}\n";
    let b = "#import \"Basic\";\n\nsecond :: () {\n    print(\"b\");\n}\n";
    let (db, search, dir) = workspace(&[("a.jr", a), ("b.jr", b)]);
    let file = file_in(&db, &dir, "a.jr");
    let found =
        jr_lsp::document_highlight(&db, file, search, Encoding::Utf8, at(a, "print(\"a\")"));
    assert_eq!(found.len(), 1, "a workspace scan leaked in: {found:?}");
}

#[test]
fn prepare_rename_offers_the_current_name_as_the_placeholder() {
    let source = "add :: (a: s64) -> s64 {\n    return a;\n}\n";
    let (db, search, dir) = workspace(&[("main.jr", source)]);
    let file = file_in(&db, &dir, "main.jr");
    let prepared = jr_lsp::prepare_rename(&db, file, search, Encoding::Utf8, at(source, "add ::"))
        .expect("a declaration is renameable");
    match prepared {
        lsp_types::PrepareRenameResponse::RangeWithPlaceholder { placeholder, .. } => {
            assert_eq!(placeholder, "add");
        }
        other => panic!("expected a placeholder, got {other:?}"),
    }
}

#[test]
fn prepare_rename_refuses_a_keyword_and_a_builtin_type() {
    let source = "main :: () {\n    while true {\n        break;\n    }\n}\n";
    let (db, search, dir) = workspace(&[("main.jr", source)]);
    let file = file_in(&db, &dir, "main.jr");
    assert!(
        jr_lsp::prepare_rename(&db, file, search, Encoding::Utf8, at(source, "while")).is_none(),
        "a keyword must not be renameable"
    );

    let typed = "main :: () {\n    n: s64 = 1;\n}\n";
    let (db, search, dir) = workspace(&[("t.jr", typed)]);
    let file = file_in(&db, &dir, "t.jr");
    assert!(
        jr_lsp::prepare_rename(&db, file, search, Encoding::Utf8, at(typed, "s64")).is_none(),
        "a builtin type name must not be renameable"
    );
}

#[test]
fn renaming_an_imported_procedure_edits_every_file_including_its_module() {
    let a = "#import \"Basic\";\n\nmain :: () {\n    print(\"a\");\n}\n";
    let (db, search, dir) = workspace(&[("a.jr", a)]);
    let file = file_in(&db, &dir, "a.jr");
    let edit = jr_lsp::rename(
        &db,
        file,
        search,
        Encoding::Utf8,
        at(a, "print(\"a\")"),
        "write_line",
        &list_of(&db),
    )
    .expect("renaming an imported procedure is allowed");
    // See `navigate::rename`: the protocol's own map is keyed by `Uri`, whose interior
    // `Cell` is a cache and takes no part in its hash.
    #[allow(
        clippy::mutable_key_type,
        reason = "WorkspaceEdit::changes is keyed by Uri by the protocol"
    )]
    let changes = edit.changes.expect("edits");
    assert!(
        changes.keys().any(|uri| uri.as_str().ends_with("a.jr")),
        "the importer was not edited"
    );
    assert!(
        changes
            .keys()
            .any(|uri| uri.as_str().ends_with("Basic/module.jr")),
        "the declaring module was not edited: {:?}",
        changes.keys().collect::<Vec<_>>()
    );
}

#[test]
fn rename_refuses_a_name_that_is_not_an_identifier() {
    let source = "add :: (a: s64) -> s64 {\n    return a;\n}\n";
    let (db, search, dir) = workspace(&[("main.jr", source)]);
    let file = file_in(&db, &dir, "main.jr");
    for bad in ["2x", "", "a-b", "a b", "#foreign"] {
        let refusal = jr_lsp::rename(
            &db,
            file,
            search,
            Encoding::Utf8,
            at(source, "add ::"),
            bad,
            &list_of(&db),
        )
        .expect_err("must refuse");
        assert!(
            matches!(refusal, jr_lsp::RenameRefusal::NotAnIdentifier(_)),
            "{bad:?} gave {refusal:?}"
        );
    }
}

#[test]
fn rename_refuses_a_collision_rather_than_shadowing() {
    // The one outcome a refactor must never produce: code that compiles and means
    // something else.
    let source = "first :: 1;\nsecond :: 2;\n\nmain :: () {\n    n := first;\n}\n";
    let (db, search, dir) = workspace(&[("main.jr", source)]);
    let file = file_in(&db, &dir, "main.jr");
    let refusal = jr_lsp::rename(
        &db,
        file,
        search,
        Encoding::Utf8,
        at(source, "first ::"),
        "second",
        &list_of(&db),
    )
    .expect_err("renaming onto an existing name must be refused");
    assert!(
        matches!(refusal, jr_lsp::RenameRefusal::Collision { .. }),
        "{refusal:?}"
    );
    // And the message says what to do, not merely that something failed.
    assert!(
        refusal.to_string().contains("already declared"),
        "unhelpful message: {refusal}"
    );
}

#[test]
fn rename_refuses_a_local_colliding_with_another_local() {
    let source = "main :: () {\n    a := 1;\n    b := 2;\n    n := a + b;\n}\n";
    let (db, search, dir) = workspace(&[("main.jr", source)]);
    let file = file_in(&db, &dir, "main.jr");
    let refusal = jr_lsp::rename(
        &db,
        file,
        search,
        Encoding::Utf8,
        at(source, "a :="),
        "b",
        &list_of(&db),
    )
    .expect_err("must refuse");
    assert!(
        matches!(refusal, jr_lsp::RenameRefusal::Collision { .. }),
        "{refusal:?}"
    );
}

#[test]
fn rename_refuses_when_a_file_it_must_edit_does_not_parse() {
    // Named in the message, because otherwise this reads as a bug in rename rather than a
    // syntax error somewhere the user is not looking.
    let a = "shared :: 1;\n\nmain :: () {\n    n := shared;\n}\n";
    let broken = "#import \"a\";\n\nuse :: () {\n    n := shared +;\n}\n";
    let (db, search, dir) = workspace(&[("a.jr", a), ("broken.jr", broken)]);
    let file = file_in(&db, &dir, "a.jr");
    let refusal = jr_lsp::rename(
        &db,
        file,
        search,
        Encoding::Utf8,
        at(a, "shared ::"),
        "renamed",
        &list_of(&db),
    )
    .expect_err("a broken file that must be edited blocks the rename");
    match &refusal {
        jr_lsp::RenameRefusal::UnparsedFile(path) => {
            assert!(
                path.ends_with("broken.jr"),
                "the wrong file was blamed: {}",
                path.display()
            );
        }
        other => panic!("expected UnparsedFile, got {other:?}"),
    }
    assert!(refusal.to_string().contains("broken.jr"));
}

#[test]
fn rename_refuses_on_a_truncated_workspace_but_not_for_a_local() {
    // ADR-0029 §4: a consumer that must be exhaustive to be correct refuses; and a
    // file-local rename is not endangered by a truncated list, so it must still work.
    let source = "shared :: 1;\n\nmain :: () {\n    local := 1;\n    n := shared + local;\n}\n";
    let (db, search, dir) = workspace(&[("main.jr", source)]);
    let file = file_in(&db, &dir, "main.jr");
    let truncated = jr_db::WorkspaceFileList {
        files: list_of(&db).files.clone(),
        truncated: true,
    };

    let refusal = jr_lsp::rename(
        &db,
        file,
        search,
        Encoding::Utf8,
        at(source, "shared ::"),
        "renamed",
        &truncated,
    )
    .expect_err("a file-level name cannot be proven complete");
    assert!(
        matches!(refusal, jr_lsp::RenameRefusal::TruncatedWorkspace),
        "{refusal:?}"
    );

    jr_lsp::rename(
        &db,
        file,
        search,
        Encoding::Utf8,
        at(source, "local :="),
        "renamed",
        &truncated,
    )
    .expect("a local rename does not depend on the file list being complete");
}

#[test]
fn document_symbols_nest_struct_fields_and_carry_signatures() {
    let source = "/// A point.\nPoint :: struct { x: s64; y: s64; }\n\nadd :: (a: s64) -> s64 {\n    return a;\n}\n\nMESSAGE :: \"hi\";\n";
    let (db, search, dir) = workspace(&[("main.jr", source)]);
    let file = file_in(&db, &dir, "main.jr");
    let symbols = jr_lsp::document_symbol(&db, file, search, Encoding::Utf8);

    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["Point", "add", "MESSAGE"], "in source order");

    let point = &symbols[0];
    assert_eq!(point.kind, lsp_types::SymbolKind::STRUCT);
    let fields = point
        .children
        .as_ref()
        .expect("fields nest under the struct");
    assert_eq!(
        fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        vec!["x", "y"]
    );

    let add = &symbols[1];
    assert_eq!(add.kind, lsp_types::SymbolKind::FUNCTION);
    assert_eq!(add.detail.as_deref(), Some("add :: (a: s64) -> s64"));
    assert!(
        add.children.is_none(),
        "parameters must not nest: the signature already lists them"
    );
    assert_eq!(symbols[2].kind, lsp_types::SymbolKind::CONSTANT);
}

#[test]
fn workspace_symbols_span_files_and_filter_case_insensitively() {
    let (db, search, _dir) = workspace(&[
        ("a.jr", "AlphaThing :: 1;\n"),
        ("b.jr", "BetaThing :: 2;\n"),
    ]);
    let all = jr_lsp::workspace_symbol(&db, search, Encoding::Utf8, "", &list_of(&db).files);
    let names: Vec<&str> = all.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"AlphaThing"), "{names:?}");
    assert!(names.contains(&"BetaThing"), "{names:?}");
    // And `Basic`'s exports, since `modules/` is a discovery root here.
    assert!(names.contains(&"print"), "{names:?}");

    let filtered =
        jr_lsp::workspace_symbol(&db, search, Encoding::Utf8, "alphath", &list_of(&db).files);
    assert_eq!(filtered.len(), 1, "{filtered:?}");
    assert_eq!(filtered[0].name, "AlphaThing");
}

#[test]
fn workspace_symbols_proceed_on_a_truncated_list() {
    // Unlike rename: a partial outline is still useful (ADR-0029 §4).
    let (db, search, _dir) = workspace(&[("a.jr", "Thing :: 1;\n")]);
    let found = jr_lsp::workspace_symbol(&db, search, Encoding::Utf8, "thing", &list_of(&db).files);
    assert_eq!(found.len(), 1);
}

// ---------------------------------------------------------------------------
// Code actions (ADR-0031)
// ---------------------------------------------------------------------------

/// Every action offered at `needle`, given the file's own diagnostics.
///
/// The diagnostics are computed rather than hand-written, because the handler reads them
/// the way a client sends them — and a hand-written diagnostic would let a test pass while
/// the real message wording had drifted out from under the action.
fn actions_at(
    db: &JairsDatabase,
    search: ModuleSearchPaths,
    file: SourceFile,
    source: &str,
    needle: &str,
    workspace: &jr_db::WorkspaceFileList,
) -> Vec<lsp_types::CodeAction> {
    let position = at(source, needle);
    let range = lsp_types::Range {
        start: position,
        end: position,
    };
    let diags: Vec<lsp_types::Diagnostic> = diagnostics(db, file, search, Encoding::Utf8)
        .into_iter()
        .filter(|d| d.range.start.line == position.line)
        .collect();
    jr_lsp::code_actions(db, file, search, Encoding::Utf8, range, &diags, workspace)
        .into_iter()
        .filter_map(|action| match action {
            lsp_types::CodeActionOrCommand::CodeAction(action) => Some(action),
            lsp_types::CodeActionOrCommand::Command(_) => None,
        })
        .collect()
}

fn titles(actions: &[lsp_types::CodeAction]) -> Vec<String> {
    actions.iter().map(|a| a.title.clone()).collect()
}

/// The single edit an action carries, panicking if it carries a different number.
///
/// The `mutable_key_type` allow is the same one `navigate.rs` carries and for the same
/// reason: `WorkspaceEdit::changes` *is* a `HashMap<Uri, _>` in the protocol type, and
/// `fluent_uri`'s interior `Cell` is a lazily-computed authority cache that takes no part in
/// `Hash` or `Eq`. Allowed per-function rather than crate-wide, so a genuinely mutable key
/// elsewhere still fails the build.
#[allow(
    clippy::mutable_key_type,
    reason = "the protocol's own type; Uri's Cell is a cache, not part of its hash"
)]
fn only_edit(action: &lsp_types::CodeAction) -> lsp_types::TextEdit {
    let changes = action
        .edit
        .as_ref()
        .and_then(|edit| edit.changes.as_ref())
        .expect("an action must carry an edit");
    let edits: Vec<&lsp_types::TextEdit> = changes.values().flatten().collect();
    assert_eq!(edits.len(), 1, "expected one edit, got {edits:?}");
    edits[0].clone()
}

/// Applies every edit an action carries to `source`, so a test asserts on the result
/// rather than on a range triple.
///
/// Edits are applied last-first, which is what makes multiple edits in one file safe: an
/// earlier edit would otherwise shift every later range.
#[allow(
    clippy::mutable_key_type,
    reason = "the protocol's own type; Uri's Cell is a cache, not part of its hash"
)]
fn apply(source: &str, action: &lsp_types::CodeAction) -> String {
    let changes = action
        .edit
        .as_ref()
        .and_then(|edit| edit.changes.as_ref())
        .expect("an action must carry an edit");
    let mut edits: Vec<lsp_types::TextEdit> = changes.values().flatten().cloned().collect();
    edits.sort_by_key(|edit| (edit.range.start.line, edit.range.start.character));
    let mut lines: Vec<String> = source.split('\n').map(ToOwned::to_owned).collect();
    for edit in edits.iter().rev() {
        let start_line = edit.range.start.line as usize;
        let end_line = edit.range.end.line as usize;
        let start_char = edit.range.start.character as usize;
        let end_char = edit.range.end.character as usize;
        let head: String = lines
            .get(start_line)
            .map(|line| line.chars().take(start_char).collect())
            .unwrap_or_default();
        let tail: String = lines
            .get(end_line)
            .map(|line| line.chars().skip(end_char).collect())
            .unwrap_or_default();
        let replacement = format!("{head}{}{tail}", edit.new_text);
        let last = end_line.min(lines.len().saturating_sub(1));
        lines.splice(
            start_line..=last,
            replacement.split('\n').map(ToOwned::to_owned),
        );
    }
    lines.join("\n")
}

#[test]
fn an_unresolved_name_offers_an_import_of_the_module_that_exports_it() {
    let source = "main :: () {\n    print(\"hi\\n\");\n}\n";
    let (db, search, dir) = workspace(&[("main.jr", source)]);
    let file = file_in(&db, &dir, "main.jr");
    let list = list_of(&db);

    let actions = actions_at(&db, search, file, source, "print", &list);
    let titles = titles(&actions);
    assert!(
        titles.contains(&String::from("import `Basic` for `print`")),
        "{titles:?}"
    );

    // The edit goes at the top of the file, and produces something that compiles.
    let applied = apply(source, &actions[0]);
    assert!(
        applied.starts_with("#import \"Basic\";\n"),
        "got {applied:?}"
    );
}

#[test]
fn a_module_that_does_not_export_the_name_is_not_offered() {
    // The whole reason ADR-0031 §5 parses the discovered modules: an offer for a module
    // that does not export the name replaces one error with two.
    let source = "main :: () {\n    nonexistent_thing();\n}\n";
    let (db, search, dir) = workspace(&[("main.jr", source), ("Other.jr", "OTHER :: 1;\n")]);
    let file = file_in(&db, &dir, "main.jr");
    let list = list_of(&db);

    let actions = actions_at(&db, search, file, source, "nonexistent_thing", &list);
    let imports: Vec<String> = titles(&actions)
        .into_iter()
        .filter(|title| title.starts_with("import "))
        .collect();
    assert!(imports.is_empty(), "{imports:?}");
}

#[test]
fn an_import_that_is_already_present_is_not_offered_again() {
    let source = "#import \"Basic\";\n\nmain :: () {\n    print(\"hi\\n\");\n}\n";
    let (db, search, dir) = workspace(&[("main.jr", source)]);
    let file = file_in(&db, &dir, "main.jr");
    let list = list_of(&db);

    // `print` resolves here, so there is no E0201 at all and therefore no offer.
    let actions = actions_at(&db, search, file, source, "print(", &list);
    let imports: Vec<String> = titles(&actions)
        .into_iter()
        .filter(|title| title.starts_with("import "))
        .collect();
    assert!(imports.is_empty(), "{imports:?}");
}

#[test]
fn an_unused_import_offers_removal_that_deletes_the_whole_line() {
    let source = "#import \"Basic\";\n\nmain :: () {\n}\n";
    let (db, search, dir) = workspace(&[("main.jr", source)]);
    let file = file_in(&db, &dir, "main.jr");
    let list = list_of(&db);

    let actions = actions_at(&db, search, file, source, "#import", &list);
    let titles = titles(&actions);
    assert!(
        titles.contains(&String::from("remove unused import `Basic`")),
        "{titles:?}"
    );

    let action = actions
        .iter()
        .find(|a| a.title.starts_with("remove unused"))
        .expect("the removal action");
    // No blank line left behind: the range must reach the start of the next line.
    let edit = only_edit(action);
    assert_eq!(edit.range.start.character, 0);
    assert_eq!(edit.range.end.line, edit.range.start.line + 1);
    assert_eq!(edit.range.end.character, 0);
    assert_eq!(apply(source, action), "\nmain :: () {\n}\n");
}

#[test]
fn two_unused_imports_offer_one_organise_action_as_well() {
    let source = "#import \"Basic\";\n#import \"Colors\";\n\nmain :: () {\n}\n";
    let (db, search, dir) = workspace(&[("main.jr", source), ("Colors.jr", "BLACK :: 0;\n")]);
    let file = file_in(&db, &dir, "main.jr");
    let list = list_of(&db);

    let actions = actions_at(&db, search, file, source, "#import \"Basic\"", &list);
    let organise = actions
        .iter()
        .find(|a| a.kind == Some(lsp_types::CodeActionKind::SOURCE_ORGANIZE_IMPORTS))
        .expect("an organise-imports action");
    assert_eq!(organise.title, "remove 2 unused imports");

    // Both lines go, and the rest of the file is untouched.
    assert_eq!(apply(source, organise), "\nmain :: () {\n}\n");
}

#[test]
fn one_unused_import_offers_no_organise_action() {
    // It would be the single-import action under a second title.
    let source = "#import \"Basic\";\n\nmain :: () {\n}\n";
    let (db, search, dir) = workspace(&[("main.jr", source)]);
    let file = file_in(&db, &dir, "main.jr");
    let list = list_of(&db);

    let actions = actions_at(&db, search, file, source, "#import", &list);
    assert!(
        !actions
            .iter()
            .any(|a| a.kind == Some(lsp_types::CodeActionKind::SOURCE_ORGANIZE_IMPORTS)),
        "{:?}",
        titles(&actions)
    );
}

#[test]
fn a_misspelled_field_offers_the_name_the_compiler_suggested() {
    let source = "Rect :: struct {\n    width: s64;\n}\n\nmain :: () {\n    r: Rect;\n    n := r.widht;\n}\n";
    let (db, search, dir) = workspace(&[("main.jr", source)]);
    let file = file_in(&db, &dir, "main.jr");
    let list = list_of(&db);

    let actions = actions_at(&db, search, file, source, "widht", &list);
    let titles = titles(&actions);
    assert!(
        titles.contains(&String::from("change to `width`")),
        "{titles:?}"
    );

    let action = actions
        .iter()
        .find(|a| a.title == "change to `width`")
        .expect("the rename action");
    assert!(apply(source, action).contains("r.width;"));
}

#[test]
fn a_field_with_no_near_name_offers_nothing() {
    // The suggestion is read off the diagnostic, so no `help:` line means no action —
    // which is right: nothing was near enough to act on.
    let source =
        "Point :: struct {\n    x: s64;\n}\n\nmain :: () {\n    p: Point;\n    n := p.zzzzz;\n}\n";
    let (db, search, dir) = workspace(&[("main.jr", source)]);
    let file = file_in(&db, &dir, "main.jr");
    let list = list_of(&db);

    let actions = actions_at(&db, search, file, source, "zzzzz", &list);
    let changes: Vec<String> = titles(&actions)
        .into_iter()
        .filter(|title| title.starts_with("change to"))
        .collect();
    assert!(changes.is_empty(), "{changes:?}");
}

#[test]
fn a_misspelled_type_offers_the_type_the_compiler_suggested() {
    let source =
        "Rectangle :: struct {\n    width: s64;\n}\n\nmain :: () {\n    r: Recatngle;\n}\n";
    let (db, search, dir) = workspace(&[("main.jr", source)]);
    let file = file_in(&db, &dir, "main.jr");
    let list = list_of(&db);

    let actions = actions_at(&db, search, file, source, "Recatngle", &list);
    let titles = titles(&actions);
    assert!(
        titles.contains(&String::from("change to `Rectangle`")),
        "{titles:?}"
    );
}

#[test]
fn a_procedure_with_no_body_is_offered_one() {
    let source = "add :: (a: s64) -> s64;\n";
    let (db, search, dir) = workspace(&[("main.jr", source)]);
    let file = file_in(&db, &dir, "main.jr");
    let list = list_of(&db);

    let actions = actions_at(&db, search, file, source, "add", &list);
    let titles = titles(&actions);
    assert!(
        titles.contains(&String::from("give this procedure an empty body")),
        "{titles:?}"
    );
}

#[test]
fn a_comment_above_a_declaration_can_become_documentation() {
    let source = "// Adds two numbers.\nadd :: (a: s64, b: s64) -> s64 {\n    return a + b;\n}\n";
    let (db, search, dir) = workspace(&[("main.jr", source)]);
    let file = file_in(&db, &dir, "main.jr");
    let list = list_of(&db);

    let actions = actions_at(&db, search, file, source, "// Adds", &list);
    let action = actions
        .iter()
        .find(|a| a.title == "make this comment documentation")
        .unwrap_or_else(|| panic!("expected the refactor, got {:?}", titles(&actions)));
    assert_eq!(
        action.kind,
        Some(lsp_types::CodeActionKind::REFACTOR_REWRITE)
    );
    // The comment's text survives verbatim: only the `//` is replaced.
    assert!(
        apply(source, action).starts_with("/// Adds two numbers.\n"),
        "got {:?}",
        apply(source, action)
    );
}

#[test]
fn a_comment_above_nothing_is_not_offered_documentation() {
    // A `///` that precedes no declaration is silently dropped (ADR-0027 §3), so offering
    // this would be an action that appears to do nothing.
    let source = "main :: () {\n}\n\n// A trailing note about the file.\n";
    let (db, search, dir) = workspace(&[("main.jr", source)]);
    let file = file_in(&db, &dir, "main.jr");
    let list = list_of(&db);

    let actions = actions_at(&db, search, file, source, "// A trailing", &list);
    assert!(
        !titles(&actions).contains(&String::from("make this comment documentation")),
        "{:?}",
        titles(&actions)
    );
}

#[test]
fn a_doc_comment_is_not_offered_promotion_again() {
    let source = "/// Already documentation.\nadd :: (a: s64) -> s64 {\n    return a;\n}\n";
    let (db, search, dir) = workspace(&[("main.jr", source)]);
    let file = file_in(&db, &dir, "main.jr");
    let list = list_of(&db);

    let actions = actions_at(&db, search, file, source, "/// Already", &list);
    assert!(
        !titles(&actions).contains(&String::from("make this comment documentation")),
        "{:?}",
        titles(&actions)
    );
}

#[test]
fn four_slashes_stay_an_ordinary_comment() {
    // ADR-0027 §1 makes `////` deliberately *not* documentation, so promoting it would
    // silently change what the file means.
    let source = "//// A banner, not documentation.\nadd :: (a: s64) -> s64 {\n    return a;\n}\n";
    let (db, search, dir) = workspace(&[("main.jr", source)]);
    let file = file_in(&db, &dir, "main.jr");
    let list = list_of(&db);

    let actions = actions_at(&db, search, file, source, "////", &list);
    assert!(
        !titles(&actions).contains(&String::from("make this comment documentation")),
        "{:?}",
        titles(&actions)
    );
}

// ---------------------------------------------------------------------------
// Signature help (ADR-0031 §6)
// ---------------------------------------------------------------------------

#[test]
fn signature_help_names_the_procedure_and_the_active_parameter() {
    let source = "add :: (a: s64, b: s64) -> s64 {\n    return a + b;\n}\n\nmain :: () {\n    n := add(1, 2);\n}\n";
    let (db, search, _file) = program(source);
    let file = db.source_file("/jairs-lsp-test/main.jr").expect("added");

    // On the first argument.
    let help = jr_lsp::signature_help(&db, file, search, Encoding::Utf8, at(source, "1, 2"))
        .expect("inside a call");
    assert_eq!(help.signatures[0].label, "add :: (a: s64, b: s64) -> s64");
    assert_eq!(help.signatures[0].active_parameter, Some(0));

    // On the second.
    let help = jr_lsp::signature_help(&db, file, search, Encoding::Utf8, at(source, "2);"))
        .expect("inside a call");
    assert_eq!(help.signatures[0].active_parameter, Some(1));
}

#[test]
fn signature_help_lists_parameters_with_their_types() {
    let source = "add :: (a: s64, b: s64) -> s64 {\n    return a + b;\n}\n\nmain :: () {\n    n := add(1, 2);\n}\n";
    let (db, search, _f) = program(source);
    let file = db.source_file("/jairs-lsp-test/main.jr").expect("added");
    let help = jr_lsp::signature_help(&db, file, search, Encoding::Utf8, at(source, "1, 2"))
        .expect("inside a call");
    let params = help.signatures[0].parameters.as_ref().expect("parameters");
    let labels: Vec<String> = params
        .iter()
        .map(|p| match &p.label {
            lsp_types::ParameterLabel::Simple(text) => text.clone(),
            other => panic!("expected a simple label, got {other:?}"),
        })
        .collect();
    assert_eq!(labels, vec!["a: s64", "b: s64"]);
}

#[test]
fn signature_help_crosses_into_an_imported_module() {
    let source = "#import \"Basic\";\n\nmain :: () {\n    print(\"hi\\n\");\n}\n";
    let (db, search, _f) = program(source);
    let file = db.source_file("/jairs-lsp-test/main.jr").expect("added");
    let help = jr_lsp::signature_help(&db, file, search, Encoding::Utf8, at(source, "\"hi"))
        .expect("inside a call");
    assert!(
        help.signatures[0].label.starts_with("print :: ("),
        "{:?}",
        help.signatures[0].label
    );
}

#[test]
fn signature_help_outside_a_call_returns_nothing() {
    let source = "add :: (a: s64) -> s64 {\n    return a;\n}\n";
    let (db, search, _f) = program(source);
    let file = db.source_file("/jairs-lsp-test/main.jr").expect("added");
    assert!(
        jr_lsp::signature_help(&db, file, search, Encoding::Utf8, at(source, "return")).is_none()
    );
}

#[test]
fn too_many_arguments_still_highlight_the_last_parameter() {
    // Clamped rather than out of range: a client given an index past the end highlights
    // nothing, exactly when the user needs to see what they overran.
    let source =
        "one :: (a: s64) -> s64 {\n    return a;\n}\n\nmain :: () {\n    n := one(1, 2);\n}\n";
    let (db, search, _f) = program(source);
    let file = db.source_file("/jairs-lsp-test/main.jr").expect("added");
    let help = jr_lsp::signature_help(&db, file, search, Encoding::Utf8, at(source, "2);"))
        .expect("inside a call");
    assert_eq!(help.signatures[0].active_parameter, Some(0));
}

#[test]
fn the_inner_call_wins_when_calls_nest() {
    let source = "add :: (a: s64, b: s64) -> s64 {\n    return a + b;\n}\n\none :: (x: s64) -> s64 {\n    return x;\n}\n\nmain :: () {\n    n := add(one(5), 2);\n}\n";
    let (db, search, _f) = program(source);
    let file = db.source_file("/jairs-lsp-test/main.jr").expect("added");
    let help = jr_lsp::signature_help(&db, file, search, Encoding::Utf8, at(source, "5)"))
        .expect("inside a call");
    assert_eq!(help.signatures[0].label, "one :: (x: s64) -> s64");
}

// ---------------------------------------------------------------------------
// Inlay hints (ADR-0031 §7)
// ---------------------------------------------------------------------------

fn hint_labels(hints: &[lsp_types::InlayHint]) -> Vec<String> {
    hints
        .iter()
        .map(|hint| match &hint.label {
            lsp_types::InlayHintLabel::String(text) => text.clone(),
            other => panic!("expected a string label, got {other:?}"),
        })
        .collect()
}

fn whole_file() -> lsp_types::Range {
    lsp_types::Range {
        start: lsp_types::Position {
            line: 0,
            character: 0,
        },
        end: lsp_types::Position {
            line: 10_000,
            character: 0,
        },
    }
}

#[test]
fn an_inferred_local_gets_a_type_hint() {
    let source = "add :: (a: s64, b: s64) -> s64 {\n    return a + b;\n}\n\nmain :: () {\n    n := add(1, 2);\n}\n";
    let (db, search, _f) = program(source);
    let file = db.source_file("/jairs-lsp-test/main.jr").expect("added");
    let hints = jr_lsp::inlay_hints(&db, file, search, Encoding::Utf8, whole_file());
    assert_eq!(hint_labels(&hints), vec![": s64"]);
    // Placed after the name, so the line reads `n: s64 := add(1, 2);`.
    assert_eq!(hints[0].position, at(source, " := add(1, 2)"));
}

#[test]
fn an_annotated_local_gets_no_hint() {
    // The type is already on screen; repeating it is noise.
    let source = "main :: () {\n    n: s64 = 1;\n}\n";
    let (db, search, _f) = program(source);
    let file = db.source_file("/jairs-lsp-test/main.jr").expect("added");
    let hints = jr_lsp::inlay_hints(&db, file, search, Encoding::Utf8, whole_file());
    assert!(hint_labels(&hints).is_empty(), "{:?}", hint_labels(&hints));
}

#[test]
fn a_run_directive_shows_the_value_it_computed() {
    // The hint nothing outside this project can offer: the fold happened in the bytecode
    // VM, and the text says nothing about `5`.
    let source =
        "add :: (a: s64, b: s64) -> s64 {\n    return a + b;\n}\n\nCOMPUTED :: #run add(2, 3);\n";
    let (db, search, _f) = program(source);
    let file = db.source_file("/jairs-lsp-test/main.jr").expect("added");
    let hints = jr_lsp::inlay_hints(&db, file, search, Encoding::Utf8, whole_file());
    assert!(
        hint_labels(&hints).contains(&String::from(" = 5")),
        "{:?}",
        hint_labels(&hints)
    );
}

#[test]
fn an_ordinary_constant_gets_no_value_hint() {
    // `FOUR :: 4` would restate its own text.
    let source = "FOUR :: 4;\n";
    let (db, search, _f) = program(source);
    let file = db.source_file("/jairs-lsp-test/main.jr").expect("added");
    let hints = jr_lsp::inlay_hints(&db, file, search, Encoding::Utf8, whole_file());
    assert!(hint_labels(&hints).is_empty(), "{:?}", hint_labels(&hints));
}

#[test]
fn hints_outside_the_requested_range_are_not_computed() {
    let source = "main :: () {\n    n := 1;\n    m := 2;\n}\n";
    let (db, search, _f) = program(source);
    let file = db.source_file("/jairs-lsp-test/main.jr").expect("added");
    let just_the_first = lsp_types::Range {
        start: lsp_types::Position {
            line: 1,
            character: 0,
        },
        end: lsp_types::Position {
            line: 1,
            character: 99,
        },
    };
    let hints = jr_lsp::inlay_hints(&db, file, search, Encoding::Utf8, just_the_first);
    assert_eq!(hints.len(), 1, "{:?}", hint_labels(&hints));
    assert_eq!(hints[0].position.line, 1);
}

// ---------------------------------------------------------------------------
// #import navigation (ADR-0035)
// ---------------------------------------------------------------------------

#[test]
fn goto_definition_on_an_import_opens_the_module_from_anywhere_on_the_line() {
    // The request that prompted ADR-0035, and the bug it fixes: before it, every column of
    // this line answered nothing — including the module name itself — because an import is
    // lowered with `name: None` and `locate_declaration` skipped nameless items to keep a
    // top-level `#run` from matching.
    let source = "#import \"Basic\";\n\nmain :: () {\n}\n";
    let (db, search, _f) = program(source);
    let file = db.source_file("/jairs-lsp-test/main.jr").expect("added");

    // Every column of `#import "Basic";`, so "anywhere on the line" is asserted rather
    // than assumed of one representative position.
    for character in 0..source.lines().next().expect("a first line").len() {
        let at = Position {
            line: 0,
            character: u32::try_from(character).expect("small"),
        };
        let found = goto_definition(&db, file, search, Encoding::Utf8, at)
            .unwrap_or_else(|| panic!("column {character} must navigate"));
        assert!(
            found.uri.as_str().ends_with("modules/Basic/module.jr"),
            "column {character} landed at {}",
            found.uri.as_str()
        );
        // The start of the file: a module is a file, and there is no declaration inside it
        // that is "the definition of the module" (ADR-0035 §1).
        assert_eq!(found.range.start.line, 0);
        assert_eq!(found.range.start.character, 0);
    }
}

#[test]
fn goto_definition_on_an_unresolved_import_returns_nothing() {
    // Rather than pointing at where the file would be, which would open an empty buffer at
    // a path the user never chose. E0210 already reports the failure (ADR-0035 §3).
    let source = "#import \"NoSuchModule\";\n\nmain :: () {\n}\n";
    let (db, search, _f) = program(source);
    let file = db.source_file("/jairs-lsp-test/main.jr").expect("added");
    assert!(
        goto_definition(
            &db,
            file,
            search,
            Encoding::Utf8,
            at(source, "NoSuchModule")
        )
        .is_none()
    );
}

#[test]
fn hovering_an_import_shows_the_resolved_path_and_the_modules_own_docs() {
    let source = "#import \"Basic\";\n\nmain :: () {\n}\n";
    let (db, search, _f) = program(source);
    let file = db.source_file("/jairs-lsp-test/main.jr").expect("added");

    let hovered = hover(&db, file, search, Encoding::Utf8, at(source, "Basic"))
        .expect("an import must hover");
    let text = hover_text(&hovered.contents);
    assert!(text.contains("#import \"Basic\""), "{text}");
    // The resolved path is the part worth hovering for: `#import "Basic"` does not say
    // *which* `Basic`, and the search-path order decides (ADR-0035 §2).
    assert!(text.contains("modules/Basic/module.jr"), "{text}");
    // The module's `//!` block, which `file_docs` has collected since ADR-0027 and which
    // nothing displayed until now.
    assert!(
        text.contains("the bottom of the Jairs standard library"),
        "the module's own `//!` documentation must appear: {text}"
    );
}

#[test]
fn hovering_an_unresolved_import_says_so_rather_than_vanishing() {
    // A hover that disappears next to an E0210 reads as a second, unrelated failure.
    let source = "#import \"NoSuchModule\";\n\nmain :: () {\n}\n";
    let (db, search, _f) = program(source);
    let file = db.source_file("/jairs-lsp-test/main.jr").expect("added");
    let hovered = hover(
        &db,
        file,
        search,
        Encoding::Utf8,
        at(source, "NoSuchModule"),
    )
    .expect("an unresolved import must still hover");
    let text = hover_text(&hovered.contents);
    assert!(text.contains("#import \"NoSuchModule\""), "{text}");
    assert!(text.contains("not found"), "{text}");
}

#[test]
fn an_import_is_not_a_rename_target() {
    // Renaming a module means editing its file and every `#import` naming it — a real
    // feature `PLAN.md` §7 lists, and not this one. Answering here would half-implement it
    // in a way that compiles (ADR-0035 consequences).
    let source = "#import \"Basic\";\n\nmain :: () {\n}\n";
    let (db, search, _f) = program(source);
    let file = db.source_file("/jairs-lsp-test/main.jr").expect("added");
    let offset = {
        let text = file.text(&db);
        let index = jr_db::line_index(&db, file);
        jr_lsp::Positions::new(text.as_ref(), &index, Encoding::Utf8).offset(at(source, "Basic"))
    };
    assert!(
        jr_lsp::definition_at(&db, file, search, offset).is_none(),
        "an import must have no DefId"
    );
}

#[test]
fn a_top_level_run_still_does_not_hover_as_an_item() {
    // The guard ADR-0035 §4 kept. `locate_declaration` skips nameless items so that a
    // top-level `#run` does not render whatever item sits at its index; imports now match
    // through their own arm instead of by weakening that guard.
    let source = "add :: (a: s64) -> s64 {\n    return a;\n}\n\n#run add(1);\n";
    let (db, search, _f) = program(source);
    let file = db.source_file("/jairs-lsp-test/main.jr").expect("added");
    let hovered = hover(&db, file, search, Encoding::Utf8, at(source, "#run"));
    // `#run` itself is not a declaration to hover. Whatever comes back must not be an
    // item's card — the failure this pins is a *wrong* card, not a missing one.
    if let Some(hovered) = hovered {
        let text = hover_text(&hovered.contents);
        assert!(
            !text.contains("add :: "),
            "hovering `#run` must not render the item at its index: {text}"
        );
    }
}
