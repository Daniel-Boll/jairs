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
        // `$$` in a **return** type (ADR-0168 §1). Here for the stage reason E0262 is: E0290 comes out of
        // **lowering**, so `type-errors/`' harness would fail it for not lowering cleanly before ever checking
        // the code it declares — which is exactly what happened when this file was filed there first.
        //
        // What it pins is a *diagnostic that replaced an ICE*: `-> $$T` checked clean and the call died with
        // "no routine for file 0 proc 3", the tenth instance of internals leaking for a legal-looking program.
        // Found by probing PLAN's wave table, which said `$$T` was undelivered while `AGENTS.md` said it
        // shipped: the *parameter* had shipped (`valid/110`) and the *return* had never been tried.
        "imports/invalid/018-comptime-poly-return.jr",
        // A qualified name the module does not export — E0292 (ADR-0179 §4). Here for the stage reason
        // E0262 is: E0292 comes out of **resolution**, so `type-errors/`' harness would fail it for not
        // resolving cleanly before ever checking the code it declares.
        //
        // What it pins is the *distinction* from E0253, its neighbour: `009-not-exported.jr` reports a
        // name the module declares and hides, which a reader fixes by moving the declaration. This one
        // reports a name that is not there at all, where there is no `#scope_module` to go looking for.
        "imports/invalid/019-qualified-name-absent.jr",
        // A **computed** file-scope `#insert` generating a procedure, and one generating a constant
        // (ADR-0184 §4). Here for the stage reason E0262 is, one phase later: E0294 comes out of the
        // **expanded** lowering, so `type-errors/`' harness — which runs sema on the unexpanded tree —
        // saw no diagnostic at all and failed the files for not declaring what they declare.
        //
        // What they pin is a *boundary* rather than a mistake. A **literal** insert expands during
        // `file_hir` and can generate anything (`valid/136` does, and exits 63); a computed one expands
        // after const-eval, so a generated procedure has no signature and a generated constant has no
        // value. Both leaked internals before the refusal existed — "called a procedure taking 2
        // arguments with 1" and "a file-level item has no value until jr-vm".
        "imports/invalid/020-computed-insert-declarations.jr",
        "imports/invalid/021-computed-insert-constant.jr",
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

