//! The register machine: one program counter, one frame per call, no block dispatch.
//!
//! # Why the loop is this small
//!
//! Every hard question was answered before execution began. `jr-mir` decided which
//! register holds each value and checked that none is read before it is written;
//! `lower.rs` linearised the blocks, turned every projection into a byte offset, and
//! replaced block parameters with copies. What is left is a `match` on an
//! instruction and a program counter, which is the shape ADR-0018 §1 was chosen to
//! produce.
//!
//! # Why every string is interned into memory before anything runs
//!
//! A `string` constant is a `{data, count}` pair (ADR-0004), and `data` has to point
//! at real bytes. Allocating those bytes lazily during a call would put them above
//! that frame's high-water mark, so returning from the frame would release them and
//! leave the cache holding a dangling address — a bug that would only show up in the
//! *second* call that used the same literal. So [`Vm::new`] walks the pool once and
//! allocates every interned string up front, below every frame mark that will ever
//! be taken. The pool is finite and already built, so this is a bounded, one-time
//! cost with no ordering hazard.
//!
//! # Why undefined values propagate but do not compute
//!
//! `Rvalue::Undef` becomes [`Value::Undefined`], and a `Move` of one is legal: `c:
//! s64 = ---;` followed by `d := c` copies nothing-in-particular, which is what the
//! source says. It is *using* one — as an operand, a condition, or a stored value —
//! that traps. Inventing a zero instead would hide exactly the bug E0227 reports
//! statically, and this is the path taken when that check was skipped.
//!
//! # Why recursion is bounded
//!
//! A Jairs call is a Rust recursive call, so an unbounded one overflows the
//! *compiler's* stack — which is the single failure mode a compiler must never have,
//! for the same reason `MirBody::reverse_postorder` uses an explicit stack. The depth
//! cap turns it into [`VmError::Exhausted`].

use jr_base::FileId;
use jr_hir::ProcId;
use jr_mir::{BinOp, Callee, GlobalData, GlobalRef, NumKind, ProcRef, UnOp, Unreachable};
use jr_pool::{Item, Pool, PoolId, StrId, TargetLayout, string_count, string_data, string_layout};
use rustc_hash::FxHashMap;

use crate::code::{
    Code, ForeignProc, Instr, Operand, PlacePlan, PlaceRoot, PlaceStep, Routine, Shape,
};
use crate::error::{Trap, TrapSite, VmError, ice};
use crate::lower::shape_of;
use crate::memory::Memory;
use crate::value::{Address, IntKind, Value};

/// How deep Jairs calls may nest before the VM gives up.
///
/// 256 frames. Deliberately far below what the host stack could take, because the
/// point is to fail diagnosably rather than to maximise depth; the slice's deepest
/// call chain is `main` → `print` → `write`.
pub const MAX_DEPTH: usize = 256;

/// How many instructions **compile-time** execution may run before the VM gives up (ADR-0121).
///
/// Ten million: far past any constant a program folds — the whole corpus's compile-time work is orders of
/// magnitude below it — and well under a second, which is what matters. Without it a `#run while true {}`
/// hung the compiler outright, and under `jr lsp` it hung the worker thread on a file the user had merely
/// *opened*, because salsa's cancellation cannot reach a loop that never touches the database.
///
/// Deliberately **not** applied under [`Mode::Runtime`]: there the interpreter is running the user's own
/// program under `jr run`, where a long loop is the program working rather than the compiler hanging, and a
/// budget would refuse legitimate work. So this bounds *compilation*, which is the thing a user did not ask
/// to be unbounded.
pub const MAX_COMPTIME_STEPS: u64 = 10_000_000;

// ---------------------------------------------------------------------------
// Execution mode
// ---------------------------------------------------------------------------

/// Whether the VM is evaluating compile-time code or running a program.
///
/// This is ADR-0006's distinction, finally given somewhere to live. ADR-0006 allows
/// compile-time code to call foreign functions *behind an explicit
/// `#foreign_at_comptime` allowance*, which wave W6 introduces and which therefore
/// does not exist yet. The bridge does exist, so without a mode the VM would grant
/// comptime FFI years early — silently, and to every program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Evaluating `#run` or a constant. A foreign call is refused (ADR-0006).
    Comptime,
    /// Running a program under `jr run`. A foreign call is the program working.
    Runtime,
}

// ---------------------------------------------------------------------------
// Program
// ---------------------------------------------------------------------------

/// Every routine the VM might call, and the layout they were compiled for.
///
/// Built by `jr-db`, which is the only layer that can see more than one file. The
/// VM resolves a [`ProcRef`] by lookup rather than by asking a query, so that the
/// interpreter has no opinion about incrementality.
#[derive(Debug, Clone)]
pub struct Program {
    routines: FxHashMap<ProcRef, Routine>,
    /// Every file-scope global the program declares, keyed by its cross-file identity (ADR-0186 §1).
    globals: FxHashMap<GlobalRef, GlobalData>,
    target: TargetLayout,
}

impl Program {
    /// An empty program for `target`.
    #[must_use]
    pub fn new(target: TargetLayout) -> Self {
        Self {
            routines: FxHashMap::default(),
            globals: FxHashMap::default(),
            target,
        }
    }

    /// Adds a routine, replacing any already registered for the same procedure.
    pub fn insert(&mut self, routine: Routine) {
        let proc = match &routine {
            Routine::Bytecode(code) => code.proc,
            Routine::Foreign(foreign) => foreign.proc,
        };
        self.routines.insert(proc, routine);
    }

    /// Records one file-scope global's type and initial value, replacing any already registered
    /// for the same [`GlobalRef`].
    pub fn insert_global(&mut self, global: GlobalRef, data: GlobalData) {
        self.globals.insert(global, data);
    }

    /// Every global the program declares, in [`GlobalRef`]'s `Ord` order.
    ///
    /// Sorted rather than left in hash order: `Vm::emit_globals` lays each one out at a fixed
    /// offset in the globals region, and that offset must be the same on every run of the same
    /// program — a `HashMap`'s iteration order is not (ADR-0186 §3).
    #[must_use]
    pub fn globals(&self) -> Vec<(GlobalRef, GlobalData)> {
        let mut globals: Vec<(GlobalRef, GlobalData)> = self
            .globals
            .iter()
            .map(|(global, data)| (*global, *data))
            .collect();
        globals.sort_by_key(|(global, _)| *global);
        globals
    }

    /// The routine for a procedure, if the program has one.
    #[must_use]
    pub fn routine(&self, proc: ProcRef) -> Option<&Routine> {
        self.routines.get(&proc)
    }

    /// The layout every routine here was compiled for.
    #[must_use]
    pub const fn target(&self) -> TargetLayout {
        self.target
    }

    /// How many routines the program holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.routines.len()
    }

    /// Whether the program holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.routines.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Frames
// ---------------------------------------------------------------------------

/// One call's registers and slot addresses.
struct Frame {
    regs: Vec<Value>,
    slots: Vec<Address>,
}

// ---------------------------------------------------------------------------
// The machine
// ---------------------------------------------------------------------------

