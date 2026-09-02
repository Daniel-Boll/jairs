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

/// The default build settings: bounds checks **on**.
///
/// Every test here but the two named for ADR-0058 wants the program as written, so the default
/// is spelled once rather than at each call.
fn checked(db: &mut JairsDatabase) -> jr_db::BuildConfig {
    db.set_build_config(true, jr_db::OptLevel::Standard)
}

fn add_file(db: &mut JairsDatabase, path: &str, text: &str) -> SourceFile {
    db.set_file_text(path, text);
    db.source_file(path)
        .expect("file must exist after set_file_text")
}

/// Loads one source string as `main.jr` with `modules/` on the search path.
///
/// Returns the build settings too, with bounds checks **on** — the program as written, which is
/// what every test here but the two named for ADR-0058 wants. Returned rather than looked up per
/// test so that a test which needs them *off* has to say so.
fn program(
    text: &str,
) -> (
    JairsDatabase,
    ModuleSearchPaths,
    SourceFile,
    jr_db::BuildConfig,
) {
    let (mut db, search) = database();
    let file = add_file(&mut db, "main.jr", text);
    db.load_modules_transitively(file);
    let config = checked(&mut db);
    (db, search, file, config)
}

/// How many call rvalues a named procedure still performs after optimisation.
fn calls_left(
    db: &JairsDatabase,
    file: SourceFile,
    search: ModuleSearchPaths,
    config: jr_db::BuildConfig,
    name: &str,
) -> usize {
    let mir = optimized_file_mir(db, file, search, config).mir;
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
                | jr_mir::Statement::TagCheck { .. }
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
fn unchanged(
    db: &JairsDatabase,
    file: SourceFile,
    search: ModuleSearchPaths,
    config: jr_db::BuildConfig,
    name: &str,
) -> bool {
    let proc = proc_named(db, file, name);
    let built = file_mir(db, file, search).mir;
    let optimized = optimized_file_mir(db, file, search, config).mir;
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
    let config = checked(&mut db);

    let before = {
        let mir = file_mir(&db, file, search).mir;
        let main = proc_named(&db, file, "main");
        let Some(Ok(body)) = mir.get(main) else {
            panic!("`main` has no lowered body");
        };
        body.block_count()
    };
    let after = {
        let mir = optimized_file_mir(&db, file, search, config).mir;
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
        unchanged(&db, file, search, config, "add"),
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
    let config = checked(&mut db);

    // Skipped rather than failed when the module could not be resolved: this test is
    // about inlining, and a search-path problem is a different failure with a
    // different fix. `a_cross_file_run_is_still_refused` is what pins module
    // resolution behaviour.
    if file_mir(&db, file, search).gated {
        return;
    }
    assert_eq!(
        calls_left(&db, file, search, config, "main"),
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
    let (db, search, file, config) = program(FROZEN_AND_FREE);
    assert!(
        !file_mir(&db, file, search).gated,
        "the program must check, or the test proves nothing"
    );
    assert!(
        unchanged(&db, file, search, config, "comptime"),
        "a `#run` root must not be rewritten"
    );
    assert_eq!(
        calls_left(&db, file, search, config, "comptime"),
        1,
        "and its call must still be a call"
    );
}

#[test]
fn the_closure_is_transitive() {
    // `leaf` is not a root; it is reached *through* `comptime`. A closure that took
    // only the direct callees would leave `leaf` free to be rewritten, and comptime
    // executes it.
    let (db, search, file, config) = program(FROZEN_AND_FREE);
    assert!(unchanged(&db, file, search, config, "leaf"));
}

#[test]
fn a_body_outside_the_closure_still_inlines_the_same_callee() {
    // The exclusion must be targeted, not a blanket "stop optimising this file". Same
    // callee, same threshold, different caller: `runtime` is not reachable from any
    // `#run`, so it gets the inlined version.
    let (db, search, file, config) = program(FROZEN_AND_FREE);
    assert_eq!(
        calls_left(&db, file, search, config, "runtime"),
        0,
        "a caller outside the closure must be optimised normally"
    );
}

/// A `#run` inside a **body** whose callee itself calls something (ADR-0069 §2).
///
/// `comptime` is the shape that matters: `unchanged` compares a body against its built form, so a
/// *leaf* callee would pass whether frozen or not — nothing inlines into a body with no calls. The
/// first version of this test asserted exactly that vacuous property and still passed with the whole
/// body walk disabled, which is why the callee here has a call of its own.
const BODY_RUN: &str = "\
leaf :: (a: s64) -> s64 { return a + 1; }\n\
comptime :: () -> s64 { return leaf(1); }\n\
runtime :: () -> s64 { return leaf(2); }\n\
main :: () {\n\
\x20   n := #run comptime();\n\
\x20   m := runtime();\n\
\x20   if n + m == 5 { return; }\n\
}\n";

#[test]
fn a_body_run_freezes_its_callee_too() {
    // ADR-0069 §2 put a `#run` in a body, which means the closure ADR-0021 §2 protects has a second
    // kind of root. Missing it would be **silent**: the inlined body is still correct at run time, and
    // only the comptime result could differ — the exact hazard §2 wrote the closure for.
    let (db, search, file, config) = program(BODY_RUN);
    assert!(
        !file_mir(&db, file, search).gated,
        "the program must check, or the test proves nothing"
    );
    assert!(
        unchanged(&db, file, search, config, "comptime"),
        "a callee a body `#run` reaches must not be rewritten"
    );
    assert_eq!(
        calls_left(&db, file, search, config, "comptime"),
        1,
        "and its call must still be a call"
    );
}

#[test]
fn a_body_run_does_not_freeze_the_whole_file() {
    // The counterweight, and the reason this closure is computed from the `#run`'s *subtree* rather
    // than from every call in the body. The first implementation walked whole body arenas — a file-level
    // arena holds only file-level expressions, so walking all of it is cheap, but a body's holds
    // everything — and froze almost every procedure in the program, which disabled the bounds-check
    // strip and broke two tests in this file.
    //
    // `runtime` is not reachable from the `#run`, so it must still be optimised.
    let (db, search, file, config) = program(BODY_RUN);
    assert_eq!(
        calls_left(&db, file, search, config, "runtime"),
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
    let (db, _search, file, _config) =
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
    let config = checked(&mut db);
    insta::assert_snapshot!(
        "hello_optimized_mir",
        dump_optimized_mir(&db, file, search, config)
    );
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
    let config = checked(&mut db);

    let built = file_mir(&db, module, search).mir;
    let optimized = optimized_file_mir(&db, module, search, config).mir;
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
    assert!(
        write_only_slots(before) > 0,
        "the built body's spill slot is written and never read; if that stops being true, \
         the assertion below proves nothing"
    );
    // **The property, not the count** (ADR-0145 §1). This used to assert `slot_count() == 0`,
    // which was the same thing while `print_line`'s only slot was its own write-only spill.
    // Non-leaf inlining changed the arithmetic rather than the property: `print_line` now
    // absorbs `print`, twice, and each copy brings a `string` temporary that *is* read — so
    // the optimized body legitimately has slots, and what must still hold is that none of
    // them is dead.
    assert_eq!(
        write_only_slots(after),
        0,
        "a slot that is only ever written must not reach any engine"
    );
}

/// How many slots a body stores to and never loads from.
///
/// Counted through places rather than through `Statement::Store` alone, because a load can
/// reach a slot by any projection path and a slot read through `s0.data` is read.
fn write_only_slots(body: &jr_mir::MirBody) -> usize {
    use rustc_hash::FxHashSet;
    let mut written: FxHashSet<jr_mir::SlotId> = FxHashSet::default();
    let mut read: FxHashSet<jr_mir::SlotId> = FxHashSet::default();
    let note = |place: &jr_mir::Place, set: &mut FxHashSet<jr_mir::SlotId>| {
        if let jr_mir::PlaceBase::Slot(slot) = place.base {
            set.insert(slot);
        }
    };
    for block in body.blocks() {
        for stmt in &block.stmts {
            match stmt {
                jr_mir::Statement::Store { place, .. } | jr_mir::Statement::Zero { place, .. } => {
                    note(place, &mut written);
                }
                jr_mir::Statement::Assign { rvalue, .. }
                | jr_mir::Statement::Discard { rvalue, .. } => match rvalue {
                    // An `Address` counts as a read: the slot escapes, so something may read
                    // it through the pointer and nothing here can prove otherwise.
                    jr_mir::Rvalue::Load(place) | jr_mir::Rvalue::Address(place) => {
                        note(place, &mut read);
                    }
                    // An atomic reaches memory through a *pointer operand*, never a `Place`, so it names
                    // no slot here — the slot it points into escaped through an `Address` above.
                    jr_mir::Rvalue::Use(_)
                    | jr_mir::Rvalue::Binary { .. }
                    | jr_mir::Rvalue::Unary { .. }
                    | jr_mir::Rvalue::Convert { .. }
                    | jr_mir::Rvalue::Call { .. }
                    | jr_mir::Rvalue::Atomic { .. }
                    | jr_mir::Rvalue::Undef => {}
                },
                jr_mir::Statement::BoundsCheck { .. }
                | jr_mir::Statement::TagCheck { .. }
                | jr_mir::Statement::Nop => {}
            }
        }
    }
    written.difference(&read).count()
}

// ---------------------------------------------------------------------------
// ADR-0058: the bounds-check build setting
// ---------------------------------------------------------------------------

/// How many `BoundsCheck` statements a named procedure's optimized body still holds.
fn checks_left(
    db: &JairsDatabase,
    file: SourceFile,
    search: ModuleSearchPaths,
    config: jr_db::BuildConfig,
    name: &str,
) -> usize {
    let mir = optimized_file_mir(db, file, search, config).mir;
    let proc = proc_named(db, file, name);
    let body = match mir.get(proc) {
        Some(Ok(body)) => body,
        other => panic!("`{name}` has no lowered body: {other:?}"),
    };
    body.blocks()
        .iter()
        .flat_map(|block| &block.stmts)
        .filter(|stmt| matches!(stmt, jr_mir::Statement::BoundsCheck { .. }))
        .count()
}

/// A program with one index the mid-end cannot prove in range.
///
/// The index comes from a **parameter**, deliberately. A constant index is exactly what
/// const-prop deletes the check for — which is ADR-0003's other half working — so a test using
/// `buf[2]` would find zero checks under both settings and prove nothing about the pass.
const INDEXED: &str = "read :: (buf: [4]s64, i: s64) -> s64 {\n    return buf[i];\n}\n\nmain :: () {\n    xs: [4]s64;\n    xs[1] = 5;\n    if read(xs, 1) == 5 {\n        return;\n    }\n}\n";

#[test]
fn the_check_is_there_by_default() {
    // The control. Every other test in this group would also pass if the pass always stripped, or
    // if lowering had quietly stopped emitting a check at all — so the *presence* of one under the
    // default settings is what the rest of the group rests on.
    let (db, search, file, config) = program(INDEXED);
    assert_eq!(
        checks_left(&db, file, search, config, "read"),
        1,
        "an index the mid-end cannot prove in range must keep its check"
    );
}

#[test]
fn no_bounds_check_strips_it() {
    let (mut db, search) = database();
    let file = add_file(&mut db, "main.jr", INDEXED);
    db.load_modules_transitively(file);
    let unchecked = db.set_build_config(false, jr_db::OptLevel::Standard);
    assert_eq!(
        checks_left(&db, file, search, unchecked, "read"),
        0,
        "`--no-bounds-check` must leave no check for either engine to run"
    );
}

#[test]
fn toggling_the_setting_re_runs_the_query() {
    // The reason ADR-0058 §2 made this a salsa input rather than a parameter a caller remembers to
    // pass. Both answers are asked of *one* database, in sequence, so a stale memo would return
    // the first answer twice — which is precisely what a non-input configuration would do, and it
    // would look like the flag having no effect.
    let (mut db, search) = database();
    let file = add_file(&mut db, "main.jr", INDEXED);
    db.load_modules_transitively(file);

    let checked_config = db.set_build_config(true, jr_db::OptLevel::Standard);
    assert_eq!(checks_left(&db, file, search, checked_config, "read"), 1);

    let unchecked = db.set_build_config(false, jr_db::OptLevel::Standard);
    assert_eq!(
        checks_left(&db, file, search, unchecked, "read"),
        0,
        "the second answer must reflect the new setting, not a memo of the first"
    );

    // And back, because an input that invalidated in one direction only would pass the assertion
    // above while being broken.
    let rechecked = db.set_build_config(true, jr_db::OptLevel::Standard);
    assert_eq!(
        checks_left(&db, file, search, rechecked, "read"),
        1,
        "turning checks back on must restore them"
    );
}

#[test]
fn no_abc_strips_one_procedure_and_leaves_the_others() {
    // ADR-0058 §3's granularity, asserted in both directions at once. A test that only checked
    // the `#no_abc` procedure would pass if the flag had been read as a *file*-level setting.
    let source = "raw :: (buf: [4]s64, i: s64) -> s64 #no_abc {\n    return buf[i];\n}\n\nsafe :: (buf: [4]s64, i: s64) -> s64 {\n    return buf[i];\n}\n\nmain :: () {\n    xs: [4]s64;\n    xs[1] = 5;\n    if raw(xs, 1) == safe(xs, 1) {\n        return;\n    }\n}\n";
    let (db, search, file, config) = program(source);
    assert_eq!(
        checks_left(&db, file, search, config, "raw"),
        0,
        "`#no_abc` must suppress the check whatever the build says"
    );
    assert_eq!(
        checks_left(&db, file, search, config, "safe"),
        1,
        "a procedure without the directive keeps its check"
    );
}

#[test]
fn comptime_keeps_its_checks_under_the_flag() {
    // ADR-0058 §4, and the reason it is a *decision* rather than an accident worth hiding: a trap
    // at compile time is a diagnostic, and folding an out-of-range read into a constant would be a
    // well-typed garbage value — this project's first named failure mode.
    //
    // The **message** is asserted, not merely the presence of an error. The first version of this
    // test used `#run read(---, 9)` and passed while proving nothing: the error was "`---` has no
    // value", raised before any index was evaluated, so the test would have stayed green with the
    // check stripped. A test that passes for the wrong reason is the thing ADR-0058 §5 is about.
    let (mut db, search) = database();
    let file = add_file(
        &mut db,
        "main.jr",
        "oob :: () -> s64 {\n    buf: [4]s64;\n    i := 9;\n    return buf[i];\n}\n\nBAD :: #run oob();\n\nmain :: () { }\n",
    );
    db.load_modules_transitively(file);
    // Set, and deliberately not passed to anything: the point is that no setting reaches
    // `file_consts`, which lowers its own MIR and never calls `optimize` (ADR-0058 §4).
    let _unchecked = db.set_build_config(false, jr_db::OptLevel::Standard);

    let diagnostics = jr_db::file_diagnostics(&db, file, search);
    let messages: Vec<String> = diagnostics.iter().map(|d| d.message.clone()).collect();
    assert!(
        messages.iter().any(|m| m.contains("index out of bounds")),
        "an out-of-range comptime index must trap even under `--no-bounds-check`, and for that \
         reason rather than another: {messages:?}"
    );
}

// ---------------------------------------------------------------------------
// ADR-0142: the optimisation level
// ---------------------------------------------------------------------------

/// A program with something for every pass to do: a leaf call to inline, a struct in a slot
/// for forwarding, arithmetic to fold, and a branch that folding makes dead.
const OPTIMISABLE: &str = "Point :: struct {\n    x: s64;\n    y: s64;\n}\n\nadd :: (a: s64, b: s64) -> s64 {\n    return a + b;\n}\n\nmain :: () {\n    p: Point;\n    p.x = 4;\n    p.y = 5;\n    total := add(p.x, p.y);\n    if total > 5 {\n        return;\n    }\n    total = 0;\n}\n";

#[test]
fn the_default_level_optimises() {
    // The control, and the thing every assertion below rests on: `OptLevel::Standard` must
    // actually rewrite this body, or "`Off` leaves it alone" would be true of both levels and
    // would say nothing.
    let (db, search, file, config) = program(OPTIMISABLE);
    assert_eq!(
        calls_left(&db, file, search, config, "main"),
        0,
        "the default level must inline the leaf call"
    );
    assert!(
        !unchanged(&db, file, search, config, "main"),
        "the default level must rewrite a body with work in it"
    );
}

#[test]
fn opt_level_off_is_an_identity() {
    // ADR-0142 §2, and stronger than "the answer is the same": *every* body is byte-identical
    // to what `file_mir` built. That is what makes `-O0` usable for attribution — a wrong
    // answer that survives it is not a pass's, because no pass ran.
    //
    // Every body rather than `main`'s, because a pass that skipped its level check for one
    // procedure kind — a module's, an instantiation's — would be invisible in a single-body
    // assertion.
    let (mut db, search) = database();
    let file = add_file(&mut db, "main.jr", OPTIMISABLE);
    db.load_modules_transitively(file);
    let off = db.set_build_config(true, jr_db::OptLevel::Off);

    let built = file_mir(&db, file, search).mir;
    let unoptimized = optimized_file_mir(&db, file, search, off).mir;
    let mut bodies = 0;
    for (proc, body) in built.iter() {
        assert_eq!(
            unoptimized.get(proc),
            Some(body),
            "`OptLevel::Off` must pass every body through unchanged"
        );
        bodies += 1;
    }
    assert!(
        bodies > 0,
        "the program must have lowered bodies to compare"
    );
}

#[test]
fn the_level_is_a_salsa_input_that_invalidates() {
    // The same property `toggling_the_setting_re_runs_the_query` asserts for the bounds-check
    // field, for the second field, and for the same reason: a memo computed under the old level
    // would make the flag look inert. Asserted in both directions, since an input that
    // invalidates one way only passes a one-way test.
    let (mut db, search) = database();
    let file = add_file(&mut db, "main.jr", OPTIMISABLE);
    db.load_modules_transitively(file);

    let standard = db.set_build_config(true, jr_db::OptLevel::Standard);
    assert_eq!(calls_left(&db, file, search, standard, "main"), 0);

    let off = db.set_build_config(true, jr_db::OptLevel::Off);
    assert_eq!(
        calls_left(&db, file, search, off, "main"),
        1,
        "at `Off` the call must still be there, not a memo of the inlined body"
    );

    let back = db.set_build_config(true, jr_db::OptLevel::Standard);
    assert_eq!(
        calls_left(&db, file, search, back, "main"),
        0,
        "returning to the default level must optimise again"
    );
}

#[test]
fn the_level_and_the_bounds_check_are_independent() {
    // ADR-0142 §2 refuses to make a safety setting depend on an optimisation setting, so the
    // strip pass runs at `Off` too. Both directions of the cross-product that can be observed
    // here: a check present at `Off` with checks on, and absent at `Off` with them off.
    let (mut db, search) = database();
    let file = add_file(&mut db, "main.jr", INDEXED);
    db.load_modules_transitively(file);

    let off_checked = db.set_build_config(true, jr_db::OptLevel::Off);
    assert_eq!(
        checks_left(&db, file, search, off_checked, "read"),
        1,
        "`-O0` must not strip a bounds check on its own"
    );

    let off_unchecked = db.set_build_config(false, jr_db::OptLevel::Off);
    assert_eq!(
        checks_left(&db, file, search, off_unchecked, "read"),
        0,
        "`--no-bounds-check` must be honoured at every level"
    );
}
