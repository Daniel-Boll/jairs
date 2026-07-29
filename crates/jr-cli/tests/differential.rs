//! The differential harness `PLAN.md` §1.4 asks for.
//!
//! # What it tests, and why it is the point of the whole slice
//!
//! `PLAN.md` §3.1 calls one invariant load-bearing:
//!
//! > comptime and runtime execute *the same* MIR. The VM consumes bytecode lowered
//! > from the identical MIR that Cranelift consumes. Any other arrangement guarantees
//! > `#run` and runtime silently disagree.
//!
//! Sharing MIR is what makes agreement *likely*. It is not what makes it *checked*.
//! Every construct passes through two independent lowerings — `jr-vm`'s bytecode and
//! `jr-codegen-clif`'s Cranelift IR — and each has its own idea of what a byte offset,
//! an integer width, an aggregate and a trap are. This is the test that says they
//! agree, and ADR-0019 rejects "trust the shared MIR" explicitly: both of this
//! project's silent miscompiles were places where a plausible argument stood in for a
//! check.
//!
//! # Why it runs the real binary in a subprocess
//!
//! Because a program's observable behaviour is its **output and its exit status**, and
//! those only exist once there is a process. Calling the back end in-process would
//! test that code was generated, not that it does the same thing: the first native run
//! of `024-hello.jr` printed both its lines perfectly and exited **1** instead of 0,
//! and no in-process assertion about generated code would have noticed.
//!
//! # Why the corpus drives it rather than a list of cases
//!
//! `modules/Basic` hid a silent miscompile for an entire wave because it was not in
//! `tests/corpus/valid/` and nothing executed it. A harness that enumerates programs
//! itself acquires each new corpus program automatically; one with a hand-written list
//! acquires only what someone remembered to add.

use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

/// The workspace root, from this crate's manifest.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The `jr` binary under test.
///
/// Cargo builds it before running an integration test of the same crate, so this is
/// the binary the change under test produced rather than whatever is on `PATH`.
fn jr() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_jr"))
}

/// What one execution engine did with a program.
///
/// `stderr` is compared as well as `stdout`, and that is not thoroughness for its own
/// sake: a trapping program's entire observable behaviour *is* its stderr, and the
/// first version of this harness compared only stdout and so reported two engines as
/// agreeing while one said `error: addition overflowed` and the other said
/// `arithmetic overflowed` with no prefix and no newline.
#[derive(Debug, PartialEq, Eq)]
struct Behaviour {
    /// Everything the program wrote to standard output.
    stdout: String,
    /// Everything it wrote to standard error, including any trap message.
    stderr: String,
    /// The status it exited with.
    status: i32,
}

/// Runs a program in the bytecode VM.
fn run_in_vm(program: &Path) -> Behaviour {
    let output = Command::new(jr())
        .arg("run")
        .arg(program)
        .arg("-I")
        .arg(workspace_root().join("modules"))
        .output()
        .expect("jr run should be executable");
    Behaviour {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        status: output.status.code().unwrap_or(-1),
    }
}

/// Compiles a program natively and runs the result.
///
/// A build failure is reported with the compiler's own message rather than as a bare
/// assertion, because "the back end refused this construct" and "the two engines
/// disagree" are different findings and only the second is a differential failure.
fn run_natively(program: &Path, dir: &Path) -> Behaviour {
    let binary = dir.join("program");
    let built = Command::new(jr())
        .arg("build")
        .arg(program)
        .arg("-o")
        .arg(&binary)
        .arg("-I")
        .arg(workspace_root().join("modules"))
        .output()
        .expect("jr build should be executable");
    assert!(
        built.status.success(),
        "`jr build {}` failed ({}):\n{}{}",
        program.display(),
        built.status,
        String::from_utf8_lossy(&built.stderr),
        String::from_utf8_lossy(&built.stdout),
    );

    let output = Command::new(&binary)
        .output()
        .unwrap_or_else(|e| panic!("cannot run {}: {e}", binary.display()));
    Behaviour {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        status: output.status.code().unwrap_or(-1),
    }
}

/// Every corpus program that declares `main`, and so can be executed.
///
/// Discovered by reading the directory rather than listed, so a new corpus program is
/// covered the day it is added.
fn executable_programs() -> Vec<PathBuf> {
    let dir = workspace_root().join("tests/corpus/valid");
    let mut programs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jr"))
        .filter(|path| {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            // A program is executable exactly when it declares `main`; Jairs-0 has no
            // entry-point attribute, so the name is the whole convention.
            text.lines().any(|line| line.starts_with("main :: "))
        })
        .collect();
    programs.sort();
    programs
}

#[test]
fn the_corpus_has_executable_programs() {
    // A harness that silently found nothing would pass forever. This is the guard
    // against the directory walk breaking rather than a statement about the corpus.
    let programs = executable_programs();
    assert!(
        programs.len() >= 15,
        "expected the corpus to contain executable programs, found {}",
        programs.len()
    );
}

#[test]
fn every_corpus_program_behaves_identically_in_both_engines() {
    let dir = TempDir::new().expect("a temporary directory");
    let mut checked = Vec::new();
    let mut disagreements = Vec::new();

    for program in executable_programs() {
        let vm = run_in_vm(&program);
        let native = run_natively(&program, dir.path());
        let name = program
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if vm == native {
            checked.push(name);
        } else {
            disagreements.push(format!(
                "{name}:\n     VM: out {:?} err {:?} exit {}\n  native: out {:?} err {:?} exit {}",
                vm.stdout, vm.stderr, vm.status, native.stdout, native.stderr, native.status
            ));
        }
    }

    assert!(
        disagreements.is_empty(),
        "the VM and the native back end disagree about {} of {} programs:\n{}",
        disagreements.len(),
        checked.len() + disagreements.len(),
        disagreements.join("\n"),
    );
}

#[test]
fn the_slice_exit_criterion_produces_output_in_both_engines() {
    // `PLAN.md` §1.4 names this file specifically. The test above would pass if every
    // program produced *no* output in both engines, so this one asserts the text.
    let dir = TempDir::new().expect("a temporary directory");
    let program = workspace_root().join("tests/corpus/valid/024-hello.jr");

    let expected = "hello from Jairs\narithmetic and pointers agree\n";
    let vm = run_in_vm(&program);
    let native = run_natively(&program, dir.path());

    assert_eq!(vm.stdout, expected, "the VM's output changed");
    assert_eq!(native.stdout, expected, "the native output changed");
    assert_eq!(vm.status, 0);
    assert_eq!(native.status, 0);
}

