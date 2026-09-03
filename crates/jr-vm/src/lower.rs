//! MIR to bytecode: linearise the CFG, resolve every offset, eliminate block parameters.
//!
//! # The three things this module actually does
//!
//! Everything else is a rename. ADR-0018 §1 chose a register machine addressed by
//! `ValueId` precisely so that most of lowering is `Statement` → `Instr` with the
//! same operands, which leaves three real jobs:
//!
//! 1. **Linearise.** Blocks are emitted in `MirBody::reverse_postorder()` and jump
//!    targets become absolute instruction indices. Forward jumps are patched
//!    afterwards, because a block's address is not known until it is emitted.
//!    Unreachable blocks are not emitted at all: `jr-mir` has no DCE so a dump shows
//!    them, but there is no reason to give them addresses.
//! 2. **Resolve layout.** Every [`jr_mir::Projection`] becomes a byte offset here,
//!    once, from `jr-pool`'s layout — the same function Cranelift will call, which
//!    is the whole point of ADR-0018 §2. The interpreter then walks adds and pointer
//!    loads and never consults the pool's field tables.
//! 3. **Eliminate block parameters.** A phi is a block parameter in MIR (ADR-0017
//!    §1); bytecode has no such thing, so each edge becomes copies from its
//!    arguments into the target's parameter registers.
//!
//! # Why the edge copies are *parallel*
//!
//! An edge's copies happen simultaneously in MIR's semantics, and a loop back-edge
//! can genuinely permute: `bb1(a, b)` reached with `args: [b, a]` must swap, and
//! emitting `a <- b; b <- a` would produce two copies of `b`. So when any
//! destination also appears as a source, every source is first read into a fresh
//! temporary and only then written to its destination. Detecting the conflict and
//! spilling *all* of them is cruder than the usual cycle-breaking algorithm and
//! costs a few registers on the rare conflicting edge; it is chosen because it is
//! obviously correct, and because a wrong answer here is a miscompile in loop code
//! that no type system would catch.
//!
//! # Why copies can go inline
//!
//! ADR-0017 §1's no-critical-edges invariant, which `jr-mir`'s verifier enforces,
//! guarantees every edge has either a single predecessor or a single successor. So
//! for a `Goto` the copies belong at the end of the source block, and for a `Branch`
//! each arm's target has exactly one predecessor, so its copies belong in a short
//! stub the branch jumps to. Neither placement needs an edge split at lowering time.
//! That is the concrete payoff ADR-0017 promised for enforcing the invariant.

use rustc_hash::FxHashMap;

use jr_mir::{
    Callee, GlobalData, GlobalRef, MirBody, MirSpan, Place, PlaceBase, Projection, Rvalue,
    Statement, Target, Terminator,
};
use jr_pool::{
    Item, Pool, PoolId, TargetLayout, field_offset, layout_of, string_count, string_data,
};

use crate::code::{
    BlockAddresses, Code, Instr, Operand, PlacePlan, PlaceRoot, PlaceStep, Reg, Shape, SlotPlan,
};
use crate::error::VmError;

/// Compiles one MIR body to bytecode, with no globals in scope.
///
/// For a body that structurally cannot read one — `jr-db`'s const-eval bodies, the only other
/// caller of this function, are exactly that: a global's own initialiser, a `#run`, or a thunked
/// top-level constant. ADR-0186 §2 makes a global's initialiser a compile-time constant, and
/// nothing runs before `main` to read *another* global's current value from, so every one of
/// those refuses a [`PlaceBase::Global`] rather than reading through it. `compile_in_file` is
/// the version [`crate::assemble::add_file`] uses for an ordinary body, which can reference one.
///
/// A thin wrapper rather than a fourth parameter on this function, because `jr-db` calls this one
/// directly with its historical three arguments for a body that has no globals to resolve at all. The
/// wrapper's extra argument is the **program's** global table rather than one file's (ADR-0189 §7), since
/// the inliner copies a `GlobalRef` from another file into a host body.
///
/// # Errors
/// [`VmError::Internal`] when MIR, the pool and the target layout disagree — an
/// unresolved struct, a projection of a non-struct, a pointer dereference of a
/// non-pointer. `jr-mir`'s verifier is meant to make all of these unreachable, and
/// returning rather than panicking is what makes a hole in it diagnosable.
///
/// [`VmError::Unsupported`] — not [`VmError::Internal`] — for the [`PlaceBase::Global`] this
/// function's contract says cannot occur: reading one here is not a compiler bug, it is exactly
/// the construct ADR-0186 §2 refuses, reached in the one place refusing it matters (`jr-db`'s
/// `consts.rs` turns this into a diagnostic naming the construct, and degrades the global's own
/// initial value to zero — see `global_data`).
pub fn compile(body: &MirBody, pool: &Pool, target: TargetLayout) -> Result<Code, VmError> {
    Compiler {
        body,
        globals: None,
        pool,
        target,
        instrs: Vec::new(),
        spans: Vec::new(),
        current: MirSpan::Synthetic,
        addresses: BlockAddresses::with_blocks(body.block_count()),
        fixups: Vec::new(),
        types: (0..body.value_count())
            .map(|index| body.value(jr_mir::ValueId::from_usize(index)).ty)
            .collect(),
    }
    .run()
}

