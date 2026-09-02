//! The LLVM back end, and the only crate in the workspace that names an `inkwell` type.
//!
//! # Why this crate exists and what it proves
//!
//! ADR-0009 put every `cranelift-*` reference behind [`jr_codegen::Backend`] on the argument
//! that "what makes wave W8's LLVM back end an addition rather than a rewrite" is the trait.
//! Until this crate had a body, that argument was a guess: an interface with one
//! implementation is a description of that implementation.
//!
//! Using it found two things the trait was missing, both fixed rather than worked around
//! (ADR-0143 §6): it could not tell the driver which libraries to link, and the *words* a
//! trapping program prints lived inside the Cranelift crate.
//!
//! # Why a third engine is worth its weight
//!
//! The corpus differential compares the VM against Cranelift and asserts exit codes rather
//! than mere agreement, because two engines agreeing is not two engines being right. A third
//! independent lowering is the strongest available check on what is *shared* — MIR, the pool's
//! layout, and `jr_base::trap_message` — and it gives a bug in either existing engine's own
//! reading of MIR two witnesses instead of one.
//!
//! # The one thing this crate must never do
//!
//! Compute a size, an alignment or an offset — and in a typed IR that prohibition has a
//! sharper form (ADR-0143 §4): **no Jairs aggregate acquires an LLVM `StructType`.** Building
//! one would put LLVM's own padding and alignment rules in charge of where a field sits, which
//! is a second computation of the thing ADR-0018 §2 says must exist once, and the failure is
//! silent. So LLVM is used as an instruction selector and a register allocator, not as a type
//! system: an aggregate is bytes at offsets this compiler chose, exactly as it is in the other
//! two engines.
//!
//! # Not built here
//!
//! LLVM's own optimisation passes. ADR-0142's `-O` level selects how much the **mid-end**
//! rewrites MIR; it does not reach a back end, in either back end. Adding a pass pipeline is
//! its own decision, with a benchmark behind it.

#![cfg(feature = "llvm")]

mod body;
mod repr;

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use inkwell::module::{Linkage, Module};
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine,
};

use inkwell::values::{FunctionValue, GlobalValue};
use jr_codegen::{Backend, CodegenError, ProcDecl, ProcKind, TRAP_HELPER, TrapKind, TrapLocations};
use jr_mir::{MirBody, ProcRef};
use jr_pool::{Item, Layout, Pool, PoolId, StrId, TargetLayout, layout_of};
use rustc_hash::FxHashMap;

use crate::repr::pointer_int;

/// How many frames the shadow call stack holds (ADR-0066 §1).
///
/// The VM's `MAX_DEPTH` is 256 and the Cranelift back end matches it, so a program that
/// recurses to the VM's limit gets the same backtrace from all three engines.
const SHADOW_CAPACITY: usize = 256;

/// The LLVM implementation of [`Backend`].
///
/// # Why the context is borrowed rather than owned
///
/// Every inkwell value borrows its [`Context`], so a back end owning one could not hand out
/// anything derived from it — the classic self-referential struct. The driver creates the
/// context and passes it in, which is also what lets a test hold one across several
/// compilations.
pub struct LlvmBackend<'ctx> {
    context: &'ctx Context,
    module: Module<'ctx>,
    target: TargetLayout,
    /// The LLVM function for every declared procedure.
    funcs: FxHashMap<ProcRef, FunctionValue<'ctx>>,
    /// Whether a declared procedure is `#foreign`, which decides whether a body may be
    /// defined for it.
    foreign: FxHashMap<ProcRef, bool>,
    /// The global holding each string constant's bytes.
    strings: FxHashMap<StrId, GlobalValue<'ctx>>,
    /// The runtime helper a trap calls.
    trap_helper: FunctionValue<'ctx>,
    /// Every library a `#foreign` declaration named, for the link line.
    libraries: Vec<String>,
    /// The Jairs procedure the `main` shim calls, its return type, and whether it takes a
    /// context.
    entry: Option<(ProcRef, PoolId, bool)>,
    /// The context struct's layout, remembered when the entry is declared so the shim can
    /// size the slot it allocates for `main`'s context (ADR-0057 §5).
    entry_context: Option<Layout>,
    /// The shadow call stack a trap reports (ADR-0066 §1), and its live depth.
    shadow: (GlobalValue<'ctx>, GlobalValue<'ctx>),
    /// The read-only global holding each procedure's source name, and its length.
    names: FxHashMap<ProcRef, (GlobalValue<'ctx>, usize)>,
    /// Trap-message globals already emitted, keyed by their bytes.
    messages: FxHashMap<String, GlobalValue<'ctx>>,
}

