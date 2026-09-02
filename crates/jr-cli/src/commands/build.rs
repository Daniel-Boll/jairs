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
use jr_base::SourceMap;
use jr_db::{Db as _, JairsDatabase, build_object, file_diagnostics};
use jr_diag::Severity;
use jr_link::{LinkRequest, link};

use crate::cli::{BuildArgs, GlobalArgs};
use crate::report::{emit_diagnostics, make_renderer};

/// Exit status for a program that was accepted but could not be built.
const BUILD_EXIT: i32 = 2;

/// Checks a **declared** `BUILD_OUTPUT` and turns it into a path, or says why not (ADR-0122).
///
/// `BUILD_OUTPUT :: #run choose_name();` lets the program name its own artefact (ADR-0102), and the value is
/// computed by arbitrary compile-time code *in the file being compiled*. So it is attacker-controlled exactly
/// when the source is — which is the ordinary case for a compiler: someone builds code they did not write.
/// Nothing checked it, and the consequences were not subtle:
///
/// - an **absolute** path, or one climbing out with `..`, made `jr build` write an executable anywhere the
///   user could — `.git/hooks/pre-commit` being the sharp example, since git runs it on the next commit;
/// - a leading `-` was passed to `cc` as its **first positional argument** and to `codesign` as its last, so
///   it was read as a flag rather than a path.
///
/// Only a *declared* name is checked. An explicit `-o` is not, because that is a person at a terminal saying
/// where they want the file, and second-guessing them would make the flag less useful than a shell
/// redirection — the same reasoning that lets `-o` beat the declaration in the first place.
///
/// Relative subdirectories stay legal (`build/app`), because naming one is an ordinary thing for a build
/// script to do and forbidding it would push people back to `-o`. Confinement is by rejecting anything that
/// *leaves* the working directory, not by flattening the name.
///
/// # Errors
/// A sentence naming what is wrong, for the driver to print.
fn confined_output(declared: &str) -> Result<std::path::PathBuf, String> {
    use std::path::{Component, Path};

    if declared.is_empty() {
        return Err("it is empty".to_owned());
    }
    if declared.contains('\0') {
        return Err("it contains a NUL byte".to_owned());
    }
    // Checked on the string rather than on a component, because it is `cc`'s argument parser that will read
    // a leading `-` as a flag, and that sees the whole path.
    if declared.starts_with('-') {
        return Err(
            "it starts with `-`, which a linker reads as a flag rather than a path".to_owned(),
        );
    }

    let path = Path::new(declared);
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                return Err(
                    "it is an absolute path, and a build writes inside the working directory"
                        .to_owned(),
                );
            }
            Component::ParentDir => {
                return Err("it climbs out of the working directory with `..`".to_owned());
            }
            Component::CurDir | Component::Normal(_) => {}
        }
    }
    // Every component was `.` — `"."` or `"./."` — so there is no file to write.
    if path.file_name().is_none() {
        return Err("it names a directory rather than a file".to_owned());
    }
    Ok(path.to_owned())
}

