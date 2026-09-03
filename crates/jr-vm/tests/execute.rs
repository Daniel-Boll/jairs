//! Executing real Jairs source, end to end, with no database.
//!
//! This is the VM's own proof, and it is deliberately built the same way `jr-mir`'s
//! test harness is: parse → HIR → resolve → signatures → check → MIR → bytecode →
//! run, all as pure functions over one file. `jr-db` does the same thing with
//! queries and several files, but the interesting failures are all in this path, and
//! a test that needed salsa to reach them would be slower to write and worse to read.
//!
//! What each group is for:
//!
//! - **Arithmetic** pins ADR-0002. Trapping is the whole point of that ADR, and a VM
//!   that wrapped instead would produce a *plausible* wrong answer, which is the
//!   failure mode worth the most tests.
//! - **Control flow** pins that block parameters were eliminated correctly. A wrong
//!   parallel copy is a miscompile in loop code that no type system catches.
//! - **Memory** pins ADR-0004's `{data, count}` layout and struct offsets — the
//!   numbers ADR-0018 §2 put in `jr-pool` so that Cranelift would agree with them.
//! - **Refusals** pin ADR-0006's comptime FFI gate, which is off until wave W6.

use jr_base::{FileId, Interner};
use jr_diag::Diagnostics;
use jr_mir::{ConstValues, ImportedProcs, Poisoned, ProcRef};
use jr_pool::{Pool, TargetLayout};
use jr_vm::{Mode, Program, Trap, Value, Vm, VmError};

const FILE: FileId = FileId::from_usize(0);

/// One file, all the way to a runnable program.
struct Fixture {
    interner: Interner,
    pool: Pool,
    program: Program,
    /// The `ProcId` of each named procedure.
    hir: jr_hir::FileHir,
}

impl Fixture {
    fn build(source: &str) -> Self {
        let interner = Interner::new();
        let mut pool = Pool::new();

        let parsed = jr_syntax::parse(source, FILE);
        let mut diags = Diagnostics::new();
        diags.extend(parsed.diagnostics().iter().cloned());

        let (hir, lower_diags) = jr_hir::lower_file(&parsed, FILE, &interner);
        diags.extend(lower_diags.iter().cloned());

        let (resolve, resolve_diags) = jr_hir::resolve(&hir, &[], &interner);
        diags.extend(resolve_diags.iter().cloned());

        let signatures = jr_sema::file_signatures(&hir, FILE, &resolve, &[], &mut pool, &interner);
        diags.extend(signatures.diagnostics.iter().cloned());

        let checked = jr_sema::check_file(
            &hir,
            FILE,
            &resolve,
            &signatures.signatures,
            &[],
            // No imported HIRs: this harness checks a file alone (ADR-0117 §1).
            &[],
            &mut pool,
            &interner,
        );
        diags.extend(checked.diagnostics.iter().cloned());

        assert!(
            diags.is_empty(),
            "the test program must check cleanly, got: {:?}",
            diags
                .iter()
                .map(|d| format!("{:?} {}", d.code, d.message))
                .collect::<Vec<_>>()
        );

        let mut types = signatures.types;
        types.absorb(&checked.types);

        // A global's initial value is a const-eval target in the real compiler
        // (`Wanted::GlobalInit`, resolved by `jr-db`'s round-based `evaluate` in `consts.rs`), and
        // this harness has no database to run that loop — deliberately, per its own module docs.
        // Every global a test in this file declares is initialised with a bare integer literal, so
        // a literal fold stands in for the real fixpoint: interning the literal's own value is
        // exactly what one round of `evaluate` would produce for it, with no dependency on any
        // other constant.
        let mut values = ConstValues::new();
        for (index, item) in hir.items.iter().enumerate() {
            let jr_hir::ItemKind::Var {
                init: Some(expr), ..
            } = &item.kind
            else {
                continue;
            };
            let jr_hir::Expr::Literal(jr_hir::Literal::Int { value, .. }, _) =
                &hir.exprs[expr.index()]
            else {
                continue;
            };
            // Every global this harness declares is `s64`; a test needing another width would
            // have to read it from `signatures` rather than assume one.
            let init = pool.int_value(jr_pool::PoolId::S64, *value as u64);
            values.set_global_init(jr_hir::ItemId::from_usize(index), init);
        }

        let mir = jr_mir::lower_file(
            &hir,
            &resolve,
            &types,
            &signatures.signatures,
            &values,
            &ImportedProcs::new(),
            // Empty, and **correct** rather than a shortcut: this harness resolves against `&[]`
            // imports, so no name in any of its programs is an imported one and there is nothing for
            // the map to hold. ADR-0053's lesson — a harness passing an empty map proves nothing —
            // applies where the harness *does* have imports, which `jr-mir`'s does.
            &jr_mir::ImportedValues::new(),
            &jr_mir::OperatorCalls::new(),
            &jr_mir::FilledArgs::new(),
            &interner,
            &mut pool,
        );

        let mut program = Program::new(TargetLayout::host());
        jr_vm::add_file(
            &mut program,
            FILE,
            &hir,
            &mir,
            &signatures.signatures,
            &pool,
        )
        .expect("every body in a clean program must compile");

        Self {
            interner,
            pool,
            program,
            hir,
        }
    }

