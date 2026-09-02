//! Integration tests for `jr-cli`.
//!
//! These tests call the command functions directly (no subprocess) and assert
//! exit codes and output.  Corpus files are copied into a temporary directory
//! so the originals are never mutated.

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Copy a corpus file into `dir`, returning the destination path.
fn copy_corpus(src: &str, dir: &Path) -> PathBuf {
    let src_path = corpus_path(src);
    let dest = dir.join(Path::new(src).file_name().unwrap());
    fs::copy(&src_path, &dest).unwrap_or_else(|e| panic!("copy {src}: {e}"));
    dest
}

/// Absolute path to a corpus file relative to the workspace root.
fn corpus_path(rel: &str) -> PathBuf {
    // crates/jr-cli/tests/ → workspace root is three levels up.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.join("../../tests/corpus").join(rel)
}

/// Run `jr check` and return the exit code.
fn run_check(paths: Vec<PathBuf>) -> i32 {
    let global = quiet_global();
    let args = jr_cli::cli::CheckArgs {
        paths,
        module_paths: Vec::new(),
    };
    jr_cli::commands::check::run(args, &global).unwrap()
}

/// Run `jr fmt` (in-place or --check) and return the exit code.
fn run_fmt(paths: Vec<PathBuf>, check: bool) -> i32 {
    let global = quiet_global();
    let args = jr_cli::cli::FmtArgs {
        check,
        stdin: false,
        paths,
    };
    jr_cli::commands::fmt::run(args, &global).unwrap()
}

/// A quiet global args instance for tests (no colour, quiet mode).
fn quiet_global() -> jr_cli::cli::GlobalArgs {
    jr_cli::cli::GlobalArgs {
        color: jr_cli::cli::ColorChoice::Never,
        quiet: true,
        verbose: false,
    }
}

// ---------------------------------------------------------------------------
// jr check
// ---------------------------------------------------------------------------

#[test]
fn check_valid_file_exits_zero() {
    let dir = TempDir::new().unwrap();
    let path = copy_corpus("valid/024-hello.jr", dir.path());
    assert_eq!(run_check(vec![path]), 0);
}

#[test]
fn check_invalid_file_exits_one() {
    let dir = TempDir::new().unwrap();
    let path = copy_corpus("invalid/009-multiple-independent-errors.jr", dir.path());
    assert_eq!(run_check(vec![path]), 1);
}

#[test]
fn check_nonexistent_path_returns_io_error() {
    let global = quiet_global();
    let args = jr_cli::cli::CheckArgs {
        paths: vec![PathBuf::from("/nonexistent/path/that/does/not/exist.jr")],
        module_paths: Vec::new(),
    };
    let result = jr_cli::commands::check::run(args, &global);
    assert!(result.is_err(), "expected Err for nonexistent path");
}

#[test]
fn check_directory_expands_to_jr_files() {
    let dir = TempDir::new().unwrap();
    // Copy a few valid files into a subdirectory.
    let sub = dir.path().join("sub");
    fs::create_dir(&sub).unwrap();
    copy_corpus("valid/003-proc-empty.jr", &sub);
    copy_corpus("valid/007-constants.jr", &sub);
    // Also put a non-.jr file that should be ignored.
    fs::write(sub.join("ignore.txt"), "not jairs").unwrap();

    let code = run_check(vec![sub]);
    assert_eq!(code, 0);
}

// ---------------------------------------------------------------------------
// jr fmt --check
// ---------------------------------------------------------------------------

/// Format a file and return the formatted text (using the real formatter).
fn format_text(text: &str, path: &Path) -> String {
    let mut map = jr_base::SourceMap::new();
    let file_id = map.add(path, text);
    let config = jr_fmt::Config::default();
    jr_fmt::format(text, file_id, &config).unwrap_or_else(|_| text.to_owned())
}

#[test]
fn fmt_check_already_formatted_exits_zero() {
    // Write a file that is already in the formatter's canonical form.
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("already_formatted.jr");

    // Use a simple file that the formatter leaves unchanged.
    let src = "noop :: () {}\n";
    // First format it to get the canonical form.
    let canonical = format_text(src, &path);
    fs::write(&path, &canonical).unwrap();

    // Now fmt --check should exit 0.
    assert_eq!(run_fmt(vec![path], true), 0);
}

#[test]
fn fmt_check_unformatted_exits_one_and_does_not_modify_file() {
    // Write a file that the formatter will change.
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("unformatted.jr");

    // A file with extra spaces that the formatter normalises.
    let unformatted = "noop  ::  ()  {}\n";
    fs::write(&path, unformatted).unwrap();

    // Check whether the formatter actually changes this.
    let formatted = format_text(unformatted, &path);
    if formatted == unformatted {
        // Formatter leaves it unchanged — skip this test variant.
        // (The stub formatter is identity; the real one may differ.)
        return;
    }

    let code = run_fmt(vec![path.clone()], true);
    assert_eq!(code, 1, "fmt --check should exit 1 for unformatted file");

    // The file must NOT have been modified.
    let after = fs::read_to_string(&path).unwrap();
    assert_eq!(unformatted, after, "fmt --check must not modify the file");
}

#[test]
fn fmt_invalid_file_exits_one_and_leaves_file_untouched() {
    let dir = TempDir::new().unwrap();
    let path = copy_corpus("invalid/001-missing-semicolon.jr", dir.path());
    let original = fs::read_to_string(&path).unwrap();

    // In-place fmt on a file that does not parse should exit 1 and not write.
    let code = run_fmt(vec![path.clone()], false);
    assert_eq!(code, 1, "fmt on unparseable file should exit 1");

    let after = fs::read_to_string(&path).unwrap();
    assert_eq!(original, after, "fmt must not modify an unparseable file");
}

#[test]
fn fmt_inplace_rewrites_file() {
    // Write a file that the formatter will change, then verify it is rewritten.
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("to_format.jr");

    // A file with extra spaces that the formatter normalises.
    let unformatted = "noop  ::  ()  {}\n";
    fs::write(&path, unformatted).unwrap();

    // Check whether the formatter actually changes this.
    let formatted = format_text(unformatted, &path);
    if formatted == unformatted {
        // Formatter is identity for this input — skip.
        return;
    }

    let code = run_fmt(vec![path.clone()], false);
    assert_eq!(code, 0, "fmt in-place should exit 0 on valid file");

    let after = fs::read_to_string(&path).unwrap();
    assert_eq!(
        formatted, after,
        "fmt should rewrite the file to canonical form"
    );
}

#[test]
fn fmt_inplace_valid_file_exits_zero() {
    // A valid file that is already formatted should exit 0 and be unchanged.
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("valid.jr");
    let src = "noop :: () {}\n";
    let canonical = format_text(src, &path);
    fs::write(&path, &canonical).unwrap();

    let code = run_fmt(vec![path.clone()], false);
    assert_eq!(code, 0);

    let after = fs::read_to_string(&path).unwrap();
    assert_eq!(canonical, after);
}

// ---------------------------------------------------------------------------
// jr parse
// ---------------------------------------------------------------------------

#[test]
fn parse_valid_file_exits_zero() {
    let dir = TempDir::new().unwrap();
    let path = copy_corpus("valid/024-hello.jr", dir.path());
    let global = quiet_global();
    let args = jr_cli::cli::ParseArgs {
        path,
        dump: false,
        tokens: false,
    };
    let code = jr_cli::commands::parse::run(args, &global).unwrap();
    assert_eq!(code, 0);
}

#[test]
fn parse_invalid_file_exits_one() {
    let dir = TempDir::new().unwrap();
    let path = copy_corpus("invalid/002-unclosed-brace.jr", dir.path());
    let global = quiet_global();
    let args = jr_cli::cli::ParseArgs {
        path,
        dump: false,
        tokens: false,
    };
    let code = jr_cli::commands::parse::run(args, &global).unwrap();
    assert_eq!(code, 1);
}

// ---------------------------------------------------------------------------
// Module loading through the CLI
// ---------------------------------------------------------------------------

fn check_with_modules(paths: Vec<std::path::PathBuf>, module_dir: Option<&str>) -> i32 {
    let args = jr_cli::cli::CheckArgs {
        paths,
        module_paths: module_dir.map(|d| vec![corpus_path(d)]).unwrap_or_default(),
    };
    jr_cli::commands::check::run(args, &quiet_global()).expect("check must not fail with I/O")
}

/// Every well-formed multi-module program must check cleanly once modules can be
/// found. This is the property the whole wave exists to deliver.
#[test]
fn imports_valid_corpus_checks_cleanly() {
    let code = check_with_modules(vec![corpus_path("imports/valid")], Some("modules"));
    assert_eq!(code, 0, "all imports/valid files must check cleanly");
}

/// A cycle between two modules must be accepted, not rejected (ADR-0014 §4).
#[test]
fn import_cycle_checks_cleanly_through_the_cli() {
    let code = check_with_modules(
        vec![corpus_path("imports/valid/005-import-cycle-is-legal.jr")],
        Some("modules"),
    );
    assert_eq!(code, 0, "import cycles are legal and must not error");
}

/// Without the module path, the same file must fail — otherwise the previous
/// test proves nothing about module loading actually happening.
#[test]
fn imports_fail_when_the_module_path_is_absent() {
    let code = check_with_modules(
        vec![corpus_path("imports/valid/001-import-directory-module.jr")],
        None,
    );
    assert_eq!(
        code, 1,
        "with no --module-path the module cannot be found, so this must fail; \
         if it passes, module resolution is not being exercised at all"
    );
}

