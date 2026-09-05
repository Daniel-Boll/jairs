//! Project discovery: what a command operates on when the command line does not say.
//!
//! # Why this is one module and not six call sites
//!
//! Five of the seven subcommands build a module search path, and three of them take an entry
//! point. Before this module each did it inline, which was fine while the answer was "whatever
//! the user typed, then the bundled directory". Once a `jairs.toml` can also contribute, an
//! inline answer per command is six chances to disagree about precedence — and precedence is
//! exactly the kind of thing a user diagnoses by reading one command's behaviour and assuming it
//! generalises.
//!
//! # Precedence, stated once
//!
//! Most specific first, in both cases:
//!
//! 1. **The command line.** A flag is an instruction from the operator.
//! 2. **The manifest.** A file is a default the project chose.
//! 3. **The bundled standard library.** Always last, so a project can shadow it (ADR-0014 §1).
//!
//! That is ADR-0102 §2's asymmetry between `-O` and a declared `BUILD_OPT_LEVEL`, applied to
//! paths: the operator outranks the artifact.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

/// The manifest governing the working directory, if there is one.
///
/// # Errors
///
/// When a manifest exists but cannot be read or parsed. A broken manifest is reported rather
/// than skipped: silently falling back to the defaults would make an editing mistake look like
/// the tool ignoring the file.
pub fn find() -> Result<Option<jr_manifest::Located>> {
    let cwd = std::env::current_dir().context("the working directory must be readable")?;
    jr_manifest::find(&cwd).map_err(Into::into)
}

/// The manifest governing `path`, if there is one.
///
/// Separate from [`find`] because `jr fmt` formats files that may live outside the working
/// directory's project, and the file's own project is the one whose style applies.
///
/// # Errors
///
/// As [`find`].
pub fn find_for(path: &Path) -> Result<Option<jr_manifest::Located>> {
    jr_manifest::find(path).map_err(Into::into)
}

/// The file a command should compile, given whatever the command line supplied.
///
/// # Errors
///
/// When no path was given and no manifest declares an entry point, or when the manifest's entry
/// point does not exist. The second case is an error rather than a fallback because a manifest
/// naming a file that is not there is a mistake in the manifest, and compiling something else
/// instead would hide it.
pub fn entry(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path);
    }

    let Some(located) = find()? else {
        bail!(
            "no input file, and no `{}` in this directory or any parent\n\
             \x20 give a path, or run `jr new <name>` to start a project",
            jr_manifest::FILE_NAME
        );
    };

    let entry = located.entry();
    if !entry.exists() {
        bail!(
            "{} names `{}` as this project's entry point, but that file does not exist",
            located.root.join(jr_manifest::FILE_NAME).display(),
            entry.display()
        );
    }
    Ok(entry)
}

/// The module search paths a command should use, given whatever `-I` supplied.
///
/// # Errors
///
/// As [`find`].
pub fn module_search_paths(explicit: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut paths = explicit.to_vec();
    if let Some(located) = find()? {
        paths.extend(located.module_paths());
    }
    // Last, so anything above shadows it (ADR-0014 §1).
    paths.push(crate::commands::check::bundled_module_dir());
    Ok(paths)
}

/// The formatter configuration that applies to `path`.
///
/// # Errors
///
/// As [`find`].
pub fn fmt_config(path: &Path) -> Result<jr_fmt::Config> {
    Ok(find_for(path)?
        .map(|located| located.fmt_config())
        .unwrap_or_default())
}

/// The artefact name the project's manifest implies, if any.
///
/// `<project root>/<name>` — beside the manifest rather than beside the source file, because a
/// build output belongs to the project and `src/main` is a path nobody asked for. Returns `None`
/// outside a project, or inside one whose manifest gives no name, which leaves the driver's own
/// fallback (the root file's stem) in place.
///
/// # Errors
///
/// As [`find`].
pub fn default_output() -> Result<Option<PathBuf>> {
    Ok(find()?.and_then(|located| {
        located
            .manifest
            .project
            .name
            .as_ref()
            .map(|name| located.root.join(name))
    }))
}
