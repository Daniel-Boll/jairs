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
    Callees, MAX_INLINE_ROUNDS, MAX_INLINE_STATEMENTS, MAX_INLINED_STATEMENTS, MirBody, MirSpan,
    Operand, Rvalue, Statement, Terminator, inline_body, is_inlinable,
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
                | Statement::BoundsCheck { span, .. }
                | Statement::TagCheck { span, .. } => out.push(*span),
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
                | Statement::TagCheck { .. }
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
fn a_recursive_callee_is_refused_so_its_backtrace_survives() {
    // **The reason changed even though the answer did not** (ADR-0145 §1). ADR-0021 §4
    // refused a recursive callee as a side effect of the leaf rule, whose purpose was
    // termination. The leaf rule is gone, so this is now its own condition — and building it
    // found that the *better* argument is not termination at all:
    //
    // An inlined callee has no frame (ADR-0021 §3), and ADR-0066 §4 defers
    // inline-provenance backtraces, so every flattened frame is permanently missing from a
    // diagnostic. In a recursive trap the *depth* is the message: four `countdown` frames
    // reported as one would be a backtrace that lies about what happened. So the case where
    // flattening costs the most is exactly the case whose benefit was never measured, and
    // `differential.rs` pins the four-frame chain that depends on this.
    let mut program = Program::new();
    let lowered = program.lower_clean(
        "down :: (a: s64) -> s64 { if a > 0 { return down(a - 1); } return 0; }\n\
         caller :: () -> s64 { return down(3); }\n",
    );
    let down = lowered.body(&program.interner, "down");
    let down_ref = lowered.proc_ref(&program.interner, "down");
    let mut callees = Callees::new();
    callees.insert(down);
    assert!(
        !is_inlinable(down_ref, down, &callees),
        "a callee that reaches itself must be refused"
    );

    let mut caller = lowered.body(&program.interner, "caller").clone();
    assert_eq!(inline_body(&mut caller, &callees, &program.pool), 0);
    assert_eq!(calls(&caller), 1, "the call must survive untouched");
}

#[test]
fn a_mutually_recursive_pair_is_refused_through_the_cycle() {
    // The case a *self*-call check would miss and this one catches: neither procedure calls
    // itself directly, so only walking the available bodies finds the cycle. Asserted because
    // the cheap check was the tempting one, and it would have flattened these frames while
    // reporting the direct case correctly — an inconsistency no reader could predict.
    let mut program = Program::new();
    let lowered = program.lower_clean(
        "ping :: (a: s64) -> s64 { if a > 0 { return pong(a - 1); } return 0; }\n\
         pong :: (a: s64) -> s64 { if a > 0 { return ping(a - 1); } return 1; }\n\
         caller :: () -> s64 { return ping(3); }\n",
    );
    let mut callees = Callees::new();
    callees.insert(lowered.body(&program.interner, "ping"));
    callees.insert(lowered.body(&program.interner, "pong"));
    assert!(!is_inlinable(
        lowered.proc_ref(&program.interner, "ping"),
        lowered.body(&program.interner, "ping"),
        &callees
    ));
    assert!(!is_inlinable(
        lowered.proc_ref(&program.interner, "pong"),
        lowered.body(&program.interner, "pong"),
        &callees
    ));

    let mut caller = lowered.body(&program.interner, "caller").clone();
    assert_eq!(inline_body(&mut caller, &callees, &program.pool), 0);
}

#[test]
fn a_non_leaf_callee_is_inlined_and_the_chain_collapses() {
    // The shape the leaf rule refused, and the reason ADR-0145 exists: a standard library is
    // full of `sort_ints` → `sort` → `less_int`, and the middle procedure stopped the chain
    // for every caller above it. Both levels must go.
    let mut program = Program::new();
    let lowered = program.lower_clean(
        "add :: (a: s64, b: s64) -> s64 { return a + b; }\n\
         twice :: (a: s64) -> s64 { return add(a, a); }\n\
         caller :: () -> s64 { return twice(21); }\n",
    );
    let mut callees = Callees::new();
    callees.insert(lowered.body(&program.interner, "add"));
    callees.insert(lowered.body(&program.interner, "twice"));
    assert!(is_inlinable(
        lowered.proc_ref(&program.interner, "add"),
        lowered.body(&program.interner, "add"),
        &callees
    ));
    assert!(
        is_inlinable(
            lowered.proc_ref(&program.interner, "twice"),
            lowered.body(&program.interner, "twice"),
            &callees
        ),
        "a callee containing a call is eligible now"
    );

    let mut caller = lowered.body(&program.interner, "caller").clone();
    assert!(inline_body(&mut caller, &callees, &program.pool) >= 2);
    assert_eq!(
        calls(&caller),
        0,
        "both levels must collapse, which is what one round could not do"
    );
}

