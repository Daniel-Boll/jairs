//! Positive statements about what the inliner does to a body.
//!
//! The corpus and the differential harness constrain the inliner only through its
//! *effects*: a program must still behave the same and a trap must still name a
//! line. Neither says that the call statement became a `Nop`, that the result
//! travels as a block parameter rather than through a copy, or that a recursive
//! procedure was left alone — and every one of those is a silent failure if it
//! regresses, because the MIR stays well-formed and merely does something else.
//! ADR-0021 is the specification; this file is where its §3 and §4 are asserted.
//!
//! The `jr-mir` harness has no VM and no module loader, so these tests inline
//! within one file. That is not a limitation of the pass — [`Callees`] is keyed by
//! [`jr_mir::ProcRef`] precisely so a cross-file callee works — and
//! `crates/jr-db/tests/optimized_mir.rs` is where the cross-file half is asserted,
//! because that needs the query that reads another file's MIR.

mod harness;

use harness::Program;
use jr_mir::{
    Callees, MAX_INLINE_STATEMENTS, MirBody, MirSpan, Operand, Rvalue, Statement, Terminator,
    inline_body, is_inlinable,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Every statement span in a body, in block then statement order.
fn spans(body: &MirBody) -> Vec<MirSpan> {
    let mut out = Vec::new();
    for block in body.blocks() {
        for stmt in &block.stmts {
            match stmt {
                Statement::Assign { span, .. }
                | Statement::Store { span, .. }
                | Statement::Discard { span, .. }
                | Statement::Zero { span, .. }
                | Statement::BoundsCheck { span, .. } => out.push(*span),
                Statement::Nop => {}
            }
        }
    }
    out
}

/// How many call rvalues a body still performs.
fn calls(body: &MirBody) -> usize {
    let mut count = 0;
    for block in body.blocks() {
        for stmt in &block.stmts {
            let rvalue = match stmt {
                Statement::Assign { rvalue, .. } | Statement::Discard { rvalue, .. } => rvalue,
                Statement::Store { .. }
                | Statement::Zero { .. }
                | Statement::BoundsCheck { .. }
                | Statement::Nop => continue,
            };
            if matches!(rvalue, Rvalue::Call { .. }) {
                count += 1;
            }
        }
    }
    count
}

fn nops(body: &MirBody) -> usize {
    body.blocks()
        .iter()
        .flat_map(|block| &block.stmts)
        .filter(|stmt| matches!(stmt, Statement::Nop))
        .count()
}

/// Whether `value` is a parameter of some block.
fn is_a_block_param(body: &MirBody, value: jr_mir::ValueId) -> bool {
    body.blocks()
        .iter()
        .any(|block| block.params.contains(&value))
}

// ---------------------------------------------------------------------------
// The splice
// ---------------------------------------------------------------------------

const ADD_AND_CALLER: &str = "add :: (a: s64, b: s64) -> s64 { return a + b; }\n\
                              caller :: () -> s64 { return add(2, 3); }\n";

#[test]
fn a_leaf_call_is_replaced_by_the_callees_body() {
    let mut program = Program::new();
    let lowered = program.lower_clean(ADD_AND_CALLER);
    let mut caller = lowered.body(&program.interner, "caller").clone();
    assert_eq!(calls(&caller), 1, "the call is there before inlining");

    let mut callees = Callees::new();
    callees.insert(lowered.body(&program.interner, "add"));
    let spliced = inline_body(&mut caller, &callees, &program.pool);

    assert_eq!(spliced, 1);
    assert_eq!(
        calls(&caller),
        0,
        "the call rvalue must be gone, not merely accompanied by a copy of the body"
    );
}

#[test]
fn the_call_statement_becomes_a_nop_rather_than_being_removed() {
    // ADR-0017 §1 declared `Statement::Nop` for a pass that wanted to delete a
    // statement without shifting every later index in its block. This is that pass
    // and its first producer, so the variant is now reachable in both engines'
    // lowering — which is why both already handle it.
    let mut program = Program::new();
    let lowered = program.lower_clean(ADD_AND_CALLER);
    let mut caller = lowered.body(&program.interner, "caller").clone();
    let mut callees = Callees::new();
    callees.insert(lowered.body(&program.interner, "add"));

    assert_eq!(nops(&caller), 0);
    inline_body(&mut caller, &callees, &program.pool);
    assert_eq!(nops(&caller), 1);
}

#[test]
fn the_calls_result_becomes_a_block_parameter_rather_than_a_copy() {
    // The point of ADR-0021's continuation-parameter choice: `x` keeps its identity,
    // so every later use of it is untouched and there is no copy for a
    // copy-propagation pass to remove — which matters because there is no such pass.
    let mut program = Program::new();
    let lowered = program.lower_clean(ADD_AND_CALLER);
    let caller_before = lowered.body(&program.interner, "caller");

    // The value the call defined, found before the splice removes the statement.
    let dest = caller_before
        .blocks()
        .iter()
        .flat_map(|block| &block.stmts)
        .find_map(|stmt| match stmt {
            Statement::Assign {
                dest,
                rvalue: Rvalue::Call { .. },
                ..
            } => Some(*dest),
            _ => None,
        })
        .expect("the caller assigns the call's result");
    assert!(!is_a_block_param(caller_before, dest));

    let mut caller = caller_before.clone();
    let copied_entry = jr_mir::BlockId::from_usize(caller.block_count() + 1);
    let mut callees = Callees::new();
    callees.insert(lowered.body(&program.interner, "add"));
    inline_body(&mut caller, &callees, &program.pool);

    assert!(
        is_a_block_param(&caller, dest),
        "the destination must be the continuation block's parameter"
    );
    // The splice allocates the continuation first and the copied blocks after it, so
    // the copied entry is the second appended block. Asserted rather than searched
    // for, because the arguments are what matters: `add(2, 3)`'s two constants must
    // travel as edge arguments to the copied entry's parameters, which are the
    // callee's own parameters (ADR-0017 §1's block parameters doing the work).
    let Terminator::Goto(target) = caller.block(caller.entry()).term.clone() else {
        panic!("the call's block must end in an unconditional edge into the callee");
    };
    assert_eq!(target.block, copied_entry);
    // **Three arguments now, not two**: the implicit context leads (ADR-0057 §4), then `add(2, 3)`'s
    // two constants. Both procedures are ordinary Jairs ones, so both take a context — this is the
    // ABI change, and the test moved with it rather than being weakened.
    assert_eq!(
        target.args.len(),
        3,
        "the context plus two arguments, matching three entry parameters"
    );
    assert!(
        matches!(target.args[0], Operand::Value(_)),
        "the leading argument is the caller's context, a value not a constant"
    );
    assert!(
        target.args[1..]
            .iter()
            .all(|arg| matches!(arg, Operand::Constant(_))),
        "`add(2, 3)` passes two constants after the context"
    );
    assert_eq!(caller.block(copied_entry).params.len(), 3);
}

#[test]
fn every_span_in_an_inlined_body_is_one_the_caller_already_had() {
    // ADR-0021 §3. A `MirSpan` names an `ExprId` in the *callee's* arena, and
    // `resolve_span` is handed the caller's `FileHir`, so a survivor resolves to a
    // plausible wrong line rather than to nothing. Asserting subset-of-before is the
    // strongest form available here: every copied span collapses to the call's, and
    // the call's span was in the body already.
    let mut program = Program::new();
    let lowered = program.lower_clean(ADD_AND_CALLER);
    let before = spans(lowered.body(&program.interner, "caller"));

    let mut caller = lowered.body(&program.interner, "caller").clone();
    let mut callees = Callees::new();
    callees.insert(lowered.body(&program.interner, "add"));
    inline_body(&mut caller, &callees, &program.pool);

    for span in spans(&caller) {
        assert!(
            before.contains(&span),
            "{span:?} came from the callee's arenas and would resolve against the wrong file"
        );
    }
}

#[test]
fn two_calls_in_one_block_are_both_inlined() {
    // The continuation carries whatever followed the call, so the second call is only
    // reached if the pass revisits the blocks it appended. A pass that iterated the
    // original block list once would silently inline the first call and leave the
    // second, which is a performance bug with no test to catch it.
    let mut program = Program::new();
    let lowered = program.lower_clean(
        "add :: (a: s64, b: s64) -> s64 { return a + b; }\n\
         caller :: () -> s64 { return add(1, 2) + add(3, 4); }\n",
    );
    let mut caller = lowered.body(&program.interner, "caller").clone();
    assert_eq!(calls(&caller), 2);

    let mut callees = Callees::new();
    callees.insert(lowered.body(&program.interner, "add"));
    let spliced = inline_body(&mut caller, &callees, &program.pool);

    assert_eq!(spliced, 2);
    assert_eq!(calls(&caller), 0);
}

#[test]
fn a_void_call_in_statement_position_needs_no_continuation_parameter() {
    let mut program = Program::new();
    let lowered = program.lower_clean(
        "poke :: (p: *s64) { p.* = 1; }\n\
         caller :: () { n := 0; q := *n; poke(q); }\n",
    );
    let mut caller = lowered.body(&program.interner, "caller").clone();
    let cont = jr_mir::BlockId::from_usize(caller.block_count());
    let mut callees = Callees::new();
    callees.insert(lowered.body(&program.interner, "poke"));

    assert_eq!(inline_body(&mut caller, &callees, &program.pool), 1);
    assert_eq!(calls(&caller), 0);
    // The continuation is the first block the splice appends. The verifier inside
    // `inline_body` would already have caught an arity mismatch; this states the
    // intent, which is that a discarded result creates no parameter at all rather
    // than a parameter nothing reads. The *copied* blocks keep their own parameters,
    // because those are the callee's `p`.
    assert!(
        caller.block(cont).params.is_empty(),
        "a void call's continuation must take no argument"
    );
}

#[test]
fn a_callee_with_control_flow_is_inlined_with_its_blocks() {
    let mut program = Program::new();
    let lowered = program.lower_clean(
        "clamp :: (a: s64) -> s64 { if a > 10 { return 10; } return a; }\n\
         caller :: () -> s64 { return clamp(42); }\n",
    );
    let callee_blocks = lowered.body(&program.interner, "clamp").block_count();
    assert!(callee_blocks > 1, "the callee must actually branch");

    let mut caller = lowered.body(&program.interner, "caller").clone();
    let before = caller.block_count();
    let mut callees = Callees::new();
    callees.insert(lowered.body(&program.interner, "clamp"));
    assert_eq!(inline_body(&mut caller, &callees, &program.pool), 1);

    assert_eq!(
        caller.block_count(),
        before + callee_blocks + 1,
        "every callee block plus one continuation"
    );
    assert_eq!(calls(&caller), 0);
}

// ---------------------------------------------------------------------------
// Eligibility (ADR-0021 §4)
// ---------------------------------------------------------------------------

#[test]
fn a_recursive_procedure_is_not_a_leaf_and_so_is_never_inlined() {
    // The whole termination argument, asserted rather than reasoned about: there is
    // no depth counter in the pass, so if a recursive callee were ever eligible the
    // splice would not terminate.
    let mut program = Program::new();
    let lowered = program.lower_clean(
        "down :: (a: s64) -> s64 { if a > 0 { return down(a - 1); } return 0; }\n\
         caller :: () -> s64 { return down(3); }\n",
    );
    let down = lowered.body(&program.interner, "down");
    assert!(!is_inlinable(down));

    let mut caller = lowered.body(&program.interner, "caller").clone();
    let mut callees = Callees::new();
    callees.insert(down);
    assert_eq!(inline_body(&mut caller, &callees, &program.pool), 0);
    assert_eq!(calls(&caller), 1, "the call must survive untouched");
}

#[test]
fn a_callee_that_calls_something_is_not_inlinable() {
    let mut program = Program::new();
    let lowered = program.lower_clean(
        "add :: (a: s64, b: s64) -> s64 { return a + b; }\n\
         twice :: (a: s64) -> s64 { return add(a, a); }\n",
    );
    assert!(is_inlinable(lowered.body(&program.interner, "add")));
    assert!(!is_inlinable(lowered.body(&program.interner, "twice")));
}

#[test]
fn a_callee_over_the_statement_threshold_is_not_inlinable() {
    let mut program = Program::new();
    let mut source = String::from("big :: () -> s64 {\n    n := 0;\n");
    for _ in 0..MAX_INLINE_STATEMENTS {
        source.push_str("    n = n + 1;\n");
    }
    source.push_str("    return n;\n}\n");
    let lowered = program.lower_clean(&source);
    assert!(
        !is_inlinable(lowered.body(&program.interner, "big")),
        "a body of {MAX_INLINE_STATEMENTS} assignments or more must be refused"
    );
}

#[test]
fn an_unavailable_callee_leaves_the_call_alone() {
    // The behaviour a gated or refused callee's file must produce: no body to copy is
    // not an error, it is simply no inlining. `Callees::new()` is the same case.
    let mut program = Program::new();
    let lowered = program.lower_clean(ADD_AND_CALLER);
    let before = lowered.body(&program.interner, "caller").clone();
    let mut caller = before.clone();

    assert_eq!(
        inline_body(&mut caller, &Callees::new(), &program.pool),
        0,
        "an empty callee set must splice nothing"
    );
    assert_eq!(caller, before, "and must leave the body byte-identical");
}
