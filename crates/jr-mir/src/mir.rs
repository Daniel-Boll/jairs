//! The typed SSA data structures: blocks, block parameters, values, and slots.
//!
//! # Why a block owns its statements
//!
//! A [`MirBody`] is a `Vec` of blocks, each owning a `Vec<Statement>` and
//! terminated by exactly one [`Terminator`]. This is rustc's shape and it is
//! chosen for rustc's reasons: passes mutate a body in place, a [`BlockId`] is an
//! index rather than a reference, and the whole structure is `Clone` and cheap to
//! share behind an `Arc` — which matters because MIR is a memoized query result.
//!
//! ADR-0017 §1 records the two rejected alternatives. Cranelift keeps entities in
//! an arena and their *order* in a separate intrusive doubly-linked map, which
//! buys O(1) splicing with ids that survive movement; that is a better structure
//! for a mature optimiser, and it stays available, because it changes the order
//! representation over the same ids. Zig's AIR is one flat instruction array with
//! structured control flow, which Zig can afford because AIR has essentially no
//! mid-end — we have committed to an inliner, DCE and const-prop over a real CFG.
//!
//! # Why a phi is a block parameter
//!
//! There is no phi statement. A value that differs per predecessor is a parameter
//! of the block, and each edge supplies arguments through [`Target::args`].
//! Cranelift's IR reference is explicit that it "does not have phi instructions
//! but uses BB parameters instead", and Swift SIL uses basic-block arguments.
//!
//! This is not cosmetic. Braun's SSA construction (see [`crate`] docs and
//! `ssa.rs`) creates an *incomplete* phi every time it reads a variable in a
//! block whose predecessors are not all known yet. With block parameters that is
//! a push onto [`BlockData::params`]. With phi statements it would be a *prepend*
//! to `BlockData::stmts` — the one mutation a `Vec` is bad at, and one that
//! invalidates every cached statement index in the body. It also makes the
//! eventual Cranelift lowering a one-to-one mapping onto `append_block_param`
//! instead of requiring an unphi pass whose only job is to undo a representation
//! choice.
//!
//! # Why there is no `IndexVec`
//!
//! rustc uses `IndexVec<K, V>`. This workspace has no such type and no
//! `index_vec` dependency, and adding one to hold three vectors is not worth a
//! new dependency (ADR-0009 pins dependencies deliberately). Instead each arena
//! is a `Vec<T>` with a [`jr_base::newtype_index`] key and a typed accessor, which
//! is exactly what `jr-hir`'s `Body` and `FileHir` already do.
//!
//! # Why these arenas are private
//!
//! Unlike `jr-hir`'s HIR, whose arenas are public fields, [`MirBody`] keeps its
//! arenas private. The reason is [`MirBody::predecessors`] and
//! [`MirBody::reverse_postorder`]: they are cached, and the cache is only sound if
//! every mutation of the blocks goes through [`MirBody::blocks_mut`], which
//! invalidates it. A public field would let a caller edit the CFG behind the
//! cache's back.

use std::sync::{Arc, OnceLock};

use jr_base::FileId;
use jr_hir::{BodyId, ExprId, ExprScope, LocalId, ProcId, StmtId};
use jr_pool::{IntCmp, IntOp, PoolId};

// ---------------------------------------------------------------------------
// Identities
// ---------------------------------------------------------------------------

/// A procedure named across file boundaries.
///
/// A bare [`ProcId`] indexes *one* file's `FileHir::procs`, so it cannot name a
/// procedure in an imported module. ADR-0017 left [`Callee::Direct`] carrying one
/// and consequently refused every cross-file call; ADR-0018 §5 amends that,
/// because `PLAN.md` §1.4's exit criterion — `024-hello.jr` calling `print` from
/// `modules/Basic` — cannot be met otherwise.
///
/// This does **not** reintroduce the cross-body dependency ADR-0017 §3 keeps out
/// of the built-MIR query. Resolving a callee needs the callee's *signature*, to
/// know it is a procedure and to type its arguments — both already done by
/// `jr-sema`, whose signature phase ADR-0016 §5 established depends only on the
/// other file's HIR. The callee's *body* is fetched later, by whoever executes the
/// call, through that file's own query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProcRef {
    /// The file the procedure is declared in.
    pub file: FileId,
    /// Its index within that file's procedures.
    pub proc: ProcId,
}

impl ProcRef {
    /// Names a procedure.
    #[must_use]
    pub const fn new(file: FileId, proc: ProcId) -> Self {
        Self { file, proc }
    }
}

jr_base::newtype_index! {
    /// A basic block within one [`MirBody`].
    pub struct BlockId;
}

jr_base::newtype_index! {
    /// An SSA value within one [`MirBody`].
    ///
    /// Every value is defined exactly once — either by a [`Statement::Assign`] or
    /// by appearing in some [`BlockData::params`] — and that is checked by the
    /// verifier rather than merely intended.
    pub struct ValueId;
}

jr_base::newtype_index! {
    /// A stack slot within one [`MirBody`].
    ///
    /// Slots exist for the locals that SSA construction refused to promote:
    /// those whose address is taken, and those whose type is not
    /// register-representable. See `escape.rs`.
    pub struct SlotId;
}

// ---------------------------------------------------------------------------
// Provenance
// ---------------------------------------------------------------------------

