//! Positive statements about what DCE, const-prop and the pipeline do.
//!
//! `tests/inlining.rs` covers the splice. This file covers
//! [ADR-0022](../../../docs/adr/0022-dce-constprop-shared-arithmetic.md) §4, §5 and
//! §6 — and in particular the three refusals in §4, which are the only place in the
//! compiler where a pass can *delete* observable behaviour. Those are asserted here at
//! the MIR level and again in `crates/jr-cli/tests/differential.rs` as running
//! programs, because "the pass kept the statement" and "both engines still trap" are
//! different claims.

mod harness;

use harness::Program;
use jr_mir::{
    BinOp, Callees, MirBody, Operand, Rvalue, Statement, Terminator, const_prop, dce, is_pure,
    optimize,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn statements(body: &MirBody) -> usize {
    body.blocks().iter().map(|block| block.stmts.len()).sum()
}

fn has_rvalue(body: &MirBody, mut pred: impl FnMut(&Rvalue) -> bool) -> bool {
    body.blocks()
        .iter()
        .flat_map(|block| &block.stmts)
        .any(|stmt| match stmt {
            Statement::Assign { rvalue, .. } | Statement::Discard { rvalue, .. } => pred(rvalue),
            Statement::Store { .. } | Statement::Nop => false,
        })
}

fn has_call(body: &MirBody) -> bool {
    has_rvalue(body, |rvalue| matches!(rvalue, Rvalue::Call { .. }))
}

fn has_arithmetic(body: &MirBody) -> bool {
    has_rvalue(body, |rvalue| matches!(rvalue, Rvalue::Binary { .. }))
}

fn nops(body: &MirBody) -> usize {
    body.blocks()
        .iter()
        .flat_map(|block| &block.stmts)
        .filter(|stmt| matches!(stmt, Statement::Nop))
        .count()
}

// ---------------------------------------------------------------------------
// §4: what DCE may and may not delete
// ---------------------------------------------------------------------------

#[test]
fn a_dead_wrapping_operation_is_removed() {
    let mut program = Program::new();
    let lowered = program.lower_clean("f :: (a: s64) { b := a +% 1; }");
    let mut body = lowered.body(&program.interner, "f").clone();
    let before = statements(&body);
    assert!(dce(&mut body, &program.pool));
    assert!(
        statements(&body) < before,
        "`+%` cannot trap, so a result nobody reads is deletable"
    );
    assert!(!has_arithmetic(&body));
}

#[test]
fn a_dead_trapping_operation_is_kept() {
    // The rule that makes this pass dangerous. ADR-0002 says overflow always traps,
    // and `jr-codegen-clif` already commits at `body.rs:266` to a discarded rvalue
    // still being evaluated. Deleting this would delete a trap.
    let mut program = Program::new();
    let lowered = program.lower_clean("f :: (a: s64) { b := a + 1; }");
    let mut body = lowered.body(&program.interner, "f").clone();
    dce(&mut body, &program.pool);
    assert!(
        has_arithmetic(&body),
        "a trapping `+` whose result is unused must survive"
    );
}

#[test]
fn a_dead_call_is_kept() {
    // A call can do anything, up to and including `modules/Basic`'s `exit`.
    let mut program = Program::new();
    let lowered = program.lower_clean(
        "g :: () -> s64 { return 1; }\n\
         f :: () { x := g(); }\n",
    );
    let mut body = lowered.body(&program.interner, "f").clone();
    dce(&mut body, &program.pool);
    assert!(
        has_call(&body),
        "a call whose result is unused must survive"
    );
}

#[test]
fn a_dead_load_is_kept() {
    // A read through a dangling pointer faults, and `jr-vm`'s `Trap::BadAddress` docs
    // note that a pointer into a released frame is expressible in a valid program.
    let mut program = Program::new();
    let lowered = program.lower_clean("f :: (p: *s64) { x := p.*; }");
    let mut body = lowered.body(&program.interner, "f").clone();
    dce(&mut body, &program.pool);
    assert!(
        has_rvalue(&body, |rvalue| matches!(rvalue, Rvalue::Load(_))),
        "a load whose result is unused must survive"
    );
}

#[test]
fn the_purity_predicate_agrees_with_the_operators_trap_flag() {
    // Stated as a property rather than a table, so that adding an operator cannot
    // leave the two out of step. `can_trap` is what codegen and the VM key off.
    for op in [
        BinOp::Add,
        BinOp::Sub,
        BinOp::Mul,
        BinOp::Div,
        BinOp::Rem,
        BinOp::WrapAdd,
        BinOp::WrapSub,
        BinOp::WrapMul,
        BinOp::Eq,
        BinOp::Lt,
    ] {
        let rvalue = Rvalue::Binary {
            op,
            lhs: Operand::Constant(jr_pool::PoolId::TRUE),
            rhs: Operand::Constant(jr_pool::PoolId::TRUE),
        };
        assert_eq!(
            is_pure(&rvalue),
            !op.can_trap(),
            "{op:?} disagrees about whether it is deletable"
        );
    }
}

#[test]
fn a_spill_slot_nothing_reads_is_removed_with_its_store() {
    // The symptom `PLAN.md` §7 named, in miniature. `modules/Basic`'s `print_line`
    // spills its `string` parameter to a slot and then passes the *value* on, so the
    // slot is written and never read. Removing it needs the dead store to go first,
    // which is the correction ADR-0022 §4 records.
    let mut program = Program::new();
    let lowered = program.lower_clean(
        "k :: (s: string) { }\n\
         h :: (s: string) { k(s); }\n",
    );
    let mut body = lowered.body(&program.interner, "h").clone();
    assert!(
        body.slot_count() > 0,
        "the parameter must have been spilled"
    );
    assert!(dce(&mut body, &program.pool));
    assert_eq!(
        body.slot_count(),
        0,
        "a slot that is only ever written must not survive"
    );
}

#[test]
fn nops_do_not_survive() {
    let mut program = Program::new();
    let lowered = program.lower_clean("f :: (a: s64) { b := a +% 1; }");
    let mut body = lowered.body(&program.interner, "f").clone();
    dce(&mut body, &program.pool);
    assert_eq!(nops(&body), 0);
}

// ---------------------------------------------------------------------------
// §5: const-prop
// ---------------------------------------------------------------------------

#[test]
fn an_operation_on_two_constants_folds() {
    let mut program = Program::new();
    let lowered = program.lower_clean("f :: () -> s64 { return 2 + 3; }");
    let mut body = lowered.body(&program.interner, "f").clone();
    assert!(const_prop(&mut body, &mut program.pool));
    assert!(!has_arithmetic(&body), "`2 + 3` must have folded");
}

#[test]
fn an_operation_that_would_trap_is_not_folded() {
    // ADR-0022 §5. Folding it to a value would be a miscompile, and folding it to a
    // trap would silently decide a language question nothing has decided: whether
    // `MAX + 1` in never-executed code is a compile error.
    let mut program = Program::new();
    let lowered = program.lower_clean("f :: () -> s64 { return 9223372036854775807 + 1; }");
    let mut body = lowered.body(&program.interner, "f").clone();
    const_prop(&mut body, &mut program.pool);
    assert!(
        has_arithmetic(&body),
        "an overflowing fold must be left to trap at run time"
    );
}

#[test]
fn a_branch_on_a_constant_becomes_an_unconditional_edge() {
    let mut program = Program::new();
    let lowered = program.lower_clean("f :: () -> s64 { if false { return 1; } return 0; }");
    let mut body = lowered.body(&program.interner, "f").clone();
    assert!(const_prop(&mut body, &mut program.pool));
    assert!(
        !body
            .blocks()
            .iter()
            .any(|block| matches!(block.term, Terminator::Branch { .. })),
        "a branch on `false` must become a goto"
    );
}

#[test]
fn a_dead_arm_disappears_once_the_branch_is_folded() {
    // The handoff between the two passes: const-prop makes the block unreachable and
    // DCE is what removes it. Asserting the pair is the point — either alone leaves
    // the program bigger than it needs to be.
    let mut program = Program::new();
    let lowered = program.lower_clean("f :: () -> s64 { if false { return 1; } return 0; }");
    let mut body = lowered.body(&program.interner, "f").clone();
    let before = body.block_count();
    const_prop(&mut body, &mut program.pool);
    dce(&mut body, &program.pool);
    assert!(
        body.block_count() < before,
        "the arm that cannot run must be gone"
    );
}

// ---------------------------------------------------------------------------
// §6: the pipeline
// ---------------------------------------------------------------------------

#[test]
fn inlining_a_literal_call_folds_all_the_way_to_a_constant() {
    // The cascade ADR-0022 §5 justifies the block-parameter collapse with: the splice
    // turns `add(2, 3)` into an edge carrying two constants, the collapse turns the
    // callee's parameters into those constants, and the fold turns `2 + 3` into `5`.
    // No single one of the four transformations gets there alone.
    let mut program = Program::new();
    let lowered = program.lower_clean(
        "add :: (a: s64, b: s64) -> s64 { return a + b; }\n\
         caller :: () -> s64 { return add(2, 3); }\n",
    );
    let mut body = lowered.body(&program.interner, "caller").clone();
    let mut callees = Callees::new();
    callees.insert(lowered.body(&program.interner, "add"));

    let stats = optimize(&mut body, &callees, &mut program.pool);
    assert_eq!(stats.inlined, 1);
    assert!(
        !stats.exhausted,
        "this must converge, not run out of rounds"
    );
    assert!(!has_call(&body), "the call is inlined");
    assert!(
        !has_arithmetic(&body),
        "and the addition it inlined is folded away"
    );
}

#[test]
fn the_pipeline_converges_rather_than_exhausting_its_rounds() {
    // `exhausted` is reported so that "we ran out of rounds" is distinguishable from
    // "we converged" — the difference between a missed optimisation and a pass that
    // lies about having changed something.
    let mut program = Program::new();
    let lowered = program.lower_clean(
        "add :: (a: s64, b: s64) -> s64 { return a + b; }\n\
         caller :: () -> s64 { if add(1, 2) > 2 { return add(3, 4); } return 0; }\n",
    );
    let mut body = lowered.body(&program.interner, "caller").clone();
    let mut callees = Callees::new();
    callees.insert(lowered.body(&program.interner, "add"));

    let stats = optimize(&mut body, &callees, &mut program.pool);
    assert!(!stats.exhausted, "stats: {stats:?}");
    assert_eq!(stats.inlined, 2);
}

#[test]
fn optimising_a_body_with_nothing_to_do_changes_nothing() {
    let mut program = Program::new();
    let lowered = program.lower_clean("f :: (a: s64) -> s64 { return a; }");
    let before = lowered.body(&program.interner, "f").clone();
    let mut body = before.clone();
    let stats = optimize(&mut body, &Callees::new(), &mut program.pool);
    assert_eq!(stats.inlined, 0);
    assert_eq!(
        stats.rounds, 1,
        "one round to discover there is nothing to do"
    );
    assert_eq!(body, before);
}