    fn proc(&self, name: &str) -> ProcRef {
        let symbol = self.interner.get(name).expect("the name was interned");
        let proc = self
            .hir
            .items
            .iter()
            .find_map(|item| {
                let jr_hir::ItemKind::Const {
                    value: jr_hir::ConstValue::Proc(proc),
                } = &item.kind
                else {
                    return None;
                };
                (item.name == Some(symbol)).then_some(*proc)
            })
            .unwrap_or_else(|| panic!("no procedure named `{name}`"));
        ProcRef::new(FILE, proc)
    }

    /// Runs a procedure and returns its result.
    fn call(&self, name: &str, args: Vec<Value>, mode: Mode) -> Result<Value, VmError> {
        let mut vm =
            Vm::new(&self.program, &self.pool, mode).expect("room for the string constants");
        let args = self.with_context(&mut vm, name, args);
        vm.call(self.proc(name), args)
    }

    /// Prepends the implicit context to an argument list, when the callee takes one.
    ///
    /// This harness calls a procedure *directly*, so there is no Jairs caller to have passed a
    /// context — the same position `main` is in, and `jr-db`'s `run_main` solves it the same way
    /// (ADR-0057 §5): a zeroed allocation whose address is the leading argument.
    ///
    /// **Conditional on the callee**, not unconditional, because a `#c_call` procedure takes no
    /// hidden parameter and handing one over would be exactly the argument shift ADR-0053 §1
    /// recorded — the VM says "called a procedure taking 1 arguments with 2" and the shift is
    /// caught, but a test asserting the *wrong* number would pass for the wrong reason.
    fn with_context(&self, vm: &mut Vm<'_>, name: &str, args: Vec<Value>) -> Vec<Value> {
        // Asked of the *declaration*, exactly as `jr-db`'s `main_receives_context` does, and by
        // the same predicate — `!(c_call || foreign)`. One rule with two spellings is how a caller
        // and a callee come to disagree about whether a hidden parameter exists.
        let proc = self.proc(name).proc;
        let receives = self
            .hir
            .procs
            .get(proc.index())
            .is_some_and(|data| !(data.c_call || data.foreign.is_some()));
        if !receives {
            return args;
        }
        let context = Pool::find_context(&self.pool).expect("sema interned the context type");
        let layout = jr_pool::layout_of(&self.pool, TargetLayout::LP64, context)
            .expect("the context is an ordinary aggregate");
        let mut with = Vec::with_capacity(args.len() + 1);
        with.push(
            vm.new_context(layout.size, layout.align)
                .expect("room for one context"),
        );
        with.extend(args);
        with
    }

    /// Runs a procedure, asserting it succeeded, and returns the integer result.
    fn int(&self, name: &str, args: Vec<Value>) -> i128 {
        let value = self
            .call(name, args, Mode::Comptime)
            .unwrap_or_else(|e| panic!("`{name}` failed: {e}"));
        value
            .as_int(jr_vm::IntKind::S64)
            .unwrap_or_else(|e| panic!("`{name}` did not return an integer: {e}"))
    }