/// A running interpreter.
pub struct Vm<'a> {
    program: &'a Program,
    pool: &'a Pool,
    memory: Memory,
    mode: Mode,
    depth: usize,
    /// Where each interned string's bytes live. Complete before execution starts.
    strings: FxHashMap<StrId, Address>,
    /// Where each compiler-emitted table's bytes live in this VM's memory (ADR-0152 §1).
    ///
    /// Keyed on the table's `PoolId`, not on its contents: the pool deduplicated by contents when it
    /// interned the item, so two identical tables are one id and get one emission.
    static_arrays: FxHashMap<PoolId, Address>,
    /// Where each file-scope global's storage lives — one program-lifetime address per
    /// [`GlobalRef`], shared by every frame (ADR-0186 §1). Complete before execution starts, by
    /// `Vm::emit_globals`, the same way [`Self::strings`] is.
    globals: FxHashMap<GlobalRef, Address>,
    /// Bytes a foreign write produced, when the bridge is capturing rather than
    /// writing through. Empty under [`Mode::Runtime`].
    captured: Vec<u8>,
    /// The instruction currently executing, for [`Vm::trap_site`].
    at: Option<TrapSite>,
    /// The procedures whose frames are live, outermost first (ADR-0066 §1).
    ///
    /// Pushed and popped in [`Vm::call`], beside `depth` — which this deliberately does *not* replace:
    /// `depth` also counts frames the shadow stack does not distinguish, and conflating "how deep are
    /// we" with "what is the chain" would make `MAX_DEPTH` depend on this feature.
    frames: Vec<ProcRef>,
    /// The chain as it stood when a trap was raised, innermost frame last.
    ///
    /// Snapshotted once, by the innermost frame to observe a [`VmError::Trap`], because `frames` is
    /// unwound as the error propagates and a caller reading it afterwards would see only its own
    /// prefix. `None` until something traps.
    trap_frames: Option<Vec<ProcRef>>,
    /// Instructions left before compile-time execution is refused (ADR-0121).
    ///
    /// `u64::MAX` under [`Mode::Runtime`], which is effectively unmetered — see [`MAX_COMPTIME_STEPS`] for
    /// why only compilation is bounded. A plain counter rather than an `Option` so the hot loop pays one
    /// decrement and one branch.
    fuel: u64,
}

impl<'a> Vm<'a> {
    /// Creates a VM over `program`, interning every string constant and laying out every global,
    /// both before any frame mark exists.
    ///
    /// # Errors
    /// [`VmError::Exhausted`] if the string constants and globals alone do not fit.
    pub fn new(program: &'a Program, pool: &'a Pool, mode: Mode) -> Result<Self, VmError> {
        let mut vm = Self {
            program,
            pool,
            memory: Memory::new(),
            mode,
            depth: 0,
            strings: FxHashMap::default(),
            static_arrays: FxHashMap::default(),
            globals: FxHashMap::default(),
            captured: Vec::new(),
            at: None,
            frames: Vec::new(),
            trap_frames: None,
            fuel: match mode {
                Mode::Comptime => MAX_COMPTIME_STEPS,
                Mode::Runtime => u64::MAX,
            },
        };
        vm.intern_strings()?;
        vm.emit_globals()?;
        Ok(vm)
    }

    /// Allocates every interned string's bytes, before any frame mark exists.
    fn intern_strings(&mut self) -> Result<(), VmError> {
        for index in 0..self.pool.len() {
            let id = PoolId::from_usize(index);
            if let Item::StrValue(str_id) = *self.pool.item(id) {
                if self.strings.contains_key(&str_id) {
                    continue;
                }
                let bytes = self.pool.resolve_str(str_id).as_bytes().to_vec();
                let address = self.memory.allocate_bytes(&bytes, 1)?;
                self.strings.insert(str_id, address);
            }
        }
        self.emit_static_arrays()?;
        Ok(())
    }

    /// Writes every compiler-emitted table into this VM's memory, once (ADR-0152 §1).
    ///
    /// The same shape as the string pass above and run in the same place, because the two solve the
    /// same problem: a *pointer* can never be a pool value (ADR-0074 found that interning one gave the
    /// evaluator's own address), so the pool holds contents and each engine supplies an address.
    ///
    /// **The byte image comes from `jr_pool::static_image`**, not from a walk written here. Three
    /// engines emit these bytes, and a byte image is offsets plus widths — the computation ADR-0018 §2
    /// centralised in the pool so the VM and both back ends cannot disagree. This function supplies
    /// only the one thing the pool must never hold: the addresses of its own interned strings.
    ///
    /// Keyed on the table's `PoolId`, because the pool already deduplicated by contents when it
    /// interned the item — two identical tables *are* one id, so one emission.
    fn emit_static_arrays(&mut self) -> Result<(), VmError> {
        for index in 0..self.pool.len() {
            let id = PoolId::from_usize(index);
            if self.static_arrays.contains_key(&id) {
                continue;
            }
            let Some(values) = self.pool.static_array_values(id).map(<[PoolId]>::to_vec) else {
                continue;
            };
            let elem = self
                .pool
                .view_elem(self.pool.type_of(id))
                .ok_or_else(|| VmError::internal("a static table with no element type"))?;
            let target = self.program.target;
            let layout = jr_pool::layout_of(self.pool, target, elem)
                .map_err(|e| VmError::internal(format!("a static table's element: {e}")))?;

            // Every string the table mentions is already in `self.strings`: the string pass above runs
            // first and covers every `StrValue` in the pool, including the ones nested inside these
            // tables. A missing one would be an ordering bug, so it is an error rather than a zero.
            let strings = self.strings.clone();
            let mut missing = None;
            let bytes = {
                // The offset is for a *native* engine, which must record a relocation rather than
                // answer with a number. The VM has real addresses, so it ignores it.
                let mut resolve = |str_id: jr_pool::StrId, _at: u64| match strings.get(&str_id) {
                    Some(address) => *address,
                    None => {
                        missing = Some(str_id);
                        0
                    }
                };
                jr_pool::static_image(self.pool, target, elem, &values, &mut resolve)
                    .map_err(|e| VmError::internal(format!("a static table's image: {e}")))?
            };
            if missing.is_some() {
                return Err(VmError::internal(
                    "a static table names a string that was not interned",
                ));
            }
            let address = self.memory.allocate_bytes(&bytes, layout.align)?;
            self.static_arrays.insert(id, address);
        }
        Ok(())
    }

    /// Lays out and initialises every global the program declares, before any frame mark exists.
    ///
    /// # Why this runs after strings and static tables
    ///
    /// A global's initialiser can itself intern a string or a compiler-emitted table — `g: string
    /// = "hi";` folds to an `Item::StrValue`, and [`Self::constant`] resolves one through
    /// [`Self::strings`]. Both passes must already hold real addresses before a global's bytes are
    /// rendered, or the render finds nothing there. This runs from [`Self::new`] right after
    /// [`Self::intern_strings`] (which itself runs [`Self::emit_static_arrays`]), so the ordering
    /// holds by construction.
    ///
    /// # Where the region lands, and why the offsets are stable
    ///
    /// There is no separate "globals region" type: a global is bump-allocated from the same
    /// [`Memory`] a call frame is, and it is the **first** thing ever allocated in a fresh `Vm` — a
    /// call frame's mark is always taken after this runs, so [`Memory::release`] can only ever
    /// rewind *down to* the end of this region, never through it. That is what gives a global
    /// program lifetime rather than frame lifetime (ADR-0186 §1) with no new machinery: the bump
    /// allocator's existing "never reclaimed below the caller's mark" guarantee already covers it,
    /// the same way it already covers an interned string constant.
    ///
    /// Each global's offset is therefore fixed by [`Program::globals`]'s sorted order plus every
    /// earlier global's `jr-pool` layout — deterministic across runs of the same program, though
    /// nothing needs it to be a *particular* number, only a stable one shared by every place lowered
    /// against this program.
    fn emit_globals(&mut self) -> Result<(), VmError> {
        let target = self.program.target;
        for (global, data) in self.program.globals() {
            let layout = jr_pool::layout_of(self.pool, target, data.ty)
                .map_err(|e| VmError::internal(format!("a global's type has no layout: {e}")))?;
            let address = self.memory.allocate(layout.size, layout.align)?;
            self.globals.insert(global, address);
            let Some(init) = data.init else {
                // Zero-initialised (ADR-0186 §2): `Memory::allocate` hands out bytes the region
                // already zero-filled and this is the first thing ever written there, so there is
                // nothing left to do.
                continue;
            };
            let value = self.constant(init)?;
            self.write_value(address, shape_of(self.pool, data.ty), layout.size, &value)?;
        }
        Ok(())
    }