/// Where a piece of MIR came from, as an HIR identity rather than a byte range.
///
/// MIR deliberately does **not** store [`jr_base::Span`]s. ADR-0013 already
/// records that spans embedded in HIR nodes make an unrelated whitespace edit
/// invalidate downstream salsa queries, and deferred the `AstIdMap` that would
/// fix it. Copying byte offsets into MIR — a *second*, larger memoized structure
/// — would deepen that debt. Instead MIR names the HIR node and a diagnostic
/// resolves the span only when it is rendered.
///
/// This is rust-analyzer's `MirSpan`, which stores `ExprId`/`PatId`/`BindingId`
/// for the same reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MirSpan {
    /// An expression, keyed by its arena as well as its index.
    ///
    /// The [`ExprScope`] is not redundant even though a body's MIR only ever
    /// references that body's arena: `FileHir::exprs` and every `Body::exprs`
    /// start at index 0, and a bare `ExprId` has already caused one real
    /// collision bug in `jr-hir`'s `ResolveMap`. Carrying the scope makes the
    /// mistake unrepresentable rather than merely avoided.
    Expr(ExprScope, ExprId),
    /// A local's declaration.
    Local(BodyId, LocalId),
    /// A statement.
    Stmt(BodyId, StmtId),
    /// A parameter of a procedure, by position in `Proc::params`.
    Param(ProcId, u32),
    /// Compiler-synthesised MIR with no source counterpart.
    Synthetic,
}

// ---------------------------------------------------------------------------
// Operators
// ---------------------------------------------------------------------------

/// A binary operator that MIR can express.
///
/// This mirrors [`jr_hir::BinOp`] minus its `And` and `Or` variants, and the
/// omission is the point: logical `&&` and `||` short-circuit, so they lower to
/// *control flow* — two blocks and a block parameter — and can never appear as an
/// [`Rvalue::Binary`]. Declaring MIR's own operator set makes that a fact the
/// type system enforces, instead of a convention a future pass could violate. It
/// also makes the HIR-to-MIR operator translation an exhaustive match, so a new
/// HIR operator is a compile error here.
///
/// The trapping and wrapping forms are deliberately kept distinct: ADR-0002 makes
/// `+`, `-` and `*` trap on overflow, and collapsing them onto the wrapping forms
/// would silently discard that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinOp {
    /// Addition, trapping on overflow (ADR-0002).
    Add,
    /// Subtraction, trapping on overflow (ADR-0002).
    Sub,
    /// Multiplication, trapping on overflow (ADR-0002).
    Mul,
    /// Division. Traps on division by zero and on `MIN / -1`.
    Div,
    /// Remainder. Traps on a zero divisor.
    Rem,
    /// Wrapping addition (`+%`).
    WrapAdd,
    /// Wrapping subtraction (`-%`).
    WrapSub,
    /// Wrapping multiplication (`*%`).
    WrapMul,
    /// Equality.
    Eq,
    /// Inequality.
    Ne,
    /// Less than.
    Lt,
    /// Less than or equal.
    Le,
    /// Greater than.
    Gt,
    /// Greater than or equal.
    Ge,
}

impl BinOp {
    /// The arithmetic operation this is, if it is one.
    ///
    /// `None` for a comparison, which [`Self::as_int_cmp`] answers instead. The two
    /// are separate because a comparison's result is a `bool` and so must not be
    /// normalised through the destination's integer width — a distinction
    /// `jr_pool::IntOp` and `jr_pool::IntCmp` make unrepresentable.
    ///
    /// This translation is the whole reason ADR-0022 §2 could leave [`BinOp`] here
    /// rather than moving it into `jr-pool`: it is exhaustive, so a new MIR operator
    /// is a compile error at this point, which is the protection ADR-0017 wanted
    /// from MIR owning its own operator set.
    #[must_use]
    pub const fn as_int_op(self) -> Option<IntOp> {
        match self {
            Self::Add => Some(IntOp::Add),
            Self::Sub => Some(IntOp::Sub),
            Self::Mul => Some(IntOp::Mul),
            Self::Div => Some(IntOp::Div),
            Self::Rem => Some(IntOp::Rem),
            Self::WrapAdd => Some(IntOp::WrapAdd),
            Self::WrapSub => Some(IntOp::WrapSub),
            Self::WrapMul => Some(IntOp::WrapMul),
            Self::Eq | Self::Ne | Self::Lt | Self::Le | Self::Gt | Self::Ge => None,
        }
    }

    /// The comparison this is, if it is one. See [`Self::as_int_op`].
    #[must_use]
    pub const fn as_int_cmp(self) -> Option<IntCmp> {
        match self {
            Self::Eq => Some(IntCmp::Eq),
            Self::Ne => Some(IntCmp::Ne),
            Self::Lt => Some(IntCmp::Lt),
            Self::Le => Some(IntCmp::Le),
            Self::Gt => Some(IntCmp::Gt),
            Self::Ge => Some(IntCmp::Ge),
            Self::Add
            | Self::Sub
            | Self::Mul
            | Self::Div
            | Self::Rem
            | Self::WrapAdd
            | Self::WrapSub
            | Self::WrapMul => None,
        }
    }

    /// Whether this operation can trap, per ADR-0002.
    ///
    /// Read by DCE, which may not delete a dead assignment whose rvalue can trap:
    /// `jr-codegen-clif` already commits, at `body.rs:266`, to a discarded rvalue
    /// still being evaluated so that an overflow nobody wanted the result of still
    /// fires. ADR-0022 §4 makes that a rule rather than a comment.
    #[must_use]
    pub const fn can_trap(self) -> bool {
        match self {
            Self::Add | Self::Sub | Self::Mul | Self::Div | Self::Rem => true,
            Self::WrapAdd
            | Self::WrapSub
            | Self::WrapMul
            | Self::Eq
            | Self::Ne
            | Self::Lt
            | Self::Le
            | Self::Gt
            | Self::Ge => false,
        }
    }
}

