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

use jr_mir::{BinOp, Callee, NumKind, ProcRef, UnOp, Unreachable};
use jr_pool::{Item, Pool, PoolId, StrId, TargetLayout, string_count, string_data, string_layout};
use rustc_hash::FxHashMap;

use crate::code::{
    Code, ForeignProc, Instr, Operand, PlacePlan, PlaceRoot, PlaceStep, Routine, Shape,
};
use crate::error::{Trap, TrapSite, VmError, ice};
use crate::memory::Memory;
use crate::value::{Address, IntKind, Value};

/// How deep Jairs calls may nest before the VM gives up.
///
/// 256 frames. Deliberately far below what the host stack could take, because the
/// point is to fail diagnosably rather than to maximise depth; the slice's deepest
/// call chain is `main` → `print` → `write`.
pub const MAX_DEPTH: usize = 256;

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
    target: TargetLayout,
}

impl Program {
    /// An empty program for `target`.
    #[must_use]
    pub fn new(target: TargetLayout) -> Self {
        Self {
            routines: FxHashMap::default(),
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
    /// Bytes a foreign write produced, when the bridge is capturing rather than
    /// writing through. Empty under [`Mode::Runtime`].
    captured: Vec<u8>,
    /// The instruction currently executing, for [`Vm::trap_site`].
    at: Option<TrapSite>,
}

impl<'a> Vm<'a> {
    /// Creates a VM over `program`, interning every string constant into memory.
    ///
    /// # Errors
    /// [`VmError::Exhausted`] if the string constants alone do not fit.
    pub fn new(program: &'a Program, pool: &'a Pool, mode: Mode) -> Result<Self, VmError> {
        let mut vm = Self {
            program,
            pool,
            memory: Memory::new(),
            mode,
            depth: 0,
            strings: FxHashMap::default(),
            captured: Vec::new(),
            at: None,
        };
        vm.intern_strings()?;
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
        Ok(())
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

    /// The VM's memory, for inspecting a result that lives there.
    #[must_use]
    pub const fn memory(&self) -> &Memory {
        &self.memory
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
        let result = match routine {
            Routine::Bytecode(code) => self.execute(code, args),
            Routine::Foreign(foreign) => self.foreign(foreign, args),
        };
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
                Instr::Store { place, value } => {
                    let value = self.operand(frame, *value)?;
                    let address = self.address(frame, place)?;
                    self.store(address, place, &value)?;
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
            // A type or a procedure as a *value* is comptime-only (wave W4) and has
            // no runtime representation; `jr_pool::LayoutError::ComptimeOnly` says
            // the same thing from the layout side.
            Item::TypeValue(_) | Item::ProcValue { .. } | Item::ForeignLibraryValue(_) => Err(
                VmError::unsupported("a type, procedure or library used as a runtime value"),
            ),
            // Exhaustive by *type* variant rather than a catch-all, and that change is this
            // wave's doing: a `ref other` arm here swallowed `Item::FloatValue` and reported
            // "expected a value constant, found the type FloatValue" at run time, while the
            // native back end computed the right answer. A catch-all cannot be a compile
            // error when a variant is added, which is exactly what `AGENTS.md` bans a `_` arm
            // for.
            Item::VoidType
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
            | Item::ViewType { .. }
            | Item::StructType { .. }
            | Item::UnionType { .. }
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
        match plan.shape {
            Shape::Void => Ok(()),
            Shape::Scalar => self
                .memory
                .write_scalar(address, plan.size, value.scalar()?),
            Shape::Aggregate => {
                let bytes = value.aggregate()?;
                if bytes.len() as u64 != plan.size {
                    return Err(VmError::internal(format!(
                        "storing {} bytes into a {}-byte place",
                        bytes.len(),
                        plan.size
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
            // A procedure-pointer value would have to carry a `ProcRef`, and the pool
            // interns a procedure as an `Item::ProcValue { decl }` — a `DeclId`, not a
            // `ProcRef`. Bridging the two needs a decl-to-proc map nothing builds yet,
            // and nothing in Jairs-0 calls through a pointer.
            Callee::Indirect(operand) => {
                let _ = self.operand(frame, *operand)?;
                Err(VmError::unsupported(
                    "calling through a procedure pointer arrives with a later wave",
                ))
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
