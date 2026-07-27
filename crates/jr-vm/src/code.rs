//! The bytecode: a register machine addressed by `ValueId` (ADR-0018 §1).
//!
//! # Why registers, and why these registers
//!
//! MIR is already SSA with dense, single-assignment [`ValueId`]s that the verifier
//! has checked are defined before use. "Which register holds this operand" is
//! therefore a question `jr-mir` already answered, so the bytecode reuses the
//! answer: a register **is** a `ValueId`, a frame is a flat array sized from
//! `MirBody::value_count()`, and lowering is a transliteration rather than a
//! translation.
//!
//! ADR-0018 §1 records the rejected alternatives. A stack machine would have to
//! discard that answer and recover an evaluation order to replace it, plus `dup`
//! traffic wherever a value has more than one use — a pass whose only purpose is to
//! rediscover what MIR states. Interpreting `MirBody` directly is cheaper still and
//! was rejected on `PLAN.md` §3.1's invariant: two independent readings of the same
//! structure is the divergence "the same MIR" exists to prevent, relocated rather
//! than removed.
//!
//! # Why an operand is still `jr_mir::Operand`
//!
//! Instructions carry [`Operand`] — `Value(ValueId) | Constant(PoolId)` — unchanged
//! from MIR, rather than materialising every constant into a register first. That
//! keeps the transliteration honest: a bytecode dump reads like the MIR beside it,
//! which is the first tool anyone reaches for when the VM and Cranelift disagree
//! about a program.
//!
//! # Why blocks become one flat instruction stream
//!
//! Blocks are emitted in `MirBody::reverse_postorder()` — the order `mir.rs`
//! documents as "the order the bytecode lowering will linearise in" — and branch
//! targets are absolute instruction indices. There is no block dispatch at run
//! time, so the interpreter's inner loop is a program counter and nothing else.
//! Unreachable blocks are simply not emitted; a MIR dump shows them because
//! `jr-mir` has no DCE, but there is no reason to give them addresses.
//!
//! # Why places are pre-planned
//!
//! A [`PlacePlan`] resolves each [`jr_mir::Projection`] to a byte offset **once, at
//! lowering time**, using `jr-pool`'s layout. The interpreter then walks a list of
//! adds and pointer loads with no access to the pool's field tables at all. This is
//! where ADR-0018 §2's "one shared computation" actually pays: the offsets in a
//! `PlacePlan` are the same numbers Cranelift will emit, because they come from the
//! same function.

use jr_mir::{BinOp, BlockId, Callee, MirSpan, ProcRef, UnOp, Unreachable, ValueId};
use jr_pool::PoolId;

/// A register index. Identical to a MIR [`ValueId`], deliberately (ADR-0018 §1).
pub type Reg = ValueId;

/// An instruction operand, unchanged from MIR.
pub type Operand = jr_mir::Operand;

// ---------------------------------------------------------------------------
// Places
// ---------------------------------------------------------------------------

/// How to reach a memory location, with every offset already computed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacePlan {
    /// Where the address comes from.
    pub base: PlaceRoot,
    /// Applied in order to the base address.
    pub steps: Vec<PlaceStep>,
    /// The size in bytes of the value at the resulting address.
    pub size: u64,
    /// What kind of [`crate::Value`] reading that address produces.
    pub shape: Shape,
}

/// What reading a place produces.
///
/// Three cases rather than a `scalar: bool`, because `void` is a real type
/// (ADR-0015 §3) and is storable: a zero-byte read must yield [`crate::Value::Void`]
/// and not `Scalar(0)`, or a `void`-returning call's result would compare equal to
/// `false`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// Zero bytes: the single value of type `void`.
    Void,
    /// Fits one register: `bool`, an integer, a pointer, a procedure.
    Scalar,
    /// Lives in memory: `string`, a struct.
    Aggregate,
}