impl<'ctx> LlvmBackend<'ctx> {
    /// Creates a back end targeting the host.
    ///
    /// String constants are given globals here, up front and in pool order, which mirrors
    /// `jr-vm`'s `intern_strings` and the Cranelift back end deliberately: all three
    /// deduplicate by [`StrId`] rather than by contents, so a program's set of string objects
    /// is the same whichever engine runs it.
    ///
    /// # Errors
    /// [`CodegenError::Internal`] when the host target cannot be described.
    pub fn new(
        context: &'ctx Context,
        pool: &Pool,
        target: TargetLayout,
        name: &str,
    ) -> Result<Self, CodegenError> {
        Target::initialize_native(&InitializationConfig::default())
            .map_err(|e| CodegenError::Internal(format!("no native LLVM target: {e}")))?;
        let module = context.create_module(name);
        module.set_triple(&TargetMachine::get_default_triple());

        let word = pointer_int(context, target);
        // The helper takes a message address and a length and does not return. Both are
        // pointer-width integers, matching this back end's rule that a pointer is an integer.
        let helper_type = context
            .void_type()
            .fn_type(&[word.into(), word.into()], false);
        let trap_helper = module.add_function(TRAP_HELPER, helper_type, Some(Linkage::Internal));

        // The shadow call stack and its depth (ADR-0066 §1), both **writable** — the only
        // mutable data this back end emits. Zero-initialised, so a program that never calls
        // anything has a depth of 0 and the helper walks nothing. Two words per frame: the
        // name's address, then its length.
        let stack_type = word.array_type(
            u32::try_from(SHADOW_CAPACITY * 2)
                .map_err(|_| CodegenError::Internal("the shadow stack is too large".to_owned()))?,
        );
        let stack = module.add_global(stack_type, None, "jr$shadow$stack");
        stack.set_initializer(&stack_type.const_zero());
        stack.set_linkage(Linkage::Internal);

        let depth = module.add_global(word, None, "jr$shadow$depth");
        depth.set_initializer(&word.const_zero());
        depth.set_linkage(Linkage::Internal);

        let mut backend = Self {
            context,
            module,
            target,
            funcs: FxHashMap::default(),
            foreign: FxHashMap::default(),
            strings: FxHashMap::default(),
            trap_helper,
            libraries: Vec::new(),
            entry: None,
            entry_context: None,
            shadow: (stack, depth),
            names: FxHashMap::default(),
            messages: FxHashMap::default(),
        };
        backend.emit_strings(pool)?;
        backend.define_trap_helper()?;
        Ok(backend)
    }

    /// The LLVM IR of the module so far, for a test or a `-Z` flag to read.
    ///
    /// Textual IR rather than a parsed structure, because what a reader wants from a back end
    /// under suspicion is the thing LLVM's own tools consume.
    #[must_use]
    pub fn print_ir(&self) -> String {
        self.module.print_to_string().to_string()
    }

