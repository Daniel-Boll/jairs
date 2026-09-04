//! Compilation orchestration: one compilation described as a **value**, so the driver can be
//! called more than once in a process.
//!
//! # Why this crate stopped being empty
//!
//! `jr build` was a `main`-shaped function reading `clap` structs: 147 lines and 25 top-level
//! statements in `jr-cli`, every one of them reading a field of `BuildArgs`. That is fine for one
//! compilation per process and impossible for two, which is exactly what a **build script** needs —
//! the script is compiled and run, and *then* the targets it asked for are compiled (ADR-0195 §3).
//!
//! So a compilation is now a [`BuildRequest`]: data, constructible by a `clap` parser or by a
//! running build script, with no argument parser and no terminal in the way.
//!
//! # Why the driver does not print
//!
//! [`build`] returns a [`BuildOutcome`] carrying diagnostics rather than rendering them. Two callers
//! want different things from the same failure — `jr build` renders to a terminal with the
//! operator's colour choice, and a build script's driver wants to say *which target* failed before
//! the diagnostics — and a crate that owns the rendering can serve only the first. `jr-cli` keeps
//! `emit_diagnostics`, which is where the colour resolution and the `SourceMap` already live.
//!
//! The alternative was passing a renderer in. Rejected: it makes every caller supply a terminal
//! concept to ask a question about a file, and the build-script driver has no terminal to supply.
//!
//! # What is deliberately still in `jr-cli`
//!
//! Flag *precedence* — `-o` beating a declared `BUILD_OUTPUT`, `-O` beating `BUILD_OPT_LEVEL`
//! (ADR-0102 §2) — is resolved by the caller before it builds a request, because "the operator
//! outranks the artefact" is a statement about a command line and this crate cannot see one. The
//! request carries the *decided* values, so a reader of `build` never has to reconstruct which
//! source won.

mod build;
mod script;

pub use build::{BuildOutcome, BuildRequest, Built, build};
pub use script::{ScriptOutcome, ScriptRequest, ScriptResult, is_build_script, run_script};