    /// The `{data, count}` view of a compiler-emitted table (ADR-0152 §1).
    fn static_array_value(&mut self, id: PoolId) -> Result<Value, VmError> {
        let address = *self
            .static_arrays
            .get(&id)
            .ok_or_else(|| VmError::internal("a static table was not emitted"))?;
        let count = self.pool.static_array_values(id).map_or(0, <[PoolId]>::len) as u64;
        let target = self.program.target;
        let layout = string_layout(target);
        let (data_offset, data) = string_data(target);
        let (count_offset, count_layout) = string_count(target);
        let mut bytes = vec![0u8; usize::try_from(layout.size).unwrap_or(16)];
        write_le(&mut bytes, data_offset, data.size, address);
        write_le(&mut bytes, count_offset, count_layout.size, count);
        Ok(Value::Aggregate(bytes))
    }

    /// Bytes written to standard output by a captured foreign call.
    #[must_use]
    pub fn captured_output(&self) -> &[u8] {
        &self.captured
    }

    /// Where the last trap happened, if one did.
    ///
    /// Valid only after a call returned [`VmError::Trap`]: execution stops at a trap,
    /// so the instruction recorded is the one that raised it. Reported as MIR identity
    /// rather than as a rendered location because resolving one needs the file's HIR
    /// and a `SourceMap`, neither of which the VM has — see [`TrapSite`] and
    /// ADR-0020 §3.
    #[must_use]
    pub const fn trap_site(&self) -> Option<TrapSite> {
        self.at
    }

    /// The procedure frames that were live when a trap was raised, **innermost first**.
    ///
    /// Valid only after a call returned [`VmError::Trap`], for the reason [`Vm::trap_site`] is: the
    /// snapshot is taken at the trap and nothing resets it. Empty when nothing trapped.
    ///
    /// Reversed here rather than at the push site, because innermost-first is a *rendering* order
    /// (ADR-0066 §2) while the stack's natural order is outermost-first — and one of the two has to
    /// flip, so it flips at the boundary where the meaning changes.
    ///
    /// Returns identities rather than names for the reason [`TrapSite`] does: resolving a `ProcRef` to
    /// a name needs the file's HIR, which the VM does not have.
    #[must_use]
    pub fn trap_frames(&self) -> Vec<ProcRef> {
        match &self.trap_frames {
            Some(frames) => frames.iter().rev().copied().collect(),
            None => Vec::new(),
        }
    }

    /// The VM's memory, for inspecting a result that lives there.
    #[must_use]
    pub const fn memory(&self) -> &Memory {
        &self.memory
    }

    /// Mutable access to the VM's memory, for the FFI bridge to satisfy `malloc` from the VM's own
    /// region (ADR-0061 §1) rather than from the host.
    pub const fn memory_mut(&mut self) -> &mut Memory {
        &mut self.memory
    }

    /// Copies the bytes a `string` value points at out of VM memory.
    ///
    /// Needed because a `string` result cannot survive the VM: it is a `{data, count}`
    /// pair (ADR-0004) whose `data` is an address in [`Self::memory`], and that memory
    /// goes away with the VM. Anything that wants to keep the string — const
    /// evaluation interning a folded `#run` back into the pool, for instance — has to
    /// copy the bytes out *while the VM is still alive*, which is what this is for.
    ///
    /// # Errors
    /// [`VmError::Internal`] if the value is not a `{data, count}` pair of the right
    /// size, and [`VmError::Trap`] if it points outside the VM's memory.
    pub fn read_string(&self, value: &Value) -> Result<Vec<u8>, VmError> {
        let target = self.program.target;
        let bytes = value.aggregate()?;
        let expected = string_layout(target).size;
        if bytes.len() as u64 != expected {
            return Err(VmError::internal(format!(
                "a string value is {} bytes, expected {expected}",
                bytes.len()
            )));
        }
        let (data_offset, data) = string_data(target);
        let (count_offset, count_layout) = string_count(target);
        let address = read_le(bytes, data_offset, data.size);
        let count = read_le(bytes, count_offset, count_layout.size);
        if count == 0 {
            // An empty string's pointer is never dereferenced, and the pool interns
            // `""`, so there is nothing to read and nothing to bounds-check.
            return Ok(Vec::new());
        }
        Ok(self.memory.read(address, count)?.to_vec())
    }

    /// Allocates a zeroed context and returns a pointer to it (ADR-0057 §5).
    ///
    /// `main` has no Jairs caller, so something must create the first context. **Zeroed rather than
    /// uninitialised**, so `context.allocator` reads 0 in a program that never sets it — a defined
    /// value rather than garbage, matching what ADR-0039 §4a decided for a default-initialised
    /// aggregate.
    ///
    /// # Errors
    /// [`VmError`] if the allocation fails.
    pub fn new_context(&mut self, size: u64, align: u32) -> Result<Value, VmError> {
        let address = self.memory.allocate(size.max(1), align)?;
        let zeros = vec![0u8; usize::try_from(size).unwrap_or(0)];
        if !zeros.is_empty() {
            self.memory.write(address, &zeros)?;
        }
        Ok(Value::Scalar(address))
    }

    /// Calls a procedure with `args` and returns its result.
    ///
    /// # Errors
    /// Any [`VmError`]; see that type for whose fault each variant is.
    pub fn call(&mut self, target: ProcRef, args: Vec<Value>) -> Result<Value, VmError> {
        if self.depth >= MAX_DEPTH {
            return Err(VmError::Exhausted("call depth"));
        }
        let routine = self
            .program
            .routine(target)
            .ok_or_else(|| ice::no_such_routine(target))?;
        self.depth += 1;
        // **The shadow call stack** (ADR-0066 §1): the frames a trap reports. Pushed here rather than
        // in `execute`, so a `#foreign` call is on it too while it runs — and popped unconditionally
        // below, including on the error path, because a trap propagating out of a callee must not leave
        // its frame behind for a later trap to report.
        //
        // `ProcRef` rather than a name: one word, already both engines' identity for a procedure, and
        // names are resolved for *rendering* by the side holding the HIR (ADR-0020 §4's split).
        self.frames.push(target);
        let result = match routine {
            Routine::Bytecode(code) => self.execute(code, args),
            Routine::Foreign(foreign) => self.foreign(foreign, args),
        };
        // **The chain is snapshotted on the way out of the frame that trapped**, not on every
        // instruction: `self.at` is updated per instruction (see `execute`), and cloning the stack that
        // often would make every arithmetic op allocate. Instead the *innermost* frame to see a `Trap`
        // records the whole live stack, and outer frames leave that snapshot alone as the error
        // propagates — so the recorded chain is the one that existed at the trap rather than the
        // partially-unwound one an outer frame would see.
        if matches!(result, Err(VmError::Trap(_))) && self.trap_frames.is_none() {
            self.trap_frames = Some(self.frames.clone());
        }
        self.frames.pop();
        self.depth -= 1;
        result
    }

    // -------------------------------------------------------------------
    // One frame
    // -------------------------------------------------------------------

