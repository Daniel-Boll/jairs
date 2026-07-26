//! SSA construction, during lowering, by the algorithm of Braun et al.
//!
//! # Why here and not in a `mem2reg` pass
//!
//! ADR-0017 §2 decides this and names the rejected alternative: lower every local
//! to memory and recover SSA afterwards, which is rustc's shape and Swift's. The
//! short version of the argument is that a `mem2reg` worth having needs a
//! dominator tree, dominance frontiers, phi insertion, renaming *and* SROA —
//! Swift ships `AllocBoxToStack`, `DefiniteInitialization`, `EarlySROA`, `SROA`,
//! `SROABBArgs` and two redundant-load passes — whereas this algorithm needs none
//! of that and runs inside a walk lowering is already doing.
//!
//! The reason it needs no dominance analysis is worth stating because it is a
//! property of *this* language rather than of the algorithm: Braun's minimality
//! result holds for reducible CFGs, and the HIR's entire control flow is `if`,
//! `while`, `return`, `break` and `continue` — no `for`, no `defer`, no labelled
//! break, no `goto`, all of which the parser rejects outright. Every CFG is
//! therefore reducible by *construction*, not by luck, so the SCC-based
//! redundant-phi path the paper needs for irreducible graphs is unreachable here
//! and is not implemented.
//!
//! # Filled, sealed, and why both
//!
//! Two per-block bits, exactly as `cranelift-frontend`'s `ssa.rs` has them. A
//! block is **filled** once all its statements are emitted, and **sealed** once
//! all its predecessors are known. They are different because a loop header is
//! reachable from its own body: lowering must emit the header's condition before
//! it has seen the back edge, so the header is filled long before it can be
//! sealed. Reading a variable in an unsealed block cannot look at predecessors —
//! there may be more coming — so it optimistically creates an *incomplete*
//! parameter and fixes up the operands at [`SsaBuilder::seal_block`].
//!
//! # A variable's value is an `Operand`, not a `ValueId`
//!
//! `read_variable` returns an [`Operand`], so `count := 10` binds the variable
//! directly to the interned constant rather than emitting a copy into a fresh
//! value. It also makes trivial-parameter removal able to collapse a parameter
//! down to a constant, which is the common case for a variable that is only ever
//! assigned one literal.
//!
//! # The one hazard: an operand held across a seal
//!
//! [`SsaBuilder::read_variable`] can return a parameter that a later
//! [`SsaBuilder::seal_block`] collapses. When that happens, every occurrence
//! *already emitted into the body* is rewritten, but an operand a caller is still
//! holding in a Rust local is not — nothing can reach it. Lowering must therefore
//! read a variable at the point it uses it, and never carry the result across a
//! `seal_block`. This is not merely a convention: emitting a stale operand leaves
//! a use of a value nothing defines, and `verify.rs` reports exactly that, so the
//! mistake is a test failure rather than silent corruption.
//!
//! # What is not here
//!
//! No dominator tree, so nothing in this module can answer "does this definition
//! dominate that use". `verify.rs` says the same thing about its own checks. That
//! question belongs to the mid-end, which does not exist yet.

use jr_hir::LocalId;
use jr_pool::PoolId;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::mir::{
    BlockId, MirBody, MirSpan, Operand, Rvalue, Statement, Terminator, UndefinedRead, ValueId,
};

// ---------------------------------------------------------------------------
// SsaBuilder
// ---------------------------------------------------------------------------

/// Which block, and which variable, a block parameter was created for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PhiOrigin {
    /// The block the parameter belongs to.
    block: BlockId,
    /// The local it carries.
    local: LocalId,
    /// The local's type, needed when the parameter's operands are filled in.
    ty: PoolId,
}

/// One edge into a block, as the position of a [`crate::mir::Target`] inside some
/// predecessor's terminator.
///
/// A block is identified by an edge rather than by a predecessor because a single
/// predecessor can branch to the same block twice — `if c { } else { }` with two
/// empty arms does exactly that — and each such edge needs its own argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Edge {
    /// The block the edge leaves.
    from: BlockId,
    /// Which of that terminator's targets it is.
    target: usize,
}

