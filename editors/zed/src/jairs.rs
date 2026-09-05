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
            .or_else(|| in_repo_build(worktree))
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

/// The compiler's own `target/release/jr`, when the worktree **is** the compiler's source tree.
///
/// `editors/nvim/lsp/jairs.lua` has always done this, and its comment gives the reason: "someone who
/// installed `jr` wants theirs, and someone hacking on the compiler wants the one they just built".
/// ADR-0026's follow-on asked that a second editor share that logic rather than re-derive it, and
/// this is as close as a WASM extension can get — it cannot read a Lua file, so the *rule* is shared
/// by being written down in both places rather than the code.
///
/// # The worktree is identified, not assumed
///
/// Returning `<root>/target/release/jr` unconditionally would hand Zed a path that does not exist for
/// every project that is not this one, and the failure would be Zed's "command not found" instead of
/// the message above, which says what to do. So the crate that *produces* the binary is read first:
/// `crates/jr-cli/Cargo.toml` declaring `name = "jr"` is the tree, and it is the most direct evidence
/// available — it is the manifest of the very thing being pointed at.
///
/// A first draft matched `crates/jr-cli` in the **workspace** manifest instead, which never matches:
/// `members` is the glob `["crates/*"]`, so no crate is named there. That would have made this
/// function silently return `None` for every project including this one — dead code that looks live.
///
/// It cannot check that the binary exists — a `Worktree` can read a text file and look on `PATH`, and
/// a compiled binary is neither. `release` rather than `debug` because a `--release` build is what an
/// editor should be talking to; someone who wants the debug one says so in settings, which wins
/// outright above.
fn in_repo_build(worktree: &zed::Worktree) -> Option<String> {
    let manifest = worktree.read_text_file("crates/jr-cli/Cargo.toml").ok()?;
    if !manifest.contains("name = \"jr-cli\"") {
        return None;
    }
    Some(format!("{}/target/release/{BINARY}", worktree.root_path()))
}

zed::register_extension!(JairsExtension);