/// Compiles one MIR body to bytecode, resolving any [`PlaceBase::Global`] it contains against
/// `file_mir` — the body's own file, per ADR-0186 §1's same-file contract.
///
/// # Errors
/// As [`compile`], minus the global refusal: every global this body can name is in `file_mir`, by
/// construction of the caller ([`crate::assemble::add_file`] passes the same file's own
/// [`FileMir`]).
pub fn compile_in_file(
    body: &MirBody,
    globals: &FxHashMap<GlobalRef, GlobalData>,
    pool: &Pool,
    target: TargetLayout,
) -> Result<Code, VmError> {
    Compiler {
        body,
        globals: Some(globals),
        pool,
        target,
        instrs: Vec::new(),
        spans: Vec::new(),
        current: MirSpan::Synthetic,
        addresses: BlockAddresses::with_blocks(body.block_count()),
        fixups: Vec::new(),
        types: (0..body.value_count())
            .map(|index| body.value(jr_mir::ValueId::from_usize(index)).ty)
            .collect(),
    }
    .run()
}

/// Which field of an already-emitted instruction still needs a real address.
#[derive(Debug, Clone, Copy)]
enum Hole {
    Jump,
    BranchThen,
    BranchElse,
}

struct Compiler<'a> {
    body: &'a MirBody,
    /// This body's file's globals, looked up by `global_data` (ADR-0186 §1).
    ///
    /// `None` for [`compile`]'s const-eval bodies, which cannot read one at all — see
    /// `global_data` for what that produces. `Some`, from `compile_in_file`, holds not
    /// the whole program but just the one [`FileMir`] the caller already has in hand: `jr-mir`'s
    /// contract keeps every [`jr_mir::GlobalRef`] this wave produces same-file, so the body's own
    /// file is enough.
    /// Every global in the **program**, not just this body's file (ADR-0189 §7).
    ///
    /// `None` for a const-eval body, which can observe no global at all. Program-wide rather than
    /// per-file because the inliner copies a `GlobalRef` from another file into this body, so a per-file
    /// table cannot resolve what a body legitimately names.
    globals: Option<&'a FxHashMap<GlobalRef, GlobalData>>,
    pool: &'a Pool,
    target: TargetLayout,
    instrs: Vec<Instr>,
    addresses: BlockAddresses,
    fixups: Vec<(usize, Hole, jr_mir::BlockId)>,
    ///
    /// Doubles as the register count: `types.len()` *is* the frame size, so a
    /// temporary cannot be allocated without giving it a type — which matters,
    /// because ADR-0002's trapping arithmetic reads the destination's type.
    types: Vec<PoolId>,
    /// The span of every emitted instruction, parallel to `instrs`.
    ///
    /// ADR-0020 §4: a span for *every* instruction rather than only for the ones
    /// that can trap. The set of trapping instructions grows every wave, and the
    /// narrow version would silently give a new one no location — an absent detail
    /// rather than a wrong answer, which is the failure mode this project has learned
    /// to distrust.
    spans: Vec<MirSpan>,
    /// The span instructions emitted right now belong to.
    ///
    /// Set once per statement and per terminator rather than threaded through every
    /// helper, because `emit` is the single choke point every instruction passes
    /// through and a field there cannot be forgotten at a call site.
    current: MirSpan,
}

