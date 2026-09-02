//! The Cranelift back end, and the only crate in the workspace that names a
//! `cranelift-*` type.
//!
//! # Why the confinement is structural
//!
//! ADR-0009 pins `cranelift-*` with `=` because its API is explicitly not
//! semver-stable, and requires that every contact with it live here behind
//! [`jr_codegen::Backend`]. That is what makes an API break, or wave W8's LLVM back
//! end, a change to one crate rather than to the compiler. `CONTRIBUTING.md` states
//! the rule; this crate is where it is kept.
//!
//! # What ADR-0017 bought
//!
//! The translation is close to mechanical, and that is the return on decisions taken
//! two waves earlier rather than luck. Block parameters map onto
//! `append_block_param`, so there is no unphi pass; slots map onto Cranelift stack
//! slots addressed by `stack_addr`, because lowering already put escaped locals in
//! memory; `reverse_postorder()` is the block order. The `body` module has the detail.
//!
//! # The one thing this crate must never do
//!
//! Compute a size, an alignment or an offset. Every one comes from [`jr_pool`], which
//! is the single layout ADR-0018 §2 put in the pool so that the comptime VM and this
//! back end cannot disagree. ADR-0019 restates it as a prohibition because the
//! failure is *silent*: a field at a different offset in a `#run` than at runtime is
//! two programs from one source, with no diagnostic and no verifier complaint.
//! The `repr` module is where the numbers enter, and it asks rather than
//! calculates.

mod body;
mod debug;
mod repr;

// `TrapKind` and `TRAP_HELPER` live in `jr-codegen` (ADR-0143 §6): they are the *words* a
// trapping program prints, paired with `jr_base::trap_message`, and a second copy in the
// LLVM back end would be a second chance to drift from the bytes the differential harness
// compares. Re-exported here because this crate's own consumers named them here first.
pub use jr_codegen::{TRAP_HELPER, TrapKind};

use cranelift_codegen::Context;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{
    AbiParam, Function, InstBuilder as _, MemFlagsData, StackSlotData, StackSlotKind, TrapCode,
    UserFuncName, Value as ClifValue, types,
};
use cranelift_codegen::isa::CallConv;
use cranelift_codegen::settings::{self, Configurable as _};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use jr_codegen::{Backend, CodegenError, ProcDecl, ProcKind, TrapLocations};
use jr_mir::{MirBody, ProcRef};
use jr_pool::{Item, Layout, Pool, PoolId, StrId, TargetLayout, layout_of};
use rustc_hash::FxHashMap;

/// One defined function's line-table rows, before its `FuncId` becomes an object symbol.
///
/// A named struct rather than a tuple, because the `FuncId` cannot be resolved to a `SymbolId` until
/// `ObjectModule::finish` has run — so these three fields travel together across that boundary and a bare
/// `(FuncId, u64, Vec<(u32, u32)>)` gives a reader no way to tell the length from the offsets.
struct PendingLines {
    /// The function the rows belong to.
    id: FuncId,
    /// Its code length in bytes, which ends the line program's sequence.
    length: u64,
    /// `(code offset from the function's start, line-vocabulary index)`, ascending by offset.
    rows: Vec<(u32, u32)>,
}

/// The Cranelift implementation of [`Backend`].
pub struct ClifBackend {
    module: ObjectModule,
    /// The module-wide `(path, line)` table every instruction's `SourceLoc` indexes into (ADR-0169 §1).
    lines: debug::LineVocabulary,
    /// Each defined function's code length and `(offset, line index)` rows.
    ///
    /// Collected in `define` because that is the only place the compiled buffer exists — `finalise` has the
    /// object but not the per-function `CompiledCode` it came from.
    function_lines: Vec<PendingLines>,
    /// The compilation directory and primary source file the line program names.
    ///
    /// Set from the first body defined, which is the root file's: DWARF wants one primary file per unit, and
    /// this back end emits one unit. A multi-unit design is the right answer eventually and is not needed to
    /// make a backtrace name a line.
    unit: Option<(String, String)>,
    /// The Cranelift id of every declared procedure.
    ids: FxHashMap<ProcRef, FuncId>,
    /// Whether a declared procedure is `#foreign`, which decides its call
    /// convention and whether a body may be defined for it.
    foreign: FxHashMap<ProcRef, bool>,
    /// The data object holding each string constant's bytes.
    strings: FxHashMap<StrId, DataId>,
    /// The data object holding each compiler-emitted table's bytes (ADR-0152 §1).
    static_arrays: FxHashMap<PoolId, DataId>,
    /// The target layout, needed to build a table's byte image at construction (ADR-0152 §1).
    ///
    /// Passed in rather than defaulted, because a byte image is a *target* answer and guessing one here
    /// would put a second layout opinion beside `jr-pool`'s — the thing ADR-0018 §2 exists to prevent.
    target: TargetLayout,
    /// The runtime helper a trap calls.
    trap_helper: FuncId,
    /// Every library a `#foreign` declaration named, for the link line.
    libraries: Vec<String>,
    /// The Jairs procedure the `main` shim calls, its return type, and whether it takes a context.
    entry: Option<(ProcRef, PoolId, bool)>,
    /// The context struct's layout and the target, remembered when the entry is declared so the
    /// entry shim (built in `finalise`, which has no pool) can size the slot it allocates for
    /// `main`'s context (ADR-0057 §5). `None` when `main` takes none.
    entry_context: Option<(Layout, TargetLayout)>,
    /// The shadow call stack a trap reports (ADR-0066 §1): `SHADOW_CAPACITY` name pointers.
    ///
    /// **The first mutable data object this back end emits** — every other one is a read-only string
    /// or message — which is why it is worth naming here rather than declaring inline. A caller writes
    /// its callee's name pointer at `shadow_depth` and increments; the trap helper walks the entries
    /// below the depth.
    shadow_stack: DataId,
    /// How many entries of [`Self::shadow_stack`] are live: one pointer-sized counter.
    shadow_depth: DataId,
    /// The read-only string holding each procedure's source name, for the backtrace (ADR-0066 §3).
    ///
    /// Per *procedure* rather than per trap site, because a name is a property of the procedure — so a
    /// procedure called from twenty places has one string, and the shadow stack stores its address.
    /// The length rides along because the string is not NUL-terminated and the helper has no `strlen`.
    names: FxHashMap<ProcRef, (DataId, usize)>,
}