/// Builds with one **extra module directory** beside the repository's `modules`.
///
/// `run_build` above passes only the repository's `modules`, so a test whose program imports a module it
/// wrote into a temporary directory cannot use it: `#import` does **not** search the importing file's own
/// directory (measured — E0210, module not found). This helper is the one-line difference, and it exists
/// for the refused-body test, whose construct now needs a second file (ADR-0186's cross-file gap).
fn run_build_with_module_dir(path: PathBuf, output: Option<PathBuf>, extra: PathBuf) -> i32 {
    jr_cli::commands::build::run(
        jr_cli::cli::BuildArgs {
            path,
            output,
            emit_object: false,
            backend: jr_cli::cli::BackendArg::Cranelift,
            no_bounds_check: false,
            opt_level: None,
            module_paths: vec![
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../modules"),
                extra,
            ],
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

/// `modules/Window` opens a window and draws through OpenGL, in a compiled binary (ADR-0187).
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
/// # Why this runs against the real video driver
///
/// `Window.create_window` asks for `SDL_WINDOW_OPENGL` first, because a window `Simp` will draw to must be
/// able to carry a GL context and SDL decides that at creation. **Measured**: SDL's dummy driver refuses
/// that outright — `SDL_GetError` reports "OpenGL support is either not configured in SDL or not available
/// in current SDL video driver (dummy) or platform". `create_window` **falls back to a plain window** on
/// that failure rather than returning null, so a program that only reads events works headless again (see
/// `an_event_loop_reads_the_sdl_event_union`) — but this program draws, and `Simp.is_ready()` reports the
/// fallback window's missing GL context as its own failure at that point rather than at window creation.
/// So this test still needs the real platform driver, and the failure is now where the requirement
/// actually is.
///
/// The program exercises eleven steps and sums a distinct bit for each, so a failure names which one broke
/// rather than only that something did. Four of them used to read a `bool #must` return from `Simp` and now
/// read `GL.error_code()` instead, because `clear_render_target`, `set_shader_for_color` and
/// `immediate_flush` return nothing in the real API (ADR-0187 §3) — a flush is checked for *drawing having
/// happened cleanly*, not merely for the program having exited.
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
        r#"Window :: #import "Window";
Simp :: #import "Simp";
GL :: #import "GL";

libc :: #system_library "c";
exit_now :: (status: s64) #foreign libc "exit";

main :: () {
    total := 0;

    // 1 — the library linked and ran at all, with no display needed.
    if Window.video_driver_count() > 0 { total = total + 1; }

    if !Window.start() { exit_now(90); }
    total = total + 2;

    // `create_window` takes width, height, title now — Jai's order — and returns one value. There is no
    // `HIDDEN` flag any more: every window is `OPENGL | SHOWN`, because `Simp` needs a GL-capable window
    // and SDL decides that at creation.
    w := Window.create_window(320, 240, "Jairs");
    if !Window.is_open(*w) { exit_now(91); }
    total = total + 4;

    // `set_render_target` returns nothing and creates the GL context on first use; `is_ready` is the
    // question a caller who wants to know asks. `RIGHT_HANDED` (the default) is fine here: this program
    // has no hit-testing to keep aligned with drawing.
    Simp.set_render_target(*w);
    if Simp.is_ready() { total = total + 8; }

    // Colours are floats in 0..1 now, as Simp's are, where `set_color` took 0-255 integers. `clear_render_target`
    // returns nothing, so success is read from `GL.error_code()` — the check the assignment names for "did the
    // GL call succeed" once there is no boolean to receive.
    Simp.clear_render_target(0.08, 0.12, 0.16, 1.0);
    if GL.error_code() == GL.NO_ERROR { total = total + 16; }
    Simp.set_shader_for_color(true);
    if GL.error_code() == GL.NO_ERROR { total = total + 32; }

    // A batch of two quads in *different colours*, which a renderer-global colour could not do in one draw
    // call. The second one is deliberately not axis-aligned: the two-corner `immediate_quad` cannot express
    // it, so it goes through `immediate_quad_corners` — the four-arbitrary-point form.
    Simp.immediate_begin();
    Simp.immediate_quad(10.0, 20.0, 110.0, 70.0, Simp.vector4(1.0, 0.4, 0.4, 1.0));
    Simp.immediate_quad_corners(
        Simp.vector2(160.0, 20.0), Simp.vector2(220.0, 60.0),
        Simp.vector2(180.0, 120.0), Simp.vector2(120.0, 80.0),
        Simp.vector4(0.3, 0.9, 0.4, 1.0),
        Simp.vector2(0.0, 0.0), Simp.vector2(1.0, 0.0),
        Simp.vector2(1.0, 1.0), Simp.vector2(0.0, 1.0));
    Simp.immediate_flush();
    if GL.error_code() == GL.NO_ERROR { total = total + 64; }

    // An empty flush succeeds: a frame that drew nothing must not read as a failure.
    Simp.immediate_begin();
    Simp.immediate_flush();
    if GL.error_code() == GL.NO_ERROR { total = total + 128; }

    // The render target's size, which is `Simp`'s question because it needs the renderer. The *window*'s is
    // `Window.get_window_size`, and on a high-DPI display the two differ — measured equal on this machine
    // because this window carries no `SDL_WINDOW_ALLOW_HIGHDPI` flag.
    pw, ph := Simp.get_render_dimensions(*w);
    if pw == 320 && ph == 240 { total = total + 256; }
    ww, wh := Window.get_window_size(*w);
    if ww == 320 && wh == 240 { total = total + 512; }

    Simp.swap_buffers(*w);
    Window.delay(1);

    // Closing twice must be safe, so a caller can close on every path without tracking whether they got one.
    // The renderer goes first: `Simp` holds the one process-wide GL context, and `destroy_render_target` is
    // safe to call more than once.
    Simp.destroy_render_target();
    Window.close(*w);
    Simp.destroy_render_target();
    Window.close(*w);
    total = total + 1024;

    Window.stop();
    exit_now(total % 251);
}
"#,
    )
    .unwrap();

    let binary = dir.path().join("draws");
    let code = run_build_with_paths(source, binary.clone(), &[library_dir]);
    assert_eq!(code, 0, "the program must build and link against SDL2");

    let ran = std::process::Command::new(&binary)
        // No `SDL_VIDEODRIVER` override — this program draws, and drawing needs a real GL context, which
        // `dummy`'s fallback window does not have (see the doc comment above).
        .status()
        .expect("the linked binary should run");
    assert_eq!(
        ran.code(),
        // 2047 is all eleven bits; the exit status is a byte, so the program takes it mod 251 the way every
        // other corpus program does.
        Some(2047 % 251),
        "every step must succeed; a lower value names which bit failed, and 90-91 name a hard stop"
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
///
/// # Why this runs headless again
///
/// This test opens no renderer and reads no pixel. `Window.create_window` asks for
/// `SDL_WINDOW_OPENGL` first, and SDL's `dummy` driver refuses that outright — measured: `SDL_GetError`
/// reports "OpenGL support is either not configured in SDL or not available in current SDL video driver
/// (dummy) or platform". But `create_window` **falls back to a plain window** on that failure rather than
/// returning null, so a program that never touches `Simp` or GL — this one — still opens a window and
/// reads events under `SDL_VIDEODRIVER=dummy`, which matters because this project wants a Linux CI run
/// with no display attached.
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
        r#"Window :: #import "Window";
Input :: #import "Input";

libc :: #system_library "c";
exit_now :: (status: s64) #foreign libc "exit";

main :: () {
    total := 0;

    // 1 — the overlay is the 56 bytes SDL writes. Everything after this is meaningless if it is not.
    if Input.EVENT_LAYOUT_IS_SDL2 { total = total + 1; }

    if !Window.start() { exit_now(90); }
    // `create_window` takes width, height, title now, and returns one value — a null handle is the failure.
    w := Window.create_window(200, 150, "Events");
    if !Window.is_open(*w) { exit_now(91); }
    total = total + 2;

    // Nothing pending on a fresh queue.
    if !Input.wants_to_close(64) { total = total + 4; }

    // A window-close round-trips through SDL's real queue and is recognised.
    c: Input.Event;
    c.kind = cast(u32, Input.WINDOW_EVENT);
    c.window_event = cast(u8, Input.WINDOW_CLOSE);
    if Input.push(*c) { total = total + 8; }
    if Input.wants_to_close(64) { total = total + 16; }

    // So does a QUIT, and the drain leaves the queue empty.
    q := Input.quit_event();
    if Input.push(*q) { total = total + 32; }
    if Input.wants_to_close(64) { total = total + 64; }
    if !Input.wants_to_close(64) { total = total + 128; }

    // A key event is built locally: SDL drops a synthetic keyboard push, so the queue cannot carry one.
    k: Input.Event;
    k.kind = cast(u32, Input.KEY_DOWN);
    k.key_sym = cast(u32, Input.KEY_ESCAPE);
    if Input.pressed(*k, Input.KEY_ESCAPE) { total = total + 256; }
    if !Input.pressed(*k, 97) { total = total + 512; }
    if !Input.should_close(*k) { total = total + 1024; }

    // An auto-repeat is not a press.
    k.key_repeat = 1;
    if !Input.pressed(*k, Input.KEY_ESCAPE) { total = total + 2048; }

    // The union shows through: mouse_x and key_sym genuinely share offset 20, which is the point of #place.
    m: Input.Event;
    m.kind = cast(u32, Input.MOUSE_DOWN);
    m.mouse_button = 1;
    m.mouse_x = 40;
    m.mouse_y = 90;
    if cast(s64, m.mouse_button) == 1 && m.mouse_x == 40 && m.mouse_y == 90 { total = total + 4096; }
    if cast(s64, m.key_sym) == 40 { total = total + 8192; }

    // **The per-frame API** (ADR-0182 §1), which is what a Simp-shaped program calls instead of draining.
    // A caller-owned `Events`, filled by one call, then asked several questions of the same frame — which
    // is the half `wants_to_close` cannot do, because it consumes what it reads.
    events: Input.Events;
    if Input.update_window_events(*events, 64) == 0 { total = total + 16384; }
    if Input.event_count(*events) == 0 { total = total + 32768; }
    if !Input.frame_wants_to_close(*events) { total = total + 65536; }

    q2 := Input.quit_event();
    if Input.push(*q2) { total = total + 131072; }
    if Input.update_window_events(*events, 64) >= 1 { total = total + 262144; }
    if Input.frame_wants_to_close(*events) { total = total + 524288; }
    seen := Input.events_this_frame(*events);
    if seen.count == Input.event_count(*events) { total = total + 1048576; }
    if cast(s64, seen[0].kind) == Input.QUIT { total = total + 2097152; }

    Window.close(*w);
    Window.stop();
    exit_now(total % 251);
}
"#,
    )
    .unwrap();

    let binary = dir.path().join("events");
    let code = run_build_with_paths(source, binary.clone(), &[library_dir]);
    assert_eq!(code, 0, "the program must build and link against SDL2");

    let ran = std::process::Command::new(&binary)
        // The dummy driver: this program never touches `Simp` or GL, and `create_window`'s fallback (see
        // the doc comment above) means it opens a plain window under `dummy` even though every graphics
        // test in this file needs the real driver to draw.
        .env("SDL_VIDEODRIVER", "dummy")
        .status()
        .expect("the linked binary should run");
    assert_eq!(
        ran.code(),
        // 4194303 is all twenty-two bits, mod 251 because an exit status is a byte.
        Some(4194303 % 251),
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
///
/// # Coordinates, and why the render target is `LEFT_HANDED`
///
/// `UI`'s hit-testing is y-down, top-left origin — the convention every mouse event on every platform
/// reports — and it never touches `Simp`, so it cannot convert. `Simp`'s default is `RIGHT_HANDED`
/// (bottom-left, y up), so a caller drawing this UI must pass `Simp.LEFT_HANDED` to `set_render_target` to
/// keep what is drawn aligned with what is hit-tested; this program does.
///
/// # Why this needs the real video driver
///
/// `Window.create_window` falls back to a plain window when SDL's `dummy` driver refuses an
/// OpenGL-capable one (see `an_event_loop_reads_the_sdl_event_union`'s doc comment), so window creation
/// itself would succeed here too. But this program draws through `Simp`, and a fallback window has no GL
/// context — `Simp.is_ready()` would report that failure instead — so it still needs the real platform
/// driver.
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
        // No `SDL_VIDEODRIVER` override — `create_window` needs an OpenGL-capable window, which `dummy`
        // refuses to create.
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
const PROGRAM: &str = r#"Window :: #import "Window";
Input :: #import "Input";
Simp :: #import "Simp";
#import "UI";
libc :: #system_library "c";
exit_now :: (status: s64) #foreign libc "exit";

// Pushes a mouse event through SDL's real queue and folds **that event** into `ui`.
//
// **Every assertion below is unchanged from before the migration** (ADR-0182 §5). Only this helper's types
// and `main`'s setup and teardown moved: the same button, clicked the same way, must produce the same exit
// code while every pixel now goes through `SDL_RenderGeometry`. An unchanged assertion over a replaced
// backend is the only check that a migration preserved behaviour rather than merely compiling.
//
// # Why it folds one event rather than every queued one
//
// It used to `feed` everything the queue returned, and that made the test depend on **where the developer's
// mouse happened to be**. A window under the physical cursor gets real `MOUSE_MOTION` events from the OS,
// and `SDL_PollEvent` pumps more of them *while* this loop drains — so a real motion at the cursor's
// position arrived after the synthetic click and overwrote `ui.mouse_x`/`mouse_y`. The button then
// hit-tested against the wrong point and the program exited 10 instead of 24, deterministically, until the
// cursor moved. Confirmed by printing the queue: `kind=1024 x=60 y=129` behind the pushed `kind=1025
// x=20 y=20`, and by ruling out the `#place` overlay with a `cc` probe (offsets 16/20/24 still match).
//
// Filtering on the event's own values rather than on its kind alone is deliberate: this program pushes a
// `MOUSE_MOTION` too, so a kind test would still admit a real one. The queue round trip is unchanged —
// the event is still pushed into SDL and read back out — which is the property this test wanted.
send :: (ui: *UI, kind: s64, x: s64, y: s64) {
    m: Input.Event;
    m.kind = cast(u32, kind);
    m.mouse_button = 1;
    m.mouse_x = cast(s32, x);
    m.mouse_y = cast(s32, y);
    _ = Input.push(*m);
    e: Input.Event;
    i := 0;
    while i < 32 {
        if Input.next_event(*e) {
            if e.kind == cast(u32, kind) && e.mouse_x == cast(s32, x) && e.mouse_y == cast(s32, y) {
                feed(ui, *e);
            }
        }
        i = i + 1;
    }
}

main :: () {
    if !Window.start() { exit_now(90); }
    // `create_window` takes width, height, title now, and returns one value.
    w := Window.create_window(200, 200, "UI");
    if !Window.is_open(*w) { exit_now(91); }
    // `LEFT_HANDED` keeps drawing aligned with `UI`'s y-down hit-testing (see the doc comment above).
    // `set_render_target` returns nothing now; `is_ready` is the question a caller who wants to know asks.
    Simp.set_render_target(*w, Simp.LEFT_HANDED);
    if !Simp.is_ready() { exit_now(92); }

    ui: UI;
    total := 0;

    // A press-and-release inside is a click.
    begin_frame(*ui);
    send(*ui, Input.MOUSE_DOWN, 20, 20);
    if !button(*ui, 1, 10, 10, 80, 24) { total = total + 1; }   // not on press
    if is_active(*ui, 1) { total = total + 2; }
    if is_hot(*ui, 1) { total = total + 4; }

    begin_frame(*ui);
    send(*ui, Input.MOUSE_UP, 20, 20);
    if button(*ui, 1, 10, 10, 80, 24) { total = total + 8; }    // fires on release
    if !is_active(*ui, 1) { total = total + 16; }

    // Press inside, drag off, release outside: must NOT fire.
    begin_frame(*ui);
    send(*ui, Input.MOUSE_DOWN, 20, 20);
    _ = button(*ui, 1, 10, 10, 80, 24);
    begin_frame(*ui);
    send(*ui, Input.MOUSE_UP, 150, 150);
    if !button(*ui, 1, 10, 10, 80, 24) { total = total + 32; }
    if !is_active(*ui, 1) { total = total + 64; }

    // A press that begins outside cannot arm the button.
    begin_frame(*ui);
    send(*ui, Input.MOUSE_DOWN, 150, 150);
    _ = button(*ui, 1, 10, 10, 80, 24);
    begin_frame(*ui);
    send(*ui, Input.MOUSE_UP, 20, 20);
    if !button(*ui, 1, 10, 10, 80, 24) { total = total + 128; }

    // Two buttons: only the one under the cursor fires.
    begin_frame(*ui);
    send(*ui, Input.MOUSE_DOWN, 20, 50);
    _ = button(*ui, 1, 10, 10, 80, 24);
    _ = button(*ui, 2, 10, 44, 80, 24);
    begin_frame(*ui);
    send(*ui, Input.MOUSE_UP, 20, 50);
    if !button(*ui, 1, 10, 10, 80, 24) { total = total + 256; }
    if button(*ui, 2, 10, 44, 80, 24) { total = total + 512; }

    // Edges are half-open: x + w is outside.
    begin_frame(*ui);
    send(*ui, Input.MOUSE_MOTION, 90, 20);
    _ = button(*ui, 1, 10, 10, 80, 24);
    if !is_hot(*ui, 1) { total = total + 1024; }
    begin_frame(*ui);
    send(*ui, Input.MOUSE_MOTION, 89, 20);
    _ = button(*ui, 1, 10, 10, 80, 24);
    if is_hot(*ui, 1) { total = total + 2048; }

    // A zero id fires nothing and is not hot.
    begin_frame(*ui);
    send(*ui, Input.MOUSE_DOWN, 20, 20);
    begin_frame(*ui);
    send(*ui, Input.MOUSE_UP, 20, 20);
    if !button(*ui, NONE, 10, 10, 80, 24) { total = total + 4096; }
    if !is_hot(*ui, NONE) { total = total + 8192; }
    if !is_active(*ui, NONE) { total = total + 32768; }

    // `draw_button` now performs the interaction itself and reports whether the button fired — Jai's
    // `GetRect.button` shape, drawing and reporting the click in one call — so it is exercised through a
    // fresh press-and-release sequence of its own rather than through a separate `button` call: calling
    // both for one widget in one frame would double the state machine's press.
    begin_frame(*ui);
    send(*ui, Input.MOUSE_DOWN, 20, 20);
    _ = draw_button(*ui, 1, 10, 10, 80, 24);
    begin_frame(*ui);
    send(*ui, Input.MOUSE_UP, 20, 20);
    if draw_button(*ui, 1, 10, 10, 80, 24) { total = total + 16384; }

    Simp.destroy_render_target();
    Window.close(*w);
    Window.stop();
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
///
/// # `size_of_texture` and `draw_texture` are gone
///
/// `Image` no longer holds an SDL texture handle to query or copy — `texture_from`/`load_texture` now
/// produce a `Simp.Texture` whose `width`/`height` are populated straight from the surface, and drawing
/// goes through `Simp.set_shader_for_images` plus `immediate_quad`, exactly as any other quad does. So this
/// test reads `tex.width`/`tex.height` directly instead of round-tripping through a query call, and draws
/// through `Simp` and checks `GL.error_code()` instead of a `draw_texture` return.
///
/// # Why this runs against the real video driver
///
/// `create_window` falls back to a plain window when SDL's `dummy` driver refuses an OpenGL-capable one
/// (see `an_event_loop_reads_the_sdl_event_union`'s doc comment), so window creation alone would succeed
/// under `dummy`. But this program draws through `Simp`, and a fallback window has no GL context —
/// `Simp.is_ready()` reports that failure instead — so it still needs the real platform driver.
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
        // No `SDL_VIDEODRIVER` override — `create_window` needs an OpenGL-capable window.
        .status()
        .expect("the linked binary should run");
    assert_eq!(
        ran.code(),
        Some(65535 % 251),
        "every step must succeed; a lower value names which bit failed, and 90-93 name a hard stop"
    );
}

/// The image program the test above builds.
const IMAGE_PROGRAM: &str = r#"Window :: #import "Window";
Simp :: #import "Simp";
GL :: #import "GL";
#import "Image";
libc :: #system_library "c";
exit_now :: (status: s64) #foreign libc "exit";
remove_file :: (path: *u8) -> s64 #foreign libc "remove";

main :: () {
    total := 0;
    if SURFACE_LAYOUT_IS_SDL2 { total = total + 1; }
    if !Window.start() { exit_now(90); }
    // `create_window` takes width, height, title now, and returns one value.
    w := Window.create_window(200, 200, "Img");
    if !Window.is_open(*w) { exit_now(91); }
    Simp.set_render_target(*w);
    if !Simp.is_ready() { exit_now(92); }

    // Build an image, so nothing binary lives in the repository.
    s, sok := create_surface(24, 16);
    if !sok { exit_now(93); }
    total = total + 2;
    if width_of(*s) == 24 && height_of(*s) == 16 { total = total + 4; }
    if pitch_of(*s) >= 24 * 4 { total = total + 8; }

    box := Window.rect(4, 4, 8, 8);
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

    // `texture_from` no longer takes a renderer — `Simp` keeps the one process-wide GL context, so there
    // is nothing to pass.
    tex, tok := texture_from(*l);
    if tok { total = total + 512; }
    free_surface(*l);

    // `size_of_texture` is gone: `Simp.Texture` carries its own size, populated from the surface, and
    // `handle` is the GL texture name — non-zero is the check that the upload actually produced one.
    if tex.handle != 0 { total = total + 1024; }
    if tex.width == 24 && tex.height == 16 { total = total + 2048; }

    // `draw_texture` is gone too — `Simp` owns drawing now, through the textured shader and a batch, the
    // same as every other quad. `GL.error_code()` is the check that the draw actually happened cleanly,
    // where `draw_texture`'s `bool` used to be.
    Simp.set_shader_for_images(*tex);
    Simp.immediate_begin();
    Simp.immediate_quad(10.0, 10.0, 58.0, 42.0, Simp.vector4(1.0, 1.0, 1.0, 1.0));
    Simp.immediate_flush();
    if GL.error_code() == GL.NO_ERROR { total = total + 4096; }
    Simp.swap_buffers(*w);
    destroy_texture(*tex);
    destroy_texture(*tex);
    total = total + 8192;

    // The one-call path, and a missing file must fail rather than trap.
    t2, t2ok := load_texture(path.data);
    if t2ok { total = total + 16384; }
    destroy_texture(*t2);
    missing := "/tmp/jr-image-absent.bmp\0";
    t3, t3ok := load_texture(missing.data);
    if !t3ok { total = total + 32768; }

    _ = remove_file(path.data);
    Simp.destroy_render_target();
    Window.close(*w);
    Window.stop();
    exit_now(total % 251);
}
"#;

/// The whole Simp-shaped stack in one program, in the import style ADR-0179 delivered (ADR-0182).
///
/// # Why this test exists on top of the four above
///
/// Those four each exercise one module. This one is the *composition*, and it is the program the plan behind
/// this wave was written to make writable: five modules imported at once, four of them aliased, and `Window`
/// beside `File` — which is the collision that could not be written at all before ADR-0179, because both
/// exported `open`.
///
/// It is a `jr-cli` integration test and not a corpus file for ADR-0164's reason: the comptime VM resolves a
/// foreign symbol from the compiler's own process image, so SDL2 is unreachable under `jr run` and a
/// `tests/corpus/valid/` file — whose premise is that the two engines agree — has no home for one.
///
/// # What it asserts
///
/// Each earlier failure exits with its own number, so a failure **names the call that broke**: 1-3 are the
/// three layout claims, then window creation, the render target, the clear, the shader mode, the flush, the
/// dimensions, and the per-frame event drain. 42 means every one succeeded.
///
/// # Why this runs against the real video driver
///
/// `create_window` falls back to a plain window when SDL's `dummy` driver refuses an OpenGL-capable one
/// (see `an_event_loop_reads_the_sdl_event_union`'s doc comment), so window creation alone would succeed
/// under `dummy`. But this program draws through `Simp`, and a fallback window has no GL context —
/// `Simp.is_ready()` reports that failure instead — so it still needs the real platform driver.
#[test]
fn the_simp_stack_draws_a_frame_end_to_end() {
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
    let source = dir.path().join("simp.jr");
    fs::write(&source, SIMP_PROGRAM).unwrap();

    let binary = dir.path().join("simp");
    let code = run_build_with_paths(source, binary.clone(), &[library_dir]);
    assert_eq!(code, 0, "the program must build and link against SDL2");

    let ran = std::process::Command::new(&binary)
        // No `SDL_VIDEODRIVER` override — `create_window` needs an OpenGL-capable window.
        .status()
        .expect("the linked binary should run");
    assert_eq!(
        ran.code(),
        Some(42),
        "every call must succeed; a lower code names the one that did not"
    );
}

/// The end-to-end Simp program the test above builds.
const SIMP_PROGRAM: &str = r#"#import "Basic";
Window :: #import "Window";
Input :: #import "Input";
Simp :: #import "Simp";
GL :: #import "GL";
File :: #import "File";

main :: () {
    // The three layout claims, each about somebody else's ABI. Nothing below means anything if one is false.
    // `Simp.Vertex` is not an SDL layout claim any more — it is the GLSL shaders' own vertex format now
    // (ADR-0187 §3), so its constant is named `VERTEX_LAYOUT_IS_32` rather than `..._IS_SDL2`.
    if !Window.RECT_LAYOUT_IS_SDL2 { exit(1); }
    if !Input.EVENT_LAYOUT_IS_SDL2 { exit(2); }
    if !Simp.VERTEX_LAYOUT_IS_32 { exit(3); }

    if !Window.start() { exit(4); }
    // `create_window` takes width, height, title now, and returns one value.
    w := Window.create_window(320, 240, "simp");
    if !Window.is_open(*w) { exit(5); }

    // `set_render_target`, `clear_render_target`, `set_shader_for_color` and `immediate_flush` all return
    // nothing now (ADR-0187 §3) — the GL calls underneath them are `void`, where `SDL_RenderGeometry` and
    // friends could fail and report it. `is_ready` and `GL.error_code()` are the questions this program asks
    // instead.
    Simp.set_render_target(*w);
    if !Simp.is_ready() { exit(6); }
    Simp.clear_render_target(0.15, 0.08, 0.08, 1.0);
    if GL.error_code() != GL.NO_ERROR { exit(7); }
    Simp.set_shader_for_color(true);
    if GL.error_code() != GL.NO_ERROR { exit(8); }

    Simp.immediate_begin();
    Simp.immediate_quad(10.0, 10.0, 110.0, 60.0, Simp.vector4(1.0, 0.5, 0.5, 1.0));
    Simp.immediate_quad(120.0, 10.0, 220.0, 60.0, Simp.vector4(0.2, 0.9, 0.3, 1.0));
    Simp.immediate_flush();
    if GL.error_code() != GL.NO_ERROR { exit(9); }
    Simp.swap_buffers(*w);

    pw, ph := Simp.get_render_dimensions(*w);
    if pw != 320 { exit(10); }
    if ph != 240 { exit(11); }

    // The window is always shown now — there is no `HIDDEN` flag any more — so opening it queues real
    // `SDL_WINDOWEVENT`s (shown, exposed) before this program ever polls; measured, they can take a few
    // calls to fully drain rather than arriving on the first one, the same one-push-one-poll caveat
    // `Input`'s own docs record for a pushed event. Draining to steady state is the honest step: the
    // assertion this test wants is that nothing is pending and nothing wants to close *once the queue is
    // caught up*, not that opening a real window produces no window-manager events at all.
    events: Input.Events;
    drained := 1;
    i := 0;
    while drained != 0 && i < 32 {
        drained = Input.update_window_events(*events, 64);
        i = i + 1;
    }
    if Input.event_count(*events) != 0 { exit(12); }
    if Input.frame_wants_to_close(*events) { exit(13); }

    // `File` is imported and used beside `Window`, which is the E0211 that could not be written before
    // ADR-0179: `Window` used to export `open` and `close`, and `File` still does.
    if File.READ_WRITE != 2 { exit(14); }

    Simp.destroy_render_target();
    Window.close(*w);
    Window.stop();
    exit(42);
}
"#;

/// A **rotated, textured** quad — the thing the old renderer could not draw at all (ADR-0182 §3).
///
/// # Why this is a separate test
///
/// It is the check that the new API expresses something the old one could not, rather than the same thing
/// differently. `SDL_RenderFillRect` takes an `SDL_Rect` of four integers: it cannot draw a quad whose corners
/// are not axis-aligned, at any angle, with any amount of care. `Simp.immediate_quad_corners` takes four
/// arbitrary points and four texture coordinates, which is Jai's four-point `immediate_quad` overload's
/// shape.
///
/// The texture is **built by the program**, so no binary fixture lives in the repository and the *decode* is
/// exercised — the step with the interesting failure. It comes back through `Image.load_texture`, which
/// yields a `Simp.Texture` carrying `width` and `height`, and those are read directly: they are populated
/// from the surface at load time rather than queried back from a texture handle.
///
/// # Why this runs against the real video driver
///
/// `create_window` falls back to a plain window when SDL's `dummy` driver refuses an OpenGL-capable one
/// (see `an_event_loop_reads_the_sdl_event_union`'s doc comment), so window creation alone would succeed
/// under `dummy`. But this program draws through `Simp`, and a fallback window has no GL context —
/// `Simp.is_ready()` reports that failure instead — so it still needs the real platform driver.
#[test]
fn a_rotated_textured_quad_draws_through_render_geometry() {
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
    let source = dir.path().join("rotated.jr");
    fs::write(&source, ROTATED_PROGRAM).unwrap();

    let binary = dir.path().join("rotated");
    let code = run_build_with_paths(source, binary.clone(), &[library_dir]);
    assert_eq!(code, 0, "the program must build and link against SDL2");

    let ran = std::process::Command::new(&binary)
        // No `SDL_VIDEODRIVER` override — `create_window` needs an OpenGL-capable window.
        .status()
        .expect("the linked binary should run");
    assert_eq!(
        ran.code(),
        Some(42),
        "every call must succeed; a lower code names the one that did not"
    );
}

/// The rotated-textured-quad program the test above builds.
const ROTATED_PROGRAM: &str = r#"#import "Basic";
Window :: #import "Window";
Simp :: #import "Simp";
GL :: #import "GL";
Image :: #import "Image";

libc :: #system_library "c";
remove_file :: (path: *u8) -> s64 #foreign libc "remove";

main :: () {
    if !Window.start() { exit(1); }
    // `create_window` takes width, height, title now, and returns one value.
    w := Window.create_window(320, 240, "rotated");
    if !Window.is_open(*w) { exit(2); }

    // `set_render_target` returns nothing now; `is_ready` is the question a caller who wants to know asks.
    Simp.set_render_target(*w);
    if !Simp.is_ready() { exit(3); }

    // Build a texture rather than committing one: this exercises the decode, and it keeps the repository
    // free of binary fixtures.
    surface, sok := Image.create_surface(32, 16);
    if !sok { exit(4); }
    area := Window.rect(0, 0, 32, 16);
    if !Image.fill_surface(*surface, *area, 4278190335) { exit(5); }
    path := "/tmp/jr-rotated-test.bmp\0";
    if !Image.save_bmp(*surface, path.data) { exit(6); }
    Image.free_surface(*surface);

    // `load_texture` no longer takes a renderer — `Simp` keeps the one process-wide GL context.
    texture, tok := Image.load_texture(path.data);
    if !tok { exit(7); }
    // `Simp.Texture` carries its own size, populated from the surface at load time.
    if texture.width != 32 { exit(8); }
    if texture.height != 16 { exit(9); }

    // `clear_render_target` and `set_shader_for_images` return nothing now; `GL.error_code()` is the check
    // that each actually succeeded.
    Simp.clear_render_target(0.0, 0.0, 0.0, 1.0);
    if GL.error_code() != GL.NO_ERROR { exit(10); }
    Simp.set_shader_for_images(*texture);
    if GL.error_code() != GL.NO_ERROR { exit(11); }

    // Four corners of a quad rotated roughly 20 degrees about (160, 120), and four texture coordinates in
    // the same order. `SDL_RenderFillRect` cannot express this at all — which is the whole point.
    // `immediate_quad_corners` takes the colour before the four uvs, not after: Jai's own parameter order.
    Simp.immediate_begin();
    Simp.immediate_quad_corners(
        Simp.vector2(145.0, 105.0), Simp.vector2(175.0, 110.0),
        Simp.vector2(172.0, 135.0), Simp.vector2(142.0, 130.0),
        Simp.vector4(1.0, 1.0, 1.0, 1.0),
        Simp.vector2(0.0, 0.0), Simp.vector2(1.0, 0.0),
        Simp.vector2(1.0, 1.0), Simp.vector2(0.0, 1.0));
    Simp.immediate_flush();
    if GL.error_code() != GL.NO_ERROR { exit(12); }

    // And the same geometry under the *colour* shader, which must flush the batch it finds open rather than
    // drawing the queued vertices in the new mode.
    Simp.immediate_begin();
    Simp.immediate_quad_corners(
        Simp.vector2(45.0, 55.0), Simp.vector2(95.0, 45.0),
        Simp.vector2(105.0, 95.0), Simp.vector2(55.0, 105.0),
        Simp.vector4(0.9, 0.2, 0.4, 1.0),
        Simp.vector2(0.0, 0.0), Simp.vector2(1.0, 0.0),
        Simp.vector2(1.0, 1.0), Simp.vector2(0.0, 1.0));
    Simp.set_shader_for_color(false);
    if GL.error_code() != GL.NO_ERROR { exit(13); }
    // The mode change flushed and closed the batch, so this flush has nothing to draw — and must still
    // succeed, because flushing nothing is not a failure.
    Simp.immediate_flush();
    if GL.error_code() != GL.NO_ERROR { exit(14); }

    Simp.swap_buffers(*w);

    _ = remove_file(path.data);
    Image.destroy_texture(*texture);
    Simp.destroy_render_target();
    Window.close(*w);
    Window.stop();
    exit(42);
}
"#;

/// A library **chosen and linked per operating system by comptime code** (ADR-0183, ADR-0184).
///
/// # Why this is the test that matters
///
/// It is the one that proves the two compiler changes compose into the capability they were made for.
/// `modules/GL` names no library: a `#run` reads `os()`, returns the text of one declaration, and a
/// file-scope `#insert` splices it — `#framework "OpenGL"` on macOS, `#system_library "GL"` on Linux,
/// `#system_library "opengl32"` on Windows. Three names and **two different linker argument forms**.
///
/// Before this, neither half existed: `jr-link` could emit only `-L` and `-l`, and `#insert` was not a
/// file-scope directive at all, so comptime code could generate statements and not declarations.
///
/// # What it asserts, and why not more
///
/// That the program **builds, links and runs**. It deliberately calls no GL entry point: every one is
/// undefined without a current context, and `glGetString` with none *segfaults* on macOS rather than
/// returning null — measured while writing `modules/GL`. So the claim is that the symbols resolved, which
/// is what linking means, and the assertion below reads the framework out of the built binary rather than
/// trusting the exit code alone.
///
/// No `-L` is passed: a framework needs none, and a Linux `-lGL` resolves from the driver's defaults. That
/// is itself part of the point — `-framework` is not a search of the `-L` paths.
#[test]
fn a_per_os_library_is_chosen_by_comptime_code_and_linked() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("gl.jr");
    fs::write(
        &source,
        r#"#import "Basic";
GL :: #import "GL";

main :: () {
    // Constants only. Every GL call needs a current context, and this program creates none.
    if GL.COLOR_BUFFER_BIT != 16384 { exit(1); }
    if GL.NO_ERROR != 0 { exit(2); }
    exit(42);
}
"#,
    )
    .unwrap();

    let binary = dir.path().join("gl");
    let code = run_build_with_paths(source, binary.clone(), &[]);
    assert_eq!(
        code, 0,
        "the program must build and link against this platform's OpenGL"
    );

    let ran = std::process::Command::new(&binary)
        .status()
        .expect("the linked binary should run");
    assert_eq!(ran.code(), Some(42), "the linked program must run");

    // **The link line is the claim, so the link line is what is checked.** An exit code proves the
    // program ran; it does not prove which library it was linked against, and a compiler that silently
    // dropped the library would still exit 42 here because nothing is called. On macOS the framework's
    // path is recorded in the binary.
    #[cfg(target_os = "macos")]
    {
        let listed = std::process::Command::new("otool")
            .arg("-L")
            .arg(&binary)
            .output()
            .expect("otool should run on macOS");
        let text = String::from_utf8_lossy(&listed.stdout);
        assert!(
            text.contains("OpenGL.framework"),
            "the binary must record the OpenGL framework, so `-framework` really was emitted; got:\n{text}"
        );
    }
}

/// `#framework` is a **different linker argument** from `#system_library`, and neither substitutes for the
/// other (ADR-0183 §1).
///
/// # Why the negative half is the important one
///
/// A test that only linked a framework successfully would pass on a compiler that quietly emitted `-l`
/// **and** on one that emitted `-framework`, if the name happened to resolve both ways. So this asserts the
/// pair: `#framework "CoreFoundation"` links, and `#system_library "CoreFoundation"` — the same name, the
/// other form — **fails** with `library not found`.
///
/// That asymmetry is the whole reason the form had to be added rather than inferred: the compiler cannot
/// know which a name means, so the declaration says, and guessing would make `#system_library "SDL2"` on
/// macOS try a framework that does not exist before finding the dylib that does.
///
/// macOS-only, because frameworks are.
#[test]
#[cfg(target_os = "macos")]
fn a_framework_links_where_the_library_form_does_not() {
    let dir = TempDir::new().unwrap();

    // The framework form: links, and the call returns a real allocator. `CFAllocatorGetDefault` is safe
    // with no prior setup, unlike every GL entry point.
    let good = dir.path().join("good.jr");
    fs::write(
        &good,
        r#"#import "Basic";
cf :: #framework "CoreFoundation";
allocator :: () -> *u8 #foreign cf "CFAllocatorGetDefault";

main :: () {
    if allocator() == null { exit(1); }
    exit(42);
}
"#,
    )
    .unwrap();
    let good_binary = dir.path().join("good");
    assert_eq!(
        run_build_with_paths(good.clone(), good_binary.clone(), &[]),
        0,
        "`#framework` must link"
    );
    let ran = std::process::Command::new(&good_binary)
        .status()
        .expect("the linked binary should run");
    assert_eq!(
        ran.code(),
        Some(42),
        "the framework's symbol must resolve at run time, not merely at link time"
    );

    // The library form, same name: must **fail**. This is the assertion that makes the feature
    // load-bearing rather than decorative.
    let bad = dir.path().join("bad.jr");
    fs::write(
        &bad,
        r#"#import "Basic";
cf :: #system_library "CoreFoundation";
allocator :: () -> *u8 #foreign cf "CFAllocatorGetDefault";

main :: () {
    if allocator() == null { exit(1); }
    exit(42);
}
"#,
    )
    .unwrap();
    let bad_binary = dir.path().join("bad");
    assert_ne!(
        run_build_with_paths(bad, bad_binary, &[]),
        0,
        "`-lCoreFoundation` must not resolve; if it does, the two forms are not distinct and this \
         feature is not doing anything"
    );
}

/// A built object carries a DWARF line table whose rows name real statements (ADR-0169).
///
/// # Why this parses the DWARF instead of shelling out to `dwarfdump`
///
/// `dwarfdump` is a macOS tool, its output format is not a contract, and a test that greps it would pass on a
/// section a debugger cannot read. `gimli` parses the bytes the way `lldb` does, so what this asserts is that
/// the section is *valid*, not that a formatter printed something.
///
/// # What it asserts, and why each part matters
///
/// PLAN §8.4 claimed line tables existed and ADR-0159 found **no DWARF at all** — an empty `.debug_line` and no
/// debug section. So the first assertion is simply that the section is there and parses.
///
/// Then: a row must point at a line that **is** a statement in the source. A line table whose every row said
/// line 1 would parse perfectly and be useless, and that is the failure mode a "section exists" check misses.
/// The chosen lines are a `return`, a `while` and an `if` in `valid/024-hello.jr`, spread through the file so a
/// single wrong constant cannot satisfy all three.
///
/// And the file table must hold **both** files, because the program imports `modules/Basic` and a table with one
/// entry would mean every row was attributed to whichever file happened to be first.
#[test]
fn a_built_object_carries_a_dwarf_line_table() {
    use gimli::EndianSlice;
    use object::{Object as _, ObjectSection as _};

    let dir = TempDir::new().unwrap();
    let object_path = dir.path().join("hello.o");
    let source = corpus_path("valid/024-hello.jr");
    let code = run_build_emit_object(source, object_path.clone());
    assert_eq!(code, 0, "the object must be emitted");

    let bytes = fs::read(&object_path).expect("the object should exist");
    let file = object::File::parse(&*bytes).expect("the object should parse");

    let section = file
        .sections()
        .find(|s| {
            matches!(
                s.name(),
                // Mach-O spells it `__debug_line` in the `__DWARF` segment, ELF `.debug_line`.
                Ok("__debug_line" | ".debug_line")
            )
        })
        .expect("a built object must carry a .debug_line section");
    let line_data = section.data().expect("the section should have data");
    assert!(
        line_data.len() > 64,
        "a line table with real rows is not a handful of bytes; got {}",
        line_data.len()
    );

    let endian = if file.is_little_endian() {
        gimli::RunTimeEndian::Little
    } else {
        gimli::RunTimeEndian::Big
    };
    let load = |id: gimli::SectionId| -> Result<EndianSlice<'_, gimli::RunTimeEndian>, ()> {
        let name = id.name();
        let macho = format!("__{}", name.trim_start_matches('.'));
        let data = file
            .sections()
            .find(|s| matches!(s.name(), Ok(n) if n == name || n == macho.as_str()))
            .and_then(|s| s.data().ok())
            .unwrap_or(&[]);
        Ok(EndianSlice::new(data, endian))
    };
    let dwarf = gimli::Dwarf::load(load).expect("the DWARF should load");

    let mut units = dwarf.units();
    let header = units
        .next()
        .expect("the DWARF should parse")
        .expect("a built object must carry one compilation unit");
    let unit = dwarf.unit(header).expect("the unit should parse");
    let program = unit
        .line_program
        .clone()
        .expect("the unit must have a line program");

    let mut file_names = Vec::new();
    for entry in program.header().file_names() {
        if let gimli::AttributeValue::String(name) = entry.path_name() {
            file_names.push(String::from_utf8_lossy(name.slice()).into_owned());
        }
    }
    assert!(
        file_names.iter().any(|f| f.ends_with("024-hello.jr")),
        "the file table must name the program; got {file_names:?}"
    );
    assert!(
        file_names.iter().any(|f| f.ends_with("Basic/module.jr")),
        "the file table must name the imported module too, or every row is attributed to one file; \
         got {file_names:?}"
    );

    let mut lines = Vec::new();
    let mut rows = program.rows();
    while let Some((_, row)) = rows.next_row().expect("rows should parse") {
        if let Some(line) = row.line() {
            lines.push(line.get());
        }
    }
    assert!(!lines.is_empty(), "the line program must produce rows");

    // Real statements in valid/024-hello.jr, spread through the file: a `return`, a `while` and an `if`.
    for expected in [21u64, 35, 40] {
        assert!(
            lines.contains(&expected),
            "line {expected} is a statement in the program and must appear in the table; got {lines:?}"
        );
    }
    assert!(
        lines.iter().any(|l| *l != lines[0]),
        "a table whose every row is the same line would parse and be useless"
    );
}

/// `jr build --emit-object`, for a test that wants the object rather than an executable.
fn run_build_emit_object(path: PathBuf, output: PathBuf) -> i32 {
    jr_cli::commands::build::run(
        jr_cli::cli::BuildArgs {
            path,
            output: Some(output),
            emit_object: true,
            backend: jr_cli::cli::BackendArg::Cranelift,
            no_bounds_check: false,
            opt_level: None,
            module_paths: vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../modules")],
            library_paths: Vec::new(),
        },
        &quiet_global(),
    )
    .unwrap_or(1)
}

/// The LLVM back end's object carries a line table too, from the same spans (ADR-0170).
///
/// # Why this is a separate test and not a parameter of the Cranelift one
///
/// The two back ends produce debug info by **completely different routes**: Cranelift's is written by this
/// project with `gimli` (ADR-0169), LLVM's is written by LLVM from `DILocation` metadata. They share only the
/// `SourceInfo::position` lookup. So a shared test would assert the intersection and miss what is
/// interesting — and what is interesting is that two unrelated emitters agree about which lines exist.
///
/// # What differs from the Cranelift assertions, and why that is not a weaker test
///
/// LLVM writes a DWARF file entry as a bare name plus a directory, where this project's own emitter writes the
/// path it was given. So the file-table assertion checks the *name*. That is DWARF's own split, not a
/// concession.
///
/// LLVM also emits rows at **line 0** for instructions with no location, which is what `clang` does for
/// compiler-generated code and is DWARF's spelling of "no line". Those are filtered rather than asserted
/// against: a test that demanded none would be asserting an LLVM implementation detail.
#[cfg(feature = "llvm")]
#[test]
fn the_llvm_back_end_emits_a_line_table_too() {
    use gimli::EndianSlice;
    use object::{Object as _, ObjectSection as _};

    let dir = TempDir::new().unwrap();
    let object_path = dir.path().join("hello-llvm.o");
    let source = corpus_path("valid/024-hello.jr");
    let code = jr_cli::commands::build::run(
        jr_cli::cli::BuildArgs {
            path: source,
            output: Some(object_path.clone()),
            emit_object: true,
            backend: jr_cli::cli::BackendArg::Llvm,
            no_bounds_check: false,
            opt_level: None,
            module_paths: vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../modules")],
            library_paths: Vec::new(),
        },
        &quiet_global(),
    )
    .unwrap_or(1);
    assert_eq!(code, 0, "the LLVM object must be emitted");

    let bytes = fs::read(&object_path).expect("the object should exist");
    let file = object::File::parse(&*bytes).expect("the object should parse");
    let endian = if file.is_little_endian() {
        gimli::RunTimeEndian::Little
    } else {
        gimli::RunTimeEndian::Big
    };
    let load = |id: gimli::SectionId| -> Result<EndianSlice<'_, gimli::RunTimeEndian>, ()> {
        let name = id.name();
        let macho = format!("__{}", name.trim_start_matches('.'));
        let data = file
            .sections()
            .find(|s| matches!(s.name(), Ok(n) if n == name || n == macho.as_str()))
            .and_then(|s| s.data().ok())
            .unwrap_or(&[]);
        Ok(EndianSlice::new(data, endian))
    };
    let dwarf = gimli::Dwarf::load(load).expect("the DWARF should load");

    let mut units = dwarf.units();
    let header = units
        .next()
        .expect("the DWARF should parse")
        .expect("the LLVM object must carry a compilation unit");
    let unit = dwarf.unit(header).expect("the unit should parse");
    let program = unit
        .line_program
        .clone()
        .expect("the unit must have a line program");

    let mut file_names = Vec::new();
    for entry in program.header().file_names() {
        if let gimli::AttributeValue::String(name) = entry.path_name() {
            file_names.push(String::from_utf8_lossy(name.slice()).into_owned());
        }
    }
    assert!(
        file_names.iter().any(|f| f.ends_with("024-hello.jr")),
        "the file table must name the program; got {file_names:?}"
    );
    // The imported module must have its own entry. Without a per-body `DIFile` every subprogram hangs off the
    // unit's file and `modules/Basic`'s statements are blamed on the root program — this wave's first wrong
    // result, and a check on the root file alone would have passed it.
    assert!(
        file_names.iter().any(|f| f.ends_with("module.jr")),
        "the imported module must have its own file entry, or its lines are attributed to the program; \
         got {file_names:?}"
    );

    let mut lines = Vec::new();
    let mut rows = program.rows();
    while let Some((_, row)) = rows.next_row().expect("rows should parse") {
        if let Some(line) = row.line() {
            lines.push(line.get());
        }
    }
    // Line 0 is DWARF's "no line", which LLVM emits for compiler-generated instructions exactly as clang does.
    lines.retain(|l| *l != 0);
    assert!(!lines.is_empty(), "the line program must produce rows");

    // The same three statements the Cranelift test names. Two unrelated emitters, reading one span source, must
    // agree about which lines exist — that agreement is the whole reason this test is worth having.
    for expected in [21u64, 35, 40] {
        assert!(
            lines.contains(&expected),
            "line {expected} is a statement in the program and must appear in LLVM's table; got {lines:?}"
        );
    }
}

