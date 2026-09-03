//! One MIR body, translated into LLVM IR.
//!
//! # What differs from the Cranelift back end, and why
//!
//! The two translations are deliberately the same shape — the same walk over the same MIR,
//! asking `jr-pool` the same layout questions in the same order — because a differential
//! test between two back ends is only as sharp as their independence is *honest*. Where they
//! differ, they differ because LLVM differs (ADR-0143 §3–§5):
//!
//! - **A block parameter becomes a `phi`.** LLVM has no block parameters, so the information
//!   is written from the other end: the block lists its predecessors. MIR forbids critical
//!   edges (ADR-0017 §1), so each edge names exactly one predecessor block and a `phi`'s
//!   incoming list is precisely what those edges carry. Phis are created empty when the
//!   blocks are, and filled once every terminator has been translated.
//! - **Every address is an opaque `ptr` and every offset is a byte GEP.** No Jairs aggregate
//!   acquires an LLVM `StructType`, because that would put LLVM's padding rules in charge of
//!   where a field sits — a second layout computation, which ADR-0018 §2 forbids for a
//!   reason that is *silent* when violated.
//! - **Poison must be avoided rather than tolerated.** A shift past the width, a division by
//!   zero, `INT_MIN / -1` and an out-of-range `fptosi` are all undefined in LLVM where Jairs
//!   requires a trap or saturation. Each is checked before the operation, and the float
//!   conversions use `llvm.fpto{s,u}i.sat`.
//!
//! # Why every `alloca` lives in a block of its own
//!
//! Cranelift's `create_sized_stack_slot` reserves space in the frame, once, wherever it is
//! called from. An LLVM `alloca` allocates where it is *executed*, so one inside a loop grows
//! the stack per iteration. Since MIR asks for a temporary at each syntactic site — an
//! aggregate load's copy, a call's result slot, a string literal's pair — the two would
//! disagree about a program's memory use, and a long-running loop would exhaust the stack in
//! one back end and not the other.
//!
//! So the function begins with an `alloca` block that branches to the real entry block, and
//! every allocation is appended there. One slot per site, reused each iteration, which is
//! what the other back end and the VM both do.
//!
//! # Why `Rvalue::Undef` is tracked rather than materialised
//!
//! The same reason as in the Cranelift back end: an undefined value must not become a zero,
//! because that would hide the bug E0227 reports, and it must not trap at its *definition*
//! either, because the VM traps on **use**. So undefinedness is a property of a `ValueId`,
//! propagated through a plain `Use`, and turned into a trap at each site that reads it.

use inkwell::basic_block::BasicBlock;
use inkwell::builder::{Builder, BuilderError};
use inkwell::context::Context as LlvmContext;
use inkwell::debug_info::AsDIScope as _;
use inkwell::intrinsics::Intrinsic;
use inkwell::module::{Linkage, Module};
use inkwell::types::{BasicTypeEnum, IntType};
use inkwell::values::{
    BasicMetadataValueEnum, BasicValue, BasicValueEnum, FunctionValue, GlobalValue, IntValue,
    PhiValue, PointerValue,
};
use inkwell::{AddressSpace, FloatPredicate, IntPredicate};
use jr_codegen::{CodegenError, SourceInfo, TrapKind};
use jr_mir::{
    BinOp, BlockId, Callee, GlobalRef, MirBody, MirSpan, NumKind, Operand, Place, PlaceBase,
    ProcRef, Projection, Rvalue, Statement, Target, Terminator, UnOp, Unreachable, ValueId,
};
use jr_pool::{
    Item, Pool, PoolId, StrId, TargetLayout, field_offset, layout_of, string_count, string_data,
};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::repr::{self, Repr, ScalarRepr, pointer_int};

/// What a translated MIR value is, once `void` is accounted for.
///
/// `void` occupies no register, so it is an absence rather than a zero — the same
/// distinction `jr-vm`'s `Shape` draws, and for the same reason (ADR-0015 §3).
type Slot<'ctx> = Option<BasicValueEnum<'ctx>>;

/// The alignment this back end claims on a load, a store or a copy.
///
/// **One, deliberately, and it is a soundness fix rather than a pessimisation** (ADR-0144 §4).
/// An LLVM `load ... align N` is a *promise* about the address, and undefined behaviour when the
/// promise is false. This back end computes every address itself from `jr-pool`'s offsets, and a
/// `#place`d field may sit at an offset its type's natural alignment does not divide — so claiming
/// the type's alignment would be claiming something the compiler has not established.
///
/// `align 1` is always true. It costs nothing here because the module is emitted at
/// `OptimizationLevel::None`, where LLVM has no alignment-dependent transform to decline; the
/// alignment that *is* established — an `alloca`'s — is still requested exactly, because there this
/// back end is the one making the promise rather than relying on one.
const CLAIMED_ALIGN: u32 = 1;

/// Turns an inkwell builder failure into this crate's error.
///
/// A builder error means the IR being requested is malformed — a value of the wrong class, a
/// terminator in a sealed block — which is a bug in this translation rather than in the
/// program, so it is [`CodegenError::Internal`].
fn built<T>(result: Result<T, BuilderError>) -> Result<T, CodegenError> {
    result.map_err(|e| CodegenError::Internal(format!("llvm builder: {e}")))
}

/// Everything the translator needs that outlives one body.
/// The debug-info handles a body needs to attach a source location to its instructions (ADR-0170).
///
/// `None` on `Shared` when the module has no compilation unit — a build with no positions at all, or a
/// caller with no source map. A body then emits no locations rather than inventing a scope, which is the
/// same "omit rather than placeholder" rule the trap path follows.
#[derive(Copy, Clone)]
pub struct DebugScope<'ctx, 'a> {
    /// The module's debug-info builder, which mints a `DILocation`.
    ///
    /// A shared reference so the scope stays `Copy` and lives on an immutable `Shared`: every method used
    /// here takes `&self`, so nothing needs the builder mutably.
    pub info: &'a inkwell::debug_info::DebugInfoBuilder<'ctx>,
    /// This function's subprogram — the scope a `DILocation` hangs from.
    ///
    /// Per body rather than per module, because LLVM **rejects** a debug location whose scope is not the
    /// enclosing function's subprogram. The verifier's message for getting it wrong is
    /// `!dbg attachment points at wrong subprogram for function`, which is at least honest.
    pub subprogram: inkwell::debug_info::DISubprogram<'ctx>,
    /// The file this body's variables are declared in.
    pub file: inkwell::debug_info::DIFile<'ctx>,
    /// Per MIR slot: its source name, type DIE and line, when it has all three (ADR-0172 §1).
    ///
    /// **Precomputed by the back end**, because building a type DIE needs the back end's cache and `&mut`
    /// access to it, while declaring a variable needs the `alloca` that exists only during translation. So the
    /// two halves happen on opposite sides of this boundary and meet here.
    ///
    /// Indexed by slot, holes included: a compiler temporary has no name and gets no entry, and dropping the
    /// holes would misalign every later slot — the same trap ADR-0171 §3 records for parameters.
    pub slots: &'a [Option<(String, inkwell::debug_info::DIType<'ctx>, u32)>],
}

pub struct Shared<'ctx, 'a> {
    /// The interned types and struct fields every layout question is asked of.
    pub pool: &'a Pool,
    /// The target's pointer width, passed to `jr-pool` rather than assumed.
    pub target: TargetLayout,
    /// The LLVM function for every declared procedure.
    pub funcs: &'a FxHashMap<ProcRef, FunctionValue<'ctx>>,
    /// The global holding each string constant's bytes, keyed by the pool's own [`StrId`] so
    /// that deduplication matches the VM's `intern_strings` and the Cranelift back end's.
    pub strings: &'a FxHashMap<StrId, GlobalValue<'ctx>>,
    /// The constant global holding each compiler-emitted table's bytes (ADR-0152 §1).
    pub static_arrays: &'a FxHashMap<jr_pool::PoolId, GlobalValue<'ctx>>,
    /// Storage for each declared file-scope mutable variable, and its declared type (ADR-0186
    /// §4) — the type is kept alongside the value because a bare `PlaceBase::Global` place, with
    /// no projection, still needs to know what it denotes.
    pub globals: &'a FxHashMap<GlobalRef, (GlobalValue<'ctx>, PoolId)>,
    /// The runtime helper a trap calls, `jr_trap(message, length)`.
    pub trap_helper: FunctionValue<'ctx>,
    /// How to render a trap's source location (ADR-0020 §3).
    pub locations: &'a dyn SourceInfo,
    /// The shadow call stack the trap helper walks (ADR-0066 §1), and its live depth.
    pub shadow: (GlobalValue<'ctx>, GlobalValue<'ctx>),
    /// The read-only name global and its length for each procedure, for a backtrace frame.
    pub names: &'a FxHashMap<ProcRef, (GlobalValue<'ctx>, usize)>,
    /// How many frames the shadow stack holds, so the push can be bounds-checked.
    pub shadow_capacity: usize,
    /// Whether each declared procedure is `#foreign` (ADR-0160 part 2).
    ///
    /// The call site needs it and the signature builder does not, for the reason the Cranelift back end's
    /// copy of this field gives: a signature is built from a declaration that knows its own kind, while a
    /// *call* has only a `ProcRef`.
    pub foreign: &'a FxHashMap<ProcRef, bool>,
    /// Where to hang a source location, when the module has debug info (ADR-0170 §1).
    pub debug: Option<DebugScope<'ctx, 'a>>,
}

/// Translates one body into `function`.
///
/// `messages` is the module-wide cache of trap-message globals, keyed by content so that two
/// sites rendering the same text share one object.
///
/// # Errors
/// [`CodegenError`] when a type has no layout, a callee was never declared, or MIR contains a
/// construct this back end does not implement.
pub fn translate<'ctx>(
    context: &'ctx LlvmContext,
    module: &Module<'ctx>,
    shared: &Shared<'ctx, '_>,
    proc: ProcRef,
    body: &MirBody,
    function: FunctionValue<'ctx>,
    messages: &mut FxHashMap<String, GlobalValue<'ctx>>,
) -> Result<(), CodegenError> {
    let mut translator = Translator {
        context,
        module,
        shared,
        proc,
        body,
        function,
        builder: context.create_builder(),
        allocas: context.create_builder(),
        blocks: FxHashMap::default(),
        phis: FxHashMap::default(),
        incomings: Vec::new(),
        values: FxHashMap::default(),
        undef: FxHashSet::default(),
        slots: Vec::new(),
        current: MirSpan::Synthetic,
        messages,
        sret: None,
    };
    translator.run()
}

/// The per-body translation state.
struct Translator<'ctx, 'a> {
    context: &'ctx LlvmContext,
    module: &'a Module<'ctx>,
    shared: &'a Shared<'ctx, 'a>,
    proc: ProcRef,
    body: &'a MirBody,
    function: FunctionValue<'ctx>,
    /// Emits into the block being translated.
    builder: Builder<'ctx>,
    /// Emits into the leading `alloca` block; see the module docs.
    allocas: Builder<'ctx>,
    blocks: FxHashMap<BlockId, BasicBlock<'ctx>>,
    /// One `phi` per MIR block parameter, in the block's own parameter order.
    phis: FxHashMap<BlockId, Vec<PhiValue<'ctx>>>,
    /// Every edge's arguments, recorded as the terminators are translated and applied to the
    /// phis afterwards: a `phi` can only be completed once its predecessors exist.
    incomings: Vec<(BlockId, BasicBlock<'ctx>, Vec<BasicValueEnum<'ctx>>)>,
    values: FxHashMap<ValueId, Slot<'ctx>>,
    undef: FxHashSet<ValueId>,
    slots: Vec<PointerValue<'ctx>>,
    /// The span instructions being emitted right now belong to.
    current: MirSpan,
    /// Trap-message globals already emitted, keyed by their bytes.
    messages: &'a mut FxHashMap<String, GlobalValue<'ctx>>,
    /// The caller-allocated result pointer, for a procedure returning an aggregate
    /// (ADR-0051 §1), held as an integer address.
    sret: Option<IntValue<'ctx>>,
}

