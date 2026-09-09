//! Implementation of `jr new` and `jr init`.
//!
//! # Why the scaffold is four files and not one
//!
//! `jr new hello` writes a manifest, a source file, a build script and a `.gitignore`. Each earns
//! its place by being something the reader would otherwise have to be *told*:
//!
//! - **`jairs.toml`** is what makes a bare `jr run` work from anywhere in the tree, and it carries
//!   the formatter settings. Its comments name every key's default, so the file doubles as the
//!   reference for what can go in it — a scaffold nobody has to leave to understand.
//! - **`src/main.jr`** is a program that compiles and runs on the first try. It imports `Basic`,
//!   which demonstrates the thing most worth knowing: the standard library needs no `-I`.
//! - **`build.jr`** is the build-script form (ADR-0102), commented out but present. A build script
//!   is a real feature that is hard to discover from a help text, and one that is *there* and
//!   inert teaches more than one the reader never learns exists.
//! - **`.gitignore`** covers the build output. Cargo does this for the same reason: the first
//!   thing a new project does is produce an artifact nobody wants committed.
//!
//! # Why nothing is ever overwritten
//!
//! Both commands refuse rather than replace. `jr init` in a directory that already has a
//! `jairs.toml` is much more likely to be a mistake than an instruction, and the cost of being
//! wrong is asymmetric: refusing costs a re-typed command, overwriting costs a file. `jr new`
//! refuses a directory that exists at all, which is `cargo new`'s rule for the same reason.

use std::path::Path;

use anyhow::{Context as _, Result, bail};

use crate::cli::{InitArgs, NewArgs};

/// Run `jr new`.
///
/// Creates `<name>/` and scaffolds a project in it.
///
/// # Errors
///
/// When the directory exists, when the name is not usable as a project name, or on any I/O
/// failure while writing the scaffold.
pub fn run_new(args: NewArgs) -> Result<i32> {
    let root = args.path;
    let name = project_name(args.name.as_deref(), &root)?;

    if root.exists() {
        bail!(
            "`{}` already exists\n  use `jr init` inside it to scaffold in place",
            root.display()
        );
    }

    std::fs::create_dir_all(&root).with_context(|| format!("cannot create {}", root.display()))?;
    scaffold(&root, &name)?;

    crate::report::note(&format!("created project `{name}` at {}", root.display()));
    print_next_steps(Some(&root));
    Ok(0)
}

/// Run `jr init`.
///
/// Scaffolds a project in an existing directory, defaulting to the working directory.
///
/// # Errors
///
/// When the directory already holds a manifest, when the name is not usable, or on any I/O
/// failure while writing the scaffold.
pub fn run_init(args: InitArgs) -> Result<i32> {
    let root = match args.path {
        Some(path) => path,
        None => std::env::current_dir().context("the working directory must be readable")?,
    };
    let name = project_name(args.name.as_deref(), &root)?;

    let manifest = root.join(jr_manifest::FILE_NAME);
    if manifest.exists() {
        bail!(
            "{} already exists\n  nothing was changed",
            manifest.display()
        );
    }

    std::fs::create_dir_all(&root).with_context(|| format!("cannot create {}", root.display()))?;
    scaffold(&root, &name)?;

    crate::report::note(&format!("initialised project `{name}`"));
    print_next_steps(None);
    Ok(0)
}

/// The project's name: what was asked for, else the directory's own name.
///
/// # Errors
///
/// When neither source yields a usable name — a path like `.` or `/` has no file name of its
/// own, and guessing one would put a name in the manifest that the user never chose.
fn project_name(explicit: Option<&str>, root: &Path) -> Result<String> {
    if let Some(name) = explicit {
        return validate_name(name);
    }

    // `canonicalize` so that `jr init .` reads the real directory name rather than `.`. It fails
    // for a path that does not exist yet, which is `jr new`'s case — and there the path was
    // supplied, so its own file name is already the answer.
    let from_path = root
        .canonicalize()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .or_else(|| {
            root.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        });

    match from_path {
        Some(name) => validate_name(&name),
        None => bail!(
            "cannot tell what to call this project from `{}`\n  pass a name: `--name <NAME>`",
            root.display()
        ),
    }
}

