//! Clap command-line types for the `jr` driver.

use clap::{Args, Parser, Subcommand};

/// The Jairs compiler driver.
///
/// Exit codes:
///   0  success
///   1  diagnostics / check failure
///   2  usage error
///   3  I/O error
#[derive(Debug, Parser)]
#[command(
    name = "jr",
    version,
    about = "The Jairs compiler driver",
    long_about = None,
)]
pub struct Cli {
    /// Global flags shared by all subcommands.
    #[command(flatten)]
    pub global: GlobalArgs,

    /// The subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// Global flags shared by all subcommands.
#[derive(Debug, Args)]
pub struct GlobalArgs {
    /// Colour output: auto (default), always, or never.
    #[arg(long, global = true, default_value = "auto", value_name = "WHEN")]
    pub color: ColorChoice,

    /// Suppress informational output.
    #[arg(short, long, global = true)]
    pub quiet: bool,

    /// Increase informational output.
    #[arg(short, long, global = true)]
    pub verbose: bool,
}

/// When to emit ANSI colour codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ColorChoice {
    /// Emit colour only when stderr is a terminal.
    Auto,
    /// Always emit colour.
    Always,
    /// Never emit colour.
    Never,
}

impl ColorChoice {
    /// Resolve `auto` by checking whether stderr is a TTY.
    #[must_use]
    pub fn resolve(self) -> bool {
        match self {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => {
                // Use `std::io::IsTerminal` (stable since 1.70).
                use std::io::IsTerminal as _;
                std::io::stderr().is_terminal()
            }
        }
    }
}

/// Available subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Parse one or more `.jr` files and report diagnostics.
    ///
    /// Accepts files and directories (directories are expanded to `**/*.jr`).
    /// Exits 0 if no errors, 1 if any errors were found.
    Check(CheckArgs),

    /// Format `.jr` source files.
    ///
    /// By default, rewrites files in place and prints the names of changed
    /// files.  Use `--check` for CI (exits 1 if any file is not formatted).
    /// Use `--stdin` for editor integration (reads stdin, writes stdout).
    Fmt(FmtArgs),

    /// Run a `.jr` program in the bytecode VM.
    ///
    /// Checks the file first and refuses to run one with errors (ADR-0017 §4), then
    /// calls its `main`. Exits 0 on completion, 1 if the file has errors, 4 if the
    /// program trapped, or with the program's own status if it called `exit`.
    Run(RunArgs),

    /// Compile a `.jr` program to a native executable.
    ///
    /// Checks the file first and refuses to build one with errors (ADR-0017 §4), then
    /// compiles every reachable file through Cranelift and links the result. Exits 0
    /// on success, 1 if the file has errors, and 2 if code generation or linking
    /// failed.
    Build(BuildArgs),

    /// Run the language server over stdin and stdout.
    ///
    /// Speaks LSP 3.17 and provides exactly what `PLAN.md` §1.4 asks for: diagnostics,
    /// hover and goto-definition. Point an editor at `jr lsp`; anything richer is wave
    /// W9's (`PLAN.md` §2.1).
    Lsp(LspArgs),

    /// Measure how long language-server requests take on a file.
    ///
    /// Prints min / median / p95 for each operation in three cache states: cold (a fresh
    /// database), warm (memoized), and after a whitespace-only edit. **Reports, never
    /// judges** — there is no threshold and no failure (ADR-0033 §4). This is not one of
    /// the six gates.
    Bench(BenchArgs),

    /// Debug aid: parse a single file and display its structure.
    ///
    /// Without flags, prints a summary (token count, node count, diagnostic
    /// count).  `--dump` prints the full CST; `--tokens` prints one token per
    /// line.
    Parse(ParseArgs),
}

/// The `--opt-level` values, as the command line spells them (ADR-0142 §2).
///
/// A separate type from [`jr_db::OptLevel`] because `clap::ValueEnum` is a foreign trait
/// and `jr_db::OptLevel` a foreign type, so `jr-cli` cannot implement one for the other —
/// and `jr-db` must not depend on `clap` to hand the driver a value type. One `From`
/// bridges them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum OptLevelArg {
    /// `-O0`: no mid-end pass runs.
    #[value(name = "0")]
    Off,
    /// `-O1`: the pipeline. The default, and what every build did before the flag.
    #[value(name = "1")]
    #[default]
    Standard,
}

impl From<OptLevelArg> for jr_db::OptLevel {
    fn from(arg: OptLevelArg) -> Self {
        match arg {
            OptLevelArg::Off => Self::Off,
            OptLevelArg::Standard => Self::Standard,
        }
    }
}

