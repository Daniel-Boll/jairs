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
