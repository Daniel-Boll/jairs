//! A MIR dump of every valid corpus file, snapshotted.
//!
//! This is to lowering what `jr fmt --check` is to the CST: the cheapest possible
//! proof that the phase is **total**. Every file in `tests/corpus/valid/` goes
//! through the real database — the real module loader, the real search paths — and
//! whatever comes out is pinned. A lowering that starts panicking, silently
//! dropping a body, or producing malformed MIR cannot survive it, because
//! `jr-mir`'s verifier runs inside `lower_body` and a refusal is printed rather
//! than omitted.
//!
//! # Why one snapshot rather than one per file
//!
//! `insta::glob!` would write twenty-five separate snapshot files. A single
//! concatenated snapshot, in sorted filename order, is reviewable in one diff —
//! which matters because the value of a snapshot is entirely in whether a human
//! reads the change. Sorting is what makes it deterministic; `FileMir` is already
//! a `Vec` in `ProcId` order for the same reason.
//!
//! # Why some bodies are refused, and why that is the point
//!
//! ADR-0017 §4 makes refusing a body a *feature*, so the snapshot records
//! refusals as `poisoned: <reason>` lines. Two reasons appear legitimately in the
//! valid corpus and neither is a defect:
//!
//! - `#run has no value until jr-vm` — ADR-0016 §4 gives `#run e` a type and no
//!   value. `024-hello.jr` and `020-run-directive.jr` use it.
//! - `a file-level item has no value until jr-vm` — `jr-sema` records a constant's
//!   type but never its value, because computing one needs an evaluator and the VM
//!   is the only evaluator there will be.
//!
//! Both disappear when `jr-vm` lands, at which point this snapshot changes and the
//! diff *is* the proof that it worked. That is a better test than asserting the
//! refusals do not happen.

use std::path::{Path, PathBuf};

use jr_db::{JairsDatabase, ModuleSearchPaths, SourceFile, dump_mir, file_mir};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn add_file(db: &mut JairsDatabase, path: &str, text: &str) -> SourceFile {
    db.set_file_text(path, text);
    db.source_file(path)
        .expect("file must exist after set_file_text")
}

fn corpus(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests")
        .join(relative)
}

/// Every `.jr` file in a corpus directory, in sorted filename order.
fn sorted_files(dir: &Path) -> Vec<(String, String)> {
    let mut files = Vec::new();
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{} must exist: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("directory entry").path();
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("jr") {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        let name = path
            .file_name()
            .expect("a file with an extension has a name")
            .to_string_lossy()
            .into_owned();
        files.push((name, text));
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

fn database() -> (JairsDatabase, ModuleSearchPaths) {
    let mut db = JairsDatabase::default();
    // The real `modules/` directory, so `#import "Basic"` resolves exactly as it
    // does for `jr check`.
    let search = db.set_module_search_paths(vec![corpus("../modules"), corpus("corpus/modules")]);
    (db, search)
}

// ---------------------------------------------------------------------------
// The dump
// ---------------------------------------------------------------------------

#[test]
fn valid_corpus_lowers_to_stable_mir() {
    let (mut db, search) = database();
    let mut out = String::new();

    for (name, text) in sorted_files(&corpus("corpus/valid")) {
        let file = add_file(&mut db, &name, &text);
        db.load_modules_transitively(file);
        out.push_str("========================================\n");
        out.push_str(&format!("{name}\n"));
        out.push_str("========================================\n");
        out.push_str(&dump_mir(&db, file, search));
        out.push('\n');
    }

    insta::assert_snapshot!("valid_corpus_mir", out);
}

// ---------------------------------------------------------------------------
// Totality
// ---------------------------------------------------------------------------

#[test]
fn no_valid_corpus_file_is_gated_by_its_own_diagnostics() {
    // A gated file is one the database refused to lower *at all* because it has
    // errors. Every file in `valid/` is supposed to check cleanly, so a gate here
    // means the corpus regressed rather than that MIR did.
    let (mut db, search) = database();
    let mut gated = Vec::new();

    for (name, text) in sorted_files(&corpus("corpus/valid")) {
        let file = add_file(&mut db, &name, &text);
        db.load_modules_transitively(file);
        if file_mir(&db, file, search).gated {
            gated.push(name);
        }
    }

    assert!(
        gated.is_empty(),
        "these valid corpus files have errors: {gated:?}"
    );
}

#[test]
fn every_refusal_in_the_valid_corpus_has_a_known_reason() {
    // Refusing is a feature (ADR-0017 §4), but refusing for a *new* reason is a
    // regression worth noticing, so the reasons are enumerated rather than merely
    // snapshotted. All three are features that have not landed yet — none is a
    // defect, and each disappears when its feature does.
    const EXPECTED: [&str; 3] = [
        // ADR-0016 §4: `#run e` has the type of `e` and no value until `jr-vm`.
        "#run has no value until jr-vm (ADR-0016 §4)",
        // `jr-sema` records a constant's type but never its value, because
        // computing one needs an evaluator and the VM is the only one there will be.
        "a file-level item has no value until jr-vm",
        // `Callee::Direct` names a `ProcId`, which indexes *this* file's procs.
        // Resolving an imported one needs the callee's signatures, which this
        // query is not given; cross-file reads arrive with the inliner (ADR-0017 §3).
        "a cross-file call needs the callee's signatures",
    ];

    let (mut db, search) = database();
    let mut unexpected = Vec::new();
    let mut lowered = 0usize;

    for (name, text) in sorted_files(&corpus("corpus/valid")) {
        let file = add_file(&mut db, &name, &text);
        db.load_modules_transitively(file);
        let result = file_mir(&db, file, search);
        for (proc, outcome) in result.mir.iter() {
            match outcome {
                Ok(_) => lowered += 1,
                Err(jr_mir::Poisoned::Here(reason)) => {
                    if !EXPECTED.contains(reason) {
                        unexpected.push(format!("{name}: proc {}: {reason}", proc.index()));
                    }
                }
                Err(jr_mir::Poisoned::Transitive(_)) => {
                    unexpected.push(format!("{name}: proc {}: transitive", proc.index()));
                }
            }
        }
    }

    assert!(
        unexpected.is_empty(),
        "unexpected refusals:\n{}",
        unexpected.join("\n")
    );
    assert!(lowered > 0, "the valid corpus must lower at least one body");
}

#[test]
fn the_imports_corpus_lowers_too() {
    // `imports/valid/` exists to prove cross-file *resolution* works. Lowering it
    // proves MIR does not choke on an imported name, even though a cross-file
    // *call* is still refused until the inliner gives it a reason to exist.
    let (mut db, search) = database();
    let mut gated = Vec::new();

    for (name, text) in sorted_files(&corpus("corpus/imports/valid")) {
        let file = add_file(&mut db, &name, &text);
        db.load_modules_transitively(file);
        if file_mir(&db, file, search).gated {
            gated.push(name);
        }
    }

    assert!(
        gated.is_empty(),
        "these import corpus files have errors: {gated:?}"
    );
}