/// The semantic failure cases must fail.
#[test]
fn imports_invalid_corpus_fails() {
    for file in [
        "imports/invalid/001-module-not-found.jr",
        "imports/invalid/002-ambiguous-imported-name.jr",
        "imports/invalid/003-unresolved-after-import.jr",
        // `using` refusals (ADR-0050 §5, §3, §1). Filed here rather than under `type-errors/`
        // because E0250 is a **resolution** diagnostic and that directory's contract is that its
        // files "parse, lower and resolve cleanly" — a rule this wave met by moving the files
        // rather than by weakening it. These three happen to import nothing, which the directory
        // permits: its contract is about the *stage* the error comes from, not about imports.
        "imports/invalid/004-using-on-a-union.jr",
        "imports/invalid/005-using-ambiguous.jr",
        "imports/invalid/006-using-on-a-non-struct.jr",
        // Multiple-return arity (ADR-0052 §2) and the named-argument rules (ADR-0053 §3).
        "imports/invalid/007-multiple-returns-arity.jr",
        "imports/invalid/008-named-argument-rules.jr",
        // A name an imported module declares but does not export (ADR-0054 §2).
        "imports/invalid/009-not-exported.jr",
        // An *imported* `#foreign` procedure installed into a procedure-pointer field (ADR-0062 §3).
        // Here rather than under `type-errors/` because reaching the case needs the import resolved,
        // and a same-file version tests a path that already worked.
        "imports/invalid/010-foreign-allocator.jr",
        // `#insert` with no string-literal operand (ADR-0072 §5). Here for the same stage reason as
        // the `using` refusals above: E0262 comes out of **lowering**, so `type-errors/`' harness would
        // fail it for not lowering cleanly before ever checking the code it declares.
        "imports/invalid/011-insert-needs-a-literal.jr",
        // A `#run` whose callee reads an **imported constant** (ADR-0073 §4). Here because reaching the
        // case needs the import resolved, and E0230 is `jr-db`'s code — no corpus directory holds one.
        // What this pins is the *diagnostic*: it used to be "internal compiler error: no routine for
        // file 0 proc 0", the third instance of internals leaking for a reasonable program.
        "imports/invalid/012-run-reads-an-imported-constant.jr",
        // A `$N` comptime-value argument that is not a compile-time constant (ADR-0088 §2). Here for
        // the same stage reason as `012`: E0271 comes out of `jr-db`'s const-eval pre-pass, so the
        // sema `type-errors/` harness cannot see it.
        "imports/invalid/013-comptime-arg-not-constant.jr",
        // An early `return` in a `#expand` macro body (ADR-0090 §2). Here for the same stage reason as
        // E0262: E0273 comes out of **lowering**, so `type-errors/`' harness would fail it for not
        // lowering cleanly before ever checking the code it declares.
        "imports/invalid/014-macro-early-return.jr",
        // An instantiation rejected by its `#modify` predicate (ADR-0095 §1). Here because E0275 is
        // `jr-db`'s code — the predicate runs in `file_mir`, the only place with the expanded tree, its MIR
        // and the VM — so the sema `type-errors/` harness cannot see it.
        "imports/invalid/015-modify-rejects.jr",
        // `#bake_arguments`, whose specialisation is not yet built (ADR-0096 §3). Here for the stage reason
        // E0262 is: E0276 comes out of **lowering**, so `type-errors/`' harness would fail it for not
        // lowering cleanly before ever checking the code it declares.
        "imports/invalid/016-bake-arguments.jr",
        // A call to a polymorphic procedure declared in **another module** (ADR-0104 §2). Here because
        // reaching the case needs the import resolved — nothing in the corpus had ever imported a template,
        // which is why the refusal did not exist: the call type-checked (a `$T` parameter's type is
        // `PoolId::ERROR`, and `ERROR` matches anything) and the missing instantiation leaked out of an
        // engine as "no routine for file 2 proc 0".
        "imports/invalid/017-cross-file-instantiation.jr",
    ] {
        let code = check_with_modules(vec![corpus_path(file)], Some("modules"));
        assert_eq!(code, 1, "{file} must report an error");
    }
}

/// Every file in `imports/invalid/` must be in the list above.
///
/// **This exists because three files were not.** ADR-0052's and ADR-0053's refusal files were added
/// to the directory and the list edits silently failed to apply, so for two waves the directory grew
/// while the test did not — and ADR-0054's filter could be disabled with the whole suite still
/// green. A hand-maintained list of files in a directory is a list that drifts; this is the check
/// that makes the drift a failure.
#[test]
fn every_imports_invalid_file_is_exercised() {
    let dir = corpus_path("imports/invalid");
    let mut found: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jr"))
        .filter_map(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .collect();
    found.sort();

    let source = include_str!("integration.rs");
    let mut missing = Vec::new();
    for name in &found {
        if !source.contains(&format!("imports/invalid/{name}")) {
            missing.push(name.clone());
        }
    }
    assert!(
        missing.is_empty(),
        "these imports/invalid files are not exercised by `imports_invalid_corpus_fails`: {missing:?}"
    );
}

/// `024-hello.jr` imports the real bundled `Basic` module, with no
/// `--module-path` at all. This is the end-to-end check that the compiler's own
/// standard library is reachable by default.
#[test]
fn the_slice_program_resolves_against_the_bundled_stdlib() {
    let code = check_with_modules(vec![corpus_path("valid/024-hello.jr")], None);
    assert_eq!(
        code, 0,
        "the slice program must resolve `print` from the bundled Basic module"
    );
}

// ---------------------------------------------------------------------------
// Type checking through the CLI
// ---------------------------------------------------------------------------

/// The whole `valid/` corpus must check cleanly with no `--module-path` at all:
/// the two files that import `Basic` resolve it in the bundled module directory.
///
/// This is the acceptance test for the type checker. The corpus constrains sema
/// only negatively — no file in `valid/` expects a diagnostic — so "silence" is
/// the property, and it is asserted over the whole directory because a single
/// stray error anywhere is a regression.
#[test]
fn valid_corpus_checks_cleanly() {
    let code = check_with_modules(vec![corpus_path("valid")], None);
    assert_eq!(code, 0, "every file in valid/ must type-check silently");
}

/// The fixture modules are libraries, not test cases: they must check cleanly on
/// their own terms (`tests/corpus/README.md`).
#[test]
fn fixture_modules_check_cleanly() {
    let code = check_with_modules(vec![corpus_path("modules")], Some("modules"));
    assert_eq!(code, 0, "the fixture modules must type-check silently");
}

/// And the positive half: every file in `type-errors/` must be rejected.
///
/// A `#run` returning a **union** is refused, and it is the one aggregate shape that must be (ADR-0074 §4).
///
/// A union is untagged (ADR-0045 §1), so its bytes do not say which field is live — and an aggregate
/// constant is interned as its *element values*, which would mean picking one silently. That is the
/// reinterpretation ADR-0045 allows only for a runtime read the programmer wrote, never for a value the
/// compiler manufactures.
///
/// Not a corpus file: E0230 is `jr-db`'s const-eval code, and no corpus directory holds one — `type-errors/`
/// is for `jr-sema` and `cfg-errors/` for `jr-mir`, so filing it in either would break that directory's
/// stage contract. Checked here by exit code instead.
#[test]
fn a_run_returning_a_union_is_refused() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("union_const.jr");
    fs::write(
        &path,
        "U :: union { a: s64; b: s64; }\n\
         mk :: () -> U { u: U; u.a = 1; return u; }\n\
         V :: #run mk();\n\
         main :: () { }\n",
    )
    .unwrap();
    assert_eq!(
        check_with_modules(vec![path], None),
        1,
        "a union constant has no defined field to read, so it must be refused"
    );
}

/// A **pointer** in a compile-time aggregate must be refused, not interned as a number.
///
/// This pins a silent miscompile that shipped and was found by probing. `reduce_element` treated a
/// pointer as a scalar, so the VM's own address was interned as a plain integer; reading `V.p.*`
/// afterwards gave **48** in the VM and a **segfault** natively — two different wrong answers, neither
/// reported. ADR-0074 §2 already refused `string` as an aggregate element on exactly this ground ("its
/// runtime form is a pointer, which has no compile-time value"); the rule simply had not been extended.
///
/// A CLI exit-code test rather than a corpus file, because E0230 is `jr-db`'s code and no corpus
/// directory holds one — `type-errors/` is `jr-sema`'s and `cfg-errors/` is `jr-mir`'s.
#[test]
fn a_pointer_in_a_compile_time_aggregate_is_refused() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("ptr_const.jr");
    fs::write(
        &path,
        "H :: struct { p: *s64; n: s64; }\n\
         mk :: () -> H { v: s64; v = 42; h: H; h.p = *v; h.n = 7; return h; }\n\
         V :: #run mk();\n\
         main :: () { }\n",
    )
    .unwrap();
    assert_eq!(
        check_with_modules(vec![path], None),
        1,
        "a compile-time pointer addresses the evaluator's memory, so it must be refused"
    );
}

