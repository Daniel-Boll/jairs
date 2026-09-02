//! Turning an object file into an executable.
//!
//! # Why the system C driver and not `ld` directly
//!
//! Linking an executable needs the platform's C runtime startup object, the dynamic
//! loader's path, the SDK's library search paths and, on Apple platforms, an
//! `-arch` and a deployment target. `cc` knows all of those and `ld` does not — on
//! macOS the correct `ld` invocation depends on the active Xcode SDK, and
//! reconstructing it here would be reimplementing `clang`'s driver badly.
//!
//! So this module shells out to a C driver. That is a real dependency on a system
//! toolchain, and it is the one dependency the slice cannot avoid: a program that
//! calls `write` has to be linked against libc regardless.
//!
//! # Why there is no runtime object
//!
//! ADR-0019 §2 chose a trap that calls a helper, and expected the helper to live in
//! a small runtime linked in here. `jr-codegen-clif` generates it into the object
//! instead — it needs only libc `write` and `exit`, which the program is already
//! linked against — so there is no runtime artifact to build, ship, or keep working
//! per platform. The decision is unchanged; only its machinery is smaller.
//!
//! # Why codesigning is usually not our problem
//!
//! On Apple silicon every executable must be signed, but `ld64` ad-hoc signs what it
//! links, so a binary produced through `cc` arrives already signed. The explicit
//! `codesign` pass here is a fallback for a toolchain that does not, and it is
//! skipped when the binary already has a signature — running it twice is an error,
//! not a no-op.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Why linking failed.
#[derive(Debug)]
pub enum LinkError {
    /// No C driver could be found on `PATH`.
    ///
    /// Carries the names that were tried, because "install a compiler" is not
    /// actionable and "none of `cc`, `clang` or `gcc` is on your PATH" is.
    NoDriver(Vec<String>),
    /// The object file could not be written to a temporary path.
    Io(std::io::Error),
    /// The driver ran and rejected the link.
    ///
    /// Carries the driver's own diagnostics, which are far more useful than anything
    /// this crate could say about them.
    Failed {
        /// The program that was run.
        driver: String,
        /// Its exit status, rendered.
        status: String,
        /// Everything it printed.
        output: String,
    },
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoDriver(tried) => write!(
                f,
                "no C driver found: none of {} is on PATH",
                tried.join(", ")
            ),
            Self::Io(error) => write!(f, "cannot write the object file: {error}"),
            Self::Failed {
                driver,
                status,
                output,
            } => {
                write!(f, "{driver} failed ({status})")?;
                if !output.trim().is_empty() {
                    write!(f, ":\n{}", output.trim_end())?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for LinkError {}

impl From<std::io::Error> for LinkError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// What to link, and into what.
pub struct LinkRequest<'a> {
    /// The object file's bytes, as `jr-codegen`'s `finalise` produced them.
    pub object: &'a [u8],
    /// Where the executable goes.
    pub output: &'a Path,
    /// Libraries every `#foreign` declaration named, without the `lib` prefix — `"c"`
    /// for libc.
    ///
    /// These come from the one resolution ADR-0019 §4 interned in the pool, so the
    /// link line cannot disagree with what `jr-sema` checked and the VM called.
    pub libraries: &'a [String],
    /// Directories to search for those libraries, in order, before the driver's own defaults.
    ///
    /// Each becomes a `-L`. Needed because a `#system_library` names *what* to link and never *where*: `-lc`
    /// resolves from the driver's defaults, and `-lSDL2` does not — the link fails with
    /// `ld: library 'SDL2' not found`, which is the wall W10 hit the moment a graphics library was tried
    /// (ADR-0163 §2).
    ///
    /// **Not read from the source.** A path is a property of the machine compiling, not of the program, so it
    /// comes from the operator through `--library-path` or `JR_LIBRARY_PATH` — the same asymmetry that makes
    /// `-o` outrank a declared `BUILD_OUTPUT` (ADR-0102 §2, ADR-0122). A program that hard-coded
    /// `/opt/homebrew/lib` would be unbuildable everywhere else.
    pub library_paths: &'a [std::path::PathBuf],
}

/// Links an object into an executable.
///
/// The object is written beside the output rather than into the system temporary
/// directory, so that a failed link leaves it where it can be inspected with
/// `objdump` — which is the first thing anyone wants when a link fails.
///
/// # Errors
/// [`LinkError`] when no driver exists, the object cannot be written, or the driver
/// rejects the link.
pub fn link(request: &LinkRequest<'_>) -> Result<(), LinkError> {
    let object_path = request.output.with_extension("o");
    std::fs::write(&object_path, request.object)?;

    let driver = find_driver().ok_or_else(|| {
        LinkError::NoDriver(DRIVERS.iter().map(|name| (*name).to_owned()).collect())
    })?;

    let mut command = Command::new(&driver);
    command.arg(not_a_flag(&object_path));
    command.arg("-o").arg(not_a_flag(request.output));
    // **Search paths before the libraries**, which is what `ld` requires: a `-L` affects the `-l`s that
    // follow it, so emitting them the other way round would look right and find nothing.
    for path in request.library_paths {
        command.arg(format!("-L{}", path.display()));
    }
    for library in request.libraries {
        command.arg(format!("-l{library}"));
    }

    let output = command.output()?;
    if !output.status.success() {
        let mut text = String::from_utf8_lossy(&output.stderr).into_owned();
        text.push_str(&String::from_utf8_lossy(&output.stdout));
        return Err(LinkError::Failed {
            driver: driver.to_string_lossy().into_owned(),
            status: output.status.to_string(),
            output: text,
        });
    }

    // The object has served its purpose; a successful link leaves only the binary.
    let _ = std::fs::remove_file(&object_path);

    ensure_signed(request.output);
    Ok(())
}

/// The C drivers tried, in order of preference.
const DRIVERS: [&str; 3] = ["cc", "clang", "gcc"];

/// A path `cc` cannot mistake for an option, by prefixing `./` when it begins with `-`.
///
/// The object path is the driver's **first positional argument** and the output its `-o` value, so a path
/// beginning with `-` is read as a flag rather than as a file — `-Wl,…` being the interesting case. Prefixing
/// `./` is behaviour-preserving for the filesystem (`./-x` and `-x` name the same file) and removes the
/// ambiguity for the argument parser.
///
/// Done **here** rather than trusting the caller, even though `jr build` now confines a declared
/// `BUILD_OUTPUT` (ADR-0122): an explicit `-o` is deliberately not confined, because that is a person saying
/// where they want the file, and this module should not depend on which of its callers checked what. A linker
/// driver that cannot be made to read its own arguments wrongly is one less thing to reason about.
fn not_a_flag(path: &Path) -> PathBuf {
    if path.as_os_str().as_encoded_bytes().starts_with(b"-") {
        return Path::new(".").join(path);
    }
    path.to_owned()
}

/// The first C driver on `PATH`.
fn find_driver() -> Option<PathBuf> {
    for name in DRIVERS {
        // `--version` rather than `which`: it answers the question that matters — can
        // this be executed — on any platform, without depending on a shell builtin.
        if Command::new(name)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
        {
            return Some(PathBuf::from(name));
        }
    }
    None
}

/// Ad-hoc signs `path` on Apple platforms if it is not already signed.
///
/// Best-effort by design: `ld64` has already signed anything it linked, so this only
/// fires for a toolchain that did not, and a failure here is not worth failing a
/// build that otherwise succeeded — the binary either runs or the operating system
/// says why far more clearly than this could.
fn ensure_signed(path: &Path) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let signed = Command::new("codesign")
        .arg("--verify")
        .arg(not_a_flag(path))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if signed {
        return;
    }
    let _ = Command::new("codesign")
        .args(["--sign", "-", "--force"])
        .arg(not_a_flag(path))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::not_a_flag;

    #[test]
    fn an_ordinary_path_is_unchanged() {
        assert_eq!(not_a_flag(Path::new("app")), Path::new("app"));
        assert_eq!(not_a_flag(Path::new("build/app")), Path::new("build/app"));
        assert_eq!(not_a_flag(Path::new("/tmp/app")), Path::new("/tmp/app"));
    }

    #[test]
    fn a_leading_dash_is_hidden_behind_a_current_directory() {
        // `./-x` and `-x` name the same file, so this is behaviour-preserving for the filesystem while
        // removing the ambiguity for `cc`'s argument parser.
        assert_eq!(
            not_a_flag(Path::new("-Wl,--version")),
            Path::new("./-Wl,--version")
        );
        assert_eq!(not_a_flag(Path::new("-o")), Path::new("./-o"));
    }
}
