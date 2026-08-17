//! The two maps lowering needs but cannot compute: constant values, and
//! cross-file callees.
//!
//! # Why these are inputs rather than something this crate works out
//!
//! `jr-mir` is a pure fold over HIR plus types, and ADR-0017 §4 keeps it that way
//! deliberately. Two facts about a Jairs-0 program are nonetheless outside what
//! HIR and types can answer, and both were standing refusals until ADR-0018:
//!
//! - **A constant has a value.** `jr-sema` records a constant's *type* but never
//!   its value, because computing one needs an evaluator and the VM is the only
//!   evaluator there will be (ADR-0016 §4). So `MESSAGE :: "hi";` followed by
//!   `print(MESSAGE)` had nothing for lowering to emit, and `#run add(2, 3)` had
//!   nothing either.
//! - **An imported name is a specific procedure.** [`crate::Callee::Direct`] names
//!   a [`ProcRef`], which is a `(FileId, ProcId)` pair, and resolving
//!   `Res::Imported` to one needs the *other* file's signatures — which ADR-0016 §5
//!   deliberately keeps out of this file's analysis.
//!
//! ADR-0018 §3 and §5 answer both the same way: `jr-db` computes them, because it
//! is the layer that already owns query wiring and evaluation order, and hands them
//! in beside the `TypeMap`. That keeps `lower_file` a function of its arguments.
//! The rejected alternative in both cases was a callback lowering could invoke on
//! demand, which would hide an ordered, cycle-prone traversal inside something
//! documented as a pure fold — and whose results salsa could not memoize.
//!
//! # Why empty is the pre-VM behaviour
//!
//! Both maps default to empty, and an empty map means "no value known", which is
//! exactly the refusal ADR-0017 shipped. That is not laziness: it keeps this
//! crate's own tests able to construct lowering inputs without standing up a VM,
//! and it means a caller that forgets to supply values gets a *refusal* rather
//! than a wrong answer.

use jr_base::FileId;
use jr_hir::{ExprId, ExprScope, ItemId, ProcId};
use jr_pool::PoolId;
use rustc_hash::FxHashMap;

use crate::mir::ProcRef;

// ---------------------------------------------------------------------------
// Operator overloads
// ---------------------------------------------------------------------------

/// Which overload each operator expression resolved to (ADR-0048 §5).
///
/// Produced by `jr-sema`, which did the resolution, and read here rather than recomputed: two
/// implementations of ADR-0048 §4's exact-match rule are two chances to disagree, and a
/// disagreement would silently call a different procedure. The same reasoning that makes `jr-mir`
/// read `TypeMap` instead of typing expressions itself.
///
/// The positional argument list of every call that used a named argument or a default (ADR-0053 §1).
///
/// Empty for a file that uses neither, which is the common case and costs nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilledArgs {
    calls: FxHashMap<(ExprScope, ExprId), Vec<FilledArg>>,
}

/// One resolved argument position, as `jr-mir` sees it (ADR-0053 §1).
///
/// The MIR-side mirror of `jr_sema::ArgSlot`. Separate rather than shared because `jr-mir` does not
/// depend on `jr-sema` — the dependency runs the other way, through `jr-db` — and because MIR only
/// needs two shapes: lower this expression, or emit this constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilledArg {
    /// Lower the expression the call site wrote.
    Expr(ExprId),
    /// Emit this already-interned default as a constant operand.
    Default(PoolId),
}

impl FilledArgs {
    /// An empty map: every call uses its source-order arguments.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the positional argument list of one call.
    pub fn set(&mut self, scope: ExprScope, expr: ExprId, args: Vec<FilledArg>) {
        self.calls.insert((scope, expr), args);
    }

    /// The positional argument list of a call, if it needed reordering or a default.
    ///
    /// `None` for an all-positional call with no defaults, which is the common case — so lowering
    /// falls back to the source order, and that order is already correct.
    #[must_use]
    pub fn get(&self, scope: ExprScope, expr: ExprId) -> Option<&[FilledArg]> {
        self.calls.get(&(scope, expr)).map(Vec::as_slice)
    }
}

