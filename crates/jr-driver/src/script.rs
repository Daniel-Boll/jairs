//! Running a build script, and performing what it asked for (ADR-0195).
//!
//! # The two phases, and why they do not interleave
//!
//! The script is compiled and run to completion **first**; the targets it recorded are compiled
//! **after**. Nothing is interleaved, and that is the whole reason this design needs no message loop:
//! ADR-0153 §1 rejected a poll because a query system that memoises cannot have a compilation
//! observed halfway through, and a script that finishes before any target starts never observes one.
//!
//! It also means a script cannot react to a target's diagnostics — it gets a `bool`. That is the
//! honest cost, and 23 real Jai build scripts want nothing more from `.COMPLETE` than which way it
//! went.
//!
//! # Why the script's own compilation is an ordinary `BuildRequest`
//!
//! It is one: a program with a `main`, compiled with the operator's module paths. Sharing the path
//! means a broken script gets the diagnostics any broken program gets, at its own lines, and there is
//! no second notion of "how a file becomes runnable" to keep in step.

use std::path::PathBuf;

use jr_base::SourceMap;
use jr_db::{BackendChoice, Db as _, JairsDatabase, OptLevel, file_diagnostics};
use jr_diag::{Diagnostics, Severity};
use jr_vm::{Host, HostArg, HostValue};

use crate::build::{BuildOutcome, BuildRequest};

/// What a script asked to have compiled.
///
/// One per `Compiler.create_target`, in creation order. Every field starts at the same default the
/// `modules/Compiler` `options` procedure reports, so a script that sets nothing still describes a
/// buildable target — and the two lists of defaults are checked against each other by
/// `a_scripts_defaults_match_the_drivers`.
#[derive(Debug, Clone)]
struct Target {
    /// The script's name for it, used when reporting what was built.
    name: String,
    /// Root files to compile, in the order `add_file` was called.
    files: Vec<PathBuf>,
    /// The artefact's name, or empty for the root file's stem.
    output: String,
    /// A directory to write into, or empty for the working directory.
    output_path: String,
    /// 0 or 1.
    opt_level: i64,
    /// Whether to emit bounds checks.
    bounds_checks: bool,
    /// 0 for Cranelift, 1 for LLVM.
    backend: i64,
    /// Extra `#import` search directories.
    module_paths: Vec<PathBuf>,
    /// Extra `#system_library` search directories.
    library_paths: Vec<PathBuf>,
}

impl Target {
    fn new(name: String) -> Self {
        Self {
            name,
            files: Vec::new(),
            output: String::new(),
            output_path: String::new(),
            opt_level: 1,
            bounds_checks: true,
            backend: 0,
            module_paths: Vec::new(),
            library_paths: Vec::new(),
        }
    }
}

/// What one build-script run produced.
#[derive(Debug, Default)]
pub struct ScriptOutcome {
    /// Every artefact written, in the order the script built them.
    pub built: Vec<PathBuf>,
    /// Every message the script or a failed target produced, in order.
    pub reports: Vec<String>,
    /// `true` when every `Compiler.build` the script called succeeded.
    pub ok: bool,
}

/// The state a running script is filling in, and the compilations it triggers.
///
/// Implements [`Host`], so the VM forwards `#foreign compiler "…"` here. Holds the operator's own
/// settings because a target inherits them: a script that adds no module path still needs the ones
/// `jr build --script -I modules` was given, or `#import "Basic"` in the *target* would fail for a
/// reason the script never mentioned.
struct ScriptHost {
    targets: Vec<Target>,
    /// The module paths the operator gave, inherited by every target.
    inherited_module_paths: Vec<PathBuf>,
    /// The library paths the operator gave, inherited by every target.
    inherited_library_paths: Vec<PathBuf>,
    /// What followed `--` on the command line.
    arguments: Vec<String>,
    outcome: ScriptOutcome,
    /// Diagnostics from a failed target, kept so the caller can render them at its own colour
    /// setting rather than having them printed from in here.
    diagnostics: Vec<(Diagnostics, SourceMap)>,
}

impl ScriptHost {
    /// The target a handle names.
    ///
    /// A handle is an index plus one, so that a zeroed `Target` struct in a script — one declared and
    /// never assigned — is `0` and fails here by name instead of silently configuring the first
    /// target. That is the same off-by-one the procedure-pointer encoding uses, for the same reason.
    fn target(&mut self, handle: i64) -> Result<&mut Target, String> {
        let index =
            usize::try_from(handle - 1).map_err(|_| format!("{handle} is not a target handle"))?;
        self.targets
            .get_mut(index)
            .ok_or_else(|| format!("target {handle} was never created"))
    }
}

