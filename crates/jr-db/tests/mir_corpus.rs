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
//! # Why nothing in the valid corpus is refused any more
//!
//! ADR-0017 §4 makes refusing a body a *feature*, and for one wave three refusals
//! stood in the valid corpus, each waiting on something that had not landed:
//!
//! - `#run has no value until jr-vm` — ADR-0016 §4 gave `#run e` a type and no value.
//! - `a file-level item has no value until jr-vm` — `jr-sema` records a constant's
//!   type but never its value.
//! - `a cross-file call needs the callee's signatures` — `Callee::Direct` named a
//!   bare `ProcId`, which indexes one file's procedures.
//!
//! All three are gone. ADR-0018 §3 evaluates constants in `file_consts`, and §5
//! widened the callee to a `ProcRef`. So the assertion here is now the strong one —
//! **no body in the valid corpus is refused at all** — and the snapshot diff that
//! deleted the three `poisoned:` lines is the evidence the VM works. That is a better
//! test than enumerating reasons, because a new refusal is now a failure rather than
//! an entry to add to a list.
//!
//! A refusal is still expected in general, and the machinery for one still exists;
//! `crates/jr-mir/tests/lowering.rs` is where the refusals are asserted positively,
//! on programs written to provoke them.

use std::path::{Path, PathBuf};

use jr_db::{JairsDatabase, ModuleCatalog, SourceFile, dump_mir, file_mir};

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

fn database() -> (JairsDatabase, ModuleCatalog) {
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
        if HOST_DEPENDENT_MIR.contains(&name.as_str()) {
            continue;
        }
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

/// Corpus programs whose MIR **legitimately** differs by host operating system, and so cannot appear in a
/// single checked-in snapshot (ADR-0206 §8).
///
/// `os()` is a compile-time value (ADR-0180 §2) folded in sema, so a program that reads it has the host's
/// answer as a *literal* in its MIR — `0_enum` on macOS, `1_enum` on Linux. Both files below exist
/// specifically to exercise that, so the dependence is the point rather than an accident: it cannot be
/// written out of them the way it was written out of `137` and `149`, whose subject was the per-OS *mapping*
/// rather than the host's own answer.
///
/// **This is a hand-maintained list, which this project distrusts on principle — so here is why it is
/// tolerable.** The rot announces itself: a new `os()`-folding program that is *not* listed makes this
/// snapshot pass on the host that generated it and **fail on the other platform's CI job**, with a diff
/// naming the file. That is the opposite of the silent staleness `AGENTS.md` records for `file_consts`'
/// feature list and `TrapKind::ALL`'s length — both of which failed *open*. This one fails closed, on a
/// platform that always runs.
///
/// The excluded programs keep their real coverage: the differential harness runs them in both engines and
/// the corpus exit-code assertions still check them, on each platform separately. What they lose is the
/// cross-platform *artifact*, which could never have described them.
///
/// **Read this before "fixing" a Linux-only snapshot failure.** From the wave that added `134` until the
/// Linux leg was read, this snapshot was wrong on x86-64 and nothing said so, because only macOS ever
/// generated it. A diff of `N_enum` values between platforms is this, not a lowering bug.
const HOST_DEPENDENT_MIR: &[&str] = &["134-target-os.jr", "135-per-os-clock.jr"];

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
fn no_body_in_the_valid_corpus_is_refused() {
    // The proof `jr-vm` works. Until ADR-0018 landed, three bodies here were refused
    // — a `#run` with no value, a constant with no value, and a cross-file call with
    // no representable callee. Const evaluation and the widened `Callee::Direct`
    // removed all three, so this asserts the absence rather than enumerating reasons:
    // a new refusal is now a test failure, not a list to extend.
    let (mut db, search) = database();
    let mut refused = Vec::new();
    let mut lowered = 0usize;

    for (name, text) in sorted_files(&corpus("corpus/valid")) {
        let file = add_file(&mut db, &name, &text);
        db.load_modules_transitively(file);
        let result = file_mir(&db, file, search);
        for (proc, outcome) in result.mir.iter() {
            match outcome {
                Ok(_) => lowered += 1,
                Err(jr_mir::Poisoned::Here(reason)) => {
                    refused.push(format!("{name}: proc {}: {reason}", proc.index()));
                }
                Err(jr_mir::Poisoned::Transitive(_)) => {
                    refused.push(format!("{name}: proc {}: transitive", proc.index()));
                }
            }
        }
    }

    assert!(
        refused.is_empty(),
        "every body in the valid corpus must lower:\n{}",
        refused.join("\n")
    );
    assert!(lowered > 0, "the valid corpus must lower at least one body");
}

#[test]
fn the_slice_exit_criterion_lowers_completely() {
    // `024-hello.jr` is `PLAN.md` §1.4's exit criterion, and it is the one file that
    // exercises everything at once: a folded `#run`, a folded string constant, a
    // struct through a slot, a cross-file call into `modules/Basic`, a loop with a
    // block parameter, and a pointer. Naming it separately means a regression says
    // which file rather than which procedure index.
    let (mut db, search) = database();
    let text = std::fs::read_to_string(corpus("corpus/valid/024-hello.jr"))
        .expect("the exit criterion must exist");
    let file = add_file(&mut db, "024-hello.jr", &text);
    db.load_modules_transitively(file);

    let result = file_mir(&db, file, search);
    assert!(!result.gated, "024-hello.jr must check cleanly");
    for (proc, outcome) in result.mir.iter() {
        assert!(
            outcome.is_ok(),
            "proc {} was refused: {outcome:?}",
            proc.index()
        );
    }

    // The two constants must have folded, or `main` would not have lowered at all.
    let dump = dump_mir(&db, file, search);
    assert!(
        dump.contains("hello from Jairs"),
        "MESSAGE must fold to its string:\n{dump}"
    );
    assert!(
        dump.contains("extern proc"),
        "print must resolve to a cross-file callee:\n{dump}"
    );
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

// ---------------------------------------------------------------------------
// The standard library
// ---------------------------------------------------------------------------

#[test]
fn the_basic_module_lowers_to_stable_mir() {
    // `modules/Basic` is not in `tests/corpus/valid/`, and `file_mir` is per file, so
    // until now the stdlib's own bodies never appeared in any snapshot — they were only
    // ever *called* from a file that was under analysis.
    //
    // That gap hid a real silent miscompile for a wave: `print :: (s: string) { write(
    // STDOUT, s.data, s.count); }` lowered both fields to `Rvalue::Undef`, because a
    // field of an aggregate *parameter* had no place, and the verifier had no objection
    // because `Undef` is a well-typed value. `write` would have been handed a garbage
    // pointer. Snapshotting the module is what makes that visible next time.
    let (mut db, search) = database();
    let path = corpus("../modules/Basic/module.jr");
    let text = std::fs::read_to_string(&path).expect("the Basic module must exist");
    let file = add_file(&mut db, "modules/Basic/module.jr", &text);
    db.load_modules_transitively(file);

    assert!(
        !file_mir(&db, file, search).gated,
        "the standard library must check cleanly"
    );
    insta::assert_snapshot!("basic_module_mir", dump_mir(&db, file, search));
}
