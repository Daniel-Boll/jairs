//! Compile-time evaluation: giving `#run` and file-level constants their values.
//!
//! # Why this is a query and not a fold inside `jr-sema`
//!
//! ADR-0016 §4 says sema does not fold `#run`, and `PLAN.md`'s pipeline diagram draws
//! const-eval as `SEMA <--> VM`. Taken literally that is a **crate cycle**:
//! `crates/jr-mir/Cargo.toml` depends on `jr-sema`, and `jr-vm` must depend on
//! `jr-mir` to consume MIR, so `jr-sema` calling the VM closes `jr-sema → jr-vm →
//! jr-mir → jr-sema`, which Cargo rejects outright. ADR-0018 §3 resolves it by
//! putting the evaluation here, between `checked` and `file_mir`, and handing the
//! answers to `jr_mir::lower_file` as a third input map beside the `TypeMap`.
//!
//! The arrow survives as a description of the *pipeline* — sema's types feed an
//! evaluator, and the evaluator's answers feed lowering — but not of the crate graph.
//!
//! # Why it is a fixpoint
//!
//! Evaluation is genuinely ordered. In `024-hello.jr`, `main` needs `COMPUTED`'s
//! value, `COMPUTED` is `#run add(2, 3)` and needs `add`'s MIR, and `add` needs
//! nothing. Worse, a `#run` may call a procedure that *itself* reads a file-level
//! constant, so the MIR a thunk needs can depend on a value another thunk produces.
//!
//! Rather than build a dependency graph over declarations, this iterates: lower the
//! file with whatever values are known, evaluate every thunk that will now lower, and
//! repeat while something new was learned. That terminates because each round either
//! adds a value — and there are finitely many — or stops. A genuine cycle
//! (`A :: #run f();` where `f` reads `A`) simply never makes progress and is reported
//! as an unevaluable constant, which is the honest answer: it has no value.
//!
//! The cost is that lowering runs once per round rather than once. ADR-0018 accepts
//! it: the rounds are bounded by the number of constants in a file, in practice one
//! or two, and the alternative is a second dependency analysis to maintain beside the
//! one salsa already does.
//!
//! # Why it lowers `jr_mir::lower_file` directly rather than calling `file_mir`
//!
//! `file_mir` consumes this query's output, so calling it from here would be a salsa
//! cycle. The intermediate lowerings are private scratch work: only the *values* they
//! produce escape, and `file_mir` then does the one lowering anybody else sees.

use std::sync::Arc;

use jr_diag::{Diagnostic, Diagnostics};
use jr_hir::{ConstValue, ExprId, ExprScope, FileHir, ItemId, ItemKind};
use jr_mir::{ConstValues, ImportedProcs, Poisoned};
use jr_pool::{Pool, PoolId};
use jr_vm::{Mode, Routine, Value, Vm, VmError};

use crate::{
    Db, SourceFile,
    mir::imported_procs,
    module_loader::{ModuleSearchPaths, file_hir, frontend_diagnostics, resolved},
    sema::checked,
};

/// Compile-time evaluation failed.
///
/// E0230 was the first free code when this query claimed it; E0227–E0229 are
/// `jr-mir`'s. It covers every way a `#run` can fail to produce a value: a trap, a
/// refusal, or an expression the thunk lowerer cannot express. They share one code
/// because they share one remedy from the user's side — the expression cannot be
/// evaluated at compile time — and the message carries the specifics.
const E0230: &str = "E0230";

/// How many rounds of lower-then-evaluate to attempt.
///
/// A bound rather than "until stable" so that a bug in the progress check is a
/// diagnosable stop instead of a hang. Anything needing more than this is a
/// dependency chain of constants far longer than a file has.
const MAX_ROUNDS: usize = 16;

// ---------------------------------------------------------------------------
// Query output
// ---------------------------------------------------------------------------

/// The compile-time values a file's constants and `#run`s evaluate to.
#[derive(Debug, Clone)]
pub struct ConstResult {
    /// The values, ready to hand to `jr_mir::lower_file`.
    pub values: Arc<ConstValues>,
    /// E0230 for anything that should have had a value and did not.
    pub diagnostics: Arc<Diagnostics>,
}

// ---------------------------------------------------------------------------
// What needs a value
// ---------------------------------------------------------------------------

/// One thing to evaluate.
#[derive(Debug, Clone, Copy)]
enum Wanted {
    /// A named constant: `MESSAGE :: "hi";`. Its value is keyed by [`ItemId`].
    Item(ItemId, ExprId),
    /// A bare top-level `#run f();`, run for its effects. It has no name to key on,
    /// so its value is keyed by the expression.
    Run(ItemId, ExprId),
}

impl Wanted {
    const fn expr(self) -> ExprId {
        match self {
            Self::Item(_, expr) | Self::Run(_, expr) => expr,
        }
    }

    const fn item(self) -> ItemId {
        match self {
            Self::Item(item, _) | Self::Run(item, _) => item,
        }
    }
}