/// How many frames the shadow call stack holds (ADR-0066 §1).
///
/// The VM's `MAX_DEPTH` is 256 and this matches it, so a program that recurses to the VM's limit gets
/// the same backtrace natively. A deeper native recursion overflows the *machine* stack long before
/// this fills; the push is guarded regardless, because a silently out-of-bounds write into a static
/// array is the kind of memory corruption that would be blamed on the program.
const SHADOW_CAPACITY: usize = 256;

impl ClifBackend {
    /// Creates a back end targeting the host.
    ///
    /// String constants are given data objects here, up front and in pool order,
    /// which mirrors `jr-vm`'s `intern_strings` deliberately: both deduplicate by
    /// [`StrId`] rather than by contents, so a program's set of string objects is
    /// the same either way.
    ///
    /// # Errors
    /// [`CodegenError::Internal`] when the host has no Cranelift backend, or when the
    /// object module rejects a declaration.
    pub fn new(pool: &Pool, target: TargetLayout, name: &str) -> Result<Self, CodegenError> {
        let mut flags = settings::builder();
        // A trap is a call to a helper that does not return, so unwind information
        // buys nothing and costs a section in every object.
        flags
            .set("unwind_info", "false")
            .map_err(|e| CodegenError::Internal(format!("cranelift flag: {e}")))?;
        // Position-independent code is not optional on Apple platforms: a direct call
        // to a dynamically imported symbol needs a text relocation, and `ld64`
        // rejects those outright ("Found illegal text-relocations"). PIC routes the
        // call through a stub instead. Every modern target wants this anyway.
        flags
            .set("is_pic", "true")
            .map_err(|e| CodegenError::Internal(format!("cranelift flag: {e}")))?;
        let isa = cranelift_native::builder()
            .map_err(|e| CodegenError::Internal(format!("no host backend: {e}")))?
            .finish(settings::Flags::new(flags))
            .map_err(|e| CodegenError::Internal(format!("cannot build an ISA: {e}")))?;

        let builder = ObjectBuilder::new(isa, name.as_bytes().to_vec(), default_libcall_names())
            .map_err(|e| CodegenError::Internal(format!("cannot build an object: {e}")))?;
        let mut module = ObjectModule::new(builder);

        let trap_helper = module
            .declare_function(TRAP_HELPER, Linkage::Local, &trap_signature(&module))
            .map_err(|e| CodegenError::Internal(format!("cannot declare {TRAP_HELPER}: {e}")))?;

        // The shadow call stack and its depth (ADR-0066 §1), both **writable** — the only mutable data
        // this back end emits. Zero-initialised, so a program that never calls anything has a depth of
        // 0 and the helper walks nothing.
        let pointer_size = usize::from(module.target_config().pointer_bytes());
        let shadow_stack = module
            .declare_data("jr$shadow$stack", Linkage::Local, true, false)
            .map_err(|e| CodegenError::Internal(format!("cannot declare the shadow stack: {e}")))?;
        let mut stack_data = DataDescription::new();
        stack_data.define_zeroinit(SHADOW_CAPACITY * pointer_size);
        module
            .define_data(shadow_stack, &stack_data)
            .map_err(|e| CodegenError::Internal(format!("cannot define the shadow stack: {e}")))?;

        let shadow_depth = module
            .declare_data("jr$shadow$depth", Linkage::Local, true, false)
            .map_err(|e| CodegenError::Internal(format!("cannot declare the shadow depth: {e}")))?;
        let mut depth_data = DataDescription::new();
        depth_data.define_zeroinit(pointer_size);
        module
            .define_data(shadow_depth, &depth_data)
            .map_err(|e| CodegenError::Internal(format!("cannot define the shadow depth: {e}")))?;

        let mut backend = Self {
            module,
            ids: FxHashMap::default(),
            foreign: FxHashMap::default(),
            strings: FxHashMap::default(),
            static_arrays: FxHashMap::default(),
            trap_helper,
            libraries: Vec::new(),
            entry: None,
            entry_context: None,
            shadow_stack,
            shadow_depth,
            names: FxHashMap::default(),
            target,
            lines: debug::LineVocabulary::default(),
            function_lines: Vec::new(),
            unit: None,
        };
        backend.emit_strings(pool)?;
        backend.define_trap_helper()?;
        Ok(backend)
    }

