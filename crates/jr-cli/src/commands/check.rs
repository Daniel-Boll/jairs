//! Implementation of `jr check`.

use anyhow::Result;
use jr_base::SourceMap;
use jr_db::{Db as _, JairsDatabase, file_diagnostics};
use jr_diag::Severity;

use crate::cli::{CheckArgs, GlobalArgs};
use crate::files::expand_paths;
use crate::report::{emit_diagnostics, make_renderer, print_check_summary};

/// The module directory shipped with the compiler.
///
/// Searched after any `--module-path` given on the command line (ADR-0014 §1).
/// Resolved relative to the workspace at build time, which is adequate while the
/// compiler runs from its own source tree; installing `jr` will need a real
/// installation-relative lookup.
fn bundled_module_dir() -> std::path::PathBuf {
    // Walk up from `crates/jr-cli` rather than joining `../../`, so the path
    // that appears in an E0210 diagnostic reads as `<repo>/modules` instead of
    // `<repo>/crates/jr-cli/../../modules`.
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap_or(manifest)
        .join("modules")
}

/// Run `jr check`.
///
/// Runs the front end as far as it currently goes: parse, lower to HIR, load
/// imported modules, then resolve names. Later waves extend this with type
/// checking and compile-time evaluation.
///
/// Returns exit code 0 if no errors, 1 if any errors were found, 3 on I/O
/// failure (propagated as `Err`).
pub fn run(args: CheckArgs, global: &GlobalArgs) -> Result<i32> {
    let colour = global.color.resolve();
    let renderer = make_renderer(colour);

    let files = expand_paths(&args.paths)?;

    // One database for the whole run, so that a module imported by several files
    // is parsed and lowered once (ADR-0007).
    let mut db = JairsDatabase::default();

    // A mistyped `--module-path` otherwise shows up only indirectly, as every
    // import failing with E0210. Saying so up front is cheaper than making the
    // user infer it from a list of paths that do not exist.
    for dir in &args.module_paths {
        if !dir.is_dir() {
            crate::report::warn(&format!(
                "module path `{}` is not a directory; it will be ignored",
                dir.display()
            ));
        }
    }

    let mut search_paths = args.module_paths.clone();
    search_paths.push(bundled_module_dir());
    let search_paths_input = db.set_module_search_paths(search_paths);

    // Register every file the user asked about, then pull in the modules they
    // import transitively.
    let mut roots = Vec::with_capacity(files.len());
    for path in &files {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;
        let key = path.to_string_lossy().into_owned();
        // `set_file_text` returns the stable `FileId`; the queries take the
        // salsa input handle, which we look up by the same key.
        let _ = db.set_file_text(key.clone(), text);
        let source_file = db
            .source_file(&key)
            .ok_or_else(|| anyhow::anyhow!("internal error: {key} was not registered"))?;
        roots.push(source_file);
    }
    for root in &roots {
        db.load_modules_transitively(*root);
    }

    let mut total_errors: usize = 0;

    // Snapshot the source map once. It is taken from the database so that spans
    // in *modules* -- not just in the files named on the command line -- can be
    // rendered. Every file is registered by now, and `source_map()` deep-clones
    // the whole map, so taking it per-file would be quadratic in source bytes.
    let map: SourceMap = db.source_map();

    for source_file in &roots {
        // `file_diagnostics` covers parse, lower and resolve in source order.
        //
        // Note there is no longer any suppression here: before module loading
        // existed, resolution diagnostics were discarded for any file containing
        // an `#import`, because every imported name would have been reported
        // unresolved. That workaround is gone, which is the point of this wave.
        let diags = file_diagnostics(&db, *source_file, search_paths_input);

        emit_diagnostics(&renderer, &map, &diags);

        total_errors += diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count();
    }

    print_check_summary(files.len(), total_errors, global.quiet);

    if total_errors > 0 { Ok(1) } else { Ok(0) }
}
