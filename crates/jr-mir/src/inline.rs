//! The inliner: splicing a small leaf callee into its caller's CFG.
//!
//! # Why this exists here and not in the back end
//!
//! ADR-0009 decided that the inliner is ours and lives in MIR, because Cranelift
//! cannot inline and a future `#expand` needs inlining as a *semantic* rather than
//! as an optimisation. ADR-0019 §6 deferred it behind a named expiry;
//! [ADR-0021](../../../docs/adr/0021-inliner-and-optimized-mir.md) is that expiry
//! coming due and is this module's specification.
//!
//! # Why splicing is more work than it looks
//!
//! In rustc a call is a *terminator*, so a call site is already a block boundary
//! and inlining is a graph substitution. Here [`Rvalue::Call`] sits inside a
//! [`Statement`] in the middle of a block (ADR-0017 §1), because a Jairs call
//! cannot unwind and so has no second edge to justify a terminator. Every splice
//! must therefore *split* its block:
//!
//! ```text
//!   before                          after
//!   ┌──────────────┐                ┌──────────────┐
//!   │ s0           │                │ s0           │
//!   │ x = f(a)     │                │ nop          │
//!   │ s2           │                └──── goto ─────┐  args: a
//!   │ term         │                                ▼
//!   └──────────────┘                     ┌──────────────────┐
//!                                        │ copy of f's body │
//!                                        └──── goto ─────┐  args: the returned operand
//!                                                        ▼
//!                                             ┌──────────────────┐
//!                                             │ params: [x]      │
//!                                             │ s2               │
//!                                             │ term             │
//!                                             └──────────────────┘
//! ```
//!
//! Two details in that picture are load-bearing.
//!
//! The call's destination value becomes the **continuation block's parameter**
//! rather than a fresh value that something has to be copied into. `x` keeps its
//! identity, so every later use of it is untouched and there is no copy to
//! propagate away later — which matters because there is no copy-propagation pass
//! to do it. The callee's `Return(v)` becomes a `Goto` supplying `v` as that
//! parameter's argument, so several returns merge exactly the way ADR-0017 §1's
//! block parameters were chosen to merge.
//!
//! The call statement is left as a [`Statement::Nop`] rather than removed.
//! ADR-0017 §1 declared that variant for a pass that wanted to delete a statement
//! without shifting every later index in the block; this is that pass, and it is
//! its first producer.
//!
//! # Why only a leaf callee, and why that is the whole termination argument
//!
//! [`is_inlinable`] requires the callee to contain no call of its own. That single
//! condition does two jobs. It bounds the work — one splice per call site, with no
//! iteration to a fixed point. And it makes termination *structural*: a recursive
//! procedure calls something, so it is not a leaf, so it is never inlined, and
//! neither is any member of a mutual-recursion cycle. There is deliberately no
//! depth counter and no recursion check in this module, because no code path needs
//! one. ADR-0021 §4 records the general cost model that was rejected in its favour.
//!
//! # Why every copied span becomes the call's span
//!
//! ADR-0021 §3. It is not only about diagnostic quality: [`MirSpan::Expr`],
//! [`MirSpan::Local`], [`MirSpan::Stmt`] and [`MirSpan::Param`] all name arenas
//! belonging to the *callee's* file, while `resolve_span` is handed the caller's
//! `FileHir`. A surviving callee span would index the wrong file's arena and
//! resolve to a plausible wrong line rather than to nothing.
//!
//! No `MirSpan` carries a `FileId`, so a verifier cannot detect a foreign span.
//! The guarantee is structural instead: every span the splice writes comes from
//! [`Splice::span`], which takes no argument and returns the call site's span, so a
//! copy site has no way to pass a callee's span through it. That is the shape
//! ADR-0020 §4 used for bytecode spans, for the same reason.
//!
//! # What is deliberately not done
//!
//! - **Nothing decides which bodies may be modified.** ADR-0021 §2 keeps every
//!   body the `#run` closure reaches byte-identical to its built form, and that is
//!   `jr-db`'s decision because the closure is a query's business: this module
//!   inlines into whatever body it is handed. A caller that hands it a frozen body
//!   gets it inlined, and the query is what must not do that.
//! - **No DCE and no const-prop.** A splice leaves `Nop`s behind and can make a
//!   copied block unreachable. Both are ADR-0021's follow-on work; neither is
//!   wrong, only untidy, and a `Nop` costs nothing in either engine.
//! - **The callee's [`Facts`](crate::Facts) are not copied.** Diagnostics are
//!   computed from *built* MIR, so the callee still reports its own undefined reads
//!   and stray jumps once, at its own definition. Copying them here would report
//!   them a second time at every call site, which is precisely the re-reporting
//!   ADR-0017 §4's follow-on work forbids.

