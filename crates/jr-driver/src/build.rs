//! One compilation, described as data and performed on request.

use std::path::{Path, PathBuf};

use jr_base::SourceMap;
use jr_db::{BackendChoice, Db as _, JairsDatabase, OptLevel, build_object, file_diagnostics};
use jr_diag::{Diagnostics, Severity};
use jr_link::{LinkRequest, link};

/// The module a target reads `add_build_string` text through (ADR-0197 §3).
///
/// `Build`, because Jai calls the same thing "build constants" and a script generating them is doing what
/// `add_build_string` is for. Short, capitalised like every other module, and unlikely to collide — a
/// project with its own `Build` module can pass its directory later in the search order and win.
pub const GENERATED_MODULE: &str = "Build";

/// Everything one compilation needs, with no argument parser in the way.
///
/// Field order follows the order [`build`] consumes them, so a reader can follow the function
/// against the struct. Every value is **already decided**: precedence between a flag and a
/// declared constant is the caller's job (see the crate docs), because it is a statement about a
/// command line.
#[derive(Debug, Clone)]
pub struct BuildRequest {
    /// The root source file to compile.
    pub path: PathBuf,
    /// Directories to search for an `#import`, in order, used **exactly as given**.
    ///
    /// The bundled module directory is *not* appended here: where the compiler's own modules live is
    /// an installation question, and a crate that cannot see a command line has no business
    /// answering it. `jr-cli` appends it, and a build script inherits the list the operator gave.
    pub module_paths: Vec<PathBuf>,
    /// Directories to search for a `#system_library`, in order.
    pub library_paths: Vec<PathBuf>,
    /// The optimisation level, or `None` to let a declared `BUILD_OPT_LEVEL` decide and fall back
    /// to [`OptLevel::Standard`].
    ///
    /// Kept as an `Option` rather than resolved by the caller, because resolving it needs
    /// *compiling* the file — which is what this function does. ADR-0154 §1's bootstrap
    /// configuration is the reason that is sound.
    pub opt_level: Option<OptLevel>,
    /// Whether to emit bounds checks.
    pub bounds_checks: bool,
    /// Which code generator to use.
    pub backend: BackendChoice,
    /// The output path, or `None` to let a declared `BUILD_OUTPUT` decide and fall back to the
    /// root file's stem.
    ///
    /// An `Option` for the same reason as `opt_level`: the declared value is compiled out of the
    /// file. A caller that has an explicit `-o` puts it here, and it is used unchanged and
    /// **unchecked** — confinement applies only to a declared name (ADR-0122).
    pub output: Option<PathBuf>,
    /// Write the object file beside the output and stop, rather than linking.
    ///
    /// Kept beside `kind` rather than folded into it, because the two answer different questions: this is
    /// `jr build --emit-object`, an operator's debugging switch, and `kind` is what the *artefact* is. A
    /// script asking for an object says so with [`jr_link::OutputKind::Object`].
    pub emit_object: bool,
    /// Which kind of artefact to produce (ADR-0197 §1).
    pub kind: jr_link::OutputKind,
    /// Extra arguments for the C driver, after everything the compiler generates.
    pub linker_arguments: Vec<String>,
    /// Extra source text to compile into this target, concatenated into one generated module
    /// (ADR-0197 §3).
    ///
    /// Jai's `add_build_string`, adapted — and the adaptation is forced rather than chosen. Jai injects the
    /// text into the **workspace's shared global scope**, so a generated `VERSION :: "abc";` is simply
    /// visible to every file. Jairs has no shared global scope: a name crosses a file boundary only through
    /// `#import` (ADR-0014 §2). So the text becomes a module named `Build`, and the target reads it with
    /// `Build :: #import "Build";`.
    ///
    /// That is not a workaround for a missing feature; it is the same feature in a language where files
    /// have scopes. What it costs is one line in the target, and what it buys is that a generated name
    /// cannot silently shadow or collide with one the program wrote — which in Jai it can.
    pub build_strings: Vec<String>,
    /// Extra module lookups, from module name to directory (ADR-0197 §5).
    ///
    /// Jai's `provide_import`, in the form that needs no message loop: a mapping supplied *before* the
    /// compilation rather than in answer to a `FAILED_IMPORT`. Appended to `module_paths`, so a name
    /// resolves the ordinary way and nothing new has to understand it.
    pub provided_imports: Vec<(String, PathBuf)>,
}