/// Braun's SSA construction state.
#[derive(Debug, Default)]
pub(crate) struct SsaBuilder {
    /// The current value of each variable, per block.
    current: FxHashMap<(BlockId, LocalId), Operand>,
    /// Blocks all of whose predecessors are known.
    sealed: FxHashSet<BlockId>,
    /// Parameters created before their block was sealed, awaiting operands.
    incomplete: FxHashMap<BlockId, Vec<(LocalId, ValueId)>>,
    /// Every parameter this builder created.
    phis: FxHashMap<ValueId, PhiOrigin>,
    /// Reads that reached the entry without finding a definition.
    undefined: Vec<UndefinedRead>,
}

impl SsaBuilder {
    /// Creates an empty builder.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Marks a block as having all its predecessors known.
    ///
    /// Must be called once per block, after every edge into it exists. Any
    /// parameter optimistically created while the block was unsealed gets its
    /// operands here, and is collapsed if it turns out to be trivial.
    pub(crate) fn seal_block(&mut self, mir: &mut MirBody, block: BlockId) {
        if !self.sealed.insert(block) {
            return;
        }
        let pending = self.incomplete.remove(&block).unwrap_or_default();
        for (local, param) in pending {
            let ty = self
                .phis
                .get(&param)
                .map_or(PoolId::ERROR, |origin| origin.ty);
            self.add_phi_operands(mir, block, local, param, ty);
            self.try_remove_trivial_phi(mir, param);
        }
    }

    /// Records that `local` now holds `value` in `block`.
    pub(crate) fn write_variable(&mut self, block: BlockId, local: LocalId, value: Operand) {
        self.current.insert((block, local), value);
    }

    /// The value of `local` on entry to the current point of `block`.
    ///
    /// `ty` is the local's type, used for any parameter this has to create, and
    /// `span` is attributed to a synthesised definition.
    pub(crate) fn read_variable(
        &mut self,
        mir: &mut MirBody,
        block: BlockId,
        local: LocalId,
        ty: PoolId,
        span: MirSpan,
    ) -> Operand {
        if let Some(value) = self.current.get(&(block, local)) {
            return *value;
        }
        self.read_variable_recursive(mir, block, local, ty, span)
    }

    /// Reads that found no definition on some path. Consumed by the
    /// definite-assignment diagnostic, which this crate does not raise.
    pub(crate) fn into_undefined_reads(self) -> Vec<UndefinedRead> {
        self.undefined
    }

    // -------------------------------------------------------------------
    // The recursive core
    // -------------------------------------------------------------------

    fn read_variable_recursive(
        &mut self,
        mir: &mut MirBody,
        block: BlockId,
        local: LocalId,
        ty: PoolId,
        span: MirSpan,
    ) -> Operand {
        if !self.sealed.contains(&block) {
            // The predecessors are not all known, so guess that a parameter is
            // needed. `seal_block` fills its operands and removes it if the guess
            // was wrong. This is the only reason the algorithm can run in one pass
            // over a loop.
            let param = self.new_phi(mir, block, local, ty, span);
            self.incomplete
                .entry(block)
                .or_default()
                .push((local, param));
            let value = Operand::Value(param);
            self.write_variable(block, local, value);
            return value;
        }

        let predecessors = mir.predecessors()[block.index()].to_vec();
        let value = match predecessors.len() {
            0 => {
                // No path defines it. `c: s64 = ---;` then reading `c` lands here.
                self.undefined.push(UndefinedRead { local, span });
                self.undef(mir, ty, span)
            }
            1 => {
                // No parameter needed: whatever the single predecessor has.
                // Deliberately *not* memoised before the recursion, because there
                // is no cycle to break through a single predecessor of a sealed
                // block.
                self.read_variable(mir, predecessors[0], local, ty, span)
            }
            _ => {
                let param = self.new_phi(mir, block, local, ty, span);
                let value = Operand::Value(param);
                // Break potential cycles *before* recursing into predecessors: a
                // loop header's parameter is read again while computing its own
                // operands, and without this the recursion would not terminate.
                self.write_variable(block, local, value);
                self.add_phi_operands(mir, block, local, param, ty);
                return self.try_remove_trivial_phi(mir, param);
            }
        };
        self.write_variable(block, local, value);
        value
    }