use rustc_hash::FxHashMap;

use jr_pool::Pool;

use crate::mir::{
    BlockId, Callee, MirBody, MirSpan, Operand, Place, PlaceBase, ProcRef, Rvalue, SlotId,
    Statement, Target, Terminator, ValueId,
};
use crate::verify;

/// How many statements a callee may have and still be inlined.
///
/// **This number is a guess and has never been measured.** ADR-0021 §4 accepts
/// that deliberately: the performance number that would justify a real threshold is
/// downstream of the wave that introduced this pass, so a measured value was not
/// available to pick. It is small on purpose — a leaf this size is the wrapper case
/// the inliner exists for (`modules/Basic`'s `print`, `024-hello.jr`'s `add`), and
/// being too conservative costs speed while being too eager costs compile time and
/// code size.
pub const MAX_INLINE_STATEMENTS: usize = 24;

// ---------------------------------------------------------------------------
// The bodies available to copy from
// ---------------------------------------------------------------------------

/// The callee bodies an inlining pass may copy from.
///
/// Keyed by [`ProcRef`] rather than [`jr_hir::ProcId`] so that a cross-file callee
/// — `024-hello.jr` calling `print` from `modules/Basic`, which is the case that
/// motivates inlining at all here — is representable. Populating this is the
/// cross-body read ADR-0017 §3 keeps out of the built-MIR query and ADR-0021 §1
/// allows in the optimized one; this type is the seam between those two claims.
#[derive(Debug, Default)]
pub struct Callees<'a> {
    bodies: FxHashMap<ProcRef, &'a MirBody>,
}

impl<'a> Callees<'a> {
    /// An empty set. Inlining against it is a no-op, which is the correct
    /// behaviour for a file whose callees were all gated or refused.
    #[must_use]
    pub fn new() -> Self {
        Self {
            bodies: FxHashMap::default(),
        }
    }

    /// Makes `body` available as a callee, under its own [`MirBody::proc`].
    ///
    /// Keying on the body's own identity rather than on a caller-supplied one means
    /// a caller cannot register a body under the wrong `ProcRef` and have every
    /// call to one procedure inline a different one.
    pub fn insert(&mut self, body: &'a MirBody) {
        self.bodies.insert(body.proc(), body);
    }

    /// The body of `proc`, if it is available.
    #[must_use]
    pub fn get(&self, proc: ProcRef) -> Option<&'a MirBody> {
        self.bodies.get(&proc).copied()
    }

    /// How many bodies are available.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bodies.len()
    }

    /// Whether no body is available.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bodies.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Eligibility
// ---------------------------------------------------------------------------

/// Whether `callee` is small enough and simple enough to inline.
///
/// The predicate is the whole of ADR-0021 §4 except the caller-side exclusion,
/// which is not this module's decision: the callee makes no call of its own, and it
/// has fewer than [`MAX_INLINE_STATEMENTS`] statements. A [`Statement::Nop`] is not
/// counted, so a body an earlier splice left holes in is not penalised for them.
#[must_use]
pub fn is_inlinable(callee: &MirBody) -> bool {
    let mut statements = 0usize;
    for block in callee.blocks() {
        for stmt in &block.stmts {
            match stmt {
                Statement::Nop => {}
                Statement::Assign { rvalue, .. } | Statement::Discard { rvalue, .. } => {
                    if contains_call(rvalue) {
                        return false;
                    }
                    statements += 1;
                }
                Statement::Store { .. } => statements += 1,
            }
        }
    }
    statements < MAX_INLINE_STATEMENTS
}