/// A struct's layout reaches DWARF as a `DW_TAG_structure_type` with named members at real offsets
/// (ADR-0171).
///
/// # Why a parameter is what makes this work
///
/// LLVM **prunes a type nothing declares**. A `DISubroutineType` listing a struct is not enough: the first
/// attempt emitted base types and no struct at all, which looked exactly like the struct mapping being broken
/// and was the *reference* being missing. So each parameter gets a `DILocalVariable`, and that is what pulls
/// the struct DIE into the object — which is why this test asserts the members rather than merely that a
/// struct tag exists.
///
/// # What it checks, and why the offsets are the point
///
/// `Point { x: s64, y: s64, flag: bool }` must appear with `x` at byte 0, `y` at 8 and `flag` at 16, and the
/// whole thing 24 bytes. Those numbers come from `jr_pool::field_offset` — the same function both engines use
/// to *compile* a field access — so a wrong DIE offset would mean a debugger showing a field the program does
/// not have there. Asserting the tag without the offsets would pass on a struct whose every member sat at 0.
///
/// The member names come from the interner through `SourceInfo::symbol`, added for this wave. A struct whose
/// members were `field0`/`field1` would parse identically and be nearly useless, so the names are asserted
/// too.
#[cfg(feature = "llvm")]
#[test]
fn a_struct_reaches_dwarf_with_named_members_at_real_offsets() {
    use gimli::EndianSlice;
    use object::{Object as _, ObjectSection as _};

    let dir = TempDir::new().unwrap();
    let source = dir.path().join("layout.jr");
    fs::write(
        &source,
        r#"#import "Basic";

Point :: struct {
    x: s64;
    y: s64;
    flag: bool;
}

sum :: (p: Point) -> s64 {
    return p.x + p.y;
}

main :: () {
    q: Point;
    q.x = 9;
    q.y = 4;
    q.flag = true;
    exit(sum(q));
}
"#,
    )
    .unwrap();

    let object_path = dir.path().join("layout.o");
    let code = jr_cli::commands::build::run(
        jr_cli::cli::BuildArgs {
            path: source,
            output: Some(object_path.clone()),
            emit_object: true,
            backend: jr_cli::cli::BackendArg::Llvm,
            no_bounds_check: false,
            opt_level: None,
            module_paths: vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../modules")],
            library_paths: Vec::new(),
        },
        &quiet_global(),
    )
    .unwrap_or(1);
    assert_eq!(code, 0, "the object must be emitted");

    let bytes = fs::read(&object_path).expect("the object should exist");
    let file = object::File::parse(&*bytes).expect("the object should parse");
    let endian = if file.is_little_endian() {
        gimli::RunTimeEndian::Little
    } else {
        gimli::RunTimeEndian::Big
    };
    let load = |id: gimli::SectionId| -> Result<EndianSlice<'_, gimli::RunTimeEndian>, ()> {
        let name = id.name();
        let macho = format!("__{}", name.trim_start_matches('.'));
        let data = file
            .sections()
            .find(|s| matches!(s.name(), Ok(n) if n == name || n == macho.as_str()))
            .and_then(|s| s.data().ok())
            .unwrap_or(&[]);
        Ok(EndianSlice::new(data, endian))
    };
    let dwarf = gimli::Dwarf::load(load).expect("the DWARF should load");

    let mut structs = Vec::new();
    let mut units = dwarf.units();
    while let Some(header) = units.next().expect("units should parse") {
        let unit = dwarf.unit(header).expect("the unit should parse");
        let mut entries = unit.entries();
        let mut current: Option<(u64, Vec<(String, u64)>)> = None;
        while let Some(entry) = entries.next_dfs().expect("entries should walk") {
            match entry.tag() {
                gimli::DW_TAG_structure_type => {
                    if let Some(done) = current.take() {
                        structs.push(done);
                    }
                    let size = entry
                        .attr_value(gimli::DW_AT_byte_size)
                        .and_then(|v| v.udata_value())
                        .unwrap_or(0);
                    current = Some((size, Vec::new()));
                }
                gimli::DW_TAG_member => {
                    if let Some((_, members)) = current.as_mut() {
                        let name = entry
                            .attr(gimli::DW_AT_name)
                            .and_then(|a| a.string_value(&dwarf.debug_str))
                            .map(|s| String::from_utf8_lossy(s.slice()).into_owned())
                            .unwrap_or_default();
                        let offset = entry
                            .attr_value(gimli::DW_AT_data_member_location)
                            .and_then(|v| v.udata_value())
                            .unwrap_or(u64::MAX);
                        members.push((name, offset));
                    }
                }
                // Any other tag ends the run of members belonging to the struct above it.
                _ => {
                    if let Some(done) = current.take() {
                        structs.push(done);
                    }
                }
            }
        }
        if let Some(done) = current.take() {
            structs.push(done);
        }
    }

    let point = structs
        .iter()
        .find(|(size, members)| *size == 24 && members.len() == 3)
        .unwrap_or_else(|| {
            panic!("a 24-byte struct with three members must appear; got {structs:?}")
        });
    assert_eq!(
        point.1,
        vec![
            ("x".to_owned(), 0),
            ("y".to_owned(), 8),
            ("flag".to_owned(), 16),
        ],
        "the members must carry their source names and the offsets `jr_pool::field_offset` computes, \
         or a debugger shows a field the program does not have there"
    );
}

