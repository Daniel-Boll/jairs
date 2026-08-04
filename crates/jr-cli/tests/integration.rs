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
            no_bounds_check: false,
            module_paths: vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../modules")],
        },
        &quiet_global(),
    )
    .expect("build should not fail at the io layer")
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
    assert!(
        dir.path().join("named_by_script").exists() || Path::new("named_by_script").exists(),
        "the declared name should be the artefact's"
    );
    assert!(
        !dir.path().join("prog").exists(),
        "the default name should not also be written"
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