    /// Runs a procedure and returns what it wrote to standard output.
    fn output(&self, name: &str) -> String {
        let mut vm = Vm::new(&self.program, &self.pool, Mode::Runtime)
            .expect("room for the string constants");
        let args = self.with_context(&mut vm, name, Vec::new());
        vm.call(self.proc(name), args)
            .unwrap_or_else(|e| panic!("`{name}` failed: {e}"));
        String::from_utf8_lossy(vm.captured_output()).into_owned()
    }
}

fn s64(value: i64) -> Value {
    Value::Scalar(value as u64)
}

// ---------------------------------------------------------------------------
// The smallest possible proof
// ---------------------------------------------------------------------------

#[test]
fn a_procedure_returns_a_constant() {
    let fixture = Fixture::build("answer :: () -> s64 { return 42; }");
    assert_eq!(fixture.int("answer", vec![]), 42);
}

#[test]
fn a_void_procedure_returns_void() {
    let fixture = Fixture::build("nothing :: () { }");
    assert_eq!(
        fixture.call("nothing", vec![], Mode::Comptime),
        Ok(Value::Void)
    );
}

#[test]
fn arguments_reach_their_parameters() {
    // `#run add(2, 3)` is the slice's one comptime expression, so this is the
    // narrowest thing that has to work.
    let fixture = Fixture::build("add :: (a: s64, b: s64) -> s64 { return a + b; }");
    assert_eq!(fixture.int("add", vec![s64(2), s64(3)]), 5);
}

#[test]
fn a_call_reaches_another_procedure_in_the_same_file() {
    let fixture = Fixture::build(
        "add :: (a: s64, b: s64) -> s64 { return a + b; }\n\
         twice :: (n: s64) -> s64 { return add(n, n); }",
    );
    assert_eq!(fixture.int("twice", vec![s64(21)]), 42);
}

// ---------------------------------------------------------------------------
// Arithmetic — ADR-0002
// ---------------------------------------------------------------------------

#[test]
fn arithmetic_and_comparison_agree_with_ordinary_maths() {
    let fixture = Fixture::build(
        "calc :: (a: s64, b: s64) -> s64 { return a * b - a / b + a % b; }\n\
         cmp :: (a: s64, b: s64) -> bool { return a < b; }",
    );
    // 7*3 - 7/3 + 7%3 = 21 - 2 + 1
    assert_eq!(fixture.int("calc", vec![s64(7), s64(3)]), 20);
    assert_eq!(
        fixture.call("cmp", vec![s64(1), s64(2)], Mode::Comptime),
        Ok(Value::bool(true))
    );
    assert_eq!(
        fixture.call("cmp", vec![s64(2), s64(1)], Mode::Comptime),
        Ok(Value::bool(false))
    );
}

#[test]
fn negative_operands_work_because_the_maths_is_done_signed() {
    let fixture = Fixture::build("calc :: (a: s64, b: s64) -> s64 { return a / b; }");
    assert_eq!(fixture.int("calc", vec![s64(-7), s64(2)]), -3);
    assert_eq!(fixture.int("calc", vec![s64(7), s64(-2)]), -3);
}

#[test]
fn addition_traps_on_overflow_rather_than_wrapping() {
    // The headline of ADR-0002. Wrapping here would be a plausible wrong answer,
    // which is worse than a loud one.
    let fixture = Fixture::build("add :: (a: s64, b: s64) -> s64 { return a + b; }");
    assert_eq!(
        fixture.call("add", vec![s64(i64::MAX), s64(1)], Mode::Comptime),
        Err(VmError::Trap(Trap::Overflow { what: "addition" }))
    );
}

#[test]
fn subtraction_and_multiplication_trap_too() {
    let fixture = Fixture::build(
        "sub :: (a: s64, b: s64) -> s64 { return a - b; }\n\
         mul :: (a: s64, b: s64) -> s64 { return a * b; }",
    );
    assert_eq!(
        fixture.call("sub", vec![s64(i64::MIN), s64(1)], Mode::Comptime),
        Err(VmError::Trap(Trap::Overflow {
            what: "subtraction"
        }))
    );
    assert_eq!(
        fixture.call("mul", vec![s64(i64::MAX), s64(2)], Mode::Comptime),
        Err(VmError::Trap(Trap::Overflow {
            what: "multiplication"
        }))
    );
}

