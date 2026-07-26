//! Positive statements about what lowering produces, which the corpus cannot make.
//!
//! The corpus constrains `jr-mir` only *negatively*: every valid file must lower
//! without the verifier objecting, and a snapshot pins whatever comes out. Neither
//! says that an address-taken local ends up in a slot, that `&&` actually
//! short-circuits, or that a loop-carried variable gets exactly one block
//! parameter. Those are the decisions ADR-0017 took, and this file is where they
//! are asserted — because every one of them is a silent failure if it regresses:
//! the MIR stays well-formed and merely means something else.
//!
//! The refusal tests matter for the same reason. ADR-0017 §4 makes refusing a
//! *feature*, so a body that starts lowering when it should not is a regression
//! no amount of well-formedness checking would catch.

mod harness;

use harness::Program;
use jr_mir::{Poisoned, Rvalue, Statement, Terminator};

// ---------------------------------------------------------------------------
// Well-formedness
// ---------------------------------------------------------------------------

#[test]
fn an_empty_void_procedure_lowers_to_a_single_returning_block() {
    let mut program = Program::new();
    let lowered = program.lower_clean("main :: () { }");
    let body = lowered.body(&program.interner, "main");
    assert_eq!(body.block_count(), 1);
    assert_eq!(body.block(body.entry()).term, Terminator::Return(None));
}

#[test]
fn a_procedure_returning_a_parameter_returns_the_entry_parameter() {
    let mut program = Program::new();
    let lowered = program.lower_clean("id :: (a: s64) -> s64 { return a; }");
    let body = lowered.body(&program.interner, "id");
    assert_eq!(
        body.params().len(),
        1,
        "one declared parameter, one entry parameter"
    );
    assert_eq!(
        body.block(body.entry()).term,
        Terminator::Return(Some(jr_mir::Operand::Value(body.params()[0]))),
        "returning a parameter must not copy it through a temporary"
    );
}

#[test]
fn every_corpus_shaped_construct_lowers_without_the_verifier_objecting() {
    // One procedure per construct the Jairs-0 subset has, so a regression in any
    // of them shows up here before it shows up in a corpus snapshot.
    let mut program = Program::new();
    let lowered = program.lower_clean(
        "
Point :: struct {
    x: s64;
    y: s64;
}

add :: (a: s64, b: s64) -> s64 { return a + b; }

arithmetic :: () {
    a := 1 + 2 * 3 - 4;
    b := a / 2;
    c := b % 3;
    d := a +% b -% c *% 2;
}

comparisons :: () {
    a := 1 < 2;
    b := 1 == 2;
    c := a && b;
    d := a || b;
    e := !a;
}

branches :: (n: s64) -> s64 {
    if n > 0 {
        return 1;
    } else {
        return 0;
    }
}

loops :: () {
    i := 0;
    while i < 10 {
        if i == 5 {
            break;
        }
        i = i + 1;
        continue;
    }
}

aggregates :: () {
    p: Point;
    p.x = 1;
    p.y = 2;
    sum := p.x + p.y;
}

pointers :: () {
    value := 42;
    p := *value;
    copied := p.*;
    p.* = 43;
}

calls :: () -> s64 {
    x := add(1, 2);
    add(3, 4);
    return add(x, add(5, 6));
}
",
    );

    for name in [
        "add",
        "arithmetic",
        "comparisons",
        "branches",
        "loops",
        "aggregates",
        "pointers",
        "calls",
    ] {
        // `body` panics on a refusal, and lowering already ran `verify::assert_valid`.
        let body = lowered.body(&program.interner, name);
        assert!(body.block_count() >= 1, "`{name}` produced no blocks");
    }
    assert_eq!(lowered.mir.lowered_count(), 8, "{}", program.dump(&lowered));
}

// ---------------------------------------------------------------------------
// Locals: registers versus memory
// ---------------------------------------------------------------------------

#[test]
fn a_plain_local_becomes_an_ssa_value_with_no_slot() {
    let mut program = Program::new();
    let lowered = program.lower_clean("main :: () -> s64 { x := 7; return x; }");
    let body = lowered.body(&program.interner, "main");
    assert_eq!(
        body.slot_count(),
        0,
        "a local whose address is never taken needs no memory"
    );
    // Reading it must yield the interned constant directly rather than a load.
    assert!(
        !body
            .blocks()
            .iter()
            .any(|block| block.stmts.iter().any(|stmt| matches!(
                stmt,
                Statement::Assign {
                    rvalue: Rvalue::Load(_),
                    ..
                }
            ))),
        "{}",
        program.dump(&lowered)
    );
}

