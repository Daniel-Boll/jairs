//! `jr lsp` — the language server, over stdin and stdout.
//!
//! # Why the server is a subcommand and not its own binary
//!
//! ADR-0024 §5. One binary to build, install and point an editor at, and it matches how
//! `jr` already carries `check`, `fmt`, `run`, `build` and `parse`. A second binary
//! would be a second thing to install and a second place for version skew between the
//! server and the compiler whose diagnostics it reports.
//!
//! This module is deliberately thin: everything it does is in `jr-lsp`, which is a
//! library so that its handlers can be tested without a transport (ADR-0024 §4).

use anyhow::{Context as _, Result};

use crate::cli::{GlobalArgs, LspArgs};

/// Runs the language server until the client shuts it down.
///
/// # Errors
/// Any transport or protocol failure. Returns exit code 0 on a clean shutdown, which is
/// what a client expects — a non-zero exit makes an editor report the server as crashed.
pub fn run(args: LspArgs, global: &GlobalArgs) -> Result<i32> {
    if !global.quiet {
        // On stderr, deliberately: stdout is the protocol channel, and one stray byte
        // there desynchronises the framing for the whole session.
        eprintln!("jr lsp: listening on stdio");
    }
    // Absolutised here, once. A server's working directory is whatever the editor
    // happened to have, so a relative search path means nothing to it — and the failure
    // is *silent*: a `Location` needs a `file:` URI, `jr_lsp::uri::from_path` correctly
    // refuses a relative path, and goto-definition into a module then answers "nothing
    // here" instead of erroring. Found by running the real server from a relative
    // `--module-path`, which is what a person types first.
    let cwd = std::env::current_dir().context("the working directory must be readable")?;
    let mut module_search_paths: Vec<std::path::PathBuf> = args
        .module_path
        .into_iter()
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                cwd.join(path)
            }
        })
        .collect();
    // **The manifest's module paths, then the bundled ones** — exactly as `check`, `run` and
    // `build` do, through the one resolver that ranks them (ADR-0202 §2).
    //
    // This server was the one subcommand of six that pushed no bundled directory at all
    // (ADR-0199 §1), and the omission was not cosmetic: `module_file` probes *only* the search
    // paths, so with none the server could resolve no `#import`, and the auto-import quick fix
    // silently offered nothing. It then had the *same* shape of bug once a `jairs.toml` could
    // declare paths of its own: a project with `[build] module_paths` resolved under `jr check`
    // and reported E0210 in the editor, which reads as the code being wrong rather than the
    // tool. A setting with two surfaces is half-wired until both read it.
    //
    // Resolved from the working directory, which is where an editor launches its server — the
    // same assumption `jr fmt --stdin` documents, and for the same reason: stdio carries no path
    // to resolve from at the moment the search paths must be set.
    module_search_paths.extend(
        crate::project::module_search_paths(&[])
            .context("reading the project manifest for the language server")?,
    );
    let options = jr_lsp::ServerOptions {
        module_search_paths,
    };
    jr_lsp::run_stdio(&options)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("the language server failed")?;
    Ok(0)
}
