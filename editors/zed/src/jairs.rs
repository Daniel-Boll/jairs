//! The Jairs extension for Zed: it supplies the `jr lsp` command and nothing else.
//!
//! # Why there is any Rust here at all
//!
//! Zed reads a grammar, a language config and query files straight from the extension directory —
//! none of that needs code. A **language server** does: Zed asks the extension for the command to
//! run, because only the extension knows how to find the binary. So this file is one method.
//!
//! # Why it passes `--module-path`
//!
//! `jr lsp` searches for `#import`ed modules only where it is told (ADR-0014 §1), and a search path
//! guessed wrong silently changes which module a program means. `jr` appends its own bundled
//! `modules/` directory (ADR-0199 §1), so a plain install already resolves the standard library —
//! but a project with modules of its own has to say where they are, and the worktree is the only
//! place this extension can learn that from.
//!
//! The directory is passed **unconditionally**, and a first draft of this file checked whether it
//! existed first. That check was removed because its premise was wrong: `module_file` resolves a name
//! by reading `<dir>/<Name>/module.jr`, so a search path that is not there simply misses — a failed
//! stat, not an error. The check bought nothing and was the most fragile code in the file, since the
//! only way to ask a `Worktree` whether a *directory* exists is to try to read it as text.

use zed_extension_api::{self as zed, LanguageServerId, Result, settings::LspSettings};

/// The subdirectory a Jairs project keeps its own modules in, by convention.
///
/// `modules` matches this repository's own layout and the root marker Neovim already uses
/// (ADR-0026), so a project laid out the one documented way needs no configuration at all.
const CONVENTIONAL_MODULE_DIR: &str = "modules";

/// The binary name, looked up on `PATH` when settings do not name one.
const BINARY: &str = "jr";

struct JairsExtension;

impl zed::Extension for JairsExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        // Settings win outright, and both halves of them: someone pointing Zed at a `jr` they built
        // wants that one, and someone passing their own arguments has a layout this cannot guess.
        let settings = LspSettings::for_worktree(server_id.as_ref(), worktree).ok();
        let configured = settings.and_then(|settings| settings.binary);

        let path = configured
            .as_ref()
            .and_then(|binary| binary.path.clone())
            .or_else(|| worktree.which(BINARY))
            .ok_or_else(|| {
                format!(
                    "`{BINARY}` was not found on PATH. Install it, or set \
                     lsp.Jairs.binary.path in your Zed settings."
                )
            })?;

        if let Some(args) = configured.and_then(|binary| binary.arguments) {
            return Ok(zed::Command {
                command: path,
                args,
                env: worktree.shell_env(),
            });
        }

        // The project's own `modules/`, plus whatever `jr` appends for itself. Absolute, because a
        // server's working directory is whatever the editor happened to have and a relative search
        // path fails *silently* — the reason `jr lsp` absolutises (ADR-0025 §5).
        let args = vec![
            String::from("lsp"),
            String::from("--module-path"),
            format!("{}/{CONVENTIONAL_MODULE_DIR}", worktree.root_path()),
        ];

        Ok(zed::Command {
            command: path,
            args,
            env: worktree.shell_env(),
        })
    }
}

zed::register_extension!(JairsExtension);