/// A unary operator that MIR can express.
///
/// [`jr_hir::UnOp`] has a third variant, `AddrOf` — prefix `*` per ADR-0011.
/// It is absent here because taking an address is not an arithmetic operation on
/// a value: it becomes [`Rvalue::Address`] over a [`Place`], which is the only
/// form Cranelift's `stack_addr` and a future load/store optimiser can reason
/// about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnOp {
    /// Arithmetic negation. Traps on the most negative value (ADR-0002).
    Neg,
    /// Logical negation of a `bool`.
    Not,
}

impl UnOp {
    /// Whether this operation can trap, per ADR-0002.
    ///
    /// Negation can: `-MIN` is one past the maximum. See [`BinOp::can_trap`].
    #[must_use]
    pub const fn can_trap(self) -> bool {
        match self {
            Self::Neg => true,
            Self::Not => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Operands and places
// ---------------------------------------------------------------------------

/// An input to a statement or terminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Operand {
    /// The result of an SSA definition earlier in the dominance order.
    Value(ValueId),
    /// A compile-time constant, named by its entry in the [`jr_pool::Pool`].
    ///
    /// Literal payloads are *not* duplicated into MIR. The pool already interns
    /// integer, boolean and string values with their types as part of the key
    /// (ADR-0015), so a constant is one `PoolId` and two constants are equal
    /// exactly when a 32-bit compare says so. Its type is
    /// [`jr_pool::Pool::type_of`], which is why no type is stored alongside —
    /// a copy here could drift from the pool's answer.
    Constant(PoolId),
}

/// The root of a memory reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PlaceBase {
    /// A stack slot belonging to this body.
    Slot(SlotId),
    /// The address held in an operand — the base of a postfix `.*` (ADR-0011).
    Deref(Operand),
}

/// One step of a memory reference, applied left to right.
///
/// # Why a field is an index and not an offset
///
/// ADR-0017 §5 keeps layout out of MIR: nothing in this crate knows a size, an
/// alignment or an offset. A field is therefore its position in
/// [`jr_pool::Pool::struct_fields`], and turning that into a byte offset is
/// `jr-codegen-clif`'s job, where the target ABI actually lives. The VM and
/// Cranelift must agree on layout exactly, and the way to guarantee that is one
/// shared computation later rather than a premature one here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Projection {
    /// A struct field, by index into [`jr_pool::Pool::struct_fields`].
    Field(u32),
    /// A further dereference (ADR-0011's postfix `.*`).
    Deref,
    /// `string`'s `.data` pseudo-field, of type `*u8`.
    ///
    /// This is not [`Projection::Field`] with index 0. ADR-0004 fixes `string` as
    /// `{data: *u8, count: s64}` in *prose only*: the pool has no fields for it
    /// and `jr-sema` hardcodes the two names as pseudo-fields. Modelling them as
    /// struct fields would assert a layout nothing has committed to, so they get
    /// their own projections and codegen decides what they mean.
    StringData,
    /// `string`'s `.count` pseudo-field, of type `s64`.
    StringCount,
}

/// A memory location: a base plus a chain of projections.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Place {
    /// Where the reference starts.
    pub base: PlaceBase,
    /// The steps applied to the base, in order.
    pub projection: Vec<Projection>,
}

impl Place {
    /// A place naming a whole slot.
    #[must_use]
    pub const fn slot(slot: SlotId) -> Self {
        Self {
            base: PlaceBase::Slot(slot),
            projection: Vec::new(),
        }
    }

    /// A place naming whatever `pointer` points at.
    #[must_use]
    pub const fn deref(pointer: Operand) -> Self {
        Self {
            base: PlaceBase::Deref(pointer),
            projection: Vec::new(),
        }
    }

    /// Returns this place with one more projection applied.
    #[must_use]
    pub fn project(mut self, step: Projection) -> Self {
        self.projection.push(step);
        self
    }
}

// ---------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------

/// What is being called.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Callee {
    /// A procedure named directly, including a `#foreign` one, and including one
    /// in an imported module (ADR-0018 §5).
    ///
    /// `ForeignInfo::library` is still an unresolved `Option<Symbol>` in the HIR;
    /// `jr-sema` resolves it only far enough to verify it names a library and
    /// records nothing, so whoever performs the call resolves it again.
    Direct(ProcRef),
    /// A procedure reached through a value of procedure-pointer type.
    Indirect(Operand),
}

/// A computation that produces one value.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Rvalue {
    /// The operand itself, unchanged.
    Use(Operand),
    /// A binary operation. Never `&&` or `||` — see [`BinOp`].
    Binary {
        /// The operator.
        op: BinOp,
        /// The left operand.
        lhs: Operand,
        /// The right operand.
        rhs: Operand,
    },
    /// A unary operation. Never address-of — see [`UnOp`].
    Unary {
        /// The operator.
        op: UnOp,
        /// The operand.
        operand: Operand,
    },
    /// A call. A call returning `void` still defines a value of type
    /// [`PoolId::VOID`], which keeps every `Rvalue` uniformly value-producing;
    /// the alternative, a separate void-call statement, buys nothing and forces
    /// every pass to handle two shapes of call.
    Call {
        /// What is being called.
        callee: Callee,
        /// The arguments, in declaration order.
        args: Vec<Operand>,
    },
    /// A read from memory.
    Load(Place),
    /// The address of a place — the lowering of ADR-0011's prefix `*`.
    Address(Place),
    /// A value that was never assigned.
    ///
    /// Produced when SSA construction reaches the entry block still looking for a
    /// definition — which is exactly what `c: s64 = ---;` followed by a read of
    /// `c` does. It is *not* a poison value: the body is well-typed, and ADR-0017
    /// §4's refusal is about type errors, not about definite assignment. Reading
    /// one is a program bug, recorded in [`Facts::undefined_reads`] for the
    /// diagnostic that owns it rather than reported from here.
    ///
    /// A zero constant was rejected as the placeholder: it would silently make an
    /// uninitialised read *defined*, hiding the very bug the definite-assignment
    /// pass exists to report, and there is no zero for a `string` or a struct.
    Undef,
}