// ---------------------------------------------------------------------------
// Programs whose *result* is observable
// ---------------------------------------------------------------------------

/// Compiles and runs a source string in both engines, returning both behaviours.
fn both_engines(source: &str, dir: &Path, name: &str) -> (Behaviour, Behaviour) {
    let path = dir.join(format!("{name}.jr"));
    std::fs::write(&path, source).expect("a writable temporary directory");
    (run_in_vm(&path), run_natively(&path, dir))
}

/// A program that computes `expr` and makes the answer observable.
///
/// Jairs-0 cannot print an integer — that needs a `s64`-to-`u8` conversion and `cast`
/// is reserved until wave W1, which `modules/Basic` documents at length. But
/// `modules/Basic` does export `exit`, so a computation's result can leave the program
/// as its **exit status**, and an exit status is observable in both engines.
///
/// This matters because it is what gives the corpus differential teeth. Only two of
/// the fifteen executable corpus programs print anything, so on its own that test
/// largely compares "no output, exit 0" with "no output, exit 0" — it would catch a
/// crash or a spurious trap, and it did catch a wrong exit status, but it says almost
/// nothing about whether the two back ends *compute* the same values.
fn exit_with(expr: &str) -> String {
    format!("#import \"Basic\";\n\nmain :: () {{\n    exit({expr});\n}}\n")
}

#[test]
fn both_engines_compute_the_same_arithmetic() {
    let dir = TempDir::new().expect("a temporary directory");
    // Kept under 256 because a process exit status is one byte, and each case is
    // chosen so a wrong answer changes it: precedence, associativity, division
    // truncation, and remainder sign.
    let cases = [
        ("2 + 3", 5),
        ("2 + 3 * 4", 14),
        ("(2 + 3) * 4", 20),
        ("100 - 30 - 20", 50),
        ("100 / 7", 14),
        ("100 % 7", 2),
        ("7 * 6 - 2", 40),
        ("(10 - 4) / 3", 2),
        ("0 - 5 + 105", 100),
    ];
    for (index, (expr, expected)) in cases.iter().enumerate() {
        let (vm, native) = both_engines(&exit_with(expr), dir.path(), &format!("arith{index}"));
        assert_eq!(vm.status, *expected, "the VM computed `{expr}` wrongly");
        assert_eq!(
            native.status, *expected,
            "the native back end computed `{expr}` wrongly"
        );
        assert_eq!(vm, native, "the two engines disagree about `{expr}`");
    }
}

#[test]
fn both_engines_agree_about_control_flow_and_state() {
    let dir = TempDir::new().expect("a temporary directory");
    let cases = [
        // A loop-carried variable, which is a block parameter in MIR.
        (
            "count := 0;\n    i := 0;\n    while i < 7 {\n        count = count + i;\n        i = i + 1;\n    }\n    exit(count);",
            21,
        ),
        // A value merged from two arms of an `if`.
        (
            "x := 0;\n    if 3 > 2 {\n        x = 11;\n    }\n    exit(x);",
            11,
        ),
        // `break` leaves the loop; `continue` restarts it.
        (
            "i := 0;\n    while i < 100 {\n        i = i + 1;\n        if i > 8 {\n            break;\n        }\n    }\n    exit(i);",
            9,
        ),
        // A pointer round-trips through address-of and dereference.
        ("v := 42;\n    p := *v;\n    exit(p.*);", 42),
        // Short-circuit `&&` must not evaluate its right operand's effect on the
        // answer, and comparisons produce `bool`.
        (
            "flag := 1 > 2 && 3 > 2;\n    if flag {\n        exit(1);\n    }\n    exit(7);",
            7,
        ),
    ];
    for (index, (body, expected)) in cases.iter().enumerate() {
        let source = format!("#import \"Basic\";\n\nmain :: () {{\n    {body}\n}}\n");
        let (vm, native) = both_engines(&source, dir.path(), &format!("flow{index}"));
        assert_eq!(vm.status, *expected, "the VM ran case {index} wrongly");
        assert_eq!(
            native.status, *expected,
            "the native back end ran case {index} wrongly"
        );
        assert_eq!(vm, native, "the two engines disagree about case {index}");
    }
}

#[test]
fn both_engines_agree_about_structs_and_calls() {
    let dir = TempDir::new().expect("a temporary directory");
    // A struct field is a byte offset, and this is the one place the VM and Cranelift
    // could disagree *silently*: ADR-0018 §2 put layout in `jr-pool` so that both ask
    // the same question, and this is the test that says they got the same answer.
    let source = "#import \"Basic\";\n\
                  \n\
                  Pair :: struct {\n    a: s64;\n    b: s64;\n}\n\
                  \n\
                  add :: (x: s64, y: s64) -> s64 {\n    return x + y;\n}\n\
                  \n\
                  main :: () {\n\
                  \x20   p: Pair;\n\
                  \x20   p.a = 30;\n\
                  \x20   p.b = 12;\n\
                  \x20   exit(add(p.a, p.b));\n\
                  }\n";
    let (vm, native) = both_engines(source, dir.path(), "structs");
    assert_eq!(vm.status, 42, "the VM got a struct field or a call wrong");
    assert_eq!(
        native.status, 42,
        "the native back end got a struct field or a call wrong"
    );
    assert_eq!(vm, native);
}

#[test]
fn both_engines_trap_on_overflow_rather_than_wrapping() {
    let dir = TempDir::new().expect("a temporary directory");
    // ADR-0002. The status is what makes this a differential rather than two separate
    // assertions: `jr run` exits 4 on a trap, and the native trap helper is given the
    // same status precisely so that the two are comparable (ADR-0019 §2).
    let source = "#import \"Basic\";\n\n\
                  MAX :: 9223372036854775807;\n\n\
                  main :: () {\n    exit(MAX + 1);\n}\n";
    let (vm, native) = both_engines(source, dir.path(), "overflow");
    assert_eq!(vm.status, 4, "the VM must trap on overflow");
    assert_eq!(native.status, 4, "native code must trap on overflow");
    assert_eq!(
        vm, native,
        "the two engines disagree about an overflow trap"
    );
}