    fn execute(&mut self, code: &'a Code, args: Vec<Value>) -> Result<Value, VmError> {
        if args.len() != code.params.len() {
            return Err(VmError::internal(format!(
                "called a procedure taking {} arguments with {}",
                code.params.len(),
                args.len()
            )));
        }

        let mark = self.memory.mark();
        let mut frame = Frame {
            regs: vec![Value::Undefined; code.registers],
            slots: Vec::with_capacity(code.slots.len()),
        };
        for plan in &code.slots {
            match self.memory.allocate(plan.size, plan.align) {
                Ok(address) => frame.slots.push(address),
                Err(e) => {
                    self.memory.release(mark);
                    return Err(e);
                }
            }
        }
        for (reg, value) in code.params.iter().zip(args) {
            frame.regs[reg.index()] = value;
        }

        let result = self.run_instrs(code, &mut frame);
        self.memory.release(mark);
        result
    }

    fn run_instrs(&mut self, code: &'a Code, frame: &mut Frame) -> Result<Value, VmError> {
        let mut pc = code.entry;
        loop {
            // **The step budget** (ADR-0121). Checked here because this is the one place every instruction
            // passes through, and counted per *VM* rather than per frame so a loop that calls a procedure a
            // billion times is bounded too. Unmetered under `Mode::Runtime`, where the counter starts at
            // `u64::MAX`.
            self.fuel = self
                .fuel
                .checked_sub(1)
                .ok_or(VmError::Exhausted("steps"))?;
            let instr = code.instr(pc).ok_or_else(|| {
                VmError::internal(format!("ran off the end of the bytecode at {pc}"))
            })?;
            // Recorded *before* the instruction runs, so that whatever it raises is
            // attributed to it. Execution stops at a trap, so the last instruction
            // recorded is the one that trapped — and a nested call overwrites this
            // with its own deeper instruction, which is what a trap inside a callee
            // should report (ADR-0020 §4).
            self.at = code.spans.get(pc).copied().map(|span| TrapSite {
                proc: code.proc,
                span,
            });
            pc += 1;
            match instr {
                Instr::Move { dest, src } => {
                    let value = self.operand(frame, *src)?;
                    frame.regs[dest.index()] = value;
                }
                Instr::Binary { dest, op, lhs, rhs } => {
                    let value = self.binary(code, frame, *dest, *op, *lhs, *rhs)?;
                    frame.regs[dest.index()] = value;
                }
                Instr::Unary { dest, op, operand } => {
                    let value = self.unary(code, frame, *dest, *op, *operand)?;
                    frame.regs[dest.index()] = value;
                }
                Instr::Convert {
                    dest,
                    operand,
                    from,
                } => {
                    let value = self.convert(code, frame, *dest, *operand, *from)?;
                    frame.regs[dest.index()] = value;
                }
                Instr::Call { dest, callee, args } => {
                    let target = self.resolve_callee(frame, callee)?;
                    let mut values = Vec::with_capacity(args.len());
                    for arg in args {
                        values.push(self.operand(frame, *arg)?);
                    }
                    let result = self.call(target, values)?;
                    if let Some(dest) = dest {
                        frame.regs[dest.index()] = result;
                    }
                }
                Instr::Load { dest, place } => {
                    let value = self.load(frame, place)?;
                    frame.regs[dest.index()] = value;
                }
                Instr::Address { dest, place } => {
                    let address = self.address(frame, place)?;
                    frame.regs[dest.index()] = Value::Scalar(address);
                }
                Instr::Zero { place, size } => {
                    let address = self.address(frame, place)?;
                    let zeros = vec![0u8; usize::try_from(*size).unwrap_or(0)];
                    self.memory.write(address, &zeros)?;
                }
                // ADR-0003's check, run rather than compiled. The comparison is
                // **unsigned**: a negative index is an enormous unsigned value and so
                // fails the same test, which is the one comparison ADR-0039 §1 relies on
                // covering both ends of the range.
                Instr::BoundsCheck { index, len } => {
                    let index = self.operand(frame, *index)?.scalar()?;
                    let len = self.operand(frame, *len)?.scalar()?;
                    if index >= len {
                        return Err(VmError::Trap(Trap::IndexOutOfBounds));
                    }
                }
                // The tag is one byte at the variant's own offset (ADR-0068 §3), so this reads a byte
                // and compares it with the case the source named. A mismatch is the trap that makes a
                // variant safer than a union — the whole point of the form.
                Instr::TagCheck { place, case } => {
                    let address = self.address(frame, place)?;
                    let bytes = self.memory.read(address, u64::from(jr_pool::TAG_SIZE))?;
                    let tag = u64::from(*bytes.first().unwrap_or(&0));
                    if tag != u64::from(*case) {
                        return Err(VmError::Trap(Trap::WrongVariantCase));
                    }
                }
                Instr::Store { place, value } => {
                    let value = self.operand(frame, *value)?;
                    let address = self.address(frame, place)?;
                    self.store(address, place, &value)?;
                }
                // **An atomic, implemented non-atomically, which is correct here** (ADR-0176 §4). Nothing
                // in this interpreter can spawn a thread — a `#foreign` call needs a machine address for
                // the thread body and there is none (ADR-0175 §4) — so there is no concurrency to be
                // atomic against, and the plain read-modify-write *is* the sequentially consistent
                // answer.
                //
                // Implemented rather than refused so a `#run` may use one and the corpus differential can
                // cover atomics at all: a single-threaded program using `atomic_add` has one right answer
                // and all three engines must give it.
                // **An atomic, implemented non-atomically, which is correct here** (ADR-0176 §4). Nothing
                // in this interpreter can spawn a thread — a `#foreign` call needs a machine address for
                // the thread body and there is none (ADR-0175 §4) — so there is no concurrency to be
                // atomic against, and the plain read-modify-write *is* the sequentially consistent answer.
                //
                // Implemented rather than refused so a `#run` may use one and the corpus differential can
                // cover atomics at all: a single-threaded program using `atomic_add` has one right answer
                // and all three engines must give it.
                Instr::Atomic {
                    dest,
                    op,
                    address,
                    value,
                    expected,
                } => {
                    let target = self.operand(frame, *address)?.scalar()?;
                    // Always eight bytes: this wave's atomics are `s64` only, stated in `AtomicOp`'s own
                    // docs rather than inferred from a type here.
                    const WIDTH: u64 = 8;
                    // A missing operand is a lowering bug the verifier already refuses, so this is an
                    // internal error rather than a program one.
                    let missing =
                        || VmError::internal("an atomic is missing an operand".to_owned());
                    let result = match op {
                        jr_mir::AtomicOp::Load => {
                            Some(Value::Scalar(self.memory.read_scalar(target, WIDTH)?))
                        }
                        jr_mir::AtomicOp::Store => {
                            let operand = value.ok_or_else(missing)?;
                            let bits = self.operand(frame, operand)?.scalar()?;
                            self.memory.write_scalar(target, WIDTH, bits)?;
                            None
                        }
                        jr_mir::AtomicOp::Add => {
                            let operand = value.ok_or_else(missing)?;
                            let addend = self.operand(frame, operand)?.scalar()?;
                            let before = self.memory.read_scalar(target, WIDTH)?;
                            // **Wrapping**, matching the hardware: an atomic add is one machine
                            // instruction with no overflow check, so trapping here would make the
                            // interpreter disagree with both back ends about a program that wraps.
                            let after = before.wrapping_add(addend);
                            self.memory.write_scalar(target, WIDTH, after)?;
                            Some(Value::Scalar(before))
                        }
                        jr_mir::AtomicOp::CompareExchange => {
                            let wanted_operand = expected.ok_or_else(missing)?;
                            let new_operand = value.ok_or_else(missing)?;
                            let wanted = self.operand(frame, wanted_operand)?.scalar()?;
                            let new = self.operand(frame, new_operand)?.scalar()?;
                            let present = self.memory.read_scalar(target, WIDTH)?;
                            let matched = present == wanted;
                            if matched {
                                self.memory.write_scalar(target, WIDTH, new)?;
                            }
                            // A boolean is a `Scalar` of 0 or 1 in this interpreter — see `Value::boolean`.
                            Some(Value::Scalar(u64::from(matched)))
                        }
                    };
                    // **A store writes `Value::Void`, not nothing.** MIR gives every rvalue a destination —
                    // `void` is a storable value here (ADR-0015 §3) — so leaving the register alone left it
                    // `Value::Undefined`, and the next read of it trapped with "read a value that was never
                    // assigned" on a program whose store had in fact succeeded.
                    if let Some(dest) = dest {
                        frame.regs[dest.index()] = result.unwrap_or(Value::Void);
                    }
                }
                Instr::Undef { dest } => frame.regs[dest.index()] = Value::Undefined,
                Instr::Jump { target } => pc = *target,
                Instr::Branch { cond, then_, else_ } => {
                    let taken = self.operand(frame, *cond)?.boolean()?;
                    pc = if taken { *then_ } else { *else_ };
                }
                Instr::Return(value) => {
                    return match value {
                        Some(operand) => {
                            let value = self.operand(frame, *operand)?;
                            // Returning an undefined value *is* a use of it: the
                            // callee's contract is to produce a value of its return
                            // type, and `Undefined` is not one. Letting it through
                            // would push the trap into the caller, or out of the VM
                            // entirely as a folded constant with no bits.
                            if value == Value::Undefined {
                                return Err(VmError::Trap(Trap::UninitialisedRead));
                            }
                            Ok(value)
                        }
                        None => Ok(Value::Void),
                    };
                }
                Instr::Trap(reason) => {
                    return Err(VmError::Trap(match reason {
                        Unreachable::Trap => Trap::Deliberate,
                        Unreachable::StrayJump => Trap::StrayJump,
                        Unreachable::FellOffEnd => Trap::FellOffEnd,
                        // Unreachable in practice: the stub is built by the *native* driver
                        // (`jr_db::build_object`), and this interpreter refuses to run a file
                        // whose body was refused rather than running a stub. Mapped anyway,
                        // because the alternative is a `_` arm that would silently mean
                        // `Deliberate` if that ever changed.
                        Unreachable::Refused => Trap::Refused,
                    }));
                }
            }
        }
    }

