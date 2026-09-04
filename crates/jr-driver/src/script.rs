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

/// A command a script is assembling, and what it produced when run.
///
/// # Why the *driver* spawns rather than `modules/Process`
///
/// `Process.run` works in a native binary and fails under `jr run`: the VM translates a pointer
/// argument one level deep, so `execvp`'s `argv` — an array of pointers — arrives with a real address
/// for the array and region-relative garbage for every string in it (ADR-0158 §3). Measured: exit
/// code 127 while reporting success.
///
/// Fixing that inside the VM needs information no *type* carries. `char **` is `argv` here and
/// `strtod`'s out-parameter there, and the second one **works** — the callee writes a pointer rather
/// than reading one — so a rule keyed on "the pointee contains a pointer" would break working code in
/// order to describe broken code. `modules/Process` records that trade and rejects it.
///
/// So the knowledge stays where it actually is: at the call site, which says "run this list". The
/// driver spawns with ordinary Rust strings and there is nothing to marshal. That leaves the VM's
/// defect standing for a general program, which is honest — it is a separate fix with a separate
/// argument, and a build script does not have to wait for it.
#[derive(Default)]
struct Command {
    /// The program and its arguments, in order. The first word is the program.
    words: Vec<String>,
    /// Everything the command wrote to standard output, after it ran.
    output: String,
}

/// What one build-script run produced.
#[derive(Debug, Default, Clone)]
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
struct ScriptState {
    /// Whether the script has reached the driver at all (ADR-0196 §9).
    ///
    /// The signal for "a `#run` did the work", and it is *any* call rather than "a target was built": a
    /// script that shells out, prints and declares nothing has done exactly what it was written to do,
    /// and keying on artefacts reported `the file declares no main` for it — an error about a missing
    /// `main` on a file that was never going to have one.
    touched: bool,
    targets: Vec<Target>,
    /// Targets a `#run` asked to have built once the script has finished (ADR-0196 §8).
    ///
    /// Handles rather than clones, so a later `set_options` on the same target still applies: a script
    /// may reasonably declare a target early and configure it as it learns things.
    pending: Vec<i64>,
    /// Commands the script has assembled, in creation order.
    commands: Vec<Command>,
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

impl ScriptState {
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