    /// The two literals a backtrace line is built from, as `(prefix, prefix_len, newline, 1)`.
    ///
    /// `"  in "` and `"\n"` are the only punctuation the chain needs, and they are the *same* bytes
    /// `jr_base::trap_message` writes — which is the coupling ADR-0020 §2 accepts deliberately: the
    /// format lives in one function, and this is that function's output assembled a piece at a time
    /// because the helper has no allocator to build a whole line in.
    ///
    /// # Errors
    /// [`CodegenError::Internal`] when the object module rejects a declaration.
    fn frame_prefix_data(
        &mut self,
        builder: &mut FunctionBuilder<'_>,
        pointer: types::Type,
        _write: cranelift_codegen::ir::FuncRef,
    ) -> Result<(ClifValue, ClifValue, ClifValue, ClifValue), CodegenError> {
        let prefix_id = self.literal_data("jr$frame$prefix", b"  in ")?;
        let newline_id = self.literal_data("jr$frame$newline", b"\n")?;
        let prefix_global = self.module.declare_data_in_func(prefix_id, builder.func);
        let newline_global = self.module.declare_data_in_func(newline_id, builder.func);
        let prefix = builder.ins().symbol_value(pointer, prefix_global);
        let prefix_len = builder.ins().iconst(pointer, 5);
        let newline = builder.ins().symbol_value(pointer, newline_global);
        let newline_len = builder.ins().iconst(pointer, 1);
        Ok((prefix, prefix_len, newline, newline_len))
    }

    /// A read-only data object holding `bytes`, declared once under `symbol`.
    ///
    /// # Errors
    /// [`CodegenError::Internal`] when the object module rejects the declaration.
    fn literal_data(&mut self, symbol: &str, bytes: &[u8]) -> Result<DataId, CodegenError> {
        let id = self
            .module
            .declare_data(symbol, Linkage::Local, false, false)
            .map_err(|e| CodegenError::Internal(format!("cannot declare {symbol}: {e}")))?;
        let mut description = DataDescription::new();
        description.define(bytes.to_vec().into_boxed_slice());
        self.module
            .define_data(id, &description)
            .map_err(|e| CodegenError::Internal(format!("cannot define {symbol}: {e}")))?;
        Ok(id)
    }

    /// Generates the body of the trap helper.
    ///
    /// ADR-0019 §2 chose "a call into a runtime helper that reports and aborts", and
    /// said the helper would live in a small runtime that `jr-link` links in. It is
    /// **generated into the object instead**, which is the same decision with less
    /// machinery: the helper only needs libc `write` and `exit`, both of which the
    /// program is already linked against, so a separate runtime object would add a
    /// build artifact, a C toolchain dependency and a second thing to keep working
    /// per platform — for a function that is nine instructions long.
    ///
    /// It writes the message to file descriptor 2 and exits with
    /// [`TrapKind::EXIT_STATUS`], which is `jr run`'s status for a trap. Matching it
    /// is what lets a script driving the compiler treat the two execution engines
    /// alike, and what lets the differential harness compare a *failing* program's
    /// behaviour and not merely a succeeding one's.
    fn define_trap_helper(&mut self) -> Result<(), CodegenError> {
        let pointer = self.module.target_config().pointer_type();

        // `write` and `exit` are the same two symbols `modules/Basic` declares
        // `#foreign`, with the same signatures, so `cranelift-module` resolves both
        // declarations to one import rather than to a duplicate.
        let mut write_sig = self.module.make_signature();
        write_sig.call_conv = CallConv::SystemV;
        for _ in 0..3 {
            write_sig.params.push(AbiParam::new(pointer));
        }
        write_sig.returns.push(AbiParam::new(pointer));
        let write = self
            .module
            .declare_function("write", Linkage::Import, &write_sig)
            .map_err(|e| CodegenError::Internal(format!("cannot declare write: {e}")))?;

        let mut exit_sig = self.module.make_signature();
        exit_sig.call_conv = CallConv::SystemV;
        exit_sig.params.push(AbiParam::new(pointer));
        let exit = self
            .module
            .declare_function("exit", Linkage::Import, &exit_sig)
            .map_err(|e| CodegenError::Internal(format!("cannot declare exit: {e}")))?;

        let mut function =
            Function::with_name_signature(UserFuncName::default(), trap_signature(&self.module));
        let mut builder_context = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut function, &mut builder_context);
        let block = builder.create_block();
        builder.append_block_params_for_function_params(block);
        builder.switch_to_block(block);

