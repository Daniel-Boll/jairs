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
    let options = jr_lsp::ServerOptions {
        module_search_paths: args
            .module_path
            .into_iter()
            .map(|path| {
                if path.is_absolute() {
                    path
                } else {
                    cwd.join(path)
                }
            })
            .collect(),
    };
    jr_lsp::run_stdio(&options)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("the language server failed")?;
    Ok(0)
}
