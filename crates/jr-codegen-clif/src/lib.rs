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
mod repr;
mod trap;

pub use trap::{TRAP_HELPER, TrapKind};

use cranelift_codegen::Context;
use cranelift_codegen::ir::{AbiParam, Function, InstBuilder as _, TrapCode, UserFuncName, types};
use cranelift_codegen::isa::CallConv;
use cranelift_codegen::settings::{self, Configurable as _};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use jr_codegen::{Backend, CodegenError, ProcDecl, ProcKind};
use jr_mir::{MirBody, ProcRef};
use jr_pool::{Item, Pool, PoolId, StrId, TargetLayout};
use rustc_hash::FxHashMap;

/// The Cranelift implementation of [`Backend`].
pub struct ClifBackend {
    module: ObjectModule,
    /// The Cranelift id of every declared procedure.
    ids: FxHashMap<ProcRef, FuncId>,
    /// Whether a declared procedure is `#foreign`, which decides its call
    /// convention and whether a body may be defined for it.
    foreign: FxHashMap<ProcRef, bool>,
    /// The data object holding each string constant's bytes.
    strings: FxHashMap<StrId, DataId>,
    /// The data object holding each trap kind's message.
    trap_messages: FxHashMap<TrapKind, DataId>,
    /// The runtime helper a trap calls.
    trap_helper: FuncId,
    /// Every library a `#foreign` declaration named, for the link line.
    libraries: Vec<String>,
    /// The Jairs procedure the `main` shim calls, and its return type.
    entry: Option<(ProcRef, PoolId)>,
}

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
    pub fn new(pool: &Pool, name: &str) -> Result<Self, CodegenError> {
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

        let mut backend = Self {
            module,
            ids: FxHashMap::default(),
            foreign: FxHashMap::default(),
            strings: FxHashMap::default(),
            trap_messages: FxHashMap::default(),
            trap_helper,
            libraries: Vec::new(),
            entry: None,
        };
        backend.emit_trap_messages()?;
        backend.emit_strings(pool)?;
        backend.define_trap_helper()?;
        Ok(backend)
    }

    /// The libraries every `#foreign` declaration named, deduplicated.
    ///
    /// `jr-link` needs these for the link line. They are collected during the declare
    /// phase rather than rediscovered, because ADR-0019 §4 made the resolution happen
    /// exactly once and this is the third consumer reading it.
    #[must_use]
    pub fn libraries(&self) -> &[String] {
        &self.libraries
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
        let Some((entry, ret)) = self.entry else {
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
        let call = builder.ins().call(callee_ref, &[]);
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

    /// Emits one read-only data object per trap message.
    fn emit_trap_messages(&mut self) -> Result<(), CodegenError> {
        for kind in TrapKind::ALL {
            let id = self
                .module
                .declare_data(kind.symbol(), Linkage::Local, false, false)
                .map_err(|e| CodegenError::Internal(format!("trap message: {e}")))?;
            let mut description = DataDescription::new();
            description.define(kind.message().as_bytes().to_vec().into_boxed_slice());
            self.module
                .define_data(id, &description)
                .map_err(|e| CodegenError::Internal(format!("trap message: {e}")))?;
            self.trap_messages.insert(kind, id);
        }
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
            &describe,
        )?;

        let id = self
            .module
            .declare_function(&name, linkage, &signature)
            .map_err(|e| CodegenError::Internal(format!("cannot declare {name}: {e}")))?;
        self.ids.insert(decl.proc, id);
        self.foreign.insert(decl.proc, foreign);
        if matches!(decl.kind, ProcKind::Local { entry: true, .. }) {
            self.entry = Some((decl.proc, decl.ret));
        }
        Ok(())
    }

    fn define(
        &mut self,
        proc: ProcRef,
        mir: &MirBody,
        pool: &Pool,
        layout: TargetLayout,
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
            trap_helper,
            trap_messages: &self.trap_messages,
        };
        body::translate(&mut builder, &mut self.module, &ctx, proc, mir)?;
        builder.finalize(self.module.target_config());

        let mut context = Context::for_function(function);
        self.module
            .define_function(id, &mut context)
            .map_err(|e| CodegenError::Internal(format!("cannot define a body: {e}")))?;
        Ok(())
    }

    fn finalise(mut self: Box<Self>) -> Result<Vec<u8>, CodegenError> {
        self.define_entry_shim()?;
        self.module
            .finish()
            .emit()
            .map_err(|e| CodegenError::Internal(format!("cannot emit an object: {e}")))
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
fn default_libcall_names() -> Box<dyn Fn(cranelift_codegen::ir::LibCall) -> String + Send + Sync> {
    Box::new(|libcall| format!("{libcall}"))
}