    /// The command a handle names, with the same plus-one discipline as [`Self::target`].
    fn command(&mut self, handle: i64) -> Result<&mut Command, String> {
        let index =
            usize::try_from(handle - 1).map_err(|_| format!("{handle} is not a command handle"))?;
        self.commands
            .get_mut(index)
            .ok_or_else(|| format!("command {handle} was never created"))
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

/// The [`Host`] the VM sees, sharing one [`ScriptState`] with the driver (ADR-0196 §7).
///
/// # Why a shared handle rather than the state itself
///
/// A `#run` is evaluated inside a salsa query, so the host has to be *installed* rather than passed —
/// which means the driver gives it away and needs it back to find out what was recorded. Handing over a
/// `Box<dyn Host>` and downcasting it later would need `Any` on the trait, so every implementor would
/// carry a method that exists for one caller's bookkeeping.
///
/// An `Rc` costs nothing here: the ambient host is thread-local, so there is no `Send` to satisfy, and
/// the driver simply keeps a clone.
///
/// The `RefCell` cannot be re-entered even though the work inside it re-enters the VM —
/// `Compiler.build` compiles a target, whose own `#run`s evaluate — because `with_ambient_host` takes
/// the host *out* of the slot for the duration of a call. A target's `#run` therefore finds no host and
/// is refused, which is right: it is a different program, and it was not asked to build anything.
struct ScriptHost {
    state: std::rc::Rc<std::cell::RefCell<ScriptState>>,
    /// Whether a `Compiler.build` may compile **now**, or must be deferred (ADR-0196 §8).
    ///
    /// `true` for a `main`-shaped script, which the driver runs itself: there is no query in the way, so
    /// a compilation can start immediately and the script gets a `bool` back.
    ///
    /// `false` for a `#run`, and the reason is not a preference — **salsa forbids it.** A compilation
    /// needs its own `JairsDatabase`, and creating one while another database's query is executing
    /// panics with "Cannot change database mid-query". Measured, and it is what makes Jai's own shape
    /// work the way it does: a `build.jai` *declares* workspaces and the compiler builds them once the
    /// metaprogram returns.
    immediate: bool,
}

impl Host for ScriptHost {
    fn call(&mut self, symbol: &str, args: &[HostArg]) -> Result<HostValue, String> {
        self.state.borrow_mut().call(symbol, args, self.immediate)
    }
}

impl ScriptState {
    fn call(
        &mut self,
        symbol: &str,
        args: &[HostArg],
        immediate: bool,
    ) -> Result<HostValue, String> {
        self.touched = true;
        match symbol {
            "create_target" => {
                self.targets.push(Target::new(text(args, 0)?.to_owned()));
                // The handle is the index plus one — see [`ScriptState::target`].
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
                if !immediate {
                    // **Refused rather than deferred-and-reported-as-success** (ADR-0196 §8). A `bool`
                    // that means "queued" is indistinguishable from one that means "built", so a script
                    // checking it would branch on nothing. The alternative surface is right here in the
                    // message, and it returns nothing precisely because there is nothing to know yet.
                    return Err(String::from(
                        "`Compiler.build` compiles immediately, which a `#run` cannot do: the                          compilation needs its own database and one is already open. Use                          `Compiler.request_build`, which asks the driver to build the target once the                          script has finished",
                    ));
                }
                let target = self.target(handle)?.clone();
                Ok(HostValue::Int(i64::from(self.perform(&target))))
            }
            "request_build" => {
                let handle = int(args, 0)?;
                // Validated now so a bad handle is reported at the call rather than after the script has
                // finished, when there is nothing left to point at.
                let _ = self.target(handle)?;
                self.pending.push(handle);
                Ok(HostValue::Void)
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
            "command_begin" => {
                self.commands.push(Command::default());
                Ok(HostValue::Int(self.commands.len() as i64))
            }
            "command_arg" => {
                let value = text(args, 1)?.to_owned();
                self.command(int(args, 0)?)?.words.push(value);
                Ok(HostValue::Void)
            }
            "command_run" => {
                let handle = int(args, 0)?;
                let words = self.command(handle)?.words.clone();
                let (status, captured) = spawn(&words);
                self.command(handle)?.output = captured;
                Ok(HostValue::Int(status))
            }
            "command_output" => {
                let handle = int(args, 0)?;
                Ok(HostValue::Str(self.command(handle)?.output.clone()))
            }
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

impl ScriptState {
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

/// Runs one command, returning its exit status and its captured standard output.
///
/// Standard error is **inherited**, not captured: it is the channel a tool uses to explain itself, so
/// it goes to the terminal where the person running the build can read it. Capturing it would make a
/// build script responsible for printing a compiler's error message, and most would forget.
///
/// A command that could not be started is a non-zero status with the reason on stdout rather than an
/// error, because "the tool is not installed" is a thing a build script should be able to handle — and
/// a script that wants to stop can read the status and `exit`.
fn spawn(words: &[String]) -> (i64, String) {
    let Some((program, arguments)) = words.split_first() else {
        return (1, String::from("no program was named"));
    };
    match std::process::Command::new(program)
        .args(arguments)
        .stderr(std::process::Stdio::inherit())
        .output()
    {
        Ok(output) => {
            // **A signal is not exit code 0.** `ExitStatus::code()` is `None` for a process killed by
            // one, and `unwrap_or(0)` would report a crashed tool as a success — which is how a build
            // "succeeds" having produced nothing.
            let status = output.status.code().map_or(-1, i64::from);
            (status, String::from_utf8_lossy(&output.stdout).into_owned())
        }
        Err(error) => (1, format!("cannot run `{program}`: {error}")),
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

/// The module a build script imports, and the one thing that makes a file a build script.
///
/// A *name* rather than a shape — "declares `build` and no `main`" was the other candidate — because
/// this one is already load-bearing: importing it is what gives a file the driver's vocabulary, so a
/// file that imports it and is compiled as a program is refused (see `build.rs`). Detecting on the
/// same fact means the flag and the refusal cannot disagree about what a build script is.
const COMPILER_MODULE: &str = "Compiler";

/// Whether `path` is a build script, by whether it imports `modules/Compiler`.
///
/// Reads the file's **own** import list, which needs one parse and one lowering and **no module
/// loading at all** — `imports_of` asks for `file_hir` and nothing else. So `jr build` on an ordinary
/// program pays a parse of one file to learn it is not a script, rather than a second module tree.
///
/// A file that reaches `modules/Compiler` *indirectly* — through a helper module of its own — is not
/// detected, and gets the refusal naming `--script` instead. That is the honest boundary of a cheap
/// check, and the case is rare enough that paying for a transitive walk on every ordinary build would
/// be the wrong trade.
///
/// # Errors
/// When the file cannot be read or registered.
pub fn is_build_script(path: &std::path::Path) -> Result<bool, String> {
    let mut db = JairsDatabase::default();
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let key = path.to_string_lossy().into_owned();
    let _ = db.set_file_text(key.clone(), text);
    let root = db
        .source_file(&key)
        .ok_or_else(|| format!("internal error: {key} was not registered"))?;
    Ok(jr_db::imports_of(&db, root)
        .iter()
        .any(|name| name.as_ref() == COMPILER_MODULE))
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

    let state = std::rc::Rc::new(std::cell::RefCell::new(ScriptState {
        touched: false,
        targets: Vec::new(),
        pending: Vec::new(),
        commands: Vec::new(),
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
    }));

    // **Installed before the file is checked, because a `#run` runs during checking** (ADR-0196 §7).
    // That is the whole of what makes `#run build();` work: `file_consts` evaluates the directive while
    // gathering diagnostics, its `Compiler` calls find this host ambiently, and by the time the gate
    // below has an answer the requests are already recorded.
    //
    // The database is fresh — created five lines up — so each `file_consts` runs exactly once and each
    // request is recorded exactly once. That is the contract `install_ambient_host` documents, and it is
    // a property of this function rather than an assumption about salsa.
    let previous = jr_vm::install_ambient_host(Box::new(ScriptHost {
        state: state.clone(),
        immediate: false,
    }));

    let mut diagnostics = Diagnostics::new();
    for file in jr_db::reachable_files(&db, root, search) {
        diagnostics.extend(file_diagnostics(&db, file, search).iter().cloned());
    }

    // What a `#run` printed, emitted here for the reason `jr-driver`'s `build` emits it: the output is
    // carried out of the query rather than written from inside one, so that a memoised evaluation cannot
    // print on one build and not the next.
    let printed = jr_db::comptime_output(&db, root, search);
    if !printed.is_empty() {
        eprint!("{}", String::from_utf8_lossy(&printed));
    }

    if diagnostics.iter().any(|d| d.severity == Severity::Error) {
        restore_ambient(previous);
        return Ok(ScriptResult::ScriptRejected {
            diagnostics,
            map: db.source_map(),
        });
    }

    // **A `#run` script has already done its work**, so there is nothing left to run. Detected by asking
    // the host what it recorded rather than by inspecting the file for a `main`: a script may legitimately
    // have both — a `#run` that configures and a `main` that does the rest — and the question here is
    // "has anything been asked for yet", which only the host can answer.
    // **The deferred builds, now that no query is open** (ADR-0196 §8). This is the moment Jai's model
    // describes: the metaprogram has finished, so the compiler builds what it declared. Performed before
    // `main` is considered, so a script with both spellings sees its `#run`'s targets already built.
    {
        let pending = std::mem::take(&mut state.borrow_mut().pending);
        for handle in pending {
            let target = match state.borrow_mut().target(handle) {
                Ok(target) => target.clone(),
                Err(reason) => {
                    state.borrow_mut().outcome.reports.push(reason);
                    state.borrow_mut().outcome.ok = false;
                    continue;
                }
            };
            // The borrow is released before `perform`, which compiles and may itself reach the VM.
            let performed = state.borrow_mut().perform(&target);
            let _ = performed;
        }
    }

    let already_worked = state.borrow().touched;

    // Taken back so the `main` path below owns it, and so a *target's* `#run` cannot reach it.
    let _ = jr_vm::take_ambient_host();

    if already_worked && jr_db::main_of(&db, root).is_none() {
        restore_ambient(previous);
        let mut borrowed = state.borrow_mut();
        return Ok(ScriptResult::Ran(
            borrowed.outcome.clone(),
            std::mem::take(&mut borrowed.diagnostics),
        ));
    }

    let mut host = ScriptHost {
        state: state.clone(),
        immediate: true,
    };

    // A `main`-shaped script, which is the other spelling and needs the host explicitly: the driver owns
    // this VM, so there is no query in the way.
    let outcome = jr_db::run_main_with_host(&db, root, search, config, &mut host);
    restore_ambient(previous);
    match outcome {
        // **A script that `exit`s non-zero has failed**, even if every target it built succeeded.
        // The status is the script's own verdict on its work, and ignoring it would make
        // `if !ok { exit(1); }` — the shape a script uses to refuse — silently mean nothing.
        Ok(jr_db::RunOutcome::Exited(status)) if status != 0 => {
            let mut borrowed = state.borrow_mut();
            borrowed.outcome.ok = false;
            borrowed
                .outcome
                .reports
                .push(format!("the build script exited with status {status}"));
            Ok(ScriptResult::Ran(
                borrowed.outcome.clone(),
                std::mem::take(&mut borrowed.diagnostics),
            ))
        }
        Ok(jr_db::RunOutcome::Completed | jr_db::RunOutcome::Exited(_)) => {
            let mut borrowed = state.borrow_mut();
            Ok(ScriptResult::Ran(
                borrowed.outcome.clone(),
                std::mem::take(&mut borrowed.diagnostics),
            ))
        }
        // A script with no `main` that already did its work in a `#run` is not a failure — `no main` is
        // the expected answer for Jai's own spelling, `#run build();`.
        Ok(jr_db::RunOutcome::Failed(message)) | Err(message) => {
            if already_worked {
                let mut borrowed = state.borrow_mut();
                Ok(ScriptResult::Ran(
                    borrowed.outcome.clone(),
                    std::mem::take(&mut borrowed.diagnostics),
                ))
            } else {
                Ok(ScriptResult::ScriptFailed(message))
            }
        }
    }
}

/// Puts back whatever host was installed before this build, if any.
///
/// Nested builds are real: `Compiler.build` compiles a target, and that target's own `#run`s evaluate
/// inside it. Restoring rather than clearing means an outer script's host survives an inner build —
/// without which a second `Compiler.build` in one script would find no host at all.
fn restore_ambient(previous: Option<Box<dyn Host>>) {
    match previous {
        Some(host) => {
            let _ = jr_vm::install_ambient_host(host);
        }
        None => {
            let _ = jr_vm::take_ambient_host();
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
