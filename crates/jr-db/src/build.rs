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
    Db, SourceFile,
    mir::file_mir,
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
) -> Result<BuildOutput, String> {
    let entry = main_of(db, root).ok_or_else(|| "the file declares no `main`".to_owned())?;

    let files = crate::run::reachable_files(db, root, search_paths);

    // Every query result is gathered before the pool is locked, because the lock must
    // never be held across a nested query call.
    let mut inputs = Vec::with_capacity(files.len());
    for file in files {
        let mir = file_mir(db, file, search_paths);
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
        let input = FileInput {
            file: *file_id,
            hir: hir.as_ref(),
            signatures: signatures.as_ref(),
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
    for (file_id, _, mir, _) in &inputs {
        for (proc, outcome) in mir.iter() {
            let Ok(body) = outcome else { continue };
            let reference = jr_mir::ProcRef::new(*file_id, proc);
            backend
                .define(reference, body, &pool, layout)
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