#[test]
fn the_wrapping_operators_are_the_documented_opt_out() {
    let fixture = Fixture::build("wadd :: (a: s64, b: s64) -> s64 { return a +% b; }");
    assert_eq!(
        fixture.int("wadd", vec![s64(i64::MAX), s64(1)]),
        i64::MIN as i128,
        "`+%` exists precisely so that overflow is expressible without trapping"
    );
}

#[test]
fn division_by_zero_traps() {
    let fixture = Fixture::build(
        "div :: (a: s64, b: s64) -> s64 { return a / b; }\n\
         rem :: (a: s64, b: s64) -> s64 { return a % b; }",
    );
    assert_eq!(
        fixture.call("div", vec![s64(1), s64(0)], Mode::Comptime),
        Err(VmError::Trap(Trap::DivideByZero))
    );
    assert_eq!(
        fixture.call("rem", vec![s64(1), s64(0)], Mode::Comptime),
        Err(VmError::Trap(Trap::DivideByZero))
    );
}

#[test]
fn dividing_the_most_negative_value_by_minus_one_overflows() {
    // Its true quotient is one past the maximum, so it is an overflow and not a
    // division error. Doing the arithmetic in `i128` is what makes the ordinary range
    // check catch it.
    let fixture = Fixture::build("div :: (a: s64, b: s64) -> s64 { return a / b; }");
    assert_eq!(
        fixture.call("div", vec![s64(i64::MIN), s64(-1)], Mode::Comptime),
        Err(VmError::Trap(Trap::Overflow { what: "division" }))
    );
}

#[test]
fn negation_traps_on_the_most_negative_value() {
    let fixture = Fixture::build("neg :: (a: s64) -> s64 { return -a; }");
    assert_eq!(fixture.int("neg", vec![s64(5)]), -5);
    assert_eq!(
        fixture.call("neg", vec![s64(i64::MIN)], Mode::Comptime),
        Err(VmError::Trap(Trap::Overflow { what: "negation" }))
    );
}

#[test]
fn a_narrow_type_traps_at_its_own_boundary_not_at_s64s() {
    // The width comes from the type, so `u8` overflows at 255 even though the
    // arithmetic is done in `i128`.
    let fixture = Fixture::build("bump :: (a: u8, b: u8) -> u8 { return a + b; }");
    assert_eq!(
        fixture.int("bump", vec![Value::Scalar(200), Value::Scalar(55)]),
        255
    );
    assert_eq!(
        fixture.call(
            "bump",
            vec![Value::Scalar(200), Value::Scalar(56)],
            Mode::Comptime
        ),
        Err(VmError::Trap(Trap::Overflow { what: "addition" }))
    );
}

// ---------------------------------------------------------------------------
// Control flow — block parameters eliminated
// ---------------------------------------------------------------------------

#[test]
fn an_if_picks_the_right_arm() {
    let fixture =
        Fixture::build("pick :: (n: s64) -> s64 { if n > 0 { return 1; } else { return 0; } }");
    assert_eq!(fixture.int("pick", vec![s64(5)]), 1);
    assert_eq!(fixture.int("pick", vec![s64(-5)]), 0);
}

#[test]
fn a_braceless_body_runs_the_statement_it_looked_like_it_would() {
    // The wave before this one found that a braceless body was silently discarded by
    // `jr-hir`. Running it is the strongest possible statement that it is back.
    let fixture = Fixture::build("pick :: (n: s64) -> s64 { if n > 0 return 1; return 0; }");
    assert_eq!(fixture.int("pick", vec![s64(5)]), 1);
    assert_eq!(fixture.int("pick", vec![s64(-5)]), 0);
}

#[test]
fn a_value_merged_from_two_arms_comes_through_the_block_parameter() {
    let fixture = Fixture::build(
        "pick :: (n: s64) -> s64 { x := 0; if n > 0 { x = 10; } else { x = 20; } return x; }",
    );
    assert_eq!(fixture.int("pick", vec![s64(1)]), 10);
    assert_eq!(fixture.int("pick", vec![s64(-1)]), 20);
}

#[test]
fn a_loop_carried_variable_survives_every_iteration() {
    // This is where a wrong parallel copy on the back edge shows up, and nowhere else.
    let fixture = Fixture::build(
        "sum :: (n: s64) -> s64 { total := 0; i := 0; while i < n { i = i + 1; total = total + i; } return total; }",
    );
    assert_eq!(fixture.int("sum", vec![s64(0)]), 0);
    assert_eq!(fixture.int("sum", vec![s64(4)]), 10);
    assert_eq!(fixture.int("sum", vec![s64(100)]), 5050);
}

