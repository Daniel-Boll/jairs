//! The bytecode compile-time execution engine: lowering from MIR, the interpreter, and the comptime FFI bridge.
//!
//! **[ADR-0018](../../../docs/adr/0018-vm-shape.md) is this crate's specification.**
//! It settles four decisions with their rejected alternatives named: the bytecode is
//! a register machine addressed by `ValueId`; layout lives in `jr-pool` with the
//! target passed in; const evaluation is a `jr-db` query rather than a fold inside
//! `jr-sema`; and foreign calls go through libffi, gated by an execution mode so
//! that ADR-0006's comptime allowance stays off until wave W6.
//!
//! It exists to serve one invariant, `PLAN.md` §3.1's:
//!
//! > comptime and runtime execute *the same* MIR. The VM consumes bytecode lowered
//! > from the identical MIR that Cranelift consumes.

mod assemble;
mod code;
mod error;
mod ffi;
mod interp;
mod lower;
mod memory;
mod value;

pub use assemble::{add_file, add_file_globals, comptime_program};
pub use code::{
    Code, ForeignProc, Instr, Operand, PlacePlan, PlaceRoot, PlaceStep, Reg, Routine, Shape,
    SlotPlan,
};
pub use error::{Trap, TrapSite, VmError};
pub use interp::{MAX_DEPTH, Mode, Program, Vm};
pub use lower::{compile, is_local_call};
pub use memory::{DEFAULT_CAPACITY, Mark, Memory};
pub use value::{Address, IntKind, Value};
