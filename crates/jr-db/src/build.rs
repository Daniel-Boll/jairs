//! Building a native executable: declaring every reachable procedure, then defining
//! every body.
//!
//! # Why the driver is here and not in `jr-cli`
//!
//! The same three reasons `run_main` is here: the `Pool` lives behind the database's
//! mutex, every reachable file's MIR comes from a query, and a `ProcRef` is built
//! from a stable `FileId` that only this crate can resolve. A back end is a pure
//! function over those, so this module is the fold that feeds it and `jr-cli` renders
//! the outcome.
//!
//! # Why the phases are separate
//!
//! ADR-0019 §1 made a back end *declare* every signature before *defining* any body,
//! and this is the loop that pays for it: `Callee::Direct` names a `(FileId, ProcId)`
//! pair (ADR-0018 §5), so `024-hello.jr` calling `print` from `modules/Basic` is a
//! reference to a procedure in a different file. Declaring the whole program first
//! means that call resolves to a real symbol when the body is generated, with no
//! second pass and no patch-up list.

use jr_codegen::{Backend, FileInput, declarations};
use jr_codegen_clif::ClifBackend;
use jr_hir::ProcId;
use jr_pool::TargetLayout;

use crate::{
    BuildConfig, Db, SourceFile, mir::optimized_file_mir, module_loader::ModuleSearchPaths,
    run::main_of,
};

/// A native object, and what it must be linked against.
pub struct BuildOutput {
    /// The object file's bytes.
    pub object: Vec<u8>,
    /// The libraries every `#foreign` declaration named.
    pub libraries: Vec<String>,
}

/// Which code generator turns MIR into machine code (ADR-0143 §2).
///
/// **Not a [`BuildConfig`] field.** The choice changes no query result — `optimized_file_mir`
/// is upstream of code generation — so making it a salsa input would invalidate every MIR
/// memo when it changed, for nothing. That is ADR-0058 §2's reasoning applied in the other
/// direction: an input is for configuration a *query* must see.
///
/// Both variants exist whatever features are compiled in, so that a build without LLVM
/// support refuses [`BackendChoice::Llvm`] with a message naming the feature rather than
/// reporting an unknown argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackendChoice {
    /// Cranelift, the verified back end and the default (ADR-0009).
    #[default]
    Cranelift,
    /// LLVM through `inkwell`, available when `jr-codegen-llvm`'s `llvm` feature is on.
    Llvm,
}