/// Checks that `name` can be written into a manifest and used as a binary name.
///
/// Deliberately permissive: this rejects only what would produce a broken project, not what
/// looks unusual. A stricter rule would be this tool inventing a naming convention for a
/// language that does not have one yet.
///
/// # Errors
///
/// When the name is empty, or contains a path separator or a character that would end the TOML
/// string it is written into.
fn validate_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        bail!("a project name cannot be empty");
    }
    if let Some(bad) = name
        .chars()
        .find(|c| matches!(c, '/' | '\\' | '"' | '\n' | '\r'))
    {
        bail!("a project name cannot contain {bad:?}");
    }
    Ok(name.to_owned())
}

/// Writes the scaffold into `root`.
///
/// # Errors
///
/// On any I/O failure. Existing files are left alone and reported, never replaced.
fn scaffold(root: &Path, name: &str) -> Result<()> {
    std::fs::create_dir_all(root.join("src"))
        .with_context(|| format!("cannot create {}", root.join("src").display()))?;

    write_new(&root.join(jr_manifest::FILE_NAME), &manifest_text(name))?;
    write_new(&root.join("src").join("main.jr"), MAIN_JR)?;
    write_new(&root.join("build.jr"), &build_jr(name))?;
    write_new(&root.join(".gitignore"), &gitignore(name))?;
    Ok(())
}

/// Writes `contents` to `path` unless it already exists.
///
/// # Errors
///
/// On any I/O failure other than the file already existing. `create_new` makes the check and the
/// write one operation, so there is no window in which the file appears between them.
fn write_new(path: &Path, contents: &str) -> Result<()> {
    use std::io::Write as _;

    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => file
            .write_all(contents.as_bytes())
            .with_context(|| format!("cannot write {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            crate::report::warn(&format!("kept the existing {}", path.display()));
            Ok(())
        }
        Err(e) => Err(e).with_context(|| format!("cannot create {}", path.display())),
    }
}

/// Tells the reader what to type next.
fn print_next_steps(root: Option<&Path>) {
    if let Some(root) = root {
        println!("  cd {}", root.display());
    }
    println!("  jr run          # compile and run in the bytecode VM");
    println!("  jr build        # produce a native executable");
    println!("  jr fmt          # format every file in the project");
}

/// The scaffolded `jairs.toml`.
///
/// Every key it does not set is named in a comment with its default beside it, so the file is
/// also the documentation for what a manifest accepts. That matters more here than brevity: an
/// unknown key is an error (see `jr-manifest`'s docs), so a reader guessing at a key name gets a
/// refusal, and this comment is what stops them having to guess.
fn manifest_text(name: &str) -> String {
    format!(
        "# The project manifest. Every setting here is optional, and `jr` works without this file
# at all — it is what lets `jr run` and `jr build` work with no path argument.
#
# An unrecognised key is an error rather than being ignored, so a typo is reported instead of
# silently doing nothing.

[project]
name = \"{name}\"
# entry = \"src/main.jr\"        # the file a bare `jr build` / `jr run` / `jr check` compiles

[fmt]
indent_style = \"space\"         # \"space\" or \"tab\"
indent_width = 2               # spaces per level; not read when indent_style = \"tab\"
case_block_style = \"next_line\" # \"next_line\" or \"same_line\" for a case's sole {{ ... }} block
max_width = 100                # breaks a long argument or parameter list. Comments are never
#                              # reflowed, so a longer line can still survive here.

[build]
# module_paths = [\"vendor\"]    # extra directories to search for `#import`ed modules.
#                              # The standard library is compiled into `jr` and always
#                              # available, so it needs no entry here.

[dependencies]
# Geometry = {{ path = \"../geometry\" }} # exact `#import \"Geometry\"` source. A directory
#                                        # names only its `module.jr`; siblings stay private.
"
    )
}