#[test]
fn both_engines_trap_on_division_by_zero() {
    let dir = TempDir::new().expect("a temporary directory");
    let source = "#import \"Basic\";\n\n\
                  ZERO :: 0;\n\n\
                  main :: () {\n    exit(10 / ZERO);\n}\n";
    let (vm, native) = both_engines(source, dir.path(), "divzero");
    assert_eq!(vm.status, 4, "the VM must trap on division by zero");
    assert_eq!(
        native.status, 4,
        "native code must trap on division by zero"
    );
    assert_eq!(vm, native);
}

#[test]
fn both_engines_trap_on_an_index_out_of_bounds() {
    let dir = TempDir::new().expect("a temporary directory");
    // ADR-0003's explicit `bounds_check`, run in the VM and compiled natively. The index
    // is computed rather than a literal, so `jr-sema`'s E0236 does not catch it first and
    // the *runtime* check is what fires.
    let source = concat!(
        "#import \"Basic\";\n\n",
        "main :: () {\n",
        "    buf: [4]u8;\n",
        "    i := 0;\n",
        "    while i < 8 {\n",
        "        buf[i] = 1;\n",
        "        i = i + 1;\n",
        "    }\n",
        "}\n",
    );
    let (vm, native) = both_engines(source, dir.path(), "oob");
    assert_eq!(vm.status, 4, "the VM must trap on an out-of-bounds index");
    assert_eq!(
        native.status, 4,
        "native code must trap on an out-of-bounds index"
    );
    assert!(
        vm.stderr.contains("index out of bounds"),
        "the trap must name what went wrong, got {:?}",
        vm.stderr
    );
    assert_eq!(
        vm, native,
        "the two engines disagree about a bounds-check trap"
    );
}

#[test]
fn a_declared_aggregate_is_zeroed_in_both_engines() {
    let dir = TempDir::new().expect("a temporary directory");
    // ADR-0039 §4a, and this is a *regression* test rather than a new feature's test.
    // `p: Point;` emitted no zeroing at all: the VM zeroes a fresh frame so it exited 0,
    // while Cranelift's `ExplicitSlot` is raw stack, so the native binary exited with
    // whatever the last call left there — 184 and then 200 on consecutive builds.
    //
    // Nothing caught it because `differential.rs` compares observable output and no
    // corpus program observed a default-initialised aggregate. This test is that
    // observation.
    let source = concat!(
        "#import \"Basic\";\n\n",
        "Point :: struct { x: s64; y: s64; }\n\n",
        "main :: () {\n",
        "    p: Point;\n",
        "    exit(p.x + p.y);\n",
        "}\n",
    );
    let (vm, native) = both_engines(source, dir.path(), "zeroagg");
    assert_eq!(
        vm.status, 0,
        "a declared aggregate must be zeroed in the VM"
    );
    assert_eq!(
        native.status, 0,
        "a declared aggregate must be zeroed natively -- an uninitialised stack slot \
         exits with whatever the last call left there"
    );
    assert_eq!(vm, native);
}

#[test]
fn both_engines_agree_about_ieee_754_edge_cases() {
    let dir = TempDir::new().expect("a temporary directory");
    // ADR-0040 §1. Float arithmetic is the one thing in this project where the VM's
    // *software* evaluation and Cranelift's *hardware* instructions are genuinely different
    // implementations of one specification. An integer add is exact in both by construction;
    // `NaN == NaN` is only false in both because IEEE-754 says so, and this is what checks
    // that both actually obey it.
    let source = concat!(
        "#import \"Basic\";\n\n",
        "main :: () {\n",
        "    nan := 0.0 / 0.0;\n",
        "    inf := 1.0 / 0.0;\n",
        "    negz := -0.0;\n",
        "    n := 0;\n",
        // Each of these would come out backwards under a raw bit compare, in opposite
        // directions: identical bits for NaN, different bits for the two zeroes.
        "    if nan != nan { n = n + 1; }\n",
        "    if negz == 0.0 { n = n + 2; }\n",
        "    if inf > 1e300 { n = n + 4; }\n",
        // Saturating float-to-int, where Cranelift's non-`_sat` form would trap and the
        // interpreter would not.
        "    if cast(s8, 1000.0) == 127 { n = n + 8; }\n",
        "    if cast(s64, nan) == 0 { n = n + 16; }\n",
        "    exit(n);\n",
        "}\n",
    );
    let (vm, native) = both_engines(source, dir.path(), "ieee");
    assert_eq!(
        vm.status, 31,
        "every IEEE-754 assertion must hold in the VM"
    );
    assert_eq!(
        native.status, 31,
        "every IEEE-754 assertion must hold natively"
    );
    assert_eq!(vm, native, "the two engines disagree about IEEE-754");
}

#[test]
fn float_arithmetic_never_traps_in_either_engine() {
    let dir = TempDir::new().expect("a temporary directory");
    // ADR-0002 makes integer overflow trap; ADR-0040 §1 scopes that to integers. A program
    // that overflows, divides by zero and produces a NaN must run to completion and exit 0 —
    // in both engines, and with the *same* absence of a trap message.
    let source = concat!(
        "#import \"Basic\";\n\n",
        "main :: () {\n",
        "    big := 1e308;\n",
        "    overflowed := big * big;\n",
        "    divided := 1.0 / 0.0;\n",
        "    undefined := 0.0 / 0.0;\n",
        "    negated := -0.0;\n",
        "    if overflowed == divided { exit(0); }\n",
        "    exit(1);\n",
        "}\n",
    );
    let (vm, native) = both_engines(source, dir.path(), "notrap");
    assert_eq!(vm.status, 0, "float overflow must not trap in the VM");
    assert_eq!(native.status, 0, "float overflow must not trap natively");
    assert!(
        vm.stderr.is_empty(),
        "no trap message expected, got {:?}",
        vm.stderr
    );
    assert_eq!(vm, native);
}

#[test]
fn both_engines_agree_about_enum_values() {
    let dir = TempDir::new().expect("a temporary directory");
    // ADR-0041 §3's three rules, and the third is the one that is easy to get wrong by
    // resetting to the member's index: an explicit value makes *later* members continue from
    // it. `Colour.RED` folds to a constant at MIR, so this also checks that the fold and both
    // back ends agree about which number a member is.
    let source = concat!(
        "#import \"Basic\";\n\n",
        "Status :: enum { OK :: 200; MISSING :: 404; NEXT; }\n\n",
        "main :: () {\n",
        "    n := 0;\n",
        "    if cast(s64, Status.OK) == 200 { n = n + 1; }\n",
        "    if cast(s64, Status.MISSING) == 404 { n = n + 2; }\n",
        // 405, not 2: the continue-from-here rule.
        "    if cast(s64, Status.NEXT) == 405 { n = n + 4; }\n",
        "    if Status.OK != Status.MISSING { n = n + 8; }\n",
        "    exit(n);\n",
        "}\n",
    );
    let (vm, native) = both_engines(source, dir.path(), "enumvals");
    assert_eq!(vm.status, 15, "every enum assertion must hold in the VM");
    assert_eq!(native.status, 15, "every enum assertion must hold natively");
    assert_eq!(vm, native, "the two engines disagree about enum values");
}