    fn new_phi(
        &mut self,
        mir: &mut MirBody,
        block: BlockId,
        local: LocalId,
        ty: PoolId,
        span: MirSpan,
    ) -> ValueId {
        let param = mir.push_block_param(block, ty, span);
        self.phis.insert(param, PhiOrigin { block, local, ty });
        param
    }

    /// A definition for a value nothing assigned.
    ///
    /// Emitted into the entry block, which dominates every reachable block, so the
    /// definition dominates every use without needing to know where the use is.
    /// Statements always precede a block's terminator, so appending is safe even
    /// after the entry block has been finished.
    fn undef(&mut self, mir: &mut MirBody, ty: PoolId, span: MirSpan) -> Operand {
        let value = mir.push_value(ty, span);
        let entry = mir.entry();
        mir.stmts_mut(entry).push(Statement::Assign {
            dest: value,
            rvalue: Rvalue::Undef,
            span,
        });
        Operand::Value(value)
    }

    /// Supplies one argument per incoming edge for a block parameter.
    fn add_phi_operands(
        &mut self,
        mir: &mut MirBody,
        block: BlockId,
        local: LocalId,
        param: ValueId,
        ty: PoolId,
    ) {
        let span = mir.value(param).span;
        let edges = incoming_edges(mir, block);
        // Every read happens before any mutation, because a read can itself create
        // parameters in a predecessor and append arguments to *its* incoming edges.
        // Target positions are stable under argument appends, so the indices
        // collected above stay correct.
        let mut values = Vec::with_capacity(edges.len());
        for edge in &edges {
            values.push(self.read_variable(mir, edge.from, local, ty, span));
        }
        for (edge, value) in edges.iter().zip(values) {
            push_argument(mir, *edge, value);
        }
    }

    // -------------------------------------------------------------------
    // Trivial parameters
    // -------------------------------------------------------------------

    /// Collapses a block parameter whose every operand is the same.
    ///
    /// Returns what callers should use in its place — which may be a constant, and
    /// commonly is for a variable only ever assigned one literal. Without this the
    /// SSA is still *correct*, but every `if` that leaves a variable alone in one
    /// arm leaves a redundant parameter behind, and every corpus snapshot carries
    /// the clutter.
    fn try_remove_trivial_phi(&mut self, mir: &mut MirBody, param: ValueId) -> Operand {
        let Some(origin) = self.phis.get(&param).copied() else {
            return Operand::Value(param);
        };
        let Some(index) = parameter_index(mir, origin.block, param) else {
            return Operand::Value(param);
        };

        let edges = incoming_edges(mir, origin.block);
        let mut same: Option<Operand> = None;
        for edge in &edges {
            let Some(operand) = argument(mir, *edge, index) else {
                // An operand has not been supplied yet, so triviality cannot be
                // decided. Leaving the parameter in place is always sound.
                return Operand::Value(param);
            };
            // A parameter that refers to itself around a loop says nothing about
            // whether it is needed, so self-references are ignored.
            if operand == Operand::Value(param) || Some(operand) == same {
                continue;
            }
            if same.is_some() {
                return Operand::Value(param);
            }
            same = Some(operand);
        }

        let Some(replacement) = same else {
            // Every operand was a self-reference: the parameter is unreachable.
            return Operand::Value(param);
        };

        remove_parameter(mir, origin.block, index);
        self.phis.remove(&param);
        self.replace_uses(mir, param, replacement);

        // Removing one parameter can make another trivial — the classic case is
        // two nested loops whose headers each carry the other's parameter.
        let dependents: Vec<ValueId> = self.phis.keys().copied().collect();
        for other in dependents {
            if self.phis.contains_key(&other) {
                self.try_remove_trivial_phi(mir, other);
            }
        }

        replacement
    }