/// One step inside a basic block.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Statement {
    /// Defines an SSA value.
    Assign {
        /// The value being defined. Defined exactly once in the whole body.
        dest: ValueId,
        /// How it is computed.
        rvalue: Rvalue,
        /// Where it came from.
        span: MirSpan,
    },
    /// Writes to memory.
    Store {
        /// The destination.
        place: Place,
        /// The value written.
        value: Operand,
        /// Where it came from.
        span: MirSpan,
    },
    /// Evaluates an rvalue for its effects and discards the result — a statement
    /// expression, typically a call in statement position.
    Discard {
        /// The rvalue to evaluate.
        rvalue: Rvalue,
        /// Where it came from.
        span: MirSpan,
    },
    /// A statement that has been removed.
    ///
    /// Retained as a variant so that a pass can delete a statement in O(1)
    /// without shifting the rest of the block and invalidating every later index
    /// in it. rustc has exactly this variant, and rust-analyzer copied it.
    /// Nothing produces it yet; the mid-end will.
    Nop,
}

// ---------------------------------------------------------------------------
// Terminators
// ---------------------------------------------------------------------------

/// The destination of a control-flow edge, with the arguments it supplies to the
/// target block's parameters.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Target {
    /// The block being entered.
    pub block: BlockId,
    /// One argument per [`BlockData::params`] entry of `block`, in order.
    pub args: Vec<Operand>,
}

impl Target {
    /// An edge that supplies no arguments.
    #[must_use]
    pub const fn new(block: BlockId) -> Self {
        Self {
            block,
            args: Vec::new(),
        }
    }

    /// An edge that supplies `args`.
    #[must_use]
    pub const fn with_args(block: BlockId, args: Vec<Operand>) -> Self {
        Self { block, args }
    }
}

/// Why control cannot continue.
///
/// The distinction is kept because the three cases mean different things to a
/// reader of a MIR dump and, later, to codegen: only [`Unreachable::Trap`] is a
/// program the compiler believes is well-formed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Unreachable {
    /// A deliberate runtime trap.
    Trap,
    /// A `break` or `continue` that was not inside a loop.
    ///
    /// Nothing rejects this today: `jr-hir` lowers both unconditionally without
    /// checking, and `jr-sema` ignores them entirely, so MIR is the first pass
    /// that can even see it. Lowering records the fact in [`Facts::stray_jumps`]
    /// and terminates the block here rather than panicking.
    StrayJump,
    /// Control reached the end of a procedure that must return a value.
    ///
    /// Whether this is *reachable* is the missing-`return` diagnostic, which
    /// needs the CFG and so was deferred by `jr-sema`.
    FellOffEnd,
}

/// How a basic block ends. Every block has exactly one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Terminator {
    /// An unconditional edge.
    Goto(Target),
    /// A two-way branch on a `bool`.
    Branch {
        /// The condition.
        cond: Operand,
        /// Taken when `cond` is true.
        then_: Target,
        /// Taken when `cond` is false.
        else_: Target,
    },
    /// Returns from the procedure. `None` for a `void` return.
    Return(Option<Operand>),
    /// Control does not continue.
    Unreachable(Unreachable),
}

impl Terminator {
    /// The edges leaving this terminator, in order.
    #[must_use]
    pub fn targets(&self) -> Vec<&Target> {
        match self {
            Self::Goto(target) => vec![target],
            Self::Branch {
                cond: _,
                then_,
                else_,
            } => vec![then_, else_],
            Self::Return(_) | Self::Unreachable(_) => Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Blocks, values, slots
// ---------------------------------------------------------------------------

/// One basic block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockData {
    /// The block's parameters — this representation's phi nodes.
    pub params: Vec<ValueId>,
    /// The block's statements, in execution order.
    pub stmts: Vec<Statement>,
    /// How the block ends.
    pub term: Terminator,
}

impl BlockData {
    /// An empty block, provisionally terminated by [`Unreachable::Trap`].
    ///
    /// Lowering overwrites the terminator once it knows where control goes. The
    /// placeholder is a trap rather than an `Option<Terminator>` so that "every
    /// block has exactly one terminator" holds at every intermediate state, and
    /// a block lowering forgot to finish is a loud trap rather than a `None` that
    /// some later `unwrap` discovers.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            params: Vec::new(),
            stmts: Vec::new(),
            term: Terminator::Unreachable(Unreachable::Trap),
        }
    }
}

impl Default for BlockData {
    fn default() -> Self {
        Self::new()
    }
}

/// An SSA value's type and provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValueData {
    /// The value's type, interned in the pool.
    pub ty: PoolId,
    /// Where it came from.
    pub span: MirSpan,
}

/// A stack slot's type and provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotData {
    /// The type stored in the slot.
    pub ty: PoolId,
    /// The local this slot stands for, when it stands for one. A slot with no
    /// local is a compiler temporary.
    pub local: Option<LocalId>,
    /// Where it came from.
    pub span: MirSpan,
}

// ---------------------------------------------------------------------------
// Facts deferred to the next wave
// ---------------------------------------------------------------------------

/// A read of a local that has no definition on some path reaching it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UndefinedRead {
    /// The local that was read.
    pub local: LocalId,
    /// Where it was read.
    pub span: MirSpan,
}

