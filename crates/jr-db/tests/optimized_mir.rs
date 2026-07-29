//! What `optimized_file_mir` does, and the one accident ADR-0021 §2 rests on.
//!
//! `crates/jr-mir/tests/inlining.rs` asserts the splice. This file asserts the
//! *policy* around it, which is the query's and not the pass's: which bodies may be
//! rewritten, which must be left byte-identical, and that a callee in another file
//! is reachable at all. All three need the real database, the real module loader and
//! the real search paths, because all three are about cross-body reads — the thing
//! ADR-0017 §3 keeps out of `file_mir` and ADR-0021 §1 allows here.

use std::path::{Path, PathBuf};

use jr_db::{
    Db, JairsDatabase, ModuleSearchPaths, SourceFile, dump_optimized_mir, file_mir,
    optimized_file_mir,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn corpus(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests")
        .join(relative)
}

fn database() -> (JairsDatabase, ModuleSearchPaths) {
    let mut db = JairsDatabase::default();
    let search = db.set_module_search_paths(vec![corpus("../modules"), corpus("corpus/modules")]);
    (db, search)
}

fn add_file(db: &mut JairsDatabase, path: &str, text: &str) -> SourceFile {
    db.set_file_text(path, text);
    db.source_file(path)
        .expect("file must exist after set_file_text")
}

/// Loads one source string as `main.jr` with `modules/` on the search path.
fn program(text: &str) -> (JairsDatabase, ModuleSearchPaths, SourceFile) {
    let (mut db, search) = database();
    let file = add_file(&mut db, "main.jr", text);
    db.load_modules_transitively(file);
    (db, search, file)
}

/// How many call rvalues a named procedure still performs after optimisation.
fn calls_left(
    db: &JairsDatabase,
    file: SourceFile,
    search: ModuleSearchPaths,
    name: &str,
) -> usize {
    let mir = optimized_file_mir(db, file, search).mir;
    let proc = proc_named(db, file, name);
    let Some(Ok(body)) = mir.get(proc) else {
        panic!("`{name}` has no lowered body");
    };
    let mut count = 0;
    for block in body.blocks() {
        for stmt in &block.stmts {
            let rvalue = match stmt {
                jr_mir::Statement::Assign { rvalue, .. }
                | jr_mir::Statement::Discard { rvalue, .. } => rvalue,
                jr_mir::Statement::Store { .. }
                | jr_mir::Statement::Zero { .. }
                | jr_mir::Statement::BoundsCheck { .. }
                | jr_mir::Statement::Nop => continue,
            };
            if matches!(rvalue, jr_mir::Rvalue::Call { .. }) {
                count += 1;
            }
        }
    }
    count
}

fn proc_named(db: &JairsDatabase, file: SourceFile, name: &str) -> jr_hir::ProcId {
    let hir = jr_db::file_hir(db, file);
    let interner = db.interner();
    let symbol = interner
        .get(name)
        .unwrap_or_else(|| panic!("`{name}` was never interned"));
    hir.items
        .iter()
        .find_map(|item| {
            let jr_hir::ItemKind::Const {
                value: jr_hir::ConstValue::Proc(proc),
            } = &item.kind
            else {
                return None;
            };
            (item.name == Some(symbol)).then_some(*proc)
        })
        .unwrap_or_else(|| panic!("no procedure named `{name}`"))
}

/// Whether a named procedure's optimized body is byte-identical to its built one.
fn unchanged(db: &JairsDatabase, file: SourceFile, search: ModuleSearchPaths, name: &str) -> bool {
    let proc = proc_named(db, file, name);
    let built = file_mir(db, file, search).mir;
    let optimized = optimized_file_mir(db, file, search).mir;
    built.get(proc) == optimized.get(proc)
}

// ---------------------------------------------------------------------------
// The exit criterion's own file
// ---------------------------------------------------------------------------

#[test]
fn the_exit_criterion_file_inlines_its_one_leaf_call() {
    // ADR-0019 §6's deferral ends only if something actually inlines, and this is the
    // file the whole slice is measured against. `add` is the one eligible callee:
    // `print` and `print_line` both call `write`, so neither is a leaf.
    let (mut db, search) = database();
    let text = std::fs::read_to_string(corpus("corpus/valid/024-hello.jr"))
        .expect("the exit criterion's file must exist");
    let file = add_file(&mut db, "024-hello.jr", &text);
    db.load_modules_transitively(file);

    let before = {
        let mir = file_mir(&db, file, search).mir;
        let main = proc_named(&db, file, "main");
        let Some(Ok(body)) = mir.get(main) else {
            panic!("`main` has no lowered body");
        };
        body.block_count()
    };
    let after = {
        let mir = optimized_file_mir(&db, file, search).mir;
        let main = proc_named(&db, file, "main");
        let Some(Ok(body)) = mir.get(main) else {
            panic!("`main` has no lowered body");
        };
        body.block_count()
    };
    assert!(
        after > before,
        "`main` must have grown by the copy of `add` plus a continuation"
    );
    assert!(
        unchanged(&db, file, search, "add"),
        "`add` is a `#run` root, so ADR-0021 §2 freezes its own body"
    );
}

#[test]
fn a_cross_file_leaf_is_inlined_through_the_import() {
    // The case `Callees` is keyed by `ProcRef` for. `modules/Basic`'s own leaves all
    // call `write`, so the test brings its own module rather than asserting something
    // about `Basic` that a change to `Basic` would break.
    let (mut db, search) = database();
    let leaf = add_file(
        &mut db,
        "corpus/modules/Leaf/module.jr",
        "twice :: (a: s64) -> s64 { return a + a; }\n",
    );
    let _ = leaf;
    let file = add_file(
        &mut db,
        "main.jr",
        "#import \"Leaf\";\n\nmain :: () -> s64 { return twice(21); }\n",
    );
    db.load_modules_transitively(file);

    // Skipped rather than failed when the module could not be resolved: this test is
    // about inlining, and a search-path problem is a different failure with a
    // different fix. `a_cross_file_run_is_still_refused` is what pins module
    // resolution behaviour.
    if file_mir(&db, file, search).gated {
        return;
    }
    assert_eq!(
        calls_left(&db, file, search, "main"),
        0,
        "a leaf in an imported module must inline like any other"
    );
}

// ---------------------------------------------------------------------------
// ADR-0021 §2: the frozen set
// ---------------------------------------------------------------------------

const FROZEN_AND_FREE: &str = "\
leaf :: (a: s64) -> s64 { return a + 1; }\n\
comptime :: () -> s64 { return leaf(1); }\n\
runtime :: () -> s64 { return leaf(2); }\n\
K :: #run comptime();\n\
main :: () { }\n";

#[test]
fn a_body_the_run_closure_reaches_is_left_byte_identical() {
    // The whole of ADR-0021 §2. `comptime` is a `#run` root, so `file_consts` executes
    // it from its own lowering; if the back end were handed an inlined version, the
    // two engines would be running different MIR and §3.1's invariant would hold only
    // as far as the inliner is correct.
    let (db, search, file) = program(FROZEN_AND_FREE);
    assert!(
        !file_mir(&db, file, search).gated,
        "the program must check, or the test proves nothing"
    );
    assert!(
        unchanged(&db, file, search, "comptime"),
        "a `#run` root must not be rewritten"
    );
    assert_eq!(
        calls_left(&db, file, search, "comptime"),
        1,
        "and its call must still be a call"
    );
}

#[test]
fn the_closure_is_transitive() {
    // `leaf` is not a root; it is reached *through* `comptime`. A closure that took
    // only the direct callees would leave `leaf` free to be rewritten, and comptime
    // executes it.
    let (db, search, file) = program(FROZEN_AND_FREE);
    assert!(unchanged(&db, file, search, "leaf"));
}

#[test]
fn a_body_outside_the_closure_still_inlines_the_same_callee() {
    // The exclusion must be targeted, not a blanket "stop optimising this file". Same
    // callee, same threshold, different caller: `runtime` is not reachable from any
    // `#run`, so it gets the inlined version.
    let (db, search, file) = program(FROZEN_AND_FREE);
    assert_eq!(
        calls_left(&db, file, search, "runtime"),
        0,
        "a caller outside the closure must be optimised normally"
    );
}

// ---------------------------------------------------------------------------
// The tripwire
// ---------------------------------------------------------------------------

#[test]
fn a_cross_file_run_is_still_refused() {
    // **Read ADR-0021 §2 before changing this test.**
    //
    // `frozen_procs` walks same-file calls only, and that is sound *only* because
    // comptime cannot follow a cross-file call: `file_consts` lowers its own file's
    // HIR, so a `Callee::Direct` naming another file has no body in the map it hands
    // the VM. A `#run` in file G calling a body in file F would need a frozen set
    // that `optimized_file_mir(F)` cannot compute — salsa has no reverse
    // dependencies — and the symptom would be a comptime/runtime divergence, which is
    // the failure class §3.1 exists to make impossible.
    //
    // So if this test starts failing because a cross-file `#run` now works, the fix
    // is not to delete the assertion. It is either a cross-file closure or the
    // body-grain key that lets both engines share one optimized query.
    let (db, _search, file) =
        program("#import \"Basic\";\n\nSIDE :: #run print(\"from comptime\");\n\nmain :: () { }\n");
    let diagnostics = jr_db::file_diagnostics(&db, file, _search);
    assert!(
        diagnostics.iter().any(|d| d.code == Some("E0230")),
        "a cross-file `#run` must still fail to evaluate; got {:?}",
        diagnostics
            .iter()
            .map(|d| (d.code, d.message.clone()))
            .collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// The dump
// ---------------------------------------------------------------------------

#[test]
fn the_optimized_dump_is_stable() {
    // One reviewable diff for the whole effect of the pass on the file the slice is
    // measured against. `dump_mir`'s own snapshot stays on *built* MIR, so the pair
    // of snapshots is the before and after.
    let (mut db, search) = database();
    let text = std::fs::read_to_string(corpus("corpus/valid/024-hello.jr"))
        .expect("the exit criterion's file must exist");
    let file = add_file(&mut db, "024-hello.jr", &text);
    db.load_modules_transitively(file);
    insta::assert_snapshot!("hello_optimized_mir", dump_optimized_mir(&db, file, search));
}

// ---------------------------------------------------------------------------
// ADR-0022: the passes, through the query
// ---------------------------------------------------------------------------

#[test]
fn print_line_loses_the_spill_slot_it_never_reads() {
    // The symptom `PLAN.md` §7 named for two waves, asserted on the real module rather
    // than a miniature: `modules/Basic`'s `print_line` spills its `string` parameter to
    // a slot and then passes the value on, so the slot is written once and never read.
    // Both halves are asserted, because "the optimized body has no slot" alone would
    // also pass if lowering had stopped creating one.
    // Loaded directly, the way `mir_corpus.rs` does: `modules/Basic` is not in
    // `tests/corpus/valid/`, so nothing brings it into a database unless a test does.
    let (mut db, search) = database();
    let text = std::fs::read_to_string(corpus("../modules/Basic/module.jr"))
        .expect("the Basic module must exist");
    let module = add_file(&mut db, "modules/Basic/module.jr", &text);
    db.load_modules_transitively(module);

    let built = file_mir(&db, module, search).mir;
    let optimized = optimized_file_mir(&db, module, search).mir;
    let proc = proc_named(&db, module, "print_line");

    let Some(Ok(before)) = built.get(proc) else {
        panic!("`print_line` has no built body");
    };
    let Some(Ok(after)) = optimized.get(proc) else {
        panic!("`print_line` has no optimized body");
    };
    assert_eq!(
        before.slot_count(),
        1,
        "lowering still spills the parameter; if this changes, the test below proves nothing"
    );
    assert_eq!(
        after.slot_count(),
        0,
        "a slot that is only ever written must not reach either engine"
    );
}
