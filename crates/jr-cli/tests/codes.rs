//! Every diagnostic code in the workspace, checked for collisions **across** crates.
//!
//! # Why this test exists here rather than in each crate
//!
//! `AGENTS.md` gives each crate a range and asks it to keep its codes in a `code.rs` with one
//! constant each. `jr-syntax` grew such a file with two tests — no code used twice, every code
//! inside a range it owns — and that file's own header states the limitation plainly: "The tests
//! below check what this crate *owns*; they cannot check a claim about somebody else's range, so
//! the claim is a comment and the comment is a liability."
//!
//! That liability was a real bug, not a hypothetical. The parser once emitted **E0200, E0201 and
//! E0202** — `jr-hir`'s "duplicate declaration", "unresolved name" and "use before declaration" —
//! for three unrelated refusals of its own, and it stood for waves. Nothing collided at compile
//! time because a `&str` cannot collide, and no per-crate test could see it because each crate was
//! individually consistent. The audit at `354d900` found the same gap still open
//! (`docs/assessment-2026-08-07.md`, finding F7), together with a range table hand-copied into
//! three places that had already drifted apart.
//!
//! So this test reads the **union**, which is the only scope at which the invariant is stateable.
//! It lives in `jr-cli` because that crate sits at the top of the dependency graph and its tests
//! already reason about the workspace as a whole (`differential.rs` compares two whole engines).
//!
//! # Why it reads source text
//!
//! The alternative is a `pub const CODES: &[(&str, &str)]` in five crates, existing only so a test
//! can read it — public API widened for a test's convenience, which `AGENTS.md`'s "private `mod`
//! plus a curated `pub use`" rule exists to discourage. Reading the declarations keeps the surface
//! unchanged, and it buys one check an exported list could not give: that a constant's **name and
//! value agree**, so `const E0231: &str = "E0232"` is caught.
//!
//! The cost is that it depends on how a code is declared. That is pinned by
//! [`every_crate_that_raises_a_diagnostic_declares_at_least_one_code`], which fails if the walk
//! stops finding things — the guard against a silently-empty search that
//! `differential.rs::the_corpus_has_executable_programs` is the model for.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The workspace root, found by walking up from this test's manifest.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root exists")
}

/// One declared code: the crate that declares it, the code, and where.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Declared {
    /// The crate directory's name, e.g. `jr-sema`.
    krate: String,
    /// The code the constant is bound to, e.g. `E0201`. This — not the constant's name — is the
    /// identity that must be unique, because two crates colliding on a *code* is what users see.
    code: String,
    /// The declaring file, relative to the workspace root, for a failure message that locates itself.
    file: String,
    /// The constant's name. Usually the code again (`const E0201`), but `jr-mir` names its codes
    /// semantically (`const USE_OF_UNINITIALISED`), which is a second legitimate convention.
    name: String,
}

/// Every `const E….: &str = "E…."` in the workspace's crates.
fn declared_codes() -> Vec<Declared> {
    let root = workspace_root();
    let crates = root.join("crates");
    let mut found = Vec::new();
    let entries = std::fs::read_dir(&crates).expect("crates/ is readable");
    for entry in entries.filter_map(Result::ok) {
        if !entry.path().is_dir() {
            continue;
        }
        let krate = entry.file_name().to_string_lossy().into_owned();
        collect_in(&entry.path().join("src"), &root, &krate, &mut found);
    }
    found.sort();
    found
}

/// Walks one directory, recursively, collecting declarations.
fn collect_in(dir: &Path, root: &Path, krate: &str, found: &mut Vec<Declared>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            collect_in(&path, root, krate, found);
            continue;
        }
        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        for line in text.lines() {
            if let Some((name, code)) = parse_declaration(line) {
                found.push(Declared {
                    krate: krate.to_owned(),
                    code,
                    file: relative.clone(),
                    name,
                });
            }
        }
    }
}

/// A `const NAME: &str = "EXXXX";` line, as `(name, code)`.
///
/// Keyed on the **value** looking like a code rather than on the name, because `jr-mir` names its
/// codes semantically (`const USE_OF_UNINITIALISED: &str = "E0227"`) while the other crates name
/// them after the code. Both are legitimate; only the value is the identity users see.
///
/// Returns `None` for anything else, including a *use* of a code — only a declaration binds a string
/// literal of this shape to a name, which is what makes the pattern specific enough to scrape.
fn parse_declaration(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    let rest = line
        .strip_prefix("const ")
        .or_else(|| line.strip_prefix("pub const "))
        .or_else(|| line.strip_prefix("pub(crate) const "))?;
    let (name, tail) = rest.split_once(':')?;
    let open = tail.find('"')?;
    let after = &tail[open + 1..];
    let close = after.find('"')?;
    let value = &after[..close];
    if !is_code_name(value) {
        return None;
    }
    Some((name.trim().to_owned(), value.to_owned()))
}

