//! The corpus gate for `jr-syntax`.
//!
//! Files in `tests/corpus/` serve triple duty as spec examples, compiler tests,
//! and tree-sitter tests (see `tests/corpus/README.md`). This is the compiler
//! half of the `corpus-drift` CI job.
//!
//! At the current milestone only the lexer exists, so the assertions are:
//!
//! * `valid/` must lex with **zero** diagnostics.
//! * every file, valid or not, must **tile**: token ranges cover the input
//!   exactly, and concatenating them reproduces the source byte for byte. This
//!   is the invariant the lossless CST and `jr fmt` are built on.
//!
//! The parser assertions are added to this same file as the parser lands.

use std::path::{Path, PathBuf};

use jr_base::{FileId, SourceMap, TextSize};
use jr_syntax::lex;

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
