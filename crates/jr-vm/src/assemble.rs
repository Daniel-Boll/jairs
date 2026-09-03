//! Building a [`Program`] from one file's HIR and MIR.
//!
//! # Why this is here and not in `jr-db`
//!
//! Assembling a program is a fold over HIR, MIR and signatures with no queries in
//! it, and both consumers need exactly the same fold: `jr-db` for the real compiler,
//! and this crate's own tests for a single file with no database at all. Putting it
//! in `jr-db` would make the VM only testable through salsa, which is the same
//! argument `jr-mir`'s test harness makes for keeping lowering a pure function.
//!
//! # Where a `#foreign` procedure's library comes from
//!
//! From [`FileSignatures::foreign_library`], and nowhere else.
//!
//! `ForeignInfo::library` is a [`jr_base::Symbol`] naming the *constant* — `libc` —
//! not the library, and getting from there to `"c"` means finding the item that
//! constant declares and reading its `#system_library` directive. This crate used to
//! perform that walk itself, which made it the second independent implementation of
//! one lookup; ADR-0018 §4 recorded that as accepted debt with a trigger, namely a
//! third consumer.
//!
//! The native back end is that third consumer, so ADR-0019 §4 fired the trigger: the
//! walk now happens once, in `jr-sema`'s signature phase, and the answer is interned
//! in the pool. Read the string with [`Pool::foreign_library_name`]. `None` still
//! means *refuse to guess* rather than "assume libc".

use jr_base::FileId;
use jr_hir::{FileHir, ProcId};
use jr_mir::{FileMir, GlobalRef, ProcRef};
use jr_pool::{Pool, TargetLayout};
use jr_sema::FileSignatures;

use crate::code::{ForeignProc, Routine};
use crate::error::VmError;
use crate::interp::Program;
use crate::lower::compile_in_file;

/// Adds every routine one file provides to `program`.
///
/// Bodies that MIR refused are skipped rather than reported: a refusal is
/// ADR-0017 §4 working, and a program that never calls the refused procedure runs
/// fine. Calling one produces [`VmError::Internal`] from the interpreter's own
/// lookup, which names the procedure — a better error than a wall of refusals at
/// assembly time for procedures nobody wanted.
///
/// # Errors
/// [`VmError::Internal`] when a body cannot be compiled because MIR, the pool and
/// the layout disagree. That is a compiler bug, not a program one.
pub fn add_file(
    program: &mut Program,
    file: FileId,
    hir: &FileHir,
    mir: &FileMir,
    signatures: &FileSignatures,
    pool: &Pool,
) -> Result<(), VmError> {
    for (_proc, outcome) in mir.iter() {
        if let Ok(body) = outcome {
            program.insert(Routine::Bytecode(compile_in_file(
                body,
                mir,
                pool,
                program.target(),
            )?));
        }
    }

    for (item, data) in mir.globals() {
        program.insert_global(GlobalRef::new(file, item), data);
    }

    for index in 0..hir.procs.len() {
        let proc = ProcId::from_usize(index);
        let data = &hir.procs[index];
        let Some(info) = &data.foreign else {
            continue;
        };
        let Some(symbol) = info.symbol.clone() else {
            // `#foreign` with no symbol string is a syntax error sema already
            // reported; leaving the routine out means a call to it fails by name.
            continue;
        };
        let Some(sig) = signatures.proc_sig(proc) else {
            continue;
        };
        program.insert(Routine::Foreign(ForeignProc {
            proc: ProcRef::new(file, proc),
            symbol,
            library: signatures
                .foreign_library(proc)
                .and_then(|id| pool.foreign_library_name(id))
                .map(str::to_owned),
            params: sig.params.clone(),
            ret: sig.ret,
        }));
    }

    Ok(())
}

/// An empty program for the host's own layout.
///
/// Comptime evaluation wants the *host* layout, not the target's: a `#run` that takes
/// an address is manipulating a pointer inside this process. `TargetLayout::host`
/// documents the distinction, which is the same number today only because the slice
/// cross-compiles nowhere.
#[must_use]
pub fn comptime_program() -> Program {
    Program::new(TargetLayout::host())
}