impl Compiler<'_> {
    fn run(mut self) -> Result<Code, VmError> {
        let slots = self.slots()?;

        for block in self.body.reverse_postorder().to_vec() {
            let start = self.instrs.len();
            self.addresses.set(block, start);
            for stmt in &self.body.block(block).stmts {
                self.current = statement_span(stmt);
                self.statement(stmt)?;
            }
            let term = self.body.block(block).term.clone();
            self.current = self.terminator_span(&term);
            self.terminator(&term)?;
        }

        self.patch()?;

        let entry = self
            .addresses
            .get(self.body.entry())
            .ok_or_else(|| VmError::internal("the entry block was not emitted"))?;

        Ok(Code {
            proc: self.body.proc(),
            instrs: self.instrs,
            registers: self.types.len(),
            types: self.types,
            slots,
            params: self.body.params().to_vec(),
            entry,
            spans: self.spans,
        })
    }

    /// Resolves every slot's size and alignment up front.
    fn slots(&self) -> Result<Vec<SlotPlan>, VmError> {
        (0..self.body.slot_count())
            .map(|index| {
                let slot = self.body.slot(jr_mir::SlotId::from_usize(index));
                let layout = layout_of(self.pool, self.target, slot.ty)
                    .map_err(|e| VmError::internal(format!("slot s{index}: {e:?}")))?;
                Ok(SlotPlan {
                    size: layout.size,
                    align: layout.align,
                })
            })
            .collect()
    }

    /// Replaces every placeholder jump target with the block's real address.
    fn patch(&mut self) -> Result<(), VmError> {
        for (pc, hole, block) in core::mem::take(&mut self.fixups) {
            let address = self
                .addresses
                .get(block)
                .ok_or_else(|| crate::error::ice::no_such_block(block))?;
            match (&mut self.instrs[pc], hole) {
                (Instr::Jump { target }, Hole::Jump) => *target = address,
                (Instr::Branch { then_, .. }, Hole::BranchThen) => *then_ = address,
                (Instr::Branch { else_, .. }, Hole::BranchElse) => *else_ = address,
                (other, _) => {
                    return Err(VmError::internal(format!(
                        "fixup at {pc} does not name a jump: {other:?}"
                    )));
                }
            }
        }
        Ok(())
    }

    fn emit(&mut self, instr: Instr) -> usize {
        self.instrs.push(instr);
        self.spans.push(self.current);
        self.instrs.len() - 1
    }

    /// The span a terminator's instructions belong to.
    ///
    /// A [`Terminator`] carries no span of its own, but its operand is a value and
    /// every value does — so a branch reports the condition that was tested and a
    /// return reports the expression that produced the result. A terminator with no
    /// operand has no source text of its own, and says so.
    fn terminator_span(&self, term: &Terminator) -> MirSpan {
        match term {
            Terminator::Branch { cond, .. } => self.operand_span(*cond),
            Terminator::Return(Some(operand)) => self.operand_span(*operand),
            Terminator::Goto(_) | Terminator::Return(None) | Terminator::Unreachable(_) => {
                MirSpan::Synthetic
            }
        }
    }

    /// The span of the value an operand names, if it names one.
    fn operand_span(&self, operand: Operand) -> MirSpan {
        match operand {
            Operand::Value(value) => self.body.value(value).span,
            // A constant was written in the source, but MIR keeps no span for the
            // literal itself — only for the value that uses it.
            Operand::Constant(_) => MirSpan::Synthetic,
        }
    }

    /// A register no MIR value uses, of type `ty`.
    fn temp(&mut self, ty: PoolId) -> Reg {
        let reg = Reg::from_usize(self.types.len());
        self.types.push(ty);
        reg
    }

    /// The type an operand holds, for typing a temporary that stages it.
    fn operand_ty(&self, operand: Operand) -> PoolId {
        match operand {
            Operand::Value(value) => self
                .types
                .get(value.index())
                .copied()
                .unwrap_or(PoolId::ERROR),
            Operand::Constant(id) => self.pool.type_of(id),
        }
    }

    // -------------------------------------------------------------------
    // Statements
    // -------------------------------------------------------------------

    fn statement(&mut self, stmt: &Statement) -> Result<(), VmError> {
        match stmt {
            Statement::Assign {
                dest,
                rvalue,
                span: _,
            } => self.rvalue(Some(*dest), rvalue),
            Statement::Store {
                place,
                value,
                span: _,
            } => {
                let plan = self.plan(place)?;
                self.emit(Instr::Store {
                    place: plan,
                    value: *value,
                });
                Ok(())
            }
            // `PlacePlan::size` is already the byte count of the value at the place, so
            // the size comes from the plan rather than from a second layout walk that
            // could disagree with it.
            Statement::Zero { place, span: _ } => {
                let plan = self.plan(place)?;
                let size = plan.size;
                self.emit(Instr::Zero { place: plan, size });
                Ok(())
            }
            Statement::BoundsCheck {
                index,
                len,
                span: _,
            } => {
                self.emit(Instr::BoundsCheck {
                    index: *index,
                    len: *len,
                });
                Ok(())
            }
            // The place is planned down to the variant itself; the tag sits at its offset 0 (ADR-0068
            // §3), so no extra step is added here.
            Statement::TagCheck { place, case, .. } => {
                let plan = self.plan(place)?;
                self.emit(Instr::TagCheck {
                    place: plan,
                    case: *case,
                });
                Ok(())
            }
            // A discarded rvalue must still be *evaluated*: ADR-0002 makes overflow
            // trap, so `a + b;` in statement position is observable even though
            // nothing reads the sum. Only a call has effects beyond that, but
            // dropping the arithmetic would drop the trap, so everything is
            // evaluated into a register nothing reads.
            Statement::Discard { rvalue, span: _ } => {
                if let Rvalue::Call { callee, args } = rvalue {
                    self.emit(Instr::Call {
                        dest: None,
                        callee: callee.clone(),
                        args: args.clone(),
                    });
                    return Ok(());
                }
                let ty = self.rvalue_ty(rvalue);
                let dest = self.temp(ty);
                self.rvalue(Some(dest), rvalue)
            }
            // Nothing produces this yet; the mid-end will.
            Statement::Nop => Ok(()),
        }
    }

    fn rvalue(&mut self, dest: Option<Reg>, rvalue: &Rvalue) -> Result<(), VmError> {
        // **An atomic is emitted even with no destination**, unlike every other rvalue: a store produces
        // nothing and must still happen (ADR-0176 §4). The early return below is what makes a
        // destination-less rvalue free, and it is right for arithmetic — dropping an unused add changes
        // nothing — and wrong for an operation whose *effect* is the point.
        if let Rvalue::Atomic {
            op,
            address,
            value,
            expected,
        } = rvalue
        {
            self.emit(Instr::Atomic {
                dest,
                op: *op,
                address: *address,
                value: *value,
                expected: *expected,
            });
            return Ok(());
        }
        let Some(dest) = dest else {
            return Ok(());
        };
        match rvalue {
            Rvalue::Use(operand) => {
                self.emit(Instr::Move {
                    dest,
                    src: *operand,
                });
            }
            Rvalue::Convert { operand, from } => {
                self.emit(Instr::Convert {
                    dest,
                    operand: *operand,
                    from: *from,
                });
            }
            Rvalue::Binary { op, lhs, rhs } => {
                self.emit(Instr::Binary {
                    dest,
                    op: *op,
                    lhs: *lhs,
                    rhs: *rhs,
                });
            }
            Rvalue::Unary { op, operand } => {
                self.emit(Instr::Unary {
                    dest,
                    op: *op,
                    operand: *operand,
                });
            }
            Rvalue::Call { callee, args } => {
                self.emit(Instr::Call {
                    dest: Some(dest),
                    callee: callee.clone(),
                    args: args.clone(),
                });
            }
            Rvalue::Load(place) => {
                let place = self.plan(place)?;
                self.emit(Instr::Load { dest, place });
            }
            Rvalue::Address(place) => {
                let place = self.plan(place)?;
                self.emit(Instr::Address { dest, place });
            }
            Rvalue::Undef => {
                self.emit(Instr::Undef { dest });
            }
            // Handled above, before the destination-less early return, because a store has no destination
            // and must still be emitted (ADR-0176 §4).
            Rvalue::Atomic { .. } => unreachable!("an atomic is emitted before the dest check"),
        }
        Ok(())
    }

    // -------------------------------------------------------------------
    // Terminators
    // -------------------------------------------------------------------

    fn terminator(&mut self, term: &Terminator) -> Result<(), VmError> {
        match term {
            Terminator::Goto(target) => {
                self.edge(target)?;
                let pc = self.emit(Instr::Jump { target: usize::MAX });
                self.fixups.push((pc, Hole::Jump, target.block));
            }
            Terminator::Branch { cond, then_, else_ } => {
                let pc = self.emit(Instr::Branch {
                    cond: *cond,
                    then_: usize::MAX,
                    else_: usize::MAX,
                });
                // Each arm gets a stub only if it has copies to make. Otherwise the
                // branch names the target directly, which keeps a dump of a plain
                // `if` free of two pointless jumps.
                self.arm(pc, Hole::BranchThen, then_)?;
                self.arm(pc, Hole::BranchElse, else_)?;
            }
            Terminator::Return(value) => {
                self.emit(Instr::Return(*value));
            }
            Terminator::Unreachable(reason) => {
                self.emit(Instr::Trap(*reason));
            }
        }
        Ok(())
    }

    /// Wires one branch arm, inserting a copy stub when the edge carries arguments.
    fn arm(&mut self, branch: usize, hole: Hole, target: &Target) -> Result<(), VmError> {
        if target.args.is_empty() {
            self.fixups.push((branch, hole, target.block));
            return Ok(());
        }
        let stub = self.instrs.len();
        self.edge(target)?;
        let pc = self.emit(Instr::Jump { target: usize::MAX });
        self.fixups.push((pc, Hole::Jump, target.block));
        match (&mut self.instrs[branch], hole) {
            (Instr::Branch { then_, .. }, Hole::BranchThen) => *then_ = stub,
            (Instr::Branch { else_, .. }, Hole::BranchElse) => *else_ = stub,
            (other, _) => {
                return Err(VmError::internal(format!(
                    "expected a branch at {branch}, found {other:?}"
                )));
            }
        }
        Ok(())
    }

    /// Emits the copies that carry one edge's arguments into the target's parameters.
    fn edge(&mut self, target: &Target) -> Result<(), VmError> {
        let params = self.body.block(target.block).params.clone();
        if params.len() != target.args.len() {
            return Err(VmError::internal(format!(
                "edge to block {} supplies {} arguments for {} parameters",
                target.block.index(),
                target.args.len(),
                params.len()
            )));
        }
        if params.is_empty() {
            return Ok(());
        }

        // Does any destination also appear as a source? If so the copies genuinely
        // permute and must be staged through temporaries.
        let conflicts = target.args.iter().any(|arg| match arg {
            Operand::Value(value) => params.contains(value),
            Operand::Constant(_) => false,
        });

        if conflicts {
            let temps: Vec<Reg> = target
                .args
                .iter()
                .map(|arg| {
                    let ty = self.operand_ty(*arg);
                    self.temp(ty)
                })
                .collect::<Vec<_>>();
            for (temp, arg) in temps.iter().zip(&target.args) {
                self.emit(Instr::Move {
                    dest: *temp,
                    src: *arg,
                });
            }
            for (param, temp) in params.iter().zip(&temps) {
                self.emit(Instr::Move {
                    dest: *param,
                    src: Operand::Value(*temp),
                });
            }
        } else {
            for (param, arg) in params.iter().zip(&target.args) {
                self.emit(Instr::Move {
                    dest: *param,
                    src: *arg,
                });
            }
        }
        Ok(())
    }

    /// The type a discarded rvalue would have produced.
    ///
    /// Only used to type the register nothing reads, so an approximation would be
    /// harmless everywhere except arithmetic — where the destination's width is what
    /// decides whether ADR-0002 traps. So it is exact for the arithmetic cases and
    /// `void` for the rest.
    fn rvalue_ty(&self, rvalue: &Rvalue) -> PoolId {
        match rvalue {
            Rvalue::Use(operand) => self.operand_ty(*operand),
            Rvalue::Binary { lhs, .. } => self.operand_ty(*lhs),
            Rvalue::Unary { operand, .. } => self.operand_ty(*operand),
            // Deliberately *not* the operand's type: a conversion's whole point is that the
            // destination differs from the source, and the destination's width is what the
            // interpreter masks with. `Convert` carries no destination type, so the caller's
            // `dest` type is authoritative and this fallback must not guess the source's.
            // An atomic's result width is the destination's, exactly as a conversion's is: the operand is a
            // *pointer* and would give the wrong width.
            Rvalue::Atomic { .. }
            | Rvalue::Convert { .. }
            | Rvalue::Call { .. }
            | Rvalue::Load(_)
            | Rvalue::Address(_)
            | Rvalue::Undef => PoolId::VOID,
        }
    }

    // -------------------------------------------------------------------
    // Places
    // -------------------------------------------------------------------

    /// The type an operand holds.
    fn operand_type(&self, operand: Operand) -> PoolId {
        self.operand_ty(operand)
    }

    fn pointee(&self, ty: PoolId) -> Result<PoolId, VmError> {
        match self.pool.item(ty) {
            Item::PointerType(pointee) => Ok(*pointee),
            other => Err(VmError::internal(format!(
                "expected a pointer, found {other:?}"
            ))),
        }
    }

    /// This body's file's declaration of `global`.
    ///
    /// # Why a missing [`Self::file_mir`] is refused rather than internal (ADR-0186 §2)
    ///
    /// `None` means this body was compiled by [`compile`] — a const-eval body, which by
    /// construction can never legitimately name a global at all: a global's own initialiser, a
    /// `#run`, and a thunked top-level constant all run with no globals laid out yet, because
    /// nothing runs before `main`. So reaching a [`PlaceBase::Global`] here is not this compiler
    /// disagreeing with itself — it is the one place ADR-0186 §2's refusal actually happens.
    /// [`VmError::Unsupported`] says so in the wording a reader outside this crate sees: `jr-db`'s
    /// `consts.rs` turns an `Err` from a failed `Wanted::GlobalInit` into exactly this — a
    /// diagnostic at the global's own declaration, and the global's initial value degrading to
    /// zero, the third of ADR-0186 §2's three `None` cases.
    ///
    /// # Why the file must be the body's own
    ///
    /// `GlobalRef::file` names where the variable is declared, but ADR-0186 §1 keeps every
    /// reference this wave produces same-file — cross-file is deliberately not built. So this
    /// checks that rather than trusting it, because [`Self::file_mir`] is the *body's* file: a
    /// mismatched `file` would silently look the item up in the wrong file's item numbering,
    /// which is a wrong-storage bug rather than a diagnosable one if it went unchecked — and
    /// unlike the missing-`file_mir` case above, a real per-file compile naming the wrong file
    /// *is* this compiler disagreeing with `jr-mir`, so it stays [`VmError::Internal`].
    fn global_data(&self, global: GlobalRef) -> Result<GlobalData, VmError> {
        let Some(globals) = self.globals else {
            return Err(VmError::unsupported(
                "a global variable's current value cannot be read here: nothing runs before \
                 `main` to have set one, so a global's own initialiser and other compile-time-only \
                 evaluation can never observe one (ADR-0186 §2)",
            ));
        };
        // **No same-file check** (ADR-0189 §7). There used to be one, comparing `global.file` against
        // the body's own and refusing a mismatch as an internal error — resting on ADR-0186's claim that
        // only same-file globals occur. That claim was wrong for a reason no *program* reveals: the
        // **inliner** copies a `GlobalRef` unchanged into a host body in another file, because a
        // `GlobalRef` is absolute (ADR-0186 §3 decided that deliberately). So `Basic.print` inlined into
        // a caller made this fire on an ordinary print, reporting a feature nobody had used.
        globals.get(&global).copied().ok_or_else(|| {
            VmError::internal(format!(
                "no global for file {} item {}",
                global.file.index(),
                global.item.index()
            ))
        })
    }

    /// Resolves a MIR place into a plan with every offset computed.
    fn plan(&mut self, place: &Place) -> Result<PlacePlan, VmError> {
        let (root, mut ty) = match &place.base {
            PlaceBase::Slot(slot) => (PlaceRoot::Slot(slot.index()), self.body.slot(*slot).ty),
            PlaceBase::Deref(operand) => {
                let pointer_ty = self.operand_type(*operand);
                (PlaceRoot::Address(*operand), self.pointee(pointer_ty)?)
            }
            PlaceBase::Global(global) => {
                (PlaceRoot::Global(*global), self.global_data(*global)?.ty)
            }
        };

        let mut steps = Vec::with_capacity(place.projection.len());
        for step in &place.projection {
            match step {
                Projection::Field(index) => {
                    let (offset, _) = field_offset(self.pool, self.target, ty, *index)
                        .map_err(|e| VmError::internal(format!("field {index}: {e:?}")))?;
                    if offset != 0 {
                        steps.push(PlaceStep::Offset(offset));
                    }
                    ty = self.field_type(ty, *index)?;
                }
                Projection::Index(index) => {
                    // Two shapes reach here. An **array** place is indexed in place. A
                    // **pointer** place — which is what a view's `data` word is — is read
                    // through first, exactly as a `Deref` step would, and then indexed. One
                    // arm rather than two projections, so an array element and a view element
                    // are scaled by the same stride computation and cannot drift.
                    let elem = match self.pool.item(ty) {
                        Item::ArrayType { elem, .. } => *elem,
                        // A vector lane, indexed exactly as an array element is: the layouts are
                        // identical, so the same stride computation below is the right one
                        // (ADR-0148 §1). In the VM a vector *is* those bytes in memory (§4), so
                        // there is nothing else it could be.
                        Item::VectorType { elem, .. } => *elem,
                        Item::PointerType(pointee) => {
                            let pointee = *pointee;
                            steps.push(PlaceStep::Indirect {
                                size: u64::from(self.target.pointer_size),
                            });
                            pointee
                        }
                        _ => {
                            return Err(VmError::internal("an index into a non-array"));
                        }
                    };
                    // The stride is the element size rounded up to its alignment — the
                    // same computation `jr-pool`'s `layout_of` uses for the array's total
                    // size, so an element's address here and the array's size there cannot
                    // disagree.
                    let elem_layout = jr_pool::layout_of(self.pool, self.target, elem)
                        .map_err(|e| VmError::internal(format!("array element: {e}")))?;
                    let stride = elem_layout.size.next_multiple_of(elem_layout.align.into());
                    steps.push(PlaceStep::ScaledIndex {
                        index: *index,
                        stride,
                    });
                    ty = elem;
                }
                Projection::Deref => {
                    steps.push(PlaceStep::Indirect {
                        size: u64::from(self.target.pointer_size),
                    });
                    ty = self.pointee(ty)?;
                }
                // ADR-0004 stops being prose here: the two pseudo-fields `jr-sema`
                // hardcodes become the offsets `jr-pool` computes.
                Projection::StringData => {
                    let (offset, _) = string_data(self.target);
                    if offset != 0 {
                        steps.push(PlaceStep::Offset(offset));
                    }
                    ty = PoolId::PTR_U8;
                }
                Projection::StringCount => {
                    let (offset, _) = string_count(self.target);
                    if offset != 0 {
                        steps.push(PlaceStep::Offset(offset));
                    }
                    ty = PoolId::S64;
                }
                // A view's two words are at the same offsets a string's are — one shared
                // computation, so the layouts cannot drift (ADR-0044 §1). What differs is the
                // *type* the step lands on: `*T` rather than `*u8`, which is what makes
                // indexing the view use the right stride.
                Projection::ViewData => {
                    let Item::ViewType { elem } = self.pool.item(ty) else {
                        return Err(VmError::internal("a view projection of a non-view"));
                    };
                    let elem = *elem;
                    let (offset, _) = jr_pool::pair_data(self.target);
                    if offset != 0 {
                        steps.push(PlaceStep::Offset(offset));
                    }
                    ty = self.pool.find(&Item::PointerType(elem)).ok_or_else(|| {
                        VmError::internal("a view's element pointer type was never interned")
                    })?;
                }
                Projection::ViewCount => {
                    let (offset, _) = jr_pool::pair_count(self.target);
                    if offset != 0 {
                        steps.push(PlaceStep::Offset(offset));
                    }
                    ty = PoolId::S64;
                }
                // A dynamic array's three projections (ADR-0136 §1). `.data` at pair_data;
                // `.count` at pair_count; `.capacity` at triple_capacity.
                Projection::DynamicArrayData => {
                    let Item::DynamicArrayType { elem } = self.pool.item(ty) else {
                        return Err(VmError::internal(
                            "a `[..]T` data projection of a non-`[..]T`",
                        ));
                    };
                    let elem = *elem;
                    let (offset, _) = jr_pool::pair_data(self.target);
                    if offset != 0 {
                        steps.push(PlaceStep::Offset(offset));
                    }
                    ty = self.pool.find(&Item::PointerType(elem)).ok_or_else(|| {
                        VmError::internal("a `[..]T`'s element pointer type was never interned")
                    })?;
                }
                Projection::DynamicArrayCount => {
                    let (offset, _) = jr_pool::pair_count(self.target);
                    if offset != 0 {
                        steps.push(PlaceStep::Offset(offset));
                    }
                    ty = PoolId::S64;
                }
                Projection::DynamicArrayCapacity => {
                    let (offset, _) = jr_pool::triple_capacity(self.target);
                    if offset != 0 {
                        steps.push(PlaceStep::Offset(offset));
                    }
                    ty = PoolId::S64;
                }
                // A variant's tag is at offset 0 — it is the *leading* field (ADR-0068 §3), which is
                // why nothing has to be computed here. The type becomes `u8`, so a load through this
                // reads one byte rather than a case's width.
                Projection::VariantTag => {
                    ty = PoolId::U8;
                }
            }
        }

        let layout = layout_of(self.pool, self.target, ty)
            .map_err(|e| VmError::internal(format!("place type: {e:?}")))?;
        Ok(PlacePlan {
            base: root,
            steps,
            size: layout.size,
            shape: shape_of(self.pool, ty),
        })
    }

    fn field_type(&self, ty: PoolId, index: u32) -> Result<PoolId, VmError> {
        // A results aggregate's element list *is* its field list (ADR-0052 §1), so it is read
        // directly rather than through the struct side table — there is no `DeclId` to key one on.
        // The context's fields are the compiler's list (ADR-0057 §1) — the **third** consumer of
        // "what type is field N", after `jr-pool`'s `field_offset` and `jr-codegen-clif`'s. ADR-0052
        // recorded that duplication as owed and this wave adds a fourth aggregate kind to all three,
        // which is the cost it predicted.
        if matches!(self.pool.item(ty), Item::ContextType) {
            return jr_pool::Pool::context_field_type(index)
                .ok_or_else(|| VmError::internal(format!("no context field {index}")));
        }
        if let Item::ResultsType { elems } = self.pool.item(ty) {
            return elems
                .get(index as usize)
                .copied()
                .ok_or_else(|| VmError::internal(format!("no result {index}")));
        }
        // A union's field list is a struct's, so this accepts both; only `field_offset`
        // distinguishes them, and that is `jr-pool`'s (ADR-0045 §5).
        let (Item::StructType { .. } | Item::UnionType { .. } | Item::VariantType { .. }) =
            self.pool.item(ty)
        else {
            return Err(VmError::internal("a field of a non-aggregate"));
        };
        // By the *instance*, so a parameterised `Box(s64)`'s field is `s64` (ADR-0085 §2).
        self.pool
            .fields_of(ty)
            .and_then(|fields| fields.get(index as usize))
            .map(|field| field.ty)
            .ok_or_else(|| VmError::internal(format!("no field {index}")))
    }
}