/// The scaffolded `src/main.jr`.
const MAIN_JR: &str =
    "// The standard library is compiled into the `jr` binary, so an import needs no search path.
#import \"Basic\";

main :: () {
    print(\"Hello from Jairs!\\n\");
}
";

/// The scaffolded `build.jr`.
///
/// Inert on purpose. A build script runs at compile time and drives the compilation itself
/// (ADR-0102), which is worth knowing about and almost impossible to discover from `--help`.
/// Commented out, so `jr build` compiles `src/main.jr` until the reader chooses otherwise.
fn build_jr(name: &str) -> String {
    format!(
        "// A build script: a Jairs program that runs at compile time and describes the build.
//
// `jr build build.jr --script` runs this file instead of compiling it. Uncomment the body to
// take control of the output name, the optimisation level, or the libraries to link.
//
// #import \"Compiler\";
//
// build :: () {{
//     target := add_target(\"src/main.jr\");
//     set_output(target, \"{name}\");
//     set_optimisation(target, 1);
// }}
"
    )
}

/// The scaffolded `.gitignore`.
fn gitignore(name: &str) -> String {
    format!("/{name}\n/*.o\n")
}

#[cfg(test)]
mod tests {
    use super::{manifest_text, project_name, validate_name};
    use std::path::Path;

    #[test]
    fn the_scaffolded_manifest_parses_and_uses_project_defaults() {
        // The scaffold must not be a file the tool would itself refuse — and since unknown keys
        // are an error, a stale comment that gets uncommented must still be accepted.
        let text = manifest_text("demo");
        let parsed: jr_manifest::Manifest = toml::from_str(&text).expect("the scaffold must parse");
        assert_eq!(parsed.project.name.as_deref(), Some("demo"));

        let located = jr_manifest::Located {
            root: Path::new("/p").to_path_buf(),
            manifest: parsed,
        };
        let config = located.fmt_config();
        let default = jr_manifest::default_fmt_config();
        // Generated projects and manifest-free files share the same two-space default.
        assert_eq!(config.indent_width, 2);
        assert_eq!(default.indent_width, 2);
        assert_eq!(config.indent_style, default.indent_style);
        assert_eq!(config.case_block_style, default.case_block_style);
        assert_eq!(config.max_width, default.max_width);
    }

    #[test]
    fn every_commented_key_in_the_scaffold_is_a_real_key() {
        // A commented-out key that does not exist is worse than no comment: the reader
        // uncomments it and gets a refusal from a file the tool wrote.
        let text = manifest_text("demo");
        let uncommented: String = text
            .lines()
            .map(|line| {
                let trimmed = line.trim_start();
                // Only the `# key = value` lines, not the prose.
                match trimmed.strip_prefix("# ") {
                    Some(rest) if rest.contains(" = ") => rest.to_owned(),
                    Some(_) | None => line.to_owned(),
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        toml::from_str::<jr_manifest::Manifest>(&uncommented)
            .expect("every commented key must be one the manifest accepts");
    }

    #[test]
    fn a_name_comes_from_the_directory_when_not_given() {
        let name = project_name(None, Path::new("/tmp/some-project")).expect("has a file name");
        assert_eq!(name, "some-project");
    }

    #[test]
    fn an_explicit_name_wins() {
        let name = project_name(Some("chosen"), Path::new("/tmp/ignored")).expect("explicit");
        assert_eq!(name, "chosen");
    }

    #[test]
    fn a_name_that_would_break_the_manifest_is_refused() {
        // A quote would end the TOML string and produce a manifest the tool then refuses to
        // parse — a scaffold that writes a broken file is worse than one that declines.
        assert!(validate_name("say\"what").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("   ").is_err());
        assert!(validate_name("ok-name_1").is_ok());
    }

    #[test]
    fn a_root_with_no_file_name_is_refused_rather_than_guessed() {
        assert!(project_name(None, Path::new("/")).is_err());
    }
}