    // -------------------------------------------------------------------
    // Operands
    // -------------------------------------------------------------------

    fn operand(&mut self, frame: &Frame, operand: Operand) -> Result<Value, VmError> {
        match operand {
            Operand::Value(value) => Ok(frame.regs[value.index()].clone()),
            Operand::Constant(id) => self.constant(id),
        }
    }

    /// One elementwise vector operation, lane by lane (ADR-0148 §4).
    ///
    /// The VM's `Value` is one scalar, so a vector lives in memory and this reads each lane out,
    /// applies the *scalar* operation, and writes the result back — which is what makes the answer
    /// bit-identical to the native engines' single instruction by construction rather than by
    /// coincidence: the arithmetic is `jr_pool::int_binary` and `float_binary`, the very functions
    /// the scalar path uses, so wrap-around and rounding cannot differ.
    ///
    /// # Errors
    /// [`VmError::unsupported`] for an operator sema should have refused (E0285) — integer division,
    /// or a trapping integer add. Reaching one means sema and the VM disagree about what was
    /// checked, which is worth an error rather than an answer.
    fn vector_binary(
        &mut self,
        op: BinOp,
        elem: PoolId,
        lanes: u64,
        left: &Value,
        right: &Value,
    ) -> Result<Value, VmError> {
        let layout = jr_pool::layout_of(self.pool, self.program.target, elem)
            .map_err(|e| VmError::internal(format!("a vector lane: {e}")))?;
        let stride = usize::try_from(layout.size.next_multiple_of(layout.align.into()))
            .map_err(|_| VmError::internal("a vector lane wider than a usize"))?;
        let a = left.aggregate()?;
        let b = right.aggregate()?;
        let lanes = usize::try_from(lanes)
            .map_err(|_| VmError::internal("more vector lanes than a usize"))?;
        let mut out = vec![0u8; stride * lanes];

        let float = jr_pool::FloatKind::of(self.pool, elem);
        let int = IntKind::of(self.pool, elem);

        for lane in 0..lanes {
            let at = lane * stride;
            // A short operand is an internal error, not a zero: reading past the end would give a
            // plausible wrong lane rather than a failure.
            let (Some(a_bytes), Some(b_bytes), Some(o_bytes)) = (
                a.get(at..at + stride),
                b.get(at..at + stride),
                out.get_mut(at..at + stride),
            ) else {
                return Err(VmError::internal("a vector operand shorter than its type"));
            };
            let a_bits = le_bits(a_bytes);
            let b_bits = le_bits(b_bytes);

            let result = if let Some(float) = float {
                let Some(arith) = op.as_float_op() else {
                    return Err(VmError::unsupported(format!(
                        "{op:?} is not defined on a float vector"
                    )));
                };
                jr_pool::float_binary(arith, float, float.decode(a_bits), float.decode(b_bits))
            } else {
                let Some(kind) = int else {
                    return Err(VmError::internal("a vector of a non-numeric element"));
                };
                let Some(arith) = op.as_int_op() else {
                    return Err(VmError::unsupported(format!(
                        "{op:?} is not defined on an integer vector"
                    )));
                };
                // `int_binary` — the *same* function the scalar path uses, which is what makes the
                // wrap-around bit-identical rather than merely intended. Sema accepts only the
                // wrapping operators on an integer vector (§6), and those cannot trap, so an
                // `IntTrap` here means sema let a trapping form through: an error, not an answer.
                jr_pool::int_binary(arith, kind, kind.decode(a_bits), kind.decode(b_bits)).map_err(
                    |_| {
                        VmError::unsupported(format!(
                            "{op:?} overflowed a vector lane, which no vector operation can report"
                        ))
                    },
                )?
            };
            o_bytes.copy_from_slice(&result.to_le_bytes()[..stride]);
        }

        Ok(Value::Aggregate(out))
    }