/// Reads one integer argument.
fn int(args: &[HostArg], at: usize) -> Result<i64, String> {
    match args.get(at) {
        Some(HostArg::Int(int)) => Ok(*int),
        Some(HostArg::Str(_)) => Err(format!("argument {at} should be an integer")),
        None => Err(format!("argument {at} is missing")),
    }
}

/// Reads one string argument.
fn text(args: &[HostArg], at: usize) -> Result<&str, String> {
    match args.get(at) {
        Some(HostArg::Str(text)) => Ok(text),
        Some(HostArg::Int(_)) => Err(format!("argument {at} should be a string")),
        None => Err(format!("argument {at} is missing")),
    }
}

impl Host for ScriptHost {
    fn call(&mut self, symbol: &str, args: &[HostArg]) -> Result<HostValue, String> {
        match symbol {
            "create_target" => {
                self.targets.push(Target::new(text(args, 0)?.to_owned()));
                // The handle is the index plus one — see [`ScriptHost::target`].
                Ok(HostValue::Int(self.targets.len() as i64))
            }
            "set_output" => {
                let value = text(args, 1)?.to_owned();
                self.target(int(args, 0)?)?.output = value;
                Ok(HostValue::Void)
            }
            "set_output_path" => {
                let value = text(args, 1)?.to_owned();
                self.target(int(args, 0)?)?.output_path = value;
                Ok(HostValue::Void)
            }
            "set_opt_level" => {
                let value = int(args, 1)?;
                self.target(int(args, 0)?)?.opt_level = value;
                Ok(HostValue::Void)
            }
            "set_bounds_checks" => {
                let value = int(args, 1)? != 0;
                self.target(int(args, 0)?)?.bounds_checks = value;
                Ok(HostValue::Void)
            }
            "set_backend" => {
                let value = int(args, 1)?;
                self.target(int(args, 0)?)?.backend = value;
                Ok(HostValue::Void)
            }
            "add_module_path" => {
                let value = PathBuf::from(text(args, 1)?);
                self.target(int(args, 0)?)?.module_paths.push(value);
                Ok(HostValue::Void)
            }
            "add_library_path" => {
                let value = PathBuf::from(text(args, 1)?);
                self.target(int(args, 0)?)?.library_paths.push(value);
                Ok(HostValue::Void)
            }
            "add_file" => {
                let value = PathBuf::from(text(args, 1)?);
                self.target(int(args, 0)?)?.files.push(value);
                Ok(HostValue::Void)
            }
            "build" => {
                let handle = int(args, 0)?;
                let target = self.target(handle)?.clone();
                Ok(HostValue::Int(i64::from(self.perform(&target))))
            }
            "report" => {
                self.outcome.reports.push(text(args, 0)?.to_owned());
                Ok(HostValue::Void)
            }
            "argument_count" => Ok(HostValue::Int(self.arguments.len() as i64)),
            "argument" => {
                let index = int(args, 0)?;
                // Out of range is empty rather than an error: a script asking for an argument it was
                // not given is ordinary, and `modules/Compiler` documents the answer.
                let text = usize::try_from(index)
                    .ok()
                    .and_then(|i| self.arguments.get(i))
                    .cloned()
                    .unwrap_or_default();
                Ok(HostValue::Str(text))
            }
            // **The *host* OS, which is the build's OS** (ADR-0180 §2). There is no `--target`, so
            // these agree; when one exists, this is the line that has to decide which the script is
            // asking about, and the answer will be the host, because a build script chooses how to
            // *run* tools.
            "target_os" => Ok(HostValue::Int(if cfg!(target_os = "linux") {
                1
            } else if cfg!(target_os = "windows") {
                2
            } else {
                0
            })),
            _ => Err(format!(
                "`{symbol}` is not a build-script procedure this compiler knows"
            )),
        }
    }
}