/// Compiles every reachable file into one native object.
///
/// The caller is responsible for having checked the file first, exactly as with
/// `run_main`: ADR-0017 §4 forbids MIR from a file with errors, so a file that does
/// not check has no bodies to define.
///
/// # Errors
/// A message when the program cannot be built — no `main`, a type with no layout, or
/// a construct the back end does not implement. Rendered as a string because the
/// caller reports it and stops; [`jr_codegen::CodegenError`] keeps the structure for
/// anything that needs it.
pub fn build_object(
    db: &dyn Db,
    root: SourceFile,
    search_paths: ModuleSearchPaths,
    config: BuildConfig,
    choice: BackendChoice,
) -> Result<BuildOutput, String> {
    let entry = main_of(db, root).ok_or_else(|| "the file declares no `main`".to_owned())?;

    let files = crate::run::reachable_files(db, root, search_paths);

    // Every query result is gathered before the pool is locked, because the lock must
    // never be held across a nested query call.
    let mut inputs = Vec::with_capacity(files.len());
    for file in files {
        let mir = optimized_file_mir(db, file, search_paths, config);
        if mir.gated {
            continue;
        }
        // The **expanded** HIR and signatures the MIR was lowered from (ADR-0082 §2): an instantiation
        // appended procedures, so the declare phase must see them or the define phase (which reads the
        // expanded MIR) declares a body for a procedure the declare phase never announced —
        // "defined without being declared".
        inputs.push((
            crate::queries::resolve_file_id(db, file),
            mir.hir.clone(),
            mir.mir.clone(),
            mir.signatures.clone(),
        ));
    }

    let pool = crate::sema::read_pool(db);
    // The target's layout, not the host's: a native build is for the target, and the
    // two are the same number today only because the slice cross-compiles nowhere
    // (ADR-0018 §2).
    let layout = TargetLayout::LP64;
    let map = db.source_map();

    // ADR-0019 §1's two phases, over `&mut dyn Backend` so that one loop feeds either back
    // end (ADR-0143 §2). Duplicating it per back end would be two chances to declare a
    // different set of procedures than the one whose bodies are defined — "defined without
    // being declared", the failure the phase split exists to prevent.
    let drive = |backend: &mut dyn Backend| -> Result<(), String> {
        // Phase 1: declare everything, so a cross-file call has a symbol to reference.
        for (file_id, hir, _, signatures) in &inputs {
            // The source name of every procedure this file binds, for a backtrace frame
            // (ADR-0066 §3). Built here because resolving a `Symbol` needs the interner, which
            // `jr-codegen` has no database to ask — the same reason a trap's *location* is
            // resolved on this side (ADR-0020 §3).
            let names = proc_names(db, hir.as_ref());
            let input = FileInput {
                file: *file_id,
                hir: hir.as_ref(),
                signatures: signatures.as_ref(),
                names: &names,
            };
            let own_entry = (*file_id == entry.file).then_some(entry.proc);
            for decl in declarations(&input, &pool, own_entry) {
                backend
                    .declare(&decl, &pool, layout)
                    .map_err(|e| e.to_string())?;
            }
        }

        // Phase 2: define every body MIR produced. A body MIR refused is skipped rather
        // than reported: the refusal is ADR-0017 §4 working, and something upstream
        // already reported the cause.
        for (file_id, hir, mir, _) in &inputs {
            for (proc, outcome) in mir.iter() {
                let Ok(body) = outcome else { continue };
                let reference = jr_mir::ProcRef::new(*file_id, proc);
                // The back end holds a `MirSpan` at every trap site and can resolve
                // none of them: that needs the file's HIR and a source map, neither of
                // which ADR-0009 lets it see. So the resolution happens here and arrives
                // as text (ADR-0020 §3).
                let locations = BodyLocations {
                    hir: hir.as_ref(),
                    map: &map,
                    interner: db.interner(),
                    body: hir
                        .procs
                        .get(proc.index())
                        .and_then(|data| data.body)
                        .and_then(|id| hir.bodies.get(id.index())),
                };
                backend
                    .define(reference, body, &pool, layout, &locations)
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    };

    match choice {
        BackendChoice::Cranelift => {
            let mut backend =
                ClifBackend::new(&pool, layout, "jairs").map_err(|e| e.to_string())?;
            drive(&mut backend)?;
            let libraries = backend.libraries().to_vec();
            let object = Box::new(backend).finalise().map_err(|e| e.to_string())?;
            Ok(BuildOutput { object, libraries })
        }
        // The LLVM back end's values borrow an `inkwell::Context`, and naming one here would
        // put an `inkwell` type in this crate — which ADR-0009's confinement forbids. So the
        // back end's own crate owns the context and takes the loop (ADR-0143 §2).
        #[cfg(feature = "llvm")]
        BackendChoice::Llvm => {
            let (object, libraries) = jr_codegen_llvm::build(&pool, layout, "jairs", &drive)?;
            Ok(BuildOutput { object, libraries })
        }
        #[cfg(not(feature = "llvm"))]
        BackendChoice::Llvm => Err(
            "this compiler was built without LLVM support; rebuild with `--features llvm`"
                .to_owned(),
        ),
    }
}

/// The `main` procedure's own [`ProcId`], for a caller that needs it separately.
///
/// A thin wrapper so `jr-cli` does not have to know that an entry point is found by
/// name (Jairs-0 has no entry-point attribute) or that procedures are constants
/// (ADR-0012) and so carry no name of their own.
#[must_use]
pub fn entry_of(db: &dyn Db, root: SourceFile) -> Option<ProcId> {
    main_of(db, root).map(|reference| reference.proc)
}

/// The source name of every procedure a file binds, indexed by [`ProcId`] (ADR-0066 §3).
///
/// Procedures are constants (ADR-0012), so a `Proc` carries no name of its own and the name lives on
/// the item binding it — the same walk `main_of` does, done once per file instead of once per lookup
/// because a backtrace may name any of them.
///
/// A slice parallel to `hir.procs` rather than a map, matching what `declarations` iterates: an entry
/// is `None` for a procedure no item binds, and its frame is then omitted rather than printed as a
/// placeholder, because an unnamed line in a backtrace tells a reader nothing.
fn proc_names(db: &dyn Db, hir: &jr_hir::FileHir) -> Vec<Option<String>> {
    let interner = db.interner();
    let mut out = vec![None; hir.procs.len()];
    for item in &hir.items {
        let jr_hir::ItemKind::Const {
            value: jr_hir::ConstValue::Proc(proc),
        } = &item.kind
        else {
            continue;
        };
        if let Some(name) = item.name
            && let Some(slot) = out.get_mut(proc.index())
        {
            *slot = Some(interner.resolve(name).to_owned());
        }
    }
    out
}

/// Resolves the trap locations of one body.
///
/// Built per body because `jr_mir::resolve_span` needs the HIR `Body` a `MirSpan`'s
/// expression, local and statement ids index into — the arena trap ADR-0017 records:
/// every body's arenas start at 0, so a span means nothing without knowing whose it
/// is.
struct BodyLocations<'a> {
    hir: &'a jr_hir::FileHir,
    body: Option<&'a jr_hir::Body>,
    map: &'a jr_base::SourceMap,
    /// For resolving a field's `Symbol` to text, which a struct's DWARF member entry needs (ADR-0171 §2).
    interner: &'a jr_base::Interner,
}

impl jr_codegen::SourceInfo for BodyLocations<'_> {
    fn position(&self, span: jr_mir::MirSpan) -> Option<jr_codegen::SourcePosition> {
        let span = jr_mir::resolve_span(self.hir, self.body, span)?;
        let file = self.map.file(span.file);
        let at = file.line_col(span.start());
        Some(jr_codegen::SourcePosition {
            path: file.path().display().to_string(),
            line: at.line,
            column: at.col,
        })
    }

    fn symbol(&self, symbol: jr_base::Symbol) -> Option<String> {
        // The driver has the interner; the back end does not, which is the whole reason this trait exists
        // (ADR-0171 §2). `resolve` is infallible on a symbol this interner minted, and every symbol a back
        // end can hold came from a pool this database filled — so `Some` is not optimism.
        Some(self.interner.resolve(symbol).to_owned())
    }
}

