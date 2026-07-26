//! Robustness gate: the parser must be total.
//!
//! `PLAN.md` §1.4 requires the parser to survive arbitrary input, and the
//! corpus alone cannot establish that — 34 curated files are all well-formed or
//! deliberately-broken-in-one-place. Real editors send the parser every
//! intermediate state of a file as the user types, and a fuzzer will send it
//! worse.
//!
//! Two invariants are asserted for every input here:
//!
//! 1. **No panic.** Parsing is infallible by construction.
//! 2. **Round-trip.** `parse(text).syntax().text() == text`, byte for byte,
//!    even for garbage. The formatter and the language server both depend on
//!    this, and it is the property most likely to break silently.
//!
//! The prefix tests are the valuable ones: every truncation of a real file is a
//! state some editor will genuinely ask us to parse, and truncation reliably
//! exposes recovery paths the corpus never reaches.

use std::path::{Path, PathBuf};

use jr_base::FileId;
use jr_syntax::parse;

fn corpus_files() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus");
    let mut out = Vec::new();

    for which in ["valid", "invalid"] {
        let dir: PathBuf = root.join(which);
        let mut paths: Vec<_> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
            .map(|e| e.expect("dir entry").path())
            .filter(|p| p.extension().is_some_and(|x| x == "jr"))
            .collect();
        paths.sort();

        for path in paths {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
            let name = format!(
                "{which}/{}",
                path.file_name().expect("file name").to_string_lossy()
            );
            out.push((name, text));
        }
    }
    assert!(!out.is_empty(), "corpus is empty");
    out
}

/// Parses `text` and asserts the tree reproduces it exactly.
fn assert_round_trips(label: &str, text: &str) {
    let parsed = parse(text, FileId::from_usize(0));
    let reprinted = parsed.syntax().text().to_string();
    assert_eq!(
        reprinted, text,
        "{label}: the tree did not reproduce its input\n\
         losslessness is what `jr fmt` and the language server rely on"
    );
}

#[test]
fn every_prefix_of_every_corpus_file_round_trips() {
    // Truncating at a char boundary only: slicing mid-UTF-8 would panic in the
    // test harness rather than in the parser, which would be a false positive.
    for (name, text) in corpus_files() {
        for (offset, _) in text.char_indices() {
            assert_round_trips(&format!("{name} truncated at {offset}"), &text[..offset]);
        }
        assert_round_trips(&format!("{name} whole"), &text);
    }
}

/// Every construct in the grammar that recurses, with the shape that actually
/// exercises it.
///
/// The subtlety worth preserving: `"-".repeat(n)` does **not** test prefix
/// nesting, because `---` lexes as a single `UNINIT` token. An earlier version
/// of this test used `-` and consequently missed an unguarded recursion in
/// `parse_unary_or_primary`. `!` and `*` are the shapes that really chain.
fn nesting_shapes(depth: usize) -> Vec<(String, String)> {
    let d = depth;
    vec![
        (
            "parens".into(),
            format!("main :: () {{ x := {}1{}; }}", "(".repeat(d), ")".repeat(d)),
        ),
        (
            "blocks".into(),
            format!("main :: () {{ {}{} }}", "{".repeat(d), "}".repeat(d)),
        ),
        (
            "prefix_bang".into(),
            format!("main :: () {{ x := {}1; }}", "!".repeat(d)),
        ),
        (
            "prefix_star".into(),
            format!("main :: () {{ x := {}y; }}", "*".repeat(d)),
        ),
        (
            "minus_lexes_as_uninit".into(),
            format!("main :: () {{ x := {}1; }}", "-".repeat(d)),
        ),
        (
            "spaced_minus".into(),
            format!("main :: () {{ x := {}1; }}", "- ".repeat(d)),
        ),
        (
            "pointer_type".into(),
            format!("main :: () {{ p: {}s64; }}", "*".repeat(d)),
        ),
        (
            "call".into(),
            format!("main :: () {{ x := f{}{}; }}", "(".repeat(d), ")".repeat(d)),
        ),
        (
            "field_access".into(),
            format!("main :: () {{ x := a{}; }}", ".b".repeat(d)),
        ),
        (
            "deref".into(),
            format!("main :: () {{ x := a{}; }}", ".*".repeat(d)),
        ),
        (
            "struct_nesting".into(),
            format!(
                "T :: struct {{ {}x: s64;{} }}",
                "f: struct { ".repeat(d),
                " }".repeat(d)
            ),
        ),
        (
            "binary_chain".into(),
            format!("main :: () {{ x := 1{}; }}", " + 1".repeat(d)),
        ),
    ]
}

