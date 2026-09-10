//! Cross-file typing: ADR-0016 §5, and the module cycle it exists to keep working.

mod harness;

use harness::Program;
use jr_base::FileId;
use jr_pool::PoolId;
use jr_sema::ImportedTemplateContext;

const SHAPES: &str = "\
Rect :: struct {
    w: s64;
    h: s64;
}

area :: (r: Rect) -> s64 {
    return r.w * r.h;
}

SHAPES_VERSION :: 1;
";

#[test]
fn an_imported_struct_type_is_the_same_type_as_the_exporter_declared() {
    // The point of interning: `Rect` in the importer and `Rect` in `Shapes` must
    // be one `PoolId`, or the call below is a type error. Both come out of
    // `StructType { decl }` keyed on the *exporting* file, which is why
    // `ImportedFile` carries the module's `FileId`.
    let mut program = Program::new();
    let shapes = FileId::from_usize(1);
    let (shapes_hir, shapes_resolve, shapes_sigs) = program.analyse_module(SHAPES, shapes);

    let importer = "\
#import \"Shapes\";

main :: () {
    r: Rect;
    r.w = 3;
    r.h = 4;
    a := area(r);
    v := SHAPES_VERSION;
}
";
    let analysis = program.analyse_with_imports(
        importer,
        FileId::from_usize(0),
        &[("Shapes", shapes, &shapes_hir, &shapes_resolve)],
        &[("Shapes", &shapes_sigs)],
    );
    analysis.assert_silent();
}

#[test]
fn passing_the_wrong_type_to_an_imported_procedure_is_still_caught() {
    // Otherwise the previous test would pass just as well with everything
    // silently poisoned.
    let mut program = Program::new();
    let shapes = FileId::from_usize(1);
    let (shapes_hir, shapes_resolve, shapes_sigs) = program.analyse_module(SHAPES, shapes);

    let importer = "\
#import \"Shapes\";

main :: () {
    a := area(1);
}
";
    let analysis = program.analyse_with_imports(
        importer,
        FileId::from_usize(0),
        &[("Shapes", shapes, &shapes_hir, &shapes_resolve)],
        &[("Shapes", &shapes_sigs)],
    );
    assert_eq!(analysis.codes(), vec!["E0214"]);
}

#[test]
fn an_imported_pure_type_template_reports_its_owner_and_resolves_in_the_owner_environment() {
    let mut program = Program::new();
    let generic_file = FileId::from_usize(2);
    let owner_file = FileId::from_usize(1);
    let caller_file = FileId::from_usize(0);

    let (generic_hir, generic_resolve, generic_sigs) =
        program.analyse_module("Box :: struct($T) {\n    value: T;\n}\n", generic_file);
    let owner_source = "\
#import \"Generic\";

read_box :: (box: *Box($T)) -> T {
    todo;
}

make_box :: () -> Box(s64) {
    todo;
}
";
    let (owner_hir, owner_resolve, owner_sigs) = program.analyse_module_with_imports(
        owner_source,
        owner_file,
        &[("Generic", generic_file, &generic_hir, &generic_resolve)],
    );
    let owner_context = ImportedTemplateContext {
        file: owner_file,
        hir: &owner_hir,
        signatures: &owner_sigs,
        imports: vec![("Generic", &generic_sigs)],
        imported_hirs: vec![(generic_file, &generic_hir)],
    };

    let caller = "\
#import \"Owner\";

main :: () {
    box := make_box();
    value: s64 = read_box(*box);
}
";
    let analysis = program.analyse_with_import_contexts(
        caller,
        caller_file,
        &[("Owner", owner_file, &owner_hir, &owner_resolve)],
        &[("Owner", &owner_sigs)],
        &[owner_context],
    );
    analysis.assert_silent();

    let read_box = owner_sigs
        .lookup(program.interner.get("read_box").unwrap())
        .and_then(|entry| entry.proc)
        .unwrap();
    let (_, (template, key)) = analysis.instantiations.iter().next().unwrap();
    assert_eq!(template.file, owner_file);
    assert_eq!(template.proc, read_box);
    assert_eq!(key, &[PoolId::S64]);
    assert!(
        analysis.type_name_imports.is_empty(),
        "resolving the owner's `Generic.Box` must not mark an owner-only import as used by the caller"
    );
}

#[test]
fn imported_comptime_and_mixed_templates_remain_e0268() {
    let mut program = Program::new();
    let owner_file = FileId::from_usize(1);
    let owner_source = "\
sized :: ($N: s64) -> s64 {
    return N;
}

picked :: (value: $$T) -> T {
    return value;
}
";
    let (owner_hir, owner_resolve, owner_sigs) = program.analyse_module(owner_source, owner_file);
    let caller = "\
#import \"Owner\";

main :: () {
    a := sized(4);
    b := picked(5);
}
";
    let analysis = program.analyse_with_imports(
        caller,
        FileId::from_usize(0),
        &[("Owner", owner_file, &owner_hir, &owner_resolve)],
        &[("Owner", &owner_sigs)],
    );
    assert_eq!(analysis.codes(), vec!["E0268", "E0268"]);
    assert!(analysis.instantiations.is_empty());
}

