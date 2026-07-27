//! One MIR body, translated into Cranelift IR.
//!
//! # What ADR-0017 already paid for
//!
//! Three of the things a back end normally has to build do not exist here, and that
//! is the return on decisions taken two waves earlier:
//!
//! - **No unphi pass.** MIR uses block *parameters*, so a merge is
//!   [`FunctionBuilder::append_block_param`] and an edge that carries values is a
//!   `jump`/`brif` with arguments. ADR-0017 §1 chose parameters over phi statements
//!   for exactly this, and forbade critical edges so an edge's copies have one
//!   unambiguous home.
//! - **No `mem2reg`.** SSA was built during lowering by Braun's algorithm, so
//!   every [`ValueId`] is already a value and only genuinely escaping locals are in
//!   slots.
//! - **No block ordering pass.** [`MirBody::reverse_postorder`] is the order, and
//!   unreachable blocks are simply never emitted.
//!
//! # Where the bytes come from
//!
//! [`crate::repr`], which asks [`jr_pool`]. Nothing in this file adds an offset to
//! another offset without having been given both by the pool. ADR-0018 §2 and
//! ADR-0019 both state the prohibition; the reason it is worth restating in three
//! places is that violating it is *silent*.
//!
//! # Why `Rvalue::Undef` is tracked rather than materialised
//!
//! An undefined value must not become a zero: that is precisely what would hide the
//! bug E0227 reports. But it must not trap at its *definition* either, because the
//! VM traps on **use** and a `Move` of an undefined value is legal there — so a
//! local that is declared and never read runs fine in the VM, and a back end that
//! trapped where it was defined would disagree about a valid program.
//!
//! So undefinedness is tracked as a property of a [`ValueId`], propagated through a
//! plain `Use`, and turned into a trap at each site that genuinely *reads* the
//! value. That reproduces `Value::Undefined`'s behaviour — `scalar()` and
//! `aggregate()` trap, `Move` clones — without needing a poison value Cranelift
//! does not have.

use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{
    Block, BlockArg, FuncRef, InstBuilder, MemFlagsData, StackSlotData, StackSlotKind, TrapCode,
    Type, Value as ClifValue,
};
use cranelift_frontend::FunctionBuilder;
use cranelift_module::{DataDescription, DataId, Linkage, Module};
use jr_codegen::{CodegenError, TrapLocations};
use jr_mir::{
    BinOp, BlockId, Callee, MirBody, MirSpan, Operand, Place, PlaceBase, ProcRef, Projection,
    Rvalue, Statement, Target, Terminator, UnOp, Unreachable, ValueId,
};
use jr_pool::{
    Item, Pool, PoolId, TargetLayout, field_offset, layout_of, string_count, string_data,
};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::repr::{Repr, pointer_type};
use crate::trap::TrapKind;

/// What a translated MIR value is, once `void` is accounted for.
///
/// `void` occupies no register, so it is an absence rather than a zero — the same
/// distinction `jr-vm`'s `Shape` draws, and for the same reason (ADR-0015 §3).
type Slot = Option<ClifValue>;

/// Everything the translator needs that outlives one body.
pub struct Context<'a> {
    /// The interned types and struct fields every layout question is asked of.
    pub pool: &'a Pool,
    /// The target's pointer width, passed to `jr-pool` rather than assumed.
    pub target: TargetLayout,
    /// The Cranelift function reference for every declared procedure.
    pub funcs: &'a FxHashMap<ProcRef, FuncRef>,
    /// The data object holding each string constant's bytes, keyed by the pool's
    /// own `StrId` so that deduplication matches the VM's `intern_strings`.
    pub strings: &'a FxHashMap<jr_pool::StrId, DataId>,
    /// The runtime helper a trap calls, `jr_trap(message, length)`.
    pub trap_helper: FuncRef,
    /// How to render a trap's source location (ADR-0020 §3).
    pub locations: &'a dyn TrapLocations,
}

/// Translates one body into the function `builder` is building.
///
/// # Errors
/// [`CodegenError`] when a type has no layout, a callee was never declared, or MIR
/// contains a construct this back end does not implement yet.
pub fn translate(
    builder: &mut FunctionBuilder<'_>,
    module: &mut dyn Module,
    ctx: &Context<'_>,
    proc: ProcRef,
    body: &MirBody,
) -> Result<(), CodegenError> {
    let mut translator = Translator {
        builder,
        module,
        ctx,
        proc,
        body,
        blocks: FxHashMap::default(),
        values: FxHashMap::default(),
        undef: FxHashSet::default(),
        slots: Vec::new(),
        current: MirSpan::Synthetic,
        messages: FxHashMap::default(),
    };
    translator.run()
}