/// Whether an rvalue performs a call.
///
/// An exhaustive match rather than a `matches!`, so that a new [`Rvalue`] variant
/// that can call something is a compile error here instead of a leaf test that
/// quietly starts lying.
fn contains_call(rvalue: &Rvalue) -> bool {
    match rvalue {
        Rvalue::Call { .. } => true,
        Rvalue::Use(_)
        | Rvalue::Binary { .. }
        | Rvalue::Unary { .. }
        | Rvalue::Load(_)
        | Rvalue::Address(_)
        | Rvalue::Undef => false,
    }
}

// ---------------------------------------------------------------------------
// The pass
// ---------------------------------------------------------------------------

/// Inlines every eligible call in `body`, and returns how many sites were spliced.
///
/// Verifies the result in debug builds, so a splice that breaks SSA or edge arity
/// is a test failure at the point of the mistake rather than a wrong answer much
/// later. The body is left untouched when nothing was eligible.
///
/// # Panics
/// In a debug build, if the spliced body is malformed. That is the point.
pub fn inline_body(body: &mut MirBody, callees: &Callees<'_>, pool: &Pool) -> usize {
    let mut spliced = 0usize;
    // An index walk rather than an iterator: a splice appends the copied blocks and
    // the continuation, and both must be visited. The continuation carries whatever
    // followed the call, so a second call in the same original block is reached on a
    // later turn of this loop. This terminates because each splice removes one call
    // from the body and copies none in — a leaf callee has none to copy.
    let mut block = 0usize;
    while block < body.block_count() {
        let id = BlockId::from_usize(block);
        if let Some(site) = next_site(body, id, callees) {
            splice(body, site, callees);
            spliced += 1;
        }
        block += 1;
    }
    if spliced > 0 {
        verify::assert_valid(body, pool);
    }
    spliced
}

/// A call worth inlining: where it is, what it calls, and what receives it.
struct Site {
    /// The block the call statement is in.
    block: BlockId,
    /// Its index within that block's statements.
    index: usize,
    /// The callee.
    proc: ProcRef,
    /// The arguments, already in the caller's value space.
    args: Vec<Operand>,
    /// The value the call defines, when the result is used.
    dest: Option<ValueId>,
    /// The call's own span, which every copied span becomes (ADR-0021 §3).
    span: MirSpan,
}

/// The first inlinable call in `block`, if there is one.
fn next_site(body: &MirBody, block: BlockId, callees: &Callees<'_>) -> Option<Site> {
    for (index, stmt) in body.block(block).stmts.iter().enumerate() {
        let (rvalue, dest, span) = match stmt {
            Statement::Assign { dest, rvalue, span } => (rvalue, Some(*dest), *span),
            Statement::Discard { rvalue, span } => (rvalue, None, *span),
            Statement::Store { .. } | Statement::Nop => continue,
        };
        let Rvalue::Call {
            callee: Callee::Direct(proc),
            args,
        } = rvalue
        else {
            // `Callee::Indirect` is refused for the reason both engines refuse it:
            // nothing maps a procedure *value* back to a `ProcRef`, so there is no
            // body to copy. When something does, this is where it becomes eligible.
            continue;
        };
        let callee = callees.get(*proc)?;
        if !is_inlinable(callee) {
            continue;
        }
        // A callee that returns nothing cannot supply the argument a `dest`
        // continuation parameter needs. Rather than invent a void value — there is
        // no `PoolId` for one, only the void *type* — the site is refused. Nothing
        // in the Jairs-0 subset produces it: a void call in statement position
        // lowers to `Discard`.
        if dest.is_some() && !returns_a_value(callee) {
            continue;
        }
        return Some(Site {
            block,
            index,
            proc: *proc,
            args: args.clone(),
            dest,
            span,
        });
    }
    None
}

/// Whether every `Return` in `callee` carries an operand.
///
/// Asked of the *terminators* rather than of [`MirBody::ret`], because the question
/// at a splice is whether there is an operand to hand the continuation, and a body
/// whose return type says one thing while a terminator does another would otherwise
/// produce an arity mismatch that only the verifier catches.
fn returns_a_value(callee: &MirBody) -> bool {
    callee.blocks().iter().all(|block| match &block.term {
        Terminator::Return(value) => value.is_some(),
        Terminator::Goto(_) | Terminator::Branch { .. } | Terminator::Unreachable(_) => true,
    })
}

