//! Unit tests for the typing rules ADR-0015 and ADR-0016 fix.
//!
//! Each test names the rule it pins down. The corpus can only constrain sema
//! *negatively* — no corpus file expected a type error before this wave — so
//! these are where the positive statements live.

mod harness;

use harness::Program;
use jr_hir::{BodyId, ExprId, ExprScope, LocalId};
use jr_pool::{Item, PoolId};
use jr_sema::ArgSlot;

// ---------------------------------------------------------------------------
// ADR-0016 §1 — context-typed integer literals
// ---------------------------------------------------------------------------

#[test]
fn todo_requires_no_expression_type_even_in_a_valued_procedure() {
    let mut program = Program::new();
    let analysis = program.analyse("unfinished :: () -> s64 {\n    todo;\n}\n");
    analysis.assert_silent();
}

#[test]
fn named_result_labels_do_not_change_type_identity_or_create_bindings() {
    let mut program = Program::new();
    let analysis = program.analyse(
        "labeled :: (n: s64) -> (value: s64) {\n\
             value := n + 1;\n\
             return value;\n\
         }\n\
         apply :: (f: (s64) -> s64, n: s64) -> s64 {\n\
             return f(n);\n\
         }\n\
         main :: () -> s64 {\n\
             return apply(labeled, 41);\n\
         }\n",
    );
    analysis.assert_silent();
}

#[test]
fn duplicate_named_result_labels_are_e0298() {
    let mut program = Program::new();
    let analysis =
        program.analyse("duplicate :: () -> (value: s64, value: bool) {\n    todo;\n}\n");
    assert_eq!(analysis.codes(), vec!["E0298"]);
}

#[test]
fn new_accepts_a_recursive_struct_behind_pointer_and_dynamic_array_indirection() {
    let mut program = Program::new();
    let analysis = program.analyse(
        "Table :: struct($K, $V) {\n\
             count: s64;\n\
         }\n\
         Node :: struct {\n\
             name: string;\n\
             children: [..]*Node;\n\
             properties: Table(string, string);\n\
         }\n\
         main :: () {\n\
             node := New(Node);\n\
             stack: [..]*Node;\n\
             stack.count = 0;\n\
             first: *Node = node;\n\
         }\n",
    );
    analysis.assert_silent();
}

#[test]
fn new_needs_a_type_argument() {
    let mut program = Program::new();
    let analysis = program.analyse("main :: () {\n    value := 1;\n    node := New(value);\n}\n");
    assert_eq!(analysis.codes(), vec!["E0261"]);
}

#[test]
fn new_needs_an_implicit_context() {
    let mut program = Program::new();
    let analysis = program.analyse("raw :: () #c_call {\n    node := New(s64);\n}\n");
    assert_eq!(analysis.codes(), vec!["E0254"]);
}

#[test]
fn new_refuses_alignment_the_allocator_protocol_cannot_request() {
    let mut program = Program::new();
    let analysis = program.analyse(
        "Wide :: struct {\n\
             value: s64 #align 32;\n\
         }\n\
         main :: () {\n\
             value := New(Wide);\n\
         }\n",
    );
    assert_eq!(analysis.codes(), vec!["E0266"]);
}

#[test]
fn polymorphic_calls_infer_through_parameterised_nominal_types() {
    let mut program = Program::new();
    let analysis = program.analyse(
        "Table :: struct($K, $V) {\n\
             key: K;\n\
             value: V;\n\
         }\n\
         measure :: (table: *Table($K, $V)) -> s64 {\n\
             return size_of(K) * 10 + size_of(V);\n\
         }\n\
         main :: () -> s64 {\n\
             table: Table(s64, u8);\n\
             return measure(*table);\n\
         }\n",
    );
    analysis.assert_silent();
    let measure = analysis
        .signatures
        .lookup(program.interner.get("measure").unwrap())
        .and_then(|entry| entry.proc)
        .unwrap();
    let (_, (template, key)) = analysis.instantiations.iter().next().unwrap();
    assert_eq!(template.file, jr_base::FileId::from_usize(0));
    assert_eq!(template.proc, measure);
    assert_eq!(key, &[PoolId::S64, PoolId::U8]);
}

