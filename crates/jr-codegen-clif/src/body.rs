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

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{
    Block, BlockArg, FuncRef, InstBuilder, MemFlagsData, StackSlotData, StackSlotKind, TrapCode,
    Type, Value as ClifValue,
};
use cranelift_frontend::FunctionBuilder;
use cranelift_module::{DataDescription, DataId, Linkage, Module};
use jr_codegen::{CodegenError, TrapLocations};
use jr_mir::{
    BinOp, BlockId, Callee, MirBody, MirSpan, NumKind, Operand, Place, PlaceBase, ProcRef,
    Projection, Rvalue, Statement, Target, Terminator, UnOp, Unreachable, ValueId,
};
use jr_pool::{
    Item, Pool, PoolId, TargetLayout, field_offset, layout_of, string_count, string_data,
};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::repr::{self, Repr, pointer_type};
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
    /// The shadow call stack the trap helper walks (ADR-0066 §1), and its live depth.
    ///
    /// A caller writes `(name, len)` for its callee at the depth's index and increments; the callee's
    /// return decrements. Held here rather than looked up per call, because both are one object for the
    /// whole module.
    pub shadow: (DataId, DataId),
    /// The read-only name string and its length for each procedure, for a backtrace frame.
    ///
    /// Absent for a procedure whose name is unknown — an anonymous one — and such a frame is simply not
    /// pushed, which is the same "omit rather than placeholder" rule the VM's renderer follows.
    pub names: &'a FxHashMap<ProcRef, (DataId, usize)>,
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
        sret: None,
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
    /// The caller-allocated result pointer, for a procedure returning an aggregate
    /// (ADR-0051 §1).
    ///
    /// `Some` exactly when [`repr::returns_via_sret`] says so, and read only by
    /// `Terminator::Return`. Held on the translator rather than looked up again at the
    /// return, because the *presence* of this parameter shifts every other parameter's
    /// position by one — deciding it twice would be two chances to disagree.
    sret: Option<ClifValue>,
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
        // The `sret` pointer is the *leading* parameter, so it must be taken before the
        // ordinary ones are bound — `bind_entry_params` walks the Cranelift list with its
        // own cursor and would otherwise bind the result pointer to the first real
        // parameter, shifting every argument by one.
        if repr::returns_via_sret(self.ctx.pool, self.ctx.target, self.body.ret())? {
            let params = self.builder.block_params(entry);
            self.sret = params.first().copied();
        }
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
        // Skip the hidden result pointer, which occupies position 0 when it exists
        // (ADR-0051 §1). Starting at 0 regardless bound the *result* pointer to the first
        // declared parameter and shifted every argument by one — a silent miscompile, and
        // the reason `sret` is decided by one shared predicate.
        let mut next = usize::from(self.sret.is_some());
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
            // ADR-0039 §4a: the zeroing the VM got for free from a fresh frame and
            // native code never did at all. `emit_small_memset` rather than a loop,
            // because Cranelift already knows how to pick between a store sequence and a
            // `memset` call for a given size.
            Statement::Zero { place, .. } => {
                let ty = self.place_type(place)?;
                // Straight from `layout_of` rather than from `Repr`, which carries a size
                // only for an aggregate — and `Statement::Zero` is emitted for a scalar
                // slot too, wherever one needs a place.
                let layout = jr_pool::layout_of(self.ctx.pool, self.ctx.target, ty)
                    .map_err(|reason| CodegenError::NoLayout { ty, reason })?;
                let address = self.address(place)?;
                self.memset_zero(address, layout.size, layout.align);
                Ok(())
            }
            // ADR-0003's check. An **unsigned** compare, so a negative index — which is a
            // huge unsigned value — fails the same test that catches one past the end.
            Statement::BoundsCheck { index, len, .. } => {
                let index = self.read_scalar(*index)?;
                let len = self.read_scalar(*len)?;
                // Phrased as `index >= len` so that `trap_if` — the existing helper, whose
                // trap block is marked cold — can be reused rather than gaining an inverted
                // twin that could drift from it.
                let out_of_range =
                    self.builder
                        .ins()
                        .icmp(IntCC::UnsignedGreaterThanOrEqual, index, len);
                self.trap_if(out_of_range, TrapKind::IndexOutOfBounds)
            }
            // The tag is one byte at the variant's own address (ADR-0068 §3), so this loads a byte and
            // compares. Phrased as `tag != case` so `trap_if`'s cold trap block is reused, exactly as
            // the bounds check above reuses it rather than gaining an inverted twin.
            Statement::TagCheck { place, case, .. } => {
                let address = self.address(place)?;
                let tag = self.builder.ins().load(
                    cranelift_codegen::ir::types::I8,
                    MemFlagsData::new(),
                    address,
                    0,
                );
                let wrong = self
                    .builder
                    .ins()
                    .icmp_imm_s(IntCC::NotEqual, tag, i64::from(*case));
                self.trap_if(wrong, TrapKind::WrongVariantCase)
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
            Rvalue::Convert { operand, from } => self.convert(*operand, *from, dest),
            Rvalue::Call { callee, args } => self.call(callee, args, dest),
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
                // **The placeholder's instruction must match the register class**, not just the
                // width: `iconst` on an `F32`/`F64` is what Cranelift's `iconst_bounds` verifier
                // rejects, and it panics with "entered unreachable code" nowhere near here.
                //
                // Not the cause of the float-constant crash `PLAN.md` §7 records — that reproduces
                // with this fixed — but wrong on its own terms, and reachable the moment a float
                // local goes uninitialised. Fixed while looking for the other bug.
                Ok(repr.clif_type(self.ctx.target).map(|clif| {
                    if clif == cranelift_codegen::ir::types::F32 {
                        self.builder.ins().f32const(0.0)
                    } else if clif == cranelift_codegen::ir::types::F64 {
                        self.builder.ins().f64const(0.0)
                    } else {
                        self.builder.ins().iconst(clif, 0)
                    }
                }))
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
            Item::FloatValue { ty, bits } => {
                // `f32const`/`f64const` take a typed immediate, so the stored bits are
                // reinterpreted at the right width. A `float32`'s bits are its low 32 —
                // `FloatKind::encode` put them there — so truncating is reading them, not
                // losing them.
                let clif = match jr_pool::FloatKind::of(self.ctx.pool, ty) {
                    Some(kind) if kind.bits == 32 => {
                        let value = f32::from_bits(bits as u32);
                        self.builder.ins().f32const(value)
                    }
                    Some(_) => {
                        let value = f64::from_bits(bits);
                        self.builder.ins().f64const(value)
                    }
                    None => {
                        return Err(CodegenError::Internal(String::from(
                            "a float constant whose type is not a float",
                        )));
                    }
                };
                Ok(Some(clif))
            }
            Item::StrValue(str_id) => self.string_constant(str_id).map(Some),
            // A **procedure value** is the code address of its target (ADR-0059 §4). Native uses a
            // real function pointer — unlike the VM's encoded handle — because `call_indirect` takes
            // an address, and nothing observes the bits so the two engines need not agree on them.
            // `funcs` already holds a `FuncRef` for every reachable procedure, imported into this
            // function; `func_addr` turns one into a pointer value.
            Item::ProcValue { ty: _, decl } => {
                let target = ProcRef::new(decl.file, jr_hir::ProcId::from_u32(decl.index));
                let func = *self.ctx.funcs.get(&target).ok_or_else(|| {
                    // A cross-file procedure value is refused in `scan` (ADR-0059 §1), and a
                    // `#foreign` one is E0256 from sema, so a missing entry is a compiler bug
                    // rather than a user error.
                    CodegenError::Internal(format!(
                        "no function declared for a procedure value {target:?}"
                    ))
                })?;
                let pointer = pointer_type(self.ctx.target);
                Ok(Some(self.builder.ins().func_addr(pointer, func)))
            }
            _ => Err(CodegenError::Unsupported {
                proc: self.proc,
                what: "a type or library used as a runtime value".to_owned(),
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

        // Floats first and separately, because every instruction differs: `fadd` not `iadd`,
        // `fcmp` not `icmp`, and — the point of ADR-0040 §1 — **no overflow check at all**.
        // Routing a float through the integer path below would emit `sadd_overflow` on a
        // float register, which Cranelift rejects, so this is a hard failure rather than a
        // silent one; the separation is for clarity, not safety.
        if jr_pool::FloatKind::of(self.ctx.pool, ty).is_some() {
            let value = match op {
                BinOp::Add => self.builder.ins().fadd(left, right),
                BinOp::Sub => self.builder.ins().fsub(left, right),
                BinOp::Mul => self.builder.ins().fmul(left, right),
                // No zero check: `x / 0.0` is `inf` and `0.0 / 0.0` is `NaN`, which
                // ADR-0040 §1 makes values rather than failures.
                BinOp::Div => self.builder.ins().fdiv(left, right),
                BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                    // `fcmp` implements IEEE-754's ordering, which is why `NaN == NaN` comes
                    // out false and `0.0 == -0.0` comes out true without either being special
                    // -cased here. A raw bit compare would get both backwards.
                    let cc = float_condition(op).ok_or_else(|| {
                        CodegenError::Internal(String::from(
                            "a comparison with no float condition code",
                        ))
                    })?;
                    self.builder.ins().fcmp(cc, left, right)
                }
                // Refused by sema (ADR-0040 §7 for `%`, ADR-0042 §5 for the bitwise forms);
                // reaching here means sema and this back end disagree about what was checked.
                BinOp::Rem
                | BinOp::WrapAdd
                | BinOp::WrapSub
                | BinOp::WrapMul
                | BinOp::BitAnd
                | BinOp::BitOr
                | BinOp::BitXor
                | BinOp::Shl
                | BinOp::Shr => {
                    return Err(CodegenError::Internal(format!(
                        "{op:?} is not defined on a floating-point operand"
                    )));
                }
            };
            return Ok(Some(value));
        }

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
            // Cranelift has all three natively and none can trap.
            BinOp::BitAnd => self.builder.ins().band(left, right),
            BinOp::BitOr => self.builder.ins().bor(left, right),
            BinOp::BitXor => self.builder.ins().bxor(left, right),
            // `sshr` for a signed type and `ushr` otherwise, which is what makes `>>`
            // arithmetic for `s8` and logical for `u8` (ADR-0042 §2) — the same
            // signedness-driven choice `division` makes between `sdiv` and `udiv`.
            BinOp::Shl | BinOp::Shr => self.shift(op, signed, left, right)?,
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

    /// Emits a shift, checking the count first (ADR-0042 §3).
    ///
    /// **Cranelift masks the shift count** — `ishl` on an `I8` uses the low 3 bits — so
    /// without this check `x << 8` would silently become `x << 0`, which is precisely the
    /// behaviour ADR-0042 §3 rejected. The check is a compare-and-trap into the *existing*
    /// cold trap block, so it reuses `trap_if` rather than adding a mechanism.
    ///
    /// The count is compared **unsigned** against the width, which catches a negative count
    /// in the same comparison: a negative count reinterpreted as unsigned is enormous. That
    /// is the same one-comparison trick `Statement::BoundsCheck` uses (ADR-0039 §1).
    fn shift(
        &mut self,
        op: BinOp,
        signed: bool,
        left: ClifValue,
        right: ClifValue,
    ) -> Result<ClifValue, CodegenError> {
        let width = i64::from(self.builder.func.dfg.value_type(left).bits());
        let out_of_range =
            self.builder
                .ins()
                .icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, right, width);
        self.trap_if(out_of_range, TrapKind::ShiftOutOfRange)?;
        Ok(match (op, signed) {
            (BinOp::Shl, _) => self.builder.ins().ishl(left, right),
            (BinOp::Shr, true) => self.builder.ins().sshr(left, right),
            (BinOp::Shr, false) => self.builder.ins().ushr(left, right),
            _ => {
                return Err(CodegenError::Internal(
                    "shift called for a non-shift operator".to_owned(),
                ));
            }
        })
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
    /// Translates a `cast(T, x)` (ADR-0037 §2).
    ///
    /// Three cases, and Cranelift insists on the distinction: `ireduce` requires the
    /// destination to be strictly *narrower*, `uextend`/`sextend` strictly *wider*, and
    /// neither accepts equal widths — passing one an equal type is a panic inside the builder
    /// rather than a compile error here. So an equal-width cast is a pass-through, which is
    /// also what makes `cast(s64, some_s64)` free.
    ///
    /// Widening uses `sextend` for a **signed source** and `uextend` otherwise, mirroring the
    /// VM's `value.as_int(from)`: `from` is the source kind precisely because sign extension
    /// cannot be decided from the destination.
    ///
    /// Never traps, matching ADR-0037 §2 and the interpreter. This is the one place where the
    /// two engines could silently disagree about a number, which is why `differential.rs`
    /// carries a case per direction.
    fn convert(
        &mut self,
        operand: Operand,
        from: NumKind,
        dest: Option<ValueId>,
    ) -> Result<Slot, CodegenError> {
        let value = self.read_scalar(operand)?;
        let source = self.builder.func.dfg.value_type(value);
        let target_ty = match dest {
            Some(dest) => self.body.value(dest).ty,
            // No destination means nothing reads the result; the conversion is then a no-op
            // rather than a guess at a width.
            None => return Ok(Some(value)),
        };
        let to = NumKind::of(self.ctx.pool, target_ty)
            .ok_or_else(|| CodegenError::Internal(String::from("a cast to a non-numeric type")))?;
        let Some(target) =
            Repr::of(self.ctx.pool, self.ctx.target, target_ty)?.clif_type(self.ctx.target)
        else {
            return Err(CodegenError::Internal(String::from(
                "a cast to a type with no register representation",
            )));
        };

        // Four directions (ADR-0040 §3), and each needs a different Cranelift instruction
        // family. The pairing is what `from` is recorded for: sign extension cannot be
        // decided from the destination, and neither can whether to emit an `fcvt` or an
        // `sextend`.
        let result = match (from, to) {
            (NumKind::Int(from), NumKind::Int(_)) => {
                // Cranelift insists on the distinction: `ireduce` requires the destination to
                // be strictly narrower, `uextend`/`sextend` strictly wider, and neither
                // accepts equal widths — passing one an equal type panics inside the builder.
                match target.bits().cmp(&source.bits()) {
                    std::cmp::Ordering::Less => self.builder.ins().ireduce(target, value),
                    std::cmp::Ordering::Equal => value,
                    std::cmp::Ordering::Greater => {
                        if from.signed {
                            self.builder.ins().sextend(target, value)
                        } else {
                            self.builder.ins().uextend(target, value)
                        }
                    }
                }
            }
            (NumKind::Int(from), NumKind::Float(_)) => {
                if from.signed {
                    self.builder.ins().fcvt_from_sint(target, value)
                } else {
                    self.builder.ins().fcvt_from_uint(target, value)
                }
            }
            (NumKind::Float(_), NumKind::Int(to)) => {
                // The **saturating** forms, matching `jr_pool::float_to_int` and ADR-0040 §4:
                // `fcvt_to_sint` *traps* on an out-of-range value, which would put a trap back
                // on a path §1 just made trap-free and would disagree with the VM. `_sat`
                // clamps and maps `NaN` to 0, which is exactly what the interpreter does.
                //
                // Cranelift's `_sat` instructions produce at least `I32`, so a narrower
                // destination needs an `ireduce` afterwards.
                let wide = if to.bits <= 32 {
                    cranelift_codegen::ir::types::I32
                } else {
                    cranelift_codegen::ir::types::I64
                };
                let converted = if to.signed {
                    self.builder.ins().fcvt_to_sint_sat(wide, value)
                } else {
                    self.builder.ins().fcvt_to_uint_sat(wide, value)
                };
                if target.bits() < wide.bits() {
                    // Narrowing here *truncates* rather than saturating a second time — but
                    // the value is already clamped to `wide`, and `jr_pool::float_to_int`
                    // clamps to the destination's own range, so the two would disagree for
                    // e.g. `cast(s8, 1000.0)`: this would give 1000 truncated to -24 while
                    // the VM gives 127. So clamp explicitly before narrowing.
                    let min = to.min() as i64;
                    let max = to.max() as i64;
                    let too_small =
                        self.builder
                            .ins()
                            .icmp_imm_s(IntCC::SignedLessThan, converted, min);
                    let min_v = self.builder.ins().iconst(wide, min);
                    let clamped_low = self.builder.ins().select(too_small, min_v, converted);
                    let too_big =
                        self.builder
                            .ins()
                            .icmp_imm_s(IntCC::SignedGreaterThan, clamped_low, max);
                    let max_v = self.builder.ins().iconst(wide, max);
                    let clamped = self.builder.ins().select(too_big, max_v, clamped_low);
                    self.builder.ins().ireduce(target, clamped)
                } else {
                    converted
                }
            }
            (NumKind::Float(_), NumKind::Float(_)) => {
                match target.bits().cmp(&source.bits()) {
                    // `float64` → `float32` rounds to nearest and saturates to `inf`
                    // (ADR-0040 §4), which is what `fdemote` does.
                    std::cmp::Ordering::Less => self.builder.ins().fdemote(target, value),
                    std::cmp::Ordering::Equal => value,
                    // `float32` → `float64` is exact, always.
                    std::cmp::Ordering::Greater => self.builder.ins().fpromote(target, value),
                }
            }
        };
        Ok(Some(result))
    }

    fn unary(&mut self, op: UnOp, operand: Operand) -> Result<Slot, CodegenError> {
        let value = self.read_scalar(operand)?;
        let ty = self.operand_type(operand);
        let signed = matches!(
            Repr::of(self.ctx.pool, self.ctx.target, ty)?,
            Repr::Scalar { signed: true, .. }
        );
        // A float negation flips the sign bit and cannot fail, which is exactly where it
        // differs from an integer's: `-MIN` is one past the maximum and traps (ADR-0002),
        // while `-0.0` is a real value (ADR-0040 §1). `fneg` rather than a subtract from
        // zero, because `0.0 - 0.0` is `+0.0` and would lose the sign.
        if jr_pool::FloatKind::of(self.ctx.pool, ty).is_some() {
            return match op {
                UnOp::Neg => Ok(Some(self.builder.ins().fneg(value))),
                UnOp::Not | UnOp::BitNot => Err(CodegenError::Internal(String::from(
                    "`!` or `~` on a floating-point operand",
                ))),
            };
        }

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
            // `~` *is* the bitwise complement `!` must not be. `bnot` works on the operand's
            // own width, so a `u8` complements within 8 bits — matching `int_not`'s
            // normalisation rather than complementing at 64 and truncating (ADR-0042 §4).
            UnOp::BitNot => self.builder.ins().bnot(value),
        };
        Ok(Some(result))
    }

    // -----------------------------------------------------------------------
    // Calls
    // -----------------------------------------------------------------------

    /// Translates a call.
    /// Emits a call, allocating the result slot when the callee returns an aggregate.
    ///
    /// `dest` gives the result's type, which is what decides whether the hidden `sret`
    /// pointer is passed — the *same* `Repr::is_aggregate` question the callee's signature
    /// asked, via the shared `repr::returns_via_sret`, so caller and callee cannot disagree
    /// about the parameter count (ADR-0051 §1).
    fn call(
        &mut self,
        callee: &Callee,
        args: &[Operand],
        dest: Option<ValueId>,
    ) -> Result<Slot, CodegenError> {
        // The `sret` slot, the argument reads and the result placement are identical whether the
        // callee is direct or indirect — only the call *instruction* differs (`call` against a
        // `FuncRef` versus `call_indirect` against an imported signature and a pointer value). So
        // the callee is resolved to a closure that emits the one instruction, and everything around
        // it is shared. Duplicating the slot logic per callee kind is how the two would drift about
        // whether an aggregate return is placed the same way.

        // The result type, from the value this call assigns to. A discarded call has no
        // destination and therefore no aggregate to place — `Statement::Discard` on an
        // aggregate-returning procedure would need a slot with no reader, and MIR does not
        // produce one because a discarded call's `dest` is `None` only for `void`.
        let ret_ty = dest.map_or(PoolId::VOID, |id| self.body.value(id).ty);
        let via_sret = repr::returns_via_sret(self.ctx.pool, self.ctx.target, ret_ty)?;

        let mut values = Vec::with_capacity(args.len() + usize::from(via_sret));
        // A **fresh** slot per call, copied out of afterwards rather than passing the
        // destination's own address (ADR-0051 §2). One extra copy, deliberately: passing the
        // destination directly would let a callee that traps halfway leave the caller's
        // variable half-assigned, and ADR-0002's traps are real control flow.
        let result_slot = if via_sret {
            let layout = layout_of(self.ctx.pool, self.ctx.target, ret_ty)
                .map_err(|reason| CodegenError::NoLayout { ty: ret_ty, reason })?;
            let size = u32::try_from(layout.size.max(1)).map_err(|_| {
                CodegenError::Internal("a call result is larger than a u32".to_owned())
            })?;
            let data = StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                size,
                layout.align.trailing_zeros().try_into().unwrap_or(0),
            );
            let handle = self.builder.create_sized_stack_slot(data);
            let pointer = pointer_type(self.ctx.target);
            let address = self.builder.ins().stack_addr(pointer, handle, 0);
            // Leading, matching the signature.
            values.push(address);
            Some(address)
        } else {
            None
        };

        for arg in args {
            if let Some(value) = self.read(*arg)? {
                values.push(value);
            }
        }
        // **The shadow-stack push, before the call** (ADR-0066 §1), matching the VM, which pushes in
        // `Vm::call`. Only a *direct* call can be pushed: an indirect one's target is a runtime pointer,
        // and the name to push is a compile-time constant — so an indirect frame is absent, exactly as
        // an inlined one is, and for the same honest reason.
        let pushed = match callee {
            Callee::Direct(target) => self.push_frame(*target)?,
            Callee::Indirect(_) => false,
        };
        let inst = match callee {
            Callee::Direct(target) => {
                let func = *self
                    .ctx
                    .funcs
                    .get(target)
                    .ok_or(CodegenError::Undeclared(*target))?;
                self.builder.ins().call(func, &values)
            }
            // A pointer value plus an imported signature (ADR-0059 §4). The signature is built
            // from the callee operand's *type* — a `ProcType` sema resolved — with the same
            // convention a direct Jairs call uses: `CallConv::Fast`, receives the context, never a
            // `#foreign` one (a `#foreign` procedure cannot be an indirect target, ADR-0059 §5).
            Callee::Indirect(operand) => {
                let sig = self.indirect_signature(*operand)?;
                let sig_ref = self.builder.import_signature(sig);
                let pointer = self.read_scalar(*operand)?;
                self.builder.ins().call_indirect(sig_ref, pointer, &values)
            }
        };
        // The pop, after the call returns. A callee that traps never returns here — the helper calls
        // `exit` — so the depth it left behind is exactly the chain the trap should report, which is why
        // the pop being skipped on that path is correct rather than a leak.
        if pushed {
            self.pop_frame();
        }
        // An `sret` call returns nothing; the result *is* the slot, and an aggregate value
        // is represented by a pointer to its bytes, so the address is the value.
        if let Some(address) = result_slot {
            return Ok(Some(address));
        }
        let results = self.builder.inst_results(inst);
        Ok(results.first().copied())
    }

    /// Writes `target`'s name onto the shadow call stack and increments the depth (ADR-0066 §1).
    ///
    /// Returns whether anything was pushed: `false` for a procedure with no known name, whose frame is
    /// omitted rather than rendered as a placeholder.
    ///
    /// **Bounds-checked**, because a static array written past its end is memory corruption that would
    /// be blamed on the program rather than on the compiler. Past `SHADOW_CAPACITY` the push is skipped
    /// and the depth still rises, so the *count* stays honest while the entries stop — and the pop is
    /// symmetric, so nothing drifts.
    fn push_frame(&mut self, target: ProcRef) -> Result<bool, CodegenError> {
        let Some((name_id, name_len)) = self.ctx.names.get(&target).copied() else {
            return Ok(false);
        };
        let pointer = pointer_type(self.ctx.target);
        let width = i64::from(self.ctx.target.pointer_size);

        let (stack_id, depth_id) = self.ctx.shadow;
        let stack_global = self
            .module
            .declare_data_in_func(stack_id, self.builder.func);
        let depth_global = self
            .module
            .declare_data_in_func(depth_id, self.builder.func);
        let name_global = self.module.declare_data_in_func(name_id, self.builder.func);

        let stack_base = self.builder.ins().symbol_value(pointer, stack_global);
        let depth_addr = self.builder.ins().symbol_value(pointer, depth_global);
        let depth = self
            .builder
            .ins()
            .load(pointer, MemFlagsData::new(), depth_addr, 0);

        // Only write the entry when it is in range; the depth is bumped either way.
        let in_range = self.builder.ins().icmp_imm_s(
            cranelift_codegen::ir::condcodes::IntCC::SignedLessThan,
            depth,
            i64::try_from(crate::SHADOW_CAPACITY).unwrap_or(i64::MAX),
        );
        let write_block = self.builder.create_block();
        let after = self.builder.create_block();
        self.builder
            .ins()
            .brif(in_range, write_block, &[], after, &[]);

        self.builder.switch_to_block(write_block);
        let offset = self.builder.ins().imul_imm_s(depth, width * 2);
        let entry = self.builder.ins().iadd(stack_base, offset);
        let name = self.builder.ins().symbol_value(pointer, name_global);
        let len = self
            .builder
            .ins()
            .iconst(pointer, i64::try_from(name_len).unwrap_or(0));
        self.builder
            .ins()
            .store(MemFlagsData::new(), name, entry, 0);
        self.builder.ins().store(
            MemFlagsData::new(),
            len,
            entry,
            i32::try_from(width).unwrap_or(8),
        );
        self.builder.ins().jump(after, &[]);
        self.builder.seal_block(write_block);

        self.builder.switch_to_block(after);
        let bumped = self.builder.ins().iadd_imm_s(depth, 1);
        self.builder
            .ins()
            .store(MemFlagsData::new(), bumped, depth_addr, 0);
        Ok(true)
    }

    /// Decrements the shadow call stack's depth, undoing one [`Self::push_frame`].
    fn pop_frame(&mut self) {
        let pointer = pointer_type(self.ctx.target);
        let (_, depth_id) = self.ctx.shadow;
        let depth_global = self
            .module
            .declare_data_in_func(depth_id, self.builder.func);
        let depth_addr = self.builder.ins().symbol_value(pointer, depth_global);
        let depth = self
            .builder
            .ins()
            .load(pointer, MemFlagsData::new(), depth_addr, 0);
        let dropped = self.builder.ins().iadd_imm_s(depth, -1);
        self.builder
            .ins()
            .store(MemFlagsData::new(), dropped, depth_addr, 0);
    }

    /// The Cranelift signature for a call through a procedure pointer (ADR-0059 §4).
    ///
    /// Built from the callee operand's `Item::ProcType` — its parameter and return types — with the
    /// convention a direct Jairs call uses: `CallConv::Fast`, and the implicit context as a leading
    /// hidden parameter. A proc-pointer type is always Jairs-convention this wave (ADR-0059 §3), so
    /// `receives_context` is always true and `foreign` always false; there is no `#c_call`
    /// proc-pointer type to vary them.
    ///
    /// The signature must match the callee's *declared* one exactly — the same `repr::signature`
    /// builds both — or the two disagree about the parameter count, which is the silent-shift
    /// failure `repr::returns_via_sret` exists to prevent (ADR-0051 §1).
    fn indirect_signature(
        &self,
        operand: Operand,
    ) -> Result<cranelift_codegen::ir::Signature, CodegenError> {
        let proc_ty = self.operand_type(operand);
        let Item::ProcType { params, ret, .. } = self.ctx.pool.item(proc_ty) else {
            return Err(CodegenError::Internal(
                "an indirect call whose callee is not of procedure type".to_owned(),
            ));
        };
        let params = params.clone();
        let ret = *ret;
        let proc = self.proc;
        repr::signature(
            self.ctx.pool,
            self.ctx.target,
            &params,
            ret,
            cranelift_codegen::isa::CallConv::Fast,
            false,
            true,
            &|what: &str| CodegenError::Unsupported {
                proc,
                what: what.to_owned(),
            },
        )
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
                Projection::Index(index) => {
                    // An array place is indexed in place; a **pointer** place — a view's
                    // `data` word — is loaded first and then indexed. The VM's `plan` has the
                    // same two shapes in one arm, deliberately: one stride computation for
                    // both, so an array element and a view element cannot land at different
                    // addresses in the two engines.
                    let elem = self.index_elem(ty)?;
                    if let Item::PointerType(_) = self.ctx.pool.item(ty) {
                        address = self
                            .builder
                            .ins()
                            .load(pointer, MemFlagsData::new(), address, 0);
                    }
                    // The stride is the element size rounded up to its alignment, the same
                    // computation `jr-pool`'s `layout_of` uses for the array's total size —
                    // so an element address here and the array's size there are derived
                    // from one rule rather than two (ADR-0018 §2).
                    let layout = jr_pool::layout_of(self.ctx.pool, self.ctx.target, elem)
                        .map_err(|reason| CodegenError::NoLayout { ty: elem, reason })?;
                    let stride = layout.size.next_multiple_of(u64::from(layout.align));
                    let index = self.read_scalar(*index)?;
                    // The index is an `s64` and a pointer is 64-bit on both targets, so no
                    // conversion is needed. `imul_imm` then `iadd`, wrapping — an index that
                    // could wrap has already failed the bounds check emitted before this.
                    let scaled = self
                        .builder
                        .ins()
                        .imul_imm_s(index, i64::try_from(stride).unwrap_or(i64::MAX));
                    address = self.builder.ins().iadd(address, scaled);
                    ty = elem;
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
                // The same offsets a string's two words have — one shared computation, so the
                // VM and Cranelift cannot disagree about a view's layout (ADR-0044 §1). The
                // result *type* differs: `*T`, not `*u8`, which is what gives an index into
                // the view the right stride.
                Projection::ViewData => {
                    let elem = self.view_elem(ty)?;
                    let (offset, _) = jr_pool::pair_data(self.ctx.target);
                    address = self.offset(address, offset);
                    ty = self
                        .ctx
                        .pool
                        .find(&Item::PointerType(elem))
                        .ok_or_else(|| {
                            CodegenError::Internal(
                                "a view's element pointer type was never interned".to_owned(),
                            )
                        })?;
                }
                Projection::ViewCount => {
                    let (offset, _) = jr_pool::pair_count(self.ctx.target);
                    address = self.offset(address, offset);
                    ty = PoolId::S64;
                }
                // The tag is the leading field, so its offset is 0 and the address is unchanged
                // (ADR-0068 §3). Only the type moves, to `u8`, so a load reads one byte.
                Projection::VariantTag => {
                    ty = PoolId::U8;
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
                Projection::Index(_) => self.index_elem(ty)?,
                Projection::Deref => self.pointee(ty)?,
                Projection::StringData => PoolId::PTR_U8,
                Projection::StringCount => PoolId::S64,
                Projection::ViewData => {
                    let elem = self.view_elem(ty)?;
                    self.ctx
                        .pool
                        .find(&Item::PointerType(elem))
                        .ok_or_else(|| {
                            CodegenError::Internal(
                                "a view's element pointer type was never interned".to_owned(),
                            )
                        })?
                }
                Projection::ViewCount => PoolId::S64,
                Projection::VariantTag => PoolId::U8,
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
    /// Sets `size` bytes at `address` to zero.
    ///
    /// `emit_small_memset` rather than a hand-rolled store loop: Cranelift already
    /// decides between an inline store sequence and a `memset` call based on the size,
    /// which is the same judgement [`Self::copy`] delegates for a byte copy.
    fn memset_zero(&mut self, address: ClifValue, size: u64, align: u32) {
        if size == 0 {
            return;
        }
        let config = self.module.target_config();
        let align = u8::try_from(align).unwrap_or(1);
        // The fill byte is a plain `u8` immediate, not a Cranelift value.
        self.builder
            .emit_small_memset(config, address, 0, size, align, MemFlagsData::new());
    }

    /// The element type of an array type.
    /// The element type an `Projection::Index` step lands on.
    ///
    /// Accepts an array *or* a pointer, because a view's element place is its `data` word
    /// indexed directly — so this replaced a stricter array-only helper rather than sitting
    /// beside one, which would have left two answers to one question.
    fn index_elem(&self, ty: PoolId) -> Result<PoolId, CodegenError> {
        match self.ctx.pool.item(ty) {
            Item::ArrayType { elem, .. } | Item::PointerType(elem) => Ok(*elem),
            _ => Err(CodegenError::Internal(
                "an index projection on neither an array nor a pointer".to_owned(),
            )),
        }
    }

    /// The element type of a view, for the two `Projection::View*` steps.
    ///
    /// Separate from [`Self::array_elem`] rather than one function accepting either: the two
    /// projections that reach here are already distinct, and a shared helper would let a
    /// `ViewData` step on an array type pass silently.
    fn view_elem(&self, ty: PoolId) -> Result<PoolId, CodegenError> {
        match self.ctx.pool.item(ty) {
            Item::ViewType { elem } => Ok(*elem),
            _ => Err(CodegenError::Internal(
                "a view projection on a non-view type".to_owned(),
            )),
        }
    }

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
                // **An aggregate result is copied into the caller's slot, and nothing is
                // returned** (ADR-0051 §1). The operand holds a *pointer* to the callee's
                // own storage — `Repr::Aggregate` travels as one — so returning it
                // directly would hand back the address of a frame about to be destroyed.
                // That dangling pointer is why the refusal this replaces existed.
                if let Some(dest) = self.sret {
                    if let Some(operand) = operand {
                        let ty = self.operand_type(*operand);
                        if let Some(src) = self.read(*operand)? {
                            let layout = layout_of(self.ctx.pool, self.ctx.target, ty)
                                .map_err(|reason| CodegenError::NoLayout { ty, reason })?;
                            self.copy(dest, src, layout.size, layout.align);
                        }
                    }
                    self.builder.ins().return_(&[]);
                    return Ok(());
                }
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
        let message = jr_base::trap_message(kind.reason(), location.as_deref(), &[]);
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
        // A results aggregate carries its element list directly (ADR-0052 §1), so there is no
        // `DeclId` and no side table — the **third** field-type walk this wave had to teach, after
        // `jr-pool`'s `field_offset` and `jr-vm`'s. Three copies of "what type is field N" is the
        // duplication ADR-0018 §2 warns about; consolidating them is owed and recorded in §7.
        // The context's fields, from the same list (ADR-0057 §1).
        if matches!(self.ctx.pool.item(ty), Item::ContextType) {
            return jr_pool::Pool::context_field_type(index)
                .ok_or_else(|| CodegenError::Internal(format!("no context field {index}")));
        }
        if let Item::ResultsType { elems } = self.ctx.pool.item(ty) {
            return elems
                .get(index as usize)
                .copied()
                .ok_or_else(|| CodegenError::Internal(format!("no result {index}")));
        }
        // Accepts a union as well as a struct: the field *list* is shared and only the
        // offsets differ (ADR-0045 §5).
        let (Item::StructType { decl } | Item::UnionType { decl } | Item::VariantType { decl }) =
            self.ctx.pool.item(ty)
        else {
            return Err(CodegenError::Internal(
                "a field of a non-aggregate".to_owned(),
            ));
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
/// The IEEE-754 condition code a comparison becomes.
///
/// The **ordered** forms for `<`, `<=`, `>`, `>=`, and `Equal`/`NotEqual` for `==`/`!=`.
/// That pairing is what gives `NaN` its two surprising answers without a special case:
/// `NaN < x` is false because `NaN` is unordered with everything, and `NaN != NaN` is true
/// because Cranelift's `NotEqual` is the *unordered-or-not-equal* form — the negation of
/// `Equal` rather than its own ordered predicate, exactly as Rust's `!=` on `f64` is.
fn float_condition(op: BinOp) -> Option<FloatCC> {
    match op {
        BinOp::Eq => Some(FloatCC::Equal),
        BinOp::Ne => Some(FloatCC::NotEqual),
        BinOp::Lt => Some(FloatCC::LessThan),
        BinOp::Le => Some(FloatCC::LessThanOrEqual),
        BinOp::Gt => Some(FloatCC::GreaterThan),
        BinOp::Ge => Some(FloatCC::GreaterThanOrEqual),
        BinOp::Add
        | BinOp::Sub
        | BinOp::Mul
        | BinOp::Div
        | BinOp::Rem
        | BinOp::WrapAdd
        | BinOp::WrapSub
        | BinOp::WrapMul
        | BinOp::BitAnd
        | BinOp::BitOr
        | BinOp::BitXor
        | BinOp::Shl
        | BinOp::Shr => None,
    }
}

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
        | BinOp::WrapMul
        | BinOp::BitAnd
        | BinOp::BitOr
        | BinOp::BitXor
        | BinOp::Shl
        | BinOp::Shr => IntCC::Equal,
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
        | Statement::Discard { span, .. }
        | Statement::Zero { span, .. }
        | Statement::BoundsCheck { span, .. }
        | Statement::TagCheck { span, .. } => *span,
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
