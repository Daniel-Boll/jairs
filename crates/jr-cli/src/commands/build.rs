//! Implementation of `jr build`.
//!
//! # What this closes
//!
//! `PLAN.md` §1.4's exit criterion is `tests/corpus/valid/024-hello.jr` running in the
//! bytecode VM *and* as a native binary, with identical output. `jr run` was the first
//! half; this is the second. Until now nothing in the compiler had ever emitted a
//! machine instruction.
//!
//! # Why it refuses to build a file with errors
//!
//! The same reason `jr run` refuses to run one: ADR-0017 §4 forbids building MIR from
//! a file with errors, so a file that fails to check has no bodies to generate code
//! for. The gate is `file_diagnostics`, the same query `jr check` and `jr run` report
//! through, so no two commands can disagree about whether a program is valid.
//!
//! # Why the exit codes are what they are
//!
//! # A build script naming its own artefact
//!
//! `BUILD_OUTPUT :: #run choose_name();` in the program names the executable, which is the makefile's most
//! basic job (ADR-0102). An explicit `-o` still wins: a person at a terminal is overriding on purpose, and a
//! script that could silently defeat the flag would make it untrustworthy.
//!
//! `1` already means "the file did not check", and it keeps that meaning here. A
//! program whose *code generation* or *link* failed is a different outcome — the
//! source was accepted and the compiler could not finish — so it is `2`. That leaves
//! `jr run`'s `4` free to keep meaning "the program trapped", which is a property of
//! running and has no analogue in a build.

use anyhow::Result;
use jr_driver::{BuildOutcome, BuildRequest, ScriptRequest, ScriptResult};

use crate::cli::{BuildArgs, GlobalArgs};
use crate::report::{emit_diagnostics, make_renderer};

/// Exit status for a program that was accepted but could not be built.
const BUILD_EXIT: i32 = 2;

/// Run `jr build`.
///
/// Everything a compilation *does* lives in `jr-driver`; this decides what to ask for and how to
/// report the answer. The split is what makes a build script possible, since a script needs the
/// driver called again with a request it computed rather than one a `clap` parser produced
/// (ADR-0195 §3).
///
/// Returns 0 on success, 1 when the file has errors, and 2 when code generation or linking failed.
///
/// # Errors
/// When the input cannot be read, or the database cannot register it — both of which are the
/// caller's problem rather than the program's.
pub fn run(args: BuildArgs, global: &GlobalArgs) -> Result<i32> {
    let renderer = make_renderer(global.color.resolve());

    for dir in &args.module_paths {
        if !dir.is_dir() {
            crate::report::warn(&format!(
                "module path `{}` is not a directory; it will be ignored",
                dir.display()
            ));
        }
    }

    let mut module_paths = args.module_paths.clone();
    module_paths.push(crate::commands::check::bundled_module_dir());

    // **`--script` is now an override, not the only way in** (ADR-0195 §6). A file that imports
    // `modules/Compiler` *is* a build script — that import is what gives it the driver's vocabulary —
    // so `jr build build.jr` does the right thing without a flag, which is the whole point of a
    // `build.jai`-shaped workflow. The flag stays for the case detection cannot see: a script that
    // reaches the module through a helper of its own.
    //
    // Detection reads one file's own imports, so an ordinary build pays a parse rather than a second
    // module tree.
    if args.script || jr_driver::is_build_script(&args.path).map_err(|e| anyhow::anyhow!(e))? {
        return run_script(&args, &renderer, module_paths);
    }

    if !args.script_args.is_empty() {
        crate::report::warn(
            "arguments after `--` are only read by a build script; add `--script` to run one",
        );
    }

    let request = BuildRequest {
        path: args.path.clone(),
        module_paths,
        library_paths: library_paths(&args),
        // `None` lets a declared `BUILD_OPT_LEVEL` decide; an explicit `-O` outranks it, which is
        // ADR-0102 §2's asymmetry — a declared name is a value the *artefact* chose, a flag is an
        // instruction from the *operator*, and the operator wins.
        opt_level: args.opt_level.map(Into::into),
        bounds_checks: !args.no_bounds_check,
        backend: args.backend.into(),
        output: args.output.clone(),
        emit_object: args.emit_object,
        kind: args.output_kind.into(),
        linker_arguments: args.linker_args.clone(),
        // Neither has a command-line form, and deliberately: generated source and a module override are
        // things a *script* computes, and an operator who wants either has a shell that can write a file.
        build_strings: Vec::new(),
        provided_imports: Vec::new(),
    };

    match jr_driver::build(&request).map_err(|e| anyhow::anyhow!(e))? {
        BuildOutcome::Built(_) => Ok(0),
        BuildOutcome::Rejected { diagnostics, map } => {
            emit_diagnostics(&renderer, &map, &diagnostics);
            Ok(1)
        }
        BuildOutcome::Failed(message) => {
            crate::report::error(&message);
            Ok(BUILD_EXIT)
        }
    }
}

/// Every directory to search for a `#system_library`, flags first.
///
/// `--library-path` comes before `JR_LIBRARY_PATH`, because a flag is the more specific instruction
/// and `ld` takes the first match — the same precedence `-o` has over a declared output
/// (ADR-0102 §2). The environment variable exists so a machine can be configured once rather than at
/// every invocation, which is what a Homebrew or Nix prefix wants.
fn library_paths(args: &crate::cli::BuildArgs) -> Vec<std::path::PathBuf> {
    let mut paths = args.library_paths.clone();
    if let Ok(value) = std::env::var("JR_LIBRARY_PATH") {
        paths.extend(std::env::split_paths(&value));
    }
    paths
}

/// Runs `jr build --script` (ADR-0195).
///
/// Reports what the script built, and every complaint it or a failed target made. The exit code
/// distinguishes the two ways a build script fails, because they have different fixes: the *script*
/// not compiling is `1`, the same as any file that does not check, and a *target* failing is
/// `BUILD_EXIT`, the same as a build that could not finish.
fn run_script(
    args: &BuildArgs,
    renderer: &jr_diag::Renderer,
    module_paths: Vec<std::path::PathBuf>,
) -> Result<i32> {
    let request = ScriptRequest {
        path: args.path.clone(),
        module_paths,
        library_paths: library_paths(args),
        arguments: args.script_args.clone(),
    };

    match jr_driver::run_script(&request).map_err(|e| anyhow::anyhow!(e))? {
        ScriptResult::ScriptRejected { diagnostics, map } => {
            emit_diagnostics(renderer, &map, &diagnostics);
            Ok(1)
        }
        ScriptResult::ScriptFailed(message) => {
            crate::report::error(&message);
            Ok(BUILD_EXIT)
        }
        ScriptResult::Ran(outcome, failures) => {
            // **A failed target's diagnostics, at its own lines.** Rendered here rather than by the
            // driver so the operator's colour choice applies, which is the reason `BuildOutcome`
            // carries them rather than printing them (see `jr-driver`'s crate docs).
            for (diagnostics, map) in &failures {
                emit_diagnostics(renderer, map, diagnostics);
            }
            for message in &outcome.reports {
                crate::report::error(message);
            }
            for path in &outcome.built {
                crate::report::note(&format!("built {}", path.display()));
            }
            if outcome.ok { Ok(0) } else { Ok(BUILD_EXIT) }
        }
    }
}
