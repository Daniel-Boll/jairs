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

/// A `$N` comptime-value argument that is not a compile-time constant (ADR-0088 §2).
///
/// Owned by this crate rather than by `jr-sema` for the same reason E0230 is: constancy of a value is a
/// const-eval judgement, and this is where the evaluator's failure becomes a diagnostic. Defined here
/// beside E0230, listed in `AGENTS.md`'s registry as `jr-db`'s.
const E0271: &str = "E0271";

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
    /// The **argument to a `$N` comptime-value parameter** at a call site (ADR-0088 §2).
    ///
    /// `make(5)` — the call needs `5` evaluated to a constant at compile time so the instantiation can
    /// bake it into a clone. Evaluated by the same thunk `BodyRun` and `InsertOperand` use, keyed
    /// additionally by the call's span, `(scope, call ExprId)` and the parameter *index* — because one
    /// call may pass several `$N` arguments and the instantiation reads them in parameter order. The
    /// scope is either a body or top-level, exactly as `BodyRun` distinguishes.
    ComptimeArg(
        ItemId,
        ExprScope,
        /// The call's expression id (for the span, read out of the same arena `scope` names).
        ExprId,
        /// The argument's expression id (this is `Wanted::expr`, the thing to evaluate).
        ExprId,
        /// Which comptime parameter this argument feeds, in the template's parameter order.
        ///
        /// **Carried for the debug-print of a `Wanted` and for future diagnostic help that names the
        /// parameter position**; not read by the evaluator, which reads only `Wanted::expr`. Kept as a
        /// distinct field rather than merged with the argument id, because a call may pass several `$N`
        /// arguments and the reader wants to know which parameter each one feeds.
        #[allow(dead_code)]
        u32,
    ),
}

impl Wanted {
    const fn expr(self) -> ExprId {
        match self {
            Self::Item(_, expr)
            | Self::Run(_, expr)
            | Self::BodyRun(_, _, expr)
            | Self::TypeAlias(_, expr)
            | Self::InsertOperand(_, _, expr, _) => expr,
            // The *argument* is what to evaluate; the call id is auxiliary and lives in the last fields.
            Self::ComptimeArg(_, _, _, arg, _) => arg,
        }
    }

    const fn item(self) -> ItemId {
        match self {
            Self::Item(item, _)
            | Self::Run(item, _)
            | Self::BodyRun(item, _, _)
            | Self::TypeAlias(item, _)
            | Self::InsertOperand(item, _, _, _)
            | Self::ComptimeArg(item, _, _, _, _) => item,
        }
    }