#[test]
fn both_engines_trap_on_an_out_of_range_shift() {
    let dir = TempDir::new().expect("a temporary directory");
    // ADR-0042 §3, and the trap is what makes this worth a differential: **Cranelift masks
    // the shift count natively** — `ishl` on an `I8` uses the low 3 bits — so without an
    // explicit compare-and-trap the native binary would compute `x << 0` and exit 1 while the
    // VM trapped. One engine right and one wrong, silently.
    let source = concat!(
        "#import \"Basic\";\n\n",
        "main :: () {\n",
        "    a: s8 = 1;\n",
        "    c := 8;\n",
        "    b := a << c;\n",
        "    exit(1);\n",
        "}\n",
    );
    let (vm, native) = both_engines(source, dir.path(), "shift");
    assert_eq!(vm.status, 4, "the VM must trap on an out-of-range shift");
    assert_eq!(
        native.status, 4,
        "native code must trap rather than masking the count"
    );
    assert!(
        vm.stderr.contains("shift count out of range"),
        "the trap must name what went wrong, got {:?}",
        vm.stderr
    );
    assert_eq!(vm, native, "the two engines disagree about a shift trap");
}

#[test]
fn both_engines_agree_about_bitwise_operators_and_precedence() {
    let dir = TempDir::new().expect("a temporary directory");
    // The two orderings that are *not* C's (ADR-0042 §1), unparenthesised so that a
    // precedence change would move the answer rather than merely the tree.
    let source = concat!(
        "#import \"Basic\";\n\n",
        "main :: () {\n",
        "    a := 6;\n",
        "    n := 0;\n",
        "    if (a & 3) == 2 { n = n + 1; }\n",
        "    if (a ^ 5) == 3 { n = n + 2; }\n",
        "    if ~cast(u8, 0) == 255 { n = n + 4; }\n",
        // `(a & 3) == 2` under Jairs; C would read `a & (3 == 2)`.
        "    if a & 3 == 2 { n = n + 8; }\n",
        // `1 + (1 << 3)` = 9 under Jairs; C would read `(1 + 1) << 3` = 16.
        "    if 1 + 1 << 3 == 9 { n = n + 16; }\n",
        "    exit(n);\n",
        "}\n",
    );
    let (vm, native) = both_engines(source, dir.path(), "bitwise");
    assert_eq!(vm.status, 31, "every bitwise assertion must hold in the VM");
    assert_eq!(
        native.status, 31,
        "every bitwise assertion must hold natively"
    );
    assert_eq!(
        vm, native,
        "the two engines disagree about bitwise operators"
    );
}

#[test]
fn both_engines_agree_about_enum_flags() {
    let dir = TempDir::new().expect("a temporary directory");
    // ADR-0043 §2's numbering, including the case that is easy to get wrong two ways: after an
    // explicit `B :: 8`, the next flag is 16 — the next power of two above the *value*, not
    // above the member's index. A combination folds to a constant at MIR, so this also checks
    // the folder and both back ends agree about which number a flag set is.
    let source = concat!(
        "#import \"Basic\";\n\n",
        "F :: enum_flags { A; B :: 8; C; }\n\n",
        "main :: () {\n",
        "    n := 0;\n",
        "    if cast(s64, F.A) == 1 { n = n + 1; }\n",
        "    if cast(s64, F.C) == 16 { n = n + 2; }\n",
        // A combination keeps the flags type and has a value no member has.
        "    both := F.A | F.B;\n",
        "    if cast(s64, both) == 9 { n = n + 4; }\n",
        "    if (both & F.A) == F.A { n = n + 8; }\n",
        "    if (both & F.C) != F.C { n = n + 16; }\n",
        "    exit(n);\n",
        "}\n",
    );
    let (vm, native) = both_engines(source, dir.path(), "enumflags");
    assert_eq!(vm.status, 31, "every flags assertion must hold in the VM");
    assert_eq!(
        native.status, 31,
        "every flags assertion must hold natively"
    );
    assert_eq!(vm, native, "the two engines disagree about enum_flags");
}

// ---------------------------------------------------------------------------
// ADR-0020 — a trap names where it happened, identically in both engines
// ---------------------------------------------------------------------------

#[test]
fn a_trap_names_its_source_location_identically_in_both_engines() {
    // The criterion §1.4 asks for, and the reason ADR-0020 §2 put the formatter in
    // `jr-base`: the native message is chosen when the object is emitted and the VM's
    // is built while the program runs, so nothing but a shared formatter keeps them
    // equal. This asserts the whole rendered message rather than just that the two
    // agree — two engines that both lost the location would agree perfectly.
    let dir = TempDir::new().expect("a temporary directory");
    let path = dir.path().join("overflow.jr");
    let source = "#import \"Basic\";\n\
                  \n\
                  MAX :: 9223372036854775807;\n\
                  \n\
                  main :: () {\n\
                  \x20   exit(MAX + 1);\n\
                  }\n";
    std::fs::write(&path, source).expect("a writable temporary directory");

    let vm = run_in_vm(&path);
    let native = run_natively(&path, dir.path());

    let expected = format!(
        "error: addition overflowed\n  --> {}:6:10\n",
        path.display()
    );
    assert_eq!(vm.stderr, expected, "the VM's located trap message changed");
    assert_eq!(
        native.stderr, expected,
        "the native located trap message changed"
    );
    assert_eq!(vm.status, 4);
    assert_eq!(native.status, 4);
}

#[test]
fn a_division_by_zero_names_its_own_line() {
    // A second operation and a second line, so that the location is demonstrably
    // computed rather than a constant that happens to match one case.
    let dir = TempDir::new().expect("a temporary directory");
    let path = dir.path().join("divzero.jr");
    let source = "#import \"Basic\";\n\
                  \n\
                  ZERO :: 0;\n\
                  \n\
                  main :: () {\n\
                  \x20   n := 10;\n\
                  \x20   exit(n / ZERO);\n\
                  }\n";
    std::fs::write(&path, source).expect("a writable temporary directory");

    let vm = run_in_vm(&path);
    let native = run_natively(&path, dir.path());

    let expected = format!("error: division by zero\n  --> {}:7:10\n", path.display());
    assert_eq!(vm.stderr, expected);
    assert_eq!(native.stderr, expected);
    assert_eq!(vm, native);
}