/// The per-body translation state.
struct Translator<'a, 'b> {
    builder: &'a mut FunctionBuilder<'b>,
    module: &'a mut dyn Module,
    ctx: &'a Context<'a>,
    proc: ProcRef,
    body: &'a MirBody,
    blocks: FxHashMap<BlockId, Block>,
    values: FxHashMap<ValueId, Slot>,
    undef: FxHashSet<ValueId>,
    slots: Vec<cranelift_codegen::ir::StackSlot>,
    /// The span instructions being emitted right now belong to.
    ///
    /// Set once per statement and per terminator, mirroring `jr-vm`'s lowering, so
    /// that a trap emitted anywhere beneath reports the construct that caused it
    /// without every helper having to thread a span through.
    current: MirSpan,
    /// Data objects for messages already emitted, keyed by their bytes.
    messages: FxHashMap<String, DataId>,
}

impl Translator<'_, '_> {
    /// Translates the whole body.
    fn run(&mut self) -> Result<(), CodegenError> {
        self.declare_slots()?;

        let order: Vec<BlockId> = self.body.reverse_postorder().to_vec();
        for id in &order {
            let block = self.builder.create_block();
            self.blocks.insert(*id, block);
        }

        // The entry block's parameters are the procedure's, so they come from the
        // signature rather than from `append_block_param`.
        let entry = self.block(self.body.entry())?;
        self.builder.append_block_params_for_function_params(entry);
        self.bind_entry_params(entry)?;

        // Every other block's parameters are MIR's own, which map one-for-one onto
        // Cranelift's — ADR-0017 §1's whole point.
        for id in &order {
            if *id == self.body.entry() {
                continue;
            }
            let block = self.block(*id)?;
            let params = self.body.block(*id).params.clone();
            for value in params {
                let ty = self.body.value(value).ty;
                match Repr::of(self.ctx.pool, self.ctx.target, ty)?.clif_type(self.ctx.target) {
                    Some(clif) => {
                        let param = self.builder.append_block_param(block, clif);
                        self.values.insert(value, Some(param));
                    }
                    // A `void` parameter carries nothing, so it gets no Cranelift
                    // parameter and the edge that feeds it passes no argument.
                    None => {
                        self.values.insert(value, None);
                    }
                }
            }
        }

        for id in &order {
            let block = self.block(*id)?;
            self.builder.switch_to_block(block);
            let data = self.body.block(*id);
            for stmt in &data.stmts {
                self.current = statement_span(stmt);
                self.statement(stmt)?;
            }
            self.current = self.terminator_span(&data.term);
            self.terminator(&data.term)?;
        }

        self.builder.seal_all_blocks();
        Ok(())
    }