/// A **non-terminating compile-time loop** must be refused rather than hang the compiler (ADR-0121).
///
/// Before the step budget this hung `jr check` outright, with no diagnostic and no way out but a signal —
/// and under `jr lsp` it hung the single worker thread on a file the user had merely *opened*, because
/// salsa's cancellation cannot reach a loop that never touches the database. So the blast radius was much
/// larger than "the compiler is slow on a silly program": opening a repository was enough.
///
/// A CLI exit-code test rather than a corpus file for the reason the two above are: E0230 is `jr-db`'s code
/// and no corpus directory holds one. It also must *not* be a corpus file — every corpus program is
/// executed by the differential harness, and this one is designed never to finish.
#[test]
fn a_non_terminating_compile_time_loop_is_refused() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("hang.jr");
    fs::write(
        &path,
        "spin :: () -> s64 { n := 0; while true { n = n + 1; } return n; }\n\
         HANG :: #run spin();\n\
         main :: () { }\n",
    )
    .unwrap();
    assert_eq!(
        check_with_modules(vec![path], None),
        1,
        "a `#run` that never terminates must exhaust the step budget and report, not hang"
    );
}

/// A **long but terminating** compile-time loop must still be evaluated (ADR-0121).
///
/// The other half of the budget, and the one that says it is not merely a small number: a hundred thousand
/// iterations is far more than any constant a real program folds, and it must fold. Without this the budget
/// could be lowered until it broke legitimate work and no test would notice.
#[test]
fn a_long_but_terminating_compile_time_loop_still_folds() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("long.jr");
    fs::write(
        &path,
        "count :: () -> s64 { n := 0; i := 0; while i < 100000 { n = n + 1; i = i + 1; } return n; }\n\
         TOTAL :: #run count();\n\
         main :: () { }\n",
    )
    .unwrap();
    assert_eq!(
        check_with_modules(vec![path], None),
        0,
        "100_000 iterations is well inside the budget and must fold"
    );
}

/// A **view** in a compile-time aggregate must be refused, for the same reason a pointer is.
///
/// A view is `{data, count}`, and its `data` word is a pointer into the evaluator's memory. Before the
/// refusal the two words were interned as one 8-byte integer, so the count survived and the pointer did
/// not — a read through it trapped with "index out of bounds", which looked like a program bug rather
/// than the compiler's.
#[test]
fn a_view_in_a_compile_time_aggregate_is_refused() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("view_const.jr");
    fs::write(
        &path,
        "H :: struct { items: []s64; n: s64; }\n\
         mk :: () -> H { buf: [2]s64; buf[0] = 7; h: H; h.items = buf[]; h.n = 2; return h; }\n\
         V :: #run mk();\n\
         main :: () { }\n",
    )
    .unwrap();
    assert_eq!(
        check_with_modules(vec![path], None),
        1,
        "a compile-time view's data pointer addresses the evaluator's memory, so it must be refused"
    );
}

/// Rejected *by sema*, not by the parser — `jr-sema`'s corpus test asserts these
/// files parse cleanly, so a parser-caused failure would show up there first.
#[test]
fn type_error_corpus_is_rejected() {
    let dir = corpus_path("type-errors");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .expect("the type-errors corpus must exist")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("jr"))
        .collect();
    files.sort();
    assert!(files.len() >= 12, "the type-errors corpus has shrunk");

    for file in files {
        let code = check_with_modules(vec![file.clone()], None);
        assert_eq!(code, 1, "{} must be rejected", file.display());
    }
}

// ---------------------------------------------------------------------------
// jr run — PLAN.md §1.4's exit criterion, VM half
// ---------------------------------------------------------------------------

/// Run `jr run` on a corpus file and return the exit code.
fn run_program(path: PathBuf) -> i32 {
    let global = quiet_global();
    let args = jr_cli::cli::RunArgs {
        path,
        module_paths: Vec::new(),
        no_bounds_check: false,
        opt_level: jr_cli::cli::OptLevelArg::Standard,
    };
    jr_cli::commands::run::run(args, &global).unwrap()
}

#[test]
fn run_executes_the_slice_exit_criterion() {
    // `024-hello.jr` is the program `PLAN.md` §1.4 names. Running it exercises a
    // folded `#run`, a folded string constant, a struct through a slot, a cross-file
    // call into `modules/Basic`, a `while` loop with a block parameter, a pointer, and
    // a foreign call to libc `write` — all at once.
    //
    // The output itself is asserted in `jr-vm`'s own tests, which can capture it; here
    // the exit code is what matters, because it is what a build script sees.
    let dir = TempDir::new().unwrap();
    let path = copy_corpus("valid/024-hello.jr", dir.path());
    assert_eq!(run_program(path), 0, "the exit criterion must run cleanly");
}

#[test]
fn run_executes_a_program_with_no_imports() {
    let dir = TempDir::new().unwrap();
    let path = copy_corpus("valid/020-run-directive.jr", dir.path());
    assert_eq!(run_program(path), 0);
}

#[test]
fn run_refuses_a_file_with_errors_and_does_not_execute_it() {
    // ADR-0017 §4: no MIR from a file with errors, so no bytecode either. Exit 1 is
    // `jr check`'s code for the same condition, deliberately — the two commands must
    // never disagree about whether a program is valid.
    let dir = TempDir::new().unwrap();
    let path = copy_corpus("type-errors/005-call-arity.jr", dir.path());
    assert_eq!(run_program(path), 1);
}

#[test]
fn run_reports_a_program_with_no_main() {
    // Jairs-0 has no entry-point attribute, so a file with no `main` is a usage error
    // rather than something to guess at.
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("nomain.jr");
    fs::write(&path, "helper :: () -> s64 { return 1; }\n").unwrap();
    let global = quiet_global();
    let args = jr_cli::cli::RunArgs {
        path,
        module_paths: Vec::new(),
        no_bounds_check: false,
        opt_level: jr_cli::cli::OptLevelArg::Standard,
    };
    let error = jr_cli::commands::run::run(args, &global).expect_err("must not run");
    assert!(
        error.to_string().contains("main"),
        "the error must say what is missing: {error}"
    );
}

// ---------------------------------------------------------------------------
// jr bench (ADR-0033)
// ---------------------------------------------------------------------------

/// `jr bench` runs and produces a finite number for every operation.
///
/// A smoke test and deliberately nothing more: ADR-0033 §4 refuses a timing assertion,
/// because a duration compared against a threshold on a shared machine fails for reasons
/// that have nothing to do with the code, and this project's gates are meant to be
/// believable.
///
/// What it does guard is the harness rotting into something that reports zeros — which is
/// the failure mode a measurement tool actually has. The first draft of `jr bench` measured
/// a cursor sitting inside the `return` keyword of `024-hello.jr`, so `references` and
/// `rename` took their "nothing here" early return and reported **0.002 ms**: a workspace
/// scan that never happened, presented as a very fast one. Deriving the cursor from the file
/// fixed it, and this test is what would notice if it regressed.
#[test]
fn bench_reports_a_number_for_every_operation() {
    let dir = TempDir::new().expect("a temporary directory");
    let file = copy_corpus("valid/024-hello.jr", dir.path());

    let args = jr_cli::cli::BenchArgs {
        file,
        paths: Vec::new(),
        throughput: false,
        // Two, because the assertion is about shape rather than about statistics; twenty
        // would make this test the slowest in the suite for no extra confidence.
        iterations: 2,
        module_paths: Vec::new(),
    };
    let code = jr_cli::commands::bench::run(args, &quiet_global()).expect("bench must run");
    assert_eq!(code, 0, "bench reports, it does not judge — it cannot fail");
}

/// The cursor `jr bench` measures at lands on a declaration, not on a keyword.
///
/// This is the assertion that actually has teeth. `references` and `rename` are keyed on a
/// `DefId` (ADR-0030 §1), so a position that yields none makes both return immediately and
/// report a sub-microsecond "scan" — the exact wrong answer, and one that looks like good
/// news. Pinning the *position* pins the meaning of half the table.
#[test]
fn bench_measures_at_a_declaration_and_not_inside_a_keyword() {
    let text = fs::read_to_string(corpus_path("valid/024-hello.jr")).expect("readable");
    let at = jr_cli::commands::bench::cursor_for_test(&text);
    let line = text
        .lines()
        .nth(at.line as usize)
        .expect("the chosen line must exist");
    assert!(
        line.contains(" :: ") || line.contains(" := "),
        "line {} is {line:?}, which is not a declaration",
        at.line
    );
    // Column 0 of a top-level declaration is its name. An indented `::` would be inside a
    // body, where the name may be a local rather than an item.
    assert_eq!(at.character, 0);
    assert!(
        !line.starts_with(char::is_whitespace),
        "a top-level declaration starts at column 0: {line:?}"
    );
}