/// What one successful compilation produced.
#[derive(Debug, Clone)]
pub struct Built {
    /// Where the artefact was written.
    pub output: PathBuf,
    /// The optimisation level actually used, after a declared `BUILD_OPT_LEVEL` was consulted.
    /// Reported so a caller can say what it did without recomputing the decision.
    pub opt_level: OptLevel,
}

/// How one compilation ended.
///
/// Three variants rather than a `Result<Built, String>`, because the three have genuinely
/// different consequences for a caller: source errors come with *diagnostics to render*, a
/// build failure comes with one sentence, and a success comes with a path. Collapsing the first
/// two would force every caller to render diagnostics it might not want to.
#[derive(Debug)]
pub enum BuildOutcome {
    /// The program was compiled and written.
    Built(Built),
    /// The source did not check. The diagnostics are every reachable file's, not only the root's
    /// (ADR-0108 §1), and the caller renders them.
    Rejected {
        /// Every diagnostic from every reachable file.
        diagnostics: Diagnostics,
        /// The source map to render them against.
        map: SourceMap,
    },
    /// The source was accepted and the compiler could not finish: no `main`, a type with no
    /// layout, a construct a back end does not implement, or a link failure.
    Failed(String),
}

/// Performs one compilation.
///
/// # Errors
/// When the root file cannot be read, or the database cannot register it. Both are the caller's
/// problem rather than the program's, which is why they are an `Err` and not a [`BuildOutcome`].
pub fn build(request: &BuildRequest) -> Result<BuildOutcome, String> {
    let mut db = JairsDatabase::default();

    let mut module_paths = request.module_paths.clone();

    // **A generated `Build` module, when the script supplied any text** (ADR-0197 §3). Written to a
    // temporary directory rather than beside the source, because it is a product of *this* build: leaving
    // it in the tree would make a second build see a stale one, and `git status` show a file nobody wrote.
    //
    // The guard outlives the compilation because `TempDir` removes the directory when dropped, and the
    // module has to still be readable while `file_hir` runs.
    let generated = if request.build_strings.is_empty() {
        None
    } else {
        let dir = tempfile::tempdir()
            .map_err(|e| format!("cannot create a directory for the generated module: {e}"))?;
        let module_dir = dir.path().join(GENERATED_MODULE);
        std::fs::create_dir_all(&module_dir)
            .map_err(|e| format!("cannot create the generated module's directory: {e}"))?;
        // Joined with a blank line between entries, so two strings cannot accidentally continue one
        // another's last line — `A :: 1;` and `B :: 2;` written adjacently would otherwise be one line and
        // one span.
        let text = request.build_strings.join("\n\n");
        std::fs::write(module_dir.join("module.jr"), text)
            .map_err(|e| format!("cannot write the generated module: {e}"))?;
        module_paths.push(dir.path().to_path_buf());
        Some(dir)
    };
    // Named so the guard is not dropped early; the directory must outlive every query below.
    let _generated = generated;

    // **Provided imports are search directories, validated** (ADR-0197 §5). Jai answers a `FAILED_IMPORT`
    // message with `provide_import(…, .PATH_TO_DIRECTORY, dir)`, so the mechanism is a directory either
    // way — but Jai only ever calls it for a module that *did* fail, which means the name is checked by
    // construction. Here it is supplied up front, so the check has to be explicit or the name would be
    // decoration and "I provided `Foo` and the import still failed" would have no explanation.
    for (name, dir) in &request.provided_imports {
        let directory_form = dir.join(name).join("module.jr");
        let file_form = dir.join(format!("{name}.jr"));
        if !directory_form.is_file() && !file_form.is_file() {
            return Ok(BuildOutcome::Failed(format!(
                "`{name}` was provided from {}, which contains neither {} nor {}",
                dir.display(),
                directory_form.display(),
                file_form.display()
            )));
        }
        module_paths.push(dir.clone());
    }

    let search = db.set_module_search_paths(module_paths);

    // **A bootstrap configuration first**, then the real one (ADR-0154 §1). A build script may
    // declare `BUILD_OPT_LEVEL`, and reading a declared constant means *compiling* — so an option
    // that affects compilation cannot be read without already having chosen one.
    //
    // Sound because of ADR-0142's check, not by assumption: every corpus program behaves
    // identically at both levels, so a constant read at one level has the same value at the other.
    let _bootstrap = db.set_build_config(request.bounds_checks, OptLevel::Standard);

    let text = std::fs::read_to_string(&request.path)
        .map_err(|e| format!("cannot read {}: {e}", request.path.display()))?;
    let key = request.path.to_string_lossy().into_owned();
    let _ = db.set_file_text(key.clone(), text);
    let root = db
        .source_file(&key)
        .ok_or_else(|| format!("internal error: {key} was not registered"))?;
    db.load_modules_transitively(root);

    let level = match request.opt_level {
        Some(explicit) => explicit,
        None => jr_db::declared_opt_level(&db, root, search).unwrap_or(OptLevel::Standard),
    };
    let config = db.set_build_config(request.bounds_checks, level);

    // **Every reachable file, not only the root** (ADR-0108 §1): a root whose imported module was
    // broken used to pass this gate and fail *inside the engine*. The reachable set is the same one
    // the MIR assembly walks, so this adds no query and cannot disagree with what is compiled.
    let mut diagnostics = Diagnostics::new();
    for file in jr_db::reachable_files(&db, root, search) {
        diagnostics.extend(file_diagnostics(&db, file, search).iter().cloned());
    }
    if diagnostics.iter().any(|d| d.severity == Severity::Error) {
        return Ok(BuildOutcome::Rejected {
            diagnostics,
            map: db.source_map(),
        });
    }

    // **What compile-time code printed, before the artefact is reported** (ADR-0196 §2). A `#run` that
    // prints has already run by now — the gate above forced every constant to a value — so emitting here
    // puts its output ahead of anything the build says about itself, which is the order it happened in.
    //
    // To stderr, like a diagnostic, rather than stdout: a build's stdout may be piped somewhere that
    // expects only the compiler's own machine-readable output, and a `#run`'s printing is commentary.
    let printed = jr_db::comptime_output(&db, root, search);
    if !printed.is_empty() {
        eprint!("{}", String::from_utf8_lossy(&printed));
    }

    // `--emit-object` is the operator's switch and reaches the same place a script's
    // `OutputKind::Object` does, so there is one path that writes an object rather than two.
    let kind = if request.emit_object {
        jr_link::OutputKind::Object
    } else {
        request.kind
    };

    // **Only an executable needs a `main`, and only an executable gets a shim** (ADR-0197 §1). A library
    // must not have one at all: a static archive containing `main` fails a C link with `duplicate symbol`.
    //
    // An object is *optional*, and that third state is why this is not a `bool` — found by a test, because
    // a library asked for as an object has no `main` and `--emit-object` on a program has always produced
    // one with an entry. Both are legitimate, so the object path uses a `main` when there is one.
    let policy = match kind {
        jr_link::OutputKind::Executable => jr_db::EntryPolicy::Required,
        jr_link::OutputKind::Object => jr_db::EntryPolicy::Optional,
        jr_link::OutputKind::Dynamic | jr_link::OutputKind::Static => jr_db::EntryPolicy::None,
    };

    let built = match build_object(&db, root, search, config, request.backend, policy) {
        Ok(built) => built,
        Err(message) => return Ok(BuildOutcome::Failed(message)),
    };

    // **A build script is not something you compile** (ADR-0195 §5). Reaching
    // `modules/Compiler` means the file's vocabulary is the *driver's*, and those symbols exist in no
    // library — so the link failed with a wall of `ld` output naming `_add_file` and `_jr$2$17`, which
    // tells a reader nothing about the mistake they actually made, which is a missing `--script`.
    //
    // Checked after code generation rather than before, because that is where the declared libraries
    // are known: the check costs one scan of a list that already exists, and doing it earlier would
    // mean resolving `#foreign` declarations a second time — the duplication ADR-0018 §4 warns about.
    if built
        .libraries
        .iter()
        .any(|library| library.kind == jr_db::LinkKind::Compiler)
    {
        return Ok(BuildOutcome::Failed(String::from(
            "this program imports `modules/Compiler`, so it is a build script rather than a program to compile \
             — run it with `jr build --script` instead",
        )));
    }

    let output = match request.output.clone() {
        Some(explicit) => explicit,
        None => match jr_db::declared_build_output(&db, root, search) {
            Some(declared) => match confined_output(&declared) {
                Ok(path) => path,
                Err(reason) => {
                    return Ok(BuildOutcome::Failed(format!(
                        "the declared BUILD_OUTPUT {declared:?} is not a usable output name: {reason}"
                    )));
                }
            },
            None => request.path.with_extension(""),
        },
    };

    // **The extension is applied here, not in `jr-link`**, because whether to override a name the caller
    // chose is a policy and that crate has none. The policy: add the extension when the name has none, and
    // leave a name that already has one alone — a script asking for `libfoo.dylib` gets exactly that, and
    // one asking for `libfoo` gets the platform's.
    let output = match kind.extension() {
        Some(extension) if output.extension().is_none() => output.with_extension(extension),
        _ => output,
    };

    // **The one conversion between the compiler's link vocabulary and the linker's** (ADR-0183 §1).
    // `jr-link` declares its own `LinkKind` because it has no dependencies — ADR-0009's seam — so
    // the translation happens at the driver, where both types are visible. Exhaustive, so a third
    // link form is a compile error on this line rather than a silent `-l`.
    let libraries: Vec<jr_link::LinkLibrary> = built
        .libraries
        .iter()
        .filter_map(|library| {
            let kind = match library.kind {
                jr_db::LinkKind::Library => jr_link::LinkKind::Library,
                jr_db::LinkKind::Framework => jr_link::LinkKind::Framework,
                // **The compiler is not a library and contributes no argument** (ADR-0195 §4). A
                // build script's `#foreign compiler "…"` is forwarded by the VM, so there is no
                // symbol for `ld` to find and nothing to ask it for — `-lcompiler` would fail the
                // link of any file that imported `modules/Compiler`, including one that only
                // *mentions* it.
                //
                // Dropped here rather than upstream because this is the one place the compiler's link
                // vocabulary meets the linker's, and a reader asking "what does this kind link as?"
                // should find the answer in one match.
                jr_db::LinkKind::Compiler => return None,
            };
            Some(jr_link::LinkLibrary {
                name: library.name.clone(),
                kind,
            })
        })
        .collect();

    if let Err(error) = link(&LinkRequest {
        kind,
        linker_arguments: &request.linker_arguments,
        object: &built.object,
        output: &output,
        libraries: &libraries,
        library_paths: &request.library_paths,
    }) {
        return Ok(BuildOutcome::Failed(error.to_string()));
    }

    Ok(BuildOutcome::Built(Built {
        output,
        opt_level: level,
    }))
}