/// Run `jr build`.
///
/// Returns 0 on success, 1 when the file has errors, and 2 when code generation or
/// linking failed.
///
/// # Errors
/// When the input cannot be read, or the database cannot register it — both of which
/// are the caller's problem rather than the program's.
pub fn run(args: BuildArgs, global: &GlobalArgs) -> Result<i32> {
    let colour = global.color.resolve();
    let renderer = make_renderer(colour);

    let mut db = JairsDatabase::default();

    for dir in &args.module_paths {
        if !dir.is_dir() {
            crate::report::warn(&format!(
                "module path `{}` is not a directory; it will be ignored",
                dir.display()
            ));
        }
    }

    let mut search_paths = args.module_paths.clone();
    search_paths.push(crate::commands::check::bundled_module_dir());
    let search = db.set_module_search_paths(search_paths);
    // The build setting, before any MIR query runs. ADR-0058 §2 makes it a salsa input so that
    // setting it late would still invalidate correctly — but setting it here means no query ever
    // runs under a value the user did not ask for, which is one fewer thing to reason about.
    // **A bootstrap configuration first**, then the real one (ADR-0154 §1). A build script may declare
    // `BUILD_OPT_LEVEL`, and reading a declared constant means *compiling* — so an option that affects
    // compilation cannot be read without already having chosen one.
    //
    // The bootstrap is sound because of ADR-0142's check, not by assumption: every corpus program behaves
    // identically at both optimisation levels, so a constant read at one level has the same value at the
    // other. Without that check this would be a guess.
    let bootstrap = db.set_build_config(!args.no_bounds_check, jr_db::OptLevel::Standard);
    let _ = bootstrap;

    let text = std::fs::read_to_string(&args.path)
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", args.path.display()))?;
    let key = args.path.to_string_lossy().into_owned();
    let _ = db.set_file_text(key.clone(), text);
    let root = db
        .source_file(&key)
        .ok_or_else(|| anyhow::anyhow!("internal error: {key} was not registered"))?;
    db.load_modules_transitively(root);

    // **The real configuration.** A `-O` on the command line wins; otherwise a declared
    // `BUILD_OPT_LEVEL` applies; otherwise the default. That order is ADR-0102 §2's asymmetry, unchanged:
    // a declared name is a value the *artefact under compilation* chose, an `-O` is an instruction from
    // the *operator* compiling it, and the operator outranks the artefact.
    let level = match args.opt_level {
        Some(flag) => flag.into(),
        None => jr_db::declared_opt_level(&db, root, search).unwrap_or(jr_db::OptLevel::Standard),
    };
    let config = db.set_build_config(!args.no_bounds_check, level);

    let map: SourceMap = db.source_map();
    // **Every reachable file, not only the root** (ADR-0108 §1). `file_diagnostics` answers for one file, so a
    // root whose imported module was broken passed this gate and failed *inside the engine* — `List` calling
    // `malloc` without importing `Basic` gave `no routine for file 2 proc 0` for a program the gate had just
    // approved (ADR-0107 §5). Refusing here is what turns that into the module's own diagnostic at its own line.
    //
    // The reachable set is the same one the MIR assembly below walks, so this adds no query and cannot disagree
    // with what is about to be compiled.
    let mut diags = jr_diag::Diagnostics::new();
    for file in jr_db::reachable_files(&db, root, search) {
        diags.extend(file_diagnostics(&db, file, search).iter().cloned());
    }
    let errors = diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    if errors > 0 {
        emit_diagnostics(&renderer, &map, &diags);
        return Ok(1);
    }

    let built = match build_object(&db, root, search, config, args.backend.into()) {
        Ok(built) => built,
        Err(message) => {
            crate::report::error(&message);
            return Ok(BUILD_EXIT);
        }
    };

    // **`-o` wins over a declared `BUILD_OUTPUT`** (ADR-0102 §2). A person at a terminal is overriding on
    // purpose, and a build script that could silently defeat `-o` would make the flag untrustworthy. The
    // reverse precedence would also make a script's own output name unpredictable from reading the file.
    let output = match args.output.clone() {
        Some(explicit) => explicit,
        None => match jr_db::declared_build_output(&db, root, search) {
            // **A declared name is confined** (ADR-0122). The value is computed by arbitrary compile-time
            // code in the file being compiled, so it is attacker-controlled whenever the source is, and
            // nothing checked it: an absolute path or a `..` chain made `jr build` write an executable
            // anywhere the user could, and a leading `-` was read as a flag by `cc`.
            Some(declared) => match confined_output(&declared) {
                Ok(path) => path,
                Err(reason) => {
                    crate::report::error(&format!(
                        "the declared BUILD_OUTPUT {declared:?} is not a usable output name: {reason}"
                    ));
                    return Ok(BUILD_EXIT);
                }
            },
            // `hello.jr` becomes `hello`, which is what every other compiler does and what
            // a shell completion expects.
            None => args.path.with_extension(""),
        },
    };

    if args.emit_object {
        let object_path = output.with_extension("o");
        if let Err(error) = std::fs::write(&object_path, &built.object) {
            crate::report::error(&format!("cannot write {}: {error}", object_path.display()));
            return Ok(BUILD_EXIT);
        }
        return Ok(0);
    }

    if let Err(error) = link(&LinkRequest {
        object: &built.object,
        output: &output,
        libraries: &built.libraries,
    }) {
        crate::report::error(&error.to_string());
        return Ok(BUILD_EXIT);
    }

    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::confined_output;

    #[test]
    fn an_ordinary_name_is_accepted() {
        assert!(confined_output("app").is_ok());
        assert!(confined_output("my-app").is_ok());
        assert!(confined_output("./app").is_ok());
    }

    #[test]
    fn a_relative_subdirectory_stays_legal() {
        // Naming a subdirectory is an ordinary thing for a build script to do, and forbidding it would push
        // people back to `-o` — confinement is about *leaving* the working directory, not about flattening.
        assert!(confined_output("build/app").is_ok());
        assert!(confined_output("target/release/app").is_ok());
    }

    #[test]
    fn an_absolute_path_is_refused() {
        let reason = confined_output("/tmp/app").expect_err("an absolute path escapes the build");
        assert!(reason.contains("absolute"), "{reason}");
    }

    #[test]
    fn climbing_out_is_refused() {
        // The sharp case: git runs `.git/hooks/pre-commit` on the next commit, so writing an executable
        // there turns "I compiled someone's file" into "I ran their code".
        let reason = confined_output("../../.git/hooks/pre-commit")
            .expect_err("`..` escapes the working directory");
        assert!(reason.contains(".."), "{reason}");
    }

    #[test]
    fn a_leading_dash_is_refused() {
        // `jr-link` passes the object path as `cc`'s **first positional argument**, so a leading `-` is read
        // as a flag. Checked on the whole string rather than a component, because that is what `cc` parses.
        let reason =
            confined_output("-Wl,--version").expect_err("a linker reads a leading `-` as a flag");
        assert!(reason.contains("flag"), "{reason}");
    }

    #[test]
    fn an_empty_or_directory_name_is_refused() {
        assert!(confined_output("").is_err());
        assert!(confined_output(".").is_err());
    }

    #[test]
    fn a_nul_byte_is_refused() {
        // Rust strings admit NUL; the OS boundary does not. Rejected here so the message names the cause
        // rather than surfacing as an opaque io error.
        assert!(confined_output("app\0evil").is_err());
    }
}
