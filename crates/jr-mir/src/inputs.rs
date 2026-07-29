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

    /// The number of recorded values.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len() + self.runs.len()
    }

    /// Whether nothing has a value.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty() && self.runs.is_empty()
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportedProcs {
    by_name: FxHashMap<(ItemId, jr_base::Symbol), ProcRef>,
}

impl ImportedProcs {
    /// An empty map: every cross-file call is refused, as ADR-0017 had it.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records that an imported name is a procedure in another file.
    pub fn set(&mut self, import: ItemId, name: jr_base::Symbol, target: ProcRef) {
        self.by_name.insert((import, name), target);
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
        self.by_name.get(&(import, name)).copied()
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