/// Which code generator `jr build` uses (ADR-0143 §2).
///
/// A separate type from [`jr_db::BackendChoice`] for the reason [`OptLevelArg`] is separate
/// from `jr_db::OptLevel`: `clap::ValueEnum` is a foreign trait and that a foreign type.
///
/// **Both values exist even in a build with no LLVM support.** A flag that appeared and
/// disappeared with a compile-time feature would make "unknown argument" the diagnostic for a
/// missing capability, which tells a reader the wrong thing; instead the driver refuses with a
/// message naming the feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum BackendArg {
    /// Cranelift, the verified back end and the default (ADR-0009).
    #[default]
    Cranelift,
    /// LLVM through `inkwell`, when the compiler was built with `--features llvm`.
    Llvm,
}

impl From<BackendArg> for jr_db::BackendChoice {
    fn from(arg: BackendArg) -> Self {
        match arg {
            BackendArg::Cranelift => Self::Cranelift,
            BackendArg::Llvm => Self::Llvm,
        }
    }
}

/// Arguments for `jr check`.
#[derive(Debug, Args)]
pub struct CheckArgs {
    /// Files or directories to check (directories expand to `**/*.jr`).
    #[arg(required = true, value_name = "PATH")]
    pub paths: Vec<std::path::PathBuf>,

    /// Directory to search for imported modules. May be repeated; searched in
    /// the order given, before the bundled module directory (ADR-0014).
    #[arg(short = 'I', long = "module-path", value_name = "DIR")]
    pub module_paths: Vec<std::path::PathBuf>,
}

/// Arguments for `jr run`.
#[derive(Debug, Args)]
pub struct RunArgs {
    /// The program to run. Must declare `main`.
    #[arg(value_name = "PATH")]
    pub path: std::path::PathBuf,

    /// Compile without array bounds checks (ADR-0003, ADR-0058 §1).
    ///
    /// Every `bounds_check` operation is stripped from the MIR the back end receives, so an
    /// out-of-range index reads or writes whatever is at that address. That is the trade the
    /// flag exists to offer, and it is undefined behaviour by construction.
    ///
    /// Two things it does **not** change. A `#no_abc` procedure has no checks either way, because
    /// that is a property of the code rather than of the build (ADR-0058 §3). And compile-time
    /// execution always checks, because a trap there is a *diagnostic* rather than a program
    /// behaviour — so `#run f(9)` on an eight-element array is still an error (ADR-0058 §4).
    #[arg(long = "no-bounds-check")]
    pub no_bounds_check: bool,

    /// How much the mid-end may optimise before the back end sees the code (ADR-0142 §1).
    ///
    /// `1`, the default, runs the pipeline: inline, forward stores, const-prop, DCE.
    /// `0` runs none of them, so the code executed is exactly what lowering produced.
    ///
    /// A level may not change what a program computes — ADR-0002 makes a trap a fact
    /// about the program rather than about the build — and the differential harness sweeps
    /// the corpus at both levels to check it. The one thing `0` does change is a
    /// backtrace: nothing is inlined, so a trap inside a leaf names the leaf's own line
    /// and lists its frame, where `1` names the call site (ADR-0021 §3).
    #[arg(short = 'O', long = "opt-level", value_enum, default_value_t)]
    pub opt_level: OptLevelArg,

    /// Directory to search for imported modules. May be repeated; searched in
    /// the order given, before the bundled module directory (ADR-0014).
    #[arg(short = 'I', long = "module-path", value_name = "DIR")]
    pub module_paths: Vec<std::path::PathBuf>,
}

/// Arguments for `jr build`.
#[derive(Debug, Args)]
pub struct BuildArgs {
    /// The program to compile. Must declare `main`.
    #[arg(value_name = "PATH")]
    pub path: std::path::PathBuf,

    /// Where to write the executable. Defaults to the input's name without its
    /// extension.
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    pub output: Option<std::path::PathBuf>,

    /// Write the object file and skip linking.
    ///
    /// Useful when the failure is in code generation rather than in the link, and
    /// when there is no C driver to link with.
    #[arg(long = "emit-object")]
    pub emit_object: bool,

    /// Compile without array bounds checks (ADR-0003, ADR-0058 §1).
    ///
    /// Every `bounds_check` operation is stripped from the MIR the back end receives, so an
    /// out-of-range index reads or writes whatever is at that address. That is the trade the
    /// flag exists to offer, and it is undefined behaviour by construction.
    ///
    /// Two things it does **not** change. A `#no_abc` procedure has no checks either way, because
    /// that is a property of the code rather than of the build (ADR-0058 §3). And compile-time
    /// execution always checks, because a trap there is a *diagnostic* rather than a program
    /// behaviour — so `#run f(9)` on an eight-element array is still an error (ADR-0058 §4).
    #[arg(long = "no-bounds-check")]
    pub no_bounds_check: bool,

