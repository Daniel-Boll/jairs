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
use inkwell::debug_info::AsDIScope as _;
use inkwell::module::{Linkage, Module};
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine,
};

use inkwell::values::{FunctionValue, GlobalValue};
use jr_codegen::{Backend, CodegenError, ProcDecl, ProcKind, SourceInfo, TRAP_HELPER, TrapKind};
use jr_mir::{MirBody, ProcRef};
use jr_pool::{Item, Layout, Pool, PoolId, StrId, TargetLayout, field_offset, layout_of};
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
    /// The constant global holding each compiler-emitted table's bytes (ADR-0152 §1).
    static_arrays: FxHashMap<PoolId, GlobalValue<'ctx>>,
    /// The runtime helper a trap calls.
    trap_helper: FunctionValue<'ctx>,
    /// The module's debug-info builder and compilation unit, once a body has told us the file (ADR-0170).
    ///
    /// Lazy because a compilation unit names a file and a directory, and this back end learns those from the
    /// first body that reports a position — the same reason the Cranelift back end's `unit` is an `Option`
    /// (ADR-0169 §1). A build with no positions gets no debug info at all rather than a unit naming nothing.
    debug: Option<(
        inkwell::debug_info::DebugInfoBuilder<'ctx>,
        inkwell::debug_info::DICompileUnit<'ctx>,
    )>,
    /// Each declared procedure's parameter and return types, for its subprogram's DWARF signature.
    ///
    /// Kept because `define` needs them and receives only a `ProcRef`: `declare` is where a `ProcDecl` exists,
    /// and rediscovering a signature later would mean asking the front end, which ADR-0009 forbids.
    signature_types: FxHashMap<ProcRef, (Vec<PoolId>, PoolId)>,
    /// Each declared procedure's parameter names, interned (ADR-0171 §3).
    ///
    /// Resolved to text only when debug info is being emitted, which is why they are kept as `Symbol`s.
    parameter_names: FxHashMap<ProcRef, Vec<jr_base::Symbol>>,
    /// A `DIType` per pool type, so a struct's DIE is emitted once (ADR-0171).
    ///
    /// Keyed by `PoolId`, which the pool already deduplicated by *structure* — two identical struct
    /// declarations are one `PoolId`, so this cache inherits that and cannot emit a duplicate DIE for a type a
    /// debugger would then show twice.
    debug_types: FxHashMap<PoolId, inkwell::debug_info::DIType<'ctx>>,
    /// A `DIFile` per source path, so a body from an imported module names *its* file.
    ///
    /// Without this every subprogram hangs off the compilation unit's file and DWARF attributes an imported
    /// module's statements to the root program — which was this wave's first wrong result: the file table had
    /// one entry and `modules/Basic`'s lines were blamed on `024-hello.jr`.
    debug_files: FxHashMap<String, inkwell::debug_info::DIFile<'ctx>>,
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
            static_arrays: FxHashMap::default(),
            trap_helper,
            libraries: Vec::new(),
            entry: None,
            entry_context: None,
            shadow: (stack, depth),
            names: FxHashMap::default(),
            messages: FxHashMap::default(),
            debug: None,
            debug_files: FxHashMap::default(),
            debug_types: FxHashMap::default(),
            signature_types: FxHashMap::default(),
            parameter_names: FxHashMap::default(),
        };
        backend.emit_strings(pool)?;
        backend.define_trap_helper()?;
        Ok(backend)
    }

    /// The `DIType` for `ty`, building it and its members if this is the first ask (ADR-0171).
    ///
    /// `None` for a type this wave does not describe — see the match's own arms for which and why. A `None`
    /// propagates: a struct with one undescribable field has no DIE either, because a struct DIE listing
    /// *some* of its members would show a debugger a type whose fields do not add up to its size, which is
    /// worse than showing it nothing.
    ///
    /// # Why the recursion terminates
    ///
    /// A pointer's DIE refers to its pointee, and a struct's to its fields — so a self-referential type
    /// (`Node :: struct { next: *Node; }`) would recurse forever. It does not, because a **pointer stops the
    /// walk**: `create_pointer_type` is given the pointee's DIE only when the pointee is already cached, and
    /// otherwise the pointer is described as opaque. That loses `next.next.value` in a debugger and keeps the
    /// compiler from hanging, which is the right trade for a first pass and is why this is stated rather than
    /// assumed.
    fn debug_type(
        &mut self,
        pool: &Pool,
        ty: PoolId,
        file: inkwell::debug_info::DIFile<'ctx>,
        info_names: &dyn SourceInfo,
    ) -> Option<inkwell::debug_info::DIType<'ctx>> {
        if let Some(found) = self.debug_types.get(&ty) {
            return Some(*found);
        }
        self.debug.as_ref()?;
        let layout = layout_of(pool, self.target, ty).ok()?;
        let bits = layout.size.checked_mul(8)?;
        let align_bits = layout.align.checked_mul(8)?;

        // DWARF's own encodings. Raw values rather than named constants, because inkwell takes the `u32` and
        // the numbers are fixed by the standard.
        const DW_ATE_ADDRESS: u32 = 0x01;
        const DW_ATE_BOOLEAN: u32 = 0x02;
        const DW_ATE_FLOAT: u32 = 0x04;
        const DW_ATE_SIGNED: u32 = 0x05;
        const DW_ATE_UNSIGNED: u32 = 0x07;

        // **Every recursive call happens before the builder is borrowed.** `self.debug` is behind a shared
        // borrow while a DIE is built and `debug_type` needs `&mut self` to cache, so holding the builder
        // across the recursion does not compile — which is a borrow checker enforcing a real ordering rather
        // than getting in the way: a member's DIE must exist before the struct that lists it.
        let described = match pool.item(ty) {
            Item::BoolType => {
                let (info, _) = self.debug.as_ref()?;
                info.create_basic_type("bool", bits, DW_ATE_BOOLEAN, 0)
                    .ok()
                    .map(|basic| basic.as_type())
            }
            Item::IntType { signed, bits: n } => {
                let (signed, n) = (*signed, *n);
                let (info, _) = self.debug.as_ref()?;
                let name = if signed {
                    format!("s{n}")
                } else {
                    format!("u{n}")
                };
                let encoding = if signed {
                    DW_ATE_SIGNED
                } else {
                    DW_ATE_UNSIGNED
                };
                info.create_basic_type(&name, bits, encoding, 0)
                    .ok()
                    .map(|basic| basic.as_type())
            }
            Item::FloatType { bits: n } => {
                let n = *n;
                let (info, _) = self.debug.as_ref()?;
                info.create_basic_type(&format!("float{n}"), bits, DW_ATE_FLOAT, 0)
                    .ok()
                    .map(|basic| basic.as_type())
            }
            Item::PointerType(pointee) => {
                let pointee = *pointee;
                // The pointee is described first, then looked up — so `*Point` carries `Point`'s members while
                // a self-referential `*Node` inside `Node` finds nothing cached yet and falls back to opaque.
                // That is the recursion's terminator, stated in this method's own docs.
                let inner = self.debug_type(pool, pointee, file, info_names);
                let (info, _) = self.debug.as_ref()?;
                match inner {
                    Some(inner) => Some(
                        info.create_pointer_type(
                            "",
                            inner,
                            bits,
                            align_bits,
                            inkwell::AddressSpace::default(),
                        )
                        .as_type(),
                    ),
                    None => info
                        .create_basic_type("*", bits, DW_ATE_ADDRESS, 0)
                        .ok()
                        .map(|basic| basic.as_type()),
                }
            }
            Item::StructType { decl, .. } => {
                let decl = *decl;
                let fields = pool.struct_fields(decl)?.to_vec();
                // Every member's DIE, name and offset, gathered before the builder is borrowed.
                let mut gathered = Vec::with_capacity(fields.len());
                for (index, field) in fields.iter().enumerate() {
                    let member = self.debug_type(pool, field.ty, file, info_names)?;
                    let (offset, member_layout) =
                        field_offset(pool, self.target, ty, u32::try_from(index).ok()?).ok()?;
                    gathered.push((
                        info_names.symbol(field.name).unwrap_or_default(),
                        member,
                        offset.checked_mul(8)?,
                        member_layout.size.checked_mul(8)?,
                        member_layout.align.checked_mul(8)?,
                    ));
                }
                let (info, _) = self.debug.as_ref()?;
                let members: Vec<_> = gathered
                    .into_iter()
                    .map(|(name, member, offset, size, align)| {
                        info.create_member_type(
                            file.as_debug_info_scope(),
                            &name,
                            file,
                            0,
                            size,
                            align,
                            offset,
                            0,
                            member,
                        )
                        .as_type()
                    })
                    .collect();
                Some(
                    info.create_struct_type(
                        file.as_debug_info_scope(),
                        // **Anonymous**, because the pool does not record a struct's *declared* name: an
                        // `Item::StructType` carries a `DeclId`, and the name lives on the HIR item that bound
                        // it, which a back end cannot see (ADR-0009). DWARF permits an unnamed struct type and
                        // `lldb` shows it with its members, which is where the value is — a reader wants `p.x`
                        // and its offset far more than the type's spelling. Recorded as owed rather than faked
                        // from the `DeclId`, which would print a number no reader recognises.
                        "",
                        file,
                        0,
                        bits,
                        align_bits,
                        0,
                        None,
                        &members,
                        0,
                        None,
                        "",
                    )
                    .as_type(),
                )
            }
            // Deliberately undescribed, and each for its own reason rather than as one bucket:
            //
            // * `VoidType` has no DIE by definition — DWARF spells a void return as an *absent* type, which is
            //   exactly what `create_subroutine_type` does with a `None` return.
            // * `StringType`, a view, an array, a union and a variant all have real DWARF spellings
            //   (`DW_TAG_array_type`, `DW_TAG_union_type`, and a tagged variant is a struct of a discriminant
            //   and a union). Each needs a decision about *naming* — a `[]s64` has no user-written name — and a
            //   wave that guessed at four of those at once would be four guesses.
            // * A procedure type wants a `DW_TAG_subroutine_type` with parameters, which is the same work the
            //   subprogram already does and is worth sharing rather than duplicating.
            _ => None,
        }?;
        self.debug_types.insert(ty, described);
        Some(described)
    }

    /// Creates the module's debug-info builder and compilation unit for `path`.
    ///
    /// **`DWARFEmissionKind::Full` and `debug_info_version` handled by inkwell.** LLVM strips every `!dbg`
    /// from a module whose `llvm.module.flags` lacks `"Debug Info Version"`, silently — a module that
    /// verifies, emits, and carries no line table. `create_debug_info_builder` sets the flag, which is the
    /// one good reason to use it rather than the raw C API.
    ///
    /// `DWARFSourceLanguage::C` because there is no `DW_LANG_Jairs` and inventing a number would make every
    /// consumer fall back to a default anyway. C is the closest honest answer for a language with C's
    /// pointers, C's integers and C's calling convention.
    fn create_unit(
        &self,
        path: &str,
    ) -> (
        inkwell::debug_info::DebugInfoBuilder<'ctx>,
        inkwell::debug_info::DICompileUnit<'ctx>,
    ) {
        let (name, directory) = split_path(path);
        self.module.create_debug_info_builder(
            // Allow unresolved: a subprogram is created before its body's instructions exist, so forward
            // references are the normal case rather than an error.
            true,
            inkwell::debug_info::DWARFSourceLanguage::C,
            &name,
            &directory,
            "jairs",
            // Not optimised — see the subprogram's own flag for why.
            false,
            "",
            0,
            "",
            inkwell::debug_info::DWARFEmissionKind::Full,
            0,
            false,
            false,
            "",
            "",
        )
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
        self.emit_static_arrays(pool)?;
        Ok(())
    }

    /// Emits every compiler-emitted table as an internal constant global (ADR-0152 §1).
    ///
    /// Runs after the string pass because a table may contain a `string`, whose `data` word is a
    /// pointer to one of those globals — and LLVM lets that be expressed *directly*, as a constant
    /// expression referring to the other global, which is why this back end needs no relocation
    /// bookkeeping of its own.
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

            // **A string's pointer is a constant expression, not a byte.** LLVM has no
            // post-hoc relocation API on a byte initialiser, so the global is built as a *packed
            // struct of chunks*: the bytes before each pointer, then the pointer as a
            // `ptrtoint` of the string's own global, then the bytes after. LLVM emits the
            // relocation for that itself.
            //
            // The chunk offsets come from the pool's image walk, so the layout is still the one
            // shared computation — this back end differs only in how it *expresses* an address it
            // cannot know yet, which is the same thing Cranelift's `write_data_addr` does.
            let mut patches: Vec<(u64, StrId)> = Vec::new();
            let bytes = {
                let mut resolve = |str_id: StrId, at: u64| {
                    patches.push((at, str_id));
                    0
                };
                jr_pool::static_image(pool, self.target, elem, &values, &mut resolve)
                    .map_err(|reason| CodegenError::NoLayout { ty: elem, reason })?
            };
            let bytes = if bytes.is_empty() { vec![0u8] } else { bytes };
            patches.sort_unstable_by_key(|(at, _)| *at);

            let pointer_width = usize::try_from(self.target.pointer_size).unwrap_or(8);
            let symbol = format!("jr$table${}", id.index());
            let mut chunks: Vec<inkwell::values::BasicValueEnum<'ctx>> = Vec::new();
            let mut cursor = 0usize;
            for (at, str_id) in &patches {
                let at = usize::try_from(*at).unwrap_or(0);
                if at > cursor && at <= bytes.len() {
                    chunks.push(self.context.const_string(&bytes[cursor..at], false).into());
                }
                let string_global = *self.strings.get(str_id).ok_or_else(|| {
                    CodegenError::Internal("a table names a string with no global".to_owned())
                })?;
                let pointer_int_ty = pointer_int(self.context, self.target);
                chunks.push(
                    string_global
                        .as_pointer_value()
                        .const_to_int(pointer_int_ty)
                        .into(),
                );
                cursor = at + pointer_width;
            }
            if cursor < bytes.len() {
                chunks.push(self.context.const_string(&bytes[cursor..], false).into());
            }

            // Packed, so LLVM inserts no padding of its own: the pool already placed every byte.
            let initializer = self.context.const_struct(&chunks, true);
            let global = self
                .module
                .add_global(initializer.get_type(), None, &symbol);
            global.set_initializer(&initializer);
            global.set_constant(true);
            global.set_linkage(Linkage::Internal);
            self.static_arrays.insert(id, global);
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
        // Kept for the subprogram's DWARF signature, which `define` builds and which has only a `ProcRef`.
        self.signature_types
            .insert(decl.proc, (decl.params.clone(), decl.ret));
        self.parameter_names
            .insert(decl.proc, decl.param_names.clone());

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
        locations: &dyn SourceInfo,
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

        // The first position this body reports, which names both the compilation unit's file and the
        // subprogram's line. Computed once: walking the blocks twice for the same answer would be the kind
        // of quiet duplication that drifts when one copy is edited.
        let first = mir
            .blocks()
            .iter()
            .flat_map(|block| block.stmts.iter().map(body::statement_span))
            .find_map(|span| locations.position(span));

        // The compilation unit, created from the first body that reports a position (ADR-0170 §1).
        if self.debug.is_none()
            && let Some(at) = &first
        {
            self.debug = Some(self.create_unit(&at.path));
        }

        // This function's subprogram. LLVM rejects a location whose scope is not the enclosing function's,
        // so one is minted per body and attached before any instruction carries a location.
        // The `DIFile` for this body's own path, created once per path.
        if let Some((info, _)) = &self.debug
            && let Some(at) = &first
            && !self.debug_files.contains_key(&at.path)
        {
            let (name, directory) = split_path(&at.path);
            let file = info.create_file(&name, &directory);
            self.debug_files.insert(at.path.clone(), file);
        }
        let body_file = first
            .as_ref()
            .and_then(|at| self.debug_files.get(&at.path).copied());

        // The signature's type DIEs, built before the subprogram so the subroutine type can reference them
        // (ADR-0171 §1). A type no DIE references is dropped by LLVM, so the subprogram's signature is what
        // makes a struct's layout actually reach the object.
        let signature = self.debug.as_ref().and_then(|(_, unit)| {
            let file = body_file.unwrap_or_else(|| unit.get_file());
            self.signature_types
                .get(&proc)
                .cloned()
                .map(|types| (file, types))
        });
        let (return_die, param_dies) = match signature {
            Some((file, (params, ret))) => {
                // A `void` return has no DIE by DWARF's own rule — an absent type *is* void — so
                // `debug_type` returning `None` for it is the right answer rather than a gap.
                let ret_die = self.debug_type(pool, ret, file, locations);
                // Holes kept, so an index still lines up with `parameter_names` — a `filter_map` here would
                // silently shift every later parameter's name onto the wrong type.
                let param_dies: Vec<Option<_>> = params
                    .iter()
                    .map(|ty| self.debug_type(pool, *ty, file, locations))
                    .collect();
                (ret_die, param_dies)
            }
            None => (None, Vec::new()),
        };

        // Per-slot names, type DIEs and lines, built before the builder is borrowed (ADR-0172 §1). Only a
        // slot that stands for a *source local* gets an entry — a compiler temporary has no name worth
        // showing, and holes are kept so an index still identifies its slot.
        let slot_debug: Vec<Option<(String, inkwell::debug_info::DIType<'ctx>, u32)>> =
            if self.debug.is_some() {
                let file = body_file;
                let mut out = Vec::with_capacity(mir.slot_count());
                for index in 0..mir.slot_count() {
                    let slot = mir.slot(jr_mir::SlotId::from_usize(index));
                    let entry = match (slot.local, file) {
                        (Some(local), Some(file)) => locations.local_name(local).and_then(|name| {
                            let die = self.debug_type(pool, slot.ty, file, locations)?;
                            let line = locations.position(slot.span).map_or(1, |at| at.line);
                            Some((name, die, line))
                        }),
                        _ => None,
                    };
                    out.push(entry);
                }
                out
            } else {
                Vec::new()
            };

        // Parameter names, resolved before the builder is borrowed. `arg{n}` when a name is unavailable — the
        // index is real information, unlike a guessed identifier.
        let parameter_names: Vec<String> = self
            .parameter_names
            .get(&proc)
            .map(|names| {
                names
                    .iter()
                    .enumerate()
                    .map(|(index, symbol)| {
                        locations
                            .symbol(*symbol)
                            .unwrap_or_else(|| format!("arg{index}"))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let parameter_line = first.as_ref().map_or(1, |at| at.line);

        let scope = self.debug.as_ref().map(|(info, unit)| {
            // This body's own file, falling back to the unit's when the body reported no position — a body
            // with no positions gets no locations either, so the fallback is never what a row points at.
            let file = body_file.unwrap_or_else(|| unit.get_file());
            let present: Vec<_> = param_dies.iter().flatten().copied().collect();
            let subroutine = info.create_subroutine_type(file, return_die, &present, 0);
            // The LLVM function's own symbol, rather than a name looked up elsewhere: `self.names` holds the
            // *global* carrying a backtrace string, not a `String`, and a debugger wants the linkage name it
            // will see in the symbol table anyway.
            let name = function.get_name().to_string_lossy().into_owned();
            let line = first.as_ref().map_or(1, |at| at.line);
            let subprogram = info.create_function(
                unit.as_debug_info_scope(),
                &name,
                None,
                file,
                line,
                subroutine,
                // Not local to the unit, and *is* a definition: this is the body, not a declaration.
                false,
                true,
                line,
                0,
                // Not optimised. ADR-0142's `-O` reaches the mid-end and never LLVM, and claiming otherwise
                // would make a debugger warn about variables it can in fact see.
                false,
            );
            function.set_subprogram(subprogram);

            // **A `DILocalVariable` per parameter, which is what makes a type DIE reach the object**
            // (ADR-0171 §3). LLVM prunes a type nothing *declares*: a `DISubroutineType` listing a struct is
            // not enough, and without these the struct DIE was silently absent while base types appeared —
            // which looked like the struct mapping being broken and was the reference being missing.
            //
            // A parameter variable earns its place independently too: `lldb` can print `p.x` at a breakpoint.
            for (index, die) in param_dies.iter().enumerate() {
                let Some(die) = die else {
                    continue;
                };
                let name = parameter_names
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| format!("arg{index}"));
                // `arg_no` is one-based in DWARF, and `always_preserve` is true so an unoptimised build keeps
                // the variable even when nothing reads it — which is the whole point at `-O0`.
                let _ = info.create_parameter_variable(
                    subprogram.as_debug_info_scope(),
                    &name,
                    u32::try_from(index).unwrap_or(0) + 1,
                    file,
                    parameter_line,
                    *die,
                    true,
                    0,
                );
            }

            body::DebugScope {
                info,
                subprogram,
                file,
                slots: &slot_debug,
            }
        });

        let shared = body::Shared {
            pool,
            target: layout,
            funcs: &self.funcs,
            strings: &self.strings,
            static_arrays: &self.static_arrays,
            trap_helper: self.trap_helper,
            locations,
            shadow: self.shadow,
            names: &self.names,
            foreign: &self.foreign,
            shadow_capacity: SHADOW_CAPACITY,
            debug: scope,
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

        // **Finalise the debug info before verifying.** An unfinalised `DIBuilder` leaves temporary metadata
        // nodes in the module, and LLVM's verifier rejects them — with a message about a malformed node
        // rather than about a missing call, which is a bad half-hour. This is also why the entry shim above
        // carries no location: it is emitted after every body, has no source of its own, and giving it one
        // would attribute the program's exit to whichever line came last.
        if let Some((info, _)) = &self.debug {
            info.finalize();
        }

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

/// Splits a source path into the file name and directory a DWARF file entry wants.
///
/// A free function because both the compilation unit and every per-body `DIFile` need it, and two copies of
/// "what counts as this file's directory" is exactly the kind of duplication that drifts — one copy gaining a
/// `current_dir` fallback the other lacks would make the unit and a file disagree about where source lives.
fn split_path(path: &str) -> (String, String) {
    let file = std::path::Path::new(path);
    let name = file
        .file_name()
        .map_or_else(|| path.to_owned(), |n| n.to_string_lossy().into_owned());
    let directory = file
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
    (name, directory)
}