/// Which overload each operator expression resolved to (ADR-0048 §5).
///
/// Empty for every file that uses no overload, which is the common case and costs nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OperatorCalls {
    calls: FxHashMap<(ExprScope, ExprId), ProcRef>,
}

impl OperatorCalls {
    /// An empty map: no operator expression resolves to an overload.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records that an operator expression resolved to `target`.
    ///
    /// Keyed by [`ExprScope`] as well as [`ExprId`] for the reason [`ConstValues::set_run`] is:
    /// a bare `ExprId` is not unique within a file and has already caused one real collision bug.
    pub fn set(&mut self, scope: ExprScope, expr: ExprId, target: ProcRef) {
        self.calls.insert((scope, expr), target);
    }

    /// The overload an operator expression resolved to, if it did.
    #[must_use]
    pub fn get(&self, scope: ExprScope, expr: ExprId) -> Option<ProcRef> {
        self.calls.get(&(scope, expr)).copied()
    }

    /// Whether anything resolved to an overload at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Constant values
// ---------------------------------------------------------------------------

/// The compile-time value of every constant and `#run` in one file.
///
/// Values are [`PoolId`]s, not a separate representation: the pool already interns
/// integers, booleans and strings with their types as part of the key (ADR-0015),
/// so a folded `#run` result is indistinguishable from a literal — which is the
/// property `020-run-directive.jr`'s comment claims and this is what makes true.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConstValues {
    items: FxHashMap<ItemId, PoolId>,
    runs: FxHashMap<(ExprScope, ExprId), PoolId>,
    any_ops: FxHashMap<(ExprScope, ExprId), AnyLowering>,
    /// The pointer type each `typed`/`untyped` call produces (ADR-0106 §1).
    ///
    /// Beside `any_ops` rather than in `runs`, because this is **real code**: a pointer's bits do not depend on
    /// its pointee, so retyping is a store-then-load through a slot (the mechanism ADR-0076 §1 built), and the
    /// builder needs the target type to make the slot. A folded value would be wrong — the *address* is only
    /// known at run time.
    pointer_views: FxHashMap<(ExprScope, ExprId), PoolId>,
    /// The procedure a polymorphic call was instantiated to (ADR-0082, DECISIONS fork 4).
    ///
    /// A call to a `$T` procedure is redirected here to the *instantiated* `ProcRef` appended to the
    /// expanded HIR, rather than the template. `call_rvalue` consults this before `direct_callee`, so the
    /// call node itself is never rewritten — the same channel `#run` and `any_op` ride.
    instantiations: FxHashMap<(ExprScope, ExprId), ProcRef>,
    /// Which argument positions to **drop** at a comptime-value call site (ADR-0088 §3).
    ///
    /// A `make(v, x)` where `make :: ($N: s64, x: s64)` redirects to an instantiation whose parameter list
    /// is `(x: s64)` — the `$N` parameter is baked into the body, not received. So the call must pass only
    /// the non-comptime arguments. This map holds, per redirected call, a boolean per source-order
    /// argument: `true` means "drop, this is a comptime argument baked into the callee". Absent for a `$T`
    /// call (which keeps every argument).
    comptime_arg_mask: FxHashMap<(ExprScope, ExprId), Vec<bool>>,
    /// Per variadic call (ADR-0138 §2), the number of *fixed* arguments and the element type
    /// of the trailing view. MIR packs `args[fixed..]` into a stack array of `element_ty` and
    /// passes a view over it as the last parameter.
    variadic_calls: FxHashMap<(ExprScope, ExprId), (usize, PoolId)>,
}