#[test]
fn a_refused_body_is_a_diagnostic_rather_than_a_crash() {
    // ADR-0047 §2. **This replaced an internal compiler error surfaced to the user**: a body
    // MIR could not lower was skipped when the program was assembled, and calling one reached
    // the interpreter's own lookup — `internal compiler error: no routine for file 0 proc 0`,
    // on a program `jr check` had just called clean.
    //
    // The construct is a **directive in a body** — `h := #system_library "c";` — and it is chosen
    // because ADR-0016 §3 refuses it *by design*: a directive has an opaque handle type and no runtime
    // value, so no wave will make this lower. That matters, because this test has now had its construct
    // replaced **twice** for the same reason:
    //
    //   * first an imported constant, chosen "so this test survives that fix rather than dying with
    //     it" — and ADR-0055 was that fix;
    //   * then a `#run` inside a body, described here as staying "refused for several waves yet" —
    //     and ADR-0069 §2 made it work one wave later.
    //
    // Both comments predicted their own obsolescence and both predictions came true immediately. The
    // lesson, now paid for twice: a test that needs a refused body must name something refused **by
    // design**, not something merely unimplemented — because every unimplemented thing is one wave
    // from working, and the test's real subject is the crash, not the gap.
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("main.jr");
    std::fs::write(
        &path,
        "#import \"Basic\";\n\nmain :: () {\n    h := #system_library \"c\";\n    exit(0);\n}\n",
    )
    .unwrap();

    // `check` **warns** rather than erroring, and the severity is deliberate: a refused body
    // nobody calls does not stop a program, and six files in `tests/corpus/imports/valid/` have
    // been in that state since they were written. Making it an error would reject programs that
    // work today.
    let global = quiet_global();
    let check = jr_cli::commands::check::run(
        jr_cli::cli::CheckArgs {
            paths: vec![path.clone()],
            module_paths: vec![dir.path().to_path_buf()],
        },
        &global,
    )
    .unwrap();
    assert_eq!(check, 0, "a refused body is a warning, not an error");

    // But `main` is called by definition, so running it must fail — with a message naming the
    // procedure and saying whose fault it is, rather than the `no routine for file 0 proc 0`
    // internal error this replaced.
    let run = jr_cli::commands::run::run(
        jr_cli::cli::RunArgs {
            path,
            module_paths: vec![dir.path().to_path_buf()],
            no_bounds_check: false,
            opt_level: jr_cli::cli::OptLevelArg::Standard,
        },
        &global,
    );
    let message = match run {
        Ok(status) => panic!("running a refused `main` must fail, got status {status}"),
        Err(e) => e.to_string(),
    };
    assert!(
        message.contains("could not lower `main`"),
        "the failure must name `main`: {message}"
    );
    assert!(
        !message.contains("no routine for"),
        "the internal compiler error must not reach the user: {message}"
    );
}

// ---------------------------------------------------------------------------
// jr build — a build script naming its own artefact (ADR-0102)
// ---------------------------------------------------------------------------

/// Build `path`, returning the exit code. `output` is `-o` when present.
fn run_build(path: PathBuf, output: Option<PathBuf>) -> i32 {
    jr_cli::commands::build::run(
        jr_cli::cli::BuildArgs {
            path,
            output,
            emit_object: false,
            backend: jr_cli::cli::BackendArg::Cranelift,
            no_bounds_check: false,
            // `None` so this helper exercises the *default* path, which is now "a declared
            // BUILD_OPT_LEVEL, else Standard" (ADR-0154 §1). A test wanting an explicit level passes the
            // flag through the binary instead, as the opt-level test below does.
            opt_level: None,
            module_paths: vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../modules")],
            // Empty: these two tests link only against libc, which the driver finds on its own.
            library_paths: Vec::new(),
        },
        &quiet_global(),
    )
    .expect("build should not fail at the io layer")
}

/// `BUILD_OPT_LEVEL :: 0;` is honoured, and `-O` outranks it (ADR-0154 §1).
///
/// Asserted through the **backtrace**, which is the one observable difference between the levels
/// (ADR-0142): at `-O0` nothing is inlined, so a trap inside a leaf names the leaf's own line and lists
/// its own frame, while at `-O1` the leaf is inlined and the trap names the call site. So this test reads
/// what the *program* printed rather than any message from the driver — the same discipline
/// `build_output_constant_names_the_executable` uses on the file that appears.
#[test]
fn a_declared_opt_level_is_honoured_and_a_flag_outranks_it() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("lvl.jr");
    // A trap inside a one-line leaf: at -O0 the backtrace names `boom`, at -O1 it is inlined away.
    fs::write(
        &source,
        "#import \"Basic\";\nBUILD_OPT_LEVEL :: 0;\n\
         boom :: (a: s64) -> s64 {\n    return a / 0;\n}\n\
         main :: () {\n    exit(boom(1));\n}\n",
    )
    .unwrap();

    let out = dir.path().join("lvl_bin");
    assert_eq!(run_build(source.clone(), Some(out.clone())), 0);
    let declared = std::process::Command::new(&out)
        .output()
        .expect("the binary should run");
    let declared_err = String::from_utf8_lossy(&declared.stderr).into_owned();
    assert!(
        declared_err.contains("in boom"),
        "a declared level of 0 should leave the leaf un-inlined, so its frame is named: {declared_err}"
    );

    // The same program built with an explicit `-O1`: the operator's instruction outranks the artefact's
    // declaration (ADR-0102 §2's asymmetry), so the leaf is inlined and its frame is gone.
    let out2 = dir.path().join("lvl_bin_o1");
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_jr"))
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(&out2)
        .arg("-O")
        .arg("1")
        .arg("-I")
        .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../modules"))
        .status()
        .expect("build should not fail at the io layer");
    assert!(status.success());
    let flagged = std::process::Command::new(&out2)
        .output()
        .expect("the binary should run");
    let flagged_err = String::from_utf8_lossy(&flagged.stderr).into_owned();
    assert!(
        !flagged_err.contains("in boom"),
        "an explicit -O1 should outrank the declaration and inline the leaf: {flagged_err}"
    );
}

#[test]
fn build_output_constant_names_the_executable() {
    // The claim: `BUILD_OUTPUT :: #run choose();` names the artefact, so a build script does the
    // makefile's most basic job. Asserted on the *file that appears*, not on any message, because the
    // whole point is that the driver acted on a value the program computed.
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("prog.jr");
    fs::write(
        &source,
        "#import \"Basic\";\nchoose :: () -> string { return \"named_by_script\"; }\n\
         BUILD_OUTPUT :: #run choose();\nmain :: () {\n    exit(3);\n}\n",
    )
    .unwrap();

    assert_eq!(run_build(source.clone(), None), 0);

    // **The artefact lands in the compiler process's working directory**, which for a test is the
    // crate directory rather than `dir`. That is ADR-0122's confinement working as designed — a
    // declared name is resolved relative to the working directory, and the test cannot change that
    // directory because every test in this binary shares it. So both places are accepted, and
    // whichever one appears is **removed**: a tracked binary that every test run rewrites was
    // committed once already, which is how this cleanup came to be written.
    let in_temp = dir.path().join("named_by_script");
    let in_cwd = PathBuf::from("named_by_script");
    let written = in_temp.exists() || in_cwd.exists();
    let _ = fs::remove_file(&in_temp);
    let _ = fs::remove_file(&in_cwd);
    assert!(written, "the declared name should be the artefact's");
    assert!(
        !dir.path().join("prog").exists(),
        "the default name should not also be written"
    );
}

#[test]
fn a_declared_build_output_cannot_escape_the_working_directory() {
    // ADR-0122. `BUILD_OUTPUT` is computed by arbitrary compile-time code *in the file being compiled*, so
    // it is attacker-controlled whenever the source is — the ordinary case for a compiler. Nothing checked
    // it, so this wrote an executable to a path git runs on the next commit, from nothing but a build.
    //
    // Asserted on the file **not** appearing as well as on the exit code, because a refusal that still wrote
    // the artefact somewhere would pass an exit-code-only test.
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("prog.jr");
    fs::write(
        &source,
        "#import \"Basic\";\nBUILD_OUTPUT :: \"../escaped\";\nmain :: () {\n    exit(3);\n}\n",
    )
    .unwrap();

    assert_eq!(
        run_build(source, None),
        2,
        "a declared output climbing out of the working directory must be refused"
    );
    assert!(
        !dir.path().parent().unwrap().join("escaped").exists(),
        "nothing should have been written outside the working directory"
    );
}

#[test]
fn a_declared_build_output_cannot_be_read_as_a_linker_flag() {
    // The other half of ADR-0122: `jr-link` passes the object path as `cc`'s **first positional argument**,
    // so a value starting with `-` was read as a flag rather than a path.
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("prog.jr");
    fs::write(
        &source,
        "#import \"Basic\";\nBUILD_OUTPUT :: \"-Wl,--version\";\nmain :: () {\n    exit(3);\n}\n",
    )
    .unwrap();

    assert_eq!(
        run_build(source, None),
        2,
        "a declared output starting with `-` must be refused"
    );
}

#[test]
fn explicit_output_flag_beats_the_build_output_constant() {
    // ADR-0102 §2's precedence, and the reason it is that way round: a person at a terminal is
    // overriding on purpose, so a script that could silently defeat `-o` would make the flag
    // untrustworthy.
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("prog.jr");
    fs::write(
        &source,
        "#import \"Basic\";\nBUILD_OUTPUT :: \"from_the_script\";\nmain :: () {\n    exit(3);\n}\n",
    )
    .unwrap();
    let flag = dir.path().join("from_the_flag");

    assert_eq!(run_build(source, Some(flag.clone())), 0);
    assert!(flag.exists(), "`-o` should decide");
    assert!(
        !dir.path().join("from_the_script").exists(),
        "the declared name should be ignored when `-o` is given"
    );
}

#[test]
fn a_file_with_no_build_output_still_defaults_to_its_own_name() {
    // The unchanged path, asserted so that adding the query cannot silently change what every
    // existing program builds to.
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("plain.jr");
    fs::write(
        &source,
        "#import \"Basic\";\nmain :: () {\n    exit(3);\n}\n",
    )
    .unwrap();

    assert_eq!(run_build(source, None), 0);
    assert!(dir.path().join("plain").exists(), "`plain.jr` → `plain`");
}

// ---------------------------------------------------------------------------
// An imported module's own diagnostics (ADR-0108)
// ---------------------------------------------------------------------------

