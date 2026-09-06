//! Enforces that a `#foreign` declaration names a library that actually contains its symbol.
//!
//! # Why this test exists
//!
//! `modules/Math` and `tests/corpus/valid/093-ffi-floats.jr` both bound `sqrt` to **libc** for their
//! whole lives. That links on macOS, where `libm.tbd` is a symlink to `libSystem.tbd`, and fails on
//! glibc where libm is a separate library — so the x86-64 Linux CI leg reported `undefined reference
//! to 'sin'` for waves while nobody read it (ADR-0205).
//!
//! Both were fixed by hand. **A comment saying "bind math to libm" would be the seventh
//! hand-maintained claim in this repository that nothing enforced**, after the E0290 code collision,
//! `file_consts`' feature list, `checked_expanded`'s "`#insert` adds no items", `callee_sig`'s "this
//! crate does not hold them", `TrapKind::ALL`'s length assertion and ADR-0184's `ItemId` re-keying.
//! Every one of those was invisible on the machine that runs. So the rule is a test.
//!
//! # Why it scans source text rather than the compiler's own view
//!
//! Asking the compiler would be better in principle — it resolves `#foreign` declarations already —
//! but the answer it has is "which library was named", not "does that library contain this symbol",
//! and nothing in the tree knows the latter. The knowledge is glibc's split between libc and libm,
//! which is a fact about a platform rather than about this compiler.
//!
//! So this reads the `.jr` files, and the cost of that choice is stated: it must skip comments, or it
//! flags the prose in `valid/091-math.jr` that *quotes* the old broken form while explaining it.

use std::path::{Path, PathBuf};

/// The symbols glibc places in **libm** rather than libc, without the `f`/`l` suffixes.
///
/// Not exhaustive over libm, deliberately: it lists what a program here could plausibly declare. A
/// symbol missing from this list is not flagged, which is the failure mode to accept — the
/// alternative is asserting against a copy of `math.h` that would go stale.
const MATH_SYMBOLS: &[&str] = &[
    "acos",
    "acosh",
    "asin",
    "asinh",
    "atan",
    "atan2",
    "atanh",
    "cbrt",
    "ceil",
    "copysign",
    "cos",
    "cosh",
    "erf",
    "erfc",
    "exp",
    "exp2",
    "expm1",
    "fabs",
    "fdim",
    "floor",
    "fma",
    "fmax",
    "fmin",
    "fmod",
    "frexp",
    "hypot",
    "ilogb",
    "ldexp",
    "lgamma",
    "log",
    "log10",
    "log1p",
    "log2",
    "logb",
    "modf",
    "nearbyint",
    "pow",
    "remainder",
    "rint",
    "round",
    "scalbn",
    "sin",
    "sinh",
    "sqrt",
    "tan",
    "tanh",
    "tgamma",
    "trunc",
];

/// Whether `symbol` is a libm symbol, allowing the `f` and `l` precision suffixes.
fn is_math_symbol(symbol: &str) -> bool {
    if MATH_SYMBOLS.contains(&symbol) {
        return true;
    }
    // `sqrtf`, `powl` — the same function at another precision, in the same library.
    matches!(symbol.chars().last(), Some('f' | 'l'))
        && MATH_SYMBOLS.contains(&&symbol[..symbol.len() - 1])
}

/// Every `.jr` file the project owns.
///
/// Skips `target/` and `editors/zed/grammars/`, the latter because Zed clones this repository into
/// the extension directory when it builds a dev extension — a second copy of every file, which
/// ADR-0203 already had to teach module discovery to ignore.
fn jairs_files(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if name == "target" || name == "grammars" || name.starts_with('.') {
                    continue;
                }
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "jr") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out.sort();
    out
}

/// The workspace root, from this crate's manifest directory.
fn workspace_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(Path::parent)
        .unwrap_or(manifest)
        .to_path_buf()
}

