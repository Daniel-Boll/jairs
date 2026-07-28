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
    /// The `.jr` file to measure requests against.
    ///
    /// One file rather than a set: the measurement is per-request latency, and the question
    /// ADR-0033 exists to answer is what one request costs.
    pub file: std::path::PathBuf,

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