/// Everything in a file that needs compile-time evaluation, in source order.
///
/// A `struct` or a procedure constant is deliberately absent: ADR-0012 makes both
/// constants, but their "value" is a declaration rather than something to compute,
/// and `Callee::Direct` already names a procedure without one.
///
/// So is a **directive** constant. `libc :: #system_library "c";` has no runtime value
/// *by design* — ADR-0016 §3 gives it an opaque handle type, and
/// `jr_pool::LayoutError::ComptimeOnly` says the same thing from the layout side. It is
/// excluded here rather than allowed to fail, because a failure would become E0230 on
/// a declaration that is perfectly correct, which is exactly the kind of false
/// positive that teaches people to ignore a diagnostic.
fn wanted(hir: &FileHir) -> Vec<Wanted> {
    let mut out = Vec::new();
    for (index, item) in hir.items.iter().enumerate() {
        let id = ItemId::from_usize(index);
        match &item.kind {
            ItemKind::Const {
                value: ConstValue::Expr(expr),
            } => {
                if !is_directive(hir, *expr) {
                    out.push(Wanted::Item(id, *expr));
                }
            }
            ItemKind::Run { expr } => out.push(Wanted::Run(id, *expr)),
            ItemKind::Const { .. } | ItemKind::Var { .. } | ItemKind::Import { .. } => {}
        }
    }
    out
}

/// Whether a file-level expression is a bare directive.
fn is_directive(hir: &FileHir, expr: ExprId) -> bool {
    matches!(
        hir.exprs.get(expr.index()),
        Some(jr_hir::Expr::Directive { .. })
    )
}

// ---------------------------------------------------------------------------
// file_consts — tracked query
// ---------------------------------------------------------------------------

/// Evaluates every `#run` and every file-level constant initialiser.
///
/// Gated on [`frontend_diagnostics`] for the same reason `file_mir` is: ADR-0017 §4
/// forbids building MIR from a file with errors, and a thunk is MIR.
///
/// Uses `no_eq` to match the rest of this crate's queries.
#[salsa::tracked(returns(clone), no_eq)]
pub fn file_consts(db: &dyn Db, file: SourceFile, search_paths: ModuleSearchPaths) -> ConstResult {
    if frontend_diagnostics(db, file, search_paths).has_errors() {
        return ConstResult {
            values: Arc::new(ConstValues::new()),
            diagnostics: Arc::new(Diagnostics::new()),
        };
    }

    let hir = file_hir(db, file);
    let targets = wanted(hir.as_ref());
    if targets.is_empty() {
        return ConstResult {
            values: Arc::new(ConstValues::new()),
            diagnostics: Arc::new(Diagnostics::new()),
        };
    }

    let resolve = resolved(db, file, search_paths).map;
    let signatures = crate::sema::file_signatures(db, file, search_paths);
    let types = checked(db, file, search_paths).types;
    let imports = imported_procs(db, file, search_paths);
    let file_id = crate::queries::resolve_file_id(db, file);
    let interner = db.interner();

    let mut pool = crate::sema::lock_pool(db);
    let mut values = ConstValues::new();
    let mut failures: Vec<(Wanted, String)> = Vec::new();

    for _round in 0..MAX_ROUNDS {
        let remaining: Vec<Wanted> = targets
            .iter()
            .copied()
            .filter(|target| !known(&values, *target))
            .collect();
        if remaining.is_empty() {
            break;
        }

        // Lower the file with what is known so far, so that a thunk calling a
        // procedure has that procedure's bytecode available.
        let mir = jr_mir::lower_file(
            hir.as_ref(),
            resolve.as_ref(),
            types.as_ref(),
            signatures.signatures.as_ref(),
            &values,
            imports.as_ref(),
            interner,
            &mut pool,
        );

        failures.clear();
        let mut progressed = false;

        for target in remaining {
            match evaluate(
                &hir,
                file_id,
                target,
                &mir,
                resolve.as_ref(),
                types.as_ref(),
                signatures.signatures.as_ref(),
                imports.as_ref(),
                &values,
                interner,
                &mut pool,
            ) {
                Ok(value) => {
                    record(&mut values, target, value);
                    progressed = true;
                }
                Err(reason) => failures.push((target, reason)),
            }
        }

        if !progressed {
            break;
        }
    }

    drop(pool);

    let mut diagnostics = Diagnostics::new();
    for (target, reason) in failures {
        // A bare top-level `#run f();` is run for its effects and produces no value,
        // so "no value" is not a failure for one — but a failure to *run* it is.
        let Some(span) = hir.items.get(target.item().index()).map(|item| item.span) else {
            continue;
        };
        diagnostics.push(
            Diagnostic::error(span, format!("compile-time evaluation failed: {reason}"))
                .with_code(E0230)
                .with_note(
                    "`#run` and file-level constants are evaluated in the bytecode VM, which \
                     consumes the same MIR the back end does",
                ),
        );
    }

    ConstResult {
        values: Arc::new(values),
        diagnostics: Arc::new(diagnostics),
    }
}

