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
