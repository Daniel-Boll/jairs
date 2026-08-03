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

/// One imported file's front end, for the comptime program (ADR-0069 §1).
///
/// **The MIR is not here, and that is the whole point.** The first attempt held an `Arc<FileMir>` from
/// `file_mir`, which salsa rejected outright with a dependency-graph cycle:
///
/// ```text
/// file_consts(A) -> file_mir(B) -> imported_values(B) -> file_consts(A)
/// ```
///
/// because `file_mir` folds *imported constants*, which needs the importer's `file_consts`. So this
/// carries the inputs and the MIR is lowered here instead, with the same empty
/// `ImportedValues`/`OperatorCalls`/`FilledArgs` this module already passes for its own file — which is
/// the honest position: const-eval runs before the check phase that fills those, for the importer and
/// the imported file alike (ADR-0018 §3).
///
/// ADR-0069 §1's claim that supplying routines "adds no dependency that was not already there" was
/// therefore *wrong about `file_mir`* and right about the principle: `imported_procs` and `checked` are
/// already called from here, and lowering from them introduces nothing new.
struct ModuleFrontend {
    id: jr_base::FileId,
    hir: Arc<FileHir>,
    resolve: Arc<jr_hir::ResolveMap>,
    types: Arc<jr_sema::TypeMap>,
    signatures: Arc<jr_sema::FileSignatures>,
    imports: Arc<ImportedProcs>,
}

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
    /// A `#run` inside a procedure body (ADR-0069 §2), keyed by the body and the expression.
    ///
    /// Its own variant rather than a `Run` with a scope field, so that every match over `Wanted` is
    /// forced to decide which arena an expression belongs to — a body's expressions and the file's both
    /// start at index 0, so confusing them reads a different expression rather than failing.
    ///
    /// The `ItemId` is the *procedure's* item, used only to place a diagnostic: a body `#run` has no
    /// item of its own.
    BodyRun(ItemId, jr_hir::BodyId, ExprId),
    /// A constant whose initialiser names a **type**: `T :: Point;` (ADR-0071 §2).
    ///
    /// Its own variant because it is the one target the VM never runs. Its value is read from
    /// `SigEntry::type_value`, which the *signature* phase already computed — and const-eval is
    /// downstream of signatures (ADR-0018 §3), so this reads a value that exists rather than inverting
    /// a phase. That is the move ADR-0070 §1 made for an array length, available for the same reason.
    ///
    /// A variant rather than a special case inside [`Wanted::Item`] so that every match over `Wanted`
    /// is forced to decide, which is the same discipline `BodyRun` above exists for. The round-robin
    /// and the cycle detector need no change: a type alias is a target like any other, it simply
    /// succeeds in the first round.
    TypeAlias(ItemId, ExprId),
    /// The **operand of a computed `#insert`** inside a body (ADR-0073 §1, step 5).
    ///
    /// `#insert S;` and `#insert #run mk();` each hold their operand as an ordinary body expression, and
    /// evaluating it is exactly evaluating a [`Wanted::BodyRun`] — same scope, same thunk. Its own variant
    /// only so the insert-operand query can pick these out of `ConstValues` afterwards and key them by the
    /// directive's *span* (invariant across the re-lowering that consumes them; the operand's `ExprId` is
    /// not). The last field is that directive span.
    InsertOperand(ItemId, jr_hir::BodyId, ExprId, jr_base::Span),
}

impl Wanted {
    const fn expr(self) -> ExprId {
        match self {
            Self::Item(_, expr)
            | Self::Run(_, expr)
            | Self::BodyRun(_, _, expr)
            | Self::TypeAlias(_, expr)
            | Self::InsertOperand(_, _, expr, _) => expr,
        }
    }

    const fn item(self) -> ItemId {
        match self {
            Self::Item(item, _)
            | Self::Run(item, _)
            | Self::BodyRun(item, _, _)
            | Self::TypeAlias(item, _)
            | Self::InsertOperand(item, _, _, _) => item,
        }
    }