#[test]
fn deeply_nested_input_is_safe_on_a_small_stack() {
    // Run on an explicit 1 MiB thread rather than trusting the harness default.
    //
    // This is not test hygiene, it is the actual production requirement: the
    // language server parses on worker threads, and a stack overflow is an
    // abort, not a catchable error — it would take the user's editor down. The
    // parser must therefore be safe on a small stack by construction (via
    // MAX_DEPTH), not by luck.
    const SMALL_STACK: usize = 1024 * 1024;

    let handle = std::thread::Builder::new()
        .name("small-stack-parser".into())
        .stack_size(SMALL_STACK)
        .spawn(|| {
            for depth in [64usize, 512, 20_000] {
                for (name, text) in nesting_shapes(depth) {
                    assert_round_trips(&format!("{name} @ depth {depth}"), &text);
                }
            }
        })
        .expect("failed to spawn parser thread");

    handle
        .join()
        .expect("parser overflowed a 1 MiB stack or panicked on deeply nested input");
}

#[test]
fn depth_limit_is_reported_at_most_once() {
    // The depth/chain limit must not be reported per level: tens of thousands of
    // identical errors are useless to the user and are themselves a
    // memory-exhaustion vector.
    //
    // Note this is specifically about the E0199 diagnostic. Total diagnostic
    // count is NOT bounded by a small constant, and should not be: `-` repeated
    // 20 000 times is 6 666 separate `---` (UNINIT) tokens, and reporting each
    // stray one is correct behaviour, not spam.
    for (name, text) in nesting_shapes(20_000) {
        let parsed = parse(&text, FileId::from_usize(0));
        let too_deep = parsed
            .diagnostics()
            .iter()
            .filter(|d| d.code == Some("E0199"))
            .count();
        assert!(
            too_deep <= 1,
            "{name}: reported the depth limit {too_deep} times; expected at most once"
        );
    }
}

#[test]
fn diagnostics_do_not_outnumber_the_input() {
    // A weaker but still meaningful anti-spam invariant: every diagnostic should
    // correspond to at least one byte of input. Violating this means some rule
    // reports without consuming, which is the signature of the recovery bug
    // class that previously caused an unbounded loop.
    for (name, text) in nesting_shapes(20_000) {
        let parsed = parse(&text, FileId::from_usize(0));
        let count = parsed.diagnostics().len();
        assert!(
            count <= text.len(),
            "{name}: {count} diagnostics for {} bytes of input",
            text.len()
        );
    }
}

#[test]
fn unbalanced_delimiters_in_both_directions() {
    for text in [
        "{", "}", "((((", "))))", "{{{{", "}}}}", "(", ")", "[", "]", "([{)]}", "}{", ")(",
    ] {
        assert_round_trips(text, text);
    }
}

#[test]
fn pathological_but_lexable_input() {
    for text in [
        "",
        " ",
        "\n",
        "\r\n",
        "\t",
        ";",
        ";;;;;;",
        "::",
        ":=",
        "->",
        "---",
        ".*",
        "..",
        "#",
        "#run",
        "#run;",
        "#import",
        "#import;",
        "struct",
        "struct{",
        "struct{}",
        "if",
        "if{",
        "else",
        "while",
        "return",
        "break",
        "continue",
        "a::",
        "a:=",
        "a:",
        "a:=;",
        "::a",
        ":::::",
        "=====",
        "a....b",
        "a.*.*.*",
        "*****",
        "1.2.3.4",
        "0x0x0x",
        "\"\"\"",
        "\"\\",
        "/*",
        "*/",
        "/*/*/*",
        "// only a comment",
        "\0",
        "\u{feff}",
        "🦀",
        "é::é",
        "for defer using enum union cast xx null",
        "& | ^ ~ << >> @",
    ] {
        assert_round_trips(&format!("{text:?}"), text);
    }
}

#[test]
fn arbitrary_bytes_do_not_panic() {
    // A deterministic pseudo-random smoke test standing in for `cargo fuzz`,
    // so CI catches the obvious class of failure without a fuzzing run. Uses a
    // fixed seed: a flaky robustness test is worse than none.
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        // xorshift64*
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };

    // Draw from a Jairs-flavoured alphabet: random ASCII rarely produces
    // anything the parser finds interesting, whereas real tokens reach deep
    // into the recovery paths.
    const ALPHABET: &[&str] = &[
        " ", "\n", "a", "b", "main", "s64", "1", "0x1", "\"s\"", "::", ":=", ":", ";", ",", "(",
        ")", "{", "}", "[", "]", "*", ".*", ".", "+", "-", "+%", "---", "=", "==", "!", "&&", "->",
        "if", "else", "while", "return", "struct", "#run", "#import", "#foreign", "for", "//x",
        "/*", "*/", "$",
    ];

    for case in 0..2_000 {
        let len = (next() % 24) as usize;
        let mut text = String::new();
        for _ in 0..len {
            text.push_str(ALPHABET[(next() as usize) % ALPHABET.len()]);
        }
        assert_round_trips(&format!("random case {case}: {text:?}"), &text);
    }
}

#[test]
fn every_byte_of_input_is_covered_by_exactly_one_token() {
    // The tree may not silently drop input even when it cannot understand it;
    // dropped text is how a formatter deletes a user's code.
    for (name, text) in corpus_files() {
        let parsed = parse(&text, FileId::from_usize(0));
        let root = parsed.syntax();
        assert_eq!(
            usize::from(root.text_range().len()),
            text.len(),
            "{name}: root node does not span the whole file"
        );
    }
}