        let params = builder.block_params(block).to_vec();
        let message = params[0];
        let length = params[1];
        let write_ref = self.module.declare_func_in_func(write, builder.func);
        let exit_ref = self.module.declare_func_in_func(exit, builder.func);

        let stderr = builder.ins().iconst(pointer, 2);
        builder.ins().call(write_ref, &[stderr, message, length]);

        // **The backtrace** (ADR-0066 §2). The message written above already carries the reason and the
        // location, both compile-time constants; the chain is the part only run time knows, so it is
        // written here by walking the shadow stack downward — innermost frame first, which is the order
        // the VM renders and `trap_message`'s tests pin.
        //
        // Each entry is a pointer to a NUL-free name string plus its length, stored as a pair, so the
        // loop needs no strlen: three `write` calls per frame ("  in ", the name, "\n"). Assembling one
        // buffer instead would need an allocator, which a trap handler must not use.
        let stack_global = self
            .module
            .declare_data_in_func(self.shadow_stack, builder.func);
        let depth_global = self
            .module
            .declare_data_in_func(self.shadow_depth, builder.func);
        let stack_base = builder.ins().symbol_value(pointer, stack_global);
        let depth_addr = builder.ins().symbol_value(pointer, depth_global);
        let depth = builder
            .ins()
            .load(pointer, MemFlagsData::new(), depth_addr, 0);

        let prefix = self.frame_prefix_data(&mut builder, pointer, write_ref)?;

        // `index` counts down from `depth`, so entry `depth - 1` (the innermost frame) is written first.
        let header = builder.create_block();
        let body_block = builder.create_block();
        let done = builder.create_block();
        builder.append_block_param(header, pointer);
        builder.ins().jump(header, &[depth.into()]);

        builder.switch_to_block(header);
        let index = builder.block_params(header)[0];
        let more = builder.ins().icmp_imm_s(IntCC::SignedGreaterThan, index, 0);
        builder.ins().brif(more, body_block, &[], done, &[]);

        builder.switch_to_block(body_block);
        let next = builder.ins().iadd_imm_s(index, -1);
        // Each frame occupies two pointer-sized words: the name's address, then its length.
        let stride = i64::from(self.module.target_config().pointer_bytes()) * 2;
        let offset = builder.ins().imul_imm_s(next, stride);
        let entry = builder.ins().iadd(stack_base, offset);
        let name = builder.ins().load(pointer, MemFlagsData::new(), entry, 0);
        let name_len = builder.ins().load(
            pointer,
            MemFlagsData::new(),
            entry,
            i32::from(self.module.target_config().pointer_bytes()),
        );
        builder.ins().call(write_ref, &[stderr, prefix.0, prefix.1]);
        builder.ins().call(write_ref, &[stderr, name, name_len]);
        builder.ins().call(write_ref, &[stderr, prefix.2, prefix.3]);
        builder.ins().jump(header, &[next.into()]);

        builder.switch_to_block(done);

        let status = builder
            .ins()
            .iconst(pointer, i64::from(TrapKind::EXIT_STATUS));
        builder.ins().call(exit_ref, &[status]);
        // `exit` does not return, but Cranelift needs a terminator and cannot know
        // that; the trap is unreachable and costs one instruction.
        builder.ins().trap(TrapCode::user(1).ok_or_else(|| {
            CodegenError::Internal("trap code 1 is not a valid user code".to_owned())
        })?);
        builder.seal_all_blocks();
        builder.finalize(self.module.target_config());

