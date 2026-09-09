//! `jairs.toml` — the optional project manifest.
//!
//! # Why this exists, and why it is optional
//!
//! ADR-0029 §1 rejected a `jairs.toml` when it was proposed as a *prerequisite* for workspace
//! discovery, and the reasoning still holds: it would have been a new artifact that this
//! repository, every corpus directory and every scratch file needed, or else it would have
//! needed a fallback to the rule it was replacing — "at which point the rule is doing the work
//! and the manifest is an optional override". That ADR left it available as a later addition on
//! exactly those terms, and this is it: **an override, never a requirement.**
//!
//! Two consequences that are load-bearing rather than incidental:
//!
//! - **Every command works with no manifest present.** A missing file is not an error and not a
//!   warning; it produces [`Manifest::default`]. `jr fmt x.jr` on a bare file in `/tmp` behaves
//!   exactly as it did before this crate existed.
//! - **A manifest never silently changes what a command already decided.** An explicit
//!   command-line flag outranks the file, because a flag is an instruction and a file is a
//!   default (the same asymmetry ADR-0102 §2 drew between `-O` and a declared `BUILD_OPT_LEVEL`).
//!
//! # Why unknown keys are an error
//!
//! `deny_unknown_fields`, deliberately. A configuration file whose typos are ignored is the
//! worst of both worlds: `indent_size = 2` next to a formatter that indents by four looks like a
//! formatter bug, and the user has no way to tell a key that does nothing from a key that is not
//! read yet. Refusing the file names the mistake at the only moment anyone can act on it.
//!
//! That decision is also why this crate exposes no setting the tools do not honour. `max_width`
//! was absent here for exactly that reason — declared in the formatter, defaulted and **never
//! read**. It is offered now because line wrapping exists, and its documentation states the
//! scope rather than implying a guarantee: two constructs are broken, comments are never
//! reflowed, and a formatted file may still hold a longer line.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::Deserialize;

/// The manifest's file name.
pub const FILE_NAME: &str = "jairs.toml";

/// The default entry point a project's `[project] entry` falls back to.
pub const DEFAULT_ENTRY: &str = "src/main.jr";

/// The formatter configuration for projects that do not override `[fmt]`.
///
/// This remains the project-facing ownership point even though the formatter crate now shares its
/// two-space default (ADR-0222).
#[must_use]
pub fn default_fmt_config() -> jr_fmt::Config {
    jr_fmt::Config::default()
}

/// A parsed `jairs.toml`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Manifest {
    /// What the project is and where it starts.
    pub project: Project,
    /// How `jr fmt` formats this project.
    pub fmt: Fmt,
    /// How this project is built.
    pub build: Build,
    /// Exact named module dependencies.
    pub dependencies: BTreeMap<String, Dependency>,
}

/// One entry in the top-level `[dependencies]` table.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dependency {
    /// A module file or directory.
    ///
    /// Relative paths are resolved against the manifest's directory by
    /// [`Located::exact_dependencies`].
    pub path: PathBuf,
}

/// The `[project]` table.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Project {
    /// The project's name. Used for the default output binary name.
    pub name: Option<String>,
    /// The file a bare `jr build`, `jr run` or `jr check` compiles.
    ///
    /// Relative to the manifest's own directory, never to the working directory — so the same
    /// command means the same thing from anywhere inside the project.
    pub entry: Option<PathBuf>,
}