    /// Creates a Cranelift stack slot for every MIR slot.
    ///
    /// Sizes and alignments come from [`jr_pool::layout_of`]. ADR-0017 §2 put
    /// escaped locals in slots during lowering precisely so that this is a
    /// mechanical mapping.
    fn declare_slots(&mut self) -> Result<(), CodegenError> {
        for index in 0..self.body.slot_count() {
            let slot = self.body.slot(jr_mir::SlotId::from_usize(index));
            let layout = layout_of(self.ctx.pool, self.ctx.target, slot.ty).map_err(|reason| {
                CodegenError::NoLayout {
                    ty: slot.ty,
                    reason,
                }
            })?;
            // A zero-sized slot is legal — `void` is storable — but Cranelift wants
            // a non-zero size, and one byte is the smallest honest request.
            let size = u32::try_from(layout.size.max(1)).map_err(|_| {
                CodegenError::Internal(format!("slot {index} is larger than a u32"))
            })?;
            let data = StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                size,
                layout.align.trailing_zeros().try_into().unwrap_or(0),
            );
            let handle = self.builder.create_sized_stack_slot(data);
            self.slots.push(handle);
        }
        Ok(())
    }

    /// Binds the entry block's Cranelift parameters to MIR's parameter values.
    ///
    /// A `void` parameter contributes no Cranelift parameter, so the two lists are
    /// walked with independent cursors rather than zipped.
    fn bind_entry_params(&mut self, entry: Block) -> Result<(), CodegenError> {
        let clif: Vec<ClifValue> = self.builder.block_params(entry).to_vec();
        let mut next = 0usize;
        for value in self.body.params() {
            let ty = self.body.value(*value).ty;
            let repr = Repr::of(self.ctx.pool, self.ctx.target, ty)?;
            if repr.clif_type(self.ctx.target).is_some() {
                let param = clif.get(next).copied().ok_or_else(|| {
                    CodegenError::Internal(
                        "the signature has fewer parameters than the body".to_owned(),
                    )
                })?;
                next += 1;
                self.values.insert(*value, Some(param));
            } else {
                self.values.insert(*value, None);
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Statements
    // -----------------------------------------------------------------------

    /// Translates one statement.
    fn statement(&mut self, stmt: &Statement) -> Result<(), CodegenError> {
        match stmt {
            Statement::Assign { dest, rvalue, .. } => {
                let value = self.rvalue(rvalue, Some(*dest))?;
                self.values.insert(*dest, value);
                Ok(())
            }
            Statement::Store { place, value, .. } => {
                let ty = self.place_type(place)?;
                let repr = Repr::of(self.ctx.pool, self.ctx.target, ty)?;
                // The VM evaluates the value operand *before* the address, so a
                // trapping operand surfaces first; the order is preserved here so
                // the two report the same failure.
                let source = self.read(*value)?;
                let address = self.address(place)?;
                self.write(address, repr, source)
            }
            // A discarded rvalue is still evaluated, deliberately: an ADR-0002
            // overflow in an expression whose result nobody wants still traps.
            Statement::Discard { rvalue, .. } => {
                self.rvalue(rvalue, None)?;
                Ok(())
            }
            Statement::Nop => Ok(()),
        }
    }

    /// Translates an rvalue, returning the value it produces.
    fn rvalue(&mut self, rvalue: &Rvalue, dest: Option<ValueId>) -> Result<Slot, CodegenError> {
        match rvalue {
            // A plain move propagates undefinedness rather than trapping, exactly as
            // the VM's `Move` clones `Value::Undefined` without inspecting it.
            Rvalue::Use(operand) => {
                if let (Operand::Value(source), Some(dest)) = (operand, dest)
                    && self.undef.contains(source)
                {
                    self.undef.insert(dest);
                }
                self.operand(*operand)
            }
            Rvalue::Binary { op, lhs, rhs } => self.binary(*op, *lhs, *rhs),
            Rvalue::Unary { op, operand } => self.unary(*op, *operand),
            Rvalue::Call { callee, args } => self.call(callee, args),
            Rvalue::Load(place) => self.load(place),
            Rvalue::Address(place) => {
                let address = self.address(place)?;
                Ok(Some(address))
            }
            Rvalue::Undef => {
                if let Some(dest) = dest {
                    self.undef.insert(dest);
                }
                // A placeholder is still needed so the SSA value exists; it is never
                // read, because every reading site checks `undef` first.
                let ty = dest.map_or(PoolId::VOID, |id| self.body.value(id).ty);
                let repr = Repr::of(self.ctx.pool, self.ctx.target, ty)?;
                Ok(repr
                    .clif_type(self.ctx.target)
                    .map(|clif| self.builder.ins().iconst(clif, 0)))
            }
        }
    }

    // -----------------------------------------------------------------------
    // Operands
    // -----------------------------------------------------------------------

    /// The value of an operand, without checking for undefinedness.
    fn operand(&mut self, operand: Operand) -> Result<Slot, CodegenError> {
        match operand {
            Operand::Value(value) => self.values.get(&value).copied().ok_or_else(|| {
                CodegenError::Internal(format!(
                    "value v{} used before it was defined",
                    value.index()
                ))
            }),
            Operand::Constant(id) => self.constant(id),
        }
    }

    /// The value of an operand that is genuinely *read*.
    ///
    /// Traps first if it is undefined, which is what `Value::scalar` and
    /// `Value::aggregate` do in the VM.
    fn read(&mut self, operand: Operand) -> Result<Slot, CodegenError> {
        if let Operand::Value(value) = operand
            && self.undef.contains(&value)
        {
            self.trap(TrapKind::UninitialisedRead)?;
        }
        self.operand(operand)
    }

    /// A scalar operand that is read, as a single Cranelift value.
    fn read_scalar(&mut self, operand: Operand) -> Result<ClifValue, CodegenError> {
        self.read(operand)?.ok_or_else(|| {
            CodegenError::Internal("expected a scalar operand, found void".to_owned())
        })
    }

    /// Materialises an interned constant.
    ///
    /// Integers are masked to their own width by construction, because the constant
    /// is emitted at that width — the same normalisation `jr-vm` applies with
    /// `IntKind::mask`.
    fn constant(&mut self, id: PoolId) -> Result<Slot, CodegenError> {
        // Cloned rather than borrowed because materialising a constant may need
        // `&mut self.builder`, and the pool borrow would outlive that.
        let item = self.ctx.pool.item(id).clone();
        match item {
            Item::VoidValue => Ok(None),
            Item::BoolValue(value) => {
                let clif = self
                    .builder
                    .ins()
                    .iconst(cranelift_codegen::ir::types::I8, i64::from(value));
                Ok(Some(clif))
            }
            Item::IntValue { ty, bits } => {
                let repr = Repr::of(self.ctx.pool, self.ctx.target, ty)?;
                let clif = repr.clif_type(self.ctx.target).ok_or_else(|| {
                    CodegenError::Internal("an integer constant of type void".to_owned())
                })?;
                // `bits` is already normalised to the type's width; `iconst` takes a
                // sign-agnostic bit pattern, so it is passed through unchanged.
                Ok(Some(self.builder.ins().iconst(clif, bits as i64)))
            }
            Item::StrValue(str_id) => self.string_constant(str_id).map(Some),
            _ => Err(CodegenError::Unsupported {
                proc: self.proc,
                what: "a type, procedure or library used as a runtime value".to_owned(),
            }),
        }
    }

    /// Builds a `{data, count}` pair for a string literal (ADR-0004).
    ///
    /// The bytes live in a read-only data object, deduplicated by `StrId` exactly as
    /// the VM's `intern_strings` deduplicates them, and the pair itself is
    /// materialised into a stack slot whose address is the aggregate's value. Both
    /// field offsets come from [`jr_pool::string_data`] and
    /// [`jr_pool::string_count`] — ADR-0004 stops being prose in the same place it
    /// does for the VM.
    fn string_constant(&mut self, str_id: jr_pool::StrId) -> Result<ClifValue, CodegenError> {
        let data = *self.ctx.strings.get(&str_id).ok_or_else(|| {
            CodegenError::Internal("a string constant was not given a data object".to_owned())
        })?;
        let count = self.ctx.pool.resolve_str(str_id).len() as i64;

        let layout = jr_pool::string_layout(self.ctx.target);
        let (data_offset, data_layout) = string_data(self.ctx.target);
        let (count_offset, count_layout) = string_count(self.ctx.target);

        let size = u32::try_from(layout.size)
            .map_err(|_| CodegenError::Internal("a string larger than a u32".to_owned()))?;
        let slot = self.builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            size,
            layout.align.trailing_zeros().try_into().unwrap_or(0),
        ));

        let pointer = pointer_type(self.ctx.target);
        let global = self.module.declare_data_in_func(data, self.builder.func);
        let address = self.builder.ins().symbol_value(pointer, global);
        let base = self.builder.ins().stack_addr(pointer, slot, 0);

        let data_ty = int_of_size(data_layout.size, pointer);
        let count_ty = int_of_size(count_layout.size, pointer);
        let flags = MemFlagsData::new();
        self.builder.ins().store(
            flags,
            address,
            base,
            i32::try_from(data_offset).unwrap_or(0),
        );
        let count_value = self.builder.ins().iconst(count_ty, count);
        self.builder.ins().store(
            flags,
            count_value,
            base,
            i32::try_from(count_offset).unwrap_or(0),
        );
        debug_assert_eq!(data_ty, pointer, "a string's data field is a pointer");
        Ok(base)
    }

    // -----------------------------------------------------------------------
    // Arithmetic
    // -----------------------------------------------------------------------

    /// Translates a binary operation, trapping per ADR-0002 where required.
    fn binary(&mut self, op: BinOp, lhs: Operand, rhs: Operand) -> Result<Slot, CodegenError> {
        let left = self.read_scalar(lhs)?;
        let right = self.read_scalar(rhs)?;
        let ty = self.operand_type(lhs);
        let signed = matches!(
            Repr::of(self.ctx.pool, self.ctx.target, ty)?,
            Repr::Scalar { signed: true, .. }
        );

        let value = match op {
            // ADR-0002: `+`, `-`, `*` trap rather than wrap. Cranelift's overflow
            // instructions are defined on the *operand width*, which is why
            // `repr` gives each integer type its own width: an `I8` add overflows
            // where an 8-bit add overflows, so a narrow type traps at its own
            // boundary and not at `s64`'s.
            BinOp::Add => self.checked(signed, left, right, Arith::Add)?,
            BinOp::Sub => self.checked(signed, left, right, Arith::Sub)?,
            BinOp::Mul => self.checked(signed, left, right, Arith::Mul)?,
            // The documented opt-out.
            BinOp::WrapAdd => self.builder.ins().iadd(left, right),
            BinOp::WrapSub => self.builder.ins().isub(left, right),
            BinOp::WrapMul => self.builder.ins().imul(left, right),
            BinOp::Div | BinOp::Rem => self.division(op, signed, left, right)?,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let cc = condition(op, signed);
                let bit = self.builder.ins().icmp(cc, left, right);
                // A comparison's result is `bool`, which is one byte wide, so the
                // `I8` Cranelift produces for `icmp` is already the right width.
                bit
            }
        };
        Ok(Some(value))
    }

    /// Emits a checked add, subtract or multiply.
    fn checked(
        &mut self,
        signed: bool,
        left: ClifValue,
        right: ClifValue,
        op: Arith,
    ) -> Result<ClifValue, CodegenError> {
        let (value, overflow) = match (op, signed) {
            (Arith::Add, true) => self.builder.ins().sadd_overflow(left, right),
            (Arith::Add, false) => self.builder.ins().uadd_overflow(left, right),
            (Arith::Sub, true) => self.builder.ins().ssub_overflow(left, right),
            (Arith::Sub, false) => self.builder.ins().usub_overflow(left, right),
            (Arith::Mul, true) => self.builder.ins().smul_overflow(left, right),
            (Arith::Mul, false) => self.builder.ins().umul_overflow(left, right),
        };
        self.trap_if(overflow, op.trap_kind())?;
        Ok(value)
    }

    /// Emits a division or remainder with its two ADR-0002 checks.
    fn division(
        &mut self,
        op: BinOp,
        signed: bool,
        left: ClifValue,
        right: ClifValue,
    ) -> Result<ClifValue, CodegenError> {
        let zero = self.builder.ins().icmp_imm_s(IntCC::Equal, right, 0);
        self.trap_if(zero, TrapKind::DivideByZero)?;

        if signed {
            // `MIN / -1` overflows rather than dividing: the VM does the arithmetic
            // in `i128` and its ordinary range check catches this for free, so the
            // check is explicit here to match.
            let ty = self.builder.func.dfg.value_type(left);
            let min = min_of(ty);
            let is_min = self.builder.ins().icmp_imm_s(IntCC::Equal, left, min);
            let is_minus_one = self.builder.ins().icmp_imm_s(IntCC::Equal, right, -1);
            let both = self.builder.ins().band(is_min, is_minus_one);
            let kind = if matches!(op, BinOp::Rem) {
                TrapKind::OverflowRem
            } else {
                TrapKind::OverflowDiv
            };
            self.trap_if(both, kind)?;
        }

        Ok(match (op, signed) {
            (BinOp::Div, true) => self.builder.ins().sdiv(left, right),
            (BinOp::Div, false) => self.builder.ins().udiv(left, right),
            (BinOp::Rem, true) => self.builder.ins().srem(left, right),
            (BinOp::Rem, false) => self.builder.ins().urem(left, right),
            _ => {
                return Err(CodegenError::Internal(
                    "division called for a non-division operator".to_owned(),
                ));
            }
        })
    }

    /// Translates a unary operation.
    fn unary(&mut self, op: UnOp, operand: Operand) -> Result<Slot, CodegenError> {
        let value = self.read_scalar(operand)?;
        let ty = self.operand_type(operand);
        let signed = matches!(
            Repr::of(self.ctx.pool, self.ctx.target, ty)?,
            Repr::Scalar { signed: true, .. }
        );
        let result = match op {
            UnOp::Neg => {
                if signed {
                    // Negating the most negative value overflows (ADR-0002).
                    let clif = self.builder.func.dfg.value_type(value);
                    let is_min = self
                        .builder
                        .ins()
                        .icmp_imm_s(IntCC::Equal, value, min_of(clif));
                    self.trap_if(is_min, TrapKind::OverflowNeg)?;
                }
                self.builder.ins().ineg(value)
            }
            // `bool` is stored as 0 or 1, so `!` is a comparison against zero
            // rather than a bitwise complement, which would produce 0xFE.
            UnOp::Not => self.builder.ins().icmp_imm_s(IntCC::Equal, value, 0),
        };
        Ok(Some(result))
    }

    // -----------------------------------------------------------------------
    // Calls
    // -----------------------------------------------------------------------

    /// Translates a call.
    fn call(&mut self, callee: &Callee, args: &[Operand]) -> Result<Slot, CodegenError> {
        let target = match callee {
            Callee::Direct(proc) => *proc,
            // Nothing maps a procedure value to a `ProcRef` yet, which is the same
            // gap the VM reports; refusing names it rather than miscompiling it.
            Callee::Indirect(_) => {
                return Err(CodegenError::Unsupported {
                    proc: self.proc,
                    what: "a call through a procedure pointer".to_owned(),
                });
            }
        };
        let func = *self
            .ctx
            .funcs
            .get(&target)
            .ok_or(CodegenError::Undeclared(target))?;

        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            if let Some(value) = self.read(*arg)? {
                values.push(value);
            }
        }
        let inst = self.builder.ins().call(func, &values);
        let results = self.builder.inst_results(inst);
        Ok(results.first().copied())
    }

    // -----------------------------------------------------------------------
    // Places
    // -----------------------------------------------------------------------

    /// Computes a place's address.
    ///
    /// Every offset is asked of [`jr_pool`]. The two `Deref`s are deliberately
    /// different, and getting them the same way round is the easiest mistake here: a
    /// [`PlaceBase::Deref`] reads its pointer out of a **register**, while a
    /// [`Projection::Deref`] reads one out of **memory**.
    fn address(&mut self, place: &Place) -> Result<ClifValue, CodegenError> {
        let pointer = pointer_type(self.ctx.target);
        let (mut address, mut ty) = match &place.base {
            PlaceBase::Slot(slot) => {
                let handle = *self
                    .slots
                    .get(slot.index())
                    .ok_or_else(|| CodegenError::Internal(format!("no slot s{}", slot.index())))?;
                (
                    self.builder.ins().stack_addr(pointer, handle, 0),
                    self.body.slot(*slot).ty,
                )
            }
            PlaceBase::Deref(operand) => {
                let value = self.read_scalar(*operand)?;
                let pointee = self.pointee(self.operand_type(*operand))?;
                (value, pointee)
            }
        };

        for step in &place.projection {
            match step {
                Projection::Field(index) => {
                    let (offset, _) = field_offset(self.ctx.pool, self.ctx.target, ty, *index)
                        .map_err(|reason| CodegenError::NoLayout { ty, reason })?;
                    address = self.offset(address, offset);
                    ty = self.field_type(ty, *index)?;
                }
                Projection::Deref => {
                    address = self
                        .builder
                        .ins()
                        .load(pointer, MemFlagsData::new(), address, 0);
                    ty = self.pointee(ty)?;
                }
                Projection::StringData => {
                    let (offset, _) = string_data(self.ctx.target);
                    address = self.offset(address, offset);
                    ty = PoolId::PTR_U8;
                }
                Projection::StringCount => {
                    let (offset, _) = string_count(self.ctx.target);
                    address = self.offset(address, offset);
                    ty = PoolId::S64;
                }
            }
        }
        Ok(address)
    }

    /// Adds a byte offset to an address, skipping the instruction when it is zero.
    fn offset(&mut self, address: ClifValue, offset: u64) -> ClifValue {
        if offset == 0 {
            return address;
        }
        // A byte offset is unsigned by construction, but the immediate is an
        // `Imm64`; a struct larger than `i64::MAX` is not representable anyway.
        let offset = i64::try_from(offset).unwrap_or(i64::MAX);
        self.builder.ins().iadd_imm_s(address, offset)
    }

    /// The type a place denotes, after every projection.
    fn place_type(&mut self, place: &Place) -> Result<PoolId, CodegenError> {
        let mut ty = match &place.base {
            PlaceBase::Slot(slot) => self.body.slot(*slot).ty,
            PlaceBase::Deref(operand) => self.pointee(self.operand_type(*operand))?,
        };
        for step in &place.projection {
            ty = match step {
                Projection::Field(index) => self.field_type(ty, *index)?,
                Projection::Deref => self.pointee(ty)?,
                Projection::StringData => PoolId::PTR_U8,
                Projection::StringCount => PoolId::S64,
            };
        }
        Ok(ty)
    }

    /// Reads a place.
    ///
    /// An aggregate read is a byte copy into a fresh slot, matching the VM's
    /// `read(...).to_vec()`: the result is a value, so a later write through the
    /// original place must not be visible through it.
    fn load(&mut self, place: &Place) -> Result<Slot, CodegenError> {
        let ty = self.place_type(place)?;
        let repr = Repr::of(self.ctx.pool, self.ctx.target, ty)?;
        let address = self.address(place)?;
        match repr {
            Repr::Void => Ok(None),
            Repr::Scalar { ty: clif, .. } => Ok(Some(self.builder.ins().load(
                clif,
                MemFlagsData::new(),
                address,
                0,
            ))),
            Repr::Aggregate { size, align } => {
                let copy = self.alloc_aggregate(size, align)?;
                self.copy(copy, address, size, align);
                Ok(Some(copy))
            }
        }
    }

    /// Writes `source` into `address`.
    fn write(&mut self, address: ClifValue, repr: Repr, source: Slot) -> Result<(), CodegenError> {
        match repr {
            // A `void` store writes nothing and never touches the address, exactly
            // as the VM's `Shape::Void` arm returns without writing.
            Repr::Void => Ok(()),
            Repr::Scalar { .. } => {
                let value = source.ok_or_else(|| {
                    CodegenError::Internal("storing void into a scalar place".to_owned())
                })?;
                self.builder
                    .ins()
                    .store(MemFlagsData::new(), value, address, 0);
                Ok(())
            }
            Repr::Aggregate { size, align } => {
                let value = source.ok_or_else(|| {
                    CodegenError::Internal("storing void into an aggregate place".to_owned())
                })?;
                self.copy(address, value, size, align);
                Ok(())
            }
        }
    }

    /// Allocates a stack slot to hold an aggregate value.
    fn alloc_aggregate(&mut self, size: u64, align: u32) -> Result<ClifValue, CodegenError> {
        let bytes = u32::try_from(size.max(1))
            .map_err(|_| CodegenError::Internal("an aggregate larger than a u32".to_owned()))?;
        let slot = self.builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            bytes,
            align.trailing_zeros().try_into().unwrap_or(0),
        ));
        Ok(self
            .builder
            .ins()
            .stack_addr(pointer_type(self.ctx.target), slot, 0))
    }

    /// Copies `size` bytes, which is what an aggregate assignment is.
    fn copy(&mut self, dest: ClifValue, src: ClifValue, size: u64, align: u32) {
        if size == 0 {
            return;
        }
        let config = self.module.target_config();
        let align = u8::try_from(align).unwrap_or(1);
        self.builder.emit_small_memory_copy(
            config,
            dest,
            src,
            size,
            align,
            align,
            true,
            MemFlagsData::new(),
        );
    }

    // -----------------------------------------------------------------------
    // Terminators
    // -----------------------------------------------------------------------

    /// Translates a terminator.
    fn terminator(&mut self, term: &Terminator) -> Result<(), CodegenError> {
        match term {
            Terminator::Goto(target) => {
                let (block, args) = self.edge(target)?;
                self.builder.ins().jump(block, &args);
                Ok(())
            }
            Terminator::Branch { cond, then_, else_ } => {
                let value = self.read_scalar(*cond)?;
                let (then_block, then_args) = self.edge(then_)?;
                let (else_block, else_args) = self.edge(else_)?;
                self.builder
                    .ins()
                    .brif(value, then_block, &then_args, else_block, &else_args);
                Ok(())
            }
            Terminator::Return(operand) => {
                match operand {
                    Some(operand) => {
                        // The VM traps rather than returning an undefined value,
                        // because the callee's contract is to produce one.
                        match self.read(*operand)? {
                            Some(value) => {
                                self.builder.ins().return_(&[value]);
                            }
                            None => {
                                self.builder.ins().return_(&[]);
                            }
                        }
                    }
                    None => {
                        self.builder.ins().return_(&[]);
                    }
                }
                Ok(())
            }
            Terminator::Unreachable(reason) => {
                // Only `Trap` is a program the compiler believes well-formed; the
                // other two are statically reported (E0228, E0229) and reaching one
                // means the program was run without being checked.
                let kind = match reason {
                    Unreachable::Trap => TrapKind::Deliberate,
                    Unreachable::StrayJump => TrapKind::StrayJump,
                    Unreachable::FellOffEnd => TrapKind::FellOffEnd,
                };
                self.report(kind)?;
                self.builder.ins().trap(TrapCode::user(1).unwrap());
                Ok(())
            }
        }
    }

    /// The Cranelift block and arguments for an edge.
    ///
    /// A `void` block parameter takes no argument, so the argument list is filtered
    /// the same way the parameter list was.
    fn edge(&mut self, target: &Target) -> Result<(Block, Vec<BlockArg>), CodegenError> {
        let block = self.block(target.block)?;
        let mut args = Vec::with_capacity(target.args.len());
        for arg in &target.args {
            if let Some(value) = self.read(*arg)? {
                args.push(BlockArg::Value(value));
            }
        }
        Ok((block, args))
    }

    // -----------------------------------------------------------------------
    // Traps
    // -----------------------------------------------------------------------

    /// Traps when `cond` is non-zero, continuing otherwise.
    ///
    /// The shape is a compare-and-branch to a dedicated block that calls the runtime
    /// helper, per ADR-0019 §2. The branch is what keeps the fast path free of the
    /// call.
    fn trap_if(&mut self, cond: ClifValue, kind: TrapKind) -> Result<(), CodegenError> {
        let trap_block = self.builder.create_block();
        let continue_block = self.builder.create_block();
        self.builder
            .ins()
            .brif(cond, trap_block, &[], continue_block, &[]);

        self.builder.switch_to_block(trap_block);
        // Cold, because a trap block is by construction the path not taken.
        self.builder.set_cold_block(trap_block);
        self.report(kind)?;
        self.builder.ins().trap(TrapCode::user(1).unwrap());
        self.builder.seal_block(trap_block);

        self.builder.switch_to_block(continue_block);
        Ok(())
    }

    /// Traps unconditionally, then continues in an unreachable block.
    ///
    /// Used where the VM traps on a value rather than on a condition — reading an
    /// undefined value. The continuation exists only so that the statements MIR
    /// still lists after it remain translatable.
    fn trap(&mut self, kind: TrapKind) -> Result<(), CodegenError> {
        self.report(kind)?;
        self.builder.ins().trap(TrapCode::user(1).unwrap());
        let unreachable = self.builder.create_block();
        self.builder.switch_to_block(unreachable);
        self.builder.seal_block(unreachable);
        Ok(())
    }

    /// Calls the runtime helper that reports a trap and aborts.
    ///
    /// ADR-0019 §2 chose a call over a bare machine trap because it is the only
    /// lowering that can carry a *message*. ADR-0020 makes that message carry a
    /// source location too, which is why it is rendered here, per site, rather than
    /// once per [`TrapKind`]: two overflows on different lines say different things.
    ///
    /// The bytes are produced by `jr_base::trap_message`, the same function the VM
    /// calls — the two engines render at different *times* and must still agree
    /// exactly, and `differential.rs` compares them (ADR-0020 §2).
    fn report(&mut self, kind: TrapKind) -> Result<(), CodegenError> {
        let location = self.ctx.locations.location(self.current);
        let message = jr_base::trap_message(kind.reason(), location.as_deref());
        let data = self.message_data(&message)?;

        let pointer = pointer_type(self.ctx.target);
        let global = self.module.declare_data_in_func(data, self.builder.func);
        let text = self.builder.ins().symbol_value(pointer, global);
        let length = self
            .builder
            .ins()
            .iconst(pointer, i64::try_from(message.len()).unwrap_or(i64::MAX));
        self.builder
            .ins()
            .call(self.ctx.trap_helper, &[text, length]);
        Ok(())
    }

    /// The data object holding `message`, created on first use.
    ///
    /// Keyed by content, so two sites that genuinely render the same text — the same
    /// line reached twice, or two traps with no location — share one object. The
    /// symbol carries the procedure's identity so that two bodies cannot collide.
    fn message_data(&mut self, message: &str) -> Result<DataId, CodegenError> {
        if let Some(id) = self.messages.get(message) {
            return Ok(*id);
        }
        let symbol = format!(
            "jr$trap${}${}${}",
            self.proc.file.index(),
            self.proc.proc.index(),
            self.messages.len()
        );
        let id = self
            .module
            .declare_data(&symbol, Linkage::Local, false, false)
            .map_err(|e| CodegenError::Internal(format!("trap message: {e}")))?;
        let mut description = DataDescription::new();
        description.define(message.as_bytes().to_vec().into_boxed_slice());
        self.module
            .define_data(id, &description)
            .map_err(|e| CodegenError::Internal(format!("trap message: {e}")))?;
        self.messages.insert(message.to_owned(), id);
        Ok(id)
    }

    // -----------------------------------------------------------------------
    // Small helpers
    // -----------------------------------------------------------------------

    /// The Cranelift block for a MIR block.
    fn block(&self, id: BlockId) -> Result<Block, CodegenError> {
        self.blocks.get(&id).copied().ok_or_else(|| {
            CodegenError::Internal(format!("block bb{} is not in the block order", id.index()))
        })
    }

    /// The type an operand holds.
    fn operand_type(&self, operand: Operand) -> PoolId {
        match operand {
            Operand::Value(value) => self.body.value(value).ty,
            Operand::Constant(id) => self.ctx.pool.type_of(id),
        }
    }

    /// The type a pointer points at.
    fn pointee(&self, ty: PoolId) -> Result<PoolId, CodegenError> {
        match self.ctx.pool.item(ty) {
            Item::PointerType(pointee) => Ok(*pointee),
            other => Err(CodegenError::Internal(format!(
                "expected a pointer, found {other:?}"
            ))),
        }
    }

    /// A struct field's type.
    fn field_type(&self, ty: PoolId, index: u32) -> Result<PoolId, CodegenError> {
        let Item::StructType { decl } = self.ctx.pool.item(ty) else {
            return Err(CodegenError::Internal("a field of a non-struct".to_owned()));
        };
        self.ctx
            .pool
            .struct_fields(*decl)
            .and_then(|fields| fields.get(index as usize))
            .map(|field| field.ty)
            .ok_or_else(|| CodegenError::Internal(format!("no field {index}")))
    }
}