        let mut context = Context::for_function(function);
        self.module
            .define_function(self.trap_helper, &mut context)
            .map_err(|e| CodegenError::Internal(format!("cannot define {TRAP_HELPER}: {e}")))?;
        Ok(())
    }

    /// Emits the `main` the system linker expects.
    ///
    /// A Jairs `main` is an ordinary procedure with a mangled symbol; this is a
    /// separate C `main` that calls it and returns a real process status. The reason
    /// is worth recording because the first native run of `024-hello.jr` demonstrated
    /// it: with the Jairs procedure named `main` directly, the program printed both
    /// its lines correctly and then exited **1**, because a `void`-returning procedure
    /// leaves the return register holding whatever it last held and the C runtime
    /// hands that to `exit`.
    ///
    /// So the shim decides the status explicitly. A `void` `main` gives 0, which is
    /// what the VM's `RunOutcome::Completed` means. An integer-returning `main` gives
    /// its value, narrowed to the `int` the C runtime expects.
    fn define_entry_shim(&mut self) -> Result<(), CodegenError> {
        let Some((entry, ret, entry_context)) = self.entry else {
            // No entry point is not an error here: a library or a test may want an
            // object with no `main` in it. The driver refuses earlier when a *program*
            // declares none.
            return Ok(());
        };
        let callee = *self
            .ids
            .get(&entry)
            .ok_or(CodegenError::Undeclared(entry))?;

        let mut signature = self.module.make_signature();
        signature.call_conv = CallConv::SystemV;
        signature.returns.push(AbiParam::new(types::I32));
        let id = self
            .module
            .declare_function("main", Linkage::Export, &signature)
            .map_err(|e| CodegenError::Internal(format!("cannot declare main: {e}")))?;

        let mut function = Function::with_name_signature(UserFuncName::default(), signature);
        let mut builder_context = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut function, &mut builder_context);
        let block = builder.create_block();
        builder.switch_to_block(block);

        let callee_ref = self.module.declare_func_in_func(callee, builder.func);
        // **`main`'s context is a zeroed stack slot in the shim** (ADR-0057 §5): `main` has no Jairs
        // caller, so the shim is where the first one is born. Zeroed, so `context.allocator` reads 0
        // in a program that never sets it — the same defined-not-garbage rule ADR-0039 §4a used.
        //
        // Only when `main` takes one: a `#c_call main` gets no argument, and passing one anyway is
        // the shift ADR-0053 §1 records.
        let mut call_args = Vec::new();
        if entry_context {
            let (layout, target) = self.entry_context.ok_or_else(|| {
                CodegenError::Internal(
                    "entry takes a context but its layout was never recorded".to_owned(),
                )
            })?;
            let size = u32::try_from(layout.size.max(1)).map_err(|_| {
                CodegenError::Internal("the context is larger than a u32".to_owned())
            })?;
            let slot = builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                size,
                layout.align.trailing_zeros().try_into().unwrap_or(0),
            ));
            let pointer = crate::repr::pointer_type(target);
            let address = builder.ins().stack_addr(pointer, slot, 0);
            // Zero the field(s), so `context.allocator` reads 0 — `emit_small_memset` is what
            // `Statement::Zero` already uses (ADR-0057 §5).
            builder.emit_small_memset(
                self.module.target_config(),
                address,
                0,
                layout.size,
                layout.align.try_into().unwrap_or(1),
                MemFlagsData::new(),
            );
            call_args.push(address);
        }
        // **`main`'s own frame** (ADR-0066 §1). Every other frame is pushed by its caller, and `main`'s
        // caller is this shim — so without this the native backtrace ends one frame short of the VM's,
        // whose `run_main` calls `main` through `Vm::call` and therefore pushes it. That asymmetry is
        // exactly what the differential harness compares, and it showed up as a missing `  in main`.
        //
        // Never popped: `main` returning means the program is over, and the shim `return`s straight
        // after, so nothing can observe the depth again.
        if let Some((name_id, name_len)) = self.names.get(&entry).copied() {
            let pointer = self.module.target_config().pointer_type();
            let stack_global = self
                .module
                .declare_data_in_func(self.shadow_stack, builder.func);
            let depth_global = self
                .module
                .declare_data_in_func(self.shadow_depth, builder.func);
            let name_global = self.module.declare_data_in_func(name_id, builder.func);
            let stack_base = builder.ins().symbol_value(pointer, stack_global);
            let depth_addr = builder.ins().symbol_value(pointer, depth_global);
            let name = builder.ins().symbol_value(pointer, name_global);
            let len = builder
                .ins()
                .iconst(pointer, i64::try_from(name_len).unwrap_or(0));
            let width = i32::from(self.module.target_config().pointer_bytes());
            // Depth is 0 here — this is the first frame — so the entry goes at offset 0 and the
            // bounds check the per-call push needs is unnecessary.
            builder
                .ins()
                .store(MemFlagsData::new(), name, stack_base, 0);
            builder
                .ins()
                .store(MemFlagsData::new(), len, stack_base, width);
            let one = builder.ins().iconst(pointer, 1);
            builder.ins().store(MemFlagsData::new(), one, depth_addr, 0);
        }
        let call = builder.ins().call(callee_ref, &call_args);
        let results = builder.inst_results(call).to_vec();

        let status = match results.first() {
            // An integer `main` returns its own value, narrowed to a C `int`. `void`
            // produces no result at all, which is why this is a `match` on presence
            // rather than on the type.
            Some(value) if ret != PoolId::VOID => {
                let width = builder.func.dfg.value_type(*value);
                if width == types::I32 {
                    *value
                } else if width.bits() > 32 {
                    builder.ins().ireduce(types::I32, *value)
                } else {
                    builder.ins().sextend(types::I32, *value)
                }
            }
            _ => builder.ins().iconst(types::I32, 0),
        };
        builder.ins().return_(&[status]);
        builder.seal_all_blocks();
        builder.finalize(self.module.target_config());

        let mut context = Context::for_function(function);
        self.module
            .define_function(id, &mut context)
            .map_err(|e| CodegenError::Internal(format!("cannot define main: {e}")))?;
        Ok(())
    }

    /// Emits one read-only data object per interned string.
    ///
    /// Not NUL-terminated: a Jairs string is `{data, count}` and carries its own
    /// length (ADR-0004), which is exactly the shape `write` wants.
    fn emit_strings(&mut self, pool: &Pool) -> Result<(), CodegenError> {
        for index in 0..pool.len() {
            let id = PoolId::from_usize(index);
            let Item::StrValue(str_id) = *pool.item(id) else {
                continue;
            };
            if self.strings.contains_key(&str_id) {
                continue;
            }
            let symbol = format!("jr$str${}", str_id.index());
            let data = self
                .module
                .declare_data(&symbol, Linkage::Local, false, false)
                .map_err(|e| CodegenError::Internal(format!("string data: {e}")))?;
            let mut description = DataDescription::new();
            let bytes = pool.resolve_str(str_id).as_bytes().to_vec();
            // An empty string still needs an address, and Cranelift will not define a
            // zero-length object, so one padding byte is emitted. `count` is zero, so
            // it is never read.
            let bytes = if bytes.is_empty() { vec![0u8] } else { bytes };
            description.define(bytes.into_boxed_slice());
            self.module
                .define_data(data, &description)
                .map_err(|e| CodegenError::Internal(format!("string data: {e}")))?;
            self.strings.insert(str_id, data);
        }
        self.emit_static_arrays(pool)?;
        Ok(())
    }

    /// Emits every compiler-emitted table as a read-only data object (ADR-0152 §1).
    ///
    /// Runs after `emit_strings` because a table may contain a `string`, whose `data` word is the
    /// address of one of those objects — and this is where the two are stitched together, using
    /// Cranelift *relocations* rather than a numeric address, because a numeric address does not exist
    /// until the linker has run.
    fn emit_static_arrays(&mut self, pool: &Pool) -> Result<(), CodegenError> {
        for index in 0..pool.len() {
            let id = PoolId::from_usize(index);
            if self.static_arrays.contains_key(&id) {
                continue;
            }
            let Some(values) = pool.static_array_values(id).map(<[PoolId]>::to_vec) else {
                continue;
            };
            let elem = pool.view_elem(pool.type_of(id)).ok_or_else(|| {
                CodegenError::Internal("a static table with no element".to_owned())
            })?;

            // **String addresses are relocations, not numbers.** The image is built with a zero in
            // every string's `data` word and a relocation is recorded at that offset, which the linker
            // fills. Writing a number here would be writing a compile-time address into a run-time
            // program — the defect ADR-0074 found, in the one engine where it would have looked like it
            // worked until the object was loaded somewhere else.
            let mut patches: Vec<(u64, StrId)> = Vec::new();
            let bytes = {
                let mut resolve = |str_id: StrId, at: u64| {
                    patches.push((at, str_id));
                    0
                };
                // The pool computes offsets and widths; see `jr_pool::static_image`.
                jr_pool::static_image(pool, self.target, elem, &values, &mut resolve)
                    .map_err(|reason| CodegenError::NoLayout { ty: elem, reason })?
            };

            let symbol = format!("jr$table${}", id.index());
            let data = self
                .module
                .declare_data(&symbol, Linkage::Local, false, false)
                .map_err(|e| CodegenError::Internal(format!("table data: {e}")))?;
            let mut description = DataDescription::new();
            let bytes = if bytes.is_empty() { vec![0u8] } else { bytes };
            description.define(bytes.into_boxed_slice());
            // **Every string pointer is a relocation**, applied at the offset the pool reported. Writing
            // a number instead left a zero in the word, and reading the name through it gave 139 where
            // the VM gave 121 — caught by this wave's own corpus file, which is what the differential is
            // for.
            for (at, str_id) in patches {
                let target_data = *self.strings.get(&str_id).ok_or_else(|| {
                    CodegenError::Internal("a table names a string with no data object".to_owned())
                })?;
                let global = self
                    .module
                    .declare_data_in_data(target_data, &mut description);
                description.write_data_addr(
                    u32::try_from(at).map_err(|_| {
                        CodegenError::Internal("a table larger than a u32".to_owned())
                    })?,
                    global,
                    0,
                );
            }
            self.module
                .define_data(data, &description)
                .map_err(|e| CodegenError::Internal(format!("table data: {e}")))?;
            self.static_arrays.insert(id, data);
        }
        Ok(())
    }
}