    /// Rewrites every use of `param` to `replacement`, including inside this
    /// builder's own memo of current variable values.
    fn replace_uses(&mut self, mir: &mut MirBody, param: ValueId, replacement: Operand) {
        let old = Operand::Value(param);
        for value in self.current.values_mut() {
            if *value == old {
                *value = replacement;
            }
        }
        for block in mir.blocks_mut() {
            for stmt in &mut block.stmts {
                match stmt {
                    Statement::Assign {
                        dest: _,
                        rvalue,
                        span: _,
                    }
                    | Statement::Discard { rvalue, span: _ } => {
                        replace_in_rvalue(rvalue, old, replacement);
                    }
                    Statement::Store {
                        place,
                        value,
                        span: _,
                    } => {
                        replace_in_place(place, old, replacement);
                        replace_operand(value, old, replacement);
                    }
                    Statement::Nop => {}
                }
            }
            match &mut block.term {
                Terminator::Goto(target) => {
                    for arg in &mut target.args {
                        replace_operand(arg, old, replacement);
                    }
                }
                Terminator::Branch { cond, then_, else_ } => {
                    replace_operand(cond, old, replacement);
                    for arg in then_.args.iter_mut().chain(else_.args.iter_mut()) {
                        replace_operand(arg, old, replacement);
                    }
                }
                Terminator::Return(operand) => {
                    if let Some(operand) = operand {
                        replace_operand(operand, old, replacement);
                    }
                }
                Terminator::Unreachable(_) => {}
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Edge and parameter surgery
// ---------------------------------------------------------------------------

/// Every edge that enters `block`, one entry per edge rather than per predecessor.
fn incoming_edges(mir: &MirBody, block: BlockId) -> Vec<Edge> {
    let mut edges = Vec::new();
    for (index, data) in mir.blocks().iter().enumerate() {
        for (position, target) in data.term.targets().iter().enumerate() {
            if target.block == block {
                edges.push(Edge {
                    from: BlockId::from_usize(index),
                    target: position,
                });
            }
        }
    }
    edges
}

/// The position of `param` in its block's parameter list.
fn parameter_index(mir: &MirBody, block: BlockId, param: ValueId) -> Option<usize> {
    mir.block(block)
        .params
        .iter()
        .position(|value| *value == param)
}

/// The argument an edge supplies at `index`, if it has been supplied yet.
fn argument(mir: &MirBody, edge: Edge, index: usize) -> Option<Operand> {
    let targets = mir.block(edge.from).term.targets();
    targets
        .get(edge.target)
        .and_then(|target| target.args.get(index).copied())
}

/// Appends one argument to an edge.
fn push_argument(mir: &mut MirBody, edge: Edge, value: Operand) {
    let block = &mut mir.blocks_mut()[edge.from.index()];
    let target = match (&mut block.term, edge.target) {
        (Terminator::Goto(target), 0) => target,
        (
            Terminator::Branch {
                cond: _,
                then_,
                else_: _,
            },
            0,
        ) => then_,
        (
            Terminator::Branch {
                cond: _,
                then_: _,
                else_,
            },
            1,
        ) => else_,
        (Terminator::Goto(_) | Terminator::Branch { .. }, _)
        | (Terminator::Return(_) | Terminator::Unreachable(_), _) => return,
    };
    target.args.push(value);
}

/// Drops a block's parameter, and the matching argument on every incoming edge.
fn remove_parameter(mir: &mut MirBody, block: BlockId, index: usize) {
    let edges = incoming_edges(mir, block);
    for edge in edges {
        let data = &mut mir.blocks_mut()[edge.from.index()];
        let target = match (&mut data.term, edge.target) {
            (Terminator::Goto(target), 0) => target,
            (
                Terminator::Branch {
                    cond: _,
                    then_,
                    else_: _,
                },
                0,
            ) => then_,
            (
                Terminator::Branch {
                    cond: _,
                    then_: _,
                    else_,
                },
                1,
            ) => else_,
            (Terminator::Goto(_) | Terminator::Branch { .. }, _)
            | (Terminator::Return(_) | Terminator::Unreachable(_), _) => continue,
        };
        if index < target.args.len() {
            target.args.remove(index);
        }
    }
    let params = &mut mir.blocks_mut()[block.index()].params;
    if index < params.len() {
        params.remove(index);
    }
}

// ---------------------------------------------------------------------------
// Operand substitution
// ---------------------------------------------------------------------------

fn replace_operand(operand: &mut Operand, old: Operand, new: Operand) {
    if *operand == old {
        *operand = new;
    }
}

fn replace_in_place(place: &mut crate::mir::Place, old: Operand, new: Operand) {
    match &mut place.base {
        crate::mir::PlaceBase::Slot(_) => {}
        crate::mir::PlaceBase::Deref(operand) => replace_operand(operand, old, new),
    }
}

fn replace_in_rvalue(rvalue: &mut Rvalue, old: Operand, new: Operand) {
    match rvalue {
        Rvalue::Use(operand) => replace_operand(operand, old, new),
        Rvalue::Binary { op: _, lhs, rhs } => {
            replace_operand(lhs, old, new);
            replace_operand(rhs, old, new);
        }
        Rvalue::Unary { op: _, operand } => replace_operand(operand, old, new),
        Rvalue::Call { callee, args } => {
            match callee {
                crate::mir::Callee::Direct(_) => {}
                crate::mir::Callee::Indirect(operand) => replace_operand(operand, old, new),
            }
            for arg in args {
                replace_operand(arg, old, new);
            }
        }
        Rvalue::Load(place) | Rvalue::Address(place) => replace_in_place(place, old, new),
        Rvalue::Undef => {}
    }
}

#[cfg(test)]
mod tests {
    use jr_hir::ProcId;
    use jr_pool::Pool;

    use super::*;
    use crate::mir::Target;
    use crate::verify;

    fn local(index: usize) -> LocalId {
        LocalId::from_usize(index)
    }

    /// The entry block, as a free function so a test can name it while `mir` is
    /// mutably borrowed by a builder call.
    const fn entry() -> BlockId {
        BlockId::from_usize(0)
    }

    fn body() -> MirBody {
        MirBody::new(ProcId::from_usize(0), PoolId::VOID)
    }

    fn int(pool: &mut Pool, value: u64) -> Operand {
        Operand::Constant(pool.int_value(PoolId::S64, value))
    }

    #[test]
    fn a_read_in_the_block_that_wrote_it_returns_that_value() {
        let mut pool = Pool::new();
        let mut mir = body();
        let mut ssa = SsaBuilder::new();
        let one = int(&mut pool, 1);
        ssa.seal_block(&mut mir, entry());
        ssa.write_variable(entry(), local(0), one);
        let read = ssa.read_variable(&mut mir, entry(), local(0), PoolId::S64, MirSpan::Synthetic);
        assert_eq!(read, one, "no parameter is needed inside one block");
        assert!(mir.block(entry()).params.is_empty());
    }

    #[test]
    fn a_read_through_a_single_predecessor_needs_no_parameter() {
        let mut pool = Pool::new();
        let mut mir = body();
        let mut ssa = SsaBuilder::new();
        let one = int(&mut pool, 1);

        let next = mir.push_block();
        mir.set_terminator(entry(), Terminator::Goto(Target::new(next)));
        mir.set_terminator(next, Terminator::Return(None));

        ssa.seal_block(&mut mir, entry());
        ssa.write_variable(entry(), local(0), one);
        ssa.seal_block(&mut mir, next);

        let read = ssa.read_variable(&mut mir, next, local(0), PoolId::S64, MirSpan::Synthetic);
        assert_eq!(read, one);
        assert!(
            mir.block(next).params.is_empty(),
            "a straight line needs no phi"
        );
    }

    #[test]
    fn a_join_of_two_different_values_creates_a_block_parameter() {
        let mut pool = Pool::new();
        let mut mir = body();
        let mut ssa = SsaBuilder::new();
        let one = int(&mut pool, 1);
        let two = int(&mut pool, 2);

        let then_ = mir.push_block();
        let else_ = mir.push_block();
        let join = mir.push_block();
        mir.set_terminator(
            entry(),
            Terminator::Branch {
                cond: Operand::Constant(PoolId::TRUE),
                then_: Target::new(then_),
                else_: Target::new(else_),
            },
        );
        mir.set_terminator(then_, Terminator::Goto(Target::new(join)));
        mir.set_terminator(else_, Terminator::Goto(Target::new(join)));
        mir.set_terminator(join, Terminator::Return(None));

        ssa.seal_block(&mut mir, entry());
        ssa.seal_block(&mut mir, then_);
        ssa.seal_block(&mut mir, else_);
        ssa.write_variable(then_, local(0), one);
        ssa.write_variable(else_, local(0), two);
        ssa.seal_block(&mut mir, join);

        let read = ssa.read_variable(&mut mir, join, local(0), PoolId::S64, MirSpan::Synthetic);
        let params = &mir.block(join).params;
        assert_eq!(
            params.len(),
            1,
            "two different values must merge through a parameter"
        );
        assert_eq!(read, Operand::Value(params[0]));
        assert_eq!(mir.block(then_).term.targets()[0].args, vec![one]);
        assert_eq!(mir.block(else_).term.targets()[0].args, vec![two]);
        assert_eq!(verify::verify(&mir, &pool), Vec::new());
    }

    #[test]
    fn a_join_of_one_value_collapses_the_parameter_away() {
        let mut pool = Pool::new();
        let mut mir = body();
        let mut ssa = SsaBuilder::new();
        let one = int(&mut pool, 1);

        let then_ = mir.push_block();
        let else_ = mir.push_block();
        let join = mir.push_block();
        mir.set_terminator(
            entry(),
            Terminator::Branch {
                cond: Operand::Constant(PoolId::TRUE),
                then_: Target::new(then_),
                else_: Target::new(else_),
            },
        );
        mir.set_terminator(then_, Terminator::Goto(Target::new(join)));
        mir.set_terminator(else_, Terminator::Goto(Target::new(join)));
        mir.set_terminator(join, Terminator::Return(None));

        ssa.seal_block(&mut mir, entry());
        ssa.write_variable(entry(), local(0), one);
        ssa.seal_block(&mut mir, then_);
        ssa.seal_block(&mut mir, else_);
        ssa.seal_block(&mut mir, join);

        let read = ssa.read_variable(&mut mir, join, local(0), PoolId::S64, MirSpan::Synthetic);
        assert_eq!(read, one, "both arms agree, so the parameter is redundant");
        assert!(
            mir.block(join).params.is_empty(),
            "a trivial parameter must be removed, or every snapshot carries the clutter"
        );
        assert!(mir.block(then_).term.targets()[0].args.is_empty());
        assert_eq!(verify::verify(&mir, &pool), Vec::new());
    }

    #[test]
    fn a_loop_header_read_before_the_back_edge_exists_terminates() {
        let mut pool = Pool::new();
        let mut mir = body();
        let mut ssa = SsaBuilder::new();
        let zero = int(&mut pool, 0);
        let one = int(&mut pool, 1);

        let header = mir.push_block();
        let body_block = mir.push_block();
        let exit = mir.push_block();

        ssa.seal_block(&mut mir, entry());
        ssa.write_variable(entry(), local(0), zero);
        mir.set_terminator(entry(), Terminator::Goto(Target::new(header)));

        // The header is filled — and read — while its back edge does not exist, so
        // it is deliberately *not* sealed yet. This is the case the incomplete
        // parameter exists for.
        let inside = ssa.read_variable(&mut mir, header, local(0), PoolId::S64, MirSpan::Synthetic);
        mir.set_terminator(
            header,
            Terminator::Branch {
                cond: Operand::Constant(PoolId::TRUE),
                then_: Target::new(body_block),
                else_: Target::new(exit),
            },
        );

        ssa.seal_block(&mut mir, body_block);
        let next = mir.push_value(PoolId::S64, MirSpan::Synthetic);
        mir.stmts_mut(body_block).push(Statement::Assign {
            dest: next,
            rvalue: Rvalue::Binary {
                op: crate::mir::BinOp::Add,
                lhs: inside,
                rhs: one,
            },
            span: MirSpan::Synthetic,
        });
        ssa.write_variable(body_block, local(0), Operand::Value(next));
        mir.set_terminator(body_block, Terminator::Goto(Target::new(header)));

        // Now the back edge exists, so the header can be sealed.
        ssa.seal_block(&mut mir, header);
        ssa.seal_block(&mut mir, exit);
        mir.set_terminator(exit, Terminator::Return(None));

        let params = &mir.block(header).params;
        assert_eq!(
            params.len(),
            1,
            "a loop-carried variable needs exactly one parameter"
        );
        assert_eq!(
            mir.block(entry()).term.targets()[0].args,
            vec![zero],
            "the pre-header supplies the initial value"
        );
        assert_eq!(
            mir.block(body_block).term.targets()[0].args,
            vec![Operand::Value(next)],
            "the back edge supplies the updated value"
        );
    }

    #[test]
    fn a_loop_whose_variable_never_changes_keeps_no_parameter() {
        let mut pool = Pool::new();
        let mut mir = body();
        let mut ssa = SsaBuilder::new();
        let zero = int(&mut pool, 0);

        let header = mir.push_block();
        let body_block = mir.push_block();
        let exit = mir.push_block();

        ssa.seal_block(&mut mir, entry());
        ssa.write_variable(entry(), local(0), zero);
        mir.set_terminator(entry(), Terminator::Goto(Target::new(header)));

        let inside = ssa.read_variable(&mut mir, header, local(0), PoolId::S64, MirSpan::Synthetic);
        mir.set_terminator(
            header,
            Terminator::Branch {
                cond: Operand::Constant(PoolId::TRUE),
                then_: Target::new(body_block),
                else_: Target::new(exit),
            },
        );
        ssa.seal_block(&mut mir, body_block);
        mir.set_terminator(body_block, Terminator::Goto(Target::new(header)));
        ssa.seal_block(&mut mir, header);
        ssa.seal_block(&mut mir, exit);
        mir.set_terminator(exit, Terminator::Return(None));

        assert!(
            mir.block(header).params.is_empty(),
            "the only operands are the initial value and the parameter itself"
        );
        // `inside` is now a *stale* handle: it named the parameter that sealing
        // collapsed. `replace_uses` rewrote every occurrence already emitted into
        // the body, but it cannot reach an operand a caller is still holding in a
        // local variable. That is why lowering must read a variable at the point it
        // uses it, and never across a `seal_block`; `verify`'s "value never
        // defined" check turns a violation into a test failure rather than silent
        // corruption.
        let _stale = inside;
    }

    #[test]
    fn a_read_with_no_definition_anywhere_records_an_undefined_read() {
        let pool = Pool::new();
        let mut mir = body();
        let mut ssa = SsaBuilder::new();
        mir.set_terminator(entry(), Terminator::Return(None));
        ssa.seal_block(&mut mir, entry());

        let read = ssa.read_variable(&mut mir, entry(), local(3), PoolId::S64, MirSpan::Synthetic);
        assert!(
            matches!(read, Operand::Value(_)),
            "an undefined read still needs a value"
        );
        let reads = ssa.into_undefined_reads();
        assert_eq!(reads.len(), 1);
        assert_eq!(reads[0].local, local(3));
        assert_eq!(
            verify::verify(&mir, &pool),
            Vec::new(),
            "an undefined value is well-formed MIR, not poison"
        );
    }

    #[test]
    fn reading_the_same_undefined_variable_twice_reuses_the_memo() {
        let pool = Pool::new();
        let mut mir = body();
        let mut ssa = SsaBuilder::new();
        mir.set_terminator(entry(), Terminator::Return(None));
        ssa.seal_block(&mut mir, entry());

        let first = ssa.read_variable(&mut mir, entry(), local(0), PoolId::S64, MirSpan::Synthetic);
        let second =
            ssa.read_variable(&mut mir, entry(), local(0), PoolId::S64, MirSpan::Synthetic);
        assert_eq!(first, second);
        assert_eq!(
            ssa.into_undefined_reads().len(),
            1,
            "one undefined local, one report"
        );
        assert_eq!(verify::verify(&mir, &pool), Vec::new());
    }

    #[test]
    fn sealing_twice_is_harmless() {
        let mut mir = body();
        let mut ssa = SsaBuilder::new();
        ssa.seal_block(&mut mir, entry());
        ssa.seal_block(&mut mir, entry());
        assert!(mir.block(entry()).params.is_empty());
    }
}
