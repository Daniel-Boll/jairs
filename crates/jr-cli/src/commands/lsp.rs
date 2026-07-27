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
    let options = jr_lsp::ServerOptions {
        module_search_paths: args.module_path,
    };
    jr_lsp::run_stdio(&options)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("the language server failed")?;
    Ok(0)
}