/// Findings that lowering produced but does not report.
///
/// Both of these are diagnostics `jr-sema` deliberately deferred because they
/// need a CFG, and both fall out of building one — the undefined reads are simply
/// the variables Braun's construction found no definition for. They are recorded
/// as data rather than emitted as diagnostics because this crate raises none:
/// the diagnostic codes and their wording belong to the pass that owns them, and
/// E0227 is the first free code.
///
/// Nothing reads this yet. That is the seam.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    /// Locals read on a path that never defines them.
    pub undefined_reads: Vec<UndefinedRead>,
    /// `break` or `continue` statements that were not inside a loop.
    pub stray_jumps: Vec<MirSpan>,
}

// ---------------------------------------------------------------------------
// Poison
// ---------------------------------------------------------------------------

/// Why a body was refused rather than lowered.
///
/// ADR-0017 §4: lowering returns a `Result`, so no consumer can be handed MIR
/// built from poison — there is nothing to hand them. A boolean taint flag was
/// rejected because it is something every future consumer can forget to check,
/// whereas a `Result` is something the compiler will not let them forget.
///
/// Refusing emits **no diagnostic**. A body is poisoned because an earlier phase
/// already reported the cause, and a second message on the same line is noise;
/// this continues the poison discipline `jr-sema` established, under which
/// [`PoolId::ERROR`] flows silently and the invalid corpus produces zero sema
/// diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Poisoned {
    /// This body is itself broken. The string is a short, stable reason suitable
    /// for a snapshot; it is not user-facing prose.
    Here(&'static str),
    /// A body this one depends on is broken.
    ///
    /// Nothing produces this yet, because nothing reads across bodies until the
    /// inliner exists. It is declared now so that the inliner propagates a
    /// distinguishable case instead of re-reporting someone else's error —
    /// Zig keys `failed_analysis` and `transitive_failed_analysis` apart for
    /// exactly this reason.
    Transitive(ProcId),
}

// ---------------------------------------------------------------------------
// The body
// ---------------------------------------------------------------------------

/// Lazily-computed CFG facts, shared across clones of a body.
///
/// Held behind an `Arc` so that cloning a [`MirBody`] — which borrowck-style
/// consumers and the future inliner both do — shares whatever has already been
/// computed. Invalidation *replaces* the `Arc` rather than mutating through it,
/// so a clone taken before a mutation keeps its own valid cache. This is rustc's
/// `BasicBlocks` design.
#[derive(Debug, Default)]
struct CfgCache {
    predecessors: OnceLock<Vec<Vec<BlockId>>>,
    reverse_postorder: OnceLock<Vec<BlockId>>,
}

/// One procedure's typed SSA body.
///
/// ADR-0017 §3 makes this per *procedure*, not per file. Zig states the same
/// split in one sentence — "unlike ZIR where there is one instance for an entire
/// source file, each function gets its own `Air` instance" — and the line lands
/// in the same place here: the pre-typing IR (HIR) is per file, the post-typing
/// IR is per body.
#[derive(Debug, Clone)]
pub struct MirBody {
    /// The procedure this body belongs to, file included so that a body knows
    /// which arena its own [`ProcId`]s and [`ProcRef`]s are relative to.
    proc: ProcRef,
    blocks: Vec<BlockData>,
    values: Vec<ValueData>,
    slots: Vec<SlotData>,
    params: Vec<ValueId>,
    ret: PoolId,
    entry: BlockId,
    facts: Facts,
    cache: Arc<CfgCache>,
}

impl PartialEq for MirBody {
    /// Compares everything except the cache, which is derived.
    fn eq(&self, other: &Self) -> bool {
        self.proc == other.proc
            && self.blocks == other.blocks
            && self.values == other.values
            && self.slots == other.slots
            && self.params == other.params
            && self.ret == other.ret
            && self.entry == other.entry
            && self.facts == other.facts
    }
}

impl Eq for MirBody {}

impl MirBody {
    /// Creates a body with a single empty entry block.
    #[must_use]
    pub fn new(proc: ProcRef, ret: PoolId) -> Self {
        Self {
            proc,
            blocks: vec![BlockData::new()],
            values: Vec::new(),
            slots: Vec::new(),
            params: Vec::new(),
            ret,
            entry: BlockId::from_usize(0),
            facts: Facts::default(),
            cache: Arc::new(CfgCache::default()),
        }
    }

    // -----------------------------------------------------------------
    // Reading
    // -----------------------------------------------------------------

    /// The procedure this body belongs to.
    #[must_use]
    pub const fn proc(&self) -> ProcRef {
        self.proc
    }

    /// The file this body was lowered from.
    ///
    /// A [`Callee::Direct`] whose [`ProcRef::file`] equals this is a call within
    /// the same file, which is the only kind ADR-0017 could represent.
    #[must_use]
    pub const fn file(&self) -> FileId {
        self.proc.file
    }

    /// The entry block.
    #[must_use]
    pub const fn entry(&self) -> BlockId {
        self.entry
    }

    /// The procedure's return type. [`PoolId::VOID`] when it returns nothing.
    #[must_use]
    pub const fn ret(&self) -> PoolId {
        self.ret
    }

    /// The values bound to the procedure's parameters, in declaration order.
    ///
    /// These are also the entry block's parameters. Parameters are *not* locals:
    /// `jr-hir`'s `Body` does not store them at all — `lower_body` builds them and
    /// then discards them — so MIR reconstructs them from `Proc::params` and the
    /// signature's parameter types.
    #[must_use]
    pub fn params(&self) -> &[ValueId] {
        &self.params
    }

    /// All blocks, in index order. Index order is *not* execution order; see
    /// [`Self::reverse_postorder`].
    #[must_use]
    pub fn blocks(&self) -> &[BlockData] {
        &self.blocks
    }

    /// One block.
    ///
    /// # Panics
    /// Panics if `block` does not belong to this body.
    #[must_use]
    pub fn block(&self, block: BlockId) -> &BlockData {
        &self.blocks[block.index()]
    }