    /// Emits one read-only global per interned string.
    ///
    /// Not NUL-terminated: a Jairs string is `{data, count}` and carries its own length
    /// (ADR-0004), which is exactly the shape `write` wants.
    fn emit_strings(&mut self, pool: &Pool) -> Result<(), CodegenError> {
        for index in 0..pool.len() {
            let id = PoolId::from_usize(index);
            let Item::StrValue(str_id) = *pool.item(id) else {
                continue;
            };
            if self.strings.contains_key(&str_id) {
                continue;
            }
            let text = pool.resolve_str(str_id);
            // An empty string still needs an address, so one padding byte is emitted; `count`
            // is zero, so it is never read.
            let bytes: Vec<u8> = if text.is_empty() {
                vec![0]
            } else {
                text.as_bytes().to_vec()
            };
            let symbol = format!("jr$str${}", str_id.index());
            let initializer = self.context.const_string(&bytes, false);
            let global = self
                .module
                .add_global(initializer.get_type(), None, &symbol);
            global.set_initializer(&initializer);
            global.set_constant(true);
            global.set_linkage(Linkage::Internal);
            self.strings.insert(str_id, global);
        }
        Ok(())
    }

    /// A read-only global holding `bytes`, under `symbol`.
    fn literal_global(&self, symbol: &str, bytes: &[u8]) -> GlobalValue<'ctx> {
        let initializer = self.context.const_string(bytes, false);
        let global = self.module.add_global(initializer.get_type(), None, symbol);
        global.set_initializer(&initializer);
        global.set_constant(true);
        global.set_linkage(Linkage::Internal);
        global
    }

    /// Declares a libc function whose parameters are all pointer-width integers.
    ///
    /// `write` and `exit` are the same two symbols `modules/Basic` declares `#foreign`, with
    /// the same shapes, so LLVM resolves both declarations to one import.
    fn declare_libc(&self, name: &str, arity: usize, returns: bool) -> FunctionValue<'ctx> {
        if let Some(existing) = self.module.get_function(name) {
            return existing;
        }
        let word = pointer_int(self.context, self.target);
        let params: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> =
            (0..arity).map(|_| word.into()).collect();
        let ty = if returns {
            word.fn_type(&params, false)
        } else {
            self.context.void_type().fn_type(&params, false)
        };
        self.module.add_function(name, ty, Some(Linkage::External))
    }

    /// Generates the body of the trap helper.
    ///
    /// ADR-0019 §2 chose "a call into a runtime helper that reports and aborts", and the
    /// Cranelift back end generates it into the object rather than linking a runtime, for
    /// reasons that hold here identically: the helper needs only libc `write` and `exit`, both
    /// of which the program is already linked against.
    ///
    /// It writes the message to file descriptor 2, walks the shadow stack innermost-first
    /// writing `  in <name>` per frame, and exits with [`TrapKind::EXIT_STATUS`] — the status
    /// `jr run` uses, which is what lets a script driving the compiler treat all three
    /// engines alike.
    fn define_trap_helper(&mut self) -> Result<(), CodegenError> {
        let word = pointer_int(self.context, self.target);
        let write = self.declare_libc("write", 3, true);
        let exit = self.declare_libc("exit", 1, false);

        let function = self.trap_helper;
        let entry = self.context.append_basic_block(function, "entry");
        let header = self.context.append_basic_block(function, "frame_test");
        let frame = self.context.append_basic_block(function, "frame");
        let done = self.context.append_basic_block(function, "done");
        let builder = self.context.create_builder();
        builder.position_at_end(entry);

        let message = function
            .get_nth_param(0)
            .ok_or_else(|| CodegenError::Internal("the helper lost its message".to_owned()))?
            .into_int_value();
        let length = function
            .get_nth_param(1)
            .ok_or_else(|| CodegenError::Internal("the helper lost its length".to_owned()))?
            .into_int_value();
        let stderr = word.const_int(2, false);
        builder
            .build_call(
                write,
                &[stderr.into(), message.into(), length.into()],
                "report",
            )
            .map_err(|e| CodegenError::Internal(format!("llvm builder: {e}")))?;

        // **The backtrace** (ADR-0066 §2). The message written above carries the reason and
        // the location, both compile-time constants; the chain is the part only run time
        // knows, so it is written here by walking the shadow stack downward — innermost frame
        // first, which is the order the VM renders and `trap_message`'s tests pin.
        //
        // Three `write` calls per frame ("  in ", the name, "\n"), because assembling one
        // buffer would need an allocator, which a trap handler must not use. The two literals
        // are the same bytes `jr_base::trap_message` writes.
        let prefix = self.literal_global("jr$frame$prefix", b"  in ");
        let newline = self.literal_global("jr$frame$newline", b"\n");
        let (stack, depth_global) = self.shadow;

        let depth = builder
            .build_load(word, depth_global.as_pointer_value(), "depth")
            .map_err(|e| CodegenError::Internal(format!("llvm builder: {e}")))?
            .into_int_value();
        builder
            .build_unconditional_branch(header)
            .map_err(|e| CodegenError::Internal(format!("llvm builder: {e}")))?;

        builder.position_at_end(header);
        let index = builder
            .build_phi(word, "index")
            .map_err(|e| CodegenError::Internal(format!("llvm builder: {e}")))?;
        index.add_incoming(&[(&depth, entry)]);
        let counter = index.as_basic_value().into_int_value();
        let more = builder
            .build_int_compare(
                inkwell::IntPredicate::UGT,
                counter,
                word.const_zero(),
                "more",
            )
            .map_err(|e| CodegenError::Internal(format!("llvm builder: {e}")))?;
        builder
            .build_conditional_branch(more, frame, done)
            .map_err(|e| CodegenError::Internal(format!("llvm builder: {e}")))?;

        builder.position_at_end(frame);
        let next = builder
            .build_int_sub(counter, word.const_int(1, false), "next")
            .map_err(|e| CodegenError::Internal(format!("llvm builder: {e}")))?;
        let width = u64::from(self.target.pointer_size);
        let byte_offset = builder
            .build_int_mul(next, word.const_int(width * 2, false), "off")
            .map_err(|e| CodegenError::Internal(format!("llvm builder: {e}")))?;
        let entry_ptr = unsafe {
            builder.build_in_bounds_gep(
                self.context.i8_type(),
                stack.as_pointer_value(),
                &[byte_offset],
                "entry",
            )
        }
        .map_err(|e| CodegenError::Internal(format!("llvm builder: {e}")))?;
        let name = builder
            .build_load(word, entry_ptr, "name")
            .map_err(|e| CodegenError::Internal(format!("llvm builder: {e}")))?
            .into_int_value();
        let len_ptr = unsafe {
            builder.build_in_bounds_gep(
                self.context.i8_type(),
                entry_ptr,
                &[word.const_int(width, false)],
                "lenp",
            )
        }
        .map_err(|e| CodegenError::Internal(format!("llvm builder: {e}")))?;
        let name_len = builder
            .build_load(word, len_ptr, "len")
            .map_err(|e| CodegenError::Internal(format!("llvm builder: {e}")))?
            .into_int_value();

        for (text, count) in [
            (prefix.as_pointer_value(), Some(5u64)),
            (stack.as_pointer_value(), None),
            (newline.as_pointer_value(), Some(1)),
        ] {
            let (address, size) = match count {
                Some(size) => (
                    builder
                        .build_ptr_to_int(text, word, "lit")
                        .map_err(|e| CodegenError::Internal(format!("llvm builder: {e}")))?,
                    word.const_int(size, false),
                ),
                // The middle write is the name, whose address and length were loaded above.
                None => (name, name_len),
            };
            builder
                .build_call(
                    write,
                    &[stderr.into(), address.into(), size.into()],
                    "frame_out",
                )
                .map_err(|e| CodegenError::Internal(format!("llvm builder: {e}")))?;
        }
        index.add_incoming(&[(&next, frame)]);
        builder
            .build_unconditional_branch(header)
            .map_err(|e| CodegenError::Internal(format!("llvm builder: {e}")))?;

        builder.position_at_end(done);
        let status = word.const_int(TrapKind::EXIT_STATUS as u64, false);
        builder
            .build_call(exit, &[status.into()], "abort")
            .map_err(|e| CodegenError::Internal(format!("llvm builder: {e}")))?;
        // `exit` does not return, but LLVM needs a terminator and cannot know that.
        builder
            .build_unreachable()
            .map_err(|e| CodegenError::Internal(format!("llvm builder: {e}")))?;
        Ok(())
    }

    /// Emits the `main` the system linker expects.
    ///
    /// A Jairs `main` is an ordinary procedure with a mangled symbol; this is a separate C
    /// `main` that calls it and returns a real process status. The reason is worth recording
    /// because the Cranelift back end demonstrated it: with the Jairs procedure named `main`
    /// directly, a `void`-returning procedure leaves the return register holding whatever it
    /// last held and the C runtime hands that to `exit`.
    fn define_entry_shim(&mut self) -> Result<(), CodegenError> {
        let Some((entry, ret, receives_context)) = self.entry else {
            // No entry point is not an error here: a library or a test may want an object
            // with no `main` in it. The driver refuses earlier when a *program* declares none.
            return Ok(());
        };
        let callee = *self
            .funcs
            .get(&entry)
            .ok_or(CodegenError::Undeclared(entry))?;

        let i32_type = self.context.i32_type();
        let shim_type = i32_type.fn_type(&[], false);
        let shim = self
            .module
            .add_function("main", shim_type, Some(Linkage::External));
        let block = self.context.append_basic_block(shim, "entry");
        let builder = self.context.create_builder();
        builder.position_at_end(block);
        let internal = |e: inkwell::builder::BuilderError| {
            CodegenError::Internal(format!("llvm builder: {e}"))
        };

        let word = pointer_int(self.context, self.target);
        let mut args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = Vec::new();
        // **`main`'s context is a zeroed stack slot in the shim** (ADR-0057 §5): `main` has no
        // Jairs caller, so the shim is where the first one is born. Zeroed, so
        // `context.allocator` reads 0 in a program that never sets it.
        if receives_context {
            let layout = self.entry_context.ok_or_else(|| {
                CodegenError::Internal(
                    "entry takes a context but its layout was never recorded".to_owned(),
                )
            })?;
            let size = u32::try_from(layout.size.max(1)).map_err(|_| {
                CodegenError::Internal("the context is larger than a u32".to_owned())
            })?;
            let slot = builder
                .build_alloca(self.context.i8_type().array_type(size), "context")
                .map_err(internal)?;
            builder
                .build_memset(
                    slot,
                    layout.align.max(1),
                    self.context.i8_type().const_zero(),
                    word.const_int(layout.size.max(1), false),
                )
                .map_err(internal)?;
            let address = builder
                .build_ptr_to_int(slot, word, "ctx")
                .map_err(internal)?;
            args.push(address.into());
        }

        // **`main`'s own frame** (ADR-0066 §1). Every other frame is pushed by its caller, and
        // `main`'s caller is this shim — so without this the native backtrace ends one frame
        // short of the VM's. Never popped: `main` returning means the program is over.
        if let Some((name_global, name_len)) = self.names.get(&entry).copied() {
            let (stack, depth_global) = self.shadow;
            let name = builder
                .build_ptr_to_int(name_global.as_pointer_value(), word, "name")
                .map_err(internal)?;
            // Depth is 0 here — this is the first frame — so the entry goes at offset 0.
            builder
                .build_store(stack.as_pointer_value(), name)
                .map_err(internal)?;
            let len_ptr = unsafe {
                builder.build_in_bounds_gep(
                    self.context.i8_type(),
                    stack.as_pointer_value(),
                    &[word.const_int(u64::from(self.target.pointer_size), false)],
                    "lenp",
                )
            }
            .map_err(internal)?;
            builder
                .build_store(len_ptr, word.const_int(name_len as u64, false))
                .map_err(internal)?;
            builder
                .build_store(depth_global.as_pointer_value(), word.const_int(1, false))
                .map_err(internal)?;
        }

        let call = builder
            .build_call(callee, &args, "jairs_main")
            .map_err(internal)?;
        // An integer `main` returns its own value, narrowed to a C `int`. `void` produces no
        // result at all, which is why this is a match on presence rather than on the type.
        let status = match call.try_as_basic_value().basic() {
            Some(value) if ret != PoolId::VOID => {
                let returned = value.into_int_value();
                match returned.get_type().get_bit_width().cmp(&32) {
                    std::cmp::Ordering::Equal => returned,
                    std::cmp::Ordering::Greater => builder
                        .build_int_truncate(returned, i32_type, "status")
                        .map_err(internal)?,
                    std::cmp::Ordering::Less => builder
                        .build_int_s_extend(returned, i32_type, "status")
                        .map_err(internal)?,
                }
            }
            _ => i32_type.const_zero(),
        };
        builder.build_return(Some(&status)).map_err(internal)?;
        Ok(())
    }
}

