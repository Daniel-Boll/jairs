//! Implementation of `jr check`.

use anyhow::Result;
use jr_base::{Interner, SourceMap};
use jr_diag::{Diagnostics, Severity};
use jr_syntax::parse;

use crate::cli::{CheckArgs, GlobalArgs};
use crate::files::{expand_paths, read_file};
use crate::report::{emit_diagnostics, make_renderer, print_check_summary};

/// Run `jr check`.
///
/// Runs the front end as far as it currently goes: parse, lower to HIR, then
/// resolve names. Later waves extend this with type checking and compile-time
/// evaluation.
///
/// Returns exit code 0 if no errors, 1 if any errors were found, 3 on I/O
/// failure (propagated as `Err`).
pub fn run(args: CheckArgs, global: &GlobalArgs) -> Result<i32> {
    let colour = global.color.resolve();
    let renderer = make_renderer(colour);

    let files = expand_paths(&args.paths)?;

    // One interner across the whole run: symbols must be comparable between
    // files, since resolution will eventually span module boundaries.
    let interner = Interner::new();
    let mut total_errors: usize = 0;
    let mut map = SourceMap::new();

    for path in &files {
        let text = read_file(path)?;
        let file_id = map.add(path.as_path(), &text);

        let mut diags = Diagnostics::new();

        let parsed = parse(&text, file_id);
        diags.extend(parsed.diagnostics().iter().cloned());

        // Only lower when the tree is sound. Lowering a tree full of ERROR
        // nodes is well-defined (it produces Expr::Error and its own
        // diagnostics), but the result is a second wave of complaints about
        // damage the parser already reported, which buries the real error.
        if !parsed.has_errors() {
            let (hir, lower_diags) = jr_hir::lower_file(&parsed, file_id, &interner);
            diags.extend(lower_diags.into_vec());

            // Resolution runs with no imported scopes: mapping `#import "Basic"`
            // to a file on disk is module-loading work that does not exist yet,
            // so names coming from another module cannot be resolved and are
            // suppressed rather than reported as unknown.
            let (_res, resolve_diags) = jr_hir::resolve(&hir, &[], &interner);
            let has_imports = hir
                .items
                .iter()
                .any(|item| matches!(item.kind, jr_hir::ItemKind::Import { .. }));
            if !has_imports {
                diags.extend(resolve_diags.into_vec());
            }
        }

        emit_diagnostics(&renderer, &map, &diags);

        total_errors += diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count();
    }

    print_check_summary(files.len(), total_errors, global.quiet);

    if total_errors > 0 { Ok(1) } else { Ok(0) }
}
