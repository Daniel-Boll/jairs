//! The `jr` command-line driver library.
//!
//! This crate is structured as a library with a thin `main.rs` entry point so
//! that integration tests can call command functions directly without spawning
//! a subprocess.
//!
//! Exit codes:
//! - `0` — success
//! - `1` — diagnostics / check failure
//! - `2` — usage error (handled by clap)
//! - `3` — I/O error

pub mod cli;
pub mod commands;
pub mod files;
pub mod report;
