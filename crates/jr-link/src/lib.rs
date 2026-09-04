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

/// Which linker argument a library's name becomes (ADR-0183 §1).
///
/// Declared **here** rather than imported from `jr-pool`, which also has one, because this crate has
/// **no dependencies at all** — that is the seam ADR-0009 drew, and it is why the linker can be read and
/// tested without the compiler. The caller converts; the duplication is two variants and it buys a leaf
/// crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// `-lNAME`, searched for in the `-L` paths. Every target supports this form.
    Library,
    /// `-framework NAME`, a macOS framework bundle. Two arguments, not one word.
    Framework,
}

/// One library to link, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkLibrary {
    /// The name as written — `"c"`, `"SDL2"`, `"OpenGL"`.
    pub name: String,
    /// Which argument form the name takes.
    pub kind: LinkKind,
}

/// What kind of artefact to produce (ADR-0197 §1).
///
/// Jai's `output_type`, which 13 of 23 real `build.jai` files set — the second most common option after
/// the executable's name — and the largest single thing a build script here could not ask for.
///
/// # Why three and not two
///
/// A **static** library is not a link at all: it is `ar` archiving objects, and no C driver is involved.
/// A **dynamic** library *is* a link, with one extra flag and a different one per platform. Collapsing
/// them into "not an executable" would put an `if` inside the link path that decides whether to run the
/// linker, which is the shape that hides a bug — so the kinds are separate and [`link`] dispatches once.
///
/// An **object** is neither: it is the bytes `jr-codegen` produced, written out. It exists so a script can
/// do its own linking, which is what Jai's `READY_FOR_CUSTOM_LINK_COMMAND` is for — and it needs no
/// mechanism here beyond not deleting the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputKind {
    /// An executable, through the C driver. The default and what every earlier version produced.
    #[default]
    Executable,
    /// A dynamic library: `.dylib` on macOS, `.so` elsewhere. Linked, with `-dynamiclib` or `-shared`.
    Dynamic,
    /// A static library: `.a`, produced by `ar` rather than by a linker.
    Static,
    /// The object file alone, for a caller that will link it itself.
    Object,
}

impl OutputKind {
    /// The extension this kind's artefact conventionally has, if it has one.
    ///
    /// `None` for an executable, which has none on Unix. Applied by the driver rather than here, because
    /// whether to *override* a name the script chose is a policy question and this crate has no policies.
    #[must_use]
    pub const fn extension(self) -> Option<&'static str> {
        match self {
            Self::Executable => None,
            // `.dylib` and `.so` are not interchangeable: `dlopen` on macOS will load a `.so`, but the
            // linker's `-lNAME` search looks for `libNAME.dylib` and will not find a `.so`. So a dynamic
            // library that another Jairs program links against must have the platform's own extension.
            Self::Dynamic => Some(if cfg!(target_os = "macos") {
                "dylib"
            } else {
                "so"
            }),
            Self::Static => Some("a"),
            Self::Object => Some("o"),
        }
    }
}

