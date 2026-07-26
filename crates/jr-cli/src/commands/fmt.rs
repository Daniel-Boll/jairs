//! Implementation of `jr fmt`.
//!
//! When `jr-fmt` is not yet implemented (its `format` function is a stub),
//! this command will report an error.  The adapter is written against the
//! published `jr_fmt` API so it starts working as soon as that crate lands.

use std::io::{self, Read as _};

use anyhow::Result;
use jr_base::SourceMap;

use crate::cli::{FmtArgs, GlobalArgs};
use crate::files::{expand_paths, read_file, write_file_atomic};
use crate::report::{emit_diagnostics, make_renderer, unified_diff};

/// Run `jr fmt`.
///
/// Returns exit code 0 on success, 1 if `--check` finds unformatted files or
/// a file fails to parse, 3 on I/O failure (propagated as `Err`).
pub fn run(args: FmtArgs, global: &GlobalArgs) -> Result<i32> {
    let colour = global.color.resolve();
    let renderer = make_renderer(colour);

    if args.stdin {
        return run_stdin(&renderer, global);
    }

    let files = expand_paths(&args.paths)?;
    let mut exit_code = 0i32;

    for path in &files {
        let text = read_file(path)?;
        let mut map = SourceMap::new();
        let file_id = map.add(path.as_path(), &text);

        let fmt_config = jr_fmt::Config::default();
        match jr_fmt::format(&text, file_id, &fmt_config) {
            Ok(formatted) => {
                if args.check {
                    if formatted != text {
                        let diff = unified_diff(path, &text, &formatted);
                        print!("{diff}");
                        if !global.quiet {
                            eprintln!("would reformat: {}", path.display());
                        }
                        exit_code = 1;
                    }
                } else {
                    // In-place mode: only write if changed.
                    if formatted != text {
                        write_file_atomic(path, &formatted)?;
                        if !global.quiet {
                            println!("{}", path.display());
                        }
                    }
                }
            }
            Err(diags) => {
                // Parse error: render diagnostics, do NOT write the file.
                emit_diagnostics(&renderer, &map, &diags);
                exit_code = 1;
            }
        }
    }

    Ok(exit_code)
}

/// Handle `jr fmt --stdin`: read from stdin, write formatted output to stdout.
fn run_stdin(renderer: &jr_diag::Renderer, _global: &GlobalArgs) -> Result<i32> {
    let mut text = String::new();
    io::stdin()
        .read_to_string(&mut text)
        .map_err(|e| anyhow::anyhow!("cannot read stdin: {e}"))?;

    let mut map = SourceMap::new();
    let file_id = map.add("<stdin>", &text);

    let fmt_config = jr_fmt::Config::default();
    match jr_fmt::format(&text, file_id, &fmt_config) {
        Ok(formatted) => {
            print!("{formatted}");
            Ok(0)
        }
        Err(diags) => {
            emit_diagnostics(renderer, &map, &diags);
            Ok(1)
        }
    }
}