// ---------------------------------------------------------------------------
// Inlining (ADR-0021)
// ---------------------------------------------------------------------------

#[test]
fn a_trap_inside_an_inlined_leaf_names_the_call_in_both_engines() {
    // ADR-0021 §3, and the reason it is asserted here rather than only in `jr-mir`:
    // the two engines resolve a span through different code at different times, so
    // "the inliner rewrote the spans" and "both engines print the same rewritten
    // span" are separate claims. `bump` is a leaf, so it is inlined into `main`, and
    // the overflow must be reported at the *call* on line 10 rather than at `a + 1`
    // on line 6 — a trap naming `bump`'s line would be naming code the caller did not
    // write, and after a cross-file inline it would name a file the program never
    // mentions.
    let dir = TempDir::new().expect("a temporary directory");
    let path = dir.path().join("inlinetrap.jr");
    let source = "#import \"Basic\";\n\
                  \n\
                  MAX :: 9223372036854775807;\n\
                  \n\
                  bump :: (a: s64) -> s64 {\n\
                  \x20   return a + 1;\n\
                  }\n\
                  \n\
                  main :: () {\n\
                  \x20   exit(bump(MAX));\n\
                  }\n";
    std::fs::write(&path, source).expect("a writable temporary directory");

    let vm = run_in_vm(&path);
    let native = run_natively(&path, dir.path());

    let expected = format!(
        "error: addition overflowed\n  --> {}:10:10\n",
        path.display()
    );
    assert_eq!(
        vm.stderr, expected,
        "the VM must name the call, not the inlined callee's line"
    );
    assert_eq!(
        native.stderr, expected,
        "and native must agree byte for byte"
    );
    assert_eq!(vm.status, 4);
    assert_eq!(native.status, 4);
}

#[test]
fn a_trap_inside_a_callee_that_was_not_inlined_names_its_own_line() {
    // The negative control for the test above, and the thing that makes it mean
    // something: the same program with a callee that calls `print` is not a leaf, so
    // ADR-0021 §4 refuses to inline it, and the trap names line 7 inside `bump`. If
    // the two tests ever agree on a line, one of them has stopped testing inlining.
    let dir = TempDir::new().expect("a temporary directory");
    let path = dir.path().join("nolinline.jr");
    let source = "#import \"Basic\";\n\
                  \n\
                  MAX :: 9223372036854775807;\n\
                  \n\
                  bump :: (a: s64) -> s64 {\n\
                  \x20   print(\"\");\n\
                  \x20   return a + 1;\n\
                  }\n\
                  \n\
                  main :: () {\n\
                  \x20   exit(bump(MAX));\n\
                  }\n";
    std::fs::write(&path, source).expect("a writable temporary directory");

    let vm = run_in_vm(&path);
    let native = run_natively(&path, dir.path());

    let expected = format!(
        "error: addition overflowed\n  --> {}:7:12\n",
        path.display()
    );
    assert_eq!(vm.stderr, expected);
    assert_eq!(native.stderr, expected);
    assert_eq!(vm, native);
}

// ---------------------------------------------------------------------------
// DCE and const-prop (ADR-0022)
// ---------------------------------------------------------------------------

#[test]
fn a_dead_expression_that_overflows_still_traps_in_both_engines() {
    // ADR-0022 §4's first refusal, as a running program. `dead` is assigned and never
    // read, so a DCE that deleted "assignments nobody reads" would delete the trap and
    // this program would print nothing and exit 0. The corpus cannot catch that: no
    // corpus program contains a dead trapping expression, which is why §7 asked for
    // targeted cases rather than trusting the enumeration.
    let dir = TempDir::new().expect("a temporary directory");
    let path = dir.path().join("deadtrap.jr");
    let source = "#import \"Basic\";\n\
                  \n\
                  MAX :: 9223372036854775807;\n\
                  \n\
                  main :: () {\n\
                  \x20   n := MAX;\n\
                  \x20   dead := n + 1;\n\
                  \x20   exit(0);\n\
                  }\n";
    std::fs::write(&path, source).expect("a writable temporary directory");

    let vm = run_in_vm(&path);
    let native = run_natively(&path, dir.path());

    let expected = format!(
        "error: addition overflowed\n  --> {}:7:13\n",
        path.display()
    );
    assert_eq!(vm.stderr, expected, "the VM lost a dead expression's trap");
    assert_eq!(
        native.stderr, expected,
        "native lost a dead expression's trap"
    );
    assert_eq!(vm.status, 4);
    assert_eq!(native.status, 4);
}

#[test]
fn a_dead_call_still_runs_in_both_engines() {
    // ADR-0022 §4's second refusal. `shout`'s result is discarded, but it prints and
    // then exits, so deleting the call is observable twice over — and `exit` is why
    // "a call can do anything" is not an abstract worry in a language whose only way
    // out of `main` is a foreign call.
    let dir = TempDir::new().expect("a temporary directory");
    let path = dir.path().join("deadcall.jr");
    let source = "#import \"Basic\";\n\
                  \n\
                  shout :: () -> s64 {\n\
                  \x20   print(\"ran\\n\");\n\
                  \x20   return 7;\n\
                  }\n\
                  \n\
                  main :: () {\n\
                  \x20   dead := shout();\n\
                  \x20   exit(3);\n\
                  }\n";
    std::fs::write(&path, source).expect("a writable temporary directory");

    let vm = run_in_vm(&path);
    let native = run_natively(&path, dir.path());

    assert_eq!(vm.stdout, "ran\n", "the VM dropped a dead call");
    assert_eq!(native.stdout, "ran\n", "native dropped a dead call");
    assert_eq!(vm.status, 3);
    assert_eq!(native, vm);
}