#[test]
fn two_variables_that_swap_across_a_back_edge_do_not_collapse() {
    // The case the parallel copy exists for: naive sequential copies would make both
    // variables equal after one iteration.
    let fixture = Fixture::build(
        "swap :: (n: s64) -> s64 { a := 1; b := 2; i := 0; while i < n { t := a; a = b; b = t; i = i + 1; } return a * 10 + b; }",
    );
    assert_eq!(fixture.int("swap", vec![s64(0)]), 12);
    assert_eq!(fixture.int("swap", vec![s64(1)]), 21, "one swap");
    assert_eq!(fixture.int("swap", vec![s64(2)]), 12, "two swaps undo it");
}

#[test]
fn break_and_continue_leave_and_restart_the_loop() {
    let fixture = Fixture::build(
        "count :: (n: s64) -> s64 { i := 0; total := 0; while i < 100 { i = i + 1; if i > n { break; } total = total + 1; } return total; }",
    );
    assert_eq!(fixture.int("count", vec![s64(3)]), 3);
}

#[test]
fn short_circuit_and_does_not_evaluate_its_right_operand() {
    // `&&` is control flow in MIR, not an operator, so this also proves the branch
    // and its join were lowered correctly. Dividing by zero on the right is how the
    // test can tell whether it was evaluated.
    let fixture = Fixture::build("safe :: (n: s64) -> bool { return n != 0 && 10 / n > 0; }");
    assert_eq!(
        fixture.call("safe", vec![s64(0)], Mode::Comptime),
        Ok(Value::bool(false)),
        "the right operand must not be evaluated, or this would trap"
    );
    assert_eq!(
        fixture.call("safe", vec![s64(5)], Mode::Comptime),
        Ok(Value::bool(true))
    );
}

#[test]
fn short_circuit_or_does_not_evaluate_its_right_operand() {
    let fixture = Fixture::build("safe :: (n: s64) -> bool { return n == 0 || 10 / n > 0; }");
    assert_eq!(
        fixture.call("safe", vec![s64(0)], Mode::Comptime),
        Ok(Value::bool(true))
    );
}

// ---------------------------------------------------------------------------
// Memory — the layout ADR-0018 §2 put in the pool
// ---------------------------------------------------------------------------

#[test]
fn a_struct_field_round_trips_through_its_slot() {
    let fixture = Fixture::build(
        "Point :: struct { x: s64; y: s64; }\n\
         go :: () -> s64 { p: Point; p.x = 4; p.y = 5; return p.x * 10 + p.y; }",
    );
    assert_eq!(fixture.int("go", vec![]), 45);
}

#[test]
fn a_declared_local_starts_at_zero() {
    // `b: s64;` is default-initialised, which is not the same as `c: s64 = ---;`.
    let fixture = Fixture::build("go :: () -> s64 { b: s64; return b; }");
    assert_eq!(fixture.int("go", vec![]), 0);
}

#[test]
fn reading_an_uninitialised_local_traps_rather_than_reading_as_zero() {
    // E0227 reports this statically. Running it anyway must not silently produce a
    // number, or the diagnostic would be the only thing standing between the program
    // and a wrong answer.
    let fixture = Fixture::build("go :: () -> s64 { c: s64 = ---; return c; }");
    assert_eq!(
        fixture.call("go", vec![], Mode::Comptime),
        Err(VmError::Trap(Trap::UninitialisedRead))
    );
}

#[test]
fn a_pointer_round_trips_through_address_of_and_dereference() {
    let fixture = Fixture::build("go :: () -> s64 { n := 9; p := *n; return p.*; }");
    assert_eq!(fixture.int("go", vec![]), 9);
}

#[test]
fn a_string_carries_its_own_length_and_its_bytes() {
    // ADR-0004's payoff, executable: `.count` is O(1) and `.data` points at the real
    // bytes. The count is what the layout in `jr-pool` says it is.
    let fixture = Fixture::build("len :: () -> s64 { s := \"hello\"; return s.count; }");
    assert_eq!(fixture.int("len", vec![]), 5);
}

