//! Project discovery: what a command operates on when the command line does not say.
//!
//! # Why this is one module and not six call sites
//!
//! Five of the seven subcommands need a project module catalog, and three of them take an entry
//! point. Before this module each assembled its own search paths, which was fine while the answer
//! was "whatever the user typed, then the bundled directory". Once a `jairs.toml` contributes
//! implicit local modules and exact dependencies, an inline answer per command is six chances to
//! disagree about project identity and precedence.
//!
//! # Precedence, stated once
//!
//! Most specific first, in both cases:
//!
//! 1. **The command line.** A flag is an instruction from the operator.
//! 2. **The manifest.** Local `src` modules, exact dependencies, and legacy module roots are
//!    declarations the project chose.
//! 3. **The bundled standard library.** An implicit local or legacy root cannot shadow it;
//!    an exact dependency can (ADR-0213 §4).
//!
//! That is ADR-0102 §2's asymmetry between `-O` and a declared `BUILD_OPT_LEVEL`, applied to
//! paths: the operator outranks the artifact.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

/// Discovers the project context governing `anchor`.
///
/// Command-line module paths are resolved against the process working directory, because that is
/// where an operator typed them. Manifest paths are resolved by `jr-project` against the manifest
/// itself. Keeping those bases distinct prevents `jr check subdir/main.jr -I vendor` from silently
/// interpreting `vendor` as `subdir/vendor`.
///
/// # Errors
///
/// When the working directory is unreadable or project discovery finds a malformed manifest,
/// unreadable source, or ambiguous module declaration.
pub fn context_for(
    anchor: &Path,
    explicit_module_paths: &[PathBuf],
) -> Result<jr_project::ProjectContext> {
    let cwd = std::env::current_dir().context("the working directory must be readable")?;
    let roots = explicit_module_paths.iter().map(|path| {
        if path.is_absolute() {
            path.clone()
        } else {
            cwd.join(path)
        }
    });
    jr_project::ProjectContext::discover(
        jr_project::DiscoverRequest::new(anchor).with_operator_roots(roots),
    )
    .map_err(Into::into)
}

/// Resolves a command's entry point and discovers the context that governs it.
///
/// An explicit file is governed by its own nearest manifest. With no file, discovery starts from
/// the working directory and the manifest supplies the entry. This removes the old split where the
/// entry could come from one project while module discovery came from another.
///
/// # Errors
///
/// As [`context_for`], or when no input/manifest entry exists.
pub fn entry_and_context(
    explicit: Option<PathBuf>,
    explicit_module_paths: &[PathBuf],
) -> Result<(PathBuf, jr_project::ProjectContext)> {
    let anchor = match &explicit {
        Some(path) => path.clone(),
        None => std::env::current_dir().context("the working directory must be readable")?,
    };
    let context = context_for(&anchor, explicit_module_paths)?;
    let entry = match explicit {
        Some(path) => path,
        None => context.entry().map(Path::to_path_buf).ok_or_else(|| {
            anyhow::anyhow!(
                "no input file, and no `{}` in this directory or any parent\n\
                 \x20 give a path, or run `jr new <name>` to start a project",
                jr_manifest::FILE_NAME
            )
        })?,
    };
    if !entry.exists()
        && let Some(root) = context.root()
    {
        bail!(
            "{} names `{}` as this project's entry point, but that file does not exist",
            root.join(jr_manifest::FILE_NAME).display(),
            entry.display()
        );
    }
    Ok((entry, context))
}

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

/// The formatter configuration that applies to `path`.
///
/// # Errors
///
/// As [`find`].
pub fn fmt_config(path: &Path) -> Result<jr_fmt::Config> {
    Ok(find_for(path)?
        .map(|located| located.fmt_config())
        .unwrap_or_else(jr_manifest::default_fmt_config))
}