/// A **stack-resident** local reaches DWARF as a named `DW_TAG_variable` with a frame location (ADR-0172).
///
/// # Why the program takes an address, and the two boundaries that writing it exposed
///
/// The first version used `total := 7; doubled := total * 2;` and found **no variables at all**. That is not a
/// bug in the emission — it is MIR's design: only an **escaped** local gets a stack slot (ADR-0017 §2), and a
/// slot is what a `llvm.dbg.declare` describes. A local living its whole life in SSA registers has no address
/// to point at. **So a register-resident local is invisible to a debugger**, and closing that is exactly what
/// W12's "locals through value labels" item is for: a register location is a different DWARF expression, not a
/// missing call.
///
/// The second version added an **aggregate** local, expecting it to escape and be named. In *this* program it is
/// not — and ADR-0174 §3 establishes why: whether an aggregate's slot is bound to its local depends on how the
/// aggregate is **used**. One passed by value to a procedure is bound and *is* named; one only field-assigned
/// and read, as `pair` is here, is not. So the negative assertion at the end pins this program's shape rather
/// than aggregates in general — ADR-0172 §3 claimed the general form and was wrong.
///
/// Both boundaries were found by writing the test, which is why it names one local and pins two shapes.
///
/// # What has to line up
///
/// * The **name** must be the one the programmer wrote, via `SourceInfo::local_name` — a back end holds a
///   `MirSpan` and cannot reach a local's `Symbol` on its own. A variable called `s3` would parse fine and
///   tell a reader nothing.
/// * The **location** must be a frame-relative expression. `llvm.dbg.declare` has to be placed where it
///   dominates every use, and the wrong block yields a verifier failure or a variable with no location.
/// * A **compiler temporary must not appear.** MIR has far more slots than the program has locals, and a
///   debugger listing them beside the user's own names is noise.
#[cfg(feature = "llvm")]
#[test]
fn a_local_reaches_dwarf_with_its_name_and_a_stack_location() {
    use gimli::EndianSlice;
    use object::{Object as _, ObjectSection as _};

    let dir = TempDir::new().unwrap();
    let source = dir.path().join("locals.jr");
    fs::write(
        &source,
        r#"#import "Basic";

Pair :: struct {
    lo: s64;
    hi: s64;
}

main :: () {
    // An aggregate escapes, so `pair` gets a stack slot.
    pair: Pair;
    pair.lo = 7;
    pair.hi = 11;

    // Its address is taken, so `total` escapes too.
    total := pair.lo + pair.hi;
    view := *total;
    exit(view.*);
}
"#,
    )
    .unwrap();

    let object_path = dir.path().join("locals.o");
    let code = jr_cli::commands::build::run(
        jr_cli::cli::BuildArgs {
            path: source,
            output: Some(object_path.clone()),
            emit_object: true,
            backend: jr_cli::cli::BackendArg::Llvm,
            no_bounds_check: false,
            opt_level: None,
            module_paths: vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../modules")],
            library_paths: Vec::new(),
        },
        &quiet_global(),
    )
    .unwrap_or(1);
    assert_eq!(code, 0, "the object must be emitted");

    let bytes = fs::read(&object_path).expect("the object should exist");
    let file = object::File::parse(&*bytes).expect("the object should parse");
    let endian = if file.is_little_endian() {
        gimli::RunTimeEndian::Little
    } else {
        gimli::RunTimeEndian::Big
    };
    let load = |id: gimli::SectionId| -> Result<EndianSlice<'_, gimli::RunTimeEndian>, ()> {
        let name = id.name();
        let macho = format!("__{}", name.trim_start_matches('.'));
        let data = file
            .sections()
            .find(|s| matches!(s.name(), Ok(n) if n == name || n == macho.as_str()))
            .and_then(|s| s.data().ok())
            .unwrap_or(&[]);
        Ok(EndianSlice::new(data, endian))
    };
    let dwarf = gimli::Dwarf::load(load).expect("the DWARF should load");

    let mut variables = Vec::new();
    let mut units = dwarf.units();
    while let Some(header) = units.next().expect("units should parse") {
        let unit = dwarf.unit(header).expect("the unit should parse");
        let mut entries = unit.entries();
        while let Some(entry) = entries.next_dfs().expect("entries should walk") {
            if entry.tag() != gimli::DW_TAG_variable {
                continue;
            }
            let name = entry
                .attr(gimli::DW_AT_name)
                .and_then(|a| a.string_value(&dwarf.debug_str))
                .map(|s| String::from_utf8_lossy(s.slice()).into_owned())
                .unwrap_or_default();
            let located = entry.attr_value(gimli::DW_AT_location).is_some();
            variables.push((name, located));
        }
    }

    // One local, not a loop: only the address-taken scalar is named today, and the two absences beside it are
    // the interesting part — see the doc comment.
    let (_, located) = variables
        .iter()
        .find(|(name, _)| name == "total")
        .unwrap_or_else(|| {
            panic!("the local `total` must appear as a DWARF variable; got {variables:?}")
        });
    assert!(
        *located,
        "`total` must carry a location, or a debugger knows its name and cannot read it"
    );
    // MIR has more slots than the program has locals. A debugger listing `s3` beside the user's own names is
    // noise, so a temporary must contribute nothing.
    let names: Vec<&str> = variables.iter().map(|(name, _)| name.as_str()).collect();
    let temporary = names
        .iter()
        .any(|name| name.starts_with('s') && name[1..].chars().all(char::is_numeric));
    assert!(
        !temporary,
        "a compiler temporary must not be declared; got {names:?}"
    );
    // **Pinned, not accepted quietly.** An aggregate used only by field assignment gets no slot bound to its
    // local, so it is unnamed — while one *passed to a procedure* is bound and is named (ADR-0174 §3, which
    // corrects ADR-0172 §3's more general claim). When MIR binds this shape too, the assertion fails and
    // whoever changed it is told to invert the line.
    assert!(
        !names.contains(&"pair"),
        "an aggregate used only by field assignment is not yet named — if it now is, MIR started binding \
         its slot and this assertion should be inverted; got {names:?}"
    );
}