impl ScriptHost {
    /// Compiles one target, and records what happened.
    ///
    /// Returns whether every file in it built. A target with no files is a *failure* with a message,
    /// not a vacuous success: a script that forgot `add_file` has asked for nothing, and reporting
    /// success would make the missing artefact look like someone else's problem.
    fn perform(&mut self, target: &Target) -> bool {
        if target.files.is_empty() {
            self.outcome.reports.push(format!(
                "target `{}` has no files — call `Compiler.add_file` before `Compiler.build`",
                target.name
            ));
            self.outcome.ok = false;
            return false;
        }

        let backend = match target.backend {
            1 => BackendChoice::Llvm,
            _ => BackendChoice::Cranelift,
        };
        let opt_level = match target.opt_level {
            0 => OptLevel::Off,
            _ => OptLevel::Standard,
        };

        let mut module_paths = target.module_paths.clone();
        module_paths.extend(self.inherited_module_paths.iter().cloned());
        let mut library_paths = target.library_paths.clone();
        library_paths.extend(self.inherited_library_paths.iter().cloned());

        let mut all_ok = true;
        for file in &target.files {
            // **The default is the source's *basename*, never its path** — and getting this wrong is
            // what made the first version of this function reject `add_file("/tmp/p/main.jr")` with
            // "it is an absolute path", blaming confinement for a default the driver had chosen.
            //
            // `jr build /tmp/p/main.jr` writes `/tmp/p/main`, beside the source, and that is right for
            // a command someone typed. It is wrong here twice over: a build script's artefact belongs
            // to the *project* rather than next to whichever file happened to be a root, and the
            // script is code the operator may not have written, so the confinement below has to be
            // able to pass for the ordinary case.
            //
            // The script's `output` names one artefact, so several files in one target share it only
            // when the script did not set it — otherwise the second would overwrite the first. Two
            // artefacts want two targets, which is what `create_target` is for.
            let name = if target.output.is_empty() {
                file.file_stem()
                    .map_or_else(|| PathBuf::from("a.out"), PathBuf::from)
            } else {
                PathBuf::from(&target.output)
            };
            let output = if target.output_path.is_empty() {
                name
            } else {
                PathBuf::from(&target.output_path).join(name)
            };

            // **A declared name from a script is confined exactly as a `BUILD_OUTPUT` is**
            // (ADR-0122). The script is code the operator may not have written, so "it asked for it"
            // is not a reason to write outside the working directory — that argument is only
            // available for an explicit `-o` typed at a terminal.
            let confined = match crate::build::confined_output(&output.to_string_lossy()) {
                Ok(path) => path,
                Err(reason) => {
                    self.outcome.reports.push(format!(
                        "target `{}` asked for the output {:?}, which is not usable: {reason}",
                        target.name,
                        output.display()
                    ));
                    self.outcome.ok = false;
                    all_ok = false;
                    continue;
                }
            };

            if let Some(parent) = confined.parent()
                && !parent.as_os_str().is_empty()
                && !parent.is_dir()
            {
                // Created rather than refused: `o.output_path = "build"` is the ordinary thing a
                // script says, and making the operator `mkdir` first would be a worse tool than the
                // makefile this replaces.
                if let Err(error) = std::fs::create_dir_all(parent) {
                    self.outcome
                        .reports
                        .push(format!("cannot create {}: {error}", parent.display()));
                    self.outcome.ok = false;
                    all_ok = false;
                    continue;
                }
            }

            let request = BuildRequest {
                path: file.clone(),
                module_paths: module_paths.clone(),
                library_paths: library_paths.clone(),
                opt_level: Some(opt_level),
                bounds_checks: target.bounds_checks,
                backend,
                output: Some(confined),
                emit_object: false,
            };

            match crate::build::build(&request) {
                Ok(BuildOutcome::Built(built)) => self.outcome.built.push(built.output),
                Ok(BuildOutcome::Rejected { diagnostics, map }) => {
                    self.outcome.reports.push(format!(
                        "target `{}` did not compile {}",
                        target.name,
                        file.display()
                    ));
                    self.diagnostics.push((diagnostics, map));
                    self.outcome.ok = false;
                    all_ok = false;
                }
                Ok(BuildOutcome::Failed(message)) => {
                    self.outcome
                        .reports
                        .push(format!("target `{}`: {message}", target.name));
                    self.outcome.ok = false;
                    all_ok = false;
                }
                Err(message) => {
                    self.outcome
                        .reports
                        .push(format!("target `{}`: {message}", target.name));
                    self.outcome.ok = false;
                    all_ok = false;
                }
            }
        }
        all_ok
    }
}

/// Everything one build-script run needs.
#[derive(Debug, Clone)]
pub struct ScriptRequest {
    /// The script to compile and run.
    pub path: PathBuf,
    /// `#import` search directories, for the script **and** inherited by every target it builds.
    pub module_paths: Vec<PathBuf>,
    /// `#system_library` search directories, inherited by every target.
    pub library_paths: Vec<PathBuf>,
    /// What followed `--` on the command line, readable as `Compiler.arguments()`.
    pub arguments: Vec<String>,
}

/// How a build-script run ended.
#[derive(Debug)]
pub enum ScriptResult {
    /// The script ran. `ok` says whether every target it asked for was built.
    Ran(ScriptOutcome, Vec<(Diagnostics, SourceMap)>),
    /// The *script itself* did not check. Rendered by the caller, at the script's own lines.
    ScriptRejected {
        /// Every diagnostic from every file reachable from the script.
        diagnostics: Diagnostics,
        /// The source map to render them against.
        map: SourceMap,
    },
    /// The script was accepted and could not be run: no `main`, or a trap.
    ScriptFailed(String),
}

