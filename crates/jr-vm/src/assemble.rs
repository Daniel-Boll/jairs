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
//! # Why a `#foreign` procedure needs its library resolved *again*
//!
//! `ForeignInfo::library` is a [`Symbol`] naming the *constant* — `libc` — not the
//! library. Getting from there to `"c"` means finding the item that constant declares
//! and reading the `#system_library "c"` directive out of it. `jr-sema` already did
//! this walk once, to check that the name denotes a library (E0225), and recorded
//! nothing: ADR-0016 §3 gives such a constant an opaque type, and a constant's
//! *value* is not something sema records at all.
//!
//! So this is the second independent implementation of one lookup, which ADR-0018 §4
//! records as accepted debt with a clear trigger: a third is the signal to intern the
//! resolved library beside [`jr_pool::Item::ForeignLibraryValue`], where both callers
//! could read it.

use jr_base::{FileId, Interner, Symbol};
use jr_hir::{ConstValue, Expr, FileHir, ItemKind, ProcId};
use jr_mir::{FileMir, ProcRef};
use jr_pool::{Pool, TargetLayout};
use jr_sema::FileSignatures;

use crate::code::{ForeignProc, Routine};
use crate::error::VmError;
use crate::interp::Program;
use crate::lower::compile;

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
    interner: &Interner,
) -> Result<(), VmError> {
    for (_proc, outcome) in mir.iter() {
        if let Ok(body) = outcome {
            program.insert(Routine::Bytecode(compile(body, pool, program.target())?));
        }
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
            library: info
                .library
                .and_then(|name| library_name(hir, interner, name)),
            params: sig.params.clone(),
            ret: sig.ret,
        }));
    }

    Ok(())
}

/// Resolves the constant `name` to the library string it declares.
///
/// `libc :: #system_library "c";` becomes `Some("c")`. Anything else — a name that
/// denotes no item, or an item that is not a `#system_library` directive — becomes
/// `None`, and the bridge then refuses to guess which library was meant.
fn library_name(hir: &FileHir, interner: &Interner, name: Symbol) -> Option<String> {
    let item = hir.scope.get(name)?;
    let ItemKind::Const {
        value: ConstValue::Expr(expr),
    } = &hir.items.get(item.index())?.kind
    else {
        return None;
    };
    let Expr::Directive {
        name: directive,
        arg,
        span: _,
    } = hir.exprs.get(expr.index())?
    else {
        return None;
    };
    if interner.resolve(*directive) != "system_library" {
        return None;
    }
    arg.clone()
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