/// The `[fmt]` table.
///
/// Mirrors EditorConfig's `indent_style` / `indent_size` vocabulary rather than inventing one,
/// because a reader who has configured an editor already knows what these two mean together.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Fmt {
    /// `"space"` or `"tab"`.
    pub indent_style: Option<IndentStyle>,
    /// Spaces per indentation level.
    ///
    /// Not read when `indent_style = "tab"`: a formatter emitting a tab does not decide how wide
    /// it looks. Documented here and in the scaffolded manifest, so the interaction is stated
    /// rather than discovered.
    pub indent_width: Option<usize>,
    /// Whether a switch arm's sole braced block begins on the next line or the arm's line.
    pub case_block_style: Option<CaseBlockStyle>,
    /// The column a line should not exceed.
    ///
    /// **Read the scope before setting it.** It breaks a call's argument list and a procedure's
    /// parameter list; it does not reflow comments and does not break a boolean chain, so a
    /// formatted file may still hold longer lines. [`jr_fmt::Config::max_width`] states exactly
    /// what is covered and why, with the corpus measurement behind the decision.
    ///
    /// This key was **absent** until line wrapping existed. It was declared in the formatter,
    /// defaulted to 100 and never read, so offering it would have been a setting that appears to
    /// work — which is the same reason unknown keys are refused here.
    pub max_width: Option<usize>,
}

/// What one level of indentation is made of, as spelled in the manifest.
///
/// A separate type from [`jr_fmt::IndentStyle`] on purpose: this one is the *file format*, and a
/// file format is a compatibility promise. Deriving `Deserialize` on the formatter's own enum
/// would make every future change to it a silent change to the manifest's accepted spellings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IndentStyle {
    /// [`Fmt::indent_width`] spaces per level.
    Space,
    /// One tab per level.
    Tab,
}

impl From<IndentStyle> for jr_fmt::IndentStyle {
    fn from(style: IndentStyle) -> Self {
        match style {
            IndentStyle::Space => Self::Space,
            IndentStyle::Tab => Self::Tab,
        }
    }
}

/// The placement of a switch arm's sole braced block, as spelled in the manifest.
///
/// Kept separate from [`jr_fmt::CaseBlockStyle`] for the same compatibility reason as
/// [`IndentStyle`]: these exact strings are part of the manifest file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum CaseBlockStyle {
    /// Put the block below the arm header.
    #[serde(rename = "next_line")]
    NextLine,
    /// Put the opening brace on the arm header's line.
    #[serde(rename = "same_line")]
    SameLine,
}

impl From<CaseBlockStyle> for jr_fmt::CaseBlockStyle {
    fn from(style: CaseBlockStyle) -> Self {
        match style {
            CaseBlockStyle::NextLine => Self::NextLine,
            CaseBlockStyle::SameLine => Self::SameLine,
        }
    }
}

/// The `[build]` table.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Build {
    /// Legacy compatibility directories for `#import`ed modules.
    ///
    /// Relative entries resolve against the manifest's directory. New projects should prefer
    /// implicit direct `src` modules or exact `[dependencies]`; these roots remain after those
    /// project declarations and before the bundled standard library (ADR-0213 §4).
    #[serde(default)]
    pub module_paths: Vec<PathBuf>,
}

/// A manifest together with the directory it was found in.
#[derive(Debug, Clone)]
pub struct Located {
    /// The directory containing the manifest. Every relative path in the manifest is resolved
    /// against this, never against the working directory.
    pub root: PathBuf,
    /// The parsed manifest.
    pub manifest: Manifest,
}

impl Located {
    /// The file a bare `jr build`, `jr run` or `jr check` should compile.
    ///
    /// Absolute, so a caller cannot accidentally resolve it against the wrong directory.
    #[must_use]
    pub fn entry(&self) -> PathBuf {
        self.root.join(
            self.manifest
                .project
                .entry
                .clone()
                .unwrap_or_else(|| PathBuf::from(DEFAULT_ENTRY)),
        )
    }

    /// The legacy module roots the manifest declares, resolved against its directory.
    #[must_use]
    pub fn module_paths(&self) -> Vec<PathBuf> {
        self.manifest
            .build
            .module_paths
            .iter()
            .map(|path| self.root.join(path))
            .collect()
    }

    /// Exact named dependencies, with paths resolved against the manifest's directory.
    ///
    /// Entries are returned in deterministic module-name order.
    #[must_use]
    pub fn exact_dependencies(&self) -> Vec<(&str, PathBuf)> {
        self.manifest
            .dependencies
            .iter()
            .map(|(name, dependency)| (name.as_str(), self.root.join(&dependency.path)))
            .collect()
    }