#[test]
fn folding_never_changes_an_answer_either_engine_computes() {
    // Const-prop bakes its answer into a `PoolId` that *both* engines then consume, so
    // a fold that disagrees with the interpreter does not show up as the two engines
    // disagreeing — it shows up as both agreeing on the wrong number. This is the case
    // ADR-0022 §2 moved the arithmetic into `jr-pool` for, and the only way to test it
    // is to assert the *value*, not the agreement.
    let dir = TempDir::new().expect("a temporary directory");
    let cases = [
        ("add(2, 3)", 5),
        ("add(100, 0 - 60)", 40),
        ("add(add(1, 2), add(3, 4))", 10),
    ];
    for (index, (expr, expected)) in cases.iter().enumerate() {
        let source = format!(
            "#import \"Basic\";\n\n\
             add :: (a: s64, b: s64) -> s64 {{ return a + b; }}\n\n\
             main :: () {{\n    exit({expr});\n}}\n"
        );
        let (vm, native) = both_engines(&source, dir.path(), &format!("fold{index}"));
        assert_eq!(
            vm.status, *expected,
            "the VM computed `{expr}` as {}",
            vm.status
        );
        assert_eq!(
            native.status, *expected,
            "native computed `{expr}` as {}",
            native.status
        );
    }
}

#[test]
fn forwarding_a_struct_field_changes_no_answer_in_either_engine() {
    // ADR-0023, as running programs. Forwarding replaces a load with an operand, so a
    // mistake about *which* store was available produces a wrong number rather than a
    // crash — and both engines would produce the same wrong number, because they
    // consume the same forwarded MIR. So these assert the value, not the agreement.
    let dir = TempDir::new().expect("a temporary directory");
    let cases = [
        // The plain case: write two fields, read them back.
        ("p.x = 4;\n    p.y = 5;", "p.x + p.y", 9),
        // A later store to the same field must win over an earlier one.
        ("p.x = 4;\n    p.x = 7;\n    p.y = 1;", "p.x + p.y", 8),
        // Interleaved, so a pass that confused the two fields gets a different answer.
        ("p.x = 10;\n    p.y = 3;\n    p.x = 20;", "p.x - p.y", 17),
    ];
    for (index, (writes, expr, expected)) in cases.iter().enumerate() {
        let source = format!(
            "#import \"Basic\";\n\n\
             Point :: struct {{ x: s64; y: s64; }}\n\n\
             main :: () {{\n    p: Point;\n    {writes}\n    exit({expr});\n}}\n"
        );
        let (vm, native) = both_engines(&source, dir.path(), &format!("fwd{index}"));
        assert_eq!(
            vm.status, *expected,
            "the VM computed `{expr}` as {}",
            vm.status
        );
        assert_eq!(
            native.status, *expected,
            "native computed `{expr}` as {}",
            native.status
        );
    }
}

#[test]
fn a_store_through_a_pointer_is_still_observed_after_forwarding() {
    // The aliasing case, made observable. `n`'s address is taken and written through,
    // so a pass that forwarded the *original* store into the later read would produce 1
    // instead of 9 — and both engines would agree on it.
    let dir = TempDir::new().expect("a temporary directory");
    let path = dir.path().join("aliased.jr");
    let source = "#import \"Basic\";\n\
                  \n\
                  main :: () {\n\
                  \x20   n := 1;\n\
                  \x20   q := *n;\n\
                  \x20   q.* = 9;\n\
                  \x20   exit(n);\n\
                  }\n";
    std::fs::write(&path, source).expect("a writable temporary directory");

    let vm = run_in_vm(&path);
    let native = run_natively(&path, dir.path());
    assert_eq!(vm.status, 9, "the VM forwarded across an indirect store");
    assert_eq!(
        native.status, 9,
        "native forwarded across an indirect store"
    );
}

#[test]
fn a_view_reads_and_writes_the_same_storage_in_both_engines() {
    // ADR-0044 §4: a view is a pointer to storage, not a copy of it. This is the property
    // that makes passing one worth doing, and the one a wrong `data` offset would break
    // silently — the callee would write somewhere else and the caller would read the array's
    // original contents, which is a plausible-looking wrong answer rather than a crash.
    //
    // A `[N]T` cannot express this test: the array would be copied and the write would be
    // invisible, which is exactly the difference being checked.
    let dir = TempDir::new().expect("a temporary directory");
    let source = "#import \"Basic\";\n\
                  \n\
                  fill :: (xs: []s64, value: s64) {\n\
                  \x20   i := 0;\n\
                  \x20   while i < xs.count {\n\
                  \x20       xs[i] = value;\n\
                  \x20       i = i + 1;\n\
                  \x20   }\n\
                  }\n\
                  \n\
                  main :: () {\n\
                  \x20   buf: [3]s64;\n\
                  \x20   fill(buf[], 5);\n\
                  \x20   exit(buf[0] + buf[1] + buf[2]);\n\
                  }\n";
    let (vm, native) = both_engines(source, dir.path(), "viewwrite");
    assert_eq!(vm.status, 15, "the VM wrote through the view");
    assert_eq!(native.status, 15, "native wrote through the view");
}

#[test]
fn a_views_count_is_loaded_rather_than_folded_in_both_engines() {
    // The difference between `[N]T` and `[]T` in one program: `total` is called twice with
    // views of different lengths, so a `.count` folded from a type — the way an array's is
    // (ADR-0039 §5) — would give the same answer twice and the sum would be wrong.
    let dir = TempDir::new().expect("a temporary directory");
    let source = "#import \"Basic\";\n\
                  \n\
                  total :: (xs: []s64) -> s64 {\n\
                  \x20   i := 0;\n\
                  \x20   t := 0;\n\
                  \x20   while i < xs.count {\n\
                  \x20       t = t + xs[i];\n\
                  \x20       i = i + 1;\n\
                  \x20   }\n\
                  \x20   return t;\n\
                  }\n\
                  \n\
                  main :: () {\n\
                  \x20   four: [4]s64;\n\
                  \x20   four[0] = 1;\n\
                  \x20   four[1] = 2;\n\
                  \x20   four[2] = 4;\n\
                  \x20   four[3] = 8;\n\
                  \x20   two: [2]s64;\n\
                  \x20   two[0] = 16;\n\
                  \x20   two[1] = 32;\n\
                  \x20   exit(total(four[]) + total(two[]));\n\
                  }\n";
    let (vm, native) = both_engines(source, dir.path(), "viewcount");
    assert_eq!(vm.status, 63, "the VM loaded each view's own count");
    assert_eq!(native.status, 63, "native loaded each view's own count");
}

