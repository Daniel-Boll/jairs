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
    BuildConfig, Db, SourceFile,
    mir::optimized_file_mir,
    module_loader::{ModuleSearchPaths, file_hir},
    run::main_of,
};

/// A native object, and what it must be linked against.
pub struct BuildOutput {
    /// The object file's bytes.
    pub object: Vec<u8>,
    /// The libraries every `#foreign` declaration named.
    pub libraries: Vec<String>,
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
        inputs.push((
            crate::queries::resolve_file_id(db, file),
            file_hir(db, file),
            mir.mir,
            crate::sema::file_signatures(db, file, search_paths).signatures,
        ));
    }

    let pool = crate::sema::lock_pool(db);
    // The target's layout, not the host's: a native build is for the target, and the
    // two are the same number today only because the slice cross-compiles nowhere
    // (ADR-0018 §2).
    let layout = TargetLayout::LP64;
    let mut backend = ClifBackend::new(&pool, "jairs").map_err(|e| e.to_string())?;

    // Phase 1: declare everything, so a cross-file call has a symbol to reference.
    for (file_id, hir, _, signatures) in &inputs {
        // The source name of every procedure this file binds, for a backtrace frame (ADR-0066 §3).
        // Built here because resolving a `Symbol` needs the interner, which `jr-codegen` has no
        // database to ask — the same reason a trap's *location* is resolved on this side (ADR-0020 §3).
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
    let map = db.source_map();
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

    let libraries = backend.libraries().to_vec();
    let object = Box::new(backend).finalise().map_err(|e| e.to_string())?;
    Ok(BuildOutput { object, libraries })
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
}

impl jr_codegen::TrapLocations for BodyLocations<'_> {
    fn location(&self, span: jr_mir::MirSpan) -> Option<String> {
        let span = jr_mir::resolve_span(self.hir, self.body, span)?;
        Some(jr_base::render_location(self.map, span))
    }
}