/// A root file is clean; the module it imports is not; the error must be reported.
///
/// **What this pins is that the compiler says so at all.** `file_diagnostics` answers for *one* file, so before
/// ADR-0108 a root whose imported module was broken passed every gate — `jr check` printed "0 errors" — and then
/// failed inside an engine with `no routine for file 2 proc 0`, a signature having crossed the module boundary
/// while a body had not. That is the fifth leaked internal error this project has turned into a real diagnostic,
/// and it was found by writing the `List` module (ADR-0107 §5).
///
/// Resolution was never the bug: checking the module alone always reported `unresolved name malloc`. Nothing
/// asked it.
///
/// The assertion is on the **exit code and the module's own path**, because the diagnostic keeps its own file and
/// span — a reader is told the line to fix. Attributing it to the `#import` would read better for someone using a
/// module they cannot edit, and would discard the only thing that locates the bug (ADR-0043's lesson).
#[test]
fn an_imported_modules_errors_are_reported() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("root.jr");
    fs::write(
        &root,
        "#import \"Broken\";\n\nmain :: () {\n    n := uses_an_unimported_name(4);\n}\n",
    )
    .unwrap();

    let modules =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/broken-modules");
    assert!(modules.is_dir(), "the broken-module fixture must exist");

    let code = jr_cli::commands::check::run(
        jr_cli::cli::CheckArgs {
            paths: vec![root],
            module_paths: vec![modules],
        },
        &quiet_global(),
    )
    .expect("check should not fail at the io layer");

    assert_eq!(
        code, 1,
        "a root whose imported module is broken must not check clean"
    );
}

// ---------------------------------------------------------------------------
// ADR-0142: the optimisation-level surface
// ---------------------------------------------------------------------------

/// The flag's *surface*, asserted at the clap layer rather than by running a program.
///
/// ADR-0142 §1 accepts two levels and refuses a third **on purpose**: a `-O2` running the same
/// four passes would be a flag whose only content is a promise. That refusal is a decision, so it
/// is a test — otherwise the day a level is added nothing would record that the surface used to
/// be closed. The default is asserted too, because it is what keeps every existing invocation
/// meaning what it meant before the flag existed.
#[test]
fn the_opt_level_surface_accepts_two_levels_and_defaults_to_one() {
    use clap::Parser as _;

    let default = jr_cli::cli::Cli::try_parse_from(["jr", "run", "x.jr"])
        .expect("`jr run` without the flag must parse");
    let jr_cli::cli::Command::Run(args) = default.command else {
        panic!("`jr run` must parse as the run subcommand");
    };
    assert_eq!(
        args.opt_level,
        jr_cli::cli::OptLevelArg::Standard,
        "the default level must be the pipeline, so no existing build changes meaning"
    );

    for (argv, expected) in [
        (["jr", "run", "-O0", "x.jr"], jr_cli::cli::OptLevelArg::Off),
        (
            ["jr", "run", "-O1", "x.jr"],
            jr_cli::cli::OptLevelArg::Standard,
        ),
    ] {
        let cli = jr_cli::cli::Cli::try_parse_from(argv)
            .unwrap_or_else(|e| panic!("{argv:?} must parse: {e}"));
        let jr_cli::cli::Command::Run(args) = cli.command else {
            panic!("{argv:?} must parse as the run subcommand");
        };
        assert_eq!(args.opt_level, expected, "{argv:?} chose the wrong level");
    }

    let refused = jr_cli::cli::Cli::try_parse_from(["jr", "build", "-O2", "x.jr"]);
    let error = refused.expect_err("`-O2` must be refused until a pass justifies it");
    let rendered = error.to_string();
    assert!(
        rendered.contains('0') && rendered.contains('1'),
        "the refusal must name the levels that do exist: {rendered}"
    );
}

// ---------------------------------------------------------------------------
// ADR-0146: compile throughput
// ---------------------------------------------------------------------------

/// `jr bench --throughput` measures a file set and reports rather than judges (ADR-0146 §1).
///
/// The assertion is on the *exit code* and on the mode having run at all, not on a rate: a
/// throughput figure is a property of the machine, and asserting one would be the threshold
/// ADR-0033 §4 refuses and ADR-0146 §2 extends that refusal to.
///
/// **The empty-input case is asserted too, and it is the interesting half.** A throughput
/// number over no files is the most misleading output this subcommand could produce — it would
/// divide zero lines by a real duration and print `0 lines/s`, which reads as "this compiler is
/// infinitely slow" rather than "you gave me nothing". So it is an error.
#[test]
fn bench_throughput_measures_a_set_and_refuses_an_empty_one() {
    let dir = TempDir::new().expect("a temporary directory");
    let one = copy_corpus("valid/024-hello.jr", dir.path());
    let two = copy_corpus("valid/030-arrays.jr", dir.path());

    let args = jr_cli::cli::BenchArgs {
        file: one,
        paths: vec![two],
        throughput: true,
        iterations: 2,
        module_paths: vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../modules")],
    };
    let code =
        jr_cli::commands::bench::run(args, &quiet_global()).expect("the throughput mode must run");
    assert_eq!(
        code, 0,
        "throughput reports, it does not judge — it cannot fail"
    );

    let empty = TempDir::new().expect("a temporary directory");
    let args = jr_cli::cli::BenchArgs {
        file: empty.path().to_path_buf(),
        paths: Vec::new(),
        throughput: true,
        iterations: 1,
        module_paths: Vec::new(),
    };
    let error = jr_cli::commands::bench::run(args, &quiet_global())
        .expect_err("a directory with no `.jr` files must be an error");
    assert!(
        error.to_string().contains("no `.jr` files"),
        "the refusal must say what was missing: {error}"
    );
}

/// `Process.run` starts a child, waits for it, and decodes its status — **in a compiled binary**.
///
/// # Why this is not a corpus program
///
/// `tests/corpus/valid/` exists on the premise that both engines agree, and `Process.spawn` cannot hold that
/// premise: `execvp`'s second argument is an **array of pointers**, and the comptime VM translates a foreign
/// call's pointer argument to a host address one level deep (ADR-0061's own region). It cannot know the bytes
/// behind that pointer contain more pointers, so libc receives region-relative garbage for every argument
/// string, `execvp` fails, and the child exits `EXEC_FAILED`. Natively there is no translation and nothing to
/// get wrong.
///
/// So the test builds and runs the binary, which is the same reasoning ADR-0126 used when the VM trapped
/// where native code wrote short: a program whose two engines legitimately differ has no home in `valid/`.
/// ADR-0158 §3 records it and PLAN's known-defects list carries the limitation.
///
/// # What it asserts
///
/// Four bits, through the **exit status**, because that is the one channel a compiled binary and this harness
/// certainly share: a successful child, a failing child's exact code, a missing command reported as 127
/// through the same status channel as any other failure, and arguments actually reaching the child.
#[test]
fn process_run_starts_a_child_and_decodes_its_status() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("spawn.jr");
    fs::write(
        &source,
        r#"#import "Basic";
#import "Process";
main :: () {
    total := 0;

    ok_argv: [1]string;
    ok_argv[0] = "/usr/bin/true";
    ok_status, ok_ran := run(view(*ok_argv[0], 1));
    if ok_ran { if succeeded(ok_status) { total = total + 1; } }

    no_argv: [1]string;
    no_argv[0] = "/usr/bin/false";
    no_status, no_ran := run(view(*no_argv[0], 1));
    if no_ran {
        if no_status.exited { if no_status.code == 1 { total = total + 2; } }
    }

    // A command that cannot be executed is reported through the *status*, as 127 — the shell's convention,
    // so a caller who only looks at the status still gets a sensible answer rather than a mystery.
    missing: [1]string;
    missing[0] = "jr-definitely-not-a-command";
    missing_status, missing_ran := run(view(*missing[0], 1));
    if missing_ran { if missing_status.code == 127 { total = total + 4; } }

    // Arguments reach the child: `test 1 -eq 1` succeeds and `test 1 -eq 2` exits 1, so the vector is being
    // built and terminated correctly rather than merely being non-empty.
    args: [4]string;
    args[0] = "/bin/test";
    args[1] = "1";
    args[2] = "-eq";
    args[3] = "1";
    eq_status, eq_ran := run(view(*args[0], 4));
    args[3] = "2";
    ne_status, ne_ran := run(view(*args[0], 4));
    if eq_ran {
        if ne_ran {
            if succeeded(eq_status) { if ne_status.code == 1 { total = total + 8; } }
        }
    }

    exit(total);
}
"#,
    )
    .unwrap();

    let binary = dir.path().join("spawn");
    assert_eq!(
        run_build(source, Some(binary.clone())),
        0,
        "the program must compile"
    );
    let status = std::process::Command::new(&binary)
        .status()
        .expect("the built binary should run");
    assert_eq!(
        status.code(),
        Some(15),
        "all four checks must pass in a compiled binary; a lower value names which bit failed"
    );
}

