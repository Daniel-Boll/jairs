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
    BuildConfig, Db, SourceFile,
    mir::optimized_file_mir,
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
    config: BuildConfig,
) -> Result<RunOutcome, String> {
    let entry = main_of(db, root).ok_or_else(|| "the file declares no `main`".to_owned())?;

    // **The entry point must have lowered.** E0245 warns about a refused body at check time,
    // because one nobody calls does not stop the program — but `main` is called by definition,
    // and skipping it used to reach the interpreter's own lookup as
    // `internal compiler error: no routine for file 0 proc 0` on a program `jr check` called
    // clean. Checked here rather than left to that lookup, so the failure names the procedure
    // and says whose fault it is (ADR-0047 §2).
    //
    // Only `main` is checked, not every reachable body: a refused body deeper in the program is
    // only a problem if it is *reached*, and deciding that statically is the call graph this
    // query deliberately does not build.
    {
        let mir = optimized_file_mir(db, root, search_paths, config);
        if let Some(Err(reason)) = mir
            .mir
            .iter()
            .find(|(proc, _)| *proc == entry.proc)
            .map(|(_, outcome)| outcome)
        {
            return Err(format!(
                "the compiler could not lower `main` ({reason:?}); this program is legal and \
                 this compiler has a gap — please report it"
            ));
        }
    }

    let files = reachable_files(db, root, search_paths);

    // Gather every query result before locking the pool: the lock must never be held
    // across a nested query call, which is the rule the rest of this crate follows.
    let mut inputs = Vec::with_capacity(files.len());
    for file in files {
        let mir = optimized_file_mir(db, file, search_paths, config);
        if mir.gated {
            continue;
        }
        // The **expanded** HIR and signatures the MIR was lowered from (ADR-0082 §2): an instantiation
        // appended procedures to them, so pairing the MIR with the base `file_hir`/`file_signatures`
        // would leave `add_file` unable to find a declaration for an instantiated `ProcId`.
        inputs.push((
            crate::queries::resolve_file_id(db, file),
            mir.hir.clone(),
            mir.mir.clone(),
            mir.signatures.clone(),
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
    // **`main`'s context is created here** (ADR-0057 §5), because `main` has no Jairs caller to have
    // passed one. Zeroed, so `context.allocator` reads 0 in a program that never sets it.
    //
    // Only when `main` receives one: a `#c_call main` takes no hidden parameter, and passing one
    // anyway would be the argument shift ADR-0053 §1 recorded — "called a procedure taking 1
    // arguments with 0" is what the VM said before this existed, from the other direction.
    let args = if main_receives_context(db, root) {
        // **From the guard this function already holds**, not a fresh lock. The mutex is not
        // reentrant, so `lock_pool` here deadlocked — the program hung rather than failing, which is
        // the same self-deadlock the completion wave hit and `jr-lsp` records: gather everything
        // before locking, never lock twice.
        //
        // The context type is interned by *sema* long before this, so reading it needs no `&mut`.
        let context = jr_pool::Pool::find_context(&pool)
            .ok_or_else(|| "the context type was never interned".to_owned())?;
        let layout = jr_pool::layout_of(&pool, jr_pool::TargetLayout::LP64, context)
            .map_err(|e| format!("the context has no layout: {e:?}"))?;
        vec![
            vm.new_context(layout.size, layout.align)
                .map_err(|e: VmError| e.to_string())?,
        ]
    } else {
        Vec::new()
    };
    Ok(match vm.call(entry, args) {
        Ok(Value::Void | Value::Scalar(_) | Value::Aggregate(_) | Value::Undefined) => {
            RunOutcome::Completed
        }
        Err(VmError::Exited(status)) => RunOutcome::Exited(status),
        Err(error) => {
            // A trap gets a source location; anything else is a compiler fault with
            // no source text of its own to point at. Rendered here rather than in
            // `jr-cli` because resolving the span needs the file's HIR, which is a
            // query — and rendered through `jr_base::trap_message` so that the native
            // back end's compile-time message and this one cannot drift (ADR-0020 §2).
            let location = vm
                .trap_site()
                .and_then(|site| trap_location(db, &inputs, site));
            // **The call chain the trap happened in** (ADR-0066). Resolved here for the same reason the
            // location is: turning a `ProcRef` into a name needs the file's HIR, which the VM has not
            // got. A frame whose name cannot be resolved is dropped rather than rendered as a
            // placeholder — an unnamed line in a backtrace tells a reader nothing and would differ from
            // native, which resolves the same names at compile time.
            let frames: Vec<String> = vm
                .trap_frames()
                .into_iter()
                .filter_map(|frame| proc_name(db, &inputs, frame))
                .collect();
            let frames: Vec<&str> = frames.iter().map(String::as_str).collect();
            RunOutcome::Failed(jr_base::trap_message(
                &error.to_string(),
                location.as_deref(),
                &frames,
            ))
        }
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

/// Every file the program reaches: the root plus every module reachable through `#import`, transitively.
///
/// **Public since ADR-0108 §1**, because the CLI needs it to report an imported module's own diagnostics. It was
/// crate-private while its only use was assembling MIR, and the gap that made it public is worth recording: a
/// root whose imported module was broken *checked clean* and failed at run time, because `file_diagnostics`
/// reports one file and nothing else.
///
/// The root is first, and the rest follow in discovery order. Distinct by construction — the seen-set is what
/// makes a legal import cycle (ADR-0014 §4) terminate rather than an assumption.
pub fn reachable_files(
    db: &dyn Db,
    root: SourceFile,
    search_paths: ModuleSearchPaths,
) -> Vec<SourceFile> {
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
            if let Some(module) = db.source_file_for_path(path.as_ref())
                && !seen.contains(&module)
            {
                seen.push(module);
                queue.push(module);
            }
        }
    }
    seen
}

/// One reachable file's query results, gathered before the pool is locked.
///
/// Named because the tuple is wide enough that clippy objects to it, and because both
/// the run driver and `build` assemble the same four things for the same reason: the
/// pool's lock must not be held across a nested query call.
pub(crate) type FileInput = (
    jr_base::FileId,
    Arc<jr_hir::FileHir>,
    Arc<jr_mir::FileMir>,
    Arc<jr_sema::FileSignatures>,
);

/// Renders a trap's source location as `path:line:col`, if it has one.
///
/// `None` when the span is `MirSpan::Synthetic` — a compiler-invented value has no
/// source text — or when the trapping procedure's file is not among the inputs, which
/// would mean the program called into a file that was never assembled.
fn trap_location(db: &dyn Db, inputs: &[FileInput], site: jr_vm::TrapSite) -> Option<String> {
    let hir = inputs
        .iter()
        .find_map(|(file_id, hir, _, _)| (*file_id == site.proc.file).then_some(hir))?;
    let body = hir
        .procs
        .get(site.proc.proc.index())
        .and_then(|data| data.body)
        .and_then(|id| hir.bodies.get(id.index()));
    let span = jr_mir::resolve_span(hir, body, site.span)?;
    Some(jr_base::render_location(&db.source_map(), span))
}

/// The source name of a procedure, for a backtrace frame (ADR-0066 §2).
///
/// Found by scanning the file's items for the constant whose value *is* this procedure, which is the
/// same walk [`main_of`] does and for the same reason: procedures are constants (ADR-0012), so a `Proc`
/// carries no name of its own and the name lives on the item that binds it.
///
/// `None` when the procedure's file is not among the inputs, or when no item binds it — an anonymous
/// procedure has no name to print, and a placeholder would be worse than omitting the frame.
fn proc_name(db: &dyn Db, inputs: &[FileInput], target: ProcRef) -> Option<String> {
    let hir = inputs
        .iter()
        .find_map(|(file_id, hir, _, _)| (*file_id == target.file).then_some(hir))?;
    let interner = db.interner();
    hir.items.iter().find_map(|item| {
        let ItemKind::Const {
            value: ConstValue::Proc(proc),
        } = &item.kind
        else {
            return None;
        };
        if *proc != target.proc {
            return None;
        }
        Some(interner.resolve(item.name?).to_owned())
    })
}

/// Whether the `main` of `root` receives the implicit context (ADR-0057 §3).
///
/// Reads the same two fields `jr-mir` does — `c_call` or `#foreign` — rather than the interned
/// `ContextKind`, so the caller and the callee cannot disagree about whether a hidden parameter
/// exists. Two answers to that question is the argument shift ADR-0053 §1 recorded.
fn main_receives_context(db: &dyn Db, root: SourceFile) -> bool {
    let Some(entry) = main_of(db, root) else {
        return false;
    };
    let hir = crate::module_loader::file_hir(db, root);
    hir.procs
        .get(entry.proc.index())
        .is_some_and(|proc| !(proc.c_call || proc.foreign.is_some()))
}