#[test]
fn polymorphic_procedures_may_return_several_values_including_the_bound_type() {
    let mut program = Program::new();
    let analysis = program.analyse(
        "pick :: (items: *[..]$T) -> (T, bool) {\n\
             if items.count == 0 {\n\
                 empty: $T;\n\
                 return empty, false;\n\
             }\n\
             return items.data.*, true;\n\
         }\n\
         main :: () -> s64 {\n\
             items: [..]s64;\n\
             value, found := pick(*items);\n\
             if found {\n\
                 return value;\n\
             }\n\
             return 0;\n\
         }\n",
    );
    analysis.assert_silent();
}

#[test]
fn parameterised_inference_requires_the_same_nominal_declaration() {
    let mut program = Program::new();
    let analysis = program.analyse(
        "Left :: struct($T) { value: T; }\n\
         Right :: struct($T) { value: T; }\n\
         read :: (value: *Left($T)) -> T { return value.value; }\n\
         main :: () {\n\
             right: Right(s64);\n\
             n := read(*right);\n\
         }\n",
    );
    assert_eq!(analysis.codes(), vec!["E0268"]);
}

#[test]
fn an_integer_literal_takes_its_type_from_its_context() {
    // The rule that makes `valid/005-decl-typed.jr` legal in a subset with no
    // `cast`. If this regresses, that corpus file stops checking.
    let mut program = Program::new();
    let analysis = program.analyse("main :: () {\n    g: u8 = 255;\n}\n");
    analysis.assert_silent();
    assert_eq!(
        analysis
            .types
            .local_type(BodyId::from_usize(0), LocalId::from_usize(0)),
        Some(PoolId::U8)
    );
}

#[test]
fn an_integer_literal_with_no_context_defaults_to_s64() {
    let mut program = Program::new();
    let analysis = program.analyse("main :: () {\n    count := 10;\n}\n");
    analysis.assert_silent();
    assert_eq!(
        analysis
            .types
            .local_type(BodyId::from_usize(0), LocalId::from_usize(0)),
        Some(PoolId::S64)
    );
}

#[test]
fn a_literal_that_does_not_fit_its_contextual_type_is_e0204() {
    // The relocated check. Lowering accepted this, because lowering tested every
    // literal against `s64` and never saw the `u8`.
    let mut program = Program::new();
    let analysis = program.analyse("main :: () {\n    g: u8 = 300;\n}\n");
    assert_eq!(analysis.codes(), vec!["E0204"]);
}

#[test]
fn a_literal_too_large_for_s64_is_reported_by_sema_not_lowering() {
    let mut program = Program::new();
    let analysis = program.analyse("X :: 9223372036854775808;\n");
    assert!(
        analysis.earlier_diagnostics.is_empty(),
        "lowering must not judge a literal whose type it cannot know"
    );
    assert_eq!(analysis.codes(), vec!["E0204"]);
}

#[test]
fn context_typing_reaches_through_arithmetic() {
    // `1 + 2` has no type of its own either, so the annotation has to reach the
    // leaves, not just the outermost node.
    let mut program = Program::new();
    let analysis = program.analyse("main :: () {\n    g: u8 = 1 + 2;\n}\n");
    analysis.assert_silent();
}

#[test]
fn a_literal_compared_with_a_typed_value_adopts_its_type() {
    let mut program = Program::new();
    let analysis = program.analyse(
        "main :: () {\n    g: u8 = 200;\n    same := g == 200;\n    also := 200 == g;\n}\n",
    );
    analysis.assert_silent();
}

#[test]
fn switch_integer_cases_compare_decoded_values() {
    let mut program = Program::new();
    let analysis = program.analyse(
        "main :: () {\n    n := 1;\n    switch n {\n        case 1;\n        case 0x1;\n        else;\n    }\n}\n",
    );
    assert_eq!(analysis.codes(), vec!["E0259"]);
}

#[test]
fn one_enum_alias_covers_its_runtime_value_class() {
    let mut program = Program::new();
    let analysis = program.analyse(
        "Status :: enum {\n    OK :: 200;\n    ALSO_OK :: 200;\n    MISSING :: 404;\n}\n\npick :: (s: Status) {\n    switch s {\n        case .OK;\n        case .MISSING;\n    }\n}\n",
    );
    analysis.assert_silent();
}

#[test]
fn two_enum_alias_cases_are_e0259() {
    let mut program = Program::new();
    let analysis = program.analyse(
        "Status :: enum {\n    OK :: 200;\n    ALSO_OK :: 200;\n}\n\npick :: (s: Status) {\n    switch s {\n        case .OK;\n        case .ALSO_OK;\n    }\n}\n",
    );
    assert_eq!(analysis.codes(), vec!["E0259"]);
}