impl<'ctx> Backend for LlvmBackend<'ctx> {
    fn declare(
        &mut self,
        decl: &ProcDecl,
        pool: &Pool,
        layout: TargetLayout,
    ) -> Result<(), CodegenError> {
        let (name, linkage, foreign) = match &decl.kind {
            ProcKind::Local { symbol, entry } => (
                symbol.clone(),
                // The entry point must be visible to the system linker; nothing else needs to
                // be. `Internal` rather than `Private` so a symbol table still names it,
                // which is what makes a native backtrace readable in a debugger.
                if *entry {
                    Linkage::External
                } else {
                    Linkage::Internal
                },
                false,
            ),
            ProcKind::Foreign(symbol) => {
                if let Some(library) = &symbol.library
                    && !self.libraries.iter().any(|held| held == library)
                {
                    self.libraries.push(library.clone());
                }
                (symbol.symbol.clone(), Linkage::External, true)
            }
        };

        let proc = decl.proc;
        let describe = move |what: &str| CodegenError::Unsupported {
            proc,
            what: what.to_owned(),
        };
        let ty = repr::function_type(
            self.context,
            pool,
            layout,
            &decl.params,
            decl.ret,
            foreign,
            decl.receives_context,
            &describe,
        )?;

        // A `#foreign` symbol may already exist: `write` and `exit` are declared by the trap
        // helper, with the same shapes, and re-adding one would give LLVM two symbols of the
        // same name (`write.1`) — one of which nothing calls.
        let function = match self.module.get_function(&name) {
            Some(existing) => existing,
            None => self.module.add_function(&name, ty, Some(linkage)),
        };
        self.funcs.insert(decl.proc, function);
        self.foreign.insert(decl.proc, foreign);

        // The read-only string a backtrace frame names (ADR-0066 §3), one per procedure. The
        // *source* name, not the mangled symbol: a reader wants `countdown`, not `jr$0$3`.
        // Skipped for a procedure with no name, whose frame is then omitted rather than
        // printed as a placeholder.
        if let Some(source_name) = &decl.name {
            let symbol = format!(
                "jr$name${}${}",
                decl.proc.file.index(),
                decl.proc.proc.index()
            );
            let global = self.literal_global(&symbol, source_name.as_bytes());
            self.names.insert(decl.proc, (global, source_name.len()));
        }

        if matches!(decl.kind, ProcKind::Local { entry: true, .. }) {
            self.entry = Some((decl.proc, decl.ret, decl.receives_context));
            if decl.receives_context {
                let ctx = pool.context_type_id().unwrap_or(PoolId::ERROR);
                if let Ok(context_layout) = layout_of(pool, layout, ctx) {
                    self.entry_context = Some(context_layout);
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
        let function = *self
            .funcs
            .get(&proc)
            .ok_or(CodegenError::Undeclared(proc))?;
        if self.foreign.get(&proc).copied().unwrap_or(false) {
            return Err(CodegenError::Internal(
                "a body was defined for a `#foreign` procedure".to_owned(),
            ));
        }

        let shared = body::Shared {
            pool,
            target: layout,
            funcs: &self.funcs,
            strings: &self.strings,
            trap_helper: self.trap_helper,
            locations,
            shadow: self.shadow,
            names: &self.names,
            shadow_capacity: SHADOW_CAPACITY,
        };
        body::translate(
            self.context,
            &self.module,
            &shared,
            proc,
            mir,
            function,
            &mut self.messages,
        )
    }

    fn finalise(mut self: Box<Self>) -> Result<Vec<u8>, CodegenError> {
        self.define_entry_shim()?;

        // **Verified before it is emitted.** LLVM's verifier catches a malformed module — a
        // `phi` missing a predecessor, a value of the wrong class — and its message names the
        // instruction. Without this the failure surfaces as a wrong object file or a crash
        // inside the code generator, which is the class of failure this project refuses to
        // debug from the far end.
        self.module
            .verify()
            .map_err(|e| CodegenError::Internal(format!("invalid LLVM module: {e}")))?;

        let triple = TargetMachine::get_default_triple();
        let target = Target::from_triple(&triple)
            .map_err(|e| CodegenError::Internal(format!("no LLVM target: {e}")))?;
        let machine = target
            .create_target_machine(
                &triple,
                TargetMachine::get_host_cpu_name().to_str().unwrap_or(""),
                TargetMachine::get_host_cpu_features()
                    .to_str()
                    .unwrap_or(""),
                // **No LLVM optimisation.** ADR-0142's `-O` selects how much the mid-end
                // rewrites MIR and does not reach a back end; asking LLVM for `-O2` here
                // would make one engine's arithmetic pass through an optimiser the others do
                // not have, on a project whose central claim is that they agree.
                OptimizationLevel::None,
                // Position-independent code is not optional on Apple platforms, and every
                // modern target wants it anyway.
                RelocMode::PIC,
                CodeModel::Default,
            )
            .ok_or_else(|| CodegenError::Internal("cannot build a target machine".to_owned()))?;

        let buffer = machine
            .write_to_memory_buffer(&self.module, FileType::Object)
            .map_err(|e| CodegenError::Internal(format!("cannot emit an object: {e}")))?;
        Ok(buffer.as_slice().to_vec())
    }

    fn libraries(&self) -> &[String] {
        &self.libraries
    }
}

/// Drives a whole LLVM build, owning the [`Context`] the values borrow.
///
/// The driver cannot create the context itself without naming `inkwell`, which ADR-0009's
/// confinement forbids — so it hands over the *loop* instead: `drive` receives the back end
/// and performs ADR-0019 §1's declare and define phases, and this function does the rest.
///
/// Returns the object bytes and the libraries the link line needs.
///
/// # Errors
/// Whatever `drive` reports, or a [`CodegenError`] from creating, verifying or emitting the
/// module.
pub fn build(
    pool: &Pool,
    target: TargetLayout,
    name: &str,
    drive: &dyn Fn(&mut dyn Backend) -> Result<(), String>,
) -> Result<(Vec<u8>, Vec<String>), String> {
    let context = Context::create();
    let mut backend = LlvmBackend::new(&context, pool, target, name).map_err(|e| e.to_string())?;
    drive(&mut backend)?;
    let libraries = backend.libraries().to_vec();
    let object = Box::new(backend).finalise().map_err(|e| e.to_string())?;
    Ok((object, libraries))
}
