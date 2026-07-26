//! The three diagnostics that need a CFG, and the corpus directory that pins them.
//!
//! `jr-mir` is the first pass with a control-flow graph, so it is the first that can
//! answer "is this read reachable from an assignment", "does every path return", and
//! "is this `break` inside a loop". The first two `jr-sema` deferred here by name;
//! the third nothing checked at all.
//!
//! # Why `tests/corpus/cfg-errors/` and not `type-errors/`
//!
//! `type-errors/` has a contract its harness enforces exactly: every file there
//! reports precisely the codes its `// EXPECT:` header names, **as reported by
//! `jr-sema`**. E0227–E0229 come from a later phase, so a file needing one would
//! report nothing in that harness and weaken the contract for everything else in
//! the directory.
//!
//! This is the same reasoning that created `type-errors/` in the first place rather
//! than filing type errors under `invalid/`: when a new kind of expectation appears,
//! give it its own directory and keep the existing contracts whole. `cfg-errors/`
//! files are well-formed *and* type-correct source, so they join the formatter and
//! tree-sitter gates exactly as `type-errors/` does.

mod harness;

use std::path::{Path, PathBuf};

use harness::Program;
use jr_base::Interner;
use jr_hir::FileHir;
use jr_mir::FileMir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Reads a corpus directory's `.jr` files in a stable order.
fn corpus_files(relative: &str) -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus")
        .join(relative);
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("jr"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
            let name = path
                .file_name()
                .expect("a file")
                .to_string_lossy()
                .into_owned();
            (name, text)
        })
        .collect()
}

/// The diagnostic codes an `// EXPECT:` header declares.
fn expected_codes(text: &str) -> Vec<String> {
    let mut codes = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.trim_start().strip_prefix("// EXPECT:") else {
            continue;
        };
        for word in rest.split_whitespace() {
            let candidate = word.trim_matches(|c: char| !c.is_ascii_alphanumeric());
            if candidate.starts_with('E') && candidate[1..].chars().all(|c| c.is_ascii_digit()) {
                codes.push(candidate.to_owned());
            }
        }
    }
    codes
}

fn codes_of(hir: &FileHir, mir: &FileMir, interner: &Interner) -> Vec<String> {
    jr_mir::file_diagnostics(hir, mir, interner)
        .sorted()
        .iter()
        .filter_map(|diag| diag.code)
        .map(str::to_owned)
        .collect()
}

// ---------------------------------------------------------------------------
// The corpus contract
// ---------------------------------------------------------------------------

#[test]
fn cfg_error_corpus_files_declare_which_code_they_expect() {
    // A file that merely fails is not a useful test; naming the code is what makes
    // an accidentally *different* diagnostic a failure rather than a pass.
    for (name, text) in corpus_files("cfg-errors") {
        assert!(
            !expected_codes(&text).is_empty(),
            "{name}: needs a `// EXPECT: E0xxx` header naming the diagnostic"
        );
    }
}