/// How the MIR builder should lower one `any_of`/`any_as` call (ADR-0076).
///
/// Carried on [`ConstValues`] because `file_consts` is where the `Type_Info` constant is built and the
/// type ids are known — the same query that folds a `#run`, reused rather than a new channel threaded
/// through five `lower_file` call sites. Unlike a `#run`, an `Any` call lowers to *real code*, so this
/// says *how* rather than *to what value*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnyLowering {
    /// `any_of(p)` — build `{type, data}` where `type` is the address of this spilled `Type_Info`
    /// constant and `data` is the pointer argument erased to `*u8` (ADR-0076 §1).
    Of {
        /// The `Type_Info` constant describing the pointee, to spill into a slot for the `type` field.
        type_info: PoolId,
        /// The `Any` struct type to build, needed because the *implicit coercion* form records this
        /// against a pointer-typed argument expression whose own type is not `Any` — so the builder
        /// cannot recover it from the expression the way the explicit call can from its result.
        any_ty: PoolId,
    },
    /// `any_as(a, T)` — trap unless `a.type.id` equals `type_id`, then read `a.data` as `*result`
    /// (ADR-0076 §2, ADR-0077).
    As {
        /// The expected type's pool id, widened — what `a.type.id` must equal.
        type_id: u64,
        /// The result type `T`, so the builder can build `*T` for the deref.
        result: PoolId,
    },
}

impl ConstValues {
    /// An empty map: nothing has a value, so everything that needs one is refused.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the value of a file-level item.
    pub fn set_item(&mut self, item: ItemId, value: PoolId) {
        self.items.insert(item, value);
    }

    /// Records the value of one `#run` expression.
    ///
    /// Keyed by [`ExprScope`] as well as [`ExprId`] for the reason
    /// [`crate::MirSpan::Expr`] carries one: `FileHir::exprs` and every
    /// `Body::exprs` start at index 0, and a bare `ExprId` has already caused one
    /// real collision bug in `jr-hir`'s `ResolveMap`.
    pub fn set_run(&mut self, scope: ExprScope, expr: ExprId, value: PoolId) {
        self.runs.insert((scope, expr), value);
    }

    /// Forgets the value of one `run`-keyed expression (ADR-0101 §3).
    ///
    /// Needed because a folded value is keyed by `ExprId`, and a computed `#insert` renumbers every id after
    /// its splice — so a value recorded against the *unexpanded* tree names a different expression in the
    /// expanded one. `file_mir` clears the unexpanded entries before re-recording the expanded check's, and
    /// clearing is the load-bearing half: a stale entry left at a live id is a genuine value in the wrong
    /// place, which no verifier can recognise as wrong.
    pub fn clear_run(&mut self, scope: ExprScope, expr: ExprId) {
        self.runs.remove(&(scope, expr));
    }

    /// Copies every entry keyed on `from` to the same [`ExprId`] under `to` (ADR-0120 §5).
    ///
    /// An instantiation's body is cloned from its template's **arena and all** — `hir.body(b).clone()` — so
    /// the clone's expression at index *i* is the template's expression at index *i*, and only the
    /// [`ExprScope`] differs. That is what makes this a scope substitution rather than a remap: without it, a
    /// `#run`, a `typed`/`untyped` or an `any_of` *inside a template body* had a value under the template's
    /// scope and none under the clone's, so `scan` refused the clone — E0245, a warning — and the call then
    /// reported `no routine for file N proc M` when it was reached.
    ///
    /// Deliberately does **not** copy the instantiation redirects or the comptime argument masks: a clone's
    /// own polymorphic calls are redirected from the *final* check by `instantiated_from`, which knows the
    /// appended targets, and copying the template's would point a clone's call at whatever the template's
    /// call resolved to.
    ///
    /// Existing entries under `to` are left alone, so a per-instantiation fold — `type_info(T)`, which is the
    /// one value that genuinely differs per binding — overrides rather than being overridden.
    pub fn copy_body_scope(&mut self, from: ExprScope, to: ExprScope) {
        let runs: Vec<(ExprId, PoolId)> = self
            .runs
            .iter()
            .filter(|((scope, _), _)| *scope == from)
            .map(|((_, expr), value)| (*expr, *value))
            .collect();
        for (expr, value) in runs {
            self.runs.entry((to, expr)).or_insert(value);
        }

        let any_ops: Vec<(ExprId, AnyLowering)> = self
            .any_ops
            .iter()
            .filter(|((scope, _), _)| *scope == from)
            .map(|((_, expr), op)| (*expr, *op))
            .collect();
        for (expr, op) in any_ops {
            self.any_ops.entry((to, expr)).or_insert(op);
        }

        let views: Vec<(ExprId, PoolId)> = self
            .pointer_views
            .iter()
            .filter(|((scope, _), _)| *scope == from)
            .map(|((_, expr), ty)| (*expr, *ty))
            .collect();
        for (expr, ty) in views {
            self.pointer_views.entry((to, expr)).or_insert(ty);
        }
    }