/// Copies `site`'s callee into the caller and rewires control flow through it.
fn splice(body: &mut MirBody, site: Site, callees: &Callees<'_>) {
    let callee = callees
        .get(site.proc)
        .expect("the site was only created because the callee was available");

    // The continuation first, so that the callee's returns have somewhere to go.
    // Its parameter *is* the call's destination value, which is what keeps every
    // later use of the result correct without a copy.
    let cont = body.push_block();
    let tail: Vec<Statement> = body.block(site.block).stmts[site.index + 1..].to_vec();
    let caller_term = body.block(site.block).term.clone();
    {
        let blocks = body.blocks_mut();
        blocks[cont.index()].params = site.dest.map(|dest| vec![dest]).unwrap_or_default();
        blocks[cont.index()].stmts = tail;
        blocks[cont.index()].term = caller_term;

        let source = &mut blocks[site.block.index()];
        source.stmts.truncate(site.index + 1);
        source.stmts[site.index] = Statement::Nop;
    }

    let mut splice = Splice {
        span: site.span,
        values: FxHashMap::default(),
        slots: FxHashMap::default(),
        blocks: FxHashMap::default(),
    };

    // Every block, value and slot of the callee gets a fresh identity in the
    // caller. Blocks are allocated before any terminator is translated, so that a
    // backward edge — a loop in the callee — has a target to name.
    for index in 0..callee.block_count() {
        let old = BlockId::from_usize(index);
        let new = body.push_block();
        splice.blocks.insert(old, new);
    }
    for index in 0..callee.value_count() {
        let old = ValueId::from_usize(index);
        let ty = callee.value(old).ty;
        let new = body.push_value(ty, splice.span());
        splice.values.insert(old, new);
    }
    for index in 0..callee.slot_count() {
        let old = SlotId::from_usize(index);
        let ty = callee.slot(old).ty;
        // `local: None` deliberately. A `SlotData::local` is a `LocalId` in the
        // *callee's* HIR body, so carrying it over would make the caller's dump
        // name one of its own locals at random. Once copied, the slot genuinely is
        // a compiler temporary, which is what `None` means.
        let new = body.push_slot(ty, None, splice.span());
        splice.slots.insert(old, new);
    }

    for index in 0..callee.block_count() {
        let old = BlockId::from_usize(index);
        let new = splice.blocks[&old];
        let data = callee.block(old);

        let params: Vec<ValueId> = data.params.iter().map(|p| splice.values[p]).collect();
        let stmts: Vec<Statement> = data.stmts.iter().map(|s| splice.stmt(s)).collect();
        let term = splice.terminator(&data.term, cont, site.dest.is_some());

        let blocks = body.blocks_mut();
        blocks[new.index()].params = params;
        blocks[new.index()].stmts = stmts;
        blocks[new.index()].term = term;
    }

    // The call becomes an edge into the copied entry, whose parameters are the
    // callee's own parameters, so the arguments travel as edge arguments.
    let entry = splice.blocks[&callee.entry()];
    body.set_terminator(
        site.block,
        Terminator::Goto(Target::with_args(entry, site.args)),
    );
}

// ---------------------------------------------------------------------------
// Translating one body's entities into another's
// ---------------------------------------------------------------------------

/// The renumbering of one callee's ids into its caller's, plus the span they all
/// collapse to.
struct Splice {
    span: MirSpan,
    values: FxHashMap<ValueId, ValueId>,
    slots: FxHashMap<SlotId, SlotId>,
    blocks: FxHashMap<BlockId, BlockId>,
}

impl Splice {
    /// The span every copied entity gets.
    ///
    /// ADR-0021 §3's choke point: it takes no argument, so no copy site can route a
    /// callee's span through it even by accident. Everything that writes a
    /// [`MirSpan`] during a splice calls this.
    const fn span(&self) -> MirSpan {
        self.span
    }

