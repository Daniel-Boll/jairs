//! Running a program: assembling the whole reachable program and calling `main`.
//!
//! # Why the driver is here and not in `jr-cli`
//!
//! Executing a program needs three things `jr-db` owns and nobody else can reach: the
//! `Pool` (behind the database's mutex), every reachable file's MIR (through queries),
//! and each file's stable `FileId` (which a `ProcRef` is built from). Putting the
//! driver in `jr-cli` would mean exposing all three, and the pool in particular is
//! deliberately not public — `jr-sema`'s `lock_pool` recovers from a poisoned lock and
//! that recovery should have one caller, not two.
//!
//! So `jr-cli` asks for an outcome and renders it. That also means the LSP or a test
//! can run a program without going through the command line.
//!
//! # Why every reachable file goes into the program
//!
//! `jr_mir::Callee::Direct` names a `(FileId, ProcId)` pair (ADR-0018 §5), and the
//! interpreter resolves one by lookup. So a cross-file call — `024-hello.jr` calling
//! `print` from `modules/Basic` — only works if `Basic`'s bytecode is in the same
//! `Program`. The walk tolerates import cycles, which ADR-0014 §4 makes legal.

use std::sync::Arc;

use jr_hir::{ConstValue, ItemKind};
use jr_mir::ProcRef;
use jr_vm::{Mode, Program, Value, Vm, VmError};

use crate::{
    Db, SourceFile,
    mir::file_mir,
    module_loader::{ModuleSearchPaths, file_hir, imports_of, module_file},
};

/// How a program ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    /// `main` returned.
    Completed,
    /// The program called `exit` with this status.
    ///
    /// Not a failure: `modules/Basic` declares `exit` so that a program has a way out
    /// that does not depend on `main` returning, which the slice does not model yet.
    Exited(i64),
    /// The program trapped, or the VM refused to run it.
    ///
    /// One variant for both because the difference is in the message, and the caller's
    /// response — report it and fail — is the same. [`VmError`]'s own variants keep the
    /// distinction for anything that needs it.
    Failed(String),
}

/// Assembles every reachable file and calls `main`.
///
/// The caller is responsible for having checked the file first: ADR-0017 §4 forbids
/// MIR from a file with errors, so a file that does not check has no bytecode and this
/// finds no `main`.
///
/// # Errors
/// A message when the program cannot be assembled at all — no `main`, or a body that
/// will not compile. A program that runs and *fails* is `Ok(RunOutcome::Failed)`,
/// because that is the program's outcome rather than the compiler's.
pub fn run_main(
    db: &dyn Db,
    root: SourceFile,
    search_paths: ModuleSearchPaths,
) -> Result<RunOutcome, String> {
    let entry = main_of(db, root).ok_or_else(|| "the file declares no `main`".to_owned())?;

    let files = reachable(db, root, search_paths);

    // Gather every query result before locking the pool: the lock must never be held
    // across a nested query call, which is the rule the rest of this crate follows.
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
    let mut program = Program::new(jr_pool::TargetLayout::host());
    for (file_id, hir, mir, signatures) in &inputs {
        jr_vm::add_file(
            &mut program,
            *file_id,
            hir.as_ref(),
            mir.as_ref(),
            signatures.as_ref(),
            &pool,
        )
        .map_err(|e: VmError| e.to_string())?;
    }

    let mut vm = Vm::new(&program, &pool, Mode::Runtime).map_err(|e: VmError| e.to_string())?;
    Ok(match vm.call(entry, Vec::new()) {
        Ok(Value::Void | Value::Scalar(_) | Value::Aggregate(_) | Value::Undefined) => {
            RunOutcome::Completed
        }
        Err(VmError::Exited(status)) => RunOutcome::Exited(status),
        Err(error) => RunOutcome::Failed(error.to_string()),
    })
}

/// The `main` procedure of a file, if it declares one.
///
/// Looked up by name because Jairs-0 has no entry-point attribute, and on the *item*
/// rather than the procedure because procedures are constants (ADR-0012) — a `Proc`
/// carries no name of its own.
#[must_use]
pub fn main_of(db: &dyn Db, file: SourceFile) -> Option<ProcRef> {
    let hir = file_hir(db, file);
    let interner = db.interner();
    let proc = hir.items.iter().find_map(|item| {
        let ItemKind::Const {
            value: ConstValue::Proc(proc),
        } = &item.kind
        else {
            return None;
        };
        let name = item.name?;
        (interner.resolve(name) == "main").then_some(*proc)
    })?;
    Some(ProcRef::new(
        crate::queries::resolve_file_id(db, file),
        proc,
    ))
}

/// Every already-loaded file reachable from `root` through `#import`, including it.
fn reachable(db: &dyn Db, root: SourceFile, search_paths: ModuleSearchPaths) -> Vec<SourceFile> {
    let mut seen = vec![root];
    let mut queue = vec![root];
    while let Some(file) = queue.pop() {
        let own_path = file.path(db);
        for name in imports_of(db, file).iter() {
            let lookup = module_file(db, search_paths, Arc::clone(name));
            let Some(path) = lookup.found else { continue };
            let path = path.to_string_lossy();
            // A self-import is a no-op (ADR-0014 §6), and a cycle is legal (§4), so the
            // seen-set is what makes this terminate rather than an assumption.
            if path.as_ref() == own_path.as_ref() {
                continue;
            }
            if let Some(module) = db.source_file_for_path(path.as_ref()) {
                if !seen.contains(&module) {
                    seen.push(module);
                    queue.push(module);
                }
            }
        }
    }
    seen
}