#[test]
fn an_out_of_range_index_through_a_view_traps_in_both_engines() {
    // The bounds check with a **runtime** length, which is the first one MIR has had: the
    // check's `len` is a loaded `.count` rather than a constant (ADR-0039 §1's operand-shaped
    // `len` finally being spent). A back end that masked or ignored it would read past the
    // array instead of trapping, and the two engines would disagree about a program's output.
    let dir = TempDir::new().expect("a temporary directory");
    let source = "#import \"Basic\";\n\
                  \n\
                  at :: (xs: []s64, i: s64) -> s64 {\n\
                  \x20   return xs[i];\n\
                  }\n\
                  \n\
                  main :: () {\n\
                  \x20   buf: [2]s64;\n\
                  \x20   exit(at(buf[], 5));\n\
                  }\n";
    let (vm, native) = both_engines(source, dir.path(), "viewtrap");
    assert!(
        vm.stderr.contains("index out of bounds"),
        "the VM trapped: {:?}",
        vm.stderr
    );
    assert!(
        native.stderr.contains("index out of bounds"),
        "native trapped: {:?}",
        native.stderr
    );
    assert_eq!(
        vm.stderr, native.stderr,
        "ADR-0020 §2's one formatter, so the wording cannot drift"
    );
}

#[test]
fn a_union_reinterprets_bits_identically_in_both_engines() {
    // ADR-0045 §1's decision, as a running program: writing one field and reading another is
    // legal and reinterprets the bits. -1 as an `s64` is every bit set, so the low byte read
    // through a `u8` field is 255 — and a layout that placed the second field anywhere but
    // offset 0 would read 0 instead.
    //
    // This is the test the decision needs. A union that merely *ran* without reinterpreting
    // would prove nothing about whether it is untagged or where its fields sit.
    let dir = TempDir::new().expect("a temporary directory");
    let source = "#import \"Basic\";\n\
                  \n\
                  Mixed :: union { byte: u8; word: s64; }\n\
                  \n\
                  main :: () {\n\
                  \x20   m: Mixed;\n\
                  \x20   m.word = -1;\n\
                  \x20   exit(cast(s64, m.byte));\n\
                  }\n";
    let (vm, native) = both_engines(source, dir.path(), "unionbits");
    assert_eq!(vm.status, 255, "the VM read the union's low byte");
    assert_eq!(native.status, 255, "native read the union's low byte");
}

#[test]
fn a_narrow_union_write_is_visible_through_the_wide_field_in_both_engines() {
    // **This was a live miscompile.** `forward.rs`'s `compare_paths` treated two different
    // `Projection::Field` steps as disjoint storage — true for a struct, false for a union
    // where every field is at offset 0 — so the stale wide store was forwarded over the narrow
    // one and this program answered 0 instead of 7.
    //
    // Both engines consume the same forwarded MIR, so they *agreed* on the wrong answer: this
    // asserts the value, not the agreement, which is the shape ADR-0023's own tests use.
    let dir = TempDir::new().expect("a temporary directory");
    let source = "#import \"Basic\";\n\
                  \n\
                  Mixed :: union { byte: u8; word: s64; }\n\
                  \n\
                  main :: () {\n\
                  \x20   m: Mixed;\n\
                  \x20   m.word = 0;\n\
                  \x20   m.byte = 7;\n\
                  \x20   exit(m.word);\n\
                  }\n";
    let (vm, native) = both_engines(source, dir.path(), "unionfwd");
    assert_eq!(
        vm.status, 7,
        "the VM saw the narrow store through the wide field"
    );
    assert_eq!(
        native.status, 7,
        "native saw the narrow store through the wide field"
    );
}

#[test]
fn a_union_is_the_size_of_its_largest_field_in_both_engines() {
    // The layout rule, made observable without a `size_of` operator: a struct holding a union
    // and an `s64` after it places the trailing field at the union's *largest* field's width.
    // If the union were laid out as a struct — 8 + 1 rounded to 16 — the trailing field would
    // move, and writing it would land on different bytes in the two engines' frames.
    let dir = TempDir::new().expect("a temporary directory");
    let source = "#import \"Basic\";\n\
                  \n\
                  Mixed :: union { byte: u8; word: s64; }\n\
                  Pair :: struct { m: Mixed; tail: s64; }\n\
                  \n\
                  main :: () {\n\
                  \x20   p: Pair;\n\
                  \x20   p.m.word = -1;\n\
                  \x20   p.tail = 42;\n\
                  \x20   exit(p.tail);\n\
                  }\n";
    let (vm, native) = both_engines(source, dir.path(), "unionsize");
    assert_eq!(
        vm.status, 42,
        "the union's width did not overrun the trailing field in the VM"
    );
    assert_eq!(
        native.status, 42,
        "the union's width did not overrun the trailing field natively"
    );
}

#[test]
fn xx_converts_exactly_as_cast_does_in_both_engines() {
    // ADR-0046 §2's equivalence, as running programs: `xx` is sugar for a `cast` whose type was
    // written elsewhere in the statement. Each case is run *twice* — once spelled `xx`, once
    // spelled `cast` — and the two must produce the same status, which is a stronger claim than
    // either producing the right one.
    let dir = TempDir::new().expect("a temporary directory");
    let cases = [
        // Truncation: 300 in a `u8` is 44, so a conversion that did not narrow would differ.
        ("u8", "300", "u8"),
        // Widening a signed narrow type, which Jairs requires a conversion for even though it
        // is lossless (ADR-0037 §2) — the case `xx` most earns its keep on.
        ("s64", "cast(s8, 100)", "s64"),
        // Float to int, saturating rather than trapping (ADR-0040 §4).
        ("s64", "7.9", "s64"),
    ];
    for (index, (target, value, cast_to)) in cases.iter().enumerate() {
        let with_xx = format!(
            "#import \"Basic\";\n\n\
             main :: () {{\n    v := {value};\n    r: {target} = xx v;\n    exit(cast(s64, r));\n}}\n"
        );
        let with_cast = format!(
            "#import \"Basic\";\n\n\
             main :: () {{\n    v := {value};\n    r: {target} = cast({cast_to}, v);\n    \
             exit(cast(s64, r));\n}}\n"
        );
        let (xx_vm, xx_native) = both_engines(&with_xx, dir.path(), &format!("xx{index}"));
        let (cast_vm, cast_native) =
            both_engines(&with_cast, dir.path(), &format!("xxcast{index}"));
        assert_eq!(
            xx_vm.status, cast_vm.status,
            "`xx` and `cast` disagreed in the VM for {value} -> {target}"
        );
        assert_eq!(
            xx_native.status, cast_native.status,
            "`xx` and `cast` disagreed natively for {value} -> {target}"
        );
        assert_eq!(
            xx_vm.status, xx_native.status,
            "the two engines disagreed about `xx` for {value} -> {target}"
        );
    }
}