#[test]
fn cfg_error_corpus_files_are_otherwise_clean() {
    // The whole point of the directory: these must be well-formed *and*
    // type-correct, so that the only thing wrong with them is control flow. A
    // parse or type error here would mean the file tests an earlier phase by
    // accident — and worse, MIR would refuse the body and report nothing at all.
    let mut failures = Vec::new();
    for (name, text) in corpus_files("cfg-errors") {
        let mut program = Program::new();
        let lowered = program.lower(&text);
        if !lowered.earlier_diagnostics.is_empty() {
            let codes: Vec<_> = lowered
                .earlier_diagnostics
                .iter()
                .map(|d| d.code.unwrap_or("?"))
                .collect();
            failures.push(format!("{name}: {codes:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "cfg-errors files must parse, resolve and type-check cleanly:\n{}",
        failures.join("\n")
    );
}

#[test]
fn cfg_error_corpus_files_report_exactly_what_they_declare() {
    let mut failures = Vec::new();
    for (name, text) in corpus_files("cfg-errors") {
        let expected = expected_codes(&text);
        let mut program = Program::new();
        let lowered = program.lower(&text);
        let actual = codes_of(&lowered.hir, &lowered.mir, &program.interner);
        if actual != expected {
            failures.push(format!("{name}: expected {expected:?}, got {actual:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "cfg-errors files must report exactly the codes they declare:\n{}",
        failures.join("\n")
    );
}

#[test]
fn cfg_error_corpus_covers_every_code_this_crate_owns() {
    // A tripwire: a new code with no corpus file is a rule nobody is testing.
    let owned = ["E0227", "E0228", "E0229"];
    let declared: Vec<String> = corpus_files("cfg-errors")
        .iter()
        .flat_map(|(_, text)| expected_codes(text))
        .collect();
    let missing: Vec<&str> = owned
        .iter()
        .copied()
        .filter(|code| !declared.contains(&(*code).to_owned()))
        .collect();
    assert!(
        missing.is_empty(),
        "these codes have no corpus file: {missing:?}"
    );
}

// ---------------------------------------------------------------------------
// The negative half
// ---------------------------------------------------------------------------

#[test]
fn a_default_initialised_local_is_not_reported() {
    // `b: s64;` is default-initialised to its type's zero value, which
    // `tests/corpus/valid/005-decl-typed.jr` states outright. Only `= ---` opts out.
    // Collapsing the two would be a false positive on legal code, so this is the
    // test that stops it.
    let mut program = Program::new();
    let lowered = program.lower_clean("main :: () -> s64 {\n    b: s64;\n    return b;\n}\n");
    let codes = codes_of(&lowered.hir, &lowered.mir, &program.interner);
    assert!(
        codes.is_empty(),
        "a zero-initialised local must not be reported, got {codes:?}"
    );
}

#[test]
fn a_procedure_returning_on_every_branch_is_not_reported() {
    // The case a syntax walk gets wrong: the last statement is an `if`, not a
    // `return`, yet every path returns.
    let mut program = Program::new();
    let lowered = program.lower_clean(
        "f :: (n: s64) -> s64 {\n    if n > 0 {\n        return 1;\n    } else {\n        return 0;\n    }\n}\n",
    );
    let codes = codes_of(&lowered.hir, &lowered.mir, &program.interner);
    assert!(
        codes.is_empty(),
        "every path returns, so nothing is missing: {codes:?}"
    );
}

#[test]
fn a_void_procedure_needs_no_return() {
    let mut program = Program::new();
    let lowered = program.lower_clean("main :: () {\n    a := 1;\n}\n");
    assert!(codes_of(&lowered.hir, &lowered.mir, &program.interner).is_empty());
}

#[test]
fn a_break_inside_a_loop_is_not_reported() {
    let mut program = Program::new();
    let lowered = program
        .lower_clean("main :: () {\n    i := 0;\n    while i < 3 {\n        break;\n    }\n}\n");
    assert!(codes_of(&lowered.hir, &lowered.mir, &program.interner).is_empty());
}

#[test]
fn an_assignment_before_the_read_clears_the_report() {
    // Definite assignment is about *paths*, so assigning first must silence it.
    let mut program = Program::new();
    let lowered = program
        .lower_clean("main :: () -> s64 {\n    c: s64 = ---;\n    c = 1;\n    return c;\n}\n");
    let codes = codes_of(&lowered.hir, &lowered.mir, &program.interner);
    assert!(
        codes.is_empty(),
        "`c` is assigned before the read: {codes:?}"
    );
}

#[test]
fn a_read_assigned_on_only_one_branch_is_still_reported() {
    // The case that makes this a dataflow question rather than a syntactic one:
    // one path assigns and one does not, so the read is not definitely assigned.
    let mut program = Program::new();
    let lowered = program.lower_clean(
        "f :: (cond: bool) -> s64 {\n    c: s64 = ---;\n    if cond {\n        c = 1;\n    }\n    return c;\n}\n",
    );
    let codes = codes_of(&lowered.hir, &lowered.mir, &program.interner);
    assert_eq!(codes, vec!["E0227"], "one branch leaves `c` unassigned");
}

#[test]
fn a_refused_body_reports_nothing_of_its_own() {
    // ADR-0017 §4: a refused body has no CFG, and its cause was already reported by
    // an earlier phase, so piling a speculative missing-`return` on top would be
    // noise. `#run` is the cheapest way to get a refusal out of a clean file.
    let mut program = Program::new();
    let lowered = program.lower_clean(
        "add :: (a: s64, b: s64) -> s64 { return a + b; }\nmain :: () -> s64 { return #run add(1, 2); }\n",
    );
    let codes = codes_of(&lowered.hir, &lowered.mir, &program.interner);
    assert!(
        codes.is_empty(),
        "a refused body must stay silent, got {codes:?}"
    );
}