    /// Turns an interned constant into a runtime value.
    fn constant(&mut self, id: PoolId) -> Result<Value, VmError> {
        match *self.pool.item(id) {
            Item::VoidValue => Ok(Value::Void),
            Item::BoolValue(value) => Ok(Value::bool(value)),
            Item::IntValue { ty, bits } => {
                let kind = IntKind::of(self.pool, ty).unwrap_or(IntKind::S64);
                Ok(Value::Scalar(bits & kind.mask()))
            }
            // Already normalised to the type's width by `FloatKind::encode`, so the bits are
            // passed through — a `float32`'s live in the low 32 and the interpretation comes
            // from the type at every use.
            Item::FloatValue { ty: _, bits } => Ok(Value::Scalar(bits)),
            Item::StrValue(str_id) => self.string_value(str_id),
            Item::StaticArray { .. } => self.static_array_value(id),
            // An aggregate constant is turned into bytes **here**, per target (ADR-0074 §1): the pool
            // interned the element *values*, deliberately not a byte image, because the pool is
            // target-independent and an image is not. This is the one place the VM turns the one into the
            // other, and `jr-codegen-clif` has its own — two materialisations from one shared value, which
            // is ADR-0019's arrangement and what the differential harness checks.
            Item::AggregateValue { ty, .. } => self.aggregate_value(id, ty),
            // A **procedure value** encodes its `ProcRef` as a scalar (ADR-0059 §4): the VM's
            // proc pointer is not a code address but a handle it decodes at the indirect call.
            // The bits differ from the native back end's real address, and that is allowed —
            // nothing observes a proc pointer's bits, only calling through it, which the
            // differential harness compares. The pack is `(file << 32) | proc`, matching
            // `resolve_callee`'s unpack exactly; a mismatch there would be a wrong call, not a
            // wrong number, so the two live next to each other in intent. The `+ 1` is what keeps
            // zero free for `null` (ADR-0110 §1).
            Item::ProcValue { ty: _, decl } => {
                let file = decl.file.index() as u64;
                let proc = u64::from(decl.index);
                // **Biased by one so that no real procedure encodes as zero** (ADR-0110 §1). Without the bias,
                // file 0 procedure 0 — an ordinary procedure, and the *first* one in the file — packed to the
                // same handle as `null`, so a null check could not tell them apart. The native back end has no
                // such collision (a code address is never zero), so this is the VM's encoding earning the same
                // property rather than a language change.
                Ok(Value::Scalar(((file << 32) | proc) + 1))
            }
            // A type or a library used as a *value* is comptime-only (wave W4) and has
            // no runtime representation; `jr_pool::LayoutError::ComptimeOnly` says
            // the same thing from the layout side.
            Item::TypeValue(_) | Item::ForeignLibraryValue(_, _) => Err(VmError::unsupported(
                "a type or library used as a runtime value",
            )),
            // Exhaustive by *type* variant rather than a catch-all, and that change is this
            // wave's doing: a `ref other` arm here swallowed `Item::FloatValue` and reported
            // "expected a value constant, found the type FloatValue" at run time, while the
            // native back end computed the right answer. A catch-all cannot be a compile
            // error when a variant is added, which is exactly what `AGENTS.md` bans a `_` arm
            // for.
            Item::ContextType
            | Item::ResultsType { .. }
            | Item::VoidType
            | Item::BoolType
            | Item::IntType { .. }
            | Item::FloatType { .. }
            | Item::EnumType { .. }
            | Item::StringType
            | Item::TypeType
            | Item::ErrorType
            | Item::ForeignLibraryType
            | Item::PointerType(_)
            | Item::ArrayType { .. }
            | Item::VectorType { .. }
            | Item::ViewType { .. }
            | Item::DynamicArrayType { .. }
            | Item::StructType { .. }
            | Item::UnionType { .. }
            | Item::VariantType { .. }
            | Item::ProcType { .. } => Err(VmError::internal(
                "a type was used where a value constant belongs",
            )),
        }
    }

    /// The `{data, count}` pair for a string literal (ADR-0004).
    fn string_value(&mut self, str_id: StrId) -> Result<Value, VmError> {
        let address = *self
            .strings
            .get(&str_id)
            .ok_or_else(|| VmError::internal("a string constant was not interned"))?;
        let count = self.pool.resolve_str(str_id).len() as u64;
        let target = self.program.target;

        let layout = string_layout(target);
        let (data_offset, data) = string_data(target);
        let (count_offset, count_layout) = string_count(target);

        let mut bytes = vec![0u8; usize::try_from(layout.size).unwrap_or(16)];
        write_le(&mut bytes, data_offset, data.size, address);
        write_le(&mut bytes, count_offset, count_layout.size, count);
        Ok(Value::Aggregate(bytes))
    }

    /// The byte image of an aggregate constant, in this program's target layout (ADR-0074 §1).
    ///
    /// The pool interned the element *values*; this writes each into the image at its own offset, which is
    /// the conversion the pool deliberately does not do — offsets are a *target* answer
    /// (`field_offset(pool, target, …)`) and the pool holds no target.
    ///
    /// A **struct** asks the pool for each field's offset; an **array** uses the element layout's size as
    /// the stride, which is the same rule `layout_of` used to compute the array's size, so the two cannot
    /// disagree about where element *n* begins. A nested element recurses through [`Self::constant`], so an
    /// array of structs needs no special case.
    fn aggregate_value(&mut self, id: PoolId, ty: PoolId) -> Result<Value, VmError> {
        let target = self.program.target;
        let layout = jr_pool::layout_of(self.pool, target, ty)
            .map_err(|e| VmError::internal(format!("an aggregate constant has no layout: {e}")))?;
        let Item::AggregateValue { elements, .. } = self.pool.item(id) else {
            return Err(VmError::internal("an aggregate constant changed shape"));
        };
        let elements = elements.clone();

        // The element offsets, resolved before any element is materialised: `constant` below takes
        // `&mut self`, and the pool borrow must end first.
        let mut placements: Vec<(u64, u64)> = Vec::with_capacity(elements.len());
        if let Item::ArrayType { elem, .. } = *self.pool.item(ty) {
            let elem_layout = jr_pool::layout_of(self.pool, target, elem).map_err(|e| {
                VmError::internal(format!("an array constant's element has no layout: {e}"))
            })?;
            for index in 0..elements.len() {
                placements.push((elem_layout.size * index as u64, elem_layout.size));
            }
        } else {
            for index in 0..elements.len() {
                let (offset, field) = jr_pool::field_offset(self.pool, target, ty, index as u32)
                    .map_err(|e| {
                        VmError::internal(format!(
                            "an aggregate constant's field has no offset: {e}"
                        ))
                    })?;
                placements.push((offset, field.size));
            }
        }

        let mut bytes = vec![0u8; usize::try_from(layout.size).unwrap_or(0)];
        for (element, (offset, size)) in elements.into_iter().zip(placements) {
            match self.constant(element)? {
                // A scalar element is its bits, written little-endian at its offset — the same
                // `write_le` a string's `{data, count}` uses, so there is one byte-order answer.
                Value::Scalar(bits) => write_le(&mut bytes, offset, size, bits),
                // A **nested** aggregate is already an image of its own; it is copied in whole.
                Value::Aggregate(inner) => {
                    let start = usize::try_from(offset).unwrap_or(0);
                    let end = start.saturating_add(inner.len()).min(bytes.len());
                    if start < end {
                        bytes[start..end].copy_from_slice(&inner[..end - start]);
                    }
                }
                // `void` occupies no bytes, so an element of it writes nothing rather than being an
                // error — the same rule `Layout::ZERO` states from the layout side.
                Value::Void => {}
                Value::Undefined => {
                    return Err(VmError::internal(
                        "an aggregate constant holds an uninitialised element",
                    ));
                }
            }
        }
        Ok(Value::Aggregate(bytes))
    }

    /// The type an operand holds, from the register table the bytecode carries.
    fn operand_type(&self, code: &Code, operand: Operand) -> PoolId {
        match operand {
            Operand::Value(value) => code
                .types
                .get(value.index())
                .copied()
                .unwrap_or(PoolId::ERROR),
            Operand::Constant(id) => self.pool.type_of(id),
        }
    }

    // -------------------------------------------------------------------
    // Arithmetic — ADR-0002
    // -------------------------------------------------------------------

