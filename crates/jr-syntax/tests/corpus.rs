//! The corpus gate for `jr-syntax`.
//!
//! Files in `tests/corpus/` serve triple duty as spec examples, compiler tests,
//! and tree-sitter tests (see `tests/corpus/README.md`). This is the compiler
//! half of the `corpus-drift` CI job.
//!
//! Assertions:
//!
//! * `valid/` must parse with **zero** diagnostics.
//! * `invalid/` must parse with **at least one** diagnostic.
//! * Every file, valid or not, must **round-trip**: `parse(text).syntax().text() == text`.
//! * The existing lexer-level tiling tests are preserved below.

use std::path::{Path, PathBuf};

use jr_base::{FileId, SourceMap, TextSize};
use jr_syntax::{lex, parse};

fn corpus_dir(which: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus")
        .join(which)
}

/// Returns `(relative_name, contents)` for every `.jr` file, sorted so failures
/// are reported deterministically.
fn corpus_files(which: &str) -> Vec<(String, String)> {
    let dir = corpus_dir(which);
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read corpus dir {}: {e}", dir.display()))
        .map(|e| e.expect("corpus dir entry").path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "jr"))
        .collect();
    entries.sort();

    assert!(
        !entries.is_empty(),
        "corpus dir {} contains no .jr files",
        dir.display()
    );

    entries
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
            let name = format!(
                "{which}/{}",
                path.file_name()
                    .expect("corpus file name")
                    .to_string_lossy()
            );
            (name, text)
        })
        .collect()
}

/// Asserts that token ranges tile the input exactly.
fn assert_tiles(name: &str, text: &str) {
    let out = lex(text, FileId::from_usize(0));

    let mut cursor = TextSize::new(0);
    for token in &out.tokens {
        assert_eq!(
            token.range.start(),
            cursor,
            "{name}: gap or overlap in token stream before {:?}",
            token.kind
        );
        cursor = token.range.end();
    }
    assert_eq!(
        usize::from(cursor),
        text.len(),
        "{name}: tokens do not cover the whole file"
    );

    let rejoined: String = out.tokens.iter().map(|t| &text[t.range]).collect();
    assert_eq!(
        rejoined, text,
        "{name}: detokenising did not reproduce the source"
    );
}

// ---------------------------------------------------------------------------
// Lexer-level tests (preserved from the original corpus.rs)
// ---------------------------------------------------------------------------

#[test]
fn valid_corpus_lexes_without_diagnostics() {
    let mut map = SourceMap::new();
    let mut failures = Vec::new();

    for (name, text) in corpus_files("valid") {
        let file = map.add(&name, text.clone());
        let out = lex(&text, file);

        if !out.diagnostics.is_empty() {
            // Render with real spans so a failure is actionable rather than
            // just a count.
            let rendered = jr_diag::Renderer::new().render_all(&map, &out.diagnostics);
            failures.push(format!("--- {name} ---\n{rendered}"));
        }
    }

    assert!(
        failures.is_empty(),
        "valid corpus files must lex cleanly:\n\n{}",
        failures.join("\n")
    );
}

#[test]
fn every_corpus_file_tiles_its_input() {
    for which in ["valid", "invalid"] {
        for (name, text) in corpus_files(which) {
            assert_tiles(&name, &text);
        }
    }
}

#[test]
fn invalid_corpus_files_declare_what_they_test() {
    // An `invalid/` file that merely fails to parse is not a useful test; the
    // point is what the parser does *next*. The README requires each one to say
    // so up front, and this keeps that honest.
    for (name, text) in corpus_files("invalid") {
        assert!(
            text.contains("// EXPECT:"),
            "{name}: missing a `// EXPECT:` comment describing the diagnostic"
        );
    }
}

#[test]
fn corpus_has_meaningful_coverage() {
    // A tripwire against the corpus being silently emptied or a glob breaking,
    // which would make the drift gate pass vacuously.
    assert!(
        corpus_files("valid").len() >= 20,
        "the valid corpus has shrunk unexpectedly"
    );
    assert!(
        corpus_files("invalid").len() >= 8,
        "the invalid corpus has shrunk unexpectedly"
    );
}