/// An aggregate crosses a `#foreign` boundary exactly as a **real C compiler** expects (ADR-0160 part 2).
///
/// # Why this compiles a C shim instead of asserting against Jairs
///
/// A test that called a Jairs procedure declared `#c_call` would pass with both sides wrong: one
/// classification emits the call *and* reads it, so an agreement proves only self-consistency. The C ABI is
/// not negotiable, and the only way to know this compiler implements it is to link against something a C
/// compiler produced. `cc` is already a hard dependency — `jr-link` shells out to it — so needing it here
/// costs nothing new.
///
/// `valid/130` covers the *return* direction in all three engines through libc's `ldiv`. This covers what a
/// corpus program cannot: an aggregate **argument**, and the homogeneous float aggregate that a size test
/// would reject. The comptime VM is absent for a stated reason — it resolves symbols from the compiler's own
/// process image, not from a link line, so it cannot reach a shim at all.
///
/// # What it asserts
///
/// Five bits, each a shape the classification treats differently:
///
///   * `1` — a two-word integer struct **passed** by value;
///   * `2` — the same struct **returned**, with its fields swapped so a register mix-up is visible;
///   * `4` — a two-`double` HFA passed by value, which travels in floating-point registers;
///   * `8` — the same HFA returned, alongside a plain `double` argument, so the two register files are used
///     at once and a spill from one into the other would show;
///   * `16` — a **nested** four-`double` HFA: thirty-two bytes, still four registers. This is the `CGRect`
///     shape, and the one a byte-count test rejects.
#[test]
fn aggregates_cross_a_foreign_boundary_as_a_c_compiler_expects() {
    let dir = TempDir::new().unwrap();

    // The shim. `-O1` rather than `-O0` deliberately: an optimising compiler is freer to keep a struct in
    // registers, which is the convention under test.
    let shim_source = dir.path().join("shim.c");
    fs::write(
        &shim_source,
        r#"#include <stdint.h>
typedef struct { int64_t a; int64_t b; } Pair;
typedef struct { double x; double y; } Point;
typedef struct { Point origin; Point size; } Rect;

int64_t pair_sum(Pair p) { return p.a + p.b; }
Pair pair_swap(Pair p) { Pair r; r.a = p.b; r.b = p.a; return r; }
double point_sum(Point p) { return p.x + p.y; }
Point point_scale(Point p, double by) { Point r; r.x = p.x * by; r.y = p.y * by; return r; }
double rect_total(Rect r) { return r.origin.x + r.origin.y + r.size.x + r.size.y; }
"#,
    )
    .unwrap();
    let shim_object = dir.path().join("shim.o");
    let compiled = std::process::Command::new("cc")
        .arg("-O1")
        .arg("-c")
        .arg(&shim_source)
        .arg("-o")
        .arg(&shim_object)
        .status();
    match compiled {
        Ok(status) if status.success() => {}
        // No C compiler, no test. Skipped rather than failed: `cc` is present wherever `jr build` works, so
        // this cannot hide a regression on a machine that can run the rest of the suite.
        _ => return,
    }

    let source = dir.path().join("aggregates.jr");
    fs::write(
        &source,
        r#"#import "Basic";
Pair :: struct { a: s64; b: s64; }
Point :: struct { x: float64; y: float64; }
Rect :: struct { origin: Point; size: Point; }

pair_sum :: (p: Pair) -> s64 #foreign libc "pair_sum";
pair_swap :: (p: Pair) -> Pair #foreign libc "pair_swap";
point_sum :: (p: Point) -> float64 #foreign libc "point_sum";
point_scale :: (p: Point, by: float64) -> Point #foreign libc "point_scale";
rect_total :: (r: Rect) -> float64 #foreign libc "rect_total";

main :: () {
    total := 0;

    p: Pair;
    p.a = 3;
    p.b = 4;
    if pair_sum(p) == 7 { total = total + 1; }

    // Swapped, so reading the two result registers in the wrong order is visible. A sum would not show it.
    swapped := pair_swap(p);
    if swapped.a == 4 { if swapped.b == 3 { total = total + 2; } }

    q: Point;
    q.x = 1.5;
    q.y = 2.5;
    if point_sum(q) == 4.0 { total = total + 4; }

    // An HFA argument *and* a plain float argument, so both register files are in use at once.
    scaled := point_scale(q, 2.0);
    if scaled.x == 3.0 { if scaled.y == 5.0 { total = total + 8; } }

    // Thirty-two bytes, four registers: the CGRect shape a byte-count test would send to memory.
    r: Rect;
    r.origin = q;
    r.size = scaled;
    if rect_total(r) == 12.0 { total = total + 16; }

    exit(total);
}
"#,
    )
    .unwrap();

    let object = dir.path().join("aggregates.o");
    let global = quiet_global();
    let code = jr_cli::commands::build::run(
        jr_cli::cli::BuildArgs {
            path: source,
            output: Some(object.clone()),
            emit_object: true,
            backend: jr_cli::cli::BackendArg::Cranelift,
            no_bounds_check: false,
            // `None` so the default level applies, which is what an ordinary build gets (ADR-0154 §1).
            opt_level: None,
            module_paths: vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../modules")],
            // Empty: these two tests link only against libc, which the driver finds on its own.
            library_paths: Vec::new(),
        },
        &global,
    )
    .expect("emitting an object must not fail at the io layer");
    assert_eq!(code, 0, "the program must compile");

    let binary = dir.path().join("aggregates");
    let linked = std::process::Command::new("cc")
        .arg(&object)
        .arg(&shim_object)
        .arg("-o")
        .arg(&binary)
        .status()
        .expect("cc should run");
    assert!(linked.success(), "the object and the shim must link");

    let ran = std::process::Command::new(&binary)
        .status()
        .expect("the linked binary should run");
    assert_eq!(
        ran.code(),
        Some(31),
        "every aggregate shape must agree with the C compiler; a lower value names which bit failed"
    );
}

/// `--library-path` puts a `-L` on the link line, so a `#system_library` outside the driver's defaults resolves
/// (ADR-0163 §2).
///
/// # Why this builds its own library instead of using SDL2
///
/// The gap was found with SDL2 — `ld: library 'SDL2' not found`, with `-lSDL2` on the line and nowhere to look
/// — and testing it with SDL2 would make the suite depend on a Homebrew package. So the test compiles a
/// one-function library into a temporary directory and points `-L` at it. That proves the mechanism, which is
/// the part this compiler owns; whether any particular library is installed is not.
///
/// The negative half runs **first** and is the half that matters: without the flag the link must *fail*.
/// A test that only checked the success case would pass even if `-L` were ignored, because a driver that
/// happened to find the library some other way would look identical.
#[test]
fn a_library_path_reaches_the_link_line() {
    let dir = TempDir::new().unwrap();

    let lib_source = dir.path().join("answer.c");
    fs::write(&lib_source, "long jr_answer(void) { return 42; }\n").unwrap();
    let lib_object = dir.path().join("answer.o");
    let compiled = std::process::Command::new("cc")
        .arg("-c")
        .arg(&lib_source)
        .arg("-o")
        .arg(&lib_object)
        .status();
    match compiled {
        Ok(status) if status.success() => {}
        // No C compiler, no test — and `jr build` could not link at all on such a machine, so this cannot
        // hide a regression anywhere the rest of the suite runs.
        _ => return,
    }
    let archive = dir.path().join("libjranswer.a");
    let archived = std::process::Command::new("ar")
        .arg("rcs")
        .arg(&archive)
        .arg(&lib_object)
        .status();
    match archived {
        Ok(status) if status.success() => {}
        _ => return,
    }

    let source = dir.path().join("uses_library.jr");
    fs::write(
        &source,
        r#"answers :: #system_library "jranswer";
libc :: #system_library "c";
jr_answer :: () -> s64 #foreign answers "jr_answer";
exit :: (status: s64) #foreign libc "exit";
main :: () {
    exit(jr_answer());
}
"#,
    )
    .unwrap();

    // Without the path, the link must fail — otherwise this test proves nothing about `-L`.
    let unfound = dir.path().join("without");
    let code = run_build_with_paths(source.clone(), unfound, &[]);
    assert_ne!(
        code, 0,
        "with no --library-path the library cannot be found, so the link must fail; if it succeeds, \
         the flag is not what is being exercised"
    );

    let binary = dir.path().join("with");
    let code = run_build_with_paths(source, binary.clone(), &[dir.path().to_path_buf()]);
    assert_eq!(code, 0, "with the path, the link must succeed");
    let ran = std::process::Command::new(&binary)
        .status()
        .expect("the linked binary should run");
    assert_eq!(
        ran.code(),
        Some(42),
        "the library's own function must be the one that ran"
    );
}

/// `jr build` with explicit library search paths.
fn run_build_with_paths(path: PathBuf, output: PathBuf, library_paths: &[PathBuf]) -> i32 {
    jr_cli::commands::build::run(
        jr_cli::cli::BuildArgs {
            path,
            output: Some(output),
            emit_object: false,
            backend: jr_cli::cli::BackendArg::Cranelift,
            no_bounds_check: false,
            opt_level: None,
            module_paths: vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../modules")],
            library_paths: library_paths.to_vec(),
        },
        &quiet_global(),
    )
    .unwrap_or(1)
}