    fn binary(
        &mut self,
        code: &Code,
        frame: &Frame,
        dest: crate::code::Reg,
        op: BinOp,
        lhs: Operand,
        rhs: Operand,
    ) -> Result<Value, VmError> {
        let left = self.operand(frame, lhs)?;
        let right = self.operand(frame, rhs)?;
        let operand_ty = self.operand_type(code, lhs);
        let dest_ty = code
            .types
            .get(dest.index())
            .copied()
            .unwrap_or(PoolId::ERROR);

        // **A vector before everything else** (ADR-0148 §4). Its operands are `Value::Aggregate`s —
        // the VM has no vector register and will not grow one — so an elementwise operation is a
        // *loop*, and the two native engines emit a single instruction for the same MIR. That the
        // three then agree byte for byte is the strongest claim the differential harness makes.
        //
        // Placed first because every test below asks about the *operand type* as a scalar, and a
        // vector answers none of them: `FloatKind::of` says `None` for a vector of floats, so a
        // float vector would fall through to the integer path and then to a bit compare — the
        // plausible-wrong-answer failure mode the float ordering above exists to prevent, one type
        // wider.
        if let Item::VectorType { elem, lanes } = *self.pool.item(operand_ty) {
            return self.vector_binary(op, elem, lanes, &left, &right);
        }

        // **Floats first, before the bit-compare fallback below.** That ordering is the
        // whole hazard ADR-0040's Consequences names: the fallback answers `==` with a raw
        // bit compare, which gets `-0.0 == 0.0` wrong (true in IEEE-754, different bits) and
        // `NaN == NaN` wrong (false in IEEE-754, identical bits). Both are *plausible wrong
        // answers* rather than errors, which is this project's named failure mode, so a float
        // must never reach it.
        if let Some(float) = jr_pool::FloatKind::of(self.pool, operand_ty) {
            let a = float.decode(left.scalar()?);
            let b = float.decode(right.scalar()?);
            if let Some(cmp) = op.as_float_cmp() {
                return Ok(Value::bool(jr_pool::float_compare(cmp, a, b)));
            }
            let Some(arith) = op.as_float_op() else {
                // `%` and the wrapping operators, which sema already refused (ADR-0040 §7).
                // Reaching here means sema and the VM disagree about what was checked.
                return Err(VmError::unsupported(format!(
                    "{op:?} is not defined on a floating-point operand"
                )));
            };
            let out = jr_pool::FloatKind::of(self.pool, dest_ty).unwrap_or(float);
            return Ok(Value::Scalar(jr_pool::float_binary(arith, out, a, b)));
        }

        // Equality on a non-integer scalar — a `bool` or a pointer — is a raw bit
        // compare. Ordering is not: `<` on pointers is not in the Jairs-0 subset, and
        // silently defining it would make the first attempt succeed by accident.
        let Some(kind) = IntKind::of(self.pool, operand_ty) else {
            return match op {
                BinOp::Eq => Ok(Value::bool(left.scalar()? == right.scalar()?)),
                BinOp::Ne => Ok(Value::bool(left.scalar()? != right.scalar()?)),
                _ => Err(VmError::unsupported(format!(
                    "{op:?} on a non-integer operand"
                ))),
            };
        };

        let a = left.as_int(kind)?;
        let b = right.as_int(kind)?;

        // Comparisons first: their result is a `bool`, not the operand type, so they
        // must not go through the destination's integer kind. `BinOp::as_int_cmp` and
        // `as_int_op` are what keep that split honest — a MIR operator is one or the
        // other, never both (ADR-0022 §2).
        if let Some(cmp) = op.as_int_cmp() {
            return Ok(Value::bool(jr_pool::int_compare(cmp, a, b)));
        }
        let Some(arith) = op.as_int_op() else {
            return Err(VmError::internal(
                "an operator was neither arithmetic nor a comparison",
            ));
        };

        let out = IntKind::of(self.pool, dest_ty).unwrap_or(kind);
        Ok(Value::Scalar(
            jr_pool::int_binary(arith, out, a, b).map_err(trap_of)?,
        ))
    }

    /// Converts an integer to the destination register's width (ADR-0037 §2).
    ///
    /// # Why this wraps where arithmetic traps
    ///
    /// ADR-0002 makes overflow trap because an overflowing `+` produces a result the program
    /// did not ask for. A narrowing `cast` is the opposite: the program asked for the low
    /// bits. So this calls `IntKind::wrap`, and `jr-mir`'s `fold_convert` calls the *same*
    /// function — which is what keeps comptime folding and runtime execution agreeing about
    /// the same program, the invariant `differential.rs` exists to check.
    ///
    /// `from` decides sign extension when widening: the incoming bits are meaningless without
    /// knowing whether their top bit was a sign.
    fn convert(
        &mut self,
        code: &Code,
        frame: &Frame,
        dest: crate::code::Reg,
        operand: Operand,
        from: NumKind,
    ) -> Result<Value, VmError> {
        let value = self.operand(frame, operand)?;
        let ty = code
            .types
            .get(dest.index())
            .copied()
            .unwrap_or(PoolId::ERROR);
        let to = NumKind::of(self.pool, ty)
            .ok_or_else(|| VmError::unsupported("a cast to a non-numeric type"))?;

        // Four directions (ADR-0040 §3), each delegating to `jr-pool` so that the folder and
        // the interpreter cannot disagree — the rule ADR-0022 §2 states and which matters
        // more for floats, not less, because a folded float constant is baked into a `PoolId`
        // both engines then read.
        Ok(Value::Scalar(match (from, to) {
            // Decoded through `from` so that a negative `s8` widens to a negative `s64`
            // rather than to 255-ish, then re-wrapped into the destination.
            (NumKind::Int(from), NumKind::Int(to)) => to.wrap(value.as_int(from)?),
            (NumKind::Int(from), NumKind::Float(to)) => {
                jr_pool::int_to_float(to, value.as_int(from)?)
            }
            (NumKind::Float(from), NumKind::Int(to)) => {
                jr_pool::float_to_int(to, from.decode(value.scalar()?))
            }
            (NumKind::Float(from), NumKind::Float(to)) => to.encode(from.decode(value.scalar()?)),
        }))
    }

    fn unary(
        &mut self,
        code: &Code,
        frame: &Frame,
        dest: crate::code::Reg,
        op: UnOp,
        operand: Operand,
    ) -> Result<Value, VmError> {
        let value = self.operand(frame, operand)?;
        match op {
            UnOp::Not => Ok(Value::bool(!value.boolean()?)),
            // Integers only (ADR-0042 §5), and normalised to the type's width by `int_not` —
            // the same function the folder calls, so a folded `~0` and a run-time one agree.
            UnOp::BitNot => {
                let ty = code
                    .types
                    .get(dest.index())
                    .copied()
                    .unwrap_or(PoolId::ERROR);
                let kind = IntKind::of(self.pool, ty)
                    .ok_or_else(|| VmError::unsupported("`~` on a non-integer"))?;
                Ok(Value::Scalar(jr_pool::int_not(kind, value.as_int(kind)?)))
            }
            UnOp::Neg => {
                let ty = code
                    .types
                    .get(dest.index())
                    .copied()
                    .unwrap_or(PoolId::ERROR);
                // Floats first, and this one is total: negating a float flips its sign bit,
                // so `-0.0` is a real value and there is nothing to trap on (ADR-0040 §1).
                if let Some(float) = jr_pool::FloatKind::of(self.pool, ty) {
                    let decoded = float.decode(value.scalar()?);
                    return Ok(Value::Scalar(jr_pool::float_negate(float, decoded)));
                }
                let kind = IntKind::of(self.pool, ty)
                    .ok_or_else(|| VmError::unsupported("negation of a non-number"))?;
                // Traps on the most negative value (ADR-0002): its negation is one
                // past the maximum, so the ordinary range check covers it.
                Ok(Value::Scalar(
                    jr_pool::int_negate(kind, value.as_int(kind)?).map_err(trap_of)?,
                ))
            }
        }
    }

    // -------------------------------------------------------------------
    // Memory
    // -------------------------------------------------------------------