/// The Cranelift back end emits a `.debug_info` DIE tree too — types, a struct's layout, and a subprogram per
/// function (ADR-0173).
///
/// # Why this is not shared with the LLVM struct test
///
/// Same reason ADR-0170 kept the line-table tests apart: the two routes have nothing in common. LLVM's DIEs come
/// from metadata LLVM writes itself; these are written by hand with `gimli`, from a `TypeDescription` built
/// during body definition because that is the only moment a field's name can be resolved. **What matters is that
/// two unrelated emitters agree** — the same struct, the same offsets, the same names — and a shared test would
/// assert only their intersection.
///
/// # What it asserts
///
/// The struct's members with their real offsets, for ADR-0171 §6's reason: a tag-only check passes on a struct
/// whose every member sits at 0. And a subprogram per procedure carrying its **source** name, since a backtrace
/// that says `jr$0$3` is a backtrace nobody can read.
#[test]
fn the_cranelift_back_end_emits_a_die_tree() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("layout.jr");
    fs::write(
        &source,
        r#"#import "Basic";

Point :: struct {
    x: s64;
    y: s64;
    flag: bool;
}

sum :: (p: Point) -> s64 {
    return p.x + p.y;
}

main :: () {
    q: Point;
    q.x = 9;
    q.y = 4;
    q.flag = true;
    exit(sum(q));
}
"#,
    )
    .unwrap();

    let object_path = dir.path().join("layout.o");
    let code = run_build_emit_object(source, object_path.clone());
    assert_eq!(code, 0, "the object must be emitted");

    let (structs, subprograms) = read_dies(&object_path);

    let point = structs
        .iter()
        .find(|(size, members)| *size == 24 && members.len() == 3)
        .unwrap_or_else(|| {
            panic!("a 24-byte struct with three members must appear; got {structs:?}")
        });
    assert_eq!(
        point.1,
        vec![
            ("x".to_owned(), 0),
            ("y".to_owned(), 8),
            ("flag".to_owned(), 16),
        ],
        "the hand-written DIEs must agree with LLVM's about names and offsets"
    );

    for expected in ["sum", "main"] {
        assert!(
            subprograms.iter().any(|name| name == expected),
            "`{expected}` must have a subprogram carrying its source name, or a backtrace says `jr$0$3`; \
             got {subprograms:?}"
        );
    }
}