    /// How much the mid-end may optimise before the back end sees the code (ADR-0142 §1).
    ///
    /// `1`, the default, runs the pipeline: inline, forward stores, const-prop, DCE.
    /// `0` runs none of them, so the compiled code is exactly what lowering produced —
    /// which is what makes a wrong answer attributable to lowering rather than to a pass.
    ///
    /// A level may not change what a program computes (ADR-0002, ADR-0142 §3). The one
    /// thing `0` does change is a backtrace: nothing is inlined, so a trap inside a leaf
    /// names the leaf's own line and lists its frame (ADR-0021 §3).
    #[arg(short = 'O', long = "opt-level", value_enum, default_value_t)]
    pub opt_level: OptLevelArg,

    /// Which code generator to use (ADR-0143 §2).
    ///
    /// `cranelift` is the default and the verified one. `llvm` needs a compiler built with
    /// `--features llvm`; without it this refuses and says so, rather than silently using the
    /// other back end.
    ///
    /// The choice does **not** change what the program computes — the three engines are held
    /// to agreement by the differential harness — and it does not change the optimisation
    /// level, which selects how much the mid-end rewrites MIR (ADR-0142) and reaches no back
    /// end.
    #[arg(long = "backend", value_enum, default_value_t)]
    pub backend: BackendArg,

    /// Directory to search for imported modules. May be repeated; searched in
    /// the order given, before the bundled module directory (ADR-0014).
    #[arg(short = 'I', long = "module-path", value_name = "DIR")]
    pub module_paths: Vec<std::path::PathBuf>,
}

/// Arguments for `jr fmt`.
#[derive(Debug, Args)]
pub struct FmtArgs {
    /// Check formatting without modifying files; exit 1 if any file differs.
    #[arg(long)]
    pub check: bool,

    /// Read from stdin and write formatted output to stdout.
    #[arg(long)]
    pub stdin: bool,

    /// Files or directories to format (ignored when `--stdin` is given).
    #[arg(value_name = "PATH")]
    pub paths: Vec<std::path::PathBuf>,
}

/// Arguments for `jr parse`.
#[derive(Debug, Args)]
pub struct ParseArgs {
    /// The file to parse.
    #[arg(value_name = "PATH")]
    pub path: std::path::PathBuf,

    /// Print the full CST dump.
    #[arg(long)]
    pub dump: bool,

    /// Print one token per line as `KIND range "text"`.
    #[arg(long)]
    pub tokens: bool,
}

/// Arguments for `jr lsp`.
#[derive(Debug, Args)]
pub struct LspArgs {
    /// Directory to search for `#import`ed modules. Repeatable.
    ///
    /// Supplied rather than discovered, for the same reason `jr check --module-path` is:
    /// guessing a search path silently changes which module a program means.
    #[arg(long = "module-path", value_name = "DIR")]
    pub module_path: Vec<std::path::PathBuf>,
}

/// Arguments for `jr bench`.
#[derive(Debug, Args)]
pub struct BenchArgs {
    /// The `.jr` file to measure requests against, or the paths to measure throughput over.
    ///
    /// One file rather than a set for the latency measurement: that is per-request, and the
    /// question ADR-0033 exists to answer is what one request costs. With `--throughput` the
    /// remaining paths join it, and a directory expands to `**/*.jr` as `jr check`'s do.
    pub file: std::path::PathBuf,

    /// Further paths, used only with `--throughput` (ADR-0146 §1).
    #[arg(value_name = "PATH")]
    pub paths: Vec<std::path::PathBuf>,

    /// Measure **compile throughput** over the given paths instead of request latency
    /// (ADR-0146 §1).
    ///
    /// Reports lines and bytes per second for `check` (every diagnostic for every reachable
    /// file) and for `build` (through MIR and a back end into an object, excluding the link,
    /// which is `cc` rather than this compiler).
    ///
    /// **Cold only**, unlike the latency measurement's three regimes: a compiler is a
    /// process, so the number a user experiences is always the cold one and a warm figure
    /// would be measuring a memo table.
    ///
    /// Reports, never judges — there is no threshold and no failure (ADR-0033 §4).
    #[arg(long = "throughput")]
    pub throughput: bool,

    /// How many times to run each operation in each cache state.
    ///
    /// Twenty by default, which is enough for a median to mean something and small enough
    /// that a cold run — a fresh database per iteration — finishes promptly.
    #[arg(long, default_value_t = 20, value_name = "N")]
    pub iterations: usize,

    /// Extra directories to search for `#import`ed modules.
    ///
    /// Same meaning as `jr check --module-path`, and passed for the same reason: guessing a
    /// search path silently changes which module a program means.
    #[arg(long = "module-path", value_name = "DIR")]
    pub module_paths: Vec<std::path::PathBuf>,
}