#[test]
fn enum_alias_missing_note_names_only_uncovered_value_classes() {
    let mut program = Program::new();
    let analysis = program.analyse(
        "Status :: enum {\n    OK :: 200;\n    ALSO_OK :: 200;\n    MISSING :: 404;\n}\n\npick :: (s: Status) {\n    switch s {\n        case .ALSO_OK;\n    }\n}\n",
    );
    assert_eq!(analysis.codes(), vec!["E0258"]);
    let diagnostic = analysis
        .sema_diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == Some("E0258"))
        .expect("the missing-value diagnostic");
    let notes: Vec<&str> = diagnostic
        .notes
        .iter()
        .map(|(_, note)| note.as_str())
        .collect();
    assert!(
        notes.contains(&"missing: `MISSING`"),
        "expected only the uncovered value class, got {notes:?}"
    );
}

#[test]
fn a_second_switch_else_remains_e0259() {
    let mut program = Program::new();
    let analysis = program.analyse(
        "main :: () {\n    n := 1;\n    switch n {\n        else;\n        else;\n    }\n}\n",
    );
    assert_eq!(analysis.codes(), vec!["E0259"]);
}

// ---------------------------------------------------------------------------
// ADR-0016 §2 — binding nothing
// ---------------------------------------------------------------------------

#[test]
fn binding_the_result_of_a_void_procedure_is_e0217() {
    let mut program = Program::new();
    let analysis = program.analyse("nothing :: () {\n}\n\nmain :: () {\n    x := nothing();\n}\n");
    assert_eq!(analysis.codes(), vec!["E0217"]);
}

#[test]
fn calling_a_void_procedure_as_a_statement_is_fine() {
    let mut program = Program::new();
    let analysis = program.analyse("nothing :: () {\n}\n\nmain :: () {\n    nothing();\n}\n");
    analysis.assert_silent();
}

// ---------------------------------------------------------------------------
// ADR-0016 §3 — the foreign library handle type
// ---------------------------------------------------------------------------

#[test]
fn a_system_library_constant_has_the_foreign_library_type() {
    let mut program = Program::new();
    let analysis = program.analyse("libc :: #system_library \"c\";\n");
    analysis.assert_silent();
    assert_eq!(
        analysis.type_of(&program.interner, "libc"),
        Some(PoolId::FOREIGN_LIBRARY)
    );
}

#[test]
fn a_foreign_binding_against_a_real_library_checks() {
    let mut program = Program::new();
    let analysis = program.analyse(
        "libc :: #system_library \"c\";\n\nwrite :: (fd: s64, buf: *u8, count: s64) -> s64 #foreign libc \"write\";\n",
    );
    analysis.assert_silent();
}

#[test]
fn a_foreign_binding_against_something_else_is_e0225() {
    let mut program = Program::new();
    let analysis =
        program.analyse("nope :: 1;\n\nwrite :: (fd: s64) -> s64 #foreign nope \"write\";\n");
    assert_eq!(analysis.codes(), vec!["E0225"]);
}

// ---------------------------------------------------------------------------
// ADR-0016 §4 — `#run` is typed, not folded
// ---------------------------------------------------------------------------

#[test]
fn run_has_the_type_of_its_expression() {
    let mut program = Program::new();
    let analysis = program.analyse(
        "COMPUTED :: #run add(2, 3);\n\nadd :: (a: s64, b: s64) -> s64 {\n    return a + b;\n}\n",
    );
    analysis.assert_silent();
    assert_eq!(
        analysis.type_of(&program.interner, "COMPUTED"),
        Some(PoolId::S64),
        "`#run` must yield a usable type even though its value waits for the VM"
    );
}

// ---------------------------------------------------------------------------
// ADR-0015 — nominal structs, and identity before completeness
// ---------------------------------------------------------------------------

#[test]
fn a_struct_may_hold_a_pointer_to_itself() {
    // Only possible because a struct type's identity is its declaration site, so
    // it has an id before its fields are resolved. If this reports a constant
    // cycle, the pre-pass in the signature phase has regressed.
    let mut program = Program::new();
    let analysis = program.analyse("Node :: struct {\n    next: *Node;\n}\n");
    analysis.assert_silent();
}

