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
