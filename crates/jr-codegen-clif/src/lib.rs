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
use cranelift_codegen::ir::{Function, UserFuncName};
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
        let isa = cranelift_native::builder()
            .map_err(|e| CodegenError::Internal(format!("no host backend: {e}")))?
            .finish(settings::Flags::new(flags))
            .map_err(|e| CodegenError::Internal(format!("cannot build an ISA: {e}")))?;

        let builder = ObjectBuilder::new(isa, name.as_bytes().to_vec(), default_libcall_names())
            .map_err(|e| CodegenError::Internal(format!("cannot build an object: {e}")))?;
        let mut module = ObjectModule::new(builder);

        let mut signature = module.make_signature();
        let pointer = module.target_config().pointer_type();
        signature
            .params
            .push(cranelift_codegen::ir::AbiParam::new(pointer));
        signature
            .params
            .push(cranelift_codegen::ir::AbiParam::new(pointer));
        signature.call_conv = CallConv::SystemV;
        let trap_helper = module
            .declare_function(TRAP_HELPER, Linkage::Import, &signature)
            .map_err(|e| CodegenError::Internal(format!("cannot declare {TRAP_HELPER}: {e}")))?;

        let mut backend = Self {
            module,
            ids: FxHashMap::default(),
            foreign: FxHashMap::default(),
            strings: FxHashMap::default(),
            trap_messages: FxHashMap::default(),
            trap_helper,
            libraries: Vec::new(),
        };
        backend.emit_trap_messages()?;
        backend.emit_strings(pool)?;
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

    fn finalise(self: Box<Self>) -> Result<Vec<u8>, CodegenError> {
        self.module
            .finish()
            .emit()
            .map_err(|e| CodegenError::Internal(format!("cannot emit an object: {e}")))
    }
}

/// The libcall naming Cranelift uses for its own helpers.
fn default_libcall_names() -> Box<dyn Fn(cranelift_codegen::ir::LibCall) -> String + Send + Sync> {
    Box::new(|libcall| format!("{libcall}"))
}
