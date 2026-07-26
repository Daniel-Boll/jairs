//! Implementation of `jr check`.

use anyhow::Result;
use jr_base::SourceMap;
use jr_syntax::parse;

use crate::cli::{CheckArgs, GlobalArgs};
use crate::files::{expand_paths, read_file};
use crate::report::{emit_diagnostics, make_renderer, print_check_summary};

/// Run `jr check`.
///
/// Returns exit code 0 if no errors, 1 if any errors were found, 3 on I/O
/// failure (propagated as `Err`).
pub fn run(args: CheckArgs, global: &GlobalArgs) -> Result<i32> {
    let colour = global.color.resolve();
    let renderer = make_renderer(colour);

    let files = expand_paths(&args.paths)?;

    let mut total_errors: usize = 0;
    let mut map = SourceMap::new();

    for path in &files {
        let text = read_file(path)?;
        let file_id = map.add(path.as_path(), &text);
        let parsed = parse(&text, file_id);
        let diags = parsed.diagnostics();

        emit_diagnostics(&renderer, &map, diags);

        if diags.has_errors() {
            total_errors += diags
                .iter()
                .filter(|d| d.severity == jr_diag::Severity::Error)
                .count();
        }
    }

    print_check_summary(files.len(), total_errors, global.quiet);

    if total_errors > 0 { Ok(1) } else { Ok(0) }
}