    /// Which expression arena [`Wanted::expr`] indexes (ADR-0069 §2).
    const fn scope(self) -> ExprScope {
        match self {
            Self::Item(_, _) | Self::Run(_, _) | Self::TypeAlias(_, _) => ExprScope::TopLevel,
            Self::BodyRun(_, body, _) | Self::InsertOperand(_, body, _, _) => ExprScope::Body(body),
            Self::ComptimeArg(_, scope, _, _, _) => scope,
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
fn wanted(
    hir: &FileHir,
    signatures: &jr_sema::FileSignatures,
    comptime_calls: &crate::sema::ComptimeCalls,
) -> Vec<Wanted> {
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
    // And every **comptime-value argument** the checker recorded (ADR-0088 §2). Each call contributes one
    // target per `$N` parameter, in parameter order, so `comptime_call_values` can zip results back into
    // that order per call. The item id is looked up per call — the checker's key is `(scope, call)`, but
    // an item id is needed only to place a diagnostic, so a call in a body uses its enclosing proc's item,
    // and a call at top level uses its enclosing `#run`/const item.
    //
    // Deterministic iteration order: `FxHashMap` is not stable, so the list is sorted by scope then id.
    // That agreement matters because a snapshot of `ConstValues` depends on the order the round-robin
    // saw its targets in.
    type SortedCall = ((ExprScope, ExprId), (jr_hir::ProcId, Vec<ExprId>));
    let mut sorted: Vec<SortedCall> = comptime_calls
        .iter()
        .map(|(k, v)| (*k, v.clone()))
        .collect();
    sorted.sort_by_key(|((scope, call), _)| (scope_ord(*scope), call.index()));
    let placeholder_item = ItemId::from_usize(0);
    for ((scope, call), (_proc, args)) in &sorted {
        for (i, arg) in args.iter().enumerate() {
            out.push(Wanted::ComptimeArg(
                item_for_scope(hir, *scope).unwrap_or(placeholder_item),
                *scope,
                *call,
                *arg,
                u32::try_from(i).unwrap_or(u32::MAX),
            ));
        }
    }
    out
}

/// The item id enclosing a scope, for placing a diagnostic on a `Wanted::ComptimeArg`.
///
/// A top-level scope has no single enclosing item (a `Wanted::ComptimeArg` at top level would be a
/// comptime call in a top-level `#run`, whose item *is* the `#run`); the resolver already keys those on
/// the same `(TopLevel, ExprId)`, so returning `None` here is safe — the eventual diagnostic falls back
/// to the file's first item, which is fine for a compiler bug that should have been refused earlier.
fn item_for_scope(hir: &FileHir, scope: ExprScope) -> Option<ItemId> {
    match scope {
        ExprScope::TopLevel => None,
        ExprScope::Body(body_id) => {
            hir.items
                .iter()
                .enumerate()
                .find_map(|(index, item)| match &item.kind {
                    ItemKind::Const {
                        value: ConstValue::Proc(proc),
                    } if hir
                        .procs
                        .get(proc.index())
                        .and_then(|p| p.body)
                        .is_some_and(|b| b == body_id) =>
                    {
                        Some(ItemId::from_usize(index))
                    }
                    _ => None,
                })
        }
    }
}

/// Total order over an `ExprScope` for deterministic iteration.
fn scope_ord(scope: ExprScope) -> (u32, u32) {
    match scope {
        ExprScope::TopLevel => (0, 0),
        ExprScope::Body(b) => (1, b.as_u32()),
    }
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
    let checked_file = checked(db, file, search_paths);
    // **Signatures first**, because `wanted` asks them whether a `::` initialiser names a type
    // (ADR-0071 §2). It also asks the checker's `comptime_calls` (ADR-0088 §2), which is why the
    // `checked` fetch above was moved ahead of this line.
    let targets = wanted(
        hir.as_ref(),
        signatures.signatures.as_ref(),
        &checked_file.comptime_calls,
    );
    // **A `type_info(T)` call needs a value too** (ADR-0075 §2), so the early return has to account for
    // one: a file whose only compile-time work is a `type_info` has no `wanted` target at all, and
    // returning here left its call unfolded — which `scan` then refused as "a name failed to resolve",
    // the callee naming no procedure. Found by running the feature's own probe.
    if targets.is_empty()
        && checked_file.type_info_calls.is_empty()
        && checked_file.folded_calls.is_empty()
        && checked_file.any_calls.is_empty()
        && checked_file.pointer_views.is_empty()
    {
        return ConstResult {
            values: Arc::new(ConstValues::new()),
            diagnostics: Arc::new(Diagnostics::new()),
        };
    }

    let types = checked_file.types.clone();
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
    let mut type_info_failures: Vec<String> = Vec::new();

    // **`type_info(T)` folds to an interned `Type_Info` aggregate** (ADR-0075 §2), built here because
    // this is where the pool is mutable and the described type is known. It needs no VM: every field is
    // something the pool can already answer — the kind from the type's own `Item`, the name from the
    // signatures, and the size and alignment from `layout_of`. So it is not a round-robin target and
    // cannot fail to converge; it is recorded before the loop and is simply available.
    //
    // Keyed as a `run` value, the channel a `#run` already uses, so `jr-mir` reads it with the mechanism
    // it has rather than a second one.
    // **A call `jr-sema` already folded** (ADR-0099 §2) needs only copying: the value is interned, and
    // sema had everything it needed to compute it. Keyed as a `run` value like `type_info`'s, and for the
    // same reason — `jr-mir` already replaces a `run`-keyed call with its constant and never emits the
    // callee, so a second channel would be a second thing to keep in step.
    for ((scope, expr), value) in checked_file.folded_calls.iter() {
        values.set_run(*scope, *expr, *value);
    }

    for ((scope, expr), described) in checked_file.type_info_calls.iter() {
        // The imported signatures are searched **as well as** this file's, because `Type_Info` is
        // declared in `Basic` and so is almost never local: looking only at the own file reported
        // "`Type_Info` is not usable" for every correct program, found by running the probe.
        let mut all_sigs: Vec<&jr_sema::FileSignatures> = vec![signatures.signatures.as_ref()];
        all_sigs.extend(modules.iter().map(|m| m.signatures.as_ref()));
        match type_info_value(&mut pool, interner, &all_sigs, *described) {
            Ok(value) => values.set_run(*scope, *expr, value),
            // Sema already refused a type with no layout (E0266), so a failure here is an internal
            // inconsistency rather than a user error. Reported as E0230 like any other const-eval
            // failure and never lowered to a placeholder: without a recorded value `scan` refuses the
            // body, which is the honest outcome.
            Err(why) => type_info_failures.push(why),
        }
    }

    // **`any_of` and `any_as` lower to real code** (ADR-0076), unlike `type_info` which folds to a
    // constant — so they record *how* to lower rather than a value. `any_of` needs a `Type_Info`
    // constant to spill for its `type` field, built here where the pool is mutable; `any_as` needs only
    // the expected type's id (ADR-0077), which is the pool id itself.
    // **A `typed`/`untyped` call carries its result pointer type through** (ADR-0106 §1). Copied rather than
    // computed, because sema already resolved the type argument — and it rides `ConstValues` for the reason
    // `any_calls` does: that struct is what `jr-mir` receives, so a fourth channel would be a fourth thing to
    // thread.
    for ((scope, expr), ty) in checked_file.pointer_views.iter() {
        values.set_pointer_view(*scope, *expr, *ty);
    }

    for ((scope, expr), (op, ty)) in checked_file.any_calls.iter() {
        let mut all_sigs: Vec<&jr_sema::FileSignatures> = vec![signatures.signatures.as_ref()];
        all_sigs.extend(modules.iter().map(|m| m.signatures.as_ref()));
        match op {
            jr_sema::AnyOp::Of => {
                // The `Any` struct type to build, looked up like `Type_Info` — both live in `Basic`.
                let any_ty = interner.intern("Any");
                let any_ty = all_sigs
                    .iter()
                    .find_map(|sigs| sigs.lookup(any_ty))
                    .and_then(|e| e.type_value);
                match (type_info_value(&mut pool, interner, &all_sigs, *ty), any_ty) {
                    (Ok(type_info), Some(any_ty)) => {
                        values.set_any_op(
                            *scope,
                            *expr,
                            jr_mir::AnyLowering::Of { type_info, any_ty },
                        );
                    }
                    (Err(why), _) => type_info_failures.push(why),
                    (_, None) => {
                        type_info_failures
                            .push("the standard library's `Any` is not usable".to_owned());
                    }
                }
            }
            jr_sema::AnyOp::As => {
                values.set_any_op(
                    *scope,
                    *expr,
                    jr_mir::AnyLowering::As {
                        type_id: u64::from(ty.as_u32()),
                        result: *ty,
                    },
                );
            }
        }
    }

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
        // A comptime-value argument that failed to evaluate is a **specific** refusal, not the generic
        // E0230: the reader wanted to pass a value at a call site, and one that is not a compile-time
        // constant is a well-known category (like a non-literal array length, ADR-0070 §1). Reported at
        // the call's span, read from the arena the target names — a `Body` scope reads from that body,
        // top-level from the file arena. Handled *before* the generic E0230 loop so this specific code
        // wins (ADR-0088 §2, E0271).
        if let Wanted::ComptimeArg(_, scope, call, _, _) = target {
            let span = match scope {
                ExprScope::TopLevel => hir.expr_spans.get(call.index()).copied(),
                ExprScope::Body(body_id) => hir
                    .bodies
                    .get(body_id.index())
                    .and_then(|b| b.expr_spans.get(call.index()).copied()),
            }
            .or_else(|| hir.items.first().map(|item| item.span));
            let Some(span) = span else { continue };
            diagnostics.push(
                Diagnostic::error(
                    span,
                    format!(
                        "a `$N` comptime-value argument must be a compile-time constant: {reason}"
                    ),
                )
                .with_code(E0271)
                .with_note(
                    "`$N`'s value is baked into the instantiation, so the argument is evaluated at \
                     compile time — the same rule as an array length (ADR-0088)",
                ),
            );
            continue;
        }
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

    // A `type_info` that sema accepted and this could not build (ADR-0075 §2). Placed at the file's
    // first item rather than at the call, because the value is keyed by expression and this query does
    // not hold the body's spans — and it is unreachable in practice, since sema has already checked the
    // layout and the struct's shape. Reported rather than dropped so the case cannot be silent.
    for reason in type_info_failures {
        let Some(span) = hir.items.first().map(|item| item.span) else {
            continue;
        };
        diagnostics.push(
            Diagnostic::error(span, format!("compile-time evaluation failed: {reason}"))
                .with_code(E0230)
                .with_note("`type_info` builds a `Type_Info` value at compile time (ADR-0075 §2)"),
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
    let checked_file = checked(db, file, search_paths);
    let mut operands = jr_hir::InsertOperands::new();

    // Re-walk the same targets `file_consts` evaluated, keeping only the insert operands — each carries
    // the directive span this map is keyed by. The value is in `consts.values` under the operand's
    // `(Body, ExprId)` key, exactly where `record` put it.
    let pool = crate::sema::lock_pool(db);
    for target in wanted(
        hir.as_ref(),
        signatures.signatures.as_ref(),
        &checked_file.comptime_calls,
    ) {
        let Wanted::InsertOperand(_, body, expr, span) = target else {
            continue;
        };
        // **A folded operand is found by span first** (ADR-0101 §3). `noted_insert(…)` and the other note
        // intrinsics are folded by *sema*, and the value is stored under the id sema saw — but this walk
        // re-derives targets from the HIR, and a body containing *two* computed `#insert`s has its ids
        // renumbered by the first splice, so the second one's `(body, expr)` key missed. It then stayed
        // pending, MIR read a hole in the type map, and the failure surfaced as the verifier panicking with
        // `mixed operand types` rather than as any diagnostic: the "well-typed placeholder" family AGENTS.md
        // names, and the reason the insert-operand map itself is keyed by span (ADR-0072 §2).
        let Some(value) = checked_file
            .folded_call_spans
            .get(&span)
            .copied()
            .or_else(|| consts.values.run(ExprScope::Body(body), expr))
        else {
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
        // A comptime argument is stored under the same `(scope, expr)` key `Run`/`BodyRun` use, because
        // it *is* a run-shape evaluation of one expression — see `record` (ADR-0088 §2).
        Wanted::ComptimeArg(_, scope, _, arg, _) => values.run(scope, arg).is_some(),
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
        // A comptime argument evaluates as an ordinary `run` in its scope (ADR-0088 §2). Storing it here
        // is exactly the `Run`/`BodyRun` path, keyed on the argument's own expression — because the
        // instantiation pass reads it back by `(scope, argument ExprId)` while walking `comptime_calls`,
        // so there is only one lookup pattern.
        Wanted::ComptimeArg(_, scope, _, arg, _) => values.set_run(scope, arg, value),
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

    // **A body this file could not lower has no routine, and calling one must not surface as an ICE.**
    //
    // `add_file` skips a refused body (it has no bytecode to add), so a thunk that calls one reached the
    // VM's `no routine for file N proc M` — compiler internals shown to someone who wrote a reasonable
    // program. That is the third instance of this shape; ADR-0069 fixed two.
    //
    // The case that actually reaches here is a `#run` calling a procedure that reads an **imported
    // constant**: `ImportedValues` is deliberately empty during const-eval (see the `lower_file` call
    // below for why), so such a body is refused, and the refusal is the honest thing to report. The
    // reason is taken from the outcome rather than invented, so it says *which* construct was not
    // lowerable.
    let refused: Vec<String> = mir
        .iter()
        .filter_map(|(_proc, outcome)| match outcome {
            Ok(_) => None,
            // Named by index rather than by identifier: this function has no interner, and the
            // *reason* is what a reader acts on. `jr-db`'s own unlowerable-body warning (E0245) names the
            // procedure, so the two together identify it.
            Err(Poisoned::Here(reason)) => {
                Some(format!("a body in this file was refused: {reason}"))
            }
            Err(Poisoned::Transitive(_)) => {
                Some("a body in this file depends on another that was refused".to_owned())
            }
        })
        .collect();
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
        // A failure here is reported with the refused bodies *this file* has, when there are any: the
        // VM's own message for a missing routine is an internal one, and the refusal is the real cause
        // (see `refused` above). When nothing was refused, the VM's message stands — it is then a genuine
        // trap or an unsupported operation, which is information rather than internals.
        let value = vm.call(thunk_proc, Vec::new()).map_err(|e| {
            if refused.is_empty() {
                e.to_string()
            } else {
                // The **first** reason only. Every refused body in the file is collected, but a reader
                // needs the cause, not an inventory — and listing several made the one-line message
                // unreadable. `jr-db`'s E0245 warning names each unlowerable body separately, so the
                // full set is still reachable.
                format!(
                    "it calls a procedure this compiler could not lower at compile time — {}",
                    refused[0]
                )
            }
        })?;
        reduce(&vm, pool, &value, ty, is_float).map_err(|e| e.to_string())?
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
        Raw::Aggregate(ref elements) => intern_aggregate(pool, ty, elements)?,
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
fn intern_aggregate(pool: &mut Pool, ty: PoolId, raws: &[Raw]) -> Result<PoolId, String> {
    // Element *types* only: the values themselves already came out of the VM as a tree, so this no longer
    // slices a byte image at offsets. Taken before interning begins so the immutable borrow ends first.
    let placements = aggregate_placements(pool, ty)?;
    if placements.len() != raws.len() {
        return Err(format!(
            "a compile-time aggregate has {} elements, expected {}",
            raws.len(),
            placements.len()
        ));
    }

    let mut elements = Vec::with_capacity(raws.len());
    for ((elem_ty, _, _), raw) in placements.into_iter().zip(raws) {
        elements.push(intern_element(pool, elem_ty, raw)?);
    }
    Ok(pool.aggregate_value(ty, elements))
}

/// Builds the `Type_Info` constant describing `described` (ADR-0075 §2).
///
/// **No VM is involved**, which is what makes `type_info` cheap and total: every field is something the
/// pool can already answer. The kind comes from the described type's own `Item`, the name from the
/// signatures' `type_name` (falling back to the compiler's own rendering for a builtin, which has no
/// declaration to have recorded a name), and the size and alignment from `layout_of`.
///
/// The result is an `Item::AggregateValue` — the representation ADR-0074 added — whose `name` element is
/// an `Item::StrValue`, which is the thing ADR-0075 §1 had to make possible first.
///
/// The layout is asked for `LP64`, matching the target of every engine in the slice. That is a **target
/// fact inside a compile-time value**, and it is the one place this wave knowingly bakes one in: a
/// `Type_Info` reports a *size*, so it cannot be target-independent the way ADR-0074 §1 kept the rest of
/// the pool. Recorded here rather than hidden, because a second target would need this reconsidered.
pub(crate) fn type_info_value(
    pool: &mut Pool,
    interner: &jr_base::Interner,
    signatures: &[&jr_sema::FileSignatures],
    described: PoolId,
) -> Result<PoolId, String> {
    let target = jr_pool::TargetLayout::LP64;
    let layout = jr_pool::layout_of(pool, target, described)
        .map_err(|e| format!("`type_info`'s argument has no layout: {e}"))?;

    let kind_name = type_info_kind_name(pool, described)
        .ok_or_else(|| "`type_info` cannot describe this shape".to_owned())?;
    // A **declared** type's name comes from the signatures, which recorded it; a builtin has no
    // declaration, so its spelling is derived from its `Item`. Only the shapes a `Type_Info` can describe
    // need a name here, because the others were refused above.
    let name = signatures
        .iter()
        .find_map(|sigs| sigs.type_name(described))
        .map(ToOwned::to_owned)
        .or_else(|| builtin_type_name(pool, described))
        .unwrap_or_else(|| kind_name.to_lowercase());

    // The `kind` field's own type and value, read from the `Type_Info_Kind` enum declared beside
    // `Type_Info` in `Basic`. Read rather than assumed: the member values are the enum's, so a
    // reordering of the declaration changes the numbers and this follows it.
    let info_ty = type_info_struct_type(interner, signatures)
        .ok_or_else(|| "the standard library's `Type_Info` is not usable".to_owned())?;
    let fields = struct_fields_of(pool, info_ty)
        .ok_or_else(|| "`Type_Info`'s fields are not recorded".to_owned())?;
    // The `kind` field's type, found **by name** rather than by position: ADR-0077 added `id` at the
    // front, so `kind` is no longer field 0, and reading it by index silently interned the enum value
    // into the wrong field's type. Sema's `TYPE_INFO_FIELDS` already pins the names, so a lookup here
    // cannot disagree with a `Type_Info` that passed validation.
    let kind_name_sym = interner.intern("kind");
    let kind_ty = fields
        .iter()
        .find(|f| f.name == kind_name_sym)
        .map(|f| f.ty)
        .ok_or_else(|| "`Type_Info` has no `kind` field".to_owned())?;
    let kind_value = enum_member_value(pool, interner, kind_ty, kind_name).ok_or_else(|| {
        format!("`Type_Info_Kind` has no member `{kind_name}`, which the compiler expects")
    })?;

    // The `id` field is the described type's own `PoolId`, widened to `s64` (ADR-0077 §1): the identity
    // `any_as` compares. It is `described` itself — the pool id the whole compiler already uses — so two
    // `type_info(T)` calls agree while two distinct types never do, and both engines see the same value
    // because they share one pool.
    let id = u64::from(described.as_u32());

    // The fixed-size per-kind facts (ADR-0078 §1, §3), read from the pool the builder already consults.
    // `count` is a struct/union/variant field count or an array length; `element` is an array's element
    // or a pointer's pointee, as a type id. Both 0 for a kind that has neither.
    let (count, element) = match *pool.item(described) {
        jr_pool::Item::ArrayType { elem, len } => (len, u64::from(elem.as_u32())),
        jr_pool::Item::PointerType(pointee) => (0, u64::from(pointee.as_u32())),
        jr_pool::Item::StructType { .. }
        | jr_pool::Item::UnionType { .. }
        | jr_pool::Item::VariantType { .. } => {
            let n = pool.fields_of(described).map_or(0, <[_]>::len);
            (n as u64, 0)
        }
        // Every other kind has neither a count nor an element (a procedure's parameter list is the
        // variable-length member ADR-0078 §3 deliberately does not summarise here).
        _ => (0, 0),
    };

    let elements = vec![
        pool.int_value(PoolId::S64, id),
        pool.int_value(kind_ty, kind_value),
        pool.str_value(&name),
        pool.int_value(PoolId::S64, layout.size),
        pool.int_value(PoolId::S64, u64::from(layout.align)),
        pool.int_value(PoolId::S64, count),
        pool.int_value(PoolId::S64, element),
    ];
    Ok(pool.aggregate_value(info_ty, elements))
}

/// The source spelling of a **builtin** type, which has no declaration to have recorded a name.
///
/// Only the scalar builtins are answered. A composite — `*Point`, `[2]s64` — would need its element
/// rendered too, and ADR-0075 §3 leaves per-kind detail out of this wave, so such a type falls back to
/// its kind rather than to a half-built spelling that looks like a real name.
fn builtin_type_name(pool: &Pool, ty: PoolId) -> Option<String> {
    match *pool.item(ty) {
        jr_pool::Item::VoidType => Some("void".to_owned()),
        jr_pool::Item::BoolType => Some("bool".to_owned()),
        jr_pool::Item::IntType { signed, bits } => {
            Some(format!("{}{bits}", if signed { 's' } else { 'u' }))
        }
        jr_pool::Item::FloatType { bits } => Some(format!("float{bits}")),
        jr_pool::Item::StringType => Some("string".to_owned()),
        _ => None,
    }
}

/// The `Type_Info_Kind` member name for a type's shape (ADR-0075 §3).
///
/// Exhaustive over `Item` rather than using a `_` arm, so that adding a type variant is a compile error
/// here — the discipline that has caught real bugs in this project. A *value* variant is not a type and
/// answers `None`, as does a type with no runtime form, which sema has already refused with E0266.
fn type_info_kind_name(pool: &Pool, ty: PoolId) -> Option<&'static str> {
    match *pool.item(ty) {
        jr_pool::Item::VoidType => Some("VOID"),
        jr_pool::Item::BoolType => Some("BOOL"),
        jr_pool::Item::IntType { .. } => Some("INTEGER"),
        jr_pool::Item::FloatType { .. } => Some("FLOAT"),
        jr_pool::Item::StringType => Some("STRING"),
        jr_pool::Item::PointerType(..) => Some("POINTER"),
        jr_pool::Item::ArrayType { .. } => Some("ARRAY"),
        jr_pool::Item::ViewType { .. } => Some("VIEW"),
        // A dynamic array reports as its own kind so a `type_info(...).kind ==
        // Type_Info_Kind.DYNAMIC_ARRAY` reads. The `Type_Info_Kind` enum in
        // `modules/Basic` will need this member added for the reflection to work; it
        // reports `Some` here so a program *can* be written that reads the kind, without
        // making the pool report a plausible-but-wrong `VIEW` instead.
        jr_pool::Item::DynamicArrayType { .. } => Some("DYNAMIC_ARRAY"),
        jr_pool::Item::StructType { .. } => Some("STRUCT"),
        jr_pool::Item::UnionType { .. } => Some("UNION"),
        jr_pool::Item::VariantType { .. } => Some("VARIANT"),
        jr_pool::Item::EnumType { .. } => Some("ENUM"),
        jr_pool::Item::ProcType { .. } => Some("PROCEDURE"),
        // No runtime form, so no `Type_Info`: sema refuses these with E0266 before reaching here.
        jr_pool::Item::TypeType
        | jr_pool::Item::ErrorType
        | jr_pool::Item::ForeignLibraryType
        | jr_pool::Item::ContextType
        | jr_pool::Item::ResultsType { .. } => None,
        // A value is not a type.
        jr_pool::Item::VoidValue
        | jr_pool::Item::BoolValue(..)
        | jr_pool::Item::IntValue { .. }
        | jr_pool::Item::FloatValue { .. }
        | jr_pool::Item::StrValue(..)
        | jr_pool::Item::TypeValue(..)
        | jr_pool::Item::ProcValue { .. }
        | jr_pool::Item::ForeignLibraryValue(..)
        | jr_pool::Item::AggregateValue { .. } => None,
    }
}

/// Looks `Type_Info` up in the signatures, without validating it.
///
/// The validation lives in `jr-sema`, which reports E0265 — this runs after that check has passed, so a
/// `None` here means the same thing and is reported as a const-eval failure.
fn type_info_struct_type(
    interner: &jr_base::Interner,
    signatures: &[&jr_sema::FileSignatures],
) -> Option<PoolId> {
    let name = interner.intern("Type_Info");
    signatures
        .iter()
        .find_map(|sigs| sigs.lookup(name))
        .and_then(|entry| entry.type_value)
}

/// The fields of a struct type, if it is one and they are recorded.
fn struct_fields_of(pool: &Pool, ty: PoolId) -> Option<Vec<jr_pool::Field>> {
    let jr_pool::Item::StructType { .. } = *pool.item(ty) else {
        return None;
    };
    pool.fields_of(ty).map(<[_]>::to_vec)
}

/// The value of a named member of an enum type.
fn enum_member_value(
    pool: &Pool,
    interner: &jr_base::Interner,
    enum_ty: PoolId,
    member: &str,
) -> Option<u64> {
    let jr_pool::Item::EnumType { decl, .. } = *pool.item(enum_ty) else {
        return None;
    };
    pool.enum_members(decl)?
        .iter()
        .find(|m| interner.resolve(m.name) == member)
        // `EnumMember::value` is an `i64` and `int_value` takes raw bits, so the cast is the
        // two's-complement encoding rather than a conversion — the same move `Colour.RED` makes.
        .map(|m| m.value as u64)
}

/// Interns one already-reduced element of an aggregate constant (ADR-0074 §1, ADR-0075 §1).
///
/// Every decode happened in `reduce_element`, while the VM was alive; this only turns a [`Raw`] into a
/// `PoolId`. **A `string` element is no longer refused** — that refusal existed because a flat byte image
/// held a dangling `{data, count}` pair, and ADR-0075 §1 removed the flat image rather than the string.
///
/// The element's *type* is still passed in and used, because a `Raw` does not carry one: `Raw::Int(0)` is
/// `0` at whatever width the field has.
fn intern_element(pool: &mut Pool, ty: PoolId, raw: &Raw) -> Result<PoolId, String> {
    match raw {
        Raw::Void => Ok(PoolId::VOID_VALUE),
        Raw::Bool(value) => Ok(pool.bool_value(*value)),
        Raw::Int(bits) => Ok(pool.int_value(ty, *bits)),
        Raw::Float(bits) => Ok(pool.float_value(ty, *bits)),
        Raw::Str(bytes) => {
            let text = std::str::from_utf8(bytes)
                .map_err(|_| "a compile-time string was not valid UTF-8".to_owned())?;
            Ok(pool.str_value(text))
        }
        Raw::Aggregate(elements) => intern_aggregate(pool, ty, elements),
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
    /// A struct or fixed-array constant, **element by element** (ADR-0074 §1, ADR-0075 §1).
    ///
    /// A *tree* rather than the flat byte image this started as, and that is the whole of ADR-0075 §1: a
    /// `string` field's bytes are a `{data, count}` pair pointing **into the VM's memory**, so a flat
    /// image could not be interned once the VM was dropped — the pointer was already dangling, which is
    /// why ADR-0074 §2 refused the case outright. Reducing each element *while the VM is alive* resolves
    /// such a field to owned text ([`Raw::Str`]) at the one moment it can still be read.
    ///
    /// Still not interned here, for the reason this whole type exists: interning needs `&mut Pool` and
    /// the VM holds `&Pool`. `intern_aggregate` turns the tree into element values once the VM is gone,
    /// and no longer has to slice bytes at offsets to do it.
    Aggregate(Vec<Raw>),
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
///
/// Takes the pool explicitly rather than reaching through the VM, because an aggregate's element types and
/// offsets are pool questions (ADR-0075 §1) and the walk has to happen here — inside the VM's lifetime —
/// so that a `string` element can be resolved to its text before the memory it points at is released.
fn reduce(
    vm: &Vm<'_>,
    pool: &Pool,
    value: &Value,
    ty: PoolId,
    is_float: bool,
) -> Result<Raw, VmError> {
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
        // A struct or array computed at compile time (ADR-0074 §1), walked **element by element while the
        // VM is alive** (ADR-0075 §1). This used to clone the byte image and let `intern_aggregate` slice
        // it afterwards, which could not work for a `string` field: its bytes are a `{data, count}` pair
        // into VM memory that is gone by then. Recursing here reaches `read_string` at the one moment the
        // pointer is still valid. A union is still refused, and still by the shared placement walk, so
        // there is one answer to "which shapes have readable elements".
        Value::Aggregate(bytes) => {
            let mut elements = Vec::new();
            for (elem_ty, offset, size) in
                aggregate_placements(pool, ty).map_err(VmError::unsupported_public)?
            {
                let start = usize::try_from(offset).unwrap_or(0);
                let end = start.saturating_add(usize::try_from(size).unwrap_or(0));
                let slice = bytes.get(start..end).ok_or_else(|| {
                    VmError::unsupported_public(
                        "a compile-time aggregate is shorter than its layout",
                    )
                })?;
                // A nested value is reduced through the same path, so a string two levels down works for
                // the same reason one level down does.
                let elem_value = Value::Aggregate(slice.to_vec());
                let elem_is_float = jr_pool::FloatKind::of(pool, elem_ty).is_some();
                elements.push(reduce_element(
                    vm,
                    pool,
                    &elem_value,
                    elem_ty,
                    elem_is_float,
                )?);
            }
            Ok(Raw::Aggregate(elements))
        }
        Value::Undefined => Err(VmError::unsupported_public(
            "the expression evaluated to no value",
        )),
    }
}

/// Reduces one element of an aggregate, whose bytes arrive as a slice rather than as a [`Value`].
///
/// Separate from [`reduce`] because the two receive an element differently: `reduce`'s scalar arrives as a
/// `Value::Scalar` the VM built, while an element's arrives as **bytes at an offset** that must be decoded
/// little-endian — the same byte order `jr-vm`'s `write_le` wrote, so the two directions share one answer.
///
/// A `string`, a nested aggregate and a `void` each delegate: a string to the VM's `read_string` (which is
/// the point of doing this inside the VM's lifetime), an aggregate back to [`reduce`], and `void` to the
/// unit value.
fn reduce_element(
    vm: &Vm<'_>,
    pool: &Pool,
    value: &Value,
    ty: PoolId,
    is_float: bool,
) -> Result<Raw, VmError> {
    if ty == PoolId::STRING {
        return Ok(Raw::Str(vm.read_string(value)?));
    }
    match *pool.item(ty) {
        jr_pool::Item::ArrayType { .. }
        | jr_pool::Item::StructType { .. }
        | jr_pool::Item::UnionType { .. }
        | jr_pool::Item::VariantType { .. } => reduce(vm, pool, value, ty, is_float),
        jr_pool::Item::VoidType => Ok(Raw::Void),
        // **A pointer or a view element is refused** rather than interned as a scalar.
        //
        // This arm exists because the scalar fallback below silently accepted both, and the result was a
        // *wrong answer with no diagnostic*: a `#run` returning `struct { p: *s64; n: s64; }` interned the
        // VM's own address as a plain integer, and reading `V.p.*` afterwards gave **48** in the VM and a
        // **segfault** natively — two different wrong answers, neither reported. The corpus differential
        // was blind to it because no corpus file held a pointer in a constant aggregate, which is exactly
        // the gap `AGENTS.md` names ("if a construct is legal in the corpus, something must execute it").
        //
        // The reason is ADR-0074 §2's, which already refused `string` as an *aggregate* element on the
        // same ground — "its runtime form is a pointer, which has no compile-time value at all" — and
        // simply had not been extended to a raw pointer or a view. A compile-time pointer addresses the
        // VM's memory, which does not exist at run time; relocating the pointee into interned data would
        // silently change what the program points *at*, so the honest answer is to refuse.
        //
        // A `string` is unaffected: it is handled above by contents, not as a pointer.
        jr_pool::Item::PointerType(..) => Err(VmError::unsupported_public(
            "a compile-time aggregate holding a pointer has no runtime meaning: the address is the \
             compile-time evaluator's, not the program's",
        )),
        jr_pool::Item::ViewType { .. } => Err(VmError::unsupported_public(
            "a compile-time aggregate holding a view has no runtime meaning: the view's data pointer is \
             the compile-time evaluator's, not the program's",
        )),
        jr_pool::Item::DynamicArrayType { .. } => Err(VmError::unsupported_public(
            "a compile-time aggregate holding a `[..]T` has no runtime meaning: its data pointer is \
             the compile-time evaluator's, not the program's",
        )),
        // Every remaining shape is a scalar held in the element's own bytes.
        jr_pool::Item::BoolType
        | jr_pool::Item::IntType { .. }
        | jr_pool::Item::FloatType { .. }
        | jr_pool::Item::StringType
        | jr_pool::Item::TypeType
        | jr_pool::Item::ErrorType
        | jr_pool::Item::ForeignLibraryType
        | jr_pool::Item::ContextType
        | jr_pool::Item::ResultsType { .. }
        | jr_pool::Item::EnumType { .. }
        | jr_pool::Item::ProcType { .. }
        | jr_pool::Item::VoidValue
        | jr_pool::Item::BoolValue(..)
        | jr_pool::Item::IntValue { .. }
        | jr_pool::Item::FloatValue { .. }
        | jr_pool::Item::StrValue(..)
        | jr_pool::Item::TypeValue(..)
        | jr_pool::Item::ProcValue { .. }
        | jr_pool::Item::ForeignLibraryValue(..)
        | jr_pool::Item::AggregateValue { .. } => {
            let bytes = value.aggregate()?;
            let mut buf = [0u8; 8];
            let take = bytes.len().min(8);
            buf[..take].copy_from_slice(&bytes[..take]);
            let bits = u64::from_le_bytes(buf);
            if ty == PoolId::BOOL {
                return Ok(Raw::Bool(bits != 0));
            }
            if is_float {
                return Ok(Raw::Float(bits));
            }
            Ok(Raw::Int(bits))
        }
    }
}

/// Where each element of an aggregate type sits: its type, byte offset and size.
///
/// **One** answer to "which shapes have readable elements, and where are they", shared by the reduction
/// walk (which needs it while the VM is alive) and by interning (which needs it after). Two copies of this
/// would be two chances to disagree about an offset, and a wrong offset is a silent wrong value rather than
/// a crash — the duplication ADR-0018 §2 made one shared layout function to prevent.
///
/// A **union** is refused (ADR-0074 §4): its fields overlap, so which one the bytes represent is
/// unanswerable, and picking one silently is exactly the reinterpretation ADR-0045 §1 allows only for a
/// runtime read the programmer wrote.
fn aggregate_placements(pool: &Pool, ty: PoolId) -> Result<Vec<(PoolId, u64, u64)>, String> {
    // Comptime execution uses the *host* layout, matching the VM that produced these bytes.
    let target = jr_pool::TargetLayout::host();
    match *pool.item(ty) {
        jr_pool::Item::ArrayType { elem, len } => {
            let elem_layout = jr_pool::layout_of(pool, target, elem)
                .map_err(|e| format!("an array constant's element has no layout: {e}"))?;
            Ok((0..len)
                .map(|index| (elem, elem_layout.size * index, elem_layout.size))
                .collect())
        }
        jr_pool::Item::StructType { .. } | jr_pool::Item::VariantType { .. } => {
            let fields = pool
                .fields_of(ty)
                .ok_or_else(|| "a struct constant's fields are not recorded".to_owned())?
                .to_vec();
            let mut out = Vec::with_capacity(fields.len());
            for (index, field) in fields.iter().enumerate() {
                let (offset, layout) = jr_pool::field_offset(pool, target, ty, index as u32)
                    .map_err(|e| format!("a struct constant's field has no offset: {e}"))?;
                out.push((field.ty, offset, layout.size));
            }
            Ok(out)
        }
        // A union's fields overlap, so the bytes do not say which is live (ADR-0074 §4).
        jr_pool::Item::UnionType { .. } => {
            Err("a compile-time union value has no defined field to read".to_owned())
        }
        _ => Err("a compile-time aggregate of this shape is not supported".to_owned()),
    }
}