#[test]
fn two_structs_may_point_at_each_other() {
    let mut program = Program::new();
    let analysis = program.analyse(
        "Left :: struct {\n    other: *Right;\n}\n\nRight :: struct {\n    other: *Left;\n}\n",
    );
    analysis.assert_silent();
}

#[test]
fn two_structs_with_identical_fields_are_different_types() {
    let mut program = Program::new();
    let analysis = program.analyse(
        "A :: struct {\n    x: s64;\n}\n\nB :: struct {\n    x: s64;\n}\n\nmain :: () {\n    a: A;\n    b: B;\n    b = a;\n}\n",
    );
    assert_eq!(
        analysis.codes(),
        vec!["E0214"],
        "struct types are nominal (ADR-0015 §1), so these must not be interchangeable"
    );
}

// ---------------------------------------------------------------------------
// Places: what can be assigned to and what has an address
// ---------------------------------------------------------------------------

#[test]
fn field_access_through_a_pointer_is_assignable() {
    // `valid/015-pointers.jr` relies on this, and no ADR states it.
    let mut program = Program::new();
    let analysis = program.analyse(
        "Point :: struct {\n    x: s64;\n}\n\nmain :: () {\n    origin: Point;\n    pp := *origin;\n    pp.x = 1;\n}\n",
    );
    analysis.assert_silent();
}

#[test]
fn a_dereference_is_assignable() {
    let mut program = Program::new();
    let analysis = program.analyse("main :: () {\n    n := 1;\n    p := *n;\n    p.* = 2;\n}\n");
    analysis.assert_silent();
}

#[test]
fn a_string_exposes_data_and_count() {
    // ADR-0004 fixes the layout and makes both directly accessible; `string` is
    // still not the struct of that shape (ADR-0015 §2).
    let mut program = Program::new();
    let analysis = program
        .analyse("main :: () {\n    s := \"text\";\n    n := s.count;\n    d := s.data;\n}\n");
    analysis.assert_silent();
    assert_eq!(
        analysis
            .types
            .local_type(BodyId::from_usize(0), LocalId::from_usize(1)),
        Some(PoolId::S64)
    );
    assert_eq!(
        analysis
            .types
            .local_type(BodyId::from_usize(0), LocalId::from_usize(2)),
        Some(PoolId::PTR_U8)
    );
}

// ---------------------------------------------------------------------------
// Poison
// ---------------------------------------------------------------------------

#[test]
fn a_parse_error_does_not_become_a_type_error() {
    // `file_diagnostics` does not gate phases, so this is the property that keeps
    // one syntax error from producing a page of invented type errors.
    let mut program = Program::new();
    let analysis = program.analyse("main :: () {\n    a := 1\n    b := 2;\n}\n");
    assert!(
        !analysis.earlier_diagnostics.is_empty(),
        "the test input must actually fail to parse"
    );
    analysis.assert_silent();
}

#[test]
fn an_unresolved_name_does_not_become_a_type_error() {
    let mut program = Program::new();
    let analysis = program.analyse("main :: () {\n    x := missing;\n}\n");
    assert!(!analysis.earlier_diagnostics.is_empty());
    analysis.assert_silent();
}

#[test]
fn an_unknown_type_poisons_rather_than_cascades() {
    let mut program = Program::new();
    let analysis = program.analyse("main :: () {\n    n: nonesuch = 1 + 2;\n    m := n + 1;\n}\n");
    assert_eq!(
        analysis.codes(),
        vec!["E0212"],
        "the unknown annotation must be the only complaint"
    );
}

// ---------------------------------------------------------------------------
// The arena trap
// ---------------------------------------------------------------------------

#[test]
fn expression_ids_from_different_arenas_get_different_types() {
    // `FileHir::exprs` and each `Body::exprs` both start at index 0. A type map
    // keyed on a bare `ExprId` would report one of these as the other — which is
    // exactly the bug that was found and fixed in `jr-hir`'s `ResolveMap`.
    let mut program = Program::new();
    let analysis = program.analyse("FLAG :: true;\n\nmain :: () {\n    n := 1;\n}\n");
    analysis.assert_silent();
    let first = ExprId::from_usize(0);
    assert_eq!(
        analysis.types.expr_type(ExprScope::TopLevel, first),
        Some(PoolId::BOOL)
    );
    assert_eq!(
        analysis
            .types
            .expr_type(ExprScope::Body(BodyId::from_usize(0)), first),
        Some(PoolId::S64)
    );
}