    /// The formatter configuration, with anything the manifest leaves out taken from
    /// [`default_fmt_config`].
    #[must_use]
    pub fn fmt_config(&self) -> jr_fmt::Config {
        let mut config = default_fmt_config();
        if let Some(style) = self.manifest.fmt.indent_style {
            config.indent_style = style.into();
        }
        if let Some(width) = self.manifest.fmt.indent_width {
            config.indent_width = width;
        }
        if let Some(style) = self.manifest.fmt.case_block_style {
            config.case_block_style = style.into();
        }
        if let Some(width) = self.manifest.fmt.max_width {
            config.max_width = width;
        }
        config
    }
}

/// Why a manifest could not be used.
#[derive(Debug)]
pub enum Error {
    /// The file exists but could not be read.
    Read {
        /// The manifest's path.
        path: PathBuf,
        /// The underlying I/O error.
        source: std::io::Error,
    },
    /// The file exists but is not a valid manifest.
    Parse {
        /// The manifest's path.
        path: PathBuf,
        /// The underlying TOML error, which carries its own line and column.
        source: toml::de::Error,
    },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(f, "cannot read {}: {source}", path.display())
            }
            Self::Parse { path, source } => {
                // The TOML error already names the line, the column and the offending key, and
                // renders them better than a re-wrapping would. Printed as-is for that reason.
                write!(f, "{} is not a valid manifest: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
        }
    }
}