/// What to link, and into what.
pub struct LinkRequest<'a> {
    /// Which kind of artefact to produce.
    pub kind: OutputKind,
    /// Extra arguments handed to the C driver, after everything this crate generates.
    ///
    /// Jai's `additional_linker_arguments`. **After**, so a script can override: `ld` and `cc` generally
    /// take the last of a conflicting pair, and a script that needs `-Wl,-dead_strip` or an `-rpath` is
    /// asking for something this crate has no opinion about.
    ///
    /// Passed through **unaltered**, including a leading `-` — that is the point of the option. The
    /// confinement that protects an output *path* does not apply: these are arguments, not filenames, and
    /// a script that can already run arbitrary subprocesses cannot be meaningfully restrained here.
    pub linker_arguments: &'a [String],
    /// The object file's bytes, as `jr-codegen`'s `finalise` produced them.
    pub object: &'a [u8],
    /// Where the executable goes.
    pub output: &'a Path,
    /// Libraries every `#foreign` declaration named, each with the argument form it takes.
    ///
    /// A `Library` is `-lNAME`, without the `lib` prefix — `"c"` for libc. A `Framework` is
    /// `-framework NAME`, which only macOS accepts (ADR-0183 §1).
    ///
    /// These come from the one resolution ADR-0019 §4 interned in the pool, so the
    /// link line cannot disagree with what `jr-sema` checked and the VM called.
    pub libraries: &'a [LinkLibrary],
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
    // **The object is always written**, whatever the kind: it is the input to every path below, and for
    // `OutputKind::Object` it *is* the answer.
    let object_path = match request.kind {
        // Writing straight to the output would give `foo.o.o` for a caller that already named it `foo.o`,
        // and the two names must not collide — the executable path deletes the object on success.
        OutputKind::Object => request.output.to_path_buf(),
        _ => request.output.with_extension("o"),
    };
    if let Some(parent) = object_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&object_path, request.object)?;

    // **An object needs nothing else**, and in particular needs no toolchain — so this returns before
    // `find_driver`, which is what makes `--emit-object` work on a machine with no `cc` at all.
    if request.kind == OutputKind::Object {
        return Ok(());
    }

    // **A static library is `ar`, not a link.** No C driver, no libraries, no search paths: an archive
    // records objects and nothing about what they need, which is why a caller linking against one still
    // has to name the same `#system_library`s itself.
    if request.kind == OutputKind::Static {
        let result = archive(&object_path, request.output);
        let _ = std::fs::remove_file(&object_path);
        return result;
    }

    let driver = find_driver().ok_or_else(|| {
        LinkError::NoDriver(DRIVERS.iter().map(|name| (*name).to_owned()).collect())
    })?;

    let mut command = Command::new(&driver);
    // **The kind flag first**, before the object: `cc` accepts it anywhere, but a reader of the failing
    // command line should see what kind of link was attempted before what went into it.
    //
    // `-dynamiclib` is Apple's and `-shared` is everyone else's; they are not aliases, and `clang` on macOS
    // accepts `-shared` while producing something `-lNAME` will not find, which is the silent-wrong-answer
    // shape this project avoids by naming the platform rather than hoping.
    if request.kind == OutputKind::Dynamic {
        command.arg(if cfg!(target_os = "macos") {
            "-dynamiclib"
        } else {
            "-shared"
        });
    }
    command.arg(not_a_flag(&object_path));
    command.arg("-o").arg(not_a_flag(request.output));
    // **Search paths before the libraries**, which is what `ld` requires: a `-L` affects the `-l`s that
    // follow it, so emitting them the other way round would look right and find nothing.
    for path in request.library_paths {
        command.arg(format!("-L{}", path.display()));
    }
    // **Two forms, and the declaration says which** (ADR-0183 §1). `-lNAME` searches the `-L` paths;
    // `-framework NAME` asks the macOS linker for a framework bundle, and is two arguments rather than
    // one concatenated word — `-frameworkOpenGL` is not a thing `ld` accepts.
    //
    // No inference from the name, and no `-l` fallback after a failed `-framework`: the compiler cannot
    // know which a name means, and guessing would make `#system_library "SDL2"` on macOS try a framework
    // that does not exist before finding the dylib that does. The source says which, and after ADR-0184
    // the *declaration itself* is generated per OS — so no file carries a form that is wrong elsewhere.
    for library in request.libraries {
        match library.kind {
            LinkKind::Library => command.arg(format!("-l{}", library.name)),
            LinkKind::Framework => command
                .arg("-framework")
                .arg(not_a_flag_name(&library.name)),
        };
    }

    // **Last, so a script can override what this crate chose** — see `LinkRequest::linker_arguments`.
    for argument in request.linker_arguments {
        command.arg(argument);
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

    // **Only an executable is signed.** `ld64` ad-hoc signs a dylib too, and `codesign` on a static
    // archive is meaningless — it is not a Mach-O image.
    if request.kind == OutputKind::Executable {
        ensure_signed(request.output);
    }
    Ok(())
}

/// Archives one object into a static library with `ar`.
///
/// # Why `ar` and not the C driver
///
/// A static library is not linked. `cc` has no mode that produces one — `-c` stops at an object and there
/// is no `-static-lib` — so this is the one artefact kind that needs a different tool. `ar rcs` is POSIX
/// and behaves the same on macOS and Linux; the `s` writes the symbol index that a later `-lNAME` needs,
/// and omitting it produces an archive the linker silently finds no symbols in.
///
/// The archive is **removed first** rather than replaced, because `ar r` *updates* an existing archive: a
/// second build would leave the previous object in place beside the new one, which is a stale-symbol bug
/// that only shows up after a rename.
///
/// # Errors
/// [`LinkError`] when `ar` is missing or rejects the archive.
fn archive(object: &Path, output: &Path) -> Result<(), LinkError> {
    let _ = std::fs::remove_file(output);
    let result = Command::new("ar")
        .arg("rcs")
        .arg(not_a_flag(output))
        .arg(not_a_flag(object))
        .output();
    let output_bytes = match result {
        Ok(bytes) => bytes,
        // Reported as a missing driver rather than an io error, so a machine without binutils gets the
        // same shape of message it gets for a missing `cc`.
        Err(_) => return Err(LinkError::NoDriver(vec![String::from("ar")])),
    };
    if !output_bytes.status.success() {
        let mut text = String::from_utf8_lossy(&output_bytes.stderr).into_owned();
        text.push_str(&String::from_utf8_lossy(&output_bytes.stdout));
        return Err(LinkError::Failed {
            driver: String::from("ar"),
            status: output_bytes.status.to_string(),
            output: text,
        });
    }
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

/// A framework name that cannot be read as a flag (ADR-0183 §1).
///
/// `-framework` takes its name as a **separate argument**, so unlike a `-lNAME` — where the name is
/// concatenated and a leading `-` is harmless — a name beginning with `-` would be handed to `cc` as an
/// option of its own. `#framework "-rpath"` would then be a linker flag the source never asked for.
///
/// The path guard above solves the same problem by prefixing `./`, which is meaningless for a framework
/// name; so this refuses by emptying instead, which fails the link with `ld: framework not found` rather
/// than doing something. A refusal a reader can see beats an argument they cannot.
fn not_a_flag_name(name: &str) -> String {
    if name.starts_with('-') {
        return String::new();
    }
    name.to_owned()
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