    fn address(&mut self, frame: &Frame, plan: &PlacePlan) -> Result<Address, VmError> {
        let mut address = match &plan.base {
            PlaceRoot::Slot(index) => *frame
                .slots
                .get(*index)
                .ok_or_else(|| VmError::internal(format!("no slot s{index} in this frame")))?,
            PlaceRoot::Address(operand) => self.operand(frame, *operand)?.scalar()?,
            // Looked up in the program-wide table [`Self::emit_globals`] filled before execution
            // started, not in `frame.slots`: a global has program lifetime, so its address does not
            // come from *this* call's frame at all (ADR-0186 §1, ADR-0186 §3).
            PlaceRoot::Global(global) => *self
                .globals
                .get(global)
                .ok_or_else(|| VmError::internal("a global was referenced but never laid out"))?,
        };
        for step in &plan.steps {
            address = match step {
                PlaceStep::Offset(offset) => address.wrapping_add(*offset),
                PlaceStep::Indirect { size } => self.memory.read_scalar(address, *size)?,
                // Wrapping, like `Offset` above. An index that would wrap has already
                // failed its bounds check — the check is a separate statement that runs
                // first — so this cannot reach a wrong address in a checked build, and in
                // an unchecked one the wrap is the same arithmetic native code does.
                PlaceStep::ScaledIndex { index, stride } => {
                    let index = self.operand(frame, *index)?.scalar()?;
                    address.wrapping_add(index.wrapping_mul(*stride))
                }
            };
        }
        Ok(address)
    }

    fn load(&mut self, frame: &Frame, plan: &PlacePlan) -> Result<Value, VmError> {
        let address = self.address(frame, plan)?;
        match plan.shape {
            Shape::Void => Ok(Value::Void),
            Shape::Scalar => Ok(Value::Scalar(self.memory.read_scalar(address, plan.size)?)),
            Shape::Aggregate => Ok(Value::Aggregate(
                self.memory.read(address, plan.size)?.to_vec(),
            )),
        }
    }

    fn store(&mut self, address: Address, plan: &PlacePlan, value: &Value) -> Result<(), VmError> {
        self.write_value(address, plan.shape, plan.size, value)
    }

    /// Writes `value` at `address` as `size` bytes of `shape`.
    ///
    /// The shared tail of [`Self::store`] and [`Self::emit_globals`]: a global's initial value and
    /// an ordinary store through a place are the same operation on the same [`Memory`], and giving
    /// each its own copy is exactly the kind of duplication that drifts the day one of them gains a
    /// case the other does not (ADR-0018 §2's argument, one level up from layout).
    fn write_value(
        &mut self,
        address: Address,
        shape: Shape,
        size: u64,
        value: &Value,
    ) -> Result<(), VmError> {
        match shape {
            Shape::Void => Ok(()),
            Shape::Scalar => self.memory.write_scalar(address, size, value.scalar()?),
            Shape::Aggregate => {
                let bytes = value.aggregate()?;
                if bytes.len() as u64 != size {
                    return Err(VmError::internal(format!(
                        "storing {} bytes into a {size}-byte place",
                        bytes.len()
                    )));
                }
                let bytes = bytes.to_vec();
                self.memory.write(address, &bytes)
            }
        }
    }

    // -------------------------------------------------------------------
    // Calls
    // -------------------------------------------------------------------

    fn resolve_callee(&mut self, frame: &Frame, callee: &Callee) -> Result<ProcRef, VmError> {
        match callee {
            Callee::Direct(target) => Ok(*target),
            // A procedure pointer is a scalar handle encoding its `ProcRef` (ADR-0059 §4):
            // `((file << 32) | proc) + 1`, the exact inverse of `constant`'s biased pack for an
            // `Item::ProcValue`. The two must agree bit-for-bit, so they are written to be read
            // together — a mismatch is a call to the wrong procedure, not a diagnosable failure.
            Callee::Indirect(operand) => {
                let handle = self.operand(frame, *operand)?.scalar()?;
                // **A null handle is a trap, not a call to file 0 proc 0** (ADR-0110 §1). Zero decodes to an
                // arbitrary *real* procedure, so calling a null pointer used to call something else — and the
                // symptom was whatever that procedure's arity happened to be, surfacing as "called a procedure
                // taking 1 arguments with 2" for the ordinary mistake of using `context.allocator` before
                // installing one. There is no procedure to call and no answer to invent.
                if handle == 0 {
                    return Err(VmError::Trap(crate::error::Trap::NullCall));
                }
                // The inverse of `constant`'s biased pack: subtract the bias before unpacking. The two must
                // agree bit-for-bit, so they are written to be read together — a mismatch is a call to the wrong
                // procedure rather than a diagnosable failure.
                let handle = handle - 1;
                let file = FileId::from_usize((handle >> 32) as usize);
                let proc = ProcId::from_u32((handle & 0xFFFF_FFFF) as u32);
                Ok(ProcRef::new(file, proc))
            }
        }
    }

    fn foreign(&mut self, foreign: &ForeignProc, args: Vec<Value>) -> Result<Value, VmError> {
        // ADR-0006: compile-time code *may* call foreign functions, behind an
        // explicit `#foreign_at_comptime` allowance that wave W6 introduces. The
        // bridge exists; the allowance does not. Refusing here is what keeps a
        // decision that has not been taken from being granted by accident.
        if self.mode == Mode::Comptime {
            return Err(VmError::unsupported(format!(
                "`{}` is a foreign procedure, and compile-time code may not call one until `#foreign_at_comptime` arrives (ADR-0006)",
                foreign.symbol
            )));
        }
        crate::ffi::call(self, foreign, &args)
    }

    // -------------------------------------------------------------------
    // Accessors the FFI bridge needs
    // -------------------------------------------------------------------

    pub(crate) const fn pool(&self) -> &Pool {
        self.pool
    }

    pub(crate) fn capture(&mut self, bytes: &[u8]) {
        self.captured.extend_from_slice(bytes);
    }
}

/// Reads a little-endian value of `size` bytes from `bytes` at `offset`.
fn read_le(bytes: &[u8], offset: u64, size: u64) -> u64 {
    let start = usize::try_from(offset).unwrap_or(0);
    let size = usize::try_from(size).unwrap_or(8).min(8);
    let mut buf = [0u8; 8];
    if let Some(source) = bytes.get(start..start + size) {
        buf[..size].copy_from_slice(source);
    }
    u64::from_le_bytes(buf)
}

/// Writes `value` little-endian into `bytes` at `offset`, `size` bytes wide.
fn write_le(bytes: &mut [u8], offset: u64, size: u64, value: u64) {
    let start = usize::try_from(offset).unwrap_or(0);
    let size = usize::try_from(size).unwrap_or(8).min(8);
    let encoded = value.to_le_bytes();
    if let Some(target) = bytes.get_mut(start..start + size) {
        target.copy_from_slice(&encoded[..size]);
    }
}

/// Turns `jr-pool`'s arithmetic failure into this crate's trap.
///
/// The whole cost of ADR-0022 §2's extraction, and it is one function. `IntTrap`
/// carries the same `&'static str` `Trap::Overflow` does, so no trap message can
/// have changed — which matters because `differential.rs` compares the finished
/// sentence and the native back end builds its copy at compile time.
const fn trap_of(trap: jr_pool::IntTrap) -> VmError {
    match trap {
        jr_pool::IntTrap::Overflow { what } => VmError::Trap(Trap::Overflow { what }),
        jr_pool::IntTrap::ShiftOutOfRange => VmError::Trap(Trap::ShiftOutOfRange),
        jr_pool::IntTrap::DivideByZero => VmError::Trap(Trap::DivideByZero),
    }
}

/// The little-endian integer a lane's bytes hold, zero-extended to 64 bits.
///
/// Little-endian unconditionally, matching every other byte-level read in this crate: the VM's
/// memory image *is* the target layout (ADR-0015), and both supported targets are little-endian. A
/// slice longer than eight bytes takes its low eight, which no vector lane is — the widest is a
/// `s64` at exactly eight.
fn le_bits(bytes: &[u8]) -> u64 {
    let mut buffer = [0u8; 8];
    let take = bytes.len().min(8);
    buffer[..take].copy_from_slice(&bytes[..take]);
    u64::from_le_bytes(buffer)
}