#[test]
fn the_round_count_bounds_the_nesting_depth() {
    // What `MAX_INLINE_ROUNDS` bounds, pinned so the number can be tuned without the
    // property changing silently (ADR-0145 §1). A chain of wrappers deeper than the round
    // count keeps a real call, because each round inlines exactly one more level: a splice
    // copies the callee's own calls in, and those are not visited until the next round.
    //
    // Asserted from *both* sides, because an off-by-one in either direction is invisible
    // otherwise: a chain at the limit must collapse completely, and one past it must not.
    let mut program = Program::new();
    let mut source = String::from("leaf :: (a: s64) -> s64 { return a + 1; }\n");
    // `w1` calls `leaf`, `w2` calls `w1`, and so on: `wN` is N + 1 levels deep.
    source.push_str("w1 :: (a: s64) -> s64 { return leaf(a); }\n");
    for level in 2..=(MAX_INLINE_ROUNDS + 2) {
        source.push_str(&format!(
            "w{level} :: (a: s64) -> s64 {{ return w{}(a); }}\n",
            level - 1
        ));
    }
    let lowered = program.lower_clean(&source);

    let mut callees = Callees::new();
    callees.insert(lowered.body(&program.interner, "leaf"));
    for level in 1..=(MAX_INLINE_ROUNDS + 2) {
        callees.insert(lowered.body(&program.interner, &format!("w{level}")));
    }

    // At the limit: `w{ROUNDS - 1}` is ROUNDS levels of call, so every one goes.
    let mut at_limit = lowered
        .body(&program.interner, &format!("w{}", MAX_INLINE_ROUNDS - 1))
        .clone();
    inline_body(&mut at_limit, &callees, &program.pool);
    assert_eq!(
        calls(&at_limit),
        0,
        "a chain within the round budget must collapse completely"
    );

    // Past it: one call survives, which is correct rather than a failure — the program still
    // computes the same thing, it is merely less flattened.
    let mut past_limit = lowered
        .body(&program.interner, &format!("w{}", MAX_INLINE_ROUNDS + 2))
        .clone();
    inline_body(&mut past_limit, &callees, &program.pool);
    assert!(
        calls(&past_limit) > 0,
        "a chain deeper than the round budget must keep a real call"
    );
}

#[test]
fn a_caller_over_the_total_budget_stops_absorbing_splices() {
    // `MAX_INLINED_STATEMENTS` (ADR-0145 §1): every individual callee may be under
    // `MAX_INLINE_STATEMENTS` and a fan-out of them still explode one body. The leaf rule
    // used to make that unlikely by refusing most callees; nothing does now, so the budget
    // has to.
    //
    // The assertion is on the *statement count* rather than on the splice count, because the
    // budget's job is to bound the body and not to bound the pass.
    let mut program = Program::new();
    let mut source = String::from("chunk :: (a: s64) -> s64 {\n    n := a;\n");
    for _ in 0..(MAX_INLINE_STATEMENTS - 4) {
        source.push_str("    n = n + 1;\n");
    }
    source.push_str("    return n;\n}\n\ncaller :: () -> s64 {\n    t := 0;\n");
    for _ in 0..40 {
        source.push_str("    t = t + chunk(t);\n");
    }
    source.push_str("    return t;\n}\n");
    let lowered = program.lower_clean(&source);
    let chunk = lowered.body(&program.interner, "chunk");
    let mut callees = Callees::new();
    callees.insert(chunk);
    assert!(
        is_inlinable(
            lowered.proc_ref(&program.interner, "chunk"),
            chunk,
            &callees
        ),
        "each callee is individually eligible"
    );

    let mut caller = lowered.body(&program.interner, "caller").clone();
    inline_body(&mut caller, &callees, &program.pool);
    let statements: usize = caller
        .blocks()
        .iter()
        .map(|block| {
            block
                .stmts
                .iter()
                .filter(|s| !matches!(s, Statement::Nop))
                .count()
        })
        .sum();
    assert!(
        statements < MAX_INLINED_STATEMENTS + MAX_INLINE_STATEMENTS,
        "the budget must stop the fan-out; the body reached {statements} statements"
    );
    assert!(
        calls(&caller) > 0,
        "and it must stop by *refusing* splices, leaving real calls behind"
    );
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
    let callees = Callees::new();
    assert!(
        !is_inlinable(
            lowered.proc_ref(&program.interner, "big"),
            lowered.body(&program.interner, "big"),
            &callees
        ),
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