    /// Which expression arena [`Wanted::expr`] indexes (ADR-0069 §2).
    const fn scope(self) -> ExprScope {
        match self {
            Self::Item(_, _) | Self::Run(_, _) | Self::TypeAlias(_, _) => ExprScope::TopLevel,
            Self::BodyRun(_, body, _) | Self::InsertOperand(_, body, _, _) => ExprScope::Body(body),
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
fn wanted(hir: &FileHir, signatures: &jr_sema::FileSignatures) -> Vec<Wanted> {
    let mut out = Vec::new();
    for (index, item) in hir.items.iter().enumerate() {
        let id = ItemId::from_usize(index);
        match &item.kind {
            ItemKind::Const {
                value: ConstValue::Expr(expr),
            } => {
                if is_directive(hir, *expr) {
                    // Excluded, per this function's docs.
                } else if names_a_type(hir, signatures, *expr) {
                    // `T :: Point;` — a type alias, whose value the signature phase already knows
                    // (ADR-0071 §2). A target rather than an exclusion, because it genuinely has a
                    // value: without it the thunk lowerer reported "a file-level item has no value
                    // yet", a const-eval internal on a perfectly correct declaration.
                    out.push(Wanted::TypeAlias(id, *expr));
                } else {
                    out.push(Wanted::Item(id, *expr));
                }
            }
            ItemKind::Run { expr } => out.push(Wanted::Run(id, *expr)),
            ItemKind::Const { .. } | ItemKind::Var { .. } | ItemKind::Import { .. } => {}
        }
    }
    // Then every `#run` inside a body (ADR-0069 §2). Collected in the same query as the file-scope ones
    // so there is **one** round-robin and one cycle detector: two places evaluating `#run` would be two
    // chances to disagree about what a `#run` means.
    for (index, item) in hir.items.iter().enumerate() {
        let id = ItemId::from_usize(index);
        let ItemKind::Const {
            value: ConstValue::Proc(proc),
        } = &item.kind
        else {
            continue;
        };
        let Some(body_id) = hir.procs.get(proc.index()).and_then(|p| p.body) else {
            continue;
        };
        let Some(body) = hir.bodies.get(body_id.index()) else {
            continue;
        };
        for (expr_index, expr) in body.exprs.iter().enumerate() {
            if matches!(expr, jr_hir::Expr::Run(_, _)) {
                out.push(Wanted::BodyRun(id, body_id, ExprId::from_usize(expr_index)));
            }
        }
        // And every **computed `#insert` operand** in this body (ADR-0073 §1, step 5): a pending insert
        // holds its operand expression, evaluated exactly as a body `#run`. Keyed additionally by the
        // directive's span, the one identifier stable across the re-lowering that consumes it.
        for stmt in &body.stmts {
            if let jr_hir::Stmt::Insert {
                operand: Some(op),
                span,
                ..
            } = stmt
            {
                out.push(Wanted::InsertOperand(id, body_id, *op, *span));
            }
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

/// Whether a `::` initialiser names a type, making it a [`Wanted::TypeAlias`] (ADR-0071 §2).
///
/// Asked of the **signatures** rather than of the HIR, because "does this name denote a type" is a
/// question the signature phase already answered — `SigEntry::type_value` is `Some` exactly for a name
/// that does. Re-deriving it here would be a second implementation of ADR-0014 §3's resolution order,
/// and a divergence would show up as a constant that evaluates in one phase's opinion and not the
/// other's.
///
/// A bare name only: `T :: Point;` and nothing more elaborate. An expression *containing* a type is
/// already refused elsewhere (E0261, ADR-0071 §3), so there is no case where this answering `false`
/// hides one.
fn names_a_type(hir: &FileHir, signatures: &jr_sema::FileSignatures, expr: ExprId) -> bool {
    let Some(jr_hir::Expr::Name { name, .. }) = hir.exprs.get(expr.index()) else {
        return false;
    };
    signatures
        .lookup(*name)
        .is_some_and(|entry| entry.type_value.is_some())
}

/// The type a `::` initialiser denotes, for a [`Wanted::TypeAlias`] (ADR-0071 §2).
///
/// The same lookup [`names_a_type`] does, returning the type rather than a yes — deliberately two
/// functions over one field, so that the classification and the value cannot disagree about *which*
/// entry they read.
fn aliased_type(
    hir: &FileHir,
    signatures: &jr_sema::FileSignatures,
    expr: ExprId,
) -> Option<PoolId> {
    let jr_hir::Expr::Name { name, .. } = hir.exprs.get(expr.index())? else {
        return None;
    };
    signatures.lookup(*name)?.type_value
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
    let resolve = resolved(db, file, search_paths).map;
    let signatures = crate::sema::file_signatures(db, file, search_paths);
    // **Signatures first**, because `wanted` asks them whether a `::` initialiser names a type
    // (ADR-0071 §2). They were already computed one line below; only the order changed.
    let targets = wanted(hir.as_ref(), signatures.signatures.as_ref());
    if targets.is_empty() {
        return ConstResult {
            values: Arc::new(ConstValues::new()),
            diagnostics: Arc::new(Diagnostics::new()),
        };
    }

    let types = checked(db, file, search_paths).types;
    let imports = imported_procs(db, file, search_paths);
    let file_id = crate::queries::resolve_file_id(db, file);
    let interner = db.interner();

    // **Every other reachable file's compiled form**, so a `#run` calling an imported procedure has
    // that procedure's bytecode (ADR-0069 §1). Without it the interpreter reported
    // `internal compiler error: no routine for file N proc M` — compiler internals shown to a user who
    // wrote a reasonable program.
    //
    // This is **not** the cross-file dependency this module refuses below: that refusal is about reading
    // another file's constant *values* (`ImportedValues` stays empty, and the argument is at the
    // `lower_file` call). A routine is not a value, and `imported_procs` already resolves cross-file
    // procedures for the ordinary runtime path — so this supplies code for a call sema already agreed
    // exists, and adds no dependency that was not already there.
    //
    // Gathered before the pool is locked, because the lock must never be held across a nested query
    // call — the same rule `build` and `run_main` follow.
    let mut modules = Vec::new();
    for other in crate::run::reachable_files(db, file, search_paths) {
        if other == file {
            continue;
        }
        if frontend_diagnostics(db, other, search_paths).has_errors() {
            continue;
        }
        modules.push(ModuleFrontend {
            id: crate::queries::resolve_file_id(db, other),
            hir: file_hir(db, other),
            resolve: resolved(db, other, search_paths).map,
            types: checked(db, other, search_paths).types,
            signatures: crate::sema::file_signatures(db, other, search_paths).signatures,
            imports: imported_procs(db, other, search_paths),
        });
    }

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
            // **Empty, and this one is not a gap.** Const-eval evaluating *this* file's constants has
            // no business reading another file's — that would make one module's constant folding
            // depend on another's, and ADR-0055 §3's acyclicity argument is about `optimized_file_mir`
            // rather than about this query. A `#run` reading an imported constant stays refused.
            &jr_mir::ImportedValues::new(),
            // **Empty, deliberately.** Const-eval runs before `checked`, so the overload map does
            // not exist yet — and asking for it here would make const-eval depend on the check
            // phase, which is the cycle ADR-0018 §3 avoided by putting const-eval downstream of
            // *signatures* rather than of checking.
            //
            // The consequence, stated because it bites: an operator overload cannot be used in a
            // `#run` or a `::` constant. `scan` refuses such a body — the operator finds no
            // overload, falls through to the builtin path, and sema has already reported the
            // operand types as unsupported — so this is a refusal rather than a wrong answer.
            &jr_mir::OperatorCalls::new(),
            // **Empty for the same reason, and with the same consequence** (ADR-0053 §2). The
            // filled-argument map is `checked`'s output, so a `#run` calling a procedure with a
            // *default* argument gets the source-order list — which for a call that omits an
            // argument is one operand short, and `scan` refuses the body rather than passing
            // garbage. A refusal, not a wrong answer.
            //
            // Named arguments in a `#run` are refused the same way. Both are recorded as owed
            // rather than discovered: lifting them means giving const-eval a checked view, which
            // is the cycle ADR-0018 §3 exists to prevent.
            &jr_mir::FilledArgs::new(),
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
                &modules,
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

// ---------------------------------------------------------------------------
// insert_operands — tracked query (ADR-0073 §1, step 5)
// ---------------------------------------------------------------------------

/// The evaluated text of every computed `#insert` operand in a file, keyed by directive span.
///
/// This is the input `expanded_file_hir` hands to [`jr_hir::lower_file_with_inserts`]. It reuses
/// [`file_consts`], which already evaluates each operand as a body `#run` in its round-robin — so there
/// is **one** evaluator and one cycle detector, the same discipline that put body `#run`s in `file_consts`
/// rather than a second query (ADR-0069 §2). This query only *reads back* the values `file_consts`
/// computed and re-keys them by span, resolving each string `PoolId` to its text.
///
/// **Acyclic:** it depends on `file_consts` → `frontend_diagnostics`, which is mir-free (it ends at
/// `checked`; only `file_diagnostics` reaches `file_mir`). So rewiring `file_mir` onto an expanded HIR
/// that depends on this query never loops back to `file_mir` — verified by reading the query graph, and
/// the reason ADR-0073 §1's acyclic-pre-pass claim holds.
///
/// A non-`string` operand is **not** reported here: `checked` already reported it (E0214, the operand is
/// checked expecting `string`), so this query silently omits it and the insert stays pending, which
/// `jr-mir`'s `scan` refuses — one diagnostic, at the operand's own span.
///
/// Uses `no_eq` to match the rest of this crate's queries.
#[salsa::tracked(returns(clone), no_eq)]
pub fn insert_operands(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
) -> Arc<jr_hir::InsertOperands> {
    let consts = file_consts(db, file, search_paths);
    let hir = file_hir(db, file);
    let signatures = crate::sema::file_signatures(db, file, search_paths);
    let mut operands = jr_hir::InsertOperands::new();

    // Re-walk the same targets `file_consts` evaluated, keeping only the insert operands — each carries
    // the directive span this map is keyed by. The value is in `consts.values` under the operand's
    // `(Body, ExprId)` key, exactly where `record` put it.
    let pool = crate::sema::lock_pool(db);
    for target in wanted(hir.as_ref(), signatures.signatures.as_ref()) {
        let Wanted::InsertOperand(_, body, expr, span) = target else {
            continue;
        };
        let Some(value) = consts.values.run(ExprScope::Body(body), expr) else {
            // The operand did not evaluate to a value (a non-string, a trap, a refusal). Left out, so
            // the insert stays pending; the reason was already reported by `checked` or `file_consts`.
            continue;
        };
        // Only a *string* operand expands. A non-string reaching here would be a `checked` bug, since the
        // operand is checked expecting `string`; guarded rather than trusted, because a wrong expansion is
        // a miscompile.
        if let jr_pool::Item::StrValue(str_id) = pool.item(value) {
            operands.set(span, pool.resolve_str(*str_id).to_owned());
        }
    }
    drop(pool);

    Arc::new(operands)
}

fn known(values: &ConstValues, target: Wanted) -> bool {
    match target {
        // A type alias's value is keyed like any other named constant's, so nothing downstream has to
        // know it was one (ADR-0071 §2).
        Wanted::Item(item, _) | Wanted::TypeAlias(item, _) => values.item(item).is_some(),
        Wanted::Run(_, expr) => values.run(ExprScope::TopLevel, expr).is_some(),
        Wanted::BodyRun(_, body, expr) | Wanted::InsertOperand(_, body, expr, _) => {
            values.run(ExprScope::Body(body), expr).is_some()
        }
    }
}

fn record(values: &mut ConstValues, target: Wanted, value: PoolId) {
    match target {
        Wanted::Item(item, expr) | Wanted::TypeAlias(item, expr) => {
            values.set_item(item, value);
            // Also key the initialiser expression, so that a `#run` *inside* a named
            // constant folds when lowering walks it rather than being re-evaluated.
            values.set_run(ExprScope::TopLevel, expr, value);
        }
        Wanted::Run(_, expr) => values.set_run(ExprScope::TopLevel, expr, value),
        Wanted::BodyRun(_, body, expr) | Wanted::InsertOperand(_, body, expr, _) => {
            values.set_run(ExprScope::Body(body), expr, value)
        }
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
    modules: &[ModuleFrontend],
    interner: &jr_base::Interner,
    pool: &mut Pool,
) -> Result<PoolId, String> {
    // **A type alias needs no VM at all** (ADR-0071 §2). Its value is `SigEntry::type_value`, computed
    // by the signature phase this query is downstream of (ADR-0018 §3) — so it is interned here and
    // returned before a thunk is built. Handled at the top rather than inside the thunk lowerer because
    // a thunk is MIR and a type value has no runtime representation to lower
    // (`jr_pool::LayoutError::ComptimeOnly`): there is nothing for the VM to do.
    if let Wanted::TypeAlias(_, expr) = target {
        let denoted = aliased_type(hir, signatures, expr)
            .ok_or_else(|| "a type alias does not name a type".to_owned())?;
        return Ok(pool.type_value(denoted));
    }

    let thunk_proc = jr_mir::thunk_ref(hir, file_id, target.expr().index());
    let body = jr_mir::lower_const(
        hir,
        file_id,
        thunk_proc,
        target.expr(),
        target.scope(),
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
        .expr_type(target.scope(), target.expr())
        .ok_or_else(|| "the expression was never typed".to_owned())?;

    let mut program = jr_vm::comptime_program();
    jr_vm::add_file(&mut program, file_id, hir, mir, signatures, pool)
        .map_err(|e: VmError| e.to_string())?;
    // Then every imported file, so a cross-file call resolves (ADR-0069 §1). Lowered here rather than
    // taken from `file_mir`, for the cycle reason `ModuleFrontend` documents — and with the same empty
    // maps this module passes for its own file, so an imported callee is subject to exactly the same
    // const-eval restrictions as a local one.
    for module in modules {
        let module_mir = jr_mir::lower_file(
            module.hir.as_ref(),
            module.resolve.as_ref(),
            module.types.as_ref(),
            module.signatures.as_ref(),
            &ConstValues::new(),
            module.imports.as_ref(),
            &jr_mir::ImportedValues::new(),
            &jr_mir::OperatorCalls::new(),
            &jr_mir::FilledArgs::new(),
            interner,
            pool,
        );
        jr_vm::add_file(
            &mut program,
            module.id,
            module.hir.as_ref(),
            &module_mir,
            module.signatures.as_ref(),
            pool,
        )
        .map_err(|e: VmError| e.to_string())?;
    }
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
    //
    // Asked **before** the borrow for exactly that reason: `reduce` needs to know whether the result
    // is a float, and the pool is unavailable once the VM holds it.
    let is_float = jr_pool::FloatKind::of(pool, ty).is_some();
    let raw = {
        let mut vm = Vm::new(&program, pool, Mode::Comptime).map_err(|e: VmError| e.to_string())?;
        let value = vm.call(thunk_proc, Vec::new()).map_err(|e| e.to_string())?;
        reduce(&vm, &value, ty, is_float).map_err(|e| e.to_string())?
    };

    Ok(match raw {
        Raw::Void => PoolId::VOID_VALUE,
        Raw::Bool(value) => pool.bool_value(value),
        Raw::Int(bits) => pool.int_value(ty, bits),
        Raw::Float(bits) => pool.float_value(ty, bits),
        Raw::Str(bytes) => {
            let text = String::from_utf8(bytes)
                .map_err(|_| "a compile-time string was not valid UTF-8".to_owned())?;
            pool.str_value(&text)
        }
        Raw::Aggregate(bytes) => intern_aggregate(pool, ty, &bytes)?,
    })
}

/// Interns an aggregate constant from the byte image the VM produced (ADR-0074 §1).
///
/// The inverse of `jr-vm`'s `aggregate_value`: it reads each element out of the image at the element's own
/// offset and interns it, so what lands in the pool is the **element values** rather than the bytes. That
/// is the whole point — the bytes are a *target* answer and the pool is target-independent, so keeping them
/// would put one target's padding and pointer width into a shared table.
///
/// Recursive, because an element may itself be an aggregate: `[2]P` reads two sub-images and interns each
/// the same way, with no special case.
///
/// A **union** is refused (ADR-0074 §4): its fields overlap, so which one the bytes represent is
/// unanswerable, and picking one silently is exactly the reinterpretation ADR-0045 §1 allows only for a
/// runtime read the programmer wrote.
fn intern_aggregate(pool: &mut Pool, ty: PoolId, bytes: &[u8]) -> Result<PoolId, String> {
    // Comptime execution uses the *host* layout, matching the VM that produced these bytes.
    let target = jr_pool::TargetLayout::host();

    // Element types and offsets first, so the immutable pool borrow ends before interning begins.
    let placements: Vec<(PoolId, u64, u64)> = match *pool.item(ty) {
        jr_pool::Item::ArrayType { elem, len } => {
            let elem_layout = jr_pool::layout_of(pool, target, elem)
                .map_err(|e| format!("an array constant's element has no layout: {e}"))?;
            (0..len)
                .map(|index| (elem, elem_layout.size * index, elem_layout.size))
                .collect()
        }
        jr_pool::Item::StructType { decl } | jr_pool::Item::VariantType { decl } => {
            let fields = pool
                .struct_fields(decl)
                .ok_or_else(|| "a struct constant's fields are not recorded".to_owned())?
                .to_vec();
            let mut out = Vec::with_capacity(fields.len());
            for (index, field) in fields.iter().enumerate() {
                let (offset, layout) = jr_pool::field_offset(pool, target, ty, index as u32)
                    .map_err(|e| format!("a struct constant's field has no offset: {e}"))?;
                out.push((field.ty, offset, layout.size));
            }
            out
        }
        // A union's fields overlap, so the bytes do not say which is live (ADR-0074 §4).
        jr_pool::Item::UnionType { .. } => {
            return Err("a compile-time union value has no defined field to read".to_owned());
        }
        _ => return Err("a compile-time aggregate of this shape is not supported".to_owned()),
    };

    let mut elements = Vec::with_capacity(placements.len());
    for (elem_ty, offset, size) in placements {
        let start = usize::try_from(offset).unwrap_or(0);
        let end = start.saturating_add(usize::try_from(size).unwrap_or(0));
        let slice = bytes
            .get(start..end)
            .ok_or_else(|| "a compile-time aggregate is shorter than its layout".to_owned())?;
        elements.push(intern_element(pool, elem_ty, slice)?);
    }
    Ok(pool.aggregate_value(ty, elements))
}

/// Interns one element of an aggregate constant from its own bytes (ADR-0074 §1).
///
/// A scalar is decoded little-endian, exactly as `jr-vm`'s `write_le` wrote it, so the two directions
/// share one byte-order answer. A nested aggregate recurses into [`intern_aggregate`]. A `string` element
/// is refused: its bytes are a `{data, count}` pair pointing into the VM's memory, and that pointer has no
/// meaning once the VM is gone — the same reason `string` interns by contents rather than as an aggregate
/// (ADR-0074 §2).
fn intern_element(pool: &mut Pool, ty: PoolId, bytes: &[u8]) -> Result<PoolId, String> {
    if ty == PoolId::STRING {
        return Err(
            "a compile-time aggregate holding a string arrives with a later wave".to_owned(),
        );
    }
    match *pool.item(ty) {
        jr_pool::Item::ArrayType { .. }
        | jr_pool::Item::StructType { .. }
        | jr_pool::Item::UnionType { .. }
        | jr_pool::Item::VariantType { .. } => intern_aggregate(pool, ty, bytes),
        jr_pool::Item::VoidType => Ok(PoolId::VOID_VALUE),
        _ => {
            let mut buf = [0u8; 8];
            let take = bytes.len().min(8);
            buf[..take].copy_from_slice(&bytes[..take]);
            let bits = u64::from_le_bytes(buf);
            if ty == PoolId::BOOL {
                return Ok(pool.bool_value(bits != 0));
            }
            if jr_pool::FloatKind::of(pool, ty).is_some() {
                return Ok(pool.float_value(ty, bits));
            }
            Ok(pool.int_value(ty, bits))
        }
    }
}

/// A result reduced to something that outlives the VM.
///
/// The VM's memory is released when it is dropped, so a value that *points* into it
/// has to be copied before then. Two steps rather than one because interning needs
/// `&mut Pool` and the VM holds `&Pool`.
enum Raw {
    Void,
    Bool(bool),
    /// A struct or fixed-array constant's **byte image**, copied out of the VM (ADR-0074 §1).
    ///
    /// Bytes rather than interned elements, because interning needs `&mut Pool` and the VM holds `&Pool`
    /// — the same two-step this whole type exists for. `intern_aggregate` turns them into element values
    /// once the VM is gone.
    Aggregate(Vec<u8>),
    Int(u64),
    /// A float's raw IEEE-754 bits, kept distinct from [`Raw::Int`] even though the VM holds both as
    /// a scalar (ADR-0040 §3).
    ///
    /// The distinction is the whole of the fix: without it a float constant interned as an
    /// `Item::IntValue` carrying a `float64` type, and the native back end emitted `iconst` on an
    /// `F64` register.
    Float(u64),
    Str(Vec<u8>),
}

/// Copies a result out of the VM.
fn reduce(vm: &Vm<'_>, value: &Value, ty: PoolId, is_float: bool) -> Result<Raw, VmError> {
    match value {
        Value::Void => Ok(Raw::Void),
        Value::Scalar(bits) => {
            if ty == PoolId::BOOL {
                return Ok(Raw::Bool(*bits != 0));
            }
            // **A float is a scalar in the VM** — ADR-0040 §3's "a float is its bits, and the
            // interpretation comes from the type" — so mapping every scalar to `Raw::Int` interned a
            // float constant as an `Item::IntValue` whose type was `float64`. `jr-codegen-clif` then
            // emitted `iconst` with an `F64` Cranelift type, and Cranelift's `iconst_bounds` verifier
            // panicked with "entered unreachable code" a long way from here.
            //
            // The VM read it back correctly, because it too takes the interpretation from the type —
            // so `jr run` gave the right answer while `jr build` crashed, which is the two engines
            // disagreeing about what *compiles* rather than about what a program computes. The
            // differential harness cannot see that: a program that does not build produces no output.
            //
            // Latent since floats landed (ADR-0040): no corpus file had a float `::` constant, and a
            // float *local* never comes through here at all.
            if is_float {
                return Ok(Raw::Float(*bits));
            }
            Ok(Raw::Int(*bits))
        }
        Value::Aggregate(_) if ty == PoolId::STRING => Ok(Raw::Str(vm.read_string(value)?)),
        // A struct or array computed at compile time (ADR-0074 §1). The **bytes** are copied out here,
        // because that is all the VM can give while it borrows the pool; turning them into interned
        // element values happens after, in `intern_aggregate`, which needs `&mut Pool`. A union is
        // refused there rather than here, because deciding needs the pool.
        Value::Aggregate(bytes) => Ok(Raw::Aggregate(bytes.clone())),
        Value::Undefined => Err(VmError::unsupported_public(
            "the expression evaluated to no value",
        )),
    }
}
