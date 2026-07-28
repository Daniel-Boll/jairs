//! Entry point for the `jr` binary.

use std::process;

use anyhow::Result;
use clap::Parser as _;
use jr_cli::cli::{Cli, Command};

fn main() {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => process::exit(code),
        Err(e) => {
            eprintln!("jr: error: {e:#}");
            process::exit(3);
        }
    }
}

fn run(cli: Cli) -> Result<i32> {
    match cli.command {
        Command::Bench(args) => jr_cli::commands::bench::run(args, &cli.global),
        Command::Build(args) => jr_cli::commands::build::run(args, &cli.global),
        Command::Check(args) => jr_cli::commands::check::run(args, &cli.global),
        Command::Fmt(args) => jr_cli::commands::fmt::run(args, &cli.global),
        Command::Lsp(args) => jr_cli::commands::lsp::run(args, &cli.global),
        Command::Run(args) => jr_cli::commands::run::run(args, &cli.global),
        Command::Parse(args) => jr_cli::commands::parse::run(args, &cli.global),
    }
}
