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

    // **A bare `jr fmt` inside a project formats the project**, which is the shape every other
    // formatter has and the reason this command needed a manifest at all. Outside a project it
    // still reports that a path is required, rather than silently formatting the working
    // directory — a formatter that rewrites files nobody named is not a convenience.
    let requested = if args.paths.is_empty() {
        match crate::project::find()? {
            Some(located) => vec![located.root],
            None => anyhow::bail!(
                "no path given, and no `{}` in this directory or any parent\n\
                 \x20 give a path, or run `jr init` to make this directory a project",
                jr_manifest::FILE_NAME
            ),
        }
    } else {
        args.paths.clone()
    };

    let files = expand_paths(&requested)?;
    let mut exit_code = 0i32;
    let mut configs = ConfigCache::default();

    for path in &files {
        let text = read_file(path)?;
        let mut map = SourceMap::new();
        let file_id = map.add(path.as_path(), &text);

        // Per file, not per run: `jr fmt a/x.jr b/y.jr` may span two projects, and each file's
        // own project is the one whose style applies to it.
        let fmt_config = configs.get(path)?;
        match jr_fmt::format(&text, file_id, fmt_config) {
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

/// The formatter configuration for each file, resolved once per directory.
///
/// `jr fmt` on a directory reaches hundreds of files that overwhelmingly share one manifest, and
/// resolving it per file would walk to the project root for each of them. Keyed on the file's
/// parent rather than on the project root, because finding the root *is* the walk being avoided.
#[derive(Default)]
struct ConfigCache {
    /// Parent directory to the configuration that applies inside it.
    by_dir: std::collections::HashMap<std::path::PathBuf, jr_fmt::Config>,
}

impl ConfigCache {
    /// The configuration that applies to `path`.
    fn get(&mut self, path: &std::path::Path) -> Result<&jr_fmt::Config> {
        let dir = path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf();
        // `entry` would need the fallible lookup evaluated eagerly, so the miss is spelled out.
        if !self.by_dir.contains_key(&dir) {
            let config = crate::project::fmt_config(path)?;
            self.by_dir.insert(dir.clone(), config);
        }
        Ok(&self.by_dir[&dir])
    }
}

/// Handle `jr fmt --stdin`: read from stdin, write formatted output to stdout.
///
/// The style comes from the working directory's project, because stdin has no path of its own to
/// resolve from. That is the right guess for the case this mode exists for — an editor formatting
/// a buffer runs the command from the project it has open.
fn run_stdin(renderer: &jr_diag::Renderer, _global: &GlobalArgs) -> Result<i32> {
    let mut text = String::new();
    io::stdin()
        .read_to_string(&mut text)
        .map_err(|e| anyhow::anyhow!("cannot read stdin: {e}"))?;

    let mut map = SourceMap::new();
    let file_id = map.add("<stdin>", &text);

    let fmt_config = crate::project::find()?
        .map(|located| located.fmt_config())
        .unwrap_or_else(jr_manifest::default_fmt_config);
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
