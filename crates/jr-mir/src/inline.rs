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
//! # A non-leaf callee, and where termination comes from now
//!
//! ADR-0021 §4 took the leaf rule — a callee containing any call was refused — and said
//! that single condition was "the whole termination argument": a recursive procedure
//! calls something, so it is never inlined. **ADR-0145 supersedes that**, because the
//! rule also refuses the shape a standard library is full of: `sort_ints` calls `sort`
//! calls `less_int`, and the middle procedure stopped the chain for every caller above
//! it.
//!
//! Termination has **two** guards now, and they are for different things. A callee that can
//! reach itself through the available bodies is **refused** — which keeps the structural
//! argument for cycles *and* preserves a recursive trap's backtrace, since an inlined callee
//! has no frame (ADR-0021 §3) and in a recursive trap the depth is the message. And
//! [`inline_body`] runs at most [`MAX_INLINE_ROUNDS`] rounds, each splicing only the sites
//! that existed when it began, which bounds the *nesting depth* without needing
//! per-statement provenance MIR does not carry.
//!
//! [`MAX_INLINED_STATEMENTS`] then stops a fan-out of medium callees from exploding one
//! body. Both numbers are guesses, said to be guesses where they are declared, with the
//! properties they bound pinned by tests instead.
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
    BlockId, Callee, MirBody, MirSpan, Operand, Place, PlaceBase, ProcRef, Projection, Rvalue,
    SlotId, Statement, Target, Terminator, ValueId,
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

/// How many rounds of splicing [`inline_body`] performs.
///
/// **This is the termination argument** (ADR-0145 §1). A callee may contain calls of its
/// own, so a splice copies calls *in*; each round visits only the sites that existed when
/// it began, which makes the round number the inlining depth. Three levels collapses the
/// wrapper chains this exists for (`sort_ints` → `sort` → `less_int`) and unrolls a
/// recursive procedure three times, leaving a real call at the bottom.
///
/// A guess, like [`MAX_INLINE_STATEMENTS`], and said to be one for the same reason: the
/// measurement that would justify a number is W8's throughput work. What is pinned by
/// tests is the *behaviour* it bounds, so this can be tuned without a property changing
/// silently.
pub const MAX_INLINE_ROUNDS: usize = 3;

/// How large a body may grow before [`inline_body`] stops splicing into it.
///
/// A fan-out of medium callees can explode one body even when every individual callee is
/// under [`MAX_INLINE_STATEMENTS`], which the leaf rule used to make unlikely by refusing
/// most of them. Checked before each splice, so a body over budget takes no further splice
/// and the pass stops **for that body only**.
///
/// Also a guess. Roughly ten times [`MAX_INLINE_STATEMENTS`], on the reasoning that a body
/// which has absorbed ten wrappers has had the benefit.
pub const MAX_INLINED_STATEMENTS: usize = 256;

/// Whether `callee` is small enough and free enough of recursion to inline.
///
/// Two conditions, and ADR-0145 §1 replaced the leaf rule with the second:
///
/// - **Size.** Fewer than [`MAX_INLINE_STATEMENTS`] statements, ignoring [`Statement::Nop`]
///   so that a body an earlier splice left holes in is not penalised for them.
/// - **No cycle.** A callee that can reach *itself* through the bodies available for
///   inlining is refused. A callee that merely calls something else is now eligible, which
///   is the point of the change.
///
/// **The cycle check is not (only) about termination — it is about backtraces**, and that is
/// what building this found. Unrolling recursion is a legitimate optimisation and it costs a
/// documented promise: an inlined callee has no frame (ADR-0021 §3), and ADR-0066 §4 defers
/// inline-provenance backtraces, so every flattened frame is a frame permanently missing
/// from a diagnostic. In a recursive trap the *depth* is the message — a chain of four
/// `countdown` frames reported as one would be a backtrace that lies about what happened —
/// so the case where flattening costs the most is exactly the case whose benefit was never
/// measured. It is refused instead.
///
/// The check is over `callees` rather than over a program call graph, and that is the right
/// scope rather than a compromise: a cycle whose members are not all available for inlining
/// cannot be spliced through anyway, because the unavailable call is not a site.
#[must_use]
pub fn is_inlinable(proc: ProcRef, callee: &MirBody, callees: &Callees<'_>) -> bool {
    statement_count(callee) < MAX_INLINE_STATEMENTS && !reaches_itself(proc, callee, callees)
}

/// Whether `proc` can reach itself through the bodies in `callees`.
///
/// A depth-first walk over direct callees, with a visited set — so a diamond is walked once
/// and a cycle *not* through `proc` terminates rather than spinning. Bounded by the number of
/// available bodies, which is what makes this cheap enough to ask per call site.
fn reaches_itself(proc: ProcRef, callee: &MirBody, callees: &Callees<'_>) -> bool {
    let mut seen: FxHashMap<ProcRef, ()> = FxHashMap::default();
    let mut stack: Vec<&MirBody> = vec![callee];
    while let Some(body) = stack.pop() {
        for target in direct_calls(body) {
            if target == proc {
                return true;
            }
            if seen.insert(target, ()).is_some() {
                continue;
            }
            if let Some(next) = callees.get(target) {
                stack.push(next);
            }
        }
    }
    false
}