    /// The value of a file-level item, if one is known.
    #[must_use]
    pub fn item(&self, item: ItemId) -> Option<PoolId> {
        self.items.get(&item).copied()
    }

    /// The value of a `#run` expression, if one is known.
    #[must_use]
    pub fn run(&self, scope: ExprScope, expr: ExprId) -> Option<PoolId> {
        self.runs.get(&(scope, expr)).copied()
    }

    /// Records how one `any_of`/`any_as` call should lower (ADR-0076).
    pub fn set_any_op(&mut self, scope: ExprScope, expr: ExprId, op: AnyLowering) {
        self.any_ops.insert((scope, expr), op);
    }

    /// How an `any_of`/`any_as` call should lower, if it is one.
    #[must_use]
    pub fn any_op(&self, scope: ExprScope, expr: ExprId) -> Option<AnyLowering> {
        self.any_ops.get(&(scope, expr)).copied()
    }

    /// Records that a `typed`/`untyped` call produces a pointer of type `ty` (ADR-0106 §1).
    pub fn set_pointer_view(&mut self, scope: ExprScope, expr: ExprId, ty: PoolId) {
        self.pointer_views.insert((scope, expr), ty);
    }

    /// The pointer type a `typed`/`untyped` call produces, if it is one.
    #[must_use]
    pub fn pointer_view(&self, scope: ExprScope, expr: ExprId) -> Option<PoolId> {
        self.pointer_views.get(&(scope, expr)).copied()
    }

    /// Records that a polymorphic call was instantiated to `target` (ADR-0082).
    pub fn set_instantiation(&mut self, scope: ExprScope, expr: ExprId, target: ProcRef) {
        self.instantiations.insert((scope, expr), target);
    }

    /// The instantiated procedure a polymorphic call was redirected to, if it was one.
    #[must_use]
    pub fn instantiation(&self, scope: ExprScope, expr: ExprId) -> Option<ProcRef> {
        self.instantiations.get(&(scope, expr)).copied()
    }

    /// Records that a comptime-value call drops these argument positions (ADR-0088 §3).
    pub fn set_comptime_arg_mask(&mut self, scope: ExprScope, expr: ExprId, mask: Vec<bool>) {
        self.comptime_arg_mask.insert((scope, expr), mask);
    }

    /// Which argument positions a comptime-value call drops, if it is one.
    ///
    /// Returns `None` for a `$T` call (no drops) or any other call. `true` at index `i` means the i-th
    /// source-order argument is baked into the callee and must not be passed as a runtime operand.
    #[must_use]
    pub fn comptime_arg_mask(&self, scope: ExprScope, expr: ExprId) -> Option<&[bool]> {
        self.comptime_arg_mask
            .get(&(scope, expr))
            .map(Vec::as_slice)
    }

    /// Records a variadic call's fixed-arg count and element type (ADR-0138 §2).
    pub fn set_variadic_call(
        &mut self,
        scope: ExprScope,
        expr: ExprId,
        fixed: usize,
        element_ty: PoolId,
    ) {
        self.variadic_calls
            .insert((scope, expr), (fixed, element_ty));
    }

    /// The variadic-call info for a call, if it is one.
    #[must_use]
    pub fn variadic_call(&self, scope: ExprScope, expr: ExprId) -> Option<(usize, PoolId)> {
        self.variadic_calls.get(&(scope, expr)).copied()
    }

    /// The number of recorded values.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len() + self.runs.len()
    }

    /// Whether nothing has a value.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
            && self.runs.is_empty()
            && self.any_ops.is_empty()
            && self.instantiations.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Cross-file callees
// ---------------------------------------------------------------------------