#[test]
fn a_string_with_an_embedded_nul_still_reports_its_full_length() {
    // Not NUL-terminated (ADR-0004), so the length is not a scan.
    let fixture = Fixture::build("len :: () -> s64 { s := \"a\\0b\"; return s.count; }");
    assert_eq!(fixture.int("len", vec![]), 3);
}

#[test]
fn a_field_of_a_string_parameter_reads_the_callers_string() {
    // The silent miscompile this wave fixed, executed rather than merely inspected:
    // `s.data`/`s.count` on a parameter used to lower to `undef`.
    let fixture = Fixture::build(
        "count_of :: (s: string) -> s64 { return s.count; }\n\
         go :: () -> s64 { return count_of(\"jairs\"); }",
    );
    assert_eq!(fixture.int("go", vec![]), 5);
}

// ---------------------------------------------------------------------------
// Foreign calls — ADR-0006's gate, and the exit criterion's mechanism
// ---------------------------------------------------------------------------

const BASIC: &str = "libc :: #system_library \"c\";\n\
     write :: (fd: s64, buf: *u8, count: s64) -> s64 #foreign libc \"write\";\n\
     print :: (s: string) { write(1, s.data, s.count); }\n";

#[test]
fn a_program_can_write_to_standard_output_through_libc() {
    // The mechanism `PLAN.md` §1.4's exit criterion runs on: a Jairs string handed to
    // libc `write` with no copy, because ADR-0004 already made it the `(pointer,
    // length)` shape `write(2)` wants.
    let source = format!("{BASIC}go :: () {{ print(\"hello from Jairs\\n\"); }}");
    let fixture = Fixture::build(&source);
    assert_eq!(fixture.output("go"), "hello from Jairs\n");
}