impl<'ctx> Translator<'ctx, '_> {
    /// Translates the whole body.
    fn run(&mut self) -> Result<(), CodegenError> {
        // The `alloca` block comes first and falls straight through; see the module docs.
        let alloca_block = self.context.append_basic_block(self.function, "alloca");
        self.allocas.position_at_end(alloca_block);

        let order: Vec<BlockId> = self.body.reverse_postorder().to_vec();
        for id in &order {
            let block = self
                .context
                .append_basic_block(self.function, &format!("bb{}", id.index()));
            self.blocks.insert(*id, block);
        }

        let entry = self.block(self.body.entry())?;
        self.allocas.position_at_end(alloca_block);
        built(self.allocas.build_unconditional_branch(entry))?;
        // Re-position, so later allocations are appended *before* the branch.
        let branch = alloca_block
            .get_first_instruction()
            .ok_or_else(|| CodegenError::Internal("the alloca block lost its branch".to_owned()))?;
        self.allocas.position_before(&branch);

        self.declare_slots(alloca_block)?;

        // The result pointer is the *leading* parameter, so it must be taken before the
        // ordinary ones are bound — binding from position 0 regardless would give the result
        // pointer to the first declared parameter and shift every argument by one.
        if repr::returns_via_sret(
            self.context,
            self.shared.pool,
            self.shared.target,
            self.body.ret(),
        )? {
            self.sret = self
                .function
                .get_nth_param(0)
                .map(inkwell::values::BasicValueEnum::into_int_value);
        }
        self.bind_entry_params()?;

        // Every other block's parameters are MIR's own, and each becomes a `phi` at the top
        // of the block. Created now, while the blocks are empty, so they precede every other
        // instruction — which LLVM requires.
        for id in &order {
            if *id == self.body.entry() {
                continue;
            }
            let block = self.block(*id)?;
            self.builder.position_at_end(block);
            let params = self.body.block(*id).params.clone();
            let mut phis = Vec::with_capacity(params.len());
            for value in params {
                let ty = self.body.value(value).ty;
                match Repr::of(self.context, self.shared.pool, self.shared.target, ty)?
                    .llvm_type(self.context, self.shared.target)
                {
                    Some(llvm) => {
                        let phi = built(self.builder.build_phi(llvm, "p"))?;
                        self.values.insert(value, Some(phi.as_basic_value()));
                        phis.push(phi);
                    }
                    // A `void` parameter carries nothing, so it gets no `phi` and the edges
                    // that feed it pass no argument.
                    None => {
                        self.values.insert(value, None);
                    }
                }
            }
            self.phis.insert(*id, phis);
        }

        for id in &order {
            let block = self.block(*id)?;
            self.builder.position_at_end(block);
            let data = self.body.block(*id);
            for stmt in &data.stmts {
                self.current = statement_span(stmt);
                self.mark_line();
                self.statement(stmt)?;
            }
            self.current = self.terminator_span(&data.term);
            self.mark_line();
            self.terminator(&data.term)?;
        }

        self.apply_incomings()?;
        Ok(())
    }

    /// Fills every `phi` with the values its predecessors supply.
    ///
    /// Deferred to the end because a `phi` names blocks, and an edge's predecessor is only
    /// known once that predecessor's terminator has been translated — trap blocks split a
    /// block in two, so the predecessor is not the block translation started in.
    fn apply_incomings(&mut self) -> Result<(), CodegenError> {
        for (target, pred, args) in std::mem::take(&mut self.incomings) {
            let phis = self.phis.get(&target).ok_or_else(|| {
                CodegenError::Internal(format!("bb{} has no phi list", target.index()))
            })?;
            if phis.len() != args.len() {
                return Err(CodegenError::Internal(format!(
                    "bb{} takes {} parameters but an edge supplied {}",
                    target.index(),
                    phis.len(),
                    args.len()
                )));
            }
            for (phi, arg) in phis.iter().zip(args) {
                phi.add_incoming(&[(&arg as &dyn BasicValue<'ctx>, pred)]);
            }
        }
        Ok(())
    }

    /// Allocates stack space for every MIR slot.
    ///
    /// Sizes and alignments come from [`jr_pool::layout_of`]. An `i8` array of the layout's
    /// own size, rather than a typed allocation, keeps LLVM out of the layout business
    /// (ADR-0143 §4).
    fn declare_slots(&mut self, alloca_block: BasicBlock<'ctx>) -> Result<(), CodegenError> {
        for index in 0..self.body.slot_count() {
            let slot = self.body.slot(jr_mir::SlotId::from_usize(index));
            let layout =
                layout_of(self.shared.pool, self.shared.target, slot.ty).map_err(|reason| {
                    CodegenError::NoLayout {
                        ty: slot.ty,
                        reason,
                    }
                })?;
            // A zero-sized slot is legal — `void` is storable — but an allocation of zero
            // bytes has no address to give out, and one byte is the smallest honest request.
            let bytes = u32::try_from(layout.size.max(1)).map_err(|_| {
                CodegenError::Internal(format!("slot {index} is larger than a u32"))
            })?;
            let pointer = self.alloca(bytes, layout.align, &format!("s{index}"))?;
            self.slots.push(pointer);

            // The DWARF variable for a slot that stands for a source local (ADR-0172 §1). A temporary gets
            // none, which is right: a debugger showing `s7` next to a user's own names is noise.
            if let Some(debug) = self.shared.debug
                && let Some(Some((name, die, line))) = debug.slots.get(index)
            {
                let variable = debug.info.create_auto_variable(
                    debug.subprogram.as_debug_info_scope(),
                    name,
                    debug.file,
                    *line,
                    *die,
                    // `always_preserve`: keep the variable at `-O0` even when nothing reads it, which is the
                    // whole point of debug info in an unoptimised build.
                    true,
                    0,
                    // The slot's own alignment, in bits.
                    layout.align.saturating_mul(8),
                );
                let location = debug.info.create_debug_location(
                    self.context,
                    *line,
                    0,
                    debug.subprogram.as_debug_info_scope(),
                    None,
                );
                // Declared in the **alloca block**, which dominates the whole body by construction
                // (ADR-0143 §4) — a declare must dominate every use of its variable.
                //
                // **The raw `llvm-sys` call, not inkwell's wrapper**, and this is an upstream bug rather than a
                // preference (ADR-0172 §2). LLVM 19 replaced the `llvm.dbg.declare` *intrinsic call* with a
                // debug **record**, which is not a value — and inkwell 0.9's `insert_declare_at_end` casts the
                // returned `LLVMDbgRecordRef` to an `LLVMValueRef` and wraps it in `InstructionValue::new`,
                // which asserts `is_instruction()`. Both of its insert helpers panic on LLVM 21 for that
                // reason, at a message naming inkwell's internals and no call of ours.
                //
                // The record itself is discarded: nothing here needs a handle on it, and the metadata is
                // attached to the block by the call.
                //
                // SAFETY: every pointer comes from a live inkwell wrapper whose lifetime outlives this call —
                // the builder from `DebugScope`, the variable and expression just created from it, the storage
                // from an `alloca` in this function, and the block from this body. An empty expression is the
                // correct one for a variable whose storage *is* its address.
                let expression = debug.info.create_expression(Vec::new());
                unsafe {
                    inkwell::llvm_sys::debuginfo::LLVMDIBuilderInsertDeclareRecordAtEnd(
                        debug.info.as_mut_ptr(),
                        inkwell::values::AsValueRef::as_value_ref(&pointer),
                        variable.as_mut_ptr(),
                        expression.as_mut_ptr(),
                        location.as_mut_ptr(),
                        alloca_block.as_mut_ptr(),
                    );
                }
            }
        }
        Ok(())
    }

    /// Reserves `size` bytes of stack in the `alloca` block.
    fn alloca(
        &self,
        size: u32,
        align: u32,
        name: &str,
    ) -> Result<PointerValue<'ctx>, CodegenError> {
        let ty = self.context.i8_type().array_type(size);
        let pointer = built(self.allocas.build_alloca(ty, name))?;
        pointer
            .as_instruction()
            .ok_or_else(|| CodegenError::Internal("an alloca with no instruction".to_owned()))?
            .set_alignment(align.max(1))
            .map_err(|e| CodegenError::Internal(format!("alloca alignment: {e}")))?;
        Ok(pointer)
    }

    /// Binds the function's parameters to MIR's parameter values.
    ///
    /// A `void` parameter contributes no LLVM parameter, so the two lists are walked with
    /// independent cursors rather than zipped. The context is an ordinary leading MIR
    /// parameter (ADR-0057 §4), so it needs no special case here.
    fn bind_entry_params(&mut self) -> Result<(), CodegenError> {
        let mut next = u32::from(self.sret.is_some());
        for value in self.body.params() {
            let ty = self.body.value(*value).ty;
            let repr = Repr::of(self.context, self.shared.pool, self.shared.target, ty)?;
            if repr.llvm_type(self.context, self.shared.target).is_some() {
                let param = self.function.get_nth_param(next).ok_or_else(|| {
                    CodegenError::Internal(
                        "the function has fewer parameters than the body".to_owned(),
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
                let repr = Repr::of(self.context, self.shared.pool, self.shared.target, ty)?;
                // The VM evaluates the value operand *before* the address, so a trapping
                // operand surfaces first; the order is preserved here so the engines report
                // the same failure.
                let source = self.read(*value)?;
                let address = self.address(place)?;
                self.write(address, repr, source)
            }
            // A discarded rvalue is still evaluated, deliberately: an ADR-0002 overflow in an
            // expression whose result nobody wants still traps.
            Statement::Discard { rvalue, .. } => {
                self.rvalue(rvalue, None)?;
                Ok(())
            }
            Statement::Zero { place, .. } => {
                let ty = self.place_type(place)?;
                // Straight from `layout_of` rather than from `Repr`, which carries a size
                // only for an aggregate — and `Zero` is emitted for a scalar slot too.
                let layout = layout_of(self.shared.pool, self.shared.target, ty)
                    .map_err(|reason| CodegenError::NoLayout { ty, reason })?;
                let address = self.address(place)?;
                self.memset_zero(address, layout.size, layout.align)
            }
            // ADR-0003's check. An **unsigned** compare, so a negative index — which is a
            // huge unsigned value — fails the same test that catches one past the end.
            Statement::BoundsCheck { index, len, .. } => {
                let index = self.read_int(*index)?;
                let len = self.read_int(*len)?;
                let out_of_range =
                    built(
                        self.builder
                            .build_int_compare(IntPredicate::UGE, index, len, "oob"),
                    )?;
                self.trap_if(out_of_range, TrapKind::IndexOutOfBounds)
            }
            // The tag is one byte at the variant's own address (ADR-0068 §3).
            Statement::TagCheck { place, case, .. } => {
                let address = self.address(place)?;
                let tag = self.load_int(address, self.context.i8_type(), "tag")?;
                let expected = self.context.i8_type().const_int(u64::from(*case), false);
                let wrong =
                    built(
                        self.builder
                            .build_int_compare(IntPredicate::NE, tag, expected, "bad"),
                    )?;
                self.trap_if(wrong, TrapKind::WrongVariantCase)
            }
            Statement::Nop => Ok(()),
        }
    }

    /// Translates an rvalue, returning the value it produces.
    /// An atomic's required operand, read as a 64-bit integer.
    ///
    /// `None` is a lowering bug the MIR verifier already refuses (ADR-0176 §2), so it is an internal
    /// error — and a helper, so the four arms cannot each invent a message for the same impossibility.
    fn require_atomic_operand(
        &mut self,
        operand: Option<Operand>,
    ) -> Result<inkwell::values::IntValue<'ctx>, CodegenError> {
        let operand = operand
            .ok_or_else(|| CodegenError::Internal("an atomic is missing an operand".to_owned()))?;
        self.read_int(operand)
    }

    fn rvalue(
        &mut self,
        rvalue: &Rvalue,
        dest: Option<ValueId>,
    ) -> Result<Slot<'ctx>, CodegenError> {
        match rvalue {
            // **Real machine atomics** (ADR-0176 §5), matching Cranelift's: a sequentially consistent
            // load, store, `atomicrmw add` and `cmpxchg`.
            //
            // No null check, unlike an indirect *call*: the address came from a pointer the program holds,
            // and a branch inserted before an atomic would change the very ordering it establishes.
            Rvalue::Atomic {
                op,
                address,
                value,
                expected,
            } => {
                let raw = self.read_int(*address)?;
                let pointer = built(self.builder.build_int_to_ptr(
                    raw,
                    self.context.ptr_type(AddressSpace::default()),
                    "atomicptr",
                ))?;
                let i64_ty = self.context.i64_type();
                let produced = match op {
                    jr_mir::AtomicOp::Load => {
                        let loaded = built(self.builder.build_load(i64_ty, pointer, "atomicload"))?;
                        let loaded = loaded.into_int_value();
                        // The ordering is set on the instruction after the fact, because inkwell's
                        // `build_load` has no ordering parameter — an `alignment` must be set too or LLVM
                        // rejects an ordered load.
                        if let Some(instruction) = loaded.as_instruction() {
                            instruction
                                .set_alignment(8)
                                .map_err(|e| CodegenError::Internal(format!("atomic load: {e}")))?;
                            instruction
                                .set_atomic_ordering(
                                    inkwell::AtomicOrdering::SequentiallyConsistent,
                                )
                                .map_err(|e| CodegenError::Internal(format!("atomic load: {e}")))?;
                        }
                        Some(loaded.into())
                    }
                    jr_mir::AtomicOp::Store => {
                        let operand = self.require_atomic_operand(*value)?;
                        let stored = built(self.builder.build_store(pointer, operand))?;
                        stored
                            .set_alignment(8)
                            .map_err(|e| CodegenError::Internal(format!("atomic store: {e}")))?;
                        stored
                            .set_atomic_ordering(inkwell::AtomicOrdering::SequentiallyConsistent)
                            .map_err(|e| CodegenError::Internal(format!("atomic store: {e}")))?;
                        None
                    }
                    jr_mir::AtomicOp::Add => {
                        let operand = self.require_atomic_operand(*value)?;
                        let previous = built(self.builder.build_atomicrmw(
                            inkwell::AtomicRMWBinOp::Add,
                            pointer,
                            operand,
                            inkwell::AtomicOrdering::SequentiallyConsistent,
                        ))?;
                        Some(previous.into())
                    }
                    jr_mir::AtomicOp::CompareExchange => {
                        let wanted = self.require_atomic_operand(*expected)?;
                        let new = self.require_atomic_operand(*value)?;
                        let outcome = built(self.builder.build_cmpxchg(
                            pointer,
                            wanted,
                            new,
                            inkwell::AtomicOrdering::SequentiallyConsistent,
                            // The *failure* ordering may not be stronger than the success one and may not
                            // be `Release`; sequential consistency for both is the only choice that keeps
                            // this identical to Cranelift's single-ordering instruction.
                            inkwell::AtomicOrdering::SequentiallyConsistent,
                        ))?;
                        // `cmpxchg` yields `{ value, did_swap }`; the boolean is field 1, and taking it
                        // here is what makes this produce the same `bool` the other two engines do.
                        let flag = built(self.builder.build_extract_value(outcome, 1, "swapped"))?;
                        Some(flag)
                    }
                };
                Ok(produced)
            }
            // A plain move propagates undefinedness rather than trapping, exactly as the
            // VM's `Move` clones `Value::Undefined` without inspecting it.
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
                Ok(Some(self.address_int(address, "addr")?.into()))
            }
            Rvalue::Undef => {
                if let Some(dest) = dest {
                    self.undef.insert(dest);
                }
                // A placeholder is still needed so the value exists; it is never read,
                // because every reading site checks `undef` first. A zero of the right
                // *class*, not merely the right width — a poison would be worse than a zero
                // here, because it can propagate into a value the program prints.
                let ty = dest.map_or(PoolId::VOID, |id| self.body.value(id).ty);
                let repr = Repr::of(self.context, self.shared.pool, self.shared.target, ty)?;
                Ok(repr
                    .llvm_type(self.context, self.shared.target)
                    .map(|llvm| match llvm {
                        BasicTypeEnum::FloatType(float) => float.const_zero().into(),
                        other => other.const_zero(),
                    }))
            }
        }
    }

    // -----------------------------------------------------------------------
    // Operands
    // -----------------------------------------------------------------------

    /// The value of an operand, without checking for undefinedness.
    fn operand(&mut self, operand: Operand) -> Result<Slot<'ctx>, CodegenError> {
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
    /// Traps first if it is undefined, which is what `Value::scalar` and `Value::aggregate`
    /// do in the VM.
    fn read(&mut self, operand: Operand) -> Result<Slot<'ctx>, CodegenError> {
        if let Operand::Value(value) = operand
            && self.undef.contains(&value)
        {
            self.trap(TrapKind::UninitialisedRead)?;
        }
        self.operand(operand)
    }

    /// A scalar operand that is read, as one LLVM value.
    fn read_scalar(&mut self, operand: Operand) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        self.read(operand)?.ok_or_else(|| {
            CodegenError::Internal("expected a scalar operand, found void".to_owned())
        })
    }

    /// A scalar operand that is read and is an integer.
    fn read_int(&mut self, operand: Operand) -> Result<IntValue<'ctx>, CodegenError> {
        let value = self.read_scalar(operand)?;
        value.try_into().map_err(|()| {
            CodegenError::Internal("expected an integer operand, found a float".to_owned())
        })
    }

    /// Materialises an interned constant.
    fn constant(&mut self, id: PoolId) -> Result<Slot<'ctx>, CodegenError> {
        let item = self.shared.pool.item(id).clone();
        match item {
            Item::VoidValue => Ok(None),
            Item::BoolValue(value) => Ok(Some(
                self.context
                    .i8_type()
                    .const_int(u64::from(value), false)
                    .into(),
            )),
            Item::IntValue { ty, bits } => {
                let repr = Repr::of(self.context, self.shared.pool, self.shared.target, ty)?;
                let Repr::Scalar(ScalarRepr::Int { ty: llvm, .. }) = repr else {
                    return Err(CodegenError::Internal(
                        "an integer constant whose type is not an integer".to_owned(),
                    ));
                };
                // `bits` is already normalised to the type's width, and `const_int` takes a
                // sign-agnostic bit pattern, so it is passed through unchanged.
                Ok(Some(llvm.const_int(bits, false).into()))
            }
            Item::FloatValue { ty, bits } => {
                let value = match jr_pool::FloatKind::of(self.shared.pool, ty) {
                    // A `float32`'s bits are its low 32 — `FloatKind::encode` put them there
                    // — so reading them at that width is reading them, not losing them.
                    Some(kind) if kind.bits == 32 => self
                        .context
                        .f32_type()
                        .const_float(f64::from(f32::from_bits(bits as u32))),
                    Some(_) => self.context.f64_type().const_float(f64::from_bits(bits)),
                    None => {
                        return Err(CodegenError::Internal(
                            "a float constant whose type is not a float".to_owned(),
                        ));
                    }
                };
                Ok(Some(value.into()))
            }
            Item::StrValue(str_id) => self.string_constant(str_id).map(Some),
            // A compiler-emitted table materialises as a `{data, count}` view over its global
            // (ADR-0152 §1) — the same two-word build a string gets, over a different global.
            Item::StaticArray { values, .. } => {
                let count = values.len() as u64;
                self.static_array_constant(id, count).map(Some)
            }
            // A **procedure value** is the address of its target (ADR-0059 §4), as an
            // integer like every other pointer here. Native code uses a real code address —
            // unlike the VM's encoded handle — and nothing observes the bits, so the two
            // engines need not agree on them.
            Item::ProcValue { ty: _, decl } => {
                let target = ProcRef::new(decl.file, jr_hir::ProcId::from_u32(decl.index));
                let func = *self.shared.funcs.get(&target).ok_or_else(|| {
                    CodegenError::Internal(format!(
                        "no function declared for a procedure value {target:?}"
                    ))
                })?;
                let pointer = func.as_global_value().as_pointer_value();
                Ok(Some(self.address_int(pointer, "proc")?.into()))
            }
            // An aggregate constant is materialised into stack space, element by element, and
            // its **address** is the value — exactly as a string's `{data, count}` pair is
            // (ADR-0074). The pool interned the element *values* rather than a byte image,
            // deliberately, because the pool is target-independent.
            Item::AggregateValue { ty, .. } => self.aggregate_constant(id, ty).map(Some),
            _ => Err(CodegenError::Unsupported {
                proc: self.proc,
                what: "a type or library used as a runtime value".to_owned(),
            }),
        }
    }

    /// Materialises an aggregate constant, returning its address (ADR-0074 §1).
    fn aggregate_constant(
        &mut self,
        id: PoolId,
        ty: PoolId,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let layout = layout_of(self.shared.pool, self.shared.target, ty)
            .map_err(|reason| CodegenError::NoLayout { ty, reason })?;
        let Item::AggregateValue { elements, .. } = self.shared.pool.item(id) else {
            return Err(CodegenError::Internal(
                "an aggregate constant changed shape".to_owned(),
            ));
        };
        let elements = elements.clone();

        // Element types and offsets, gathered before anything is emitted. An array's stride
        // is the element layout's size, which is the same rule `layout_of` used for the
        // array's total size, so the two cannot disagree about where element *n* begins.
        let mut placements: Vec<(PoolId, u64)> = Vec::with_capacity(elements.len());
        if let Item::ArrayType { elem, .. } = *self.shared.pool.item(ty) {
            let elem_layout = layout_of(self.shared.pool, self.shared.target, elem)
                .map_err(|reason| CodegenError::NoLayout { ty: elem, reason })?;
            for index in 0..elements.len() {
                placements.push((elem, elem_layout.size * index as u64));
            }
        } else {
            for (index, element) in elements.iter().enumerate() {
                let (offset, _) = field_offset(
                    self.shared.pool,
                    self.shared.target,
                    ty,
                    u32::try_from(index).unwrap_or(0),
                )
                .map_err(|reason| CodegenError::NoLayout { ty, reason })?;
                // The **element's own** type rather than the field's: they are the same when
                // the constant is well-formed, and this one is what was actually interned.
                placements.push((self.shared.pool.type_of(*element), offset));
            }
        }

        let size = u32::try_from(layout.size)
            .map_err(|_| CodegenError::Internal("an aggregate larger than a u32".to_owned()))?;
        let base = self.alloca(size.max(1), layout.align, "const")?;

        for (element, (elem_ty, offset)) in elements.into_iter().zip(placements) {
            let destination = self.offset(base, offset, "elem")?;
            match Repr::of(self.context, self.shared.pool, self.shared.target, elem_ty)? {
                Repr::Scalar(_) => {
                    let Some(value) = self.constant(element)? else {
                        continue;
                    };
                    self.store(destination, value, elem_ty)?;
                }
                // A **nested** aggregate's `constant` yields an *address*, so the bytes are
                // copied rather than stored — the element is already an image of itself.
                Repr::Aggregate { size, align } => {
                    let Some(source) = self.constant(element)? else {
                        continue;
                    };
                    let source = self.pointer_of(source, "src")?;
                    self.copy(destination, source, size, align)?;
                }
                // A vector element is copied like an aggregate, because a vector constant is an
                // aggregate of lanes in the pool (ADR-0148 §5 defers every way of naming one), so
                // `constant` hands back an address here too. Its size and alignment come from
                // `layout_of`, not from the `Repr`, which carries only the LLVM type — the one
                // layout computation stays the pool's (ADR-0018 §2).
                Repr::Vector { .. } => {
                    let Some(source) = self.constant(element)? else {
                        continue;
                    };
                    let elem_layout =
                        jr_pool::layout_of(self.shared.pool, self.shared.target, elem_ty).map_err(
                            |reason| CodegenError::NoLayout {
                                ty: elem_ty,
                                reason,
                            },
                        )?;
                    let source = self.pointer_of(source, "src")?;
                    self.copy(destination, source, elem_layout.size, elem_layout.align)?;
                }
                // A `void` element occupies no bytes, so it writes nothing.
                Repr::Void => {}
            }
        }
        Ok(self.address_int(base, "const_addr")?.into())
    }

    /// Builds a `{data, count}` pair for a string literal (ADR-0004).
    ///
    /// The bytes live in a read-only global, deduplicated by [`StrId`] exactly as the VM's
    /// `intern_strings` deduplicates them, and the pair itself is materialised into stack
    /// space whose address is the aggregate's value. Both field offsets come from
    /// [`jr_pool::string_data`] and [`jr_pool::string_count`], so ADR-0004 stops being prose
    /// in the same place it does for the other two engines.
    fn string_constant(&mut self, str_id: StrId) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let global = *self.shared.strings.get(&str_id).ok_or_else(|| {
            CodegenError::Internal("a string constant was not given a global".to_owned())
        })?;
        let count = self.shared.pool.resolve_str(str_id).len() as u64;

        let layout = jr_pool::string_layout(self.shared.target);
        let (data_offset, _) = string_data(self.shared.target);
        let (count_offset, _) = string_count(self.shared.target);

        let size = u32::try_from(layout.size)
            .map_err(|_| CodegenError::Internal("a string larger than a u32".to_owned()))?;
        let base = self.alloca(size.max(1), layout.align, "str")?;

        let pointer_ty = pointer_int(self.context, self.shared.target);
        let data = self.address_int(global.as_pointer_value(), "sdata")?;
        let data_place = self.offset(base, data_offset, "sdp")?;
        self.store_int(data_place, data, "sd")?;
        let count_place = self.offset(base, count_offset, "scp")?;
        self.store_int(count_place, pointer_ty.const_int(count, false), "sc")?;
        Ok(self.address_int(base, "str_addr")?.into())
    }

    /// Builds a `{data, count}` view over a compiler-emitted table (ADR-0152 §1).
    ///
    /// Deliberately the same shape as [`Translator::string_constant`] above: bytes in a constant global,
    /// a two-word descriptor built where it is used, and the `count` taken from the pool's element list
    /// rather than from the bytes so the two cannot disagree.
    fn static_array_constant(
        &mut self,
        id: jr_pool::PoolId,
        count: u64,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let global = *self.shared.static_arrays.get(&id).ok_or_else(|| {
            CodegenError::Internal("a table constant was not given a global".to_owned())
        })?;

        let layout = jr_pool::pair_layout(self.shared.target);
        let (data_offset, _) = jr_pool::pair_data(self.shared.target);
        let (count_offset, _) = jr_pool::pair_count(self.shared.target);

        let size = u32::try_from(layout.size)
            .map_err(|_| CodegenError::Internal("a view larger than a u32".to_owned()))?;
        let base = self.alloca(size.max(1), layout.align, "tbl")?;

        let pointer_ty = pointer_int(self.context, self.shared.target);
        let data = self.address_int(global.as_pointer_value(), "tdata")?;
        let data_place = self.offset(base, data_offset, "tdp")?;
        self.store_int(data_place, data, "td")?;
        let count_place = self.offset(base, count_offset, "tcp")?;
        self.store_int(count_place, pointer_ty.const_int(count, false), "tc")?;
        Ok(self.address_int(base, "tbl_addr")?.into())
    }

    // -----------------------------------------------------------------------
    // Arithmetic
    // -----------------------------------------------------------------------

    /// One elementwise vector operation (ADR-0148 §4).
    ///
    /// LLVM's arithmetic instructions are polymorphic over `<N x T>`, so this is the *same* builder
    /// call the scalar path makes with a different operand type — which is the point: a vector add is
    /// one instruction here and a loop in the VM, and the differential harness asserts they agree.
    ///
    /// **No overflow check on the integer forms**, and that is a language decision rather than an
    /// omission: sema accepts only `+% -% *%` on an integer vector (§6), because no target has a
    /// per-lane overflow flag and a trapping vector add would need a compare and a branch for every
    /// lane. So the wrapping instruction *is* the whole meaning.
    ///
    /// # Errors
    /// [`CodegenError::Internal`] for an operator sema should have refused (E0285) — integer
    /// division, a trapping integer add, or a comparison, which needs a mask type this language does
    /// not have yet (§5).
    fn vector_binary(
        &mut self,
        op: BinOp,
        vector: inkwell::types::VectorType<'ctx>,
        signed: bool,
        left: BasicValueEnum<'ctx>,
        right: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let left = left.into_vector_value();
        let right = right.into_vector_value();
        // Float lanes or integer lanes, asked of the *lane* type rather than re-derived from the
        // Jairs type: this is the same LLVM vector the `Repr` built, so the two cannot disagree.
        let float_lanes = vector.get_element_type().is_float_type();
        let _ = signed;

        let value: BasicValueEnum<'ctx> = if float_lanes {
            match op {
                BinOp::Add => built(self.builder.build_float_add(left, right, "vfadd"))?.into(),
                BinOp::Sub => built(self.builder.build_float_sub(left, right, "vfsub"))?.into(),
                BinOp::Mul => built(self.builder.build_float_mul(left, right, "vfmul"))?.into(),
                BinOp::Div => built(self.builder.build_float_div(left, right, "vfdiv"))?.into(),
                _ => {
                    return Err(CodegenError::Internal(format!(
                        "{op:?} is not defined on a float vector"
                    )));
                }
            }
        } else {
            match op {
                BinOp::WrapAdd => built(self.builder.build_int_add(left, right, "vadd"))?.into(),
                BinOp::WrapSub => built(self.builder.build_int_sub(left, right, "vsub"))?.into(),
                BinOp::WrapMul => built(self.builder.build_int_mul(left, right, "vmul"))?.into(),
                _ => {
                    return Err(CodegenError::Internal(format!(
                        "{op:?} is not defined on an integer vector"
                    )));
                }
            }
        };
        Ok(value)
    }

    /// Translates a binary operation, trapping per ADR-0002 where required.
    fn binary(
        &mut self,
        op: BinOp,
        lhs: Operand,
        rhs: Operand,
    ) -> Result<Slot<'ctx>, CodegenError> {
        let left = self.read_scalar(lhs)?;
        let right = self.read_scalar(rhs)?;
        let ty = self.operand_type(lhs);
        let repr = Repr::of(self.context, self.shared.pool, self.shared.target, ty)?;

        // **A vector before the scalar paths** (ADR-0148 §4). Its operands are `VectorValue`s, so
        // `into_int_value` below would panic rather than fail — which is what it did the first time
        // this ran, and is exactly why the arm is here and not appended after them.
        if let Repr::Vector { ty: vector, signed } = repr {
            return self
                .vector_binary(op, vector, signed, left, right)
                .map(Some);
        }

        // Floats first and separately, because every instruction differs — and, the point of
        // ADR-0040 §1, **no overflow check at all**.
        if let Repr::Scalar(ScalarRepr::Float(_)) = repr {
            let left = left.into_float_value();
            let right = right.into_float_value();
            let value: BasicValueEnum<'ctx> = match op {
                BinOp::Add => built(self.builder.build_float_add(left, right, "fadd"))?.into(),
                BinOp::Sub => built(self.builder.build_float_sub(left, right, "fsub"))?.into(),
                BinOp::Mul => built(self.builder.build_float_mul(left, right, "fmul"))?.into(),
                // No zero check: `x / 0.0` is `inf` and `0.0 / 0.0` is `NaN`, which
                // ADR-0040 §1 makes values rather than failures.
                BinOp::Div => built(self.builder.build_float_div(left, right, "fdiv"))?.into(),
                BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                    let predicate = float_predicate(op).ok_or_else(|| {
                        CodegenError::Internal("a comparison with no float predicate".to_owned())
                    })?;
                    let bit = built(
                        self.builder
                            .build_float_compare(predicate, left, right, "fcmp"),
                    )?;
                    self.bool_of(bit)?.into()
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

        let signed = matches!(repr, Repr::Scalar(ScalarRepr::Int { signed: true, .. }));
        let left = left.into_int_value();
        let right = right.into_int_value();
        let value: BasicValueEnum<'ctx> = match op {
            // ADR-0002: `+`, `-`, `*` trap rather than wrap.
            BinOp::Add => self.checked(signed, left, right, Arith::Add)?.into(),
            BinOp::Sub => self.checked(signed, left, right, Arith::Sub)?.into(),
            BinOp::Mul => self.checked(signed, left, right, Arith::Mul)?.into(),
            // The documented opt-out.
            BinOp::WrapAdd => built(self.builder.build_int_add(left, right, "wadd"))?.into(),
            BinOp::WrapSub => built(self.builder.build_int_sub(left, right, "wsub"))?.into(),
            BinOp::WrapMul => built(self.builder.build_int_mul(left, right, "wmul"))?.into(),
            BinOp::Div | BinOp::Rem => self.division(op, signed, left, right)?.into(),
            BinOp::BitAnd => built(self.builder.build_and(left, right, "and"))?.into(),
            BinOp::BitOr => built(self.builder.build_or(left, right, "or"))?.into(),
            BinOp::BitXor => built(self.builder.build_xor(left, right, "xor"))?.into(),
            BinOp::Shl | BinOp::Shr => self.shift(op, signed, left, right)?.into(),
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let predicate = int_predicate(op, signed);
                let bit = built(
                    self.builder
                        .build_int_compare(predicate, left, right, "cmp"),
                )?;
                self.bool_of(bit)?.into()
            }
        };
        Ok(Some(value))
    }

    /// Widens an LLVM `i1` to the byte a Jairs `bool` occupies.
    ///
    /// LLVM's comparisons produce one *bit*; a `bool` is one *byte* holding 0 or 1
    /// (`layout_of` says so, and the VM stores it that way). Zero-extending here is what
    /// keeps a comparison's result storable into a `bool` field.
    fn bool_of(&self, bit: IntValue<'ctx>) -> Result<IntValue<'ctx>, CodegenError> {
        built(
            self.builder
                .build_int_z_extend(bit, self.context.i8_type(), "b"),
        )
    }

    /// Emits a shift, checking the count first (ADR-0042 §3).
    ///
    /// **A shift count at or past the width is poison in LLVM**, where Cranelift masks it and
    /// the VM traps. Poison is worse than either: it can propagate into a printed value, and
    /// an optimiser may assume it never happens. So the count is compared against the width
    /// and the out-of-range case traps, which is what ADR-0042 §3 chose.
    ///
    /// The comparison is **unsigned**, which catches a negative count in the same test: a
    /// negative count reinterpreted as unsigned is enormous.
    fn shift(
        &mut self,
        op: BinOp,
        signed: bool,
        left: IntValue<'ctx>,
        right: IntValue<'ctx>,
    ) -> Result<IntValue<'ctx>, CodegenError> {
        let width = left.get_type().get_bit_width();
        let limit = right.get_type().const_int(u64::from(width), false);
        let out_of_range =
            built(
                self.builder
                    .build_int_compare(IntPredicate::UGE, right, limit, "sh"),
            )?;
        self.trap_if(out_of_range, TrapKind::ShiftOutOfRange)?;
        match (op, signed) {
            (BinOp::Shl, _) => built(self.builder.build_left_shift(left, right, "shl")),
            // `ashr` for a signed type and `lshr` otherwise, which is what makes `>>`
            // arithmetic for `s8` and logical for `u8` (ADR-0042 §2).
            (BinOp::Shr, sign) => built(self.builder.build_right_shift(left, right, sign, "shr")),
            _ => Err(CodegenError::Internal(
                "shift called for a non-shift operator".to_owned(),
            )),
        }
    }

    /// Emits a checked add, subtract or multiply through LLVM's overflow intrinsics.
    ///
    /// The intrinsics return `{value, i1}`, so ADR-0002's "trap, never wrap" is one branch on
    /// the second element — and the pair comes from one operation, which a hand-rolled test
    /// after a plain `add` would not tell LLVM.
    fn checked(
        &mut self,
        signed: bool,
        left: IntValue<'ctx>,
        right: IntValue<'ctx>,
        op: Arith,
    ) -> Result<IntValue<'ctx>, CodegenError> {
        let name = op.intrinsic(signed);
        let intrinsic = Intrinsic::find(name)
            .ok_or_else(|| CodegenError::Internal(format!("no intrinsic {name}")))?;
        let declaration = intrinsic
            .get_declaration(self.module, &[left.get_type().into()])
            .ok_or_else(|| CodegenError::Internal(format!("cannot declare {name}")))?;
        let call = built(self.builder.build_call(
            declaration,
            &[left.into(), right.into()],
            "ovf",
        ))?;
        let pair = call
            .try_as_basic_value()
            .basic()
            .ok_or_else(|| CodegenError::Internal(format!("{name} returned nothing")))?
            .into_struct_value();
        let value = built(self.builder.build_extract_value(pair, 0, "v"))?.into_int_value();
        let overflowed = built(self.builder.build_extract_value(pair, 1, "o"))?.into_int_value();
        self.trap_if(overflowed, op.trap_kind())?;
        Ok(value)
    }

    /// Emits a division or remainder with its two ADR-0002 checks.
    ///
    /// Both checks are mandatory rather than defensive: division by zero is **undefined** in
    /// LLVM, and so is `INT_MIN / -1`. The VM catches both through its range check, so
    /// without these the three engines would disagree about a program whose behaviour is
    /// specified.
    fn division(
        &mut self,
        op: BinOp,
        signed: bool,
        left: IntValue<'ctx>,
        right: IntValue<'ctx>,
    ) -> Result<IntValue<'ctx>, CodegenError> {
        let zero = right.get_type().const_zero();
        let by_zero = built(
            self.builder
                .build_int_compare(IntPredicate::EQ, right, zero, "dz"),
        )?;
        self.trap_if(by_zero, TrapKind::DivideByZero)?;

        if signed {
            let ty = left.get_type();
            let min = ty.const_int(min_bits(ty), false);
            let minus_one = ty.const_all_ones();
            let is_min = built(
                self.builder
                    .build_int_compare(IntPredicate::EQ, left, min, "dm"),
            )?;
            let is_minus_one =
                built(
                    self.builder
                        .build_int_compare(IntPredicate::EQ, right, minus_one, "d1"),
                )?;
            let both = built(self.builder.build_and(is_min, is_minus_one, "dmo"))?;
            let kind = if matches!(op, BinOp::Rem) {
                TrapKind::OverflowRem
            } else {
                TrapKind::OverflowDiv
            };
            self.trap_if(both, kind)?;
        }

        match (op, signed) {
            (BinOp::Div, true) => built(self.builder.build_int_signed_div(left, right, "sdiv")),
            (BinOp::Div, false) => built(self.builder.build_int_unsigned_div(left, right, "udiv")),
            (BinOp::Rem, true) => built(self.builder.build_int_signed_rem(left, right, "srem")),
            (BinOp::Rem, false) => built(self.builder.build_int_unsigned_rem(left, right, "urem")),
            _ => Err(CodegenError::Internal(
                "division called for a non-division operator".to_owned(),
            )),
        }
    }

    /// Translates a unary operation.
    fn unary(&mut self, op: UnOp, operand: Operand) -> Result<Slot<'ctx>, CodegenError> {
        let value = self.read_scalar(operand)?;
        let ty = self.operand_type(operand);
        let repr = Repr::of(self.context, self.shared.pool, self.shared.target, ty)?;

        // A float negation flips the sign bit and cannot fail, which is exactly where it
        // differs from an integer's: `-MIN` is one past the maximum and traps (ADR-0002),
        // while `-0.0` is a real value (ADR-0040 §1). A negation rather than a subtract from
        // zero, because `0.0 - 0.0` is `+0.0` and would lose the sign.
        if let Repr::Scalar(ScalarRepr::Float(_)) = repr {
            return match op {
                UnOp::Neg => Ok(Some(
                    built(
                        self.builder
                            .build_float_neg(value.into_float_value(), "fneg"),
                    )?
                    .into(),
                )),
                UnOp::Not | UnOp::BitNot => Err(CodegenError::Internal(
                    "`!` or `~` on a floating-point operand".to_owned(),
                )),
            };
        }

        let signed = matches!(repr, Repr::Scalar(ScalarRepr::Int { signed: true, .. }));
        let value = value.into_int_value();
        let result = match op {
            UnOp::Neg => {
                if signed {
                    // Negating the most negative value overflows (ADR-0002).
                    let ty = value.get_type();
                    let min = ty.const_int(min_bits(ty), false);
                    let is_min =
                        built(
                            self.builder
                                .build_int_compare(IntPredicate::EQ, value, min, "nm"),
                        )?;
                    self.trap_if(is_min, TrapKind::OverflowNeg)?;
                }
                built(self.builder.build_int_neg(value, "neg"))?
            }
            // `bool` is stored as 0 or 1, so `!` is a comparison against zero rather than a
            // bitwise complement, which would produce 0xFE.
            UnOp::Not => {
                let zero = value.get_type().const_zero();
                let bit =
                    built(
                        self.builder
                            .build_int_compare(IntPredicate::EQ, value, zero, "not"),
                    )?;
                self.bool_of(bit)?
            }
            // `~` *is* the bitwise complement `!` must not be, on the operand's own width, so
            // a `u8` complements within 8 bits (ADR-0042 §4).
            UnOp::BitNot => built(self.builder.build_not(value, "bnot"))?,
        };
        Ok(Some(result.into()))
    }

    /// Translates a `cast(T, x)` (ADR-0037 §2).
    ///
    /// Four directions (ADR-0040 §3), and `from` is recorded because sign extension cannot be
    /// decided from the destination. Never traps, matching ADR-0037 §2 and the interpreter.
    fn convert(
        &mut self,
        operand: Operand,
        from: NumKind,
        dest: Option<ValueId>,
    ) -> Result<Slot<'ctx>, CodegenError> {
        let value = self.read_scalar(operand)?;
        let target_ty = match dest {
            Some(dest) => self.body.value(dest).ty,
            // No destination means nothing reads the result; the conversion is then a no-op
            // rather than a guess at a width.
            None => return Ok(Some(value)),
        };
        let to = NumKind::of(self.shared.pool, target_ty)
            .ok_or_else(|| CodegenError::Internal("a cast to a non-numeric type".to_owned()))?;
        let repr = Repr::of(
            self.context,
            self.shared.pool,
            self.shared.target,
            target_ty,
        )?;

        let result: BasicValueEnum<'ctx> = match (from, to) {
            (NumKind::Int(from_kind), NumKind::Int(_)) => {
                let Repr::Scalar(ScalarRepr::Int { ty: target, .. }) = repr else {
                    return Err(CodegenError::Internal(
                        "an integer cast to a non-integer".to_owned(),
                    ));
                };
                let value = value.into_int_value();
                let source = value.get_type().get_bit_width();
                match target.get_bit_width().cmp(&source) {
                    std::cmp::Ordering::Less => {
                        built(self.builder.build_int_truncate(value, target, "trunc"))?.into()
                    }
                    std::cmp::Ordering::Equal => value.into(),
                    std::cmp::Ordering::Greater => {
                        if from_kind.signed {
                            built(self.builder.build_int_s_extend(value, target, "sext"))?.into()
                        } else {
                            built(self.builder.build_int_z_extend(value, target, "zext"))?.into()
                        }
                    }
                }
            }
            (NumKind::Int(from_kind), NumKind::Float(_)) => {
                let Repr::Scalar(ScalarRepr::Float(target)) = repr else {
                    return Err(CodegenError::Internal(
                        "an int-to-float cast to a non-float".to_owned(),
                    ));
                };
                let value = value.into_int_value();
                if from_kind.signed {
                    built(
                        self.builder
                            .build_signed_int_to_float(value, target, "sitofp"),
                    )?
                    .into()
                } else {
                    built(
                        self.builder
                            .build_unsigned_int_to_float(value, target, "uitofp"),
                    )?
                    .into()
                }
            }
            (NumKind::Float(_), NumKind::Int(to_kind)) => {
                let Repr::Scalar(ScalarRepr::Int { ty: target, .. }) = repr else {
                    return Err(CodegenError::Internal(
                        "a float-to-int cast to a non-integer".to_owned(),
                    ));
                };
                // The **saturating** intrinsics, matching `jr_pool::float_to_int` and
                // ADR-0040 §4. A plain `fptosi` is **poison** out of range, which would put a
                // silent wrong answer where the VM clamps — and unlike a trap, poison is not
                // even reliably observable.
                let name = if to_kind.signed {
                    "llvm.fptosi.sat"
                } else {
                    "llvm.fptoui.sat"
                };
                let intrinsic = Intrinsic::find(name)
                    .ok_or_else(|| CodegenError::Internal(format!("no intrinsic {name}")))?;
                let float = value.into_float_value();
                let declaration = intrinsic
                    .get_declaration(self.module, &[target.into(), float.get_type().into()])
                    .ok_or_else(|| CodegenError::Internal(format!("cannot declare {name}")))?;
                built(self.builder.build_call(declaration, &[float.into()], "sat"))?
                    .try_as_basic_value()
                    .basic()
                    .ok_or_else(|| CodegenError::Internal(format!("{name} returned nothing")))?
            }
            (NumKind::Float(_), NumKind::Float(_)) => {
                let Repr::Scalar(ScalarRepr::Float(target)) = repr else {
                    return Err(CodegenError::Internal(
                        "a float cast to a non-float".to_owned(),
                    ));
                };
                let value = value.into_float_value();
                let source = value.get_type();
                // `float64` → `float32` rounds to nearest and saturates to `inf`
                // (ADR-0040 §4); `float32` → `float64` is exact, always.
                if target.size_of() == source.size_of() {
                    value.into()
                } else {
                    built(self.builder.build_float_cast(value, target, "fcast"))?.into()
                }
            }
        };
        Ok(Some(result))
    }

    // -----------------------------------------------------------------------
    // Calls
    // -----------------------------------------------------------------------

    /// Emits a call, allocating the result slot when the callee returns an aggregate.
    ///
    /// `dest` gives the result's type, which decides whether the hidden result pointer is
    /// passed — the *same* question the callee's function type asked, through the shared
    /// [`repr::returns_via_sret`], so caller and callee cannot disagree about the parameter
    /// count (ADR-0051 §1).
    fn call(
        &mut self,
        callee: &Callee,
        args: &[Operand],
        dest: Option<ValueId>,
    ) -> Result<Slot<'ctx>, CodegenError> {
        let ret_ty = dest.map_or(PoolId::VOID, |id| self.body.value(id).ty);
        // **Whether this call crosses a C boundary** (ADR-0160 part 2), which changes the return convention
        // and how an aggregate argument travels. Only a direct call can be foreign (ADR-0059 §5).
        let crosses_c = match callee {
            Callee::Direct(target) => self.shared.foreign.get(target).copied().unwrap_or(false),
            Callee::Indirect(_) => false,
        };
        // A classified C aggregate return comes back **in registers**, so it takes no leading pointer even
        // though `returns_via_sret` — which describes Jairs's own convention — says an aggregate does.
        let c_return_in_registers = crosses_c
            && matches!(
                jr_pool::classify(self.shared.pool, self.shared.target, ret_ty),
                Ok(Some(
                    jr_pool::Class::Integer { .. } | jr_pool::Class::Float { .. }
                ))
            );
        let via_sret = !c_return_in_registers
            && repr::returns_via_sret(self.context, self.shared.pool, self.shared.target, ret_ty)?;

        let mut values: Vec<BasicMetadataValueEnum<'ctx>> = Vec::with_capacity(args.len() + 1);
        // A **fresh** slot per call, copied out of afterwards rather than passing the
        // destination's own address (ADR-0051 §2). One extra copy, deliberately: passing the
        // destination directly would let a callee that traps halfway leave the caller's
        // variable half-assigned, and ADR-0002's traps are real control flow.
        let result_slot = if via_sret {
            let layout = layout_of(self.shared.pool, self.shared.target, ret_ty)
                .map_err(|reason| CodegenError::NoLayout { ty: ret_ty, reason })?;
            let size = u32::try_from(layout.size.max(1)).map_err(|_| {
                CodegenError::Internal("a call result is larger than a u32".to_owned())
            })?;
            let slot = self.alloca(size, layout.align, "ret")?;
            let address = self.address_int(slot, "ret_addr")?;
            values.push(address.into());
            Some(address)
        } else {
            None
        };

        for arg in args {
            let ty = self.operand_type(*arg);
            if crosses_c
                && Repr::of(self.context, self.shared.pool, self.shared.target, ty)?.is_aggregate()
                && let Some(address) = self.read(*arg)?
            {
                // The aggregate's address is a machine word here, as every Jairs aggregate value is.
                self.push_aggregate_pieces(address.into_int_value(), ty, &mut values)?;
                continue;
            }
            if let Some(value) = self.read(*arg)? {
                values.push(value.into());
            }
        }

        // **The shadow-stack push, before the call** (ADR-0066 §1), matching the VM, which
        // pushes in `Vm::call`. Only a *direct* call can be pushed: an indirect one's target
        // is a runtime pointer and the name is a compile-time constant, so an indirect frame
        // is absent — exactly as an inlined one is, and for the same honest reason.
        let pushed = match callee {
            Callee::Direct(target) => self.push_frame(*target)?,
            Callee::Indirect(_) => false,
        };

        let call = match callee {
            Callee::Direct(target) => {
                let func = *self
                    .shared
                    .funcs
                    .get(target)
                    .ok_or(CodegenError::Undeclared(*target))?;
                built(self.builder.build_call(func, &values, "call"))?
            }
            // A pointer value plus a function type built from the callee operand's own
            // `ProcType` (ADR-0059 §4), by the same `repr::function_type` a declaration uses.
            Callee::Indirect(operand) => {
                let fn_type = self.indirect_type(*operand)?;
                let address = self.read_int(*operand)?;
                // **A null pointer traps rather than jumping to address zero** (ADR-0110 §1),
                // which is the ordinary mistake of using `context.allocator` before
                // installing one. Checked so all three engines raise the *same* trap.
                let zero = address.get_type().const_zero();
                let is_null = built(self.builder.build_int_compare(
                    IntPredicate::EQ,
                    address,
                    zero,
                    "nullcall",
                ))?;
                self.trap_if(is_null, TrapKind::NullCall)?;
                let pointer = built(self.builder.build_int_to_ptr(
                    address,
                    self.context.ptr_type(AddressSpace::default()),
                    "callee",
                ))?;
                built(
                    self.builder
                        .build_indirect_call(fn_type, pointer, &values, "icall"),
                )?
            }
        };

        // The pop, after the call returns. A callee that traps never returns here — the
        // helper calls `exit` — so the depth it left behind is exactly the chain the trap
        // should report, which is why skipping the pop on that path is correct.
        if pushed {
            self.pop_frame()?;
        }

        // An `sret` call returns nothing; the result *is* the slot, and an aggregate value is
        // represented by its address.
        if let Some(address) = result_slot {
            return Ok(Some(address.into()));
        }
        // **A C aggregate returned in registers is stored back into a fresh slot** (ADR-0160 part 2),
        // because a Jairs aggregate value is its address. LLVM hands the struct back as one value, so the
        // members are extracted and stored at the offsets `push_aggregate_pieces` reads from.
        if c_return_in_registers {
            let returned = call.try_as_basic_value().basic().ok_or_else(|| {
                CodegenError::Internal(String::from(
                    "a `#foreign` aggregate return produced no value",
                ))
            })?;
            return self.store_aggregate_pieces(ret_ty, returned).map(Some);
        }
        Ok(call.try_as_basic_value().basic())
    }

    /// Loads a classified aggregate's register pieces from `address` and appends them to `values`.
    ///
    /// **Whole words from the start**, matching the Cranelift back end byte for byte: the classification
    /// counts words from the layout's *size*, so a `{ s64, u8 }` is two registers with one meaningful byte in
    /// the second (ADR-0160 §4). A float class loads one member per register at its own stride, which is what
    /// makes an HFA work.
    fn push_aggregate_pieces(
        &mut self,
        address: IntValue<'ctx>,
        ty: PoolId,
        values: &mut Vec<BasicMetadataValueEnum<'ctx>>,
    ) -> Result<(), CodegenError> {
        let pointer = built(self.builder.build_int_to_ptr(
            address,
            self.context.ptr_type(AddressSpace::default()),
            "agg",
        ))?;
        match jr_pool::classify(self.shared.pool, self.shared.target, ty) {
            Ok(Some(jr_pool::Class::Integer { words })) => {
                let word = repr::pointer_int(self.context, self.shared.target);
                for index in 0..words {
                    let piece = self.load_piece(pointer, word.into(), u64::from(index) * 8)?;
                    values.push(piece.into());
                }
                Ok(())
            }
            Ok(Some(jr_pool::Class::Float { kind, count })) => {
                let member: BasicTypeEnum<'ctx> = if kind.bits == 32 {
                    self.context.f32_type().into()
                } else {
                    self.context.f64_type().into()
                };
                let stride = u64::from(kind.bits / 8);
                for index in 0..count {
                    let piece = self.load_piece(pointer, member, u64::from(index) * stride)?;
                    values.push(piece.into());
                }
                Ok(())
            }
            // Unreachable: the signature builder refused `Memory` before a body was lowered, and E0286
            // refused it before that.
            _ => Err(CodegenError::Internal(String::from(
                "an aggregate reached a `#foreign` call site with no register class",
            ))),
        }
    }

    /// One register piece, loaded from `pointer` at `offset`.
    fn load_piece(
        &mut self,
        pointer: inkwell::values::PointerValue<'ctx>,
        ty: BasicTypeEnum<'ctx>,
        offset: u64,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let at = if offset == 0 {
            pointer
        } else {
            let byte = self.context.i8_type();
            // SAFETY: the offset is inside the aggregate's slot, whose size the classification bounded to the
            // pieces it asked for — the same bound `store_aggregate_pieces` allocates to.
            unsafe {
                built(self.builder.build_gep(
                    byte,
                    pointer,
                    &[self.context.i64_type().const_int(offset, false)],
                    "piece_at",
                ))?
            }
        };
        built(self.builder.build_load(ty, at, "piece"))
    }

    /// Stores a register-returned aggregate's members into a fresh slot and answers its address.
    ///
    /// The mirror of [`Self::push_aggregate_pieces`], written beside it because the offsets have to agree.
    /// LLVM returns the struct as one value, so each member is *extracted* rather than read from a result
    /// list — the one place the two native back ends differ in shape while agreeing on the ABI.
    fn store_aggregate_pieces(
        &mut self,
        ty: PoolId,
        returned: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let layout = layout_of(self.shared.pool, self.shared.target, ty)
            .map_err(|reason| CodegenError::NoLayout { ty, reason })?;
        let class = match jr_pool::classify(self.shared.pool, self.shared.target, ty) {
            Ok(Some(class)) => class,
            _ => {
                return Err(CodegenError::Internal(String::from(
                    "an aggregate returned from a `#foreign` call with no register class",
                )));
            }
        };
        let (count, stride) = match class {
            jr_pool::Class::Integer { words } => (words, 8_u64),
            jr_pool::Class::Float { kind, count } => (count, u64::from(kind.bits / 8)),
            jr_pool::Class::Memory => {
                return Err(CodegenError::Internal(String::from(
                    "an aggregate returned from a `#foreign` call with no register class",
                )));
            }
        };
        // Rounded up to the pieces' total, so storing the last whole word cannot write past the slot.
        let pieces = u64::from(count) * stride;
        let size = u32::try_from(layout.size.max(1).max(pieces))
            .map_err(|_| CodegenError::Internal("a call result is larger than a u32".to_owned()))?;
        let slot = self.alloca(size, layout.align, "cagg")?;
        let address = self.address_int(slot, "cagg_addr")?;
        let pointer = built(self.builder.build_int_to_ptr(
            address,
            self.context.ptr_type(AddressSpace::default()),
            "cagg_ptr",
        ))?;
        for index in 0..count {
            let member = built(self.builder.build_extract_value(
                returned.into_struct_value(),
                index,
                "member",
            ))?;
            let offset = u64::from(index) * stride;
            let at = if offset == 0 {
                pointer
            } else {
                let byte = self.context.i8_type();
                // SAFETY: bounded by the slot allocated just above, whose size is the pieces' total.
                unsafe {
                    built(self.builder.build_gep(
                        byte,
                        pointer,
                        &[self.context.i64_type().const_int(offset, false)],
                        "member_at",
                    ))?
                }
            };
            built(self.builder.build_store(at, member))?;
        }
        Ok(address.into())
    }

    /// Writes `target`'s name onto the shadow call stack and increments the depth
    /// (ADR-0066 §1).
    ///
    /// Returns whether anything was pushed: `false` for a procedure with no known name, whose
    /// frame is omitted rather than rendered as a placeholder.
    ///
    /// **Bounds-checked**, because a static array written past its end is memory corruption
    /// that would be blamed on the program. Past the capacity the write is skipped and the
    /// depth still rises, so the *count* stays honest while the entries stop.
    fn push_frame(&mut self, target: ProcRef) -> Result<bool, CodegenError> {
        let Some((name_global, name_len)) = self.shared.names.get(&target).copied() else {
            return Ok(false);
        };
        let word = pointer_int(self.context, self.shared.target);
        let width = u64::from(self.shared.target.pointer_size);
        let (stack, depth_global) = self.shared.shadow;

        let depth_addr = depth_global.as_pointer_value();
        let depth = self.load_int(depth_addr, word, "d")?;

        let capacity = word.const_int(self.shared.shadow_capacity as u64, false);
        let in_range =
            built(
                self.builder
                    .build_int_compare(IntPredicate::ULT, depth, capacity, "room"),
            )?;
        let write_block = self.context.append_basic_block(self.function, "frame");
        let after = self.context.append_basic_block(self.function, "framed");
        built(
            self.builder
                .build_conditional_branch(in_range, write_block, after),
        )?;

        self.builder.position_at_end(write_block);
        // Each frame occupies two pointer-sized words: the name's address, then its length.
        let stride = word.const_int(width * 2, false);
        let byte_offset = built(self.builder.build_int_mul(depth, stride, "off"))?;
        let entry = built(unsafe {
            self.builder.build_in_bounds_gep(
                self.context.i8_type(),
                stack.as_pointer_value(),
                &[byte_offset],
                "entry",
            )
        })?;
        let name = self.address_int(name_global.as_pointer_value(), "name")?;
        self.store_int(entry, name, "fname")?;
        let len_place = self.offset(entry, width, "lenp")?;
        self.store_int(len_place, word.const_int(name_len as u64, false), "flen")?;
        built(self.builder.build_unconditional_branch(after))?;

        self.builder.position_at_end(after);
        let one = word.const_int(1, false);
        let bumped = built(self.builder.build_int_add(depth, one, "d1"))?;
        self.store_int(depth_addr, bumped, "sd")?;
        Ok(true)
    }

    /// Decrements the shadow call stack's depth, undoing one [`Self::push_frame`].
    fn pop_frame(&mut self) -> Result<(), CodegenError> {
        let word = pointer_int(self.context, self.shared.target);
        let (_, depth_global) = self.shared.shadow;
        let depth_addr = depth_global.as_pointer_value();
        let depth = self.load_int(depth_addr, word, "d")?;
        let one = word.const_int(1, false);
        let dropped = built(self.builder.build_int_sub(depth, one, "dm1"))?;
        self.store_int(depth_addr, dropped, "sd")?;
        Ok(())
    }

    /// The LLVM function type for a call through a procedure pointer (ADR-0059 §4).
    ///
    /// Built from the callee operand's `Item::ProcType` by the same [`repr::function_type`]
    /// that builds a declaration's, or the two would disagree about the parameter count —
    /// which is the silent-shift failure `returns_via_sret` exists to prevent.
    fn indirect_type(
        &self,
        operand: Operand,
    ) -> Result<inkwell::types::FunctionType<'ctx>, CodegenError> {
        let proc_ty = self.operand_type(operand);
        let Item::ProcType {
            params,
            ret,
            context,
            ..
        } = self.shared.pool.item(proc_ty)
        else {
            return Err(CodegenError::Internal(
                "an indirect call whose callee is not of procedure type".to_owned(),
            ));
        };
        let params = params.clone();
        let ret = *ret;
        // **The context parameter comes from the callee's type** (ADR-0175 §2), matching Cranelift and
        // the VM. LLVM needs no calling-convention flag here: a `#c_call` procedure is already emitted
        // with C's convention at its *declaration*, and an indirect call adopts the pointee's — so only
        // the hidden parameter differs, and getting it wrong puts the context where C expects the first
        // real argument.
        let takes_context = *context != jr_pool::ContextKind::CCall;
        let proc = self.proc;
        repr::function_type(
            self.context,
            self.shared.pool,
            self.shared.target,
            &params,
            ret,
            false,
            takes_context,
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
    /// Every offset is asked of [`jr_pool`]. The two `Deref`s are deliberately different, and
    /// getting them the same way round is the easiest mistake here: a [`PlaceBase::Deref`]
    /// reads its pointer out of a **register**, while a [`Projection::Deref`] reads one out of
    /// **memory**.
    fn address(&mut self, place: &Place) -> Result<PointerValue<'ctx>, CodegenError> {
        let (mut address, mut ty) = match &place.base {
            PlaceBase::Slot(slot) => {
                let pointer = *self
                    .slots
                    .get(slot.index())
                    .ok_or_else(|| CodegenError::Internal(format!("no slot s{}", slot.index())))?;
                (pointer, self.body.slot(*slot).ty)
            }
            PlaceBase::Deref(operand) => {
                let value = self.read_int(*operand)?;
                let pointee = self.pointee(self.operand_type(*operand))?;
                (self.pointer_of(value.into(), "deref")?, pointee)
            }
            // A global's storage is a memory root exactly like a slot's `alloca` — the same
            // reason `Place::global` carries no projection of its own — so its address is just
            // the global's own pointer, and every later projection step runs unchanged.
            PlaceBase::Global(global) => {
                let (value, ty) = *self.shared.globals.get(global).ok_or_else(|| {
                    CodegenError::Internal(format!(
                        "global g{} in file {} was referenced without being declared",
                        global.item.index(),
                        global.file.index()
                    ))
                })?;
                (value.as_pointer_value(), ty)
            }
        };

        for step in &place.projection {
            match step {
                Projection::Field(index) => {
                    let (offset, _) =
                        field_offset(self.shared.pool, self.shared.target, ty, *index)
                            .map_err(|reason| CodegenError::NoLayout { ty, reason })?;
                    address = self.offset(address, offset, "field")?;
                    ty = self.field_type(ty, *index)?;
                }
                Projection::Index(index) => {
                    // An array place is indexed in place; a **pointer** place — a view's
                    // `data` word — is loaded first and then indexed. One stride computation
                    // for both, so an array element and a view element cannot land at
                    // different addresses in the three engines.
                    let elem = self.index_elem(ty)?;
                    if let Item::PointerType(_) = self.shared.pool.item(ty) {
                        let word = pointer_int(self.context, self.shared.target);
                        let loaded = self.load_int(address, word, "data")?;
                        address = self.pointer_of(loaded.into(), "dataptr")?;
                    }
                    // The stride is the element size rounded up to its alignment, the same
                    // computation `layout_of` uses for the array's total size — so an element
                    // address here and the array's size there come from one rule.
                    let layout = layout_of(self.shared.pool, self.shared.target, elem)
                        .map_err(|reason| CodegenError::NoLayout { ty: elem, reason })?;
                    let stride = layout.size.next_multiple_of(u64::from(layout.align));
                    let index = self.read_int(*index)?;
                    let word = pointer_int(self.context, self.shared.target);
                    let index = self.widen_index(index, word)?;
                    let scaled = built(self.builder.build_int_mul(
                        index,
                        word.const_int(stride, false),
                        "scaled",
                    ))?;
                    address = built(unsafe {
                        self.builder.build_in_bounds_gep(
                            self.context.i8_type(),
                            address,
                            &[scaled],
                            "elem",
                        )
                    })?;
                    ty = elem;
                }
                Projection::Deref => {
                    let word = pointer_int(self.context, self.shared.target);
                    let loaded = self.load_int(address, word, "p")?;
                    address = self.pointer_of(loaded.into(), "pderef")?;
                    ty = self.pointee(ty)?;
                }
                Projection::StringData => {
                    let (offset, _) = string_data(self.shared.target);
                    address = self.offset(address, offset, "sdata")?;
                    ty = PoolId::PTR_U8;
                }
                Projection::StringCount => {
                    let (offset, _) = string_count(self.shared.target);
                    address = self.offset(address, offset, "scount")?;
                    ty = PoolId::S64;
                }
                // The same offsets a string's two words have — one shared computation, so no
                // two engines can disagree about a view's layout (ADR-0044 §1). The result
                // *type* differs: `*T`, not `*u8`, which gives an index the right stride.
                Projection::ViewData => {
                    let elem = self.view_elem(ty)?;
                    let (offset, _) = jr_pool::pair_data(self.shared.target);
                    address = self.offset(address, offset, "vdata")?;
                    ty = self.pointer_to(elem, "a view's element")?;
                }
                Projection::ViewCount => {
                    let (offset, _) = jr_pool::pair_count(self.shared.target);
                    address = self.offset(address, offset, "vcount")?;
                    ty = PoolId::S64;
                }
                // A dynamic array's three projections (ADR-0136 §1).
                Projection::DynamicArrayData => {
                    let elem = self.dynamic_array_elem(ty)?;
                    let (offset, _) = jr_pool::pair_data(self.shared.target);
                    address = self.offset(address, offset, "ddata")?;
                    ty = self.pointer_to(elem, "a `[..]T`'s element")?;
                }
                Projection::DynamicArrayCount => {
                    let (offset, _) = jr_pool::pair_count(self.shared.target);
                    address = self.offset(address, offset, "dcount")?;
                    ty = PoolId::S64;
                }
                Projection::DynamicArrayCapacity => {
                    let (offset, _) = jr_pool::triple_capacity(self.shared.target);
                    address = self.offset(address, offset, "dcap")?;
                    ty = PoolId::S64;
                }
                // The tag is the leading field, so its offset is 0 and the address is
                // unchanged (ADR-0068 §3). Only the type moves, to `u8`.
                Projection::VariantTag => {
                    ty = PoolId::U8;
                }
            }
        }
        Ok(address)
    }

    /// The interned `*T` for an element type.
    fn pointer_to(&self, elem: PoolId, what: &str) -> Result<PoolId, CodegenError> {
        self.shared
            .pool
            .find(&Item::PointerType(elem))
            .ok_or_else(|| {
                CodegenError::Internal(format!("{what} pointer type was never interned"))
            })
    }

    /// Widens or narrows an index to the pointer width, so the multiply is well-typed.
    ///
    /// An index is an `s64` and a pointer is 64 bits on every target this compiles for, so
    /// this is normally the identity; it exists so that a 32-bit target is a widening rather
    /// than a verifier failure.
    fn widen_index(
        &self,
        index: IntValue<'ctx>,
        word: IntType<'ctx>,
    ) -> Result<IntValue<'ctx>, CodegenError> {
        match index.get_type().get_bit_width().cmp(&word.get_bit_width()) {
            std::cmp::Ordering::Equal => Ok(index),
            std::cmp::Ordering::Less => built(self.builder.build_int_s_extend(index, word, "iext")),
            std::cmp::Ordering::Greater => {
                built(self.builder.build_int_truncate(index, word, "itrunc"))
            }
        }
    }

    /// Adds a byte offset to an address, skipping the instruction when it is zero.
    fn offset(
        &self,
        address: PointerValue<'ctx>,
        offset: u64,
        name: &str,
    ) -> Result<PointerValue<'ctx>, CodegenError> {
        if offset == 0 {
            return Ok(address);
        }
        let word = pointer_int(self.context, self.shared.target);
        built(unsafe {
            self.builder.build_in_bounds_gep(
                self.context.i8_type(),
                address,
                &[word.const_int(offset, false)],
                name,
            )
        })
    }

    /// The type a place denotes, after every projection.
    fn place_type(&mut self, place: &Place) -> Result<PoolId, CodegenError> {
        let mut ty = match &place.base {
            PlaceBase::Slot(slot) => self.body.slot(*slot).ty,
            PlaceBase::Deref(operand) => self.pointee(self.operand_type(*operand))?,
            PlaceBase::Global(global) => self
                .shared
                .globals
                .get(global)
                .map(|(_, ty)| *ty)
                .ok_or_else(|| {
                    CodegenError::Internal(format!(
                        "global g{} in file {} was referenced without being declared",
                        global.item.index(),
                        global.file.index()
                    ))
                })?,
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
                    self.pointer_to(elem, "a view's element")?
                }
                Projection::ViewCount => PoolId::S64,
                Projection::DynamicArrayData => {
                    let elem = self.dynamic_array_elem(ty)?;
                    self.pointer_to(elem, "a `[..]T`'s element")?
                }
                Projection::DynamicArrayCount => PoolId::S64,
                Projection::DynamicArrayCapacity => PoolId::S64,
                Projection::VariantTag => PoolId::U8,
            };
        }
        Ok(ty)
    }

    /// Reads a place.
    ///
    /// An aggregate read is a byte copy into fresh space, matching the VM's
    /// `read(...).to_vec()`: the result is a value, so a later write through the original
    /// place must not be visible through it.
    fn load(&mut self, place: &Place) -> Result<Slot<'ctx>, CodegenError> {
        let ty = self.place_type(place)?;
        let repr = Repr::of(self.context, self.shared.pool, self.shared.target, ty)?;
        let address = self.address(place)?;
        match repr {
            Repr::Void => Ok(None),
            // A vector loads as itself, exactly as a scalar does — one instruction into a register,
            // so there is nothing for a later write through the original place to alias (ADR-0148
            // §1). The two share this arm because `llvm_type` already answers for both.
            Repr::Scalar(_) | Repr::Vector { .. } => {
                let llvm = repr
                    .llvm_type(self.context, self.shared.target)
                    .ok_or_else(|| CodegenError::Internal("a scalar with no type".to_owned()))?;
                let loaded = built(self.builder.build_load(llvm, address, "load"))?;
                if let Some(instruction) = loaded.as_instruction_value() {
                    instruction
                        .set_alignment(CLAIMED_ALIGN)
                        .map_err(|e| CodegenError::Internal(format!("load alignment: {e}")))?;
                }
                Ok(Some(loaded))
            }
            Repr::Aggregate { size, align } => {
                let bytes = u32::try_from(size.max(1)).map_err(|_| {
                    CodegenError::Internal("an aggregate larger than a u32".to_owned())
                })?;
                let copy = self.alloca(bytes, align, "agg")?;
                self.copy(copy, address, size, align)?;
                Ok(Some(self.address_int(copy, "agg_addr")?.into()))
            }
        }
    }

    /// Writes `source` into `address`.
    fn write(
        &mut self,
        address: PointerValue<'ctx>,
        repr: Repr<'ctx>,
        source: Slot<'ctx>,
    ) -> Result<(), CodegenError> {
        match repr {
            // A `void` store writes nothing and never touches the address, exactly as the
            // VM's `Shape::Void` arm returns without writing.
            Repr::Void => Ok(()),
            Repr::Scalar(_) | Repr::Vector { .. } => {
                let value = source.ok_or_else(|| {
                    CodegenError::Internal("storing void into a scalar place".to_owned())
                })?;
                let store = built(self.builder.build_store(address, value))?;
                store
                    .set_alignment(CLAIMED_ALIGN)
                    .map_err(|e| CodegenError::Internal(format!("store alignment: {e}")))?;
                Ok(())
            }
            Repr::Aggregate { size, align } => {
                let value = source.ok_or_else(|| {
                    CodegenError::Internal("storing void into an aggregate place".to_owned())
                })?;
                let source = self.pointer_of(value, "src")?;
                self.copy(address, source, size, align)
            }
        }
    }

    /// Stores a scalar of a known Jairs type, taking the alignment from its layout.
    fn store(
        &mut self,
        address: PointerValue<'ctx>,
        value: BasicValueEnum<'ctx>,
        ty: PoolId,
    ) -> Result<(), CodegenError> {
        let repr = Repr::of(self.context, self.shared.pool, self.shared.target, ty)?;
        self.write(address, repr, Some(value))
    }

    /// Stores one pointer-width integer.
    fn store_int(
        &self,
        address: PointerValue<'ctx>,
        value: IntValue<'ctx>,
        _name: &str,
    ) -> Result<(), CodegenError> {
        let store = built(self.builder.build_store(address, value))?;
        store
            .set_alignment(CLAIMED_ALIGN)
            .map_err(|e| CodegenError::Internal(format!("store alignment: {e}")))?;
        Ok(())
    }

    /// Loads one integer of a given width.
    fn load_int(
        &self,
        address: PointerValue<'ctx>,
        ty: IntType<'ctx>,
        name: &str,
    ) -> Result<IntValue<'ctx>, CodegenError> {
        let loaded = built(self.builder.build_load(ty, address, name))?;
        if let Some(instruction) = loaded.as_instruction_value() {
            instruction
                .set_alignment(CLAIMED_ALIGN)
                .map_err(|e| CodegenError::Internal(format!("load alignment: {e}")))?;
        }
        Ok(loaded.into_int_value())
    }

    /// An address as the integer a Jairs pointer is carried in.
    fn address_int(
        &self,
        address: PointerValue<'ctx>,
        name: &str,
    ) -> Result<IntValue<'ctx>, CodegenError> {
        built(self.builder.build_ptr_to_int(
            address,
            pointer_int(self.context, self.shared.target),
            name,
        ))
    }

    /// An integer address as the `ptr` LLVM's memory instructions insist on.
    fn pointer_of(
        &self,
        value: BasicValueEnum<'ctx>,
        name: &str,
    ) -> Result<PointerValue<'ctx>, CodegenError> {
        let address: IntValue<'ctx> = value
            .try_into()
            .map_err(|()| CodegenError::Internal("an address that is not an integer".to_owned()))?;
        built(self.builder.build_int_to_ptr(
            address,
            self.context.ptr_type(AddressSpace::default()),
            name,
        ))
    }

    /// Copies `size` bytes, which is what an aggregate assignment is.
    fn copy(
        &self,
        dest: PointerValue<'ctx>,
        src: PointerValue<'ctx>,
        size: u64,
        align: u32,
    ) -> Result<(), CodegenError> {
        if size == 0 {
            return Ok(());
        }
        let word = pointer_int(self.context, self.shared.target);
        // `align` stays a parameter because a caller reads it from `jr-pool`, but what is
        // *claimed* is `CLAIMED_ALIGN`, for that constant's reason.
        let _ = align;
        built(self.builder.build_memcpy(
            dest,
            CLAIMED_ALIGN,
            src,
            CLAIMED_ALIGN,
            word.const_int(size, false),
        ))?;
        Ok(())
    }

    /// Sets `size` bytes at `address` to zero.
    fn memset_zero(
        &self,
        address: PointerValue<'ctx>,
        size: u64,
        align: u32,
    ) -> Result<(), CodegenError> {
        if size == 0 {
            return Ok(());
        }
        let word = pointer_int(self.context, self.shared.target);
        let _ = align;
        built(self.builder.build_memset(
            address,
            CLAIMED_ALIGN,
            self.context.i8_type().const_zero(),
            word.const_int(size, false),
        ))?;
        Ok(())
    }

    /// The element type an [`Projection::Index`] step lands on.
    ///
    /// Accepts an array *or* a pointer, because a view's element place is its `data` word
    /// indexed directly.
    fn index_elem(&self, ty: PoolId) -> Result<PoolId, CodegenError> {
        match self.shared.pool.item(ty) {
            // A vector lane, for the reason the Cranelift back end's twin gives: the layouts are
            // identical, so this feeds the right stride (ADR-0148 §1).
            Item::ArrayType { elem, .. }
            | Item::VectorType { elem, .. }
            | Item::PointerType(elem) => Ok(*elem),
            _ => Err(CodegenError::Internal(
                "an index projection on neither an array nor a pointer".to_owned(),
            )),
        }
    }

    /// The element type of a view.
    fn view_elem(&self, ty: PoolId) -> Result<PoolId, CodegenError> {
        match self.shared.pool.item(ty) {
            Item::ViewType { elem } => Ok(*elem),
            _ => Err(CodegenError::Internal(
                "a view projection on a non-view type".to_owned(),
            )),
        }
    }

    /// The element type of `[..]T`.
    fn dynamic_array_elem(&self, ty: PoolId) -> Result<PoolId, CodegenError> {
        match self.shared.pool.item(ty) {
            Item::DynamicArrayType { elem } => Ok(*elem),
            _ => Err(CodegenError::Internal(
                "a `[..]T` projection on a non-`[..]T` type".to_owned(),
            )),
        }
    }

    // -----------------------------------------------------------------------
    // Terminators
    // -----------------------------------------------------------------------

    /// Translates a terminator.
    fn terminator(&mut self, term: &Terminator) -> Result<(), CodegenError> {
        match term {
            Terminator::Goto(target) => {
                let block = self.edge(target)?;
                built(self.builder.build_unconditional_branch(block))?;
                Ok(())
            }
            Terminator::Branch { cond, then_, else_ } => {
                let value = self.read_int(*cond)?;
                // A Jairs `bool` is a byte holding 0 or 1; LLVM branches on a bit.
                let zero = value.get_type().const_zero();
                let bit =
                    built(
                        self.builder
                            .build_int_compare(IntPredicate::NE, value, zero, "cond"),
                    )?;
                let then_block = self.edge(then_)?;
                let else_block = self.edge(else_)?;
                built(
                    self.builder
                        .build_conditional_branch(bit, then_block, else_block),
                )?;
                Ok(())
            }
            Terminator::Return(operand) => {
                // **An aggregate result is copied into the caller's slot, and nothing is
                // returned** (ADR-0051 §1). The operand holds a *pointer* to the callee's own
                // storage, so returning it directly would hand back the address of a frame
                // about to be destroyed.
                if let Some(dest) = self.sret {
                    if let Some(operand) = operand {
                        let ty = self.operand_type(*operand);
                        if let Some(src) = self.read(*operand)? {
                            let layout = layout_of(self.shared.pool, self.shared.target, ty)
                                .map_err(|reason| CodegenError::NoLayout { ty, reason })?;
                            let dest = self.pointer_of(dest.into(), "sret")?;
                            let src = self.pointer_of(src, "srcv")?;
                            self.copy(dest, src, layout.size, layout.align)?;
                        }
                    }
                    built(self.builder.build_return(None))?;
                    return Ok(());
                }
                match operand {
                    Some(operand) => match self.read(*operand)? {
                        Some(value) => {
                            built(self.builder.build_return(Some(&value)))?;
                        }
                        None => {
                            built(self.builder.build_return(None))?;
                        }
                    },
                    None => {
                        built(self.builder.build_return(None))?;
                    }
                }
                Ok(())
            }
            Terminator::Unreachable(reason) => {
                // Only `Trap` is a program the compiler believes well-formed; the other two
                // are statically reported (E0228, E0229) and reaching one means the program
                // was run without being checked.
                let kind = match reason {
                    Unreachable::Trap => TrapKind::Deliberate,
                    Unreachable::StrayJump => TrapKind::StrayJump,
                    Unreachable::FellOffEnd => TrapKind::FellOffEnd,
                    // The stub a refused body gets (`jr_mir::MirBody::refused`), so that the
                    // `Export` symbol the declare phase promised exists. Both back ends need this
                    // arm for the same reason, which is why it is not a Cranelift detail.
                    Unreachable::Refused => TrapKind::Refused,
                };
                self.report(kind)?;
                built(self.builder.build_unreachable())?;
                Ok(())
            }
        }
    }

    /// The LLVM block for an edge, recording the arguments it supplies.
    ///
    /// A `void` block parameter takes no argument, so the argument list is filtered the same
    /// way the parameter list was. The predecessor recorded is the block the builder is *in*,
    /// which is not necessarily the block translation started in: a trap check splits it.
    fn edge(&mut self, target: &Target) -> Result<BasicBlock<'ctx>, CodegenError> {
        let block = self.block(target.block)?;
        let mut args = Vec::with_capacity(target.args.len());
        for arg in &target.args {
            if let Some(value) = self.read(*arg)? {
                args.push(value);
            }
        }
        let pred = self
            .builder
            .get_insert_block()
            .ok_or_else(|| CodegenError::Internal("an edge emitted outside a block".to_owned()))?;
        self.incomings.push((target.block, pred, args));
        Ok(block)
    }

    // -----------------------------------------------------------------------
    // Traps
    // -----------------------------------------------------------------------

    /// Traps when `cond` is true, continuing otherwise.
    ///
    /// The shape is a compare-and-branch to a dedicated block that calls the runtime helper,
    /// per ADR-0019 §2. The branch is what keeps the fast path free of the call.
    fn trap_if(&mut self, cond: IntValue<'ctx>, kind: TrapKind) -> Result<(), CodegenError> {
        let trap_block = self.context.append_basic_block(self.function, "trap");
        let continue_block = self.context.append_basic_block(self.function, "cont");
        built(
            self.builder
                .build_conditional_branch(cond, trap_block, continue_block),
        )?;

        self.builder.position_at_end(trap_block);
        self.report(kind)?;
        built(self.builder.build_unreachable())?;

        self.builder.position_at_end(continue_block);
        Ok(())
    }

    /// Traps unconditionally, then continues in an unreachable block.
    ///
    /// Used where the VM traps on a value rather than on a condition — reading an undefined
    /// value. The continuation exists only so that the statements MIR still lists after it
    /// remain translatable.
    fn trap(&mut self, kind: TrapKind) -> Result<(), CodegenError> {
        self.report(kind)?;
        built(self.builder.build_unreachable())?;
        let unreachable = self.context.append_basic_block(self.function, "after_trap");
        self.builder.position_at_end(unreachable);
        Ok(())
    }

    /// Calls the runtime helper that reports a trap and aborts.
    ///
    /// The bytes are produced by `jr_base::trap_message`, the same function the VM and the
    /// Cranelift back end call — three engines rendering at different *times* must still
    /// agree exactly, and the differential harness compares them (ADR-0020 §2).
    fn report(&mut self, kind: TrapKind) -> Result<(), CodegenError> {
        let location = self.shared.locations.location(self.current);
        let message = jr_base::trap_message(kind.reason(), location.as_deref(), &[]);
        let global = self.message_data(&message)?;

        let word = pointer_int(self.context, self.shared.target);
        let text = self.address_int(global.as_pointer_value(), "msg")?;
        let length = word.const_int(message.len() as u64, false);
        built(self.builder.build_call(
            self.shared.trap_helper,
            &[text.into(), length.into()],
            "trap",
        ))?;
        Ok(())
    }

    /// The global holding `message`, created on first use.
    ///
    /// Keyed by content, so two sites that genuinely render the same text — the same line
    /// reached twice, or two traps with no location — share one object.
    fn message_data(&mut self, message: &str) -> Result<GlobalValue<'ctx>, CodegenError> {
        if let Some(global) = self.messages.get(message) {
            return Ok(*global);
        }
        let symbol = format!(
            "jr$trap${}${}${}",
            self.proc.file.index(),
            self.proc.proc.index(),
            self.messages.len()
        );
        let bytes = self.context.const_string(message.as_bytes(), false);
        let global = self.module.add_global(bytes.get_type(), None, &symbol);
        global.set_initializer(&bytes);
        global.set_constant(true);
        global.set_linkage(Linkage::Internal);
        self.messages.insert(message.to_owned(), global);
        Ok(global)
    }

    // -----------------------------------------------------------------------
    // Small helpers
    // -----------------------------------------------------------------------

    /// The LLVM block for a MIR block.
    fn block(&self, id: BlockId) -> Result<BasicBlock<'ctx>, CodegenError> {
        self.blocks.get(&id).copied().ok_or_else(|| {
            CodegenError::Internal(format!("block bb{} is not in the block order", id.index()))
        })
    }

    /// The type an operand holds.
    fn operand_type(&self, operand: Operand) -> PoolId {
        match operand {
            Operand::Value(value) => self.body.value(value).ty,
            Operand::Constant(id) => self.shared.pool.type_of(id),
        }
    }

    /// The type a pointer points at.
    fn pointee(&self, ty: PoolId) -> Result<PoolId, CodegenError> {
        match self.shared.pool.item(ty) {
            Item::PointerType(pointee) => Ok(*pointee),
            other => Err(CodegenError::Internal(format!(
                "expected a pointer, found {other:?}"
            ))),
        }
    }

    /// A struct field's type.
    ///
    /// The same three-way walk `jr-codegen-clif` does — a context's fields, a results
    /// aggregate's element list, then an ordinary field list read by *instance* type so a
    /// parameterised `Box(s64)` field is `s64` (ADR-0085 §2).
    fn field_type(&self, ty: PoolId, index: u32) -> Result<PoolId, CodegenError> {
        if matches!(self.shared.pool.item(ty), Item::ContextType) {
            return Pool::context_field_type(index)
                .ok_or_else(|| CodegenError::Internal(format!("no context field {index}")));
        }
        if let Item::ResultsType { elems } = self.shared.pool.item(ty) {
            return elems
                .get(index as usize)
                .copied()
                .ok_or_else(|| CodegenError::Internal(format!("no result {index}")));
        }
        let (Item::StructType { .. } | Item::UnionType { .. } | Item::VariantType { .. }) =
            self.shared.pool.item(ty)
        else {
            return Err(CodegenError::Internal(
                "a field of a non-aggregate".to_owned(),
            ));
        };
        self.shared
            .pool
            .fields_of(ty)
            .and_then(|fields| fields.get(index as usize))
            .map(|field| field.ty)
            .ok_or_else(|| CodegenError::Internal(format!("no field {index}")))
    }

    /// The span a terminator's instructions belong to.
    ///
    /// A [`Terminator`] carries no span, but its operand is a value and every value does — so
    /// a branch reports the condition tested and a return reports the expression that
    /// produced the result. Mirrors the other two engines, because all three must attribute a
    /// trap to the same construct or their messages differ.
    fn terminator_span(&self, term: &Terminator) -> MirSpan {
        match term {
            Terminator::Branch { cond, .. } => self.span_of(*cond),
            Terminator::Return(Some(operand)) => self.span_of(*operand),
            Terminator::Goto(_) | Terminator::Return(None) | Terminator::Unreachable(_) => {
                MirSpan::Synthetic
            }
        }
    }

    /// Attaches the current span's line and column to every instruction emitted from here on.
    ///
    /// Called once per statement and once per terminator, matching the Cranelift back end exactly
    /// (ADR-0169 §3) — the two engines must attribute code to the same construct or a debugger tells a
    /// different story about the same program depending on which back end built it.
    ///
    /// **The column *is* set here, unlike Cranelift's line table**, and that is not an inconsistency: LLVM
    /// requires a `DILocation` to carry one, and it writes whatever it is given. Passing 0 — "no column" —
    /// is what LLVM itself emits for a statement whose column is unknown, so that is what a synthetic-free
    /// per-statement span deserves. ADR-0169 §4's argument was against *inventing* a column; this passes
    /// the one the span actually has, which is the statement's first byte, and a consumer reading DWARF
    /// column 0 knows to ignore it.
    ///
    /// A span with no position clears the location, so a synthetic instruction inherits nothing.
    fn mark_line(&mut self) {
        let Some(debug) = self.shared.debug else {
            return;
        };
        let Some(at) = self.shared.locations.position(self.current) else {
            self.builder.unset_current_debug_location();
            return;
        };
        let location = debug.info.create_debug_location(
            self.context,
            at.line,
            at.column,
            debug.subprogram.as_debug_info_scope(),
            None,
        );
        self.builder.set_current_debug_location(location);
    }

    /// The span of the value an operand names, if it names one.
    fn span_of(&self, operand: Operand) -> MirSpan {
        match operand {
            Operand::Value(value) => self.body.value(value).span,
            Operand::Constant(_) => MirSpan::Synthetic,
        }
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
    /// One kind per operation, because `jr-vm` names the operation in its message and the
    /// differential harness compares those messages.
    const fn trap_kind(self) -> TrapKind {
        match self {
            Self::Add => TrapKind::OverflowAdd,
            Self::Sub => TrapKind::OverflowSub,
            Self::Mul => TrapKind::OverflowMul,
        }
    }

    /// The LLVM intrinsic that performs this operation and reports overflow.
    const fn intrinsic(self, signed: bool) -> &'static str {
        match (self, signed) {
            (Self::Add, true) => "llvm.sadd.with.overflow",
            (Self::Add, false) => "llvm.uadd.with.overflow",
            (Self::Sub, true) => "llvm.ssub.with.overflow",
            (Self::Sub, false) => "llvm.usub.with.overflow",
            (Self::Mul, true) => "llvm.smul.with.overflow",
            (Self::Mul, false) => "llvm.umul.with.overflow",
        }
    }
}