// ---------------------------------------------------------------------------
// Order independence and cycles within a file
// ---------------------------------------------------------------------------

#[test]
fn a_constant_may_refer_to_one_declared_later() {
    let mut program = Program::new();
    let analysis = program.analyse("LIMIT :: MAX;\nMAX :: 4096;\n");
    analysis.assert_silent();
    assert_eq!(
        analysis.type_of(&program.interner, "LIMIT"),
        Some(PoolId::S64)
    );
}

#[test]
fn a_constant_cycle_is_reported_once() {
    let mut program = Program::new();
    let analysis = program.analyse("FIRST :: SECOND;\nSECOND :: FIRST;\n");
    assert_eq!(analysis.codes(), vec!["E0226"]);
}

// ---------------------------------------------------------------------------
// Calls
// ---------------------------------------------------------------------------

#[test]
fn inferred_parameter_defaults_get_fixed_natural_types() {
    let mut program = Program::new();
    let analysis = program.analyse(
        "inferred :: (whole := 9, ratio := 0.5, flag := true, label := \"x\", negative := -9) -> s64 {\n\
             return whole + negative;\n\
         }\n\
         main :: () {\n\
             a := inferred();\n\
             b := inferred(10, ratio = 1.5, flag = false, label = \"y\", negative = -10);\n\
         }\n",
    );
    analysis.assert_silent();
    assert!(
        analysis.earlier_diagnostics.is_empty(),
        "{:?}",
        analysis.earlier_diagnostics
    );

    let inferred = program.interner.get("inferred").unwrap();
    let proc = analysis
        .signatures
        .lookup(inferred)
        .and_then(|entry| entry.proc)
        .unwrap();
    let sig = analysis.signatures.proc_sig(proc).unwrap();
    assert_eq!(sig.params[0], PoolId::S64);
    assert!(matches!(
        program.pool.item(sig.params[1]),
        Item::FloatType { bits: 64 }
    ));
    assert_eq!(sig.params[2], PoolId::BOOL);
    assert_eq!(sig.params[3], PoolId::STRING);
    assert_eq!(sig.params[4], PoolId::S64);
    assert!(sig.defaults.iter().all(Option::is_some));
}

#[test]
fn supplied_arguments_do_not_reinfer_an_inferred_parameter() {
    let mut program = Program::new();
    let analysis = program.analyse("take :: (amount := 9) {}\nmain :: () {\n    take(true);\n}\n");
    assert_eq!(analysis.codes(), vec!["E0214"]);
}

#[test]
fn null_cannot_infer_a_parameter_type() {
    let mut program = Program::new();
    let analysis = program.analyse("take :: (pointer := null) {}\n");
    assert_eq!(analysis.codes(), vec!["E0257"]);
}

#[test]
fn inferred_parameter_defaults_are_still_literal_only() {
    let mut program = Program::new();
    let analysis = program.analyse("SIZE :: 9;\ntake :: (amount := SIZE) {}\n");
    assert_eq!(analysis.codes(), vec!["E0252"]);
}

#[test]
fn pure_type_templates_align_named_arguments_and_fill_fixed_defaults() {
    let mut program = Program::new();
    let analysis = program.analyse(
        "choose :: (value: $T, scale: s64 = 2, label := \"item\") -> T {\n\
             return value;\n\
         }\n\
         main :: () {\n\
             first := choose(7);\n\
             second := choose(scale = 4, value = 8);\n\
             third := choose(value = 9, label = \"named\");\n\
         }\n",
    );
    analysis.assert_silent();
    assert_eq!(analysis.instantiations.len(), 3);
    assert_eq!(analysis.filled_calls.len(), 3);

    let mut shapes: Vec<Vec<char>> = analysis
        .filled_calls
        .values()
        .map(|slots| {
            slots
                .iter()
                .map(|slot| match slot {
                    ArgSlot::Given(_) => 'G',
                    ArgSlot::Default(_) => 'D',
                })
                .collect()
        })
        .collect();
    shapes.sort();
    assert_eq!(
        shapes,
        vec![
            vec!['G', 'D', 'D'],
            vec!['G', 'D', 'G'],
            vec!['G', 'G', 'D'],
        ]
    );
}