    /// One value's type and provenance.
    ///
    /// # Panics
    /// Panics if `value` does not belong to this body.
    #[must_use]
    pub fn value(&self, value: ValueId) -> &ValueData {
        &self.values[value.index()]
    }

    /// One slot's type and provenance.
    ///
    /// # Panics
    /// Panics if `slot` does not belong to this body.
    #[must_use]
    pub fn slot(&self, slot: SlotId) -> &SlotData {
        &self.slots[slot.index()]
    }

    /// The number of blocks.
    #[must_use]
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// The number of SSA values.
    #[must_use]
    pub fn value_count(&self) -> usize {
        self.values.len()
    }

    /// The number of stack slots.
    #[must_use]
    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// Findings that lowering recorded but did not report.
    #[must_use]
    pub const fn facts(&self) -> &Facts {
        &self.facts
    }

    // -----------------------------------------------------------------
    // Building
    // -----------------------------------------------------------------

    /// Appends an empty block and returns its id.
    pub fn push_block(&mut self) -> BlockId {
        self.invalidate_cfg();
        let id = BlockId::from_usize(self.blocks.len());
        self.blocks.push(BlockData::new());
        id
    }

    /// Declares a new SSA value.
    pub fn push_value(&mut self, ty: PoolId, span: MirSpan) -> ValueId {
        let id = ValueId::from_usize(self.values.len());
        self.values.push(ValueData { ty, span });
        id
    }

    /// Declares a new stack slot.
    pub fn push_slot(&mut self, ty: PoolId, local: Option<LocalId>, span: MirSpan) -> SlotId {
        let id = SlotId::from_usize(self.slots.len());
        self.slots.push(SlotData { ty, local, span });
        id
    }

    /// Sets the procedure's parameter values.
    pub fn set_params(&mut self, params: Vec<ValueId>) {
        self.params = params;
    }

    /// Replaces the recorded facts.
    pub fn set_facts(&mut self, facts: Facts) {
        self.facts = facts;
    }

    /// Mutable access to one block's statements, which cannot change the CFG.
    ///
    /// The terminator is deliberately not reachable through this, so the cached
    /// predecessors and block order stay valid. rustc draws the same line with
    /// its `as_mut_preserves_cfg`.
    pub fn stmts_mut(&mut self, block: BlockId) -> &mut Vec<Statement> {
        &mut self.blocks[block.index()].stmts
    }

    /// Mutable access to all blocks, invalidating the CFG cache.
    ///
    /// Anything that can change a terminator or add a block must go through
    /// here. This is why [`Self::blocks`] is not a public field.
    pub fn blocks_mut(&mut self) -> &mut Vec<BlockData> {
        self.invalidate_cfg();
        &mut self.blocks
    }

    /// Sets one block's terminator.
    pub fn set_terminator(&mut self, block: BlockId, term: Terminator) {
        self.invalidate_cfg();
        self.blocks[block.index()].term = term;
    }

    /// Appends a parameter to a block and returns the value bound to it.
    ///
    /// This is how a phi is created. Callers must keep every incoming
    /// [`Target::args`] in step, which the verifier checks.
    pub fn push_block_param(&mut self, block: BlockId, ty: PoolId, span: MirSpan) -> ValueId {
        let value = self.push_value(ty, span);
        self.blocks[block.index()].params.push(value);
        value
    }

    // -----------------------------------------------------------------
    // Derived CFG facts
    // -----------------------------------------------------------------

    fn invalidate_cfg(&mut self) {
        self.cache = Arc::new(CfgCache::default());
    }

    // -----------------------------------------------------------------
    // Arena compaction
    // -----------------------------------------------------------------

    /// Drops the blocks `keep` marks false, renumbering every survivor.
    ///
    /// # Why this is here rather than in the pass that wants it
    ///
    /// [`Self::blocks`] and [`Self::entry`] are private (see the module docs) so
    /// that no caller can edit the CFG behind the cached predecessors and block
    /// order. Renumbering has to rewrite the entry and every [`Target::block`] in
    /// step, and doing that from outside would need both of them public — which is
    /// the invariant this type exists to hold. ADR-0022 §4 wants unreachable blocks
    /// gone, so the compaction is a method.
    ///
    /// # Panics
    /// If `keep` is not one entry per block, or if it would drop the entry block.
    /// The second is a pass bug rather than a program property: the entry is
    /// reachable from itself by definition, so anything computing reachability that
    /// excludes it has computed something else.
    pub fn retain_blocks(&mut self, keep: &[bool]) {
        assert_eq!(
            keep.len(),
            self.blocks.len(),
            "retain_blocks needs one flag per block"
        );
        assert!(
            keep[self.entry.index()],
            "the entry block is always reachable"
        );

        let mut remap = vec![None; self.blocks.len()];
        let mut next = 0usize;
        for (index, keep_it) in keep.iter().enumerate() {
            if *keep_it {
                remap[index] = Some(BlockId::from_usize(next));
                next += 1;
            }
        }

        self.invalidate_cfg();
        let mut index = 0usize;
        self.blocks.retain(|_| {
            let keep_it = keep[index];
            index += 1;
            keep_it
        });
        self.entry = remap[self.entry.index()].expect("the entry is kept");

        for block in &mut self.blocks {
            for target in targets_mut(&mut block.term) {
                target.block = remap[target.block.index()]
                    .expect("a kept block cannot branch to a dropped one");
            }
        }
    }

