//! Implementation of `jr run`.
//!
//! # What this closes
//!
//! `PLAN.md` §1.4's exit criterion is `tests/corpus/valid/024-hello.jr` running in the
//! bytecode VM *and* as a native binary, with identical output. This is the first
//! half. It is also the first time the compiler executes a Jairs program at all: `jr
//! check` stops at the type checker, and until ADR-0018 there was no evaluator to go
//! further.
//!
//! # Why it refuses to run a file with errors
//!
//! ADR-0017 §4 forbids building MIR from a file with errors, and `file_mir` enforces
//! it — so a file that fails to check has no bytecode, and the honest thing is to
//! report the diagnostics and stop rather than run whatever happened to lower. The
//! gate is `file_diagnostics`, the same query `jr check` reports through, so the two
//! commands can never disagree about whether a program is valid.
//!
//! # Why the exit codes are what they are
//!
//! `1` already means "the file did not check" for `jr check`, and a program that
//! compiled and then trapped is a different outcome from one that never compiled — a
//! script driving the compiler should be able to tell them apart. So a trap is `4`,
//! and a program that called `exit` gets its own status, because that is the status it
//! asked for.

use anyhow::Result;
use jr_base::SourceMap;
use jr_db::{Db as _, JairsDatabase, RunOutcome, file_diagnostics, run_main};
use jr_diag::Severity;

use crate::cli::{GlobalArgs, RunArgs};
use crate::report::{emit_diagnostics, make_renderer};

/// Exit status for a program that trapped or that the VM refused.
const TRAP_EXIT: i32 = 4;

/// Run `jr run`.
///
/// Returns 0 when the program ran to completion, 1 when the file has errors, 4 when it
/// trapped, and the program's own status when it called `exit`.
pub fn run(args: RunArgs, global: &GlobalArgs) -> Result<i32> {
    let colour = global.color.resolve();
    let renderer = make_renderer(colour);

    let mut db = JairsDatabase::default();

    for dir in &args.module_paths {
        if !dir.is_dir() {
            crate::report::warn(&format!(
                "module path `{}` is not a directory; it will be ignored",
                dir.display()
            ));
        }
    }

    let mut search_paths = args.module_paths.clone();
    search_paths.push(crate::commands::check::bundled_module_dir());
    let search = db.set_module_search_paths(search_paths);
    // The build setting, before any MIR query runs. ADR-0058 §2 makes it a salsa input so that
    // setting it late would still invalidate correctly — but setting it here means no query ever
    // runs under a value the user did not ask for, which is one fewer thing to reason about.
    let config = db.set_build_config(!args.no_bounds_check);

    let text = std::fs::read_to_string(&args.path)
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", args.path.display()))?;
    let key = args.path.to_string_lossy().into_owned();
    let _ = db.set_file_text(key.clone(), text);
    let root = db
        .source_file(&key)
        .ok_or_else(|| anyhow::anyhow!("internal error: {key} was not registered"))?;
    db.load_modules_transitively(root);

    let map: SourceMap = db.source_map();
    let diags = file_diagnostics(&db, root, search);
    let errors = diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    if errors > 0 {
        emit_diagnostics(&renderer, &map, &diags);
        return Ok(1);
    }

    match run_main(&db, root, search, config) {
        Ok(RunOutcome::Completed) => Ok(0),
        Ok(RunOutcome::Exited(status)) => Ok(i32::try_from(status).unwrap_or(TRAP_EXIT)),
        Ok(RunOutcome::Failed(message)) => {
            // Written verbatim: `run_main` already rendered it through
            // `jr_base::trap_message`, prefix and trailing newline included, because
            // the native back end must emit exactly the same bytes (ADR-0020 §2).
            // Passing it through `report::error` would add a second `error: `.
            eprint!("{message}");
            Ok(TRAP_EXIT)
        }
        // Assembling the program failed, which is a compiler problem rather than a
        // program one, so it propagates as an error rather than an exit status.
        Err(message) => Err(anyhow::anyhow!(
            "cannot run {}: {message}",
            args.path.display()
        )),
    }
}