/// Where a [`PlacePlan`] starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaceRoot {
    /// A stack slot of the current frame, by index into [`Code::slots`].
    Slot(usize),
    /// The address held in an operand — the base of ADR-0011's postfix `.*`.
    Address(Operand),
}

/// One step of a [`PlacePlan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceStep {
    /// Add a constant byte offset — a struct field, or `string`'s `.data`/`.count`.
    ///
    /// The three MIR projections that were symbolic (`Field`, `StringData`,
    /// `StringCount`) all collapse to this, which is the point of ADR-0018 §2: once
    /// layout exists, they are the same operation with different numbers.
    Offset(u64),
    /// Read a pointer of `size` bytes at the current address and continue there.
    Indirect {
        /// The target's pointer width, from [`jr_pool::TargetLayout`].
        size: u64,
    },
}

// ---------------------------------------------------------------------------
// Instructions
// ---------------------------------------------------------------------------

/// One bytecode instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Instr {
    /// `dest <- src`. Also the edge copies that replace block parameters.
    Move {
        /// The destination register.
        dest: Reg,
        /// What to copy.
        src: Operand,
    },
    /// `dest <- lhs op rhs`. Never `&&` or `||`: MIR has no such operator, because
    /// short-circuiting is control flow.
    Binary {
        /// The destination register.
        dest: Reg,
        /// The operator.
        op: BinOp,
        /// Left operand.
        lhs: Operand,
        /// Right operand.
        rhs: Operand,
    },
    /// `dest <- op operand`.
    Unary {
        /// The destination register.
        dest: Reg,
        /// The operator.
        op: UnOp,
        /// The operand.
        operand: Operand,
    },
    /// Calls a procedure. `dest` is `None` for a call in statement position.
    Call {
        /// Where the result goes, if anyone wants it.
        dest: Option<Reg>,
        /// What to call.
        callee: Callee,
        /// The arguments, in declaration order.
        args: Vec<Operand>,
    },
    /// `dest <- *place`.
    Load {
        /// The destination register.
        dest: Reg,
        /// Where to read from.
        place: PlacePlan,
    },
    /// `dest <- &place`.
    Address {
        /// The destination register.
        dest: Reg,
        /// Whose address to take.
        place: PlacePlan,
    },
    /// `*place <- value`.
    Store {
        /// Where to write.
        place: PlacePlan,
        /// What to write.
        value: Operand,
    },
    /// `dest <- undefined`. Reading the result traps; see [`crate::Value::Undefined`].
    Undef {
        /// The destination register.
        dest: Reg,
    },
    /// Jump to an absolute instruction index.
    Jump {
        /// Where to go.
        target: usize,
    },
    /// Branch on a `bool`.
    Branch {
        /// The condition.
        cond: Operand,
        /// Taken when true.
        then_: usize,
        /// Taken when false.
        else_: usize,
    },
    /// Return from the procedure. `None` for a `void` return.
    Return(Option<Operand>),
    /// Stop, because control cannot continue.
    ///
    /// Carries MIR's reason so that the trap message can distinguish a deliberate
    /// trap from a missing `return` — only [`Unreachable::Trap`] is a program the
    /// compiler believes is well-formed.
    Trap(Unreachable),
}

// ---------------------------------------------------------------------------
// A compiled body
// ---------------------------------------------------------------------------

/// A stack slot's size and alignment, resolved at lowering time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotPlan {
    /// Size in bytes.
    pub size: u64,
    /// Required alignment in bytes.
    pub align: u32,
}