    /// Drops the slots `keep` marks false, renumbering every survivor.
    ///
    /// Here for the same reason as [`Self::retain_blocks`]: the slot arena is
    /// private, and every [`PlaceBase::Slot`] in the body has to move with it.
    ///
    /// # Panics
    /// If `keep` is not one entry per slot, or if a kept statement still names a
    /// dropped slot — which means the pass computed liveness wrongly.
    pub fn retain_slots(&mut self, keep: &[bool]) {
        assert_eq!(
            keep.len(),
            self.slots.len(),
            "retain_slots needs one flag per slot"
        );

        let mut remap = vec![None; self.slots.len()];
        let mut next = 0usize;
        for (index, keep_it) in keep.iter().enumerate() {
            if *keep_it {
                remap[index] = Some(SlotId::from_usize(next));
                next += 1;
            }
        }

        let mut index = 0usize;
        self.slots.retain(|_| {
            let keep_it = keep[index];
            index += 1;
            keep_it
        });

        for block in &mut self.blocks {
            for stmt in &mut block.stmts {
                match stmt {
                    Statement::Assign { rvalue, .. } | Statement::Discard { rvalue, .. } => {
                        remap_rvalue_slots(rvalue, &remap);
                    }
                    Statement::Store { place, value, .. } => {
                        remap_place_slots(place, &remap);
                        remap_operand_slots(value, &remap);
                    }
                    Statement::Nop => {}
                }
            }
        }
    }

    // -----------------------------------------------------------------
    // Derived CFG facts
    // -----------------------------------------------------------------

    /// The predecessors of every block, indexed by [`BlockId`].
    ///
    /// Computed once and cached. rustc uses a `SmallVec<[_; 4]>` per block with
    /// the observation that "typically 95%+ of basic blocks have 4 or fewer
    /// predecessors"; this workspace has no `smallvec` dependency, so a plain
    /// `Vec` pays an allocation per block for now.
    #[must_use]
    pub fn predecessors(&self) -> &[Vec<BlockId>] {
        self.cache.predecessors.get_or_init(|| {
            let mut preds = vec![Vec::new(); self.blocks.len()];
            for (index, block) in self.blocks.iter().enumerate() {
                let from = BlockId::from_usize(index);
                for target in block.term.targets() {
                    preds[target.block.index()].push(from);
                }
            }
            preds
        })
    }

    /// Every block reachable from the entry, in reverse postorder.
    ///
    /// This is the order the bytecode lowering will linearise in, and the order
    /// a forward dataflow pass wants. Unreachable blocks are absent.
    #[must_use]
    pub fn reverse_postorder(&self) -> &[BlockId] {
        self.cache.reverse_postorder.get_or_init(|| {
            let mut order = Vec::with_capacity(self.blocks.len());
            let mut seen = vec![false; self.blocks.len()];
            // An explicit stack rather than recursion: a deeply nested body must
            // not be able to overflow the compiler's stack.
            let mut stack = vec![(self.entry, 0usize)];
            seen[self.entry.index()] = true;
            while let Some((block, next)) = stack.pop() {
                let targets = self.blocks[block.index()].term.targets();
                if next < targets.len() {
                    // Successors are visited in *reverse* order. Any DFS yields a
                    // valid reverse postorder, but visiting them forwards makes a
                    // branch's `else` arm finish last and so appear *first* in the
                    // reversed order. Walking them backwards puts `then` before
                    // `else`, which is the order a reader of a MIR dump expects.
                    let successor = targets[targets.len() - 1 - next].block;
                    stack.push((block, next + 1));
                    if !seen[successor.index()] {
                        seen[successor.index()] = true;
                        stack.push((successor, 0));
                    }
                } else {
                    order.push(block);
                }
            }
            order.reverse();
            order
        })
    }
}

// ---------------------------------------------------------------------------
// Renumbering helpers
// ---------------------------------------------------------------------------

/// The edges leaving a terminator, mutably.
///
/// The mirror of [`Terminator::targets`], private because the only caller is
/// [`MirBody::retain_blocks`] and handing out `&mut Target` generally would let a
/// caller rewrite an edge without invalidating the CFG cache.
fn targets_mut(term: &mut Terminator) -> Vec<&mut Target> {
    match term {
        Terminator::Goto(target) => vec![target],
        Terminator::Branch {
            cond: _,
            then_,
            else_,
        } => vec![then_, else_],
        Terminator::Return(_) | Terminator::Unreachable(_) => Vec::new(),
    }
}

fn remap_operand_slots(operand: &mut Operand, _remap: &[Option<SlotId>]) {
    match operand {
        // An operand names no slot: a value is an SSA definition and a constant is a
        // pool entry. The arm exists so that a future operand kind that *does* name
        // one is a compile error here.
        Operand::Value(_) | Operand::Constant(_) => {}
    }
}

fn remap_place_slots(place: &mut Place, remap: &[Option<SlotId>]) {
    match &mut place.base {
        PlaceBase::Slot(slot) => {
            *slot = remap[slot.index()].expect("a live place named a dropped slot");
        }
        PlaceBase::Deref(operand) => remap_operand_slots(operand, remap),
    }
}

fn remap_rvalue_slots(rvalue: &mut Rvalue, remap: &[Option<SlotId>]) {
    match rvalue {
        Rvalue::Use(operand) => remap_operand_slots(operand, remap),
        Rvalue::Binary { op: _, lhs, rhs } => {
            remap_operand_slots(lhs, remap);
            remap_operand_slots(rhs, remap);
        }
        Rvalue::Unary { op: _, operand } => remap_operand_slots(operand, remap),
        Rvalue::Call { callee, args } => {
            match callee {
                Callee::Direct(_) => {}
                Callee::Indirect(operand) => remap_operand_slots(operand, remap),
            }
            for arg in args {
                remap_operand_slots(arg, remap);
            }
        }
        Rvalue::Load(place) | Rvalue::Address(place) => remap_place_slots(place, remap),
        Rvalue::Undef => {}
    }
}

// ---------------------------------------------------------------------------
// A file's worth of bodies
// ---------------------------------------------------------------------------