#[test]
fn a_polymorphic_parameter_cannot_supply_its_own_default() {
    let mut program = Program::new();
    let analysis = program.analyse(
        "bad_direct :: (value: $T = 3) -> T {\n\
             return value;\n\
         }\n\
         bad_use :: (seed: $T, value: T = 3) -> T {\n\
             return value;\n\
         }\n",
    );
    assert_eq!(analysis.codes(), vec!["E0252", "E0252"]);
}

#[test]
fn defaults_remain_deferred_on_comptime_and_mixed_templates() {
    let mut program = Program::new();
    let analysis = program.analyse(
        "typed :: ($N: s64, scale: s64 = 2) -> s64 {\n\
             return N * scale;\n\
         }\n\
         inferred :: ($N := 9) -> s64 {\n\
             return N;\n\
         }\n\
         mixed :: (value: $T, $N: s64, label := \"item\") -> T {\n\
             return value;\n\
         }\n",
    );
    assert_eq!(analysis.codes(), vec!["E0252", "E0252", "E0252"]);
}

#[test]
fn named_arguments_remain_deferred_on_comptime_and_mixed_templates() {
    let mut program = Program::new();
    let analysis = program.analyse(
        "valued :: ($N: s64) -> s64 {\n\
             return N;\n\
         }\n\
         mixed :: (value: $T, $N: s64) -> T {\n\
             return value;\n\
         }\n\
         main :: () {\n\
             a := valued(N = 3);\n\
             b := mixed(N = 4, value = 8);\n\
         }\n",
    );
    assert_eq!(analysis.codes(), vec!["E0252", "E0252"]);
}

#[test]
fn inferred_defaults_remain_illegal_on_foreign_parameters() {
    let mut program = Program::new();
    let analysis = program.analyse(
        "libc :: #system_library \"c\";\n\
         read :: (fd := 1, data: *u8, count: s64) -> s64 #foreign libc \"read\";\n",
    );
    assert_eq!(analysis.codes(), vec!["E0252"]);
}

#[test]
fn assert_records_an_unmessaged_check() {
    let mut program = Program::new();
    let analysis = program.analyse("main :: () {\n    assert(true);\n}\n");
    analysis.assert_silent();
    assert_eq!(analysis.assertions.len(), 1);
    assert_eq!(analysis.assertions.values().next(), Some(&None));
}

#[test]
fn assert_records_the_decoded_literal_message() {
    let mut program = Program::new();
    let analysis = program.analyse("main :: () {\n    assert(false, \"first\\nsecond\");\n}\n");
    analysis.assert_silent();
    assert_eq!(
        analysis.assertions.values().next(),
        Some(&Some("first\nsecond".to_owned()))
    );
}

#[test]
fn assert_requires_one_or_two_arguments() {
    let mut program = Program::new();
    let analysis = program.analyse("main :: () {\n    assert();\n}\n");
    assert_eq!(analysis.codes(), vec!["E0216"]);
    assert!(analysis.assertions.is_empty());
}

#[test]
fn assert_requires_a_boolean_condition() {
    let mut program = Program::new();
    let analysis = program.analyse("main :: () {\n    assert(1);\n}\n");
    assert_eq!(analysis.codes(), vec!["E0214"]);
}

#[test]
fn assert_requires_a_literal_message() {
    let mut program = Program::new();
    let analysis = program
        .analyse("main :: () {\n    message := \"computed\";\n    assert(true, message);\n}\n");
    assert_eq!(analysis.codes(), vec!["E0297"]);
    assert!(analysis.assertions.is_empty());
}

#[test]
fn a_declared_assert_shadows_the_intrinsic() {
    let mut program = Program::new();
    let analysis =
        program.analyse("assert :: (condition: bool) {\n}\n\nmain :: () {\n    assert(true);\n}\n");
    analysis.assert_silent();
    assert!(analysis.assertions.is_empty());
}

#[test]
fn assert_has_no_named_parameters() {
    let mut program = Program::new();
    let analysis = program.analyse("main :: () {\n    assert(condition = true);\n}\n");
    assert_eq!(analysis.codes(), vec!["E0252"]);
    assert!(analysis.assertions.is_empty());
}

#[test]
fn a_call_checks_its_arguments_against_the_parameters() {
    let mut program = Program::new();
    let analysis =
        program.analyse("takes_bool :: (flag: bool) {\n}\n\nmain :: () {\n    takes_bool(1);\n}\n");
    assert_eq!(analysis.codes(), vec!["E0214"]);
}