#[test]
fn a_bare_member_equals_its_qualified_form_in_both_engines() {
    // ADR-0046 §3: `.RED` and `Colour.RED` must produce the identical constant, because they
    // differ only in *how the enum was found* — a difference sema has resolved before MIR runs.
    // Written as one program comparing the two spellings, so a mismatch is an exit status rather
    // than a comparison of two runs.
    let dir = TempDir::new().expect("a temporary directory");
    let source = "#import \"Basic\";\n\
                  \n\
                  Colour :: enum { RED; GREEN; BLUE; }\n\
                  \n\
                  main :: () {\n\
                  \x20   bare: Colour = .BLUE;\n\
                  \x20   qualified := Colour.BLUE;\n\
                  \x20   if bare == qualified {\n\
                  \x20       exit(cast(s64, bare));\n\
                  \x20   }\n\
                  \x20   exit(99);\n\
                  }\n";
    let (vm, native) = both_engines(source, dir.path(), "baremember");
    assert_eq!(
        vm.status, 2,
        "the VM agreed the two spellings are one value"
    );
    assert_eq!(
        native.status, 2,
        "native agreed the two spellings are one value"
    );
}

#[test]
fn a_bare_member_reaches_a_call_argument_and_a_comparison_in_both_engines() {
    // ADR-0041 §2's step 4 and ADR-0046 §3: the two contexts a Jai programmer tries first. Both
    // work because `check_operands` and the call path already thread a context — machinery that
    // predates this wave — so this asserts that the context genuinely *reaches* the member
    // rather than that a new rule fires.
    let dir = TempDir::new().expect("a temporary directory");
    let source = "#import \"Basic\";\n\
                  \n\
                  Colour :: enum { RED; GREEN; }\n\
                  \n\
                  is_green :: (c: Colour) -> bool {\n\
                  \x20   return c == .GREEN;\n\
                  }\n\
                  \n\
                  main :: () {\n\
                  \x20   n := 0;\n\
                  \x20   if is_green(.GREEN) { n = n + 1; }\n\
                  \x20   if !is_green(.RED) { n = n + 2; }\n\
                  \x20   exit(n);\n\
                  }\n";
    let (vm, native) = both_engines(source, dir.path(), "bareargs");
    assert_eq!(
        vm.status, 3,
        "the VM reached the member through both contexts"
    );
    assert_eq!(
        native.status, 3,
        "native reached the member through both contexts"
    );
}

#[test]
fn an_operator_overload_runs_identically_in_both_engines() {
    // ADR-0048 §5: an overload lowers to an ordinary direct call, so a disagreement here would be
    // about the *call* rather than the operator — which is exactly why it is worth asserting.
    // Both engines consume the same MIR, so this asserts the value, not merely the agreement.
    //
    // The overload returns a **scalar**, deliberately: the native back end cannot return an
    // aggregate at all, so a `Vec2`-returning `operator +` would pass under `jr run` and fail
    // `jr build`, testing the pre-existing hole rather than the operator.
    let dir = TempDir::new().expect("a temporary directory");
    let source = "#import \"Basic\";\n\
                  \n\
                  Vec2 :: struct { x: s64; y: s64; }\n\
                  \n\
                  operator + :: (a: Vec2, b: Vec2) -> s64 {\n\
                  \x20   return a.x + b.x + a.y + b.y;\n\
                  }\n\
                  \n\
                  main :: () {\n\
                  \x20   p: Vec2;\n\
                  \x20   p.x = 1;\n\
                  \x20   p.y = 2;\n\
                  \x20   q: Vec2;\n\
                  \x20   q.x = 10;\n\
                  \x20   q.y = 20;\n\
                  \x20   exit(p + q);\n\
                  }\n";
    let (vm, native) = both_engines(source, dir.path(), "overload");
    assert_eq!(vm.status, 33, "the VM called the overload");
    assert_eq!(native.status, 33, "native called the overload");
}

#[test]
fn a_mixed_type_overload_resolves_per_operand_order_in_both_engines() {
    // ADR-0048 §4's no-ranking rule, which is only *visible* when both orders are written and
    // differ: `Vec2 * s64` doubles and `s64 * Vec2` triples, so a resolver that ignored operand
    // order — or that ranked one as a fallback for the other — would produce a different sum.
    let dir = TempDir::new().expect("a temporary directory");
    let source = "#import \"Basic\";\n\
                  \n\
                  Vec2 :: struct { x: s64; }\n\
                  \n\
                  operator * :: (a: Vec2, b: s64) -> s64 {\n\
                  \x20   return a.x * b * 2;\n\
                  }\n\
                  \n\
                  operator * :: (a: s64, b: Vec2) -> s64 {\n\
                  \x20   return a * b.x * 3;\n\
                  }\n\
                  \n\
                  main :: () {\n\
                  \x20   v: Vec2;\n\
                  \x20   v.x = 5;\n\
                  \x20   exit(v * 1 + 1 * v);\n\
                  }\n";
    let (vm, native) = both_engines(source, dir.path(), "overloadorder");
    assert_eq!(vm.status, 25, "the VM picked an overload per operand order");
    assert_eq!(
        native.status, 25,
        "native picked an overload per operand order"
    );
}

#[test]
fn a_builtin_operator_is_unaffected_by_an_overload_in_scope() {
    // ADR-0048 §4: a builtin meaning always wins, and §3's orphan rule is what guarantees it —
    // no overload can exist for two builtin types, so `s64 + s64` cannot find one. This program
    // declares an overload *and* does builtin arithmetic, so a lookup that matched too eagerly
    // would change the answer.
    let dir = TempDir::new().expect("a temporary directory");
    let source = "#import \"Basic\";\n\
                  \n\
                  Vec2 :: struct { x: s64; }\n\
                  \n\
                  operator + :: (a: Vec2, b: Vec2) -> s64 {\n\
                  \x20   return 99;\n\
                  }\n\
                  \n\
                  main :: () {\n\
                  \x20   exit(7 + 5);\n\
                  }\n";
    let (vm, native) = both_engines(source, dir.path(), "overloadbuiltin");
    assert_eq!(vm.status, 12, "the VM used builtin addition");
    assert_eq!(native.status, 12, "native used builtin addition");
}
