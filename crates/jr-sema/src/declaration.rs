//! Evaluated integer values consumed while declarations are shaped.
//!
//! The evaluator does not live in this crate. `jr-db` lowers each retained expression through MIR,
//! executes it in the compile-time VM, and passes only the resulting integers back through this map
//! (ADR-0245). Keeping the interface this small preserves the crate graph: sema depends on neither
//! MIR nor the VM, and no second evaluator appears here.

use jr_base::Span;
use jr_hir::{ExprId, ExprScope};
use rustc_hash::FxHashMap;

/// Integer values available while resolving declaration shapes.
#[derive(Debug, Clone, Default)]
pub struct DeclarationValues {
    ints: FxHashMap<(ExprScope, ExprId), i128>,
    spans: FxHashMap<Span, i128>,
}

impl DeclarationValues {
    /// Records one evaluated expression.
    pub fn insert_int(&mut self, scope: ExprScope, expr: ExprId, value: i128) {
        self.ints.insert((scope, expr), value);
    }

    /// Records one evaluated expression under both its arena identity and source span.
    pub fn insert_int_at(&mut self, scope: ExprScope, expr: ExprId, span: Span, value: i128) {
        self.insert_int(scope, expr, value);
        self.spans.insert(span, value);
    }

    /// Returns the evaluated integer for one expression.
    #[must_use]
    pub fn int(&self, scope: ExprScope, expr: ExprId) -> Option<i128> {
        self.ints.get(&(scope, expr)).copied()
    }

    /// Returns an evaluated integer, preferring source identity across HIR expansion.
    ///
    /// Expansion may allocate a different expression at an arena index the source HIR used for
    /// this value. The span is therefore the stronger identity when the caller has one; the arena
    /// key remains the fallback for synthetic expressions without stable source identity.
    #[must_use]
    pub fn int_at(&self, scope: ExprScope, expr: ExprId, span: Span) -> Option<i128> {
        self.spans
            .get(&span)
            .copied()
            .or_else(|| self.int(scope, expr))
    }
}