#[test]
fn an_address_taken_local_is_lowered_through_a_stack_slot() {
    let mut program = Program::new();
    let lowered = program.lower_clean("main :: () { value := 1; p := *value; }");
    let body = lowered.body(&program.interner, "main");
    assert_eq!(
        body.slot_count(),
        1,
        "taking an address forces the local into memory"
    );
    let has_store = body.blocks().iter().any(|block| {
        block
            .stmts
            .iter()
            .any(|stmt| matches!(stmt, Statement::Store { .. }))
    });
    let has_address = body.blocks().iter().any(|block| {
        block.stmts.iter().any(|stmt| {
            matches!(
                stmt,
                Statement::Assign {
                    rvalue: Rvalue::Address(_),
                    ..
                }
            )
        })
    });
    assert!(
        has_store,
        "the initialiser must be written to the slot: {}",
        program.dump(&lowered)
    );
    assert!(
        has_address,
        "prefix `*` must become an address of that slot"
    );
}

#[test]
fn a_struct_local_is_lowered_through_a_stack_slot() {
    let mut program = Program::new();
    let lowered = program
        .lower_clean("Point :: struct { x: s64; y: s64; }\nmain :: () { p: Point; p.x = 1; }");
    let body = lowered.body(&program.interner, "main");
    assert_eq!(
        body.slot_count(),
        1,
        "an aggregate is not register-representable"
    );
}

// ---------------------------------------------------------------------------
// Control flow
// ---------------------------------------------------------------------------

#[test]
fn an_if_that_assigns_in_both_arms_merges_through_a_block_parameter() {
    let mut program = Program::new();
    let lowered = program.lower_clean(
        "main :: (c: bool) -> s64 { x := 0; if c { x = 1; } else { x = 2; } return x; }",
    );
    let body = lowered.body(&program.interner, "main");
    let merges: usize = body
        .blocks()
        .iter()
        .skip(1) // the entry block's parameters are the procedure's parameters
        .map(|block| block.params.len())
        .sum();
    assert_eq!(
        merges,
        1,
        "two arms, two values, one merge: {}",
        program.dump(&lowered)
    );
}

#[test]
fn an_if_that_leaves_a_variable_alone_keeps_no_block_parameter() {
    let mut program = Program::new();
    let lowered = program.lower_clean("main :: (c: bool) -> s64 { x := 0; if c { } return x; }");
    let body = lowered.body(&program.interner, "main");
    let merges: usize = body
        .blocks()
        .iter()
        .skip(1)
        .map(|block| block.params.len())
        .sum();
    assert_eq!(
        merges,
        0,
        "both paths agree, so the parameter is trivial and must be collapsed: {}",
        program.dump(&lowered)
    );
}

#[test]
fn a_loop_carried_variable_gets_exactly_one_block_parameter() {
    let mut program = Program::new();
    let lowered = program.lower_clean("main :: () { i := 0; while i < 3 { i = i + 1; } }");
    let body = lowered.body(&program.interner, "main");
    let merges: usize = body.blocks().iter().map(|block| block.params.len()).sum();
    assert_eq!(
        merges,
        1,
        "the loop header carries `i` and nothing else does: {}",
        program.dump(&lowered)
    );
}

#[test]
fn short_circuit_and_lowers_to_a_branch_rather_than_an_operator() {
    let mut program = Program::new();
    let lowered = program.lower_clean("main :: (a: bool, b: bool) -> bool { return a && b; }");
    let body = lowered.body(&program.interner, "main");
    // MIR's `BinOp` has no `And`, so the only possible lowering is control flow.
    assert!(
        body.block_count() > 1,
        "`&&` must branch: {}",
        program.dump(&lowered)
    );
    let branches = body
        .blocks()
        .iter()
        .filter(|block| matches!(block.term, Terminator::Branch { .. }))
        .count();
    assert_eq!(branches, 1);
}

#[test]
fn short_circuit_or_lowers_to_a_branch_rather_than_an_operator() {
    let mut program = Program::new();
    let lowered = program.lower_clean("main :: (a: bool, b: bool) -> bool { return a || b; }");
    let body = lowered.body(&program.interner, "main");
    let branches = body
        .blocks()
        .iter()
        .filter(|block| matches!(block.term, Terminator::Branch { .. }))
        .count();
    assert_eq!(branches, 1, "{}", program.dump(&lowered));
}

#[test]
fn break_and_continue_reach_the_loop_exit_and_header() {
    let mut program = Program::new();
    let lowered = program.lower_clean(
        "main :: () { i := 0; while i < 3 { if i == 1 { break; } i = i + 1; continue; } }",
    );
    let body = lowered.body(&program.interner, "main");
    assert!(
        body.facts().stray_jumps.is_empty(),
        "both jumps are inside the loop"
    );
}