/// `modules/Window` opens a window and draws through SDL2, in a compiled binary (ADR-0163 §1).
///
/// # Why this is not a corpus program
///
/// `tests/corpus/valid/` asserts that the VM and the native back ends agree, and the VM **cannot** reach
/// SDL2: it resolves a foreign symbol from the compiler's own process image, and `jr` is not linked against
/// SDL2. That is the same reason `Process` got an integration test while `Socket` got a corpus file
/// (ADR-0158 §3) — the boundary is what the VM can call, not what the language can express.
///
/// # Why it skips instead of failing
///
/// SDL2 is a third-party library, which ADR-0163 §1 accepted as this foundation's stated cost. A machine
/// without it cannot run this and the rest of the suite is unaffected — so the test looks for the library
/// first and returns if it is absent. The skip is narrow on purpose: it checks for the *library*, and every
/// assertion after that point is unconditional.
///
/// The program exercises ten steps and sums a distinct bit for each, so a failure names which one broke
/// rather than only that something did.
#[test]
fn a_window_opens_and_draws_through_sdl2() {
    // The directories `-L` would search, in the order a developer on either supported platform would have
    // them. Homebrew on arm64 macOS first, then Intel's prefix, then the two usual Linux locations.
    let candidates = [
        "/opt/homebrew/lib",
        "/usr/local/lib",
        "/usr/lib/x86_64-linux-gnu",
        "/usr/lib",
    ];
    let library_dir = candidates.iter().map(PathBuf::from).find(|dir| {
        dir.join("libSDL2.dylib").exists()
            || dir.join("libSDL2.so").exists()
            || dir.join("libSDL2-2.0.so.0").exists()
    });
    let Some(library_dir) = library_dir else {
        // No SDL2, no test. ADR-0163 §1 accepted this dependency explicitly; the rest of the suite has none.
        return;
    };

    let dir = TempDir::new().unwrap();
    let source = dir.path().join("draws.jr");
    fs::write(
        &source,
        r#"#import "Window";

libc :: #system_library "c";
exit_now :: (status: s64) #foreign libc "exit";

main :: () {
    total := 0;

    // 1 — the library linked and ran at all, with no display needed.
    if video_driver_count() > 0 { total = total + 1; }

    if !start() { exit_now(90); }
    total = total + 2;

    // `HIDDEN`, so the whole creation path runs with no display. `SOFTWARE`, because `ACCELERATED` fails
    // on a machine with no GPU driver and this must pass on a CI runner.
    title := "Jairs\0";
    w, ok := open(title.data, 320, 240, HIDDEN);
    if !ok { exit_now(91); }
    total = total + 4;

    r, rok := renderer_for(*w, SOFTWARE);
    if !rok { exit_now(92); }
    total = total + 8;

    if set_color(*r, 20, 30, 40, 255) { total = total + 16; }
    if clear(*r) { total = total + 32; }

    // A rect crosses as a `*Rect`, which is why SDL2 was reachable before Cocoa: no aggregate by value.
    box := rect(10, 20, 100, 50);
    if fill(*r, *box) { total = total + 64; }
    if outline(*r, *box) { total = total + 128; }
    if line(*r, 0, 0, 319, 239) { total = total + 256; }

    present(*r);
    delay(1);

    // Closing twice must be safe, so a caller can close on every path without tracking whether they got one.
    destroy(*r);
    close(*w);
    destroy(*r);
    close(*w);
    total = total + 512;

    stop();
    exit_now(total % 251);
}
"#,
    )
    .unwrap();

    let binary = dir.path().join("draws");
    let code = run_build_with_paths(source, binary.clone(), &[library_dir]);
    assert_eq!(code, 0, "the program must build and link against SDL2");

    let ran = std::process::Command::new(&binary)
        // The dummy driver, so no display is needed and a headless runner behaves like a desktop.
        .env("SDL_VIDEODRIVER", "dummy")
        .status()
        .expect("the linked binary should run");
    assert_eq!(
        ran.code(),
        // 1023 is all ten bits; the exit status is a byte, so the program takes it mod 251 the way every
        // other corpus program does.
        Some(1023 % 251),
        "every step must succeed; a lower value names which bit failed, and 90-92 name a hard stop"
    );
}

/// `modules/Window`'s event loop: a `#place` overlay of `SDL_Event`, round-tripped through SDL's own queue
/// (ADR-0165).
///
/// # Why this test exists separately from the drawing one
///
/// ADR-0164 §5 recorded that an event loop was **impossible** here, because `SDL_Event` is a union and E0286
/// refuses one at a `#foreign` boundary. That was wrong, and it was wrong for the reason `AGENTS.md` warns
/// about: it was recorded without writing the thing. `SDL_PollEvent` takes a **pointer**, and a pointer to a
/// union is just a pointer. This test is the correction's evidence.
///
/// # What it does and does not push through SDL
///
/// A `QUIT` and a window-close go through SDL's real queue, because SDL delivers those. **A keyboard event
/// does not**: `SDL_PushEvent` returns success for a synthetic `KEY_DOWN` and SDL then drops it, which was
/// found by instrumenting rather than assumed. So the key assertions build an `Event` locally and read it back
/// through `pressed` — which tests the overlay's offsets and the auto-repeat filter, the part this project
/// owns. Whether SDL delivers a fabricated keypress is SDL's business.
///
/// The close checks **drain** rather than polling once, because SDL's queue does not promise one-push-one-poll:
/// a single poll can return nothing while an event is pending. A one-poll version of this test passed on the
/// first push and failed on the second, which is exactly the bug `wants_to_close` exists to stop a caller
/// writing.
#[test]
fn an_event_loop_reads_the_sdl_event_union() {
    let candidates = [
        "/opt/homebrew/lib",
        "/usr/local/lib",
        "/usr/lib/x86_64-linux-gnu",
        "/usr/lib",
    ];
    let library_dir = candidates.iter().map(PathBuf::from).find(|dir| {
        dir.join("libSDL2.dylib").exists()
            || dir.join("libSDL2.so").exists()
            || dir.join("libSDL2-2.0.so.0").exists()
    });
    let Some(library_dir) = library_dir else {
        return;
    };

    let dir = TempDir::new().unwrap();
    let source = dir.path().join("events.jr");
    fs::write(
        &source,
        r#"#import "Window";

libc :: #system_library "c";
exit_now :: (status: s64) #foreign libc "exit";

main :: () {
    total := 0;

    // 1 — the overlay is the 56 bytes SDL writes. Everything after this is meaningless if it is not.
    if layout_is_sdl2() { total = total + 1; }

    if !start() { exit_now(90); }
    title := "Events ";
    w, ok := open(title.data, 200, 150, HIDDEN);
    if !ok { exit_now(91); }
    total = total + 2;

    // Nothing pending on a fresh queue.
    if !wants_to_close(64) { total = total + 4; }

    // A window-close round-trips through SDL's real queue and is recognised.
    c: Event;
    c.kind = cast(u32, WINDOW_EVENT);
    c.window_event = cast(u8, WINDOW_CLOSE);
    if push(*c) { total = total + 8; }
    if wants_to_close(64) { total = total + 16; }

    // So does a QUIT, and the drain leaves the queue empty.
    q := quit_event();
    if push(*q) { total = total + 32; }
    if wants_to_close(64) { total = total + 64; }
    if !wants_to_close(64) { total = total + 128; }

    // A key event is built locally: SDL drops a synthetic keyboard push, so the queue cannot carry one.
    k: Event;
    k.kind = cast(u32, KEY_DOWN);
    k.key_sym = cast(u32, KEY_ESCAPE);
    if pressed(*k, KEY_ESCAPE) { total = total + 256; }
    if !pressed(*k, 97) { total = total + 512; }
    if !should_close(*k) { total = total + 1024; }

    // An auto-repeat is not a press.
    k.key_repeat = 1;
    if !pressed(*k, KEY_ESCAPE) { total = total + 2048; }

    // The union shows through: mouse_x and key_sym genuinely share offset 20, which is the point of #place.
    m: Event;
    m.kind = cast(u32, MOUSE_DOWN);
    m.mouse_button = 1;
    m.mouse_x = 40;
    m.mouse_y = 90;
    if cast(s64, m.mouse_button) == 1 && m.mouse_x == 40 && m.mouse_y == 90 { total = total + 4096; }
    if cast(s64, m.key_sym) == 40 { total = total + 8192; }

    close(*w);
    stop();
    exit_now(total % 251);
}
"#,
    )
    .unwrap();

    let binary = dir.path().join("events");
    let code = run_build_with_paths(source, binary.clone(), &[library_dir]);
    assert_eq!(code, 0, "the program must build and link against SDL2");

    let ran = std::process::Command::new(&binary)
        .env("SDL_VIDEODRIVER", "dummy")
        .status()
        .expect("the linked binary should run");
    assert_eq!(
        ran.code(),
        // 16383 is all fourteen bits, mod 251 because an exit status is a byte.
        Some(16383 % 251),
        "every step must succeed; a lower value names which bit failed, and 90-91 name a hard stop"
    );
}

/// `modules/UI`'s immediate-mode button, driven through SDL's real event queue (ADR-0166).
///
/// # What this actually proves
///
/// It is the first test here that needs a window, an event queue **and** a renderer at once, so it is the one
/// that says the graphics stack composes rather than that three modules each work.
///
/// The interesting assertions are the negative ones. A button must fire on **release inside after press
/// inside** — so pressing it, dragging off and releasing elsewhere must fire *nothing*, and a press that begins
/// outside must not arm it. Returning `true` on press is the naive implementation, and it passes every positive
/// test while breaking the escape hatch every user expects to work.
///
/// A synthetic mouse event survives SDL's queue with its coordinates intact — checked, unlike a synthetic
/// `KEY_DOWN`, which SDL accepts and drops (ADR-0165 §4). So this drives the real queue rather than building
/// events locally, which is stronger evidence.
///
/// **`is_hot(ui, NONE)` is in here because it was a bug.** `hot` *is* `NONE` when nothing is hot, so a bare
/// comparison answered `true` for the sentinel on every frame — a widget that does not exist, reported as
/// hovered. Found by this test, fixed by refusing the sentinel.
#[test]
fn an_immediate_mode_button_fires_on_release_inside() {
    let candidates = [
        "/opt/homebrew/lib",
        "/usr/local/lib",
        "/usr/lib/x86_64-linux-gnu",
        "/usr/lib",
    ];
    let library_dir = candidates.iter().map(PathBuf::from).find(|dir| {
        dir.join("libSDL2.dylib").exists()
            || dir.join("libSDL2.so").exists()
            || dir.join("libSDL2-2.0.so.0").exists()
    });
    let Some(library_dir) = library_dir else {
        return;
    };

    let dir = TempDir::new().unwrap();
    let source = dir.path().join("ui.jr");
    fs::write(&source, PROGRAM).unwrap();

    let binary = dir.path().join("ui");
    let code = run_build_with_paths(source, binary.clone(), &[library_dir]);
    assert_eq!(code, 0, "the program must build and link against SDL2");

    let ran = std::process::Command::new(&binary)
        .env("SDL_VIDEODRIVER", "dummy")
        .status()
        .expect("the linked binary should run");
    assert_eq!(
        ran.code(),
        // 65535 is all sixteen bits, mod 251 because an exit status is a byte.
        Some(65535 % 251),
        "every interaction must behave; a lower value names which bit failed, and 90-92 name a hard stop"
    );
}