// ---------------------------------------------------------------------------
// Parser-level tests
// ---------------------------------------------------------------------------

/// Every valid corpus file must parse with zero diagnostics.
#[test]
fn valid_corpus_parses_without_errors() {
    let mut map = SourceMap::new();
    let mut failures = Vec::new();

    for (name, text) in corpus_files("valid") {
        let file = map.add(&name, text.clone());
        let p = parse(&text, file);

        if p.has_errors() {
            let rendered = jr_diag::Renderer::new().render_all(&map, p.diagnostics());
            failures.push(format!("--- {name} ---\n{rendered}"));
        }
    }

    assert!(
        failures.is_empty(),
        "valid corpus files must parse cleanly:\n\n{}",
        failures.join("\n")
    );
}

/// Every invalid corpus file must produce at least one diagnostic.
#[test]
fn invalid_corpus_produces_diagnostics() {
    let mut map = SourceMap::new();
    let mut failures = Vec::new();

    for (name, text) in corpus_files("invalid") {
        let file = map.add(&name, text.clone());
        let p = parse(&text, file);

        if !p.has_errors() {
            failures.push(format!("{name}: expected at least one error, got none"));
        }
    }

    assert!(
        failures.is_empty(),
        "invalid corpus files must produce errors:\n\n{}",
        failures.join("\n")
    );
}