#[test]
fn an_imported_polymorphic_expand_macro_remains_e0272() {
    let mut program = Program::new();
    let owner_file = FileId::from_usize(1);
    let owner_source = "\
expanded :: (value: $T) -> T #expand {
    return value;
}
";
    let (owner_hir, owner_resolve, owner_sigs) = program.analyse_module(owner_source, owner_file);
    let caller = "\
#import \"Owner\";

main :: () {
    value := expanded(42);
}
";
    let analysis = program.analyse_with_imports(
        caller,
        FileId::from_usize(0),
        &[("Owner", owner_file, &owner_hir, &owner_resolve)],
        &[("Owner", &owner_sigs)],
    );
    assert_eq!(analysis.codes(), vec!["E0272"]);
    assert!(analysis.instantiations.is_empty());
}

#[test]
fn a_module_cycle_types_in_both_directions() {
    // ADR-0014 §4 makes the cycle legal and ADR-0016 §5 is what keeps it
    // terminating: signatures depend on the other file's HIR, never on its check.
    // Both halves are computed here the same way `jr-db` computes them.
    let mut program = Program::new();
    let a_file = FileId::from_usize(0);
    let b_file = FileId::from_usize(1);

    let a_source = "\
#import \"Cycle_B\";

A_VALUE :: 1;

a_calls_b :: () -> s64 {
    return b_value();
}
";
    let b_source = "\
#import \"Cycle_A\";

b_value :: () -> s64 {
    return A_VALUE;
}
";

    let (a_hir, a_resolve, a_sigs) = program.analyse_module(a_source, a_file);
    let (b_hir, b_resolve, b_sigs) = program.analyse_module(b_source, b_file);

    let a = program.analyse_with_imports(
        a_source,
        a_file,
        &[("Cycle_B", b_file, &b_hir, &b_resolve)],
        &[("Cycle_B", &b_sigs)],
    );
    a.assert_silent();

    let b = program.analyse_with_imports(
        b_source,
        b_file,
        &[("Cycle_A", a_file, &a_hir, &a_resolve)],
        &[("Cycle_A", &a_sigs)],
    );
    b.assert_silent();
}

#[test]
fn an_imported_constant_keeps_its_type() {
    let mut program = Program::new();
    let colors = FileId::from_usize(1);
    let (hir, resolve, sigs) = program.analyse_module("BLACK :: 0;\nLABEL :: \"black\";\n", colors);

    let importer = "\
#import \"Colors\";

main :: () {
    n := BLACK;
    s := LABEL;
    bad: bool = LABEL;
}
";
    let analysis = program.analyse_with_imports(
        importer,
        FileId::from_usize(0),
        &[("Colors", colors, &hir, &resolve)],
        &[("Colors", &sigs)],
    );
    assert_eq!(
        analysis.codes(),
        vec!["E0214"],
        "an imported string constant must not satisfy a `bool` annotation"
    );
    assert_eq!(
        sigs.lookup(program.interner.get("BLACK").unwrap())
            .map(|e| e.ty),
        Some(PoolId::S64)
    );
}

#[test]
fn a_type_name_provided_by_two_modules_is_ambiguous() {
    // Name resolution raises E0211 for an ambiguous name used in an *expression*,
    // but a `TypeRef` is not an expression and `ResolveMap` never sees it. Type
    // position is this crate's to catch, under the same code.
    let mut program = Program::new();
    let first = FileId::from_usize(1);
    let second = FileId::from_usize(2);
    let source = "Shape :: struct {\n    x: s64;\n}\n";
    let (first_hir, first_resolve, first_sigs) = program.analyse_module(source, first);
    let (second_hir, second_resolve, second_sigs) = program.analyse_module(source, second);

    let importer = "\
#import \"First\";
#import \"Second\";

main :: () {
    s: Shape;
}
";
    let analysis = program.analyse_with_imports(
        importer,
        FileId::from_usize(0),
        &[
            ("First", first, &first_hir, &first_resolve),
            ("Second", second, &second_hir, &second_resolve),
        ],
        &[("First", &first_sigs), ("Second", &second_sigs)],
    );
    assert_eq!(analysis.codes(), vec!["E0211"]);
}

#[test]
fn a_file_level_declaration_shadows_an_imported_one() {
    // ADR-0014 §3, at the type level: adding an export to a module must never
    // break an importer that already declares that name itself.
    let mut program = Program::new();
    let module = FileId::from_usize(1);
    let (hir, resolve, sigs) =
        program.analyse_module("Shape :: struct {\n    x: s64;\n}\n", module);

    let importer = "\
#import \"Shapes\";

Shape :: struct {
    y: bool;
}

main :: () {
    s: Shape;
    s.y = true;
}
";
    let analysis = program.analyse_with_imports(
        importer,
        FileId::from_usize(0),
        &[("Shapes", module, &hir, &resolve)],
        &[("Shapes", &sigs)],
    );
    analysis.assert_silent();
}