/// One procedure's bytecode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Code {
    /// Which procedure this is.
    pub proc: ProcRef,
    /// The instruction stream, blocks flattened in reverse postorder.
    pub instrs: Vec<Instr>,
    /// How many registers a frame needs.
    ///
    /// `MirBody::value_count()` plus any temporaries a parallel copy needed. Sized
    /// by value count rather than by live range: a liveness pass would shrink it and
    /// ADR-0018 records the over-allocation as an accepted cost, because nothing
    /// measures a problem.
    pub registers: usize,
    /// The type of each register, indexed like [`Self::registers`].
    ///
    /// Carried so the interpreter never needs the `MirBody` it was compiled from.
    /// ADR-0002's trapping arithmetic needs the signedness and width of the
    /// *destination*, and a comparison needs them for its *operands*, so the types
    /// are not optional decoration — they are what makes `+` able to trap at the
    /// right boundary. They come from `MirBody::value(id).ty`, so this is a copy of
    /// MIR's answer rather than a second one.
    pub types: Vec<PoolId>,
    /// The frame's stack slots, in MIR `SlotId` order.
    pub slots: Vec<SlotPlan>,
    /// The registers the caller's arguments are bound to, in declaration order.
    pub params: Vec<Reg>,
    /// The instruction index execution starts at.
    pub entry: usize,
    /// The MIR provenance of every instruction, indexed like [`Self::instrs`].
    ///
    /// ADR-0020 §4. Without this the interpreter can say *what* went wrong and not
    /// *where*: MIR remembers, but a `Code` compiled from it did not, so a trap had no
    /// location however well the front end had tracked one.
    ///
    /// A span for every instruction rather than only for those that can trap, because
    /// the set of trapping instructions grows every wave and the narrow version would
    /// give a new one no location silently. A [`MirSpan`] is a small `Copy` enum and
    /// this structure already carries a [`PoolId`] per register.
    pub spans: Vec<MirSpan>,
}

impl Code {
    /// The instruction at `pc`, if there is one.
    #[must_use]
    pub fn instr(&self, pc: usize) -> Option<&Instr> {
        self.instrs.get(pc)
    }
}

// ---------------------------------------------------------------------------
// Foreign procedures
// ---------------------------------------------------------------------------

/// Everything the FFI bridge needs about a `#foreign` procedure.
///
/// The library is an `Option<String>` because `ForeignInfo::library` is *still* an
/// unresolved `Option<Symbol>` in the HIR: `jr-sema` checks that it names a library
/// (E0225) and records nothing, so the caller resolves it again. ADR-0018 §4 records
/// that this is the second independent resolution of the same declaration, and that
/// a third is the signal to intern the answer beside
/// [`jr_pool::Item::ForeignLibraryValue`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignProc {
    /// Which procedure this is.
    pub proc: ProcRef,
    /// The C symbol to call, e.g. `write`.
    pub symbol: String,
    /// The library it lives in, e.g. `c`, if the declaration named one.
    pub library: Option<String>,
    /// Parameter types, in declaration order.
    pub params: Vec<PoolId>,
    /// The return type. [`PoolId::VOID`] when the source omitted the arrow.
    pub ret: PoolId,
}

/// A procedure the VM can call: bytecode, or a foreign symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Routine {
    /// A Jairs procedure, lowered from MIR.
    Bytecode(Code),
    /// A `#foreign` declaration, called through the bridge.
    Foreign(ForeignProc),
}

// ---------------------------------------------------------------------------
// Block addressing during lowering
// ---------------------------------------------------------------------------

/// Where each MIR block landed in the instruction stream.
///
/// Kept as a separate table rather than folded into [`Code`] because it is only
/// needed while patching forward jumps; once lowering finishes, every target is an
/// absolute index and the table has no consumer.
#[derive(Debug, Default)]
pub(crate) struct BlockAddresses(Vec<Option<usize>>);

impl BlockAddresses {
    pub(crate) fn with_blocks(count: usize) -> Self {
        Self(vec![None; count])
    }

    pub(crate) fn set(&mut self, block: BlockId, pc: usize) {
        if let Some(slot) = self.0.get_mut(block.index()) {
            *slot = Some(pc);
        }
    }

    pub(crate) fn get(&self, block: BlockId) -> Option<usize> {
        self.0.get(block.index()).copied().flatten()
    }
}