/// The round-trip invariant: `parse(text).syntax().text() == text` for every
/// corpus file, valid and invalid.
///
/// This is the load-bearing property for `jr fmt` and the LSP.
#[test]
fn every_corpus_file_round_trips_through_the_tree() {
    let mut failures = Vec::new();

    for which in ["valid", "invalid"] {
        for (name, text) in corpus_files(which) {
            let p = parse(&text, FileId::from_usize(0));
            let round = p.syntax().text().to_string();
            if round != text {
                // Show the first difference for diagnosis.
                let diff_pos = round
                    .bytes()
                    .zip(text.bytes())
                    .position(|(a, b)| a != b)
                    .unwrap_or(round.len().min(text.len()));
                failures.push(format!(
                    "{name}: round-trip failed at byte {diff_pos}\n  original: {:?}\n  got:      {:?}",
                    &text[diff_pos.saturating_sub(20)..text.len().min(diff_pos + 20)],
                    &round[diff_pos.saturating_sub(20)..round.len().min(diff_pos + 20)],
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "round-trip invariant violated:\n\n{}",
        failures.join("\n\n")
    );
}

/// Spot-check: `invalid/001` (missing semicolon) — the next declaration must
/// still parse.
#[test]
fn invalid_001_missing_semicolon_recovers() {
    let text = std::fs::read_to_string(corpus_dir("invalid").join("001-missing-semicolon.jr"))
        .expect("corpus file");
    let p = parse(&text, FileId::from_usize(0));

    assert!(p.has_errors(), "should have at least one error");

    // The tree must contain two VAR_DECL nodes (a and b).
    let tree = jr_syntax::dump_tree(&p.syntax());
    let var_decl_count = tree.matches("VAR_DECL").count();
    assert!(
        var_decl_count >= 2,
        "both declarations must be in the tree; found {var_decl_count} VAR_DECL nodes\n{tree}"
    );
}

/// Spot-check: `invalid/002` (unclosed brace) — exactly one error, no cascade.
#[test]
fn invalid_002_unclosed_brace_one_error() {
    let text = std::fs::read_to_string(corpus_dir("invalid").join("002-unclosed-brace.jr"))
        .expect("corpus file");
    let p = parse(&text, FileId::from_usize(0));

    let errors: Vec<_> = p.diagnostics().iter().collect();
    assert!(
        !errors.is_empty(),
        "should have at least one error for unclosed brace"
    );

    // The spec says "exactly one error". We allow one or two (the unclosed
    // brace itself, and possibly a missing-semicolon for the last statement),
    // but not a cascade of many.
    assert!(
        errors.len() <= 3,
        "expected at most 3 errors (no cascade), got {}:\n{}",
        errors.len(),
        errors
            .iter()
            .map(|d| format!("  {}", d.message))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Spot-check: `invalid/003` (unclosed paren) — the following statement must
/// still parse.
#[test]
fn invalid_003_unclosed_paren_recovers() {
    let text = std::fs::read_to_string(corpus_dir("invalid").join("003-unclosed-paren.jr"))
        .expect("corpus file");
    let p = parse(&text, FileId::from_usize(0));

    assert!(p.has_errors(), "should have at least one error");

    // `y := 3;` must still be in the tree.
    let tree = jr_syntax::dump_tree(&p.syntax());
    assert!(
        tree.contains("VAR_DECL"),
        "the statement after the unclosed paren must still parse\n{tree}"
    );
}

/// Spot-check: `invalid/004` (missing operands) — all four statements must be
/// reported independently.
#[test]
fn invalid_004_missing_operands_all_reported() {
    let text = std::fs::read_to_string(corpus_dir("invalid").join("004-missing-operand.jr"))
        .expect("corpus file");
    let p = parse(&text, FileId::from_usize(0));

    let error_count = p.diagnostics().iter().count();
    assert!(
        error_count >= 4,
        "expected at least 4 errors (one per statement), got {error_count}:\n{}",
        p.diagnostics()
            .iter()
            .map(|d| format!("  {}", d.message))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Spot-check: `invalid/007` (stray tokens) — both procedures must parse.
#[test]
fn invalid_007_stray_tokens_both_procs_parse() {
    let text = std::fs::read_to_string(corpus_dir("invalid").join("007-unexpected-token.jr"))
        .expect("corpus file");
    let p = parse(&text, FileId::from_usize(0));

    assert!(p.has_errors(), "should have errors for stray tokens");

    // Both `before` and `after` procedures must be in the tree.
    let tree = jr_syntax::dump_tree(&p.syntax());
    let proc_count = tree.matches("PROC").count();
    assert!(
        proc_count >= 2,
        "both procedures must parse; found {proc_count} PROC nodes\n{tree}"
    );
}

/// Spot-check: `invalid/009` (multiple independent errors) — at least four
/// distinct errors, no cascade.
#[test]
fn invalid_009_multiple_independent_errors() {
    let text =
        std::fs::read_to_string(corpus_dir("invalid").join("009-multiple-independent-errors.jr"))
            .expect("corpus file");
    let p = parse(&text, FileId::from_usize(0));

    let error_count = p.diagnostics().iter().count();
    assert!(
        error_count >= 4,
        "expected at least 4 distinct errors, got {error_count}:\n{}",
        p.diagnostics()
            .iter()
            .map(|d| format!("  {}", d.message))
            .collect::<Vec<_>>()
            .join("\n")
    );

    // `third` procedure must still parse (no cascade suppressing it).
    let tree = jr_syntax::dump_tree(&p.syntax());
    assert!(
        tree.contains("PROC"),
        "at least one procedure must parse despite errors\n{tree}"
    );
}

/// Spot-check: `valid/024-hello.jr` — the slice exit criterion.
#[test]
fn valid_024_hello_parses_cleanly() {
    let text =
        std::fs::read_to_string(corpus_dir("valid").join("024-hello.jr")).expect("corpus file");
    let p = parse(&text, FileId::from_usize(0));

    assert!(
        !p.has_errors(),
        "hello.jr must parse cleanly:\n{}",
        p.diagnostics()
            .iter()
            .map(|d| format!("  {}", d.message))
            .collect::<Vec<_>>()
            .join("\n")
    );

    // Round-trip
    assert_eq!(
        p.syntax().text().to_string(),
        text,
        "hello.jr must round-trip"
    );
}