/// The immediate-mode UI program the test above builds.
///
/// A constant rather than an inline literal, because it is long enough that the assertions would be lost
/// after it.
const PROGRAM: &str = r#"#import "Window";
#import "UI";
libc :: #system_library "c";
exit_now :: (status: s64) #foreign libc "exit";

// Pushes a mouse event and folds every queued event into `ui`.
send :: (ui: *UI, kind: s64, x: s64, y: s64) {
    m: Event;
    m.kind = cast(u32, kind);
    m.mouse_button = 1;
    m.mouse_x = cast(s32, x);
    m.mouse_y = cast(s32, y);
    _ = push(*m);
    e: Event;
    i := 0;
    while i < 32 {
        if next_event(*e) { feed(ui, *e); }
        i = i + 1;
    }
}

main :: () {
    if !start() { exit_now(90); }
    t := "UI\0";
    w, ok := open(t.data, 200, 200, HIDDEN);
    if !ok { exit_now(91); }
    r, rok := renderer_for(*w, SOFTWARE);
    if !rok { exit_now(92); }

    ui: UI;
    total := 0;

    // A press-and-release inside is a click.
    begin_frame(*ui);
    send(*ui, MOUSE_DOWN, 20, 20);
    if !button(*ui, 1, 10, 10, 80, 24) { total = total + 1; }   // not on press
    if is_active(*ui, 1) { total = total + 2; }
    if is_hot(*ui, 1) { total = total + 4; }

    begin_frame(*ui);
    send(*ui, MOUSE_UP, 20, 20);
    if button(*ui, 1, 10, 10, 80, 24) { total = total + 8; }    // fires on release
    if !is_active(*ui, 1) { total = total + 16; }

    // Press inside, drag off, release outside: must NOT fire.
    begin_frame(*ui);
    send(*ui, MOUSE_DOWN, 20, 20);
    _ = button(*ui, 1, 10, 10, 80, 24);
    begin_frame(*ui);
    send(*ui, MOUSE_UP, 150, 150);
    if !button(*ui, 1, 10, 10, 80, 24) { total = total + 32; }
    if !is_active(*ui, 1) { total = total + 64; }

    // A press that begins outside cannot arm the button.
    begin_frame(*ui);
    send(*ui, MOUSE_DOWN, 150, 150);
    _ = button(*ui, 1, 10, 10, 80, 24);
    begin_frame(*ui);
    send(*ui, MOUSE_UP, 20, 20);
    if !button(*ui, 1, 10, 10, 80, 24) { total = total + 128; }

    // Two buttons: only the one under the cursor fires.
    begin_frame(*ui);
    send(*ui, MOUSE_DOWN, 20, 50);
    _ = button(*ui, 1, 10, 10, 80, 24);
    _ = button(*ui, 2, 10, 44, 80, 24);
    begin_frame(*ui);
    send(*ui, MOUSE_UP, 20, 50);
    if !button(*ui, 1, 10, 10, 80, 24) { total = total + 256; }
    if button(*ui, 2, 10, 44, 80, 24) { total = total + 512; }

    // Edges are half-open: x + w is outside.
    begin_frame(*ui);
    send(*ui, MOUSE_MOTION, 90, 20);
    _ = button(*ui, 1, 10, 10, 80, 24);
    if !is_hot(*ui, 1) { total = total + 1024; }
    begin_frame(*ui);
    send(*ui, MOUSE_MOTION, 89, 20);
    _ = button(*ui, 1, 10, 10, 80, 24);
    if is_hot(*ui, 1) { total = total + 2048; }

    // A zero id fires nothing and is not hot.
    begin_frame(*ui);
    send(*ui, MOUSE_DOWN, 20, 20);
    begin_frame(*ui);
    send(*ui, MOUSE_UP, 20, 20);
    if !button(*ui, NONE, 10, 10, 80, 24) { total = total + 4096; }
    if !is_hot(*ui, NONE) { total = total + 8192; }
    if !is_active(*ui, NONE) { total = total + 32768; }

    // Drawing composes with all three states.
    if draw_button(*r, *ui, 1, 10, 10, 80, 24) { total = total + 16384; }

    destroy(*r);
    close(*w);
    stop();
    exit_now(total % 251);
}
"#;

/// `modules/Image`: a BMP round trip, a texture upload, and a draw (ADR-0167).
///
/// # Why the fixture is built rather than committed
///
/// The program creates a 24x16 surface, fills a rectangle, saves it as a BMP, and loads it back. So there is no
/// binary file in the repository, and the test exercises the *decode* — which is the step with the interesting
/// failure — rather than trusting a blob somebody generated once.
///
/// The last two assertions are the ones worth having: the one-call `load_texture` path, and a **missing file**,
/// which must return `false` rather than trap or produce a texture that draws nothing.
#[test]
fn a_bmp_round_trips_into_a_texture() {
    let candidates = [
        "/opt/homebrew/lib",
        "/usr/local/lib",
        "/usr/lib/x86_64-linux-gnu",
        "/usr/lib",
    ];
    let library_dir = candidates.iter().map(PathBuf::from).find(|dir| {
        dir.join("libSDL2.dylib").exists()
            || dir.join("libSDL2.so").exists()
            || dir.join("libSDL2-2.0.so.0").exists()
    });
    let Some(library_dir) = library_dir else {
        return;
    };

    let dir = TempDir::new().unwrap();
    let source = dir.path().join("image.jr");
    fs::write(&source, IMAGE_PROGRAM).unwrap();

    let binary = dir.path().join("image");
    let code = run_build_with_paths(source, binary.clone(), &[library_dir]);
    assert_eq!(code, 0, "the program must build and link against SDL2");

    let ran = std::process::Command::new(&binary)
        .env("SDL_VIDEODRIVER", "dummy")
        .status()
        .expect("the linked binary should run");
    assert_eq!(
        ran.code(),
        Some(65535 % 251),
        "every step must succeed; a lower value names which bit failed, and 90-93 name a hard stop"
    );
}

/// The image program the test above builds.
const IMAGE_PROGRAM: &str = r#"#import "Window";
#import "Image";
libc :: #system_library "c";
exit_now :: (status: s64) #foreign libc "exit";
remove_file :: (path: *u8) -> s64 #foreign libc "remove";

main :: () {
    total := 0;
    if surface_layout_is_sdl2() { total = total + 1; }
    if !start() { exit_now(90); }
    t := "Img\0";
    w, ok := open(t.data, 200, 200, HIDDEN);
    if !ok { exit_now(91); }
    r, rok := renderer_for(*w, SOFTWARE);
    if !rok { exit_now(92); }

    // Build an image, so nothing binary lives in the repository.
    s, sok := create_surface(24, 16);
    if !sok { exit_now(93); }
    total = total + 2;
    if width_of(*s) == 24 && height_of(*s) == 16 { total = total + 4; }
    if pitch_of(*s) >= 24 * 4 { total = total + 8; }

    box := rect(4, 4, 8, 8);
    if fill_surface(*s, *box, 255) { total = total + 16; }

    path := "/tmp/jr-image-test.bmp\0";
    if save_bmp(*s, path.data) { total = total + 32; }
    free_surface(*s);
    free_surface(*s);
    total = total + 64;

    // Load it back, and the size must survive the round trip.
    l, lok := load_bmp(path.data);
    if lok { total = total + 128; }
    if width_of(*l) == 24 && height_of(*l) == 16 { total = total + 256; }

    tex, tok := texture_from(*r, *l);
    if tok { total = total + 512; }
    free_surface(*l);

    tw: s32;
    th: s32;
    if size_of_texture(*tex, *tw, *th) { total = total + 1024; }
    if cast(s64, tw) == 24 && cast(s64, th) == 16 { total = total + 2048; }

    dst := rect(10, 10, 48, 32);
    if draw_texture(*r, *tex, *dst) { total = total + 4096; }
    present(*r);
    destroy_texture(*tex);
    destroy_texture(*tex);
    total = total + 8192;

    // The one-call path, and a missing file must fail rather than trap.
    t2, t2ok := load_texture(*r, path.data);
    if t2ok { total = total + 16384; }
    destroy_texture(*t2);
    missing := "/tmp/jr-image-absent.bmp\0";
    t3, t3ok := load_texture(*r, missing.data);
    if !t3ok { total = total + 32768; }

    _ = remove_file(path.data);
    destroy(*r);
    close(*w);
    stop();
    exit_now(total % 251);
}
"#;
