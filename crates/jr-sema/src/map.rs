//! The type map: what the checker learned, as opposed to what it complained about.
//!
//! Nothing consumes this yet — `jr-mir` does not exist. It is produced anyway
//! because the alternative is a checker whose only output is diagnostics, and a
//! checker that throws its conclusions away has to be re-run from scratch by the
//! first consumer that needs them. The LSP wants exactly this map for hover.

use jr_hir::{BodyId, ExprId, ExprScope, LocalId};
use jr_pool::PoolId;
use rustc_hash::FxHashMap;

// ---------------------------------------------------------------------------
// TypeMap
// ---------------------------------------------------------------------------

/// The type of every expression and every local the checker visited.
///
/// # Why expressions are keyed on `(ExprScope, ExprId)`
///
/// An [`ExprId`] is **not** unique within a file: `FileHir::exprs` and every
/// `Body::exprs` are independent arenas that all start at index 0. A map keyed
/// on a bare `ExprId` silently collides, and the last writer wins — which is a
/// real bug that was found and fixed in `jr-hir`'s `ResolveMap`. We use the same
/// key shape here for the same reason, and reuse [`ExprScope`] rather than
/// declaring a parallel enum so the two maps cannot drift apart.
#[derive(Debug, Clone, Default)]
pub struct TypeMap {
    /// The type of each expression, keyed by its arena and index.
    exprs: FxHashMap<(ExprScope, ExprId), PoolId>,
    /// The type of each local, keyed by its body and index.
    locals: FxHashMap<(BodyId, LocalId), PoolId>,
}

impl TypeMap {
    /// Creates an empty map.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the type of an expression, if the checker reached it.
    ///
    /// Absent means "not visited", which happens for expressions under an
    /// unreachable or error-recovered statement. It does not mean "untyped".
    #[must_use]
    pub fn expr_type(&self, scope: ExprScope, id: ExprId) -> Option<PoolId> {
        self.exprs.get(&(scope, id)).copied()
    }

    /// Returns the type of a local, if the checker reached its declaration.
    #[must_use]
    pub fn local_type(&self, body: BodyId, local: LocalId) -> Option<PoolId> {
        self.locals.get(&(body, local)).copied()
    }

    /// The number of typed expressions. Used by tests to prove the map is not
    /// silently empty.
    #[must_use]
    pub fn expr_count(&self) -> usize {
        self.exprs.len()
    }

    /// The number of typed locals.
    #[must_use]
    pub fn local_count(&self) -> usize {
        self.locals.len()
    }

    /// Records an expression's type.
    pub(crate) fn set_expr(&mut self, scope: ExprScope, id: ExprId, ty: PoolId) {
        self.exprs.insert((scope, id), ty);
    }

    /// Records a local's type.
    pub(crate) fn set_local(&mut self, body: BodyId, local: LocalId, ty: PoolId) {
        self.locals.insert((body, local), ty);
    }

    /// Merges another map into this one.
    ///
    /// Sema types a file in two phases, and each returns its own map: the
    /// signature phase types file-level declarations, the check phase types
    /// bodies. A consumer that wants one map for the whole file — the LSP, or
    /// eventually `jr-mir` — folds them together with this. The two never
    /// disagree, because neither phase types an expression the other one did.
    pub fn absorb(&mut self, other: &Self) {
        self.exprs.extend(other.exprs.iter().map(|(k, v)| (*k, *v)));
        self.locals
            .extend(other.locals.iter().map(|(k, v)| (*k, *v)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expr_ids_from_different_arenas_do_not_collide() {
        // This is the arena trap, as a test: the same index in two arenas must
        // stay two entries.
        let mut map = TypeMap::new();
        let id = ExprId::from_usize(0);
        let body = BodyId::from_usize(0);
        map.set_expr(ExprScope::TopLevel, id, PoolId::S64);
        map.set_expr(ExprScope::Body(body), id, PoolId::BOOL);
        assert_eq!(map.expr_type(ExprScope::TopLevel, id), Some(PoolId::S64));
        assert_eq!(map.expr_type(ExprScope::Body(body), id), Some(PoolId::BOOL));
        assert_eq!(map.expr_count(), 2);
    }

    #[test]
    fn absent_is_distinguishable_from_error() {
        let map = TypeMap::new();
        assert_eq!(
            map.expr_type(ExprScope::TopLevel, ExprId::from_usize(7)),
            None
        );
    }
}