// ---------------------------------------------------------------------------
// Facts left for the next wave
// ---------------------------------------------------------------------------

#[test]
fn reading_an_uninitialised_local_records_an_undefined_read() {
    let mut program = Program::new();
    let lowered = program.lower_clean("main :: () -> s64 { c: s64 = ---; return c; }");
    let body = lowered.body(&program.interner, "main");
    assert_eq!(
        body.facts().undefined_reads.len(),
        1,
        "definite assignment is the diagnostic this fact feeds: {}",
        program.dump(&lowered)
    );
}

#[test]
fn an_initialised_local_records_no_undefined_read() {
    let mut program = Program::new();
    let lowered = program.lower_clean("main :: () -> s64 { c: s64 = 1; return c; }");
    let body = lowered.body(&program.interner, "main");
    assert!(body.facts().undefined_reads.is_empty());
}

// ---------------------------------------------------------------------------
// Refusals — ADR-0017 §4
// ---------------------------------------------------------------------------

#[test]
fn a_body_containing_run_is_refused_because_run_has_no_value_yet() {
    let mut program = Program::new();
    let lowered = program.lower_clean("add :: (a: s64, b: s64) -> s64 { return a + b; }\nmain :: () -> s64 { return #run add(1, 2); }");
    assert_eq!(
        lowered.refusal(&program.interner, "main"),
        Poisoned::Here("#run has no value until jr-vm (ADR-0016 §4)"),
        "ADR-0016 §4: lowering it as a runtime call would make comptime and runtime disagree"
    );
    // The procedure `#run` calls is itself perfectly lowerable.
    let _ = lowered.body(&program.interner, "add");
}

#[test]
fn a_body_referring_to_a_file_level_constant_is_refused() {
    let mut program = Program::new();
    // `jr-sema` records a constant's type but never its value: computing one needs
    // an evaluator, and the VM is the only evaluator there will be.
    let lowered = program.lower_clean("LIMIT :: 4096;\nmain :: () -> s64 { return LIMIT; }");
    assert_eq!(
        lowered.refusal(&program.interner, "main"),
        Poisoned::Here("a file-level item has no value until jr-vm")
    );
}

#[test]
fn a_body_whose_type_failed_to_resolve_is_refused_silently() {
    let mut program = Program::new();
    // An unresolved type name poisons to `PoolId::ERROR`, which is the signal the
    // gate can actually see.
    let lowered = program.lower("main :: () { x: NoSuchType = 1; }");
    assert!(
        !lowered.earlier_diagnostics.is_empty(),
        "sema must have reported the unknown type"
    );
    let refusal = lowered.refusal(&program.interner, "main");
    assert!(
        matches!(refusal, Poisoned::Here(_)),
        "a poisoned body is refused, never lowered from poison"
    );
}

#[test]
fn a_reported_error_that_does_not_poison_a_type_still_lowers() {
    let mut program = Program::new();
    // `x: u8 = 300;` is E0204. Sema reports it and then carries on with `u8`, so
    // nothing in the `TypeMap` is `PoolId::ERROR` and this crate's gate cannot see
    // it. That is a real hole and it is *not* closed here: `jr-mir` is a pure
    // function over HIR plus types, and it is handed no diagnostics to consult.
    //
    // Closing it is the caller's job — `jr-db` must not ask for MIR for a file
    // whose `file_diagnostics` reports errors. This test exists to pin the
    // division of responsibility, so that a future reader does not mistake the
    // behaviour for an oversight. See ADR-0017 §4.
    let lowered = program.lower("main :: () { x: u8 = 300; }");
    assert!(
        !lowered.earlier_diagnostics.is_empty(),
        "sema reported the out-of-range literal"
    );
    assert!(
        lowered.outcome(&program.interner, "main").is_ok(),
        "documented behaviour: an error sema did not poison is invisible here"
    );
}

#[test]
fn a_foreign_procedure_has_no_body_and_so_no_entry_at_all() {
    let mut program = Program::new();
    let lowered = program.lower_clean(
        "libc :: #system_library \"c\";\nwrite :: (fd: s64, buf: *u8, count: s64) -> s64 #foreign libc \"write\";",
    );
    let proc = lowered
        .proc_id(&program.interner, "write")
        .expect("the procedure exists");
    assert!(
        lowered.mir.get(proc).is_none(),
        "a bodiless procedure is not a failure, so it must not be recorded as one"
    );
    assert!(lowered.mir.is_empty());
}