/// Whether `name` is `E` followed by four digits.
fn is_code_name(name: &str) -> bool {
    let Some(digits) = name.strip_prefix('E') else {
        return false;
    };
    digits.len() == 4 && digits.bytes().all(|b| b.is_ascii_digit())
}

/// Codes deliberately declared in two crates, with the reason.
///
/// A shared code is not automatically wrong — it is wrong when the two uses mean *different things*,
/// which no test can judge. So each one is listed here with its justification, and adding to this
/// list is the moment to ask whether the two really are one diagnostic.
const SHARED: &[(&str, &str)] = &[(
    "E0211",
    "ambiguous imported name: jr-hir raises it during resolution and jr-sema for the same \
     ambiguity reached through a type name — one meaning, two phases (documented in \
     jr-sema/src/code.rs)",
)];

#[test]
fn every_crate_that_raises_a_diagnostic_declares_at_least_one_code() {
    // The guard against a silently-empty walk. A scraping test that stopped finding declarations
    // would pass forever and check nothing, which is worse than not existing.
    let declared = declared_codes();
    assert!(
        declared.len() >= 90,
        "expected the workspace to declare many diagnostic codes, found {} — the walk or the \
         declaration pattern has probably broken",
        declared.len()
    );
    for krate in ["jr-syntax", "jr-sema", "jr-hir", "jr-mir", "jr-db"] {
        assert!(
            declared.iter().any(|d| d.krate == krate),
            "{krate} raises diagnostics but no code declaration was found in it"
        );
    }
}

#[test]
fn a_codes_name_matches_the_literal_it_binds() {
    // `const E0231: &str = "E0232";` compiles, passes every per-crate test, and reports the wrong
    // code forever. Only a check that reads both halves can see it.
    //
    // Applies only where the constant is *named* after a code. `jr-mir`'s semantic names
    // (`USE_OF_UNINITIALISED`) carry no such claim, so there is nothing to disagree with.
    for d in declared_codes() {
        if !is_code_name(&d.name) {
            continue;
        }
        assert_eq!(
            d.name, d.code,
            "{} declares {} bound to {:?} — a constant named after a code must bind that code",
            d.file, d.name, d.code
        );
    }
}

#[test]
fn no_code_is_declared_by_two_crates() {
    // **The invariant no per-crate test can state.** E0200/E0201/E0202 were `jr-hir`'s and the
    // parser used them too; each crate was internally consistent and the collision stood for waves.
    let mut owner: BTreeMap<String, Declared> = BTreeMap::new();
    let mut collisions = Vec::new();
    for d in declared_codes() {
        match owner.get(&d.code) {
            Some(first) if first.krate != d.krate => {
                if SHARED.iter().any(|(code, _)| *code == d.code) {
                    continue;
                }
                collisions.push(format!(
                    "{} is declared by both {} ({}) and {} ({})",
                    d.code, first.krate, first.file, d.krate, d.file
                ));
            }
            Some(_) => {}
            None => {
                owner.insert(d.code.clone(), d);
            }
        }
    }
    assert!(
        collisions.is_empty(),
        "a diagnostic code means one thing across the workspace:\n  {}\n\nIf two crates genuinely \
         raise the same diagnostic, add it to SHARED with the reason.",
        collisions.join("\n  ")
    );
}

#[test]
fn the_first_free_code_is_what_agents_md_claims() {
    // `AGENTS.md` names the first free code, and that sentence has been wrong before — ADR-0047
    // found it stale once already, and the audit found it stale in two more places. It is checkable,
    // so it is checked: the claim is only trustworthy if something fails when it rots.
    const FIRST_FREE: u32 = 296;

    let highest = declared_codes()
        .iter()
        .filter_map(|d| d.code[1..].parse::<u32>().ok())
        .max()
        .expect("at least one code is declared");
    assert!(
        highest < FIRST_FREE,
        "E{highest:04} is declared, so E{FIRST_FREE:04} is not the first free code — update \
         FIRST_FREE here and the range table in AGENTS.md together"
    );
    assert_eq!(
        highest,
        FIRST_FREE - 1,
        "AGENTS.md claims E{FIRST_FREE:04} is the first free code, so E{:04} should be the highest \
         declared, but the highest is E{highest:04}",
        FIRST_FREE - 1
    );
}