/// Which procedure each imported name refers to (ADR-0018 §5).
///
/// Keyed on the `#import` item and the name, matching `Res::Imported(ItemId,
/// Symbol)`'s own shape, rather than on the referring expression: two references
/// to the same imported procedure resolve identically, and keying on the
/// resolution means lowering looks up what it already has in hand.
/// The value of each imported *constant* a file reads (ADR-0055 §1).
///
/// Filled by `jr-db` from the other module's `file_consts`, and read by lowering where it used to
/// refuse with "an imported name has no value until jr-vm". A `PoolId` needs no translation across
/// files because the pool is shared (ADR-0018 §2), which is what makes a *value* the cheap thing to
/// carry across a module boundary and a field list the expensive one.
///
/// Empty for a file that reads no imported constant, which costs nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportedValues {
    by_name: FxHashMap<(ItemId, jr_base::Symbol), PoolId>,
}

impl ImportedValues {
    /// An empty map: every imported constant is refused, as it was before ADR-0055.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the value an imported name has.
    ///
    /// Keyed on the *importing* file's `#import` item plus the name in the other scope — the same key
    /// [`ImportedProcs`] uses, because `Res::Imported` yields exactly that pair and a second key
    /// shape would be a second thing to keep in step (ADR-0055 §1).
    pub fn set(&mut self, import: ItemId, name: jr_base::Symbol, value: PoolId) {
        self.by_name.insert((import, name), value);
    }

    /// The value of an imported name, if the other file's const-eval produced one.
    ///
    /// `None` for a constant const-eval could not fold — which is E0230 in its *own* file already, so
    /// the importing file refuses as it did before rather than inventing a second diagnostic.
    #[must_use]
    pub fn get(&self, import: ItemId, name: jr_base::Symbol) -> Option<PoolId> {
        self.by_name.get(&(import, name)).copied()
    }
}

/// The `ProcRef` each cross-file callee resolves to (ADR-0018 §5).
///
/// Filled by `jr-db` from the other module's signatures. Keying on the resolution rather than on the
/// callee expression means two calls to the same imported procedure resolve identically, and lowering
/// looks up what it already has in hand.
///
/// Empty for a file that calls nothing across a boundary, in which case every cross-file call is
/// refused exactly as ADR-0017 shipped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportedProcs {
    by_name: FxHashMap<(ItemId, jr_base::Symbol), ImportedProc>,
}

/// A resolved cross-file callee: where it is, and whether it takes the implicit context.
///
/// The context flag rides along because it cannot be recomputed on the *importing* side — the
/// callee's `#c_call`/`#foreign` status is in its *own* file's HIR, which `jr-db` reads at fill time
/// and lowering does not have (ADR-0057 §3). Recording it here is the same "resolve across files in
/// `jr-db`, hand `jr-mir` the answer" shape ADR-0018 §5 established, extended by one bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportedProc {
    /// The procedure.
    pub target: ProcRef,
    /// Whether it receives the implicit context (ADR-0057 §3).
    pub receives_context: bool,
}

impl ImportedProcs {
    /// An empty map: every cross-file call is refused, as ADR-0017 had it.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records that an imported name is a procedure in another file.
    pub fn set(&mut self, import: ItemId, name: jr_base::Symbol, target: ProcRef) {
        self.set_full(import, name, target, true);
    }

    /// Records a procedure with its context flag (ADR-0057 §3).
    pub fn set_full(
        &mut self,
        import: ItemId,
        name: jr_base::Symbol,
        target: ProcRef,
        receives_context: bool,
    ) {
        self.by_name.insert(
            (import, name),
            ImportedProc {
                target,
                receives_context,
            },
        );
    }

    /// Records a procedure by file and index.
    ///
    /// A convenience over [`Self::set`] for callers that have the two halves
    /// separately, which `jr-db` does.
    pub fn set_parts(&mut self, import: ItemId, name: jr_base::Symbol, file: FileId, proc: ProcId) {
        self.set(import, name, ProcRef::new(file, proc));
    }

    /// The procedure an imported name refers to, if it refers to one.
    ///
    /// `None` covers both "not resolved" and "resolved to something that is not a
    /// procedure"; lowering refuses either way, so the distinction would have no
    /// consumer.
    #[must_use]
    pub fn get(&self, import: ItemId, name: jr_base::Symbol) -> Option<ProcRef> {
        self.by_name.get(&(import, name)).map(|p| p.target)
    }