    fn operand(&self, operand: &Operand) -> Operand {
        match operand {
            Operand::Value(value) => Operand::Value(self.values[value]),
            // A constant is a `PoolId`, and the pool is shared by every body in the
            // program, so it needs no translation at all.
            Operand::Constant(id) => Operand::Constant(*id),
        }
    }

    fn place(&self, place: &Place) -> Place {
        Place {
            base: match &place.base {
                PlaceBase::Slot(slot) => PlaceBase::Slot(self.slots[slot]),
                PlaceBase::Deref(operand) => PlaceBase::Deref(self.operand(operand)),
            },
            // A projection is a field index or a deref step, neither of which names
            // anything body-local (ADR-0017 §5 keeps offsets out of MIR).
            projection: place.projection.clone(),
        }
    }

    fn rvalue(&self, rvalue: &Rvalue) -> Rvalue {
        match rvalue {
            Rvalue::Use(operand) => Rvalue::Use(self.operand(operand)),
            Rvalue::Binary { op, lhs, rhs } => Rvalue::Binary {
                op: *op,
                lhs: self.operand(lhs),
                rhs: self.operand(rhs),
            },
            Rvalue::Unary { op, operand } => Rvalue::Unary {
                op: *op,
                operand: self.operand(operand),
            },
            // Unreachable in practice — `is_inlinable` refuses a callee containing a
            // call — but translated faithfully rather than panicked on, so that
            // relaxing the leaf rule is a change of *policy* and not a crash.
            Rvalue::Call { callee, args } => Rvalue::Call {
                callee: match callee {
                    Callee::Direct(proc) => Callee::Direct(*proc),
                    Callee::Indirect(operand) => Callee::Indirect(self.operand(operand)),
                },
                args: args.iter().map(|arg| self.operand(arg)).collect(),
            },
            Rvalue::Load(place) => Rvalue::Load(self.place(place)),
            Rvalue::Address(place) => Rvalue::Address(self.place(place)),
            Rvalue::Undef => Rvalue::Undef,
        }
    }

    fn stmt(&self, stmt: &Statement) -> Statement {
        match stmt {
            Statement::Assign {
                dest,
                rvalue,
                span: _,
            } => Statement::Assign {
                dest: self.values[dest],
                rvalue: self.rvalue(rvalue),
                span: self.span(),
            },
            Statement::Store {
                place,
                value,
                span: _,
            } => Statement::Store {
                place: self.place(place),
                value: self.operand(value),
                span: self.span(),
            },
            Statement::Discard { rvalue, span: _ } => Statement::Discard {
                rvalue: self.rvalue(rvalue),
                span: self.span(),
            },
            Statement::Nop => Statement::Nop,
        }
    }

    fn target(&self, target: &Target) -> Target {
        Target::with_args(
            self.blocks[&target.block],
            target.args.iter().map(|arg| self.operand(arg)).collect(),
        )
    }

    /// Translates a terminator, turning a `Return` into an edge to `cont`.
    ///
    /// `wants_value` says whether `cont` has a parameter to feed. A returned
    /// operand is dropped when it does not — the call was in statement position, so
    /// nothing was going to read the result.
    fn terminator(&self, term: &Terminator, cont: BlockId, wants_value: bool) -> Terminator {
        match term {
            Terminator::Goto(target) => Terminator::Goto(self.target(target)),
            Terminator::Branch { cond, then_, else_ } => Terminator::Branch {
                cond: self.operand(cond),
                then_: self.target(then_),
                else_: self.target(else_),
            },
            Terminator::Return(value) => {
                let args = match (wants_value, value) {
                    (true, Some(operand)) => vec![self.operand(operand)],
                    // `(true, None)` cannot occur: `next_site` refuses a site whose
                    // result is used unless every return carries an operand.
                    (true, None) | (false, _) => Vec::new(),
                };
                Terminator::Goto(Target::with_args(cont, args))
            }
            // A trap inside the callee stays a trap. A terminator carries no
            // `MirSpan` of its own, so there is nothing to rewrite here; the traps
            // ADR-0021 §3 is about are the arithmetic ones, and those live in
            // statements, which `stmt` has already re-spanned.
            Terminator::Unreachable(reason) => Terminator::Unreachable(*reason),
        }
    }
}