#[test]
fn a_call_with_the_wrong_arity_is_e0216() {
    let mut program = Program::new();
    let analysis = program.analyse(
        "one :: (a: s64) -> s64 {\n    return a;\n}\n\nmain :: () {\n    x := one();\n}\n",
    );
    assert_eq!(analysis.codes(), vec!["E0216"]);
}

#[test]
fn a_return_type_mismatch_is_reported_against_the_signature() {
    let mut program = Program::new();
    let analysis = program.analyse("flag :: () -> bool {\n    return 1;\n}\n");
    assert_eq!(analysis.codes(), vec!["E0214"]);
}

// ---------------------------------------------------------------------------
// ADR-0019 §4 — the resolved `#foreign` library
// ---------------------------------------------------------------------------

#[test]
fn a_foreign_procedures_library_is_resolved_and_interned() {
    // The single resolution ADR-0019 §4 consolidated. `#foreign libc "write"`
    // names the *constant* `libc`; the recorded answer must be the library `"c"`
    // that constant declares.
    //
    // Asserted on the string rather than merely on `Some(_)`, because the VM's
    // FFI bridge falls back to a process-wide `dlsym` when the library is unknown
    // — so `libc` resolving to nothing would still run `024-hello.jr` correctly
    // and hide the regression. The native back end has no such fallback.
    let mut program = Program::new();
    let analysis = program.analyse(
        "libc :: #system_library \"c\";\n\
         write :: (fd: s64, buf: *u8, count: s64) -> s64 #foreign libc \"write\";\n",
    );
    analysis.assert_silent();

    let library = analysis
        .signatures
        .foreign_library(jr_hir::ProcId::from_usize(0))
        .expect("a `#foreign` procedure naming a `#system_library` must record its library");
    assert_eq!(program.pool.foreign_library_name(library), Some("c"));
}

#[test]
fn a_procedure_that_is_not_foreign_records_no_library() {
    let mut program = Program::new();
    let analysis = program.analyse("add :: (a: s64) -> s64 {\n    return a;\n}\n");
    analysis.assert_silent();
    assert_eq!(
        analysis
            .signatures
            .foreign_library(jr_hir::ProcId::from_usize(0)),
        None
    );
}

#[test]
fn a_foreign_library_that_is_not_a_system_library_records_nothing() {
    // E0225's own case, from the recording side. Sema already refuses this; the
    // point here is that nothing is recorded either, so a back end refuses to
    // guess a library name rather than emitting a link against `"NOT_A_LIBRARY"`.
    let mut program = Program::new();
    let analysis = program.analyse(
        "NOT_A_LIBRARY :: 42;\n\
         write :: (fd: s64) -> s64 #foreign NOT_A_LIBRARY \"write\";\n",
    );
    assert_eq!(analysis.codes(), vec!["E0225"]);
    assert_eq!(
        analysis
            .signatures
            .foreign_library(jr_hir::ProcId::from_usize(0)),
        None
    );
}

// ---------------------------------------------------------------------------
// ADR-0071 — a type is a compile-time value
// ---------------------------------------------------------------------------

#[test]
fn a_type_bound_to_a_local_is_refused() {
    // The silent miscompile ADR-0071 §3 closes, and the reason the sub-wave shipped alone.
    //
    // Before E0261 this analysed **silently** and both engines exited 0, lowering to `s0: type` and
    // `v1: type = undef` — a placeholder that is a legitimate value, stored into a slot of a type
    // with no runtime layout at all (`LayoutError::ComptimeOnly`). PLAN §5's first named failure
    // mode, invisible to the verifier and to ADR-0017 §4's poison gate alike.
    //
    // The `:=` form specifically, because that is what got through: every position *with* an
    // expectation was already caught by an ordinary mismatch (`takes(Point)` is E0214, `if Point` is
    // E0222), and a binding has no expectation to mismatch against.
    let mut program = Program::new();
    let analysis = program.analyse(
        "Point :: struct {\n    x: s64;\n}\n\
         main :: () {\n    t := Point;\n}\n",
    );
    assert_eq!(analysis.codes(), vec!["E0261"]);
}