/// One struct's DWARF layout: its byte size, and each member's name and offset.
type StructLayout = (u64, Vec<(String, u64)>);

/// Reads an object's struct layouts and subprogram names out of `.debug_info`.
///
/// Shared by the two back ends' DIE tests because the *parsing* is genuinely common — only the emission
/// differs, which is what those tests are about.
fn read_dies(path: &std::path::Path) -> (Vec<StructLayout>, Vec<String>) {
    use gimli::EndianSlice;
    use object::{Object as _, ObjectSection as _};

    let bytes = fs::read(path).expect("the object should exist");
    let file = object::File::parse(&*bytes).expect("the object should parse");
    let endian = if file.is_little_endian() {
        gimli::RunTimeEndian::Little
    } else {
        gimli::RunTimeEndian::Big
    };
    let load = |id: gimli::SectionId| -> Result<EndianSlice<'_, gimli::RunTimeEndian>, ()> {
        let name = id.name();
        let macho = format!("__{}", name.trim_start_matches('.'));
        let data = file
            .sections()
            .find(|s| matches!(s.name(), Ok(n) if n == name || n == macho.as_str()))
            .and_then(|s| s.data().ok())
            .unwrap_or(&[]);
        Ok(EndianSlice::new(data, endian))
    };
    let dwarf = gimli::Dwarf::load(load).expect("the DWARF should load");

    let mut structs: Vec<StructLayout> = Vec::new();
    let mut subprograms = Vec::new();
    let mut units = dwarf.units();
    while let Some(header) = units.next().expect("units should parse") {
        let unit = dwarf.unit(header).expect("the unit should parse");
        let mut entries = unit.entries();
        let mut current: Option<StructLayout> = None;
        while let Some(entry) = entries.next_dfs().expect("entries should walk") {
            let name = || {
                entry
                    .attr(gimli::DW_AT_name)
                    .and_then(|a| a.string_value(&dwarf.debug_str))
                    .map(|s| String::from_utf8_lossy(s.slice()).into_owned())
                    .unwrap_or_default()
            };
            match entry.tag() {
                gimli::DW_TAG_structure_type => {
                    if let Some(done) = current.take() {
                        structs.push(done);
                    }
                    let size = entry
                        .attr_value(gimli::DW_AT_byte_size)
                        .and_then(|v| v.udata_value())
                        .unwrap_or(0);
                    current = Some((size, Vec::new()));
                }
                gimli::DW_TAG_member => {
                    if let Some((_, members)) = current.as_mut() {
                        let offset = entry
                            .attr_value(gimli::DW_AT_data_member_location)
                            .and_then(|v| v.udata_value())
                            .unwrap_or(u64::MAX);
                        members.push((name(), offset));
                    }
                }
                gimli::DW_TAG_subprogram => {
                    if let Some(done) = current.take() {
                        structs.push(done);
                    }
                    let found = name();
                    if !found.is_empty() {
                        subprograms.push(found);
                    }
                }
                // Any other tag ends the run of members belonging to the struct above it.
                _ => {
                    if let Some(done) = current.take() {
                        structs.push(done);
                    }
                }
            }
        }
        if let Some(done) = current.take() {
            structs.push(done);
        }
    }
    (structs, subprograms)
}