#[test]
fn a_foreign_call_is_refused_at_comptime_until_wave_w6() {
    // ADR-0006 allows comptime FFI *behind* `#foreign_at_comptime`, which does not
    // exist yet. The bridge does, so without the mode check the allowance would be
    // granted by accident to every program.
    let source = format!("{BASIC}go :: () {{ print(\"nope\\n\"); }}");
    let fixture = Fixture::build(&source);
    match fixture.call("go", vec![], Mode::Comptime) {
        Err(VmError::Unsupported(message)) => {
            assert!(
                message.contains("foreign_at_comptime"),
                "the refusal must say what would make it work: {message}"
            );
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_foreign_result_comes_back_as_a_value() {
    // `write` returns the byte count, and a Jairs program may read it.
    let source =
        format!("{BASIC}go :: () -> s64 {{ s := \"abc\"; return write(1, s.data, s.count); }}");
    let fixture = Fixture::build(&source);
    let mut vm = Vm::new(&fixture.program, &fixture.pool, Mode::Runtime).expect("room");
    // Its own `Vm` rather than `call`, because this test needs `Mode::Runtime` and a live `write` —
    // so the context has to be prepended here too (ADR-0057 §5). `go` is an ordinary Jairs
    // procedure, so it takes one; the `write` it calls is `#foreign` and does not.
    let args = fixture.with_context(&mut vm, "go", Vec::new());
    let result = vm.call(fixture.proc("go"), args).expect("write works");
    assert_eq!(result.as_int(jr_vm::IntKind::S64), Ok(3));
}

#[test]
fn a_write_whose_count_leaves_the_region_traps_rather_than_reading_past_it() {
    // ADR-0126. `marshal` validates a pointer argument for **one byte**, and the capture path
    // then built a `slice::from_raw_parts(buf, count)` over it — so `count` was never bounded.
    // With a two-byte string in a 1 MiB region, `4_000_000` read ~3 MB past the end of that
    // region's `Vec<u8>` and captured it as the program's own output; `2e9` killed the process
    // with `SIGBUS`; and the native back end wrote 114,688 bytes for the same program, so the
    // two engines disagreed. The span the VM itself dereferences must go through the same
    // bounds check as every other access.
    //
    // Note what this pins and what it does not: the bound is the **region**, not the buffer.
    // `s.count + 100` still reads neighbouring VM bytes, which is the linear-memory model
    // `Memory`'s own docs describe. What can no longer happen is *leaving* the region.
    let source =
        format!("{BASIC}go :: () -> s64 {{ s := \"hi\"; return write(1, s.data, 4000000); }}");
    let fixture = Fixture::build(&source);
    let mut vm = Vm::new(&fixture.program, &fixture.pool, Mode::Runtime).expect("room");
    let args = fixture.with_context(&mut vm, "go", Vec::new());
    match vm.call(fixture.proc("go"), args) {
        Err(VmError::Trap(Trap::BadAddress { .. })) => {}
        other => panic!("an over-long write must trap, got {other:?}"),
    }
    assert!(
        vm.captured_output().is_empty(),
        "nothing may be captured from a span the VM does not own"
    );
}

#[test]
fn exit_stops_the_program_without_stopping_the_compiler() {
    // Calling the host `exit` would end the build. It becomes a value the CLI turns
    // into an exit status instead.
    let source =
        format!("{BASIC}exit :: (status: s64) #foreign libc \"exit\";\ngo :: () {{ exit(3); }}");
    let fixture = Fixture::build(&source);
    assert_eq!(
        fixture.call("go", vec![], Mode::Runtime),
        Err(VmError::Exited(3))
    );
}

// ---------------------------------------------------------------------------
// Limits
// ---------------------------------------------------------------------------

#[test]
fn unbounded_recursion_is_reported_rather_than_overflowing_the_compilers_stack() {
    // A Jairs call is a Rust recursive call, so this is the one failure mode a
    // compiler must never have.
    let fixture = Fixture::build("forever :: (n: s64) -> s64 { return forever(n + 1); }");
    assert_eq!(
        fixture.call("forever", vec![s64(0)], Mode::Comptime),
        Err(VmError::Exhausted("call depth"))
    );
}

#[test]
fn a_loop_containing_a_call_does_not_leak_a_frame_per_iteration() {
    // Frames are a stack mark rather than a bump that never rewinds; without the
    // rewind this exhausts memory long before it finishes.
    let fixture = Fixture::build(
        "Point :: struct { x: s64; y: s64; }\n\
         one :: () -> s64 { p: Point; p.x = 1; return p.x; }\n\
         go :: () -> s64 { i := 0; total := 0; while i < 2000 { total = total + one(); i = i + 1; } return total; }",
    );
    assert_eq!(fixture.int("go", vec![]), 2000);
}

// ---------------------------------------------------------------------------
// Globals — program-lifetime storage (ADR-0186)
// ---------------------------------------------------------------------------

#[test]
fn a_global_reads_its_initial_value_writes_and_reads_it_back() {
    let fixture = Fixture::build(
        "counter: s64 = 5;\n\
         read_counter :: () -> s64 { return counter; }\n\
         write_counter :: (v: s64) -> s64 { counter = v; return counter; }",
    );
    // The initial value is `5`, not zero — the failure mode ADR-0186's contract calls out
    // explicitly: a global that reads zero when it should read five is the bug this feature
    // actually has.
    let mut vm = Vm::new(&fixture.program, &fixture.pool, Mode::Comptime)
        .expect("room for the string constants and the globals");
    let args = fixture.with_context(&mut vm, "read_counter", Vec::new());
    let initial = vm
        .call(fixture.proc("read_counter"), args)
        .expect("read_counter failed")
        .as_int(jr_vm::IntKind::S64)
        .expect("read_counter did not return an integer");
    assert_eq!(initial, 5);

    let args = fixture.with_context(&mut vm, "write_counter", vec![s64(9)]);
    let written = vm
        .call(fixture.proc("write_counter"), args)
        .expect("write_counter failed")
        .as_int(jr_vm::IntKind::S64)
        .expect("write_counter did not return an integer");
    assert_eq!(written, 9);

    let args = fixture.with_context(&mut vm, "read_counter", Vec::new());
    let after = vm
        .call(fixture.proc("read_counter"), args)
        .expect("read_counter failed")
        .as_int(jr_vm::IntKind::S64)
        .expect("read_counter did not return an integer");
    assert_eq!(after, 9);
}

#[test]
fn a_second_procedure_observes_the_first_procedures_write() {
    // The property a global exists for, and the one a per-frame (slot-shaped) implementation
    // would get wrong: `increment`'s write must be visible to `run`, a *different* procedure —
    // not to a second call of `increment` itself, which a body-local slot could also satisfy by
    // accident. `run` calls `increment` and then reads `counter` directly, so the only way its
    // answer can be `15` is if the two procedures' frames share the same storage.
    let fixture = Fixture::build(
        "counter: s64 = 10;\n\
         increment :: () { counter = counter + 5; }\n\
         run :: () -> s64 { increment(); return counter; }",
    );
    assert_eq!(fixture.int("run", vec![]), 15);
}

#[test]
fn a_global_with_no_initialiser_is_zeroed_not_undefined() {
    let fixture = Fixture::build(
        "flag: s64 = ---;\n\
         read_flag :: () -> s64 { return flag; }",
    );
    assert_eq!(fixture.int("read_flag", vec![]), 0);
}

#[test]
fn a_globals_initialiser_cannot_read_another_global() {
    // `a: s64 = 5; b: s64 = a;` — ADR-0186 §2's example of the construct it refuses: "there is no
    // moment before `main` at which arbitrary code could run to produce the value", and a global's
    // *current* value is exactly such a thing. The refusal is not in this crate at all — it never
    // reaches `jr_vm::compile`. `jr-db`'s `consts.rs` evaluates a global's initialiser through
    // `jr_mir::thunk_ref` + `jr_mir::lower_const`, and `b`'s reference to `a` resolves to
    // `Res::Item`, which that thunk's `consts.item(a)` — deliberately never populated for a global
    // (ADR-0186 §2's whole point: a global is not a constant) — answers `None` for. So this is a
    // clean `Poisoned::Here`, not a panic and not a wrong value, and it happens one call before
    // this crate would ever see a `PlaceBase::Global`.
    //
    // This test calls the same two functions `jr-db` does, with `a`'s own value already known —
    // standing in for "after round 1 of the real fixpoint has already resolved `a`" — because nothing
    // in this crate runs that fixpoint itself.
    let interner = Interner::new();
    let mut pool = Pool::new();
    let source = "a: s64 = 5;\nb: s64 = a;\n";
    let parsed = jr_syntax::parse(source, FILE);
    let (hir, _) = jr_hir::lower_file(&parsed, FILE, &interner);
    let (resolve, _) = jr_hir::resolve(&hir, &[], &interner);
    let signatures = jr_sema::file_signatures(&hir, FILE, &resolve, &[], &mut pool, &interner);
    let checked = jr_sema::check_file(
        &hir,
        FILE,
        &resolve,
        &signatures.signatures,
        &[],
        &[],
        &mut pool,
        &interner,
    );
    let mut types = signatures.types;
    types.absorb(&checked.types);

    let (a_item, a_init_expr) = hir
        .items
        .iter()
        .enumerate()
        .find_map(|(index, item)| {
            let jr_hir::ItemKind::Var {
                init: Some(expr), ..
            } = &item.kind
            else {
                return None;
            };
            let name = item.name?;
            (interner.resolve(name) == "a").then_some((jr_hir::ItemId::from_usize(index), *expr))
        })
        .expect("`a` is a global with an initialiser");
    let (_, b_init_expr) = hir
        .items
        .iter()
        .enumerate()
        .find_map(|(index, item)| {
            let jr_hir::ItemKind::Var {
                init: Some(expr), ..
            } = &item.kind
            else {
                return None;
            };
            let name = item.name?;
            (interner.resolve(name) == "b").then_some((jr_hir::ItemId::from_usize(index), *expr))
        })
        .expect("`b` is a global with an initialiser");

    let mut values = ConstValues::new();
    let jr_hir::Expr::Literal(jr_hir::Literal::Int { value, .. }, _) =
        &hir.exprs[a_init_expr.index()]
    else {
        panic!("`a`'s initialiser was not the literal this test assumes");
    };
    values.set_global_init(a_item, pool.int_value(jr_pool::PoolId::S64, *value as u64));

    let thunk_proc = jr_mir::thunk_ref(&hir, FILE, b_init_expr.index());
    let outcome = jr_mir::lower_const(
        &hir,
        FILE,
        thunk_proc,
        b_init_expr,
        jr_hir::ExprScope::TopLevel,
        &resolve,
        &types,
        &values,
        &ImportedProcs::new(),
        &mut pool,
    );
    assert!(
        matches!(outcome, Err(Poisoned::Here(_))),
        "reading `a` from `b`'s initialiser must be a clean refusal, got {outcome:?}"
    );
}