/// Finds the manifest governing `from`, by walking up from it.
///
/// Returns `Ok(None)` when there is no manifest in `from` or any ancestor — the ordinary case
/// for a scratch file, and not an error.
///
/// Walking up rather than reading only `from` is what makes `jr fmt src/deep/x.jr` and
/// `cd src/deep && jr fmt x.jr` format identically. The walk stops at the first manifest found
/// rather than merging every one on the way up: a nested project is a project, and inheriting
/// half of a parent's settings would make the effective configuration something no single file
/// states.
///
/// # Errors
///
/// When a manifest exists but cannot be read or parsed. A malformed manifest is an error rather
/// than a silent fallback to the defaults, because the alternative is a formatter that quietly
/// ignores the file the user just edited.
pub fn find(from: &Path) -> Result<Option<Located>, Error> {
    let start = if from.is_dir() {
        from
    } else {
        from.parent().unwrap_or(from)
    };

    for dir in start.ancestors() {
        let candidate = dir.join(FILE_NAME);
        // `read_to_string` rather than `exists()` then read: one syscall, and no window in which
        // the file is deleted between the two.
        match std::fs::read_to_string(&candidate) {
            Ok(text) => {
                let manifest = toml::from_str(&text).map_err(|source| Error::Parse {
                    path: candidate.clone(),
                    source,
                })?;
                return Ok(Some(Located {
                    root: dir.to_path_buf(),
                    manifest,
                }));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(Error::Read {
                    path: candidate,
                    source,
                });
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{CaseBlockStyle, Error, FILE_NAME, IndentStyle, Located, Manifest, find};

    fn located(text: &str) -> Located {
        Located {
            root: Path::new("/proj").to_path_buf(),
            manifest: toml::from_str::<Manifest>(text).expect("should parse"),
        }
    }

    #[test]
    fn an_empty_manifest_is_all_defaults() {
        let l = located("");
        assert_eq!(l.entry(), Path::new("/proj/src/main.jr"));
        assert_eq!(l.fmt_config().indent_width, 2);
        assert_eq!(l.fmt_config().indent_style, jr_fmt::IndentStyle::Space);
        assert_eq!(
            l.fmt_config().case_block_style,
            jr_fmt::CaseBlockStyle::NextLine
        );
        assert_eq!(l.fmt_config().max_width, 100);
        assert!(l.module_paths().is_empty());
        assert!(l.exact_dependencies().is_empty());
    }

    #[test]
    fn exact_dependencies_parse_file_and_directory_paths() {
        let l = located(
            "[dependencies]\n\
             Geometry = { path = \"../geometry\" }\n\
             Math = { path = \"vendor/math.jr\" }\n",
        );

        assert_eq!(
            l.manifest.dependencies["Geometry"].path,
            Path::new("../geometry")
        );
        assert_eq!(
            l.manifest.dependencies["Math"].path,
            Path::new("vendor/math.jr")
        );
    }

    #[test]
    fn exact_dependencies_are_resolved_relative_to_the_manifest_root() {
        let l = located(
            "[dependencies]\n\
             Zed = { path = \"/opt/zed/module.jr\" }\n\
             Alpha = { path = \"../alpha\" }\n",
        );

        assert_eq!(
            l.exact_dependencies(),
            vec![
                ("Alpha", Path::new("/proj/../alpha").to_path_buf()),
                ("Zed", Path::new("/opt/zed/module.jr").to_path_buf()),
            ]
        );
    }

    #[test]
    fn an_unknown_exact_dependency_key_is_refused() {
        let err = toml::from_str::<Manifest>(
            "[dependencies]\nGeometry = { path = \"../geometry\", version = \"1\" }\n",
        )
        .expect_err("an unknown dependency key must be refused");
        assert!(err.to_string().contains("version"), "got: {err}");
    }

    #[test]
    fn tabs_are_selectable() {
        let l = located("[fmt]\nindent_style = \"tab\"\n");
        assert_eq!(l.fmt_config().indent_style, jr_fmt::IndentStyle::Tab);
    }

    #[test]
    fn max_width_is_read() {
        let l = located("[fmt]\nmax_width = 60\n");
        assert_eq!(l.fmt_config().max_width, 60);
        // Setting one key must not disturb the others.
        assert_eq!(l.fmt_config().indent_width, 2);
    }

    #[test]
    fn indent_width_is_read() {
        let l = located("[fmt]\nindent_width = 2\n");
        assert_eq!(l.fmt_config().indent_width, 2);
        // Unset keys must not be disturbed by a set one.
        assert_eq!(l.fmt_config().indent_style, jr_fmt::IndentStyle::Space);
    }

    #[test]
    fn case_block_style_is_read() {
        let l = located("[fmt]\ncase_block_style = \"same_line\"\n");
        assert_eq!(
            l.fmt_config().case_block_style,
            jr_fmt::CaseBlockStyle::SameLine
        );
        assert_eq!(l.fmt_config().indent_width, 2);
    }

    #[test]
    fn an_entry_is_relative_to_the_manifest_not_the_cwd() {
        // The whole reason `Located` carries a root: resolving against the working directory
        // would make one command mean two things depending on where it was run.
        let l = located("[project]\nentry = \"src/other.jr\"\n");
        assert_eq!(l.entry(), Path::new("/proj/src/other.jr"));
    }

    #[test]
    fn module_paths_are_relative_to_the_manifest() {
        let l = located("[build]\nmodule_paths = [\"vendor\", \"lib/mods\"]\n");
        assert_eq!(
            l.module_paths(),
            vec![
                Path::new("/proj/vendor").to_path_buf(),
                Path::new("/proj/lib/mods").to_path_buf(),
            ]
        );
    }

    #[test]
    fn an_unknown_key_is_refused() {
        // The reason this crate exists in the shape it does. A tolerated typo is a setting that
        // appears to work.
        let err = toml::from_str::<Manifest>("[fmt]\nindent_size = 2\n")
            .expect_err("an unknown key must be refused");
        assert!(
            err.to_string().contains("indent_size"),
            "the error must name the offending key, got: {err}"
        );
    }

    #[test]
    fn an_unknown_table_is_refused() {
        let err = toml::from_str::<Manifest>("[frmt]\nindent_width = 2\n")
            .expect_err("an unknown table must be refused");
        assert!(err.to_string().contains("frmt"), "got: {err}");
    }

    #[test]
    fn a_bad_indent_style_is_refused() {
        let err = toml::from_str::<Manifest>("[fmt]\nindent_style = \"tabs\"\n")
            .expect_err("only `space` and `tab` are spellings");
        assert!(err.to_string().contains("tabs"), "got: {err}");
    }

    #[test]
    fn a_bad_case_block_style_is_refused() {
        let err = toml::from_str::<Manifest>("[fmt]\ncase_block_style = \"same-line\"\n")
            .expect_err("only `next_line` and `same_line` are spellings");
        assert!(err.to_string().contains("same-line"), "got: {err}");
    }

    #[test]
    fn the_style_spellings_are_exactly_two() {
        assert_eq!(
            toml::from_str::<super::Fmt>("indent_style = \"space\"")
                .expect("space")
                .indent_style,
            Some(IndentStyle::Space)
        );
        assert_eq!(
            toml::from_str::<super::Fmt>("indent_style = \"tab\"")
                .expect("tab")
                .indent_style,
            Some(IndentStyle::Tab)
        );
    }

    #[test]
    fn the_case_block_style_spellings_are_exactly_two() {
        assert_eq!(
            toml::from_str::<super::Fmt>("case_block_style = \"next_line\"")
                .expect("next line")
                .case_block_style,
            Some(CaseBlockStyle::NextLine)
        );
        assert_eq!(
            toml::from_str::<super::Fmt>("case_block_style = \"same_line\"")
                .expect("same line")
                .case_block_style,
            Some(CaseBlockStyle::SameLine)
        );
    }

    #[test]
    fn no_manifest_anywhere_is_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let found = find(dir.path()).expect("a missing manifest is not an error");
        assert!(found.is_none());
    }

    #[test]
    fn the_walk_finds_a_manifest_in_an_ancestor() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(FILE_NAME), "[project]\nname = \"p\"\n").expect("write");
        let deep = dir.path().join("src").join("deep");
        std::fs::create_dir_all(&deep).expect("mkdir");

        let from_deep = find(&deep).expect("ok").expect("found from a subdirectory");
        let from_root = find(dir.path()).expect("ok").expect("found from the root");

        // Both must agree about the root, or the same file formats two ways depending on where
        // the command was run from.
        assert_eq!(from_deep.root, from_root.root);
        assert_eq!(from_deep.manifest.project.name.as_deref(), Some("p"));
    }

    #[test]
    fn the_walk_starts_at_a_files_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(FILE_NAME), "").expect("write");
        let file = dir.path().join("x.jr");
        std::fs::write(&file, "main :: () {}\n").expect("write");

        assert!(
            find(&file).expect("ok").is_some(),
            "a path naming a file must find the manifest beside it"
        );
    }

    #[test]
    fn the_nearest_manifest_wins() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(FILE_NAME), "[fmt]\nindent_width = 8\n").expect("write");
        let inner = dir.path().join("inner");
        std::fs::create_dir_all(&inner).expect("mkdir");
        std::fs::write(inner.join(FILE_NAME), "[fmt]\nindent_width = 2\n").expect("write");

        let found = find(&inner).expect("ok").expect("found");
        assert_eq!(
            found.fmt_config().indent_width,
            2,
            "the nearest manifest must win outright, not merge with its parent"
        );
    }

    #[test]
    fn a_malformed_manifest_is_an_error_not_a_silent_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join(FILE_NAME),
            "[fmt]\nindent_width = \"four\"\n",
        )
        .expect("write");

        let err = find(dir.path()).expect_err("a malformed manifest must be reported");
        match err {
            Error::Parse { ref path, .. } => {
                assert!(path.ends_with(FILE_NAME));
                assert!(
                    err.to_string().contains("not a valid manifest"),
                    "got: {err}"
                );
            }
            Error::Read { .. } => panic!("expected a parse error, got a read error"),
        }
    }
}