/// What reading a value of `ty` produces.
///
/// A free function rather than a [`Compiler`] method, because [`crate::interp::Vm`] needs the
/// same classification to lay out a global's initial value before any body exists to lower — the
/// bytecode compiler and the interpreter must agree byte for byte about what is a register value
/// and what lives in memory, and a second copy of this match is exactly the kind of drift that
/// would let them disagree silently.
///
/// Matched exhaustively so that a new [`Item`] is a compile error here rather
/// than silently classified as an aggregate — which would read the wrong number
/// of bytes rather than failing.
pub(crate) fn shape_of(pool: &Pool, ty: PoolId) -> Shape {
    match pool.item(ty) {
        Item::VoidType => Shape::Void,
        // A float is a scalar: fixed, small, register-sized. Which *interpretation* its
        // bits carry comes from the type, which every consumer already has.
        // An enum is its backing integer at run time (ADR-0041 §3).
        Item::BoolType
        | Item::IntType { .. }
        | Item::FloatType { .. }
        | Item::EnumType { .. }
        | Item::PointerType(_)
        | Item::ProcType { .. } => Shape::Scalar,
        // A view is two words, so it reads as an aggregate — the same classification
        // `StringType` gets, and for the same reason (ADR-0044 §1).
        Item::StringType
        // A compiler-emitted table is held as the view it materialises to — two words, so an
        // aggregate, exactly as a `string` constant is (ADR-0152 §1).
        | Item::StaticArray { .. }
        | Item::ArrayType { .. }
        // **A vector reads as an aggregate**, which is the whole shape of ADR-0148 §4: the VM's
        // `Value` is one scalar, so sixteen bytes live in memory and an elementwise operation is
        // a loop over them. That is deliberately a *different number of operations* from the one
        // instruction the native engines emit, and it is what the three-way differential is
        // there to hold together.
        | Item::VectorType { .. }
        | Item::ViewType { .. }
        | Item::DynamicArrayType { .. }
        | Item::StructType { .. }
        | Item::UnionType { .. }
        | Item::VariantType { .. }
        // A results aggregate is bytes laid out like a struct's (ADR-0052 §1), so it reads as
        // one. Classifying it as a scalar would read one word where several live — a wrong
        // number of bytes rather than a failure, which is what this match is exhaustive to
        // prevent.
        | Item::ResultsType { .. }
        // A context is an aggregate: its fields live in memory and it is reached through a
        // pointer (ADR-0057 §2).
        | Item::ContextType
        | Item::TypeType
        | Item::ErrorType
        | Item::ForeignLibraryType
        | Item::VoidValue
        | Item::BoolValue(_)
        | Item::IntValue { .. }
        | Item::FloatValue { .. }
        | Item::StrValue(_)
        | Item::TypeValue(_)
        | Item::ProcValue { .. }
        | Item::ForeignLibraryValue(_, _)
        // A value reaching a *type* classifier is already a compiler fault, and this arm's
        // conservative answer is the safe one: an aggregate is read by size, so a wrong
        // classification here reads too few bytes rather than too many (ADR-0074 §1).
        | Item::AggregateValue { .. } => Shape::Aggregate,
    }
}

/// Whether a callee is a procedure in the same file as `body`.
///
/// Not used by lowering — a `ProcRef` is resolved by the interpreter, which has the
/// whole program — but exposed because a dump wants to know, for the same reason
/// `jr-mir`'s dump does.
#[must_use]
pub fn is_local_call(body: &MirBody, callee: &Callee) -> bool {
    match callee {
        Callee::Direct(target) => target.file == body.file(),
        Callee::Indirect(_) => false,
    }
}

/// The span a statement's instructions belong to.
///
/// Every [`Statement`] variant except [`Statement::Nop`] carries one; a `Nop` emits
/// nothing, so its span is never read.
fn statement_span(stmt: &Statement) -> MirSpan {
    match stmt {
        Statement::Assign { span, .. }
        | Statement::Store { span, .. }
        | Statement::Discard { span, .. }
        | Statement::Zero { span, .. }
        | Statement::BoundsCheck { span, .. }
        | Statement::TagCheck { span, .. } => *span,
        Statement::Nop => MirSpan::Synthetic,
    }
}
