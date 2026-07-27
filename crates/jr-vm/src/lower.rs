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

use jr_mir::{
    Callee, MirBody, MirSpan, Place, PlaceBase, Projection, Rvalue, Statement, Target, Terminator,
};
use jr_pool::{
    Item, Pool, PoolId, TargetLayout, field_offset, layout_of, string_count, string_data,
};

use crate::code::{
    BlockAddresses, Code, Instr, Operand, PlacePlan, PlaceRoot, PlaceStep, Reg, Shape, SlotPlan,
};
use crate::error::VmError;

/// Compiles one MIR body to bytecode.
///
/// # Errors
/// [`VmError::Internal`] when MIR, the pool and the target layout disagree — an
/// unresolved struct, a projection of a non-struct, a pointer dereference of a
/// non-pointer. `jr-mir`'s verifier is meant to make all of these unreachable, and
/// returning rather than panicking is what makes a hole in it diagnosable.
pub fn compile(body: &MirBody, pool: &Pool, target: TargetLayout) -> Result<Code, VmError> {
    Compiler {
        body,
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
    pool: &'a Pool,
    target: TargetLayout,
    instrs: Vec<Instr>,
    addresses: BlockAddresses,
    fixups: Vec<(usize, Hole, jr_mir::BlockId)>,
    /// The type of every register, growing as temporaries are added.
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
            Rvalue::Call { .. } | Rvalue::Load(_) | Rvalue::Address(_) | Rvalue::Undef => {
                PoolId::VOID
            }
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

    /// Resolves a MIR place into a plan with every offset computed.
    fn plan(&mut self, place: &Place) -> Result<PlacePlan, VmError> {
        let (root, mut ty) = match &place.base {
            PlaceBase::Slot(slot) => (PlaceRoot::Slot(slot.index()), self.body.slot(*slot).ty),
            PlaceBase::Deref(operand) => {
                let pointer_ty = self.operand_type(*operand);
                (PlaceRoot::Address(*operand), self.pointee(pointer_ty)?)
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
            }
        }

        let layout = layout_of(self.pool, self.target, ty)
            .map_err(|e| VmError::internal(format!("place type: {e:?}")))?;
        Ok(PlacePlan {
            base: root,
            steps,
            size: layout.size,
            shape: self.shape(ty),
        })
    }

    fn field_type(&self, ty: PoolId, index: u32) -> Result<PoolId, VmError> {
        let Item::StructType { decl } = self.pool.item(ty) else {
            return Err(VmError::internal("a field of a non-struct"));
        };
        self.pool
            .struct_fields(*decl)
            .and_then(|fields| fields.get(index as usize))
            .map(|field| field.ty)
            .ok_or_else(|| VmError::internal(format!("no field {index}")))
    }

    /// What reading a value of `ty` produces.
    ///
    /// Matched exhaustively so that a new [`Item`] is a compile error here rather
    /// than silently classified as an aggregate — which would read the wrong number
    /// of bytes rather than failing.
    fn shape(&self, ty: PoolId) -> Shape {
        match self.pool.item(ty) {
            Item::VoidType => Shape::Void,
            Item::BoolType
            | Item::IntType { .. }
            | Item::PointerType(_)
            | Item::ProcType { .. } => Shape::Scalar,
            Item::StringType
            | Item::StructType { .. }
            | Item::TypeType
            | Item::ErrorType
            | Item::ForeignLibraryType
            | Item::VoidValue
            | Item::BoolValue(_)
            | Item::IntValue { .. }
            | Item::StrValue(_)
            | Item::TypeValue(_)
            | Item::ProcValue { .. }
            | Item::ForeignLibraryValue(_) => Shape::Aggregate,
        }
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
        | Statement::Discard { span, .. } => *span,
        Statement::Nop => MirSpan::Synthetic,
    }
}