/// Every procedure `body` calls directly.
///
/// `Callee::Indirect` contributes nothing, for the same reason it is never an inline site:
/// nothing maps a procedure *value* back to a [`ProcRef`]. That makes this check blind to a
/// cycle closed through a procedure pointer — which is stated rather than hidden, and is
/// harmless here because such a call is not a site either, so no cycle can be *spliced*
/// through it.
fn direct_calls(body: &MirBody) -> Vec<ProcRef> {
    let mut out = Vec::new();
    for block in body.blocks() {
        for stmt in &block.stmts {
            let rvalue = match stmt {
                Statement::Assign { rvalue, .. } | Statement::Discard { rvalue, .. } => rvalue,
                Statement::Store { .. }
                | Statement::Zero { .. }
                | Statement::BoundsCheck { .. }
                | Statement::TagCheck { .. }
                | Statement::Nop => continue,
            };
            if let Rvalue::Call {
                callee: Callee::Direct(target),
                ..
            } = rvalue
            {
                out.push(*target);
            }
        }
    }
    out
}

/// How many statements a body has, ignoring [`Statement::Nop`].
///
/// Shared by [`is_inlinable`]'s callee test and [`inline_body`]'s caller budget, because
/// the two are the same question about different bodies and two spellings of "how big is
/// this" would be free to disagree about whether a `Nop` counts.
fn statement_count(body: &MirBody) -> usize {
    body.blocks()
        .iter()
        .flat_map(|block| block.stmts.iter())
        .filter(|stmt| !matches!(stmt, Statement::Nop))
        .count()
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
    // **Rounds are the termination argument** (ADR-0145 §1). A splice copies the callee's
    // own calls into the caller, so those are new sites; visiting them only on the next
    // round makes the round number the inlining depth, with no provenance on a statement.
    // Without the bound a recursive callee would splice itself forever.
    for _ in 0..MAX_INLINE_ROUNDS {
        // **A worklist, not an index walk over every block.** A splice appends two kinds of
        // block: the *continuation*, which holds the caller's own statements from after the
        // call, and the *copied callee* blocks. The continuation must be visited in this
        // round, or a second call in one original block would wait a round for no reason.
        // The copied blocks must **not** be, because their calls are the deeper level — that
        // is what makes the round number the depth. Only `splice` can tell them apart, so it
        // returns its continuation and this pushes that.
        let mut worklist: Vec<BlockId> = (0..body.block_count()).map(BlockId::from_usize).collect();
        let mut round = 0usize;
        let mut next = 0usize;
        while next < worklist.len() {
            let id = worklist[next];
            next += 1;
            // Checked per splice rather than once per round, so a body that crosses the
            // budget mid-round stops there rather than absorbing the rest of it.
            if statement_count(body) >= MAX_INLINED_STATEMENTS {
                break;
            }
            if let Some(site) = next_site(body, id, callees) {
                let cont = splice(body, site, callees);
                worklist.push(cont);
                round += 1;
                // The same block may hold another call before the one just spliced was
                // reached; re-visiting it costs one `next_site` scan and is what keeps a
                // block of several calls to one round.
                worklist.push(id);
            }
        }
        spliced += round;
        if round == 0 {
            break;
        }
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
            Statement::Store { .. }
            | Statement::Zero { .. }
            | Statement::BoundsCheck { .. }
            | Statement::TagCheck { .. }
            | Statement::Nop => continue,
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
        if !is_inlinable(*proc, callee, callees) {
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
///
/// Returns the **continuation** block — the one holding whatever followed the call. The
/// caller needs it to tell a continuation apart from a copied callee block, which is what
/// bounds the inlining depth (ADR-0145 §1).
fn splice(body: &mut MirBody, site: Site, callees: &Callees<'_>) -> BlockId {
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
    cont
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
            // A projection used to be only a field index or a deref step, neither of
            // which names anything body-local — so cloning the path was correct.
            // `Projection::Index` carries an `Operand`, which *is* body-local: cloning it
            // would leave the callee's `ValueId` in the caller's body, where it means a
            // different value or none at all. Remapped like every other operand.
            projection: place
                .projection
                .iter()
                .map(|step| match step {
                    Projection::Index(operand) => Projection::Index(self.operand(operand)),
                    Projection::Field(_)
                    | Projection::Deref
                    | Projection::StringData
                    | Projection::StringCount
                    | Projection::ViewData
                    | Projection::ViewCount
                    | Projection::DynamicArrayData
                    | Projection::DynamicArrayCount
                    | Projection::DynamicArrayCapacity
                    | Projection::VariantTag => *step,
                })
                .collect(),
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
            Rvalue::Convert { operand, from } => Rvalue::Convert {
                operand: self.operand(operand),
                from: *from,
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
            Statement::Zero { place, span: _ } => Statement::Zero {
                place: self.place(place),
                span: self.span(),
            },
            Statement::BoundsCheck {
                index,
                len,
                span: _,
            } => Statement::BoundsCheck {
                index: self.operand(index),
                len: self.operand(len),
                span: self.span(),
            },
            // The case index is a constant that travels unchanged; only the place is remapped into the
            // caller's value space, and the span becomes the call's (ADR-0021 §3).
            Statement::TagCheck {
                place,
                case,
                span: _,
            } => Statement::TagCheck {
                place: self.place(place),
                case: *case,
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