#[test]
fn a_type_named_as_a_bare_statement_is_refused() {
    // The other expectation-free position. `Point;` is a statement whose result is discarded, so
    // `check_stmt` imposes nothing — and it too analysed silently before this wave.
    let mut program = Program::new();
    let analysis = program.analyse(
        "Point :: struct {\n    x: s64;\n}\n\
         main :: () {\n    Point;\n}\n",
    );
    assert_eq!(analysis.codes(), vec!["E0261"]);
}

#[test]
fn a_field_access_receiver_may_name_a_type() {
    // The first of the two positions that *do* accept a type (ADR-0071 §3): `Colour.RED`'s receiver
    // is the enum type used as a value (ADR-0041 §1).
    //
    // This is the test that would catch an over-broad refusal, and it is why the allowlist is
    // populated by the code that creates each position rather than inferred from an expression's
    // shape. Without the `type_position` entry, every enum member access in the corpus breaks.
    let mut program = Program::new();
    let analysis = program.analyse(
        "Colour :: enum {\n    RED;\n    GREEN;\n}\n\
         main :: () {\n    c := Colour.RED;\n}\n",
    );
    analysis.assert_silent();
}

#[test]
fn a_type_valued_constant_denotes_the_type_it_aliases() {
    // ADR-0071 §2, asserted on `type_value` rather than on silence — and the distinction has teeth.
    //
    // A `SigEntry` whose `ty` is `PoolId::TYPE` but whose `type_value` is `None` analyses perfectly
    // quietly and then reports "`Pair` is a constant, not a type" (E0213) the moment anyone writes
    // `p: Pair;`, because `resolve_type_name` reads exactly this field. So silence alone would pass
    // with the alias half of the feature disabled; this asserts the *identity*.
    //
    // Compared against `Point`'s own entry, which is what proves an alias creates no second nominal
    // type (ADR-0015 §1 makes identity the declaration site).
    let mut program = Program::new();
    let analysis = program.analyse(
        "Point :: struct {\n    x: s64;\n}\n\
         Pair :: Point;\n",
    );
    analysis.assert_silent();

    let point = program.interner.get("Point").expect("`Point` is interned");
    let pair = program.interner.get("Pair").expect("`Pair` is interned");
    let denoted = analysis
        .signatures
        .lookup(pair)
        .expect("`Pair` has a signature")
        .type_value;
    let aliased = analysis
        .signatures
        .lookup(point)
        .expect("`Point` has a signature")
        .type_value;
    assert!(aliased.is_some(), "a struct denotes its own type");
    assert_eq!(
        denoted, aliased,
        "an alias must denote the *same* type, not a second one"
    );
}

#[test]
fn an_alias_of_an_alias_is_not_followed() {
    // ADR-0071 §5's deliberate limit, and the line ADR-0070 §4 drew for an array length: one level
    // is a lookup, a chain needs a fixpoint and a cycle check.
    //
    // Asserted as `None` rather than as a diagnostic, because the refusal surfaces where the chain is
    // *used* — `p: Second;` is E0213 — and pinning it here says which decision produced that.
    let mut program = Program::new();
    let analysis = program.analyse(
        "Point :: struct {\n    x: s64;\n}\n\
         First :: Point;\n\
         Second :: First;\n",
    );
    analysis.assert_silent();

    let second = program
        .interner
        .get("Second")
        .expect("`Second` is interned");
    assert_eq!(
        analysis
            .signatures
            .lookup(second)
            .expect("`Second` has a signature")
            .type_value,
        None,
        "a chain of aliases is deliberately not followed (ADR-0071 §5)"
    );
}

#[test]
fn a_poisoned_context_suppresses_the_type_refusal() {
    // `expect`'s rule, applied to E0261: poison propagates silently in both directions, because
    // `file_diagnostics` does not gate later phases on earlier ones. Without it,
    // `n: nosuchtype = Point;` reported E0212 *and* E0261 — two diagnostics for one mistake.
    //
    // Found by probing after the wave was committed, and worth a test rather than only a fix: this
    // arm returns *before* reaching `expect`, so it has to know what `expect` knows, and a later
    // refusal added to the same arm would have the same trap waiting.
    let mut program = Program::new();
    let analysis = program.analyse(
        "Point :: struct {\n    x: s64;\n}\n\
         main :: () {\n    n: nosuchtype = Point;\n}\n",
    );
    assert_eq!(
        analysis.codes(),
        vec!["E0212"],
        "the unknown type is the one mistake; the type-as-value refusal must stay quiet"
    );
}