/// `line` with any `//` comment removed, so prose quoting a declaration is not read as one.
///
/// Naive about a `//` inside a string literal, which is acceptable here: a `#foreign` declaration's
/// only string is the symbol name, and a symbol containing `//` is not a symbol.
fn strip_comment(line: &str) -> &str {
    match line.find("//") {
        Some(at) => &line[..at],
        None => line,
    }
}

/// A math symbol is declared against a library named `m`, everywhere in the tree.
#[test]
fn every_math_symbol_is_declared_against_libm() {
    let root = workspace_root();
    let mut wrong = Vec::new();

    for file in jairs_files(&root) {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        // What each `#system_library` alias resolves to in this file, so a declaration naming
        // `libm` can be checked against the *library* rather than against the alias's spelling —
        // a file is free to call it `m` or `math` or anything else.
        let mut alias_library: Vec<(String, String)> = Vec::new();
        for line in text.lines().map(strip_comment) {
            if let Some((alias, rest)) = line.split_once("::")
                && let Some(start) = rest.find("#system_library")
            {
                let after = &rest[start..];
                if let Some(open) = after.find('"')
                    && let Some(close) = after[open + 1..].find('"')
                {
                    alias_library.push((
                        alias.trim().to_owned(),
                        after[open + 1..open + 1 + close].to_owned(),
                    ));
                }
            }
        }

        for (number, line) in text.lines().enumerate() {
            let line = strip_comment(line);
            let Some(at) = line.find("#foreign") else {
                continue;
            };
            let after = &line[at + "#foreign".len()..];
            let mut parts = after.split_whitespace();
            let Some(alias) = parts.next() else { continue };
            let Some(symbol) = parts.next() else { continue };
            let symbol = symbol.trim_matches(|c: char| c == '"' || c == ';');
            if !is_math_symbol(symbol) {
                continue;
            }
            let library = alias_library
                .iter()
                .find(|(name, _)| name == alias)
                .map(|(_, library)| library.as_str());
            if library != Some("m") {
                wrong.push(format!(
                    "{}:{} — `{symbol}` is declared against `{alias}` (library {:?}), but glibc \
                     puts it in libm",
                    file.strip_prefix(&root).unwrap_or(&file).display(),
                    number + 1,
                    library.unwrap_or("<undeclared>")
                ));
            }
        }
    }

    assert!(
        wrong.is_empty(),
        "a math symbol is bound to the wrong library, which links on macOS and fails on Linux \
         (ADR-0205):\n  {}",
        wrong.join("\n  ")
    );
}

/// The scanner finds a real declaration and ignores one inside a comment.
///
/// Without this the test above could pass by finding nothing at all — and it *would* have found
/// nothing if `strip_comment` had eaten whole lines, which is exactly the bug a scanner like this
/// gets. `valid/091-math.jr` quotes the old broken form in its prose, so the comment case is real
/// rather than hypothetical.
#[test]
fn the_scanner_reads_declarations_and_not_prose() {
    assert!(is_math_symbol("sqrt"), "a plain math symbol");
    assert!(is_math_symbol("sqrtf"), "the float suffix");
    assert!(is_math_symbol("powl"), "the long-double suffix");
    assert!(!is_math_symbol("write"), "not a math symbol");
    assert!(!is_math_symbol("f"), "a bare suffix is not a symbol");

    assert_eq!(
        strip_comment("a :: b; // #foreign libc \"sqrt\""),
        "a :: b; "
    );
    assert_eq!(strip_comment("// all prose"), "");
    assert_eq!(strip_comment("no comment here"), "no comment here");

    // And the corpus file whose prose quotes the broken form is genuinely present, so the comment
    // case this guards against cannot quietly disappear.
    let quoting = workspace_root().join("tests/corpus/valid/091-math.jr");
    let text = std::fs::read_to_string(&quoting).expect("091-math.jr must exist");
    assert!(
        text.contains("#foreign libc \"sqrt\""),
        "091-math.jr should still quote the old form in prose; if it no longer does, this test's \
         premise has expired and the comment-stripping needs another witness"
    );
}