/// The IEEE-754 predicate a comparison becomes.
///
/// The **ordered** forms for `<`, `<=`, `>`, `>=`, and `OEQ`/`UNE` for `==`/`!=`. That pairing
/// is what gives `NaN` its two surprising answers without a special case: `NaN < x` is false
/// because `NaN` is unordered with everything, and `NaN != NaN` is true because `UNE` is the
/// *unordered-or-not-equal* form — the negation of `OEQ` rather than its own ordered
/// predicate, exactly as Rust's `!=` on `f64` is.
fn float_predicate(op: BinOp) -> Option<FloatPredicate> {
    match op {
        BinOp::Eq => Some(FloatPredicate::OEQ),
        BinOp::Ne => Some(FloatPredicate::UNE),
        BinOp::Lt => Some(FloatPredicate::OLT),
        BinOp::Le => Some(FloatPredicate::OLE),
        BinOp::Gt => Some(FloatPredicate::OGT),
        BinOp::Ge => Some(FloatPredicate::OGE),
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

/// The integer predicate a comparison becomes.
fn int_predicate(op: BinOp, signed: bool) -> IntPredicate {
    match op {
        BinOp::Eq => IntPredicate::EQ,
        BinOp::Ne => IntPredicate::NE,
        BinOp::Lt if signed => IntPredicate::SLT,
        BinOp::Lt => IntPredicate::ULT,
        BinOp::Le if signed => IntPredicate::SLE,
        BinOp::Le => IntPredicate::ULE,
        BinOp::Gt if signed => IntPredicate::SGT,
        BinOp::Gt => IntPredicate::UGT,
        BinOp::Ge if signed => IntPredicate::SGE,
        BinOp::Ge => IntPredicate::UGE,
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
        | BinOp::Shr => IntPredicate::EQ,
    }
}

/// The bit pattern of the most negative value of a signed integer type.
///
/// As a `u64` because `const_int` takes a sign-agnostic pattern: the sign bit set and nothing
/// else, at the type's own width.
fn min_bits(ty: IntType<'_>) -> u64 {
    1u64 << (ty.get_bit_width() - 1)
}

/// The span a statement's instructions belong to.
pub(crate) fn statement_span(stmt: &Statement) -> MirSpan {
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