/// Which checked arithmetic operation is being emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arith {
    /// `+`.
    Add,
    /// `-`.
    Sub,
    /// `*`.
    Mul,
}

impl Arith {
    /// The trap this operation raises on overflow.
    ///
    /// One kind per operation, because `jr-vm` names the operation in its message and
    /// the differential harness compares those messages.
    const fn trap_kind(self) -> TrapKind {
        match self {
            Self::Add => TrapKind::OverflowAdd,
            Self::Sub => TrapKind::OverflowSub,
            Self::Mul => TrapKind::OverflowMul,
        }
    }
}

/// The Cranelift condition for a comparison operator.
fn condition(op: BinOp, signed: bool) -> IntCC {
    match op {
        BinOp::Eq => IntCC::Equal,
        BinOp::Ne => IntCC::NotEqual,
        BinOp::Lt if signed => IntCC::SignedLessThan,
        BinOp::Lt => IntCC::UnsignedLessThan,
        BinOp::Le if signed => IntCC::SignedLessThanOrEqual,
        BinOp::Le => IntCC::UnsignedLessThanOrEqual,
        BinOp::Gt if signed => IntCC::SignedGreaterThan,
        BinOp::Gt => IntCC::UnsignedGreaterThan,
        BinOp::Ge if signed => IntCC::SignedGreaterThanOrEqual,
        BinOp::Ge => IntCC::UnsignedGreaterThanOrEqual,
        // Only the six comparisons reach here; the caller matched on them.
        BinOp::Add
        | BinOp::Sub
        | BinOp::Mul
        | BinOp::Div
        | BinOp::Rem
        | BinOp::WrapAdd
        | BinOp::WrapSub
        | BinOp::WrapMul => IntCC::Equal,
    }
}