impl Backend for ClifBackend {
    fn declare(
        &mut self,
        decl: &ProcDecl,
        pool: &Pool,
        layout: TargetLayout,
    ) -> Result<(), CodegenError> {
        let (name, linkage, foreign) = match &decl.kind {
            ProcKind::Local { symbol, entry } => (
                symbol.clone(),
                // The entry point must be visible to the system linker; nothing else
                // needs to be.
                if *entry {
                    Linkage::Export
                } else {
                    Linkage::Local
                },
                false,
            ),
            ProcKind::Foreign(symbol) => {
                if let Some(library) = &symbol.library
                    && !self.libraries.iter().any(|held| held == library)
                {
                    self.libraries.push(library.clone());
                }
                (symbol.symbol.clone(), Linkage::Import, true)
            }
        };

        let proc = decl.proc;
        let describe = move |what: &str| CodegenError::Unsupported {
            proc,
            what: what.to_owned(),
        };
        // A `#foreign` procedure is implicitly `#c_call` (ADR-0001), so it takes the
        // platform's C convention; everything else is ours and may use the fast one.
        let call_conv = if foreign {
            CallConv::SystemV
        } else {
            CallConv::Fast
        };
        let signature = repr::signature(
            pool,
            layout,
            &decl.params,
            decl.ret,
            call_conv,
            foreign,
            decl.receives_context,
            &describe,
        )?;

        let id = self
            .module
            .declare_function(&name, linkage, &signature)
            .map_err(|e| CodegenError::Internal(format!("cannot declare {name}: {e}")))?;
        self.ids.insert(decl.proc, id);
        self.foreign.insert(decl.proc, foreign);
        // The read-only string a backtrace frame names (ADR-0066 §3), one per procedure. The *source*
        // name, not the mangled symbol: a reader wants `countdown`, not `jr$0$3`. Emitted at declare
        // time so a call site in any body can reference it, and skipped for a procedure with no name —
        // whose frame is then omitted rather than printed as a placeholder.
        if let Some(source_name) = &decl.name {
            let symbol = format!(
                "jr$name${}${}",
                decl.proc.file.index(),
                decl.proc.proc.index()
            );
            let data = self.literal_data(&symbol, source_name.as_bytes())?;
            self.names.insert(decl.proc, (data, source_name.len()));
        }
        if matches!(decl.kind, ProcKind::Local { entry: true, .. }) {
            self.entry = Some((decl.proc, decl.ret, decl.receives_context));
            if decl.receives_context {
                // Declaring the entry means checking ran, which interned the context; falling back
                // to `ERROR` is defensive rather than panicking in codegen.
                let ctx = pool.context_type_id().unwrap_or(PoolId::ERROR);
                if let Ok(context_layout) = layout_of(pool, layout, ctx) {
                    self.entry_context = Some((context_layout, layout));
                }
            }
        }
        Ok(())
    }