/// Every procedure body in one file, lowered or refused.
///
/// A `Vec` in [`ProcId`] order rather than a map, so that iteration — and
/// therefore a snapshot of a MIR dump — is deterministic by construction rather
/// than by remembering to sort.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileMir {
    bodies: Vec<(ProcId, Result<MirBody, Poisoned>)>,
}

impl FileMir {
    /// Creates an empty result.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the outcome for one procedure. Callers push in [`ProcId`] order.
    pub fn push(&mut self, proc: ProcId, body: Result<MirBody, Poisoned>) {
        self.bodies.push((proc, body));
    }

    /// The outcome for one procedure, if it has a body at all.
    #[must_use]
    pub fn get(&self, proc: ProcId) -> Option<&Result<MirBody, Poisoned>> {
        self.bodies
            .iter()
            .find(|(id, _)| *id == proc)
            .map(|(_, body)| body)
    }

    /// Every outcome, in [`ProcId`] order.
    pub fn iter(&self) -> impl Iterator<Item = (ProcId, &Result<MirBody, Poisoned>)> {
        self.bodies.iter().map(|(proc, body)| (*proc, body))
    }

    /// The number of procedures with a body.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bodies.len()
    }

    /// Whether no procedure in the file had a body.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bodies.is_empty()
    }

    /// How many bodies lowered successfully.
    #[must_use]
    pub fn lowered_count(&self) -> usize {
        self.bodies.iter().filter(|(_, body)| body.is_ok()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> MirBody {
        MirBody::new(
            ProcRef::new(FileId::from_usize(0), ProcId::from_usize(0)),
            PoolId::VOID,
        )
    }

    #[test]
    fn a_fresh_body_has_one_block_terminated_by_a_trap() {
        let mir = body();
        assert_eq!(mir.block_count(), 1);
        assert_eq!(
            mir.block(mir.entry()).term,
            Terminator::Unreachable(Unreachable::Trap),
            "an unfinished block must be a loud trap, not a silent fallthrough"
        );
    }

    #[test]
    fn predecessors_are_computed_from_terminators() {
        let mut mir = body();
        let second = mir.push_block();
        mir.set_terminator(mir.entry(), Terminator::Goto(Target::new(second)));
        assert_eq!(mir.predecessors()[second.index()], vec![mir.entry()]);
        assert!(mir.predecessors()[mir.entry().index()].is_empty());
    }

    #[test]
    fn adding_a_block_invalidates_the_cached_predecessors() {
        let mut mir = body();
        let second = mir.push_block();
        mir.set_terminator(mir.entry(), Terminator::Goto(Target::new(second)));
        assert_eq!(mir.predecessors().len(), 2);
        let third = mir.push_block();
        mir.set_terminator(second, Terminator::Goto(Target::new(third)));
        assert_eq!(
            mir.predecessors().len(),
            3,
            "the cache must not survive a new block"
        );
        assert_eq!(mir.predecessors()[third.index()], vec![second]);
    }

    #[test]
    fn reverse_postorder_visits_the_entry_first_and_skips_unreachable_blocks() {
        let mut mir = body();
        let reachable = mir.push_block();
        let _orphan = mir.push_block();
        mir.set_terminator(mir.entry(), Terminator::Goto(Target::new(reachable)));
        mir.set_terminator(reachable, Terminator::Return(None));
        assert_eq!(mir.reverse_postorder(), &[mir.entry(), reachable]);
    }

    #[test]
    fn a_diamond_orders_both_arms_after_the_head() {
        let mut mir = body();
        let then_ = mir.push_block();
        let else_ = mir.push_block();
        let join = mir.push_block();
        mir.set_terminator(
            mir.entry(),
            Terminator::Branch {
                cond: Operand::Constant(PoolId::TRUE),
                then_: Target::new(then_),
                else_: Target::new(else_),
            },
        );
        mir.set_terminator(then_, Terminator::Goto(Target::new(join)));
        mir.set_terminator(else_, Terminator::Goto(Target::new(join)));
        mir.set_terminator(join, Terminator::Return(None));

        let order = mir.reverse_postorder();
        let position = |block: BlockId| order.iter().position(|b| *b == block).expect("reachable");
        assert_eq!(position(mir.entry()), 0);
        assert!(position(join) > position(then_));
        assert!(position(join) > position(else_));
    }

    #[test]
    fn a_block_parameter_is_a_value_of_the_block() {
        let mut mir = body();
        let block = mir.push_block();
        let param = mir.push_block_param(block, PoolId::S64, MirSpan::Synthetic);
        assert_eq!(mir.block(block).params, vec![param]);
        assert_eq!(mir.value(param).ty, PoolId::S64);
    }

    #[test]
    fn a_clone_keeps_its_own_cache_across_a_mutation_of_the_original() {
        let mut mir = body();
        let second = mir.push_block();
        mir.set_terminator(mir.entry(), Terminator::Goto(Target::new(second)));
        let snapshot = mir.clone();
        assert_eq!(snapshot.predecessors().len(), 2);
        let _third = mir.push_block();
        assert_eq!(
            snapshot.predecessors().len(),
            2,
            "the clone must not see the new block"
        );
        assert_eq!(mir.predecessors().len(), 3);
    }

    #[test]
    fn file_mir_iterates_in_push_order() {
        let mut file = FileMir::new();
        file.push(ProcId::from_usize(0), Err(Poisoned::Here("test")));
        file.push(ProcId::from_usize(1), Ok(body()));
        assert_eq!(file.len(), 2);
        assert_eq!(file.lowered_count(), 1);
        let ids: Vec<_> = file.iter().map(|(proc, _)| proc).collect();
        assert_eq!(ids, vec![ProcId::from_usize(0), ProcId::from_usize(1)]);
    }
}