fn known(values: &ConstValues, target: Wanted) -> bool {
    match target {
        Wanted::Item(item, _) => values.item(item).is_some(),
        Wanted::Run(_, expr) => values.run(ExprScope::TopLevel, expr).is_some(),
    }
}

fn record(values: &mut ConstValues, target: Wanted, value: PoolId) {
    match target {
        Wanted::Item(item, expr) => {
            values.set_item(item, value);
            // Also key the initialiser expression, so that a `#run` *inside* a named
            // constant folds when lowering walks it rather than being re-evaluated.
            values.set_run(ExprScope::TopLevel, expr, value);
        }
        Wanted::Run(_, expr) => values.set_run(ExprScope::TopLevel, expr, value),
    }
}

// ---------------------------------------------------------------------------
// One evaluation
// ---------------------------------------------------------------------------

/// Builds a thunk for one target, runs it, and interns the result.
///
/// Returns the reason as a `String` rather than a diagnostic so that a failure in an
/// early round — which a later round may fix — costs nothing.
#[allow(clippy::too_many_arguments)]
fn evaluate(
    hir: &Arc<FileHir>,
    file_id: jr_base::FileId,
    target: Wanted,
    mir: &jr_mir::FileMir,
    resolve: &jr_hir::ResolveMap,
    types: &jr_sema::TypeMap,
    signatures: &jr_sema::FileSignatures,
    imports: &ImportedProcs,
    values: &ConstValues,
    interner: &jr_base::Interner,
    pool: &mut Pool,
) -> Result<PoolId, String> {
    let thunk_proc = jr_mir::thunk_ref(hir, file_id, target.expr().index());
    let body = jr_mir::lower_const(
        hir,
        file_id,
        thunk_proc,
        target.expr(),
        resolve,
        types,
        values,
        imports,
        pool,
    )
    .map_err(|poison| match poison {
        Poisoned::Here(reason) => reason.to_owned(),
        Poisoned::Transitive(proc) => format!("proc {} is broken", proc.index()),
    })?;

    let ty = types
        .expr_type(ExprScope::TopLevel, target.expr())
        .ok_or_else(|| "the expression was never typed".to_owned())?;

    let mut program = jr_vm::comptime_program();
    jr_vm::add_file(&mut program, file_id, hir, mir, signatures, pool, interner)
        .map_err(|e: VmError| e.to_string())?;
    program.insert(Routine::Bytecode(
        jr_vm::compile(&body, pool, program.target()).map_err(|e: VmError| e.to_string())?,
    ));

    // `Mode::Comptime` is what refuses a foreign call until wave W6's
    // `#foreign_at_comptime` (ADR-0006). That refusal arrives here as an
    // `Unsupported`, and becomes E0230 with the VM's own wording.
    //
    // The result is reduced to `Raw` *inside* this scope, while the VM is still
    // alive: a `string` result is a `{data, count}` pair whose bytes live in the VM's
    // memory, so keeping it means copying them out before the VM goes away. The pool
    // cannot be interned into here either, because the VM borrows it.
    let raw = {
        let mut vm = Vm::new(&program, pool, Mode::Comptime).map_err(|e: VmError| e.to_string())?;
        let value = vm.call(thunk_proc, Vec::new()).map_err(|e| e.to_string())?;
        reduce(&vm, &value, ty).map_err(|e| e.to_string())?
    };

    Ok(match raw {
        Raw::Void => PoolId::VOID_VALUE,
        Raw::Bool(value) => pool.bool_value(value),
        Raw::Int(bits) => pool.int_value(ty, bits),
        Raw::Str(bytes) => {
            let text = String::from_utf8(bytes)
                .map_err(|_| "a compile-time string was not valid UTF-8".to_owned())?;
            pool.str_value(&text)
        }
    })
}

/// A result reduced to something that outlives the VM.
///
/// The VM's memory is released when it is dropped, so a value that *points* into it
/// has to be copied before then. Two steps rather than one because interning needs
/// `&mut Pool` and the VM holds `&Pool`.
enum Raw {
    Void,
    Bool(bool),
    Int(u64),
    Str(Vec<u8>),
}

/// Copies a result out of the VM.
fn reduce(vm: &Vm<'_>, value: &Value, ty: PoolId) -> Result<Raw, VmError> {
    match value {
        Value::Void => Ok(Raw::Void),
        Value::Scalar(bits) => {
            if ty == PoolId::BOOL {
                Ok(Raw::Bool(*bits != 0))
            } else {
                Ok(Raw::Int(*bits))
            }
        }
        Value::Aggregate(_) if ty == PoolId::STRING => Ok(Raw::Str(vm.read_string(value)?)),
        // A struct computed at compile time would need the pool to intern an aggregate
        // value, which ADR-0015's `Item` has no variant for. Nothing in the corpus
        // does it.
        Value::Aggregate(_) => Err(VmError::unsupported_public(
            "a compile-time struct value arrives with a later wave",
        )),
        Value::Undefined => Err(VmError::unsupported_public(
            "the expression evaluated to no value",
        )),
    }
}