/// Compiles `request.path`, runs it, and performs the compilations it asked for.
///
/// # Errors
/// When the script cannot be read or registered.
pub fn run_script(request: &ScriptRequest) -> Result<ScriptResult, String> {
    let mut db = JairsDatabase::default();
    let search = db.set_module_search_paths(request.module_paths.clone());
    // The script is not the thing being optimised, and nothing it declares can change that: a
    // `BUILD_OPT_LEVEL` in a *script* would be describing the script's own compilation, which nobody
    // asked about. Standard, unconditionally, so a script's own speed is not a variable.
    let config = db.set_build_config(true, OptLevel::Standard);

    let text = std::fs::read_to_string(&request.path)
        .map_err(|e| format!("cannot read {}: {e}", request.path.display()))?;
    let key = request.path.to_string_lossy().into_owned();
    let _ = db.set_file_text(key.clone(), text);
    let root = db
        .source_file(&key)
        .ok_or_else(|| format!("internal error: {key} was not registered"))?;
    db.load_modules_transitively(root);

    let mut diagnostics = Diagnostics::new();
    for file in jr_db::reachable_files(&db, root, search) {
        diagnostics.extend(file_diagnostics(&db, file, search).iter().cloned());
    }
    if diagnostics.iter().any(|d| d.severity == Severity::Error) {
        return Ok(ScriptResult::ScriptRejected {
            diagnostics,
            map: db.source_map(),
        });
    }

    let mut host = ScriptHost {
        targets: Vec::new(),
        inherited_module_paths: request.module_paths.clone(),
        inherited_library_paths: request.library_paths.clone(),
        arguments: request.arguments.clone(),
        // **`true` until something fails.** A script that builds nothing at all succeeded at what it
        // was asked to do, which is what `jr build --script` on a script that only prints should
        // report — the alternative would make "no targets" indistinguishable from "a target failed".
        outcome: ScriptOutcome {
            built: Vec::new(),
            reports: Vec::new(),
            ok: true,
        },
        diagnostics: Vec::new(),
    };

    match jr_db::run_main_with_host(&db, root, search, config, &mut host) {
        // **A script that `exit`s non-zero has failed**, even if every target it built succeeded.
        // The status is the script's own verdict on its work, and ignoring it would make
        // `if !ok { exit(1); }` — the shape a script uses to refuse — silently mean nothing.
        Ok(jr_db::RunOutcome::Exited(status)) if status != 0 => {
            host.outcome.ok = false;
            host.outcome
                .reports
                .push(format!("the build script exited with status {status}"));
            Ok(ScriptResult::Ran(host.outcome, host.diagnostics))
        }
        Ok(jr_db::RunOutcome::Completed | jr_db::RunOutcome::Exited(_)) => {
            Ok(ScriptResult::Ran(host.outcome, host.diagnostics))
        }
        // A trap inside the script. Its own message already names the line and the frames, so it is
        // passed through rather than wrapped — a script's bug reads like any other program's.
        Ok(jr_db::RunOutcome::Failed(message)) | Err(message) => {
            Ok(ScriptResult::ScriptFailed(message))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The defaults in `modules/Compiler`'s `options` and the ones a [`Target`] starts with must
    /// agree, or a script that reads the options and writes them straight back would *change* the
    /// build — silently, and only for the fields it did not touch.
    ///
    /// Asserted by reading the module's source, because the two lists are written in different
    /// languages and no compiler checks one against the other. This is the cheapest thing that
    /// fails when someone edits one of them.
    #[test]
    fn a_scripts_defaults_match_the_drivers() {
        let module = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("the workspace root is two levels up")
            .join("modules/Compiler/module.jr");
        let source = std::fs::read_to_string(&module)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", module.display()));

        let defaults = Target::new(String::from("t"));
        for (field, written) in [
            ("output", format!("o.output = \"{}\";", defaults.output)),
            (
                "output_path",
                format!("o.output_path = \"{}\";", defaults.output_path),
            ),
            (
                "opt_level",
                format!("o.opt_level = {};", defaults.opt_level),
            ),
            (
                "bounds_checks",
                format!("o.bounds_checks = {};", defaults.bounds_checks),
            ),
        ] {
            assert!(
                source.contains(&written),
                "modules/Compiler must set the same default for `{field}` as the driver: \
                 expected to find `{written}`"
            );
        }
        // The backend is an enum on one side and an integer on the other, so it is checked by name.
        assert_eq!(
            defaults.backend, 0,
            "the driver's default backend must be the one `Backend.CRANELIFT` casts to"
        );
        assert!(
            source.contains("o.backend = Backend.CRANELIFT;"),
            "modules/Compiler must default to the backend the driver does"
        );
    }
}
