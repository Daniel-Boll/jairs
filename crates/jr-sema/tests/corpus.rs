//! The corpus is the acceptance test, and for sema it is mostly a *negative* one.
//!
//! No corpus file expected a type error before this wave (ADR-0016's Context
//! says so), so the obligation is that every well-formed file checks **silently**
//! — and `tests/corpus/type-errors/` is the positive half added alongside it.
//!
//! ## Why type errors are not in `tests/corpus/invalid/`
//!
//! `invalid/` has a contract with three enforcers: `jr-syntax` asserts every file
//! there produces a *parse* diagnostic, `jr-fmt` asserts `jr fmt` refuses it, and
//! the tree-sitter gate excludes it because those files do not parse. A type
//! error parses perfectly well, so putting one there would have meant weakening
//! all three — trading the parser's recovery corpus for somewhere to put a new
//! file. `type-errors/` keeps both contracts intact: its files are well-formed
//! source that must be *rejected by sema*, so they join the formatter and
//! tree-sitter gates rather than being excluded from them.

mod harness;

use std::path::{Path, PathBuf};

use harness::Program;

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

/// Extracts the diagnostic codes an `// EXPECT:` header declares.
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

// ---------------------------------------------------------------------------
// The positive half
// ---------------------------------------------------------------------------

#[test]
fn type_error_corpus_files_declare_which_code_they_expect() {
    // A file that merely fails is not a useful test. Naming the code is what
    // makes it one, and what makes an accidental *different* error a failure
    // rather than a pass.
    for (name, text) in corpus_files("type-errors") {
        assert!(
            !expected_codes(&text).is_empty(),
            "{name}: needs a `// EXPECT: E0xxx` header naming the diagnostic"
        );
    }
}

#[test]
fn type_error_corpus_files_parse_cleanly() {
    // The whole point of the directory: these are well-formed programs that sema
    // must reject. A parse error here would mean the file is testing the parser
    // by accident.
    let mut failures = Vec::new();
    for (name, text) in corpus_files("type-errors") {
        let mut program = Program::new();
        let analysis = program.analyse(&text);
        if !analysis.earlier_diagnostics.is_empty() {
            let codes: Vec<_> = analysis
                .earlier_diagnostics
                .iter()
                .map(|d| d.code.unwrap_or("?"))
                .collect();
            failures.push(format!("{name}: {codes:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "type-errors files must parse, lower and resolve cleanly:\n{}",
        failures.join("\n")
    );
}

#[test]
fn type_error_corpus_files_report_exactly_what_they_declare() {
    let mut failures = Vec::new();
    for (name, text) in corpus_files("type-errors") {
        let expected = expected_codes(&text);
        let mut program = Program::new();
        let analysis = program.analyse(&text);
        let actual: Vec<String> = analysis.codes().iter().map(|c| (*c).to_owned()).collect();
        if actual != expected {
            failures.push(format!("{name}: expected {expected:?}, got {actual:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "type-errors files must report exactly the codes they declare:\n{}",
        failures.join("\n")
    );
}

#[test]
fn type_error_corpus_covers_every_code_this_crate_owns() {
    // A tripwire: a new code with no corpus file is a rule nobody is testing.
    let owned = [
        "E0204", "E0212", "E0213", "E0214", "E0215", "E0216", "E0217", "E0218", "E0219", "E0220",
        "E0221", "E0222", "E0223", "E0224", "E0225", "E0226",
    ];
    let declared: Vec<String> = corpus_files("type-errors")
        .iter()
        .flat_map(|(_, text)| expected_codes(text))
        .collect();
    let missing: Vec<&str> = owned
        .iter()
        .copied()
        .filter(|code| !declared.iter().any(|d| d == code))
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
fn valid_corpus_files_produce_no_sema_diagnostics() {
    // Checked without module resolution, which is deliberate: the files that
    // `#import "Basic"` will have unresolved names, and sema must stay silent
    // about them rather than inventing type errors on poison. The version *with*
    // modules is covered end-to-end by the `jr-cli` corpus tests.
    let mut failures = Vec::new();
    for (name, text) in corpus_files("valid") {
        let mut program = Program::new();
        let analysis = program.analyse(&text);
        if !analysis.sema_diagnostics.is_empty() {
            let messages: Vec<String> = analysis
                .sema_diagnostics
                .iter()
                .map(|d| format!("{:?} {}", d.code, d.message))
                .collect();
            failures.push(format!("{name}: {}", messages.join("; ")));
        }
    }
    assert!(
        failures.is_empty(),
        "valid corpus files must type-check silently:\n{}",
        failures.join("\n")
    );
}

#[test]
fn fixture_modules_produce_no_sema_diagnostics() {
    let mut failures = Vec::new();
    let mut sources: Vec<(String, String)> = corpus_files("modules");
    for directory in ["Shapes", "Cycle_A", "Cycle_B"] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/corpus/modules")
            .join(directory)
            .join("module.jr");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        sources.push((format!("{directory}/module.jr"), text));
    }
    let basic = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../modules/Basic/module.jr");
    sources.push((
        "Basic/module.jr".to_owned(),
        std::fs::read_to_string(&basic).expect("the bundled Basic module must exist"),
    ));

    for (name, text) in sources {
        let mut program = Program::new();
        let analysis = program.analyse(&text);
        if !analysis.sema_diagnostics.is_empty() {
            let messages: Vec<String> = analysis
                .sema_diagnostics
                .iter()
                .map(|d| format!("{:?} {}", d.code, d.message))
                .collect();
            failures.push(format!("{name}: {}", messages.join("; ")));
        }
    }
    assert!(
        failures.is_empty(),
        "the fixture and bundled modules must type-check silently:\n{}",
        failures.join("\n")
    );
}