/// Checks a **declared** `BUILD_OUTPUT` and turns it into a path, or says why not (ADR-0122).
///
/// `BUILD_OUTPUT :: #run choose_name();` lets the program name its own artefact (ADR-0102), and the
/// value is computed by arbitrary compile-time code *in the file being compiled*. So it is
/// attacker-controlled exactly when the source is — the ordinary case for a compiler, since someone
/// builds code they did not write. Nothing checked it, and the consequences were not subtle:
///
/// - an **absolute** path, or one climbing out with `..`, made `jr build` write an executable
///   anywhere the user could — `.git/hooks/pre-commit` being the sharp example, since git runs it on
///   the next commit;
/// - a leading `-` was passed to `cc` as its **first positional argument** and to `codesign` as its
///   last, so it was read as a flag rather than a path.
///
/// Only a *declared* name is checked. An explicit `-o` is not, because that is a person at a
/// terminal saying where they want the file, and second-guessing them would make the flag less
/// useful than a shell redirection — the same reasoning that lets `-o` beat the declaration.
///
/// Relative subdirectories stay legal (`build/app`), because naming one is an ordinary thing for a
/// build script to do and forbidding it would push people back to `-o`. Confinement is by rejecting
/// anything that *leaves* the working directory, not by flattening the name.
///
/// # Errors
/// A sentence naming what is wrong, for the caller to print.
pub(crate) fn confined_output(declared: &str) -> Result<PathBuf, String> {
    use std::path::Component;

    if declared.is_empty() {
        return Err("it is empty".to_owned());
    }
    if declared.contains('\0') {
        return Err("it contains a NUL byte".to_owned());
    }
    // Checked on the string rather than on a component, because it is `cc`'s argument parser that
    // will read a leading `-` as a flag, and that sees the whole path.
    if declared.starts_with('-') {
        return Err(
            "it starts with `-`, which a linker reads as a flag rather than a path".to_owned(),
        );
    }

    let path = Path::new(declared);
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                return Err(
                    "it is an absolute path, and a build writes inside the working directory"
                        .to_owned(),
                );
            }
            Component::ParentDir => {
                return Err("it climbs out of the working directory with `..`".to_owned());
            }
            Component::CurDir | Component::Normal(_) => {}
        }
    }
    // Every component was `.` — `"."` or `"./."` — so there is no file to write.
    if path.file_name().is_none() {
        return Err("it names a directory rather than a file".to_owned());
    }
    Ok(path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::confined_output;

    // **These moved here with the function** (ADR-0195 §1). They were `jr-cli`'s, and the wave that
    // split the driver out dropped them: the workspace count went 1082 to 1081 while *adding* six
    // tests, which is exactly the silent loss of coverage AGENTS.md tracks the number for. Every one
    // guards a real escape ADR-0122 found, so losing them would have been the worst possible seven.

    #[test]
    fn an_ordinary_name_is_accepted() {
        assert!(confined_output("app").is_ok());
        assert!(confined_output("my-app").is_ok());
        assert!(confined_output("./app").is_ok());
    }

    #[test]
    fn a_relative_subdirectory_stays_legal() {
        // Naming a subdirectory is an ordinary thing for a build script to do, and forbidding it would push
        // people back to `-o` — confinement is about *leaving* the working directory, not about flattening.
        assert!(confined_output("build/app").is_ok());
        assert!(confined_output("target/release/app").is_ok());
    }

    #[test]
    fn an_absolute_path_is_refused() {
        let reason = confined_output("/tmp/app").expect_err("an absolute path escapes the build");
        assert!(reason.contains("absolute"), "{reason}");
    }

    #[test]
    fn climbing_out_is_refused() {
        // The sharp case: git runs `.git/hooks/pre-commit` on the next commit, so writing an executable
        // there turns "I compiled someone's file" into "I ran their code".
        let reason = confined_output("../../.git/hooks/pre-commit")
            .expect_err("`..` escapes the working directory");
        assert!(reason.contains(".."), "{reason}");
    }

    #[test]
    fn a_leading_dash_is_refused() {
        // `jr-link` passes the object path as `cc`'s **first positional argument**, so a leading `-` is read
        // as a flag. Checked on the whole string rather than a component, because that is what `cc` parses.
        let reason =
            confined_output("-Wl,--version").expect_err("a linker reads a leading `-` as a flag");
        assert!(reason.contains("flag"), "{reason}");
    }

    #[test]
    fn an_empty_or_directory_name_is_refused() {
        assert!(confined_output("").is_err());
        assert!(confined_output(".").is_err());
    }

    #[test]
    fn a_nul_byte_is_refused() {
        // Rust strings admit NUL; the OS boundary does not. Rejected here so the message names the cause
        // rather than surfacing as an opaque io error.
        assert!(confined_output("app\0evil").is_err());
    }
}