    fn define(
        &mut self,
        proc: ProcRef,
        mir: &MirBody,
        pool: &Pool,
        layout: TargetLayout,
        locations: &dyn TrapLocations,
    ) -> Result<(), CodegenError> {
        let id = *self.ids.get(&proc).ok_or(CodegenError::Undeclared(proc))?;
        if self.foreign.get(&proc).copied().unwrap_or(false) {
            return Err(CodegenError::Internal(
                "a body was defined for a `#foreign` procedure".to_owned(),
            ));
        }

        let signature = self
            .module
            .declarations()
            .get_function_decl(id)
            .signature
            .clone();
        let mut function = Function::with_name_signature(UserFuncName::default(), signature);
        let mut builder_context = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut function, &mut builder_context);

        let funcs: FxHashMap<ProcRef, cranelift_codegen::ir::FuncRef> = self
            .ids
            .iter()
            .map(|(reference, id)| {
                (
                    *reference,
                    self.module.declare_func_in_func(*id, builder.func),
                )
            })
            .collect();
        let trap_helper = self
            .module
            .declare_func_in_func(self.trap_helper, builder.func);

        let ctx = body::Context {
            pool,
            target: layout,
            funcs: &funcs,
            strings: &self.strings,
            static_arrays: &self.static_arrays,
            trap_helper,
            locations,
            shadow: (self.shadow_stack, self.shadow_depth),
            names: &self.names,
            foreign: &self.foreign,
        };
        body::translate(
            &mut builder,
            &mut self.module,
            &ctx,
            proc,
            mir,
            &mut self.lines,
        )?;
        builder.finalize(self.module.target_config());

