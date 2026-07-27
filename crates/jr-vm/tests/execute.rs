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
use jr_mir::{ConstValues, ImportedProcs, ProcRef};
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

        let mir = jr_mir::lower_file(
            &hir,
            &resolve,
            &types,
            &signatures.signatures,
            &ConstValues::new(),
            &ImportedProcs::new(),
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
            &interner,
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
        vm.call(self.proc(name), args)
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
        vm.call(self.proc(name), Vec::new())
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
    let result = vm
        .call(fixture.proc("go"), Vec::new())
        .expect("write works");
    assert_eq!(result.as_int(jr_vm::IntKind::S64), Ok(3));
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