    /// The full resolved callee, including whether it takes a context (ADR-0057 §3).
    #[must_use]
    pub fn resolved(&self, import: ItemId, name: jr_base::Symbol) -> Option<ImportedProc> {
        self.by_name.get(&(import, name)).copied()
    }

    /// Whether the imported procedure at `target` receives the implicit context (ADR-0057 §3).
    ///
    /// Keyed by [`ProcRef`] rather than by `(import, name)`, because a caller that already resolved the
    /// callee has the reference and not the name it was imported under — which is the position
    /// `thunk.rs` is in when it decides whether to pass a context (ADR-0069 §1).
    ///
    /// `false` for a procedure this map does not hold, which is the safe direction: a call that passes
    /// no context to a callee expecting one is an argument-count mismatch the interpreter reports,
    /// whereas passing one to a `#c_call` callee would corrupt its first argument.
    #[must_use]
    pub fn receives_context(&self, target: ProcRef) -> bool {
        self.by_name
            .values()
            .any(|p| p.target == target && p.receives_context)
    }

    /// The number of resolved imported procedures.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    /// Whether nothing is resolved.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jr_base::Interner;
    use jr_hir::BodyId;

    #[test]
    fn an_empty_const_map_knows_nothing() {
        let consts = ConstValues::new();
        assert!(consts.is_empty());
        assert_eq!(consts.item(ItemId::from_usize(0)), None);
        assert_eq!(
            consts.run(ExprScope::TopLevel, ExprId::from_usize(0)),
            None,
            "an empty map must refuse rather than invent a value"
        );
    }

    #[test]
    fn a_run_value_is_keyed_by_scope_as_well_as_index() {
        // The arena trap: FileHir::exprs and every Body::exprs both start at 0.
        let mut consts = ConstValues::new();
        let expr = ExprId::from_usize(0);
        consts.set_run(ExprScope::TopLevel, expr, PoolId::TRUE);
        assert_eq!(consts.run(ExprScope::TopLevel, expr), Some(PoolId::TRUE));
        assert_eq!(
            consts.run(ExprScope::Body(BodyId::from_usize(0)), expr),
            None,
            "the same index in a different arena is a different expression"
        );
    }

    #[test]
    fn items_and_runs_do_not_share_a_namespace() {
        let mut consts = ConstValues::new();
        consts.set_item(ItemId::from_usize(3), PoolId::TRUE);
        consts.set_run(ExprScope::TopLevel, ExprId::from_usize(3), PoolId::FALSE);
        assert_eq!(consts.item(ItemId::from_usize(3)), Some(PoolId::TRUE));
        assert_eq!(
            consts.run(ExprScope::TopLevel, ExprId::from_usize(3)),
            Some(PoolId::FALSE)
        );
        assert_eq!(consts.len(), 2);
    }

    #[test]
    fn an_imported_proc_is_keyed_by_import_and_name() {
        let interner = Interner::new();
        let mut imports = ImportedProcs::new();
        let import = ItemId::from_usize(0);
        let print = interner.intern("print");
        let other = interner.intern("print_line");
        let target = ProcRef::new(FileId::from_usize(7), ProcId::from_usize(2));
        imports.set(import, print, target);

        assert_eq!(imports.get(import, print), Some(target));
        assert_eq!(imports.get(import, other), None);
        assert_eq!(
            imports.get(ItemId::from_usize(1), print),
            None,
            "the same name through a different #import is a different resolution"
        );
        assert_eq!(imports.len(), 1);
        assert!(!imports.is_empty());
    }

    #[test]
    fn set_parts_agrees_with_set() {
        let interner = Interner::new();
        let name = interner.intern("write");
        let import = ItemId::from_usize(0);
        let file = FileId::from_usize(1);
        let proc = ProcId::from_usize(4);

        let mut a = ImportedProcs::new();
        a.set(import, name, ProcRef::new(file, proc));
        let mut b = ImportedProcs::new();
        b.set_parts(import, name, file, proc);
        assert_eq!(a, b);
    }
}