        let mut context = Context::for_function(function);
        self.module
            .define_function(id, &mut context)
            .map_err(|e| CodegenError::Internal(format!("cannot define a body: {e:?}")))?;

        // The compiled buffer's source locations, for this function's line-table rows (ADR-0169 §1). Read
        // here because `finalise` has the object but not the per-function `CompiledCode` it came from.
        //
        // `get_srclocs_sorted` returns ascending, non-overlapping ranges, which is what a DWARF line program
        // needs — a row's line holds until the next row, so an unsorted list would silently attribute code to
        // the wrong statement.
        if let Some(compiled) = context.compiled_code() {
            let mut rows = Vec::new();
            for loc in compiled.buffer.get_srclocs_sorted() {
                if loc.loc.is_default() {
                    // A synthetic instruction: no row, rather than inheriting the previous line.
                    continue;
                }
                rows.push((loc.start, loc.loc.bits()));
            }
            if !rows.is_empty() {
                self.function_lines.push(PendingLines {
                    id,
                    length: u64::from(compiled.buffer.total_size()),
                    rows,
                });
            }
        }

        // The unit's primary file is the first one any body reported, which is the root file's.
        if self.unit.is_none()
            && let Some(at) = mir
                .blocks()
                .iter()
                .flat_map(|block| block.stmts.iter().map(body::statement_span))
                .find_map(|span| locations.position(span))
        {
            let path = std::path::PathBuf::from(&at.path);
            let dir = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map_or_else(
                    || {
                        std::env::current_dir()
                            .unwrap_or_default()
                            .display()
                            .to_string()
                    },
                    |p| p.display().to_string(),
                );
            self.unit = Some((dir, at.path));
        }
        Ok(())
    }

    fn finalise(mut self: Box<Self>) -> Result<Vec<u8>, CodegenError> {
        self.define_entry_shim()?;
        let endian = if self.module.isa().endianness() == cranelift_codegen::ir::Endianness::Little
        {
            gimli::RunTimeEndian::Little
        } else {
            gimli::RunTimeEndian::Big
        };
        let lines = std::mem::take(&mut self.lines);
        let function_lines = std::mem::take(&mut self.function_lines);
        let unit = self.unit.take();
        let mut product = self.module.finish();

        // The line table, added to the object before it is emitted (ADR-0169). A `FuncId` becomes the
        // `SymbolId` a relocation names only here, because `ObjectProduct` is what holds the mapping.
        if let Some((comp_dir, primary)) = unit {
            let functions: Vec<debug::FunctionLines> = function_lines
                .into_iter()
                .map(|pending| debug::FunctionLines {
                    symbol: product.function_symbol(pending.id),
                    length: pending.length,
                    rows: pending.rows,
                })
                .collect();
            debug::emit(
                &mut product.object,
                &lines,
                &functions,
                &comp_dir,
                &primary,
                endian,
            )
            .map_err(|e| CodegenError::Internal(format!("cannot write debug info: {e}")))?;
        }

        product
            .emit()
            .map_err(|e| CodegenError::Internal(format!("cannot emit an object: {e}")))
    }

    /// The libraries every `#foreign` declaration named, deduplicated.
    ///
    /// `jr-link` needs these for the link line. They are collected during the declare
    /// phase rather than rediscovered, because ADR-0019 §4 made the resolution happen
    /// exactly once and this is the third consumer reading it. On the trait since
    /// ADR-0143 §6, because a driver that names a concrete back end to ask can drive
    /// only one.
    fn libraries(&self) -> &[String] {
        &self.libraries
    }
}

/// The signature of the trap helper: a message pointer and a length, no result.
fn trap_signature(module: &ObjectModule) -> cranelift_codegen::ir::Signature {
    let pointer = module.target_config().pointer_type();
    let mut signature = module.make_signature();
    signature.call_conv = CallConv::SystemV;
    signature.params.push(AbiParam::new(pointer));
    signature.params.push(AbiParam::new(pointer));
    signature
}

/// The libcall naming Cranelift uses for its own helpers.
///
/// **Delegated to `cranelift-module`'s own namer rather than derived from `Display`.** The
/// hand-rolled `format!("{libcall}")` produced Cranelift's *internal* spelling — `Memcpy`,
/// capitalised — where the C library exports `memcpy`, so any emitted libcall failed to
/// link. That was latent for the whole project: nothing emitted one until ADR-0051's
/// aggregate return copied a struct big enough for `emit_small_memory_copy` to stop
/// unrolling and call `memcpy` instead. A 16-byte `Vec2` inlines and links; a 64-byte
/// struct did not, which is why the corpus program returns both sizes.
fn default_libcall_names() -> Box<dyn Fn(cranelift_codegen::ir::LibCall) -> String + Send + Sync> {
    cranelift_module::default_libcall_names()
}