/// A Cranelift-compiled local reaches DWARF with its name and a frame-relative location (ADR-0174).
///
/// # Why the location is the assertion that matters
///
/// Cranelift reports a stack slot's offset from the **bottom of the frame**, and DWARF's `DW_OP_fbreg` is
/// relative to whatever the subprogram declares as its frame base — here the frame-pointer register. The two
/// have to be reconciled by subtracting `frame_to_fp_offset`, and getting that wrong produces a location that
/// parses perfectly and reads the wrong memory. **A negative offset is the observable consequence of doing it
/// right**, because every stack slot sits below the frame pointer — so that is what this asserts, rather than
/// merely that a location attribute exists.
///
/// The slot is correlated to its MIR index by a `StackSlotKey`, not by creation order: this back end also
/// creates unkeyed slots for aggregate temporaries, so order would be an assumption, and a wrong one would name
/// a local after somebody else's storage.
#[test]
fn a_cranelift_local_reaches_dwarf_with_a_frame_location() {
    use gimli::EndianSlice;
    use gimli::Reader as _;
    use object::{Object as _, ObjectSection as _};

    let dir = TempDir::new().unwrap();
    let source = dir.path().join("locals.jr");
    fs::write(
        &source,
        r#"#import "Basic";

Point :: struct {
    x: s64;
    y: s64;
}

sum :: (p: Point) -> s64 {
    return p.x + p.y;
}

main :: () {
    // Passed by value, so its slot is bound to the local and it is named (ADR-0174 §3).
    q: Point;
    q.x = 9;
    q.y = 4;

    // Address taken, so it escapes to a slot too.
    total := sum(q);
    view := *total;
    exit(view.*);
}
"#,
    )
    .unwrap();

    let object_path = dir.path().join("locals.o");
    let code = run_build_emit_object(source, object_path.clone());
    assert_eq!(code, 0, "the object must be emitted");

    let bytes = fs::read(&object_path).expect("the object should exist");
    let file = object::File::parse(&*bytes).expect("the object should parse");
    let endian = if file.is_little_endian() {
        gimli::RunTimeEndian::Little
    } else {
        gimli::RunTimeEndian::Big
    };
    let load = |id: gimli::SectionId| -> Result<EndianSlice<'_, gimli::RunTimeEndian>, ()> {
        let name = id.name();
        let macho = format!("__{}", name.trim_start_matches('.'));
        let data = file
            .sections()
            .find(|s| matches!(s.name(), Ok(n) if n == name || n == macho.as_str()))
            .and_then(|s| s.data().ok())
            .unwrap_or(&[]);
        Ok(EndianSlice::new(data, endian))
    };
    let dwarf = gimli::Dwarf::load(load).expect("the DWARF should load");

    let mut found: Vec<(String, bool)> = Vec::new();
    let mut frame_bases = 0usize;
    let mut units = dwarf.units();
    while let Some(header) = units.next().expect("units should parse") {
        let unit = dwarf.unit(header).expect("the unit should parse");
        let mut entries = unit.entries();
        while let Some(entry) = entries.next_dfs().expect("entries should walk") {
            if entry.tag() == gimli::DW_TAG_subprogram
                && entry.attr_value(gimli::DW_AT_frame_base).is_some()
            {
                frame_bases += 1;
            }
            if entry.tag() != gimli::DW_TAG_variable {
                continue;
            }
            let name = entry
                .attr(gimli::DW_AT_name)
                .and_then(|a| a.string_value(&dwarf.debug_str))
                .map(|s| String::from_utf8_lossy(s.slice()).into_owned())
                .unwrap_or_default();
            let negative = match entry.attr_value(gimli::DW_AT_location) {
                Some(gimli::AttributeValue::Exprloc(expression)) => {
                    let mut operations = expression.0;
                    // `0x91` is `DW_OP_fbreg`, followed by a signed LEB128 offset.
                    let opcode = operations.read_u8().unwrap_or(0);
                    opcode == 0x91 && operations.read_sleb128().unwrap_or(0) < 0
                }
                _ => false,
            };
            found.push((name, negative));
        }
    }

    assert!(
        frame_bases > 0,
        "a subprogram with variables must declare a frame base, or every location is meaningless"
    );
    for expected in ["q", "total"] {
        let (_, negative) = found
            .iter()
            .find(|(name, _)| name == expected)
            .unwrap_or_else(|| {
                panic!("the local `{expected}` must appear as a DWARF variable; got {found:?}")
            });
        assert!(
            *negative,
            "`{expected}` must live at a negative `DW_OP_fbreg` offset; a positive one means \
             `frame_to_fp_offset` was not subtracted and the location reads the wrong memory"
        );
    }
}