/// The output name a build script declared, if the file declares one (ADR-0102 §1).
///
/// `BUILD_OUTPUT :: #run choose_name();` — or a plain string constant — names the artefact `jr build` writes,
/// which is the makefile's most basic job and the smallest thing that makes PLAN §2.1's "a build script
/// replaces the makefile" true of something.
///
/// **A declared constant rather than an intrinsic call** (`set_build_output("app")`). A call has to *happen*,
/// so its effect depends on evaluation order and on the script being reached at all, while a declared constant
/// is a fact about the file. Order-dependent configuration is the failure mode makefiles are notorious for,
/// and it would be strange to import it into their replacement.
///
/// `None` when the file declares no such constant, when it is not a `string`, or when it did not evaluate. All
/// three mean "the driver decides", and none is an error here: a non-`string` is already a type error at its
/// own declaration, and reporting it again from the driver would say the same thing twice in a worse place.
///
/// Read from `file_consts`, so a `#run` computing the name works exactly as a literal does — there is no
/// second path for the computed case, which is what ADR-0073 bought.
pub fn declared_build_output(
    db: &dyn Db,
    root: SourceFile,
    search_paths: ModuleSearchPaths,
) -> Option<String> {
    let hir = crate::file_hir(db, root);
    let name = db.interner().intern(BUILD_OUTPUT);
    let item = hir.items.iter().enumerate().find_map(|(index, item)| {
        (item.name == Some(name) && matches!(item.kind, jr_hir::ItemKind::Const { .. }))
            .then_some(jr_hir::ItemId::from_usize(index))
    })?;
    let consts = crate::consts::file_consts(db, root, search_paths);
    let value = consts.values.item(item)?;
    let pool = crate::sema::read_pool(db);
    match pool.item(value) {
        jr_pool::Item::StrValue(id) => Some(pool.resolve_str(*id).to_owned()),
        _ => None,
    }
}

/// The optimisation level a build script declared, if any (ADR-0154 §1).
///
/// `BUILD_OPT_LEVEL :: 0;` — the **second** build option, and the one that makes PLAN §2.1's "a build
/// script replaces the makefile" true of more than a filename: naming the artefact and choosing the
/// optimisation are the two things every makefile does.
///
/// # Why a second constant rather than a `Build_Options` struct
///
/// ADR-0102 §3 deferred a struct until there were enough options to justify one, and probing while
/// writing this wave found a harder reason to keep waiting: **this language has no struct literals**.
/// `BUILD :: Build_Options.{ output = "app", opt_level = 1 };` does not parse — E0117, "expected a field
/// name after `.`" — so the struct form is blocked on a language feature rather than on a judgement about
/// how many options is enough. That is now a named blocker instead of a vague threshold (ADR-0154 §2).
///
/// # Errors and absent values
///
/// `None` when the constant is not declared, is not an integer, or is not a level this compiler has. A
/// *wrong* value is deliberately `None` rather than an error: the same asymmetry ADR-0102 §2 established
/// for `-o`, where an operator's instruction outranks an artefact's declaration, means a bad declaration
/// falls back to the default rather than stopping a build the operator asked for.
pub fn declared_opt_level(
    db: &dyn Db,
    root: SourceFile,
    search_paths: ModuleSearchPaths,
) -> Option<crate::OptLevel> {
    let hir = crate::file_hir(db, root);
    let name = db.interner().intern(BUILD_OPT_LEVEL);
    let item = hir.items.iter().enumerate().find_map(|(index, item)| {
        (item.name == Some(name) && matches!(item.kind, jr_hir::ItemKind::Const { .. }))
            .then_some(jr_hir::ItemId::from_usize(index))
    })?;
    let consts = crate::consts::file_consts(db, root, search_paths);
    let value = consts.values.item(item)?;
    let pool = crate::sema::read_pool(db);
    let jr_pool::Item::IntValue { bits, .. } = pool.item(value) else {
        return None;
    };
    match bits {
        0 => Some(crate::OptLevel::Off),
        1 => Some(crate::OptLevel::Standard),
        // Any other number names no level. `None`, so the default applies, for the reason in the doc
        // comment: a declaration is the artefact's preference and must not stop the operator's build.
        _ => None,
    }
}

/// The name of the constant [`declared_opt_level`] reads.
const BUILD_OPT_LEVEL: &str = "BUILD_OPT_LEVEL";

/// The name of the constant [`declared_build_output`] reads.
///
/// A screaming-case name because it *is* a constant, and one the compiler knows: a lowercase `build_output`
/// would read like an ordinary local and give no hint that the driver is watching it.
const BUILD_OUTPUT: &str = "BUILD_OUTPUT";
