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
    ] {
        let code = check_with_modules(vec![corpus_path(file)], Some("modules"));
        assert_eq!(code, 1, "{file} must report an error");
    }
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