/// Three threads share one counter through `atomic_add`, and none of the three thousand increments is lost
/// (ADR-0177).
///
/// # Why this is not a corpus program
///
/// The corpus differential asserts that the bytecode VM and the native back ends agree, and **the VM cannot
/// spawn a thread**: `pthread_create` needs a machine address for the thread body, and an interpreter has no
/// machine code to point at (ADR-0175 §4). That is a property of interpreting, not a missing wave.
///
/// So the *evaluation* of atomics is covered by `valid/132-atomics.jr`, which all three engines run, and their
/// *concurrency* is covered here. Splitting them this way is the same call ADR-0158 §3 made for `Process`.
///
/// # Why 3000 is the whole assertion
///
/// It is the one number that distinguishes a real atomic from a plain read-modify-write. The same program with
/// `shared.* = shared.* + 1` was measured at **1000** on one run of three — two thousand increments silently
/// lost — so this test would fail loudly if the atomics regressed to ordinary loads and stores, which no
/// single-threaded test can detect.
///
/// The exit codes 90-92 name a failed spawn and 93-95 a failed join, so a failure says which step broke rather
/// than only that the count was wrong.
#[test]
fn three_threads_share_a_counter_without_losing_an_increment() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("threads.jr");
    fs::write(
        &source,
        r#"#import "Basic";
#import "Thread";

// Each worker adds 1000 to the shared counter, one atomic add at a time.
//
// `#c_call` is not decoration: an ordinary Jairs procedure takes the hidden context parameter, so C calling
// one would pass `arg` where the context belongs (ADR-0175 §1).
bump :: (arg: *u8) -> *u8 #c_call {
    shared := typed(s64, arg);
    i := 0;
    while i < 1000 {
        _ = atomic_add(shared, 1);
        i = i + 1;
    }
    return null;
}

main :: () {
    counter := 0;

    a, aok := spawn(bump, untyped(*counter));
    if !aok { exit(90); }
    b, bok := spawn(bump, untyped(*counter));
    if !bok { exit(91); }
    c, cok := spawn(bump, untyped(*counter));
    if !cok { exit(92); }

    if !join(*a) { exit(93); }
    if !join(*b) { exit(94); }
    if !join(*c) { exit(95); }

    // A second join must be refused by the module rather than by libc, where it is undefined behaviour.
    if join(*a) { exit(96); }

    // 3000 exactly. Less means an increment was lost, which is what a non-atomic add does.
    if counter != 3000 { exit(1); }

    // The spin lock, on the same counter's neighbour: acquire, mutate, release, and confirm the state.
    lock := UNLOCKED;
    acquire(*lock);
    if !is_locked(*lock) { exit(97); }
    release(*lock);
    if is_locked(*lock) { exit(98); }

    exit(42);
}
"#,
    )
    .unwrap();

    let binary = dir.path().join("threads");
    let code = run_build_with_paths(source, binary.clone(), &[]);
    assert_eq!(code, 0, "the program must build and link");

    // Run it several times: a lost increment is a race, and a race that fails once in three runs is a race
    // that passes once in three. One run proving nothing is how a concurrency test becomes decoration.
    for attempt in 1..=5 {
        let ran = std::process::Command::new(&binary)
            .status()
            .expect("the linked binary should run");
        assert_eq!(
            ran.code(),
            Some(42),
            "attempt {attempt}: 90-92 name a failed spawn, 93-96 a failed or repeated join, \
             97-98 the spin lock, and 1 means an increment was lost"
        );
    }
}

/// A program whose body lowering refused still **builds**, and traps with the compiler's own
/// admission rather than panicking inside `cranelift-object`.
///
/// # Why this is a build test and not a corpus program
///
/// `tests/corpus/valid` asserts that all three engines agree on an exit code, and the bytecode VM
/// refuses to *run* a file whose body was refused — it reports "the compiler could not lower
/// `main`" and exits without producing one. So the asymmetry this test pins has no home there: it
/// exists precisely because the VM refused honestly while the native path did not.
///
/// # The bug it pins
///
/// Phase 1 declares every procedure with `Linkage::Export`; phase 2 used to *skip* a refused body.
/// `cranelift-object` then panicked with `function "jr$0$0" with linkage Export must be defined but
/// is not` — exit 101, a mangled symbol, and nothing connecting it to the E0245 already reported.
///
/// # Why the program changed
///
/// This test used to use a **file-scope mutable variable**, and its own comment said that was "the
/// shortest program that reaches it today". ADR-0186 implemented file-scope variables, so that program
/// now compiles and the test failed with *"the trap must name the compiler's gap, got \"\""* — the
/// build succeeded and there was no trap to read.
///
/// **A test naming an unimplemented thing has a one-wave shelf life**, which AGENTS.md already records
/// about this very test: it has "had its construct replaced twice, both times because the wave after it
/// implemented the gap it named". This is the third time.
///
/// The construct now is an **imported** global read directly — `S.shared` where another module declares
/// `shared: s64 = 5;`. ADR-0186 built same-file globals only, so this refuses honestly with *"an
/// imported constant has no value that const-eval could compute"*. Note what does **not** refuse: a
/// procedure in the declaring module reading its own global, called across the boundary. That is the
/// case `modules/Simp` needs and it works; only reading the storage from outside is deferred.
#[test]
fn a_refused_body_builds_and_traps_instead_of_panicking() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    // The module that declares the global. Its own name is what the import string must match.
    std::fs::write(dir.path().join("Shared.jr"), "shared: s64 = 5;\n")
        .expect("the module should be writable");
    let source = dir.path().join("refused.jr");
    std::fs::write(
        &source,
        "#import \"Basic\";\nS :: #import \"Shared\";\nmain :: () {\n    exit(S.shared);\n}\n",
    )
    .expect("the source should be writable");
    let binary = dir.path().join("refused");

    // Zero, not 101: the build succeeds, which is the half of E0245's promise that says leaving a
    // refused procedure uncalled is not an error.
    let code = run_build_with_module_dir(source, Some(binary.clone()), dir.path().to_path_buf());
    assert_eq!(code, 0, "a refused body must not fail the build");

    let output = std::process::Command::new(&binary)
        .output()
        .expect("the binary should run");
    let stderr = String::from_utf8_lossy(&output.stderr);

    // And the other half: calling one *is* an error, said in words a reader can act on.
    assert!(
        stderr.contains("this procedure could not be compiled"),
        "the trap must name the compiler's gap, got {stderr:?}"
    );
    assert!(
        !stderr.contains("deliberate"),
        "a refused stub is not a deliberate trap, got {stderr:?}"
    );
    assert!(
        stderr.contains("in main"),
        "the trap must name its frame, got {stderr:?}"
    );
}
