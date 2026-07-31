//! Implementation of `jr build`.
//!
//! # What this closes
//!
//! `PLAN.md` §1.4's exit criterion is `tests/corpus/valid/024-hello.jr` running in the
//! bytecode VM *and* as a native binary, with identical output. `jr run` was the first
//! half; this is the second. Until now nothing in the compiler had ever emitted a
//! machine instruction.
//!
//! # Why it refuses to build a file with errors
//!
//! The same reason `jr run` refuses to run one: ADR-0017 §4 forbids building MIR from
//! a file with errors, so a file that fails to check has no bodies to generate code
//! for. The gate is `file_diagnostics`, the same query `jr check` and `jr run` report
//! through, so no two commands can disagree about whether a program is valid.
//!
//! # Why the exit codes are what they are
//!
//! `1` already means "the file did not check", and it keeps that meaning here. A
//! program whose *code generation* or *link* failed is a different outcome — the
//! source was accepted and the compiler could not finish — so it is `2`. That leaves
//! `jr run`'s `4` free to keep meaning "the program trapped", which is a property of
//! running and has no analogue in a build.

use anyhow::Result;
use jr_base::SourceMap;
use jr_db::{Db as _, JairsDatabase, build_object, file_diagnostics};
use jr_diag::Severity;
use jr_link::{LinkRequest, link};

use crate::cli::{BuildArgs, GlobalArgs};
use crate::report::{emit_diagnostics, make_renderer};

/// Exit status for a program that was accepted but could not be built.
const BUILD_EXIT: i32 = 2;

/// Run `jr build`.
///
/// Returns 0 on success, 1 when the file has errors, and 2 when code generation or
/// linking failed.
///
/// # Errors
/// When the input cannot be read, or the database cannot register it — both of which
/// are the caller's problem rather than the program's.
pub fn run(args: BuildArgs, global: &GlobalArgs) -> Result<i32> {
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

    let built = match build_object(&db, root, search, config) {
        Ok(built) => built,
        Err(message) => {
            crate::report::error(&message);
            return Ok(BUILD_EXIT);
        }
    };

    let output = args.output.clone().unwrap_or_else(|| {
        // `hello.jr` becomes `hello`, which is what every other compiler does and what
        // a shell completion expects.
        args.path.with_extension("")
    });

    if args.emit_object {
        let object_path = output.with_extension("o");
        if let Err(error) = std::fs::write(&object_path, &built.object) {
            crate::report::error(&format!("cannot write {}: {error}", object_path.display()));
            return Ok(BUILD_EXIT);
        }
        return Ok(0);
    }

    if let Err(error) = link(&LinkRequest {
        object: &built.object,
        output: &output,
        libraries: &built.libraries,
    }) {
        crate::report::error(&error.to_string());
        return Ok(BUILD_EXIT);
    }

    Ok(0)
}