/// The most negative value of a signed Cranelift integer type.
fn min_of(ty: Type) -> i64 {
    match ty.bits() {
        8 => i64::from(i8::MIN),
        16 => i64::from(i16::MIN),
        32 => i64::from(i32::MIN),
        _ => i64::MIN,
    }
}

/// The Cranelift integer type of a given byte size, defaulting to `fallback`.
fn int_of_size(size: u64, fallback: Type) -> Type {
    match size {
        1 => cranelift_codegen::ir::types::I8,
        2 => cranelift_codegen::ir::types::I16,
        4 => cranelift_codegen::ir::types::I32,
        8 => cranelift_codegen::ir::types::I64,
        _ => fallback,
    }
}

/// The span a statement's instructions belong to.
fn statement_span(stmt: &Statement) -> MirSpan {
    match stmt {
        Statement::Assign { span, .. }
        | Statement::Store { span, .. }
        | Statement::Discard { span, .. } => *span,
        Statement::Nop => MirSpan::Synthetic,
    }
}

impl Translator<'_, '_> {
    /// The span a terminator's instructions belong to.
    ///
    /// A [`Terminator`] carries no span, but its operand is a value and every value
    /// does — so a branch reports the condition tested and a return reports the
    /// expression that produced the result. Mirrors `jr-vm`'s lowering, because the two
    /// engines must attribute a trap to the same construct or their messages differ.
    fn terminator_span(&self, term: &Terminator) -> MirSpan {
        match term {
            Terminator::Branch { cond, .. } => self.span_of(*cond),
            Terminator::Return(Some(operand)) => self.span_of(*operand),
            Terminator::Goto(_) | Terminator::Return(None) | Terminator::Unreachable(_) => {
                MirSpan::Synthetic
            }
        }
    }

    /// The span of the value an operand names, if it names one.
    fn span_of(&self, operand: Operand) -> MirSpan {
        match operand {
            Operand::Value(value) => self.body.value(value).span,
            Operand::Constant(_) => MirSpan::Synthetic,
        }
    }
}
