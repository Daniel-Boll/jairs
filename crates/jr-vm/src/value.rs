//! The runtime value a register holds, and the integer arithmetic ADR-0002 specifies.
//!
//! # Why a register is one of three things
//!
//! [`Value`] has a scalar case and an aggregate case, and nothing else. That
//! follows from `jr-mir`: `escape.rs`'s `is_register_representable` already decided
//! that `bool`, the integer types and pointers are the only types that fit a
//! register, and everything else — `string` above all, which ADR-0004 makes a
//! two-word `{data, count}` pair — lives in memory.
//!
//! An aggregate can nonetheless *reach* a register, which is why the case exists: a
//! `string` parameter is a block parameter, a `string` literal is an
//! `Operand::Constant`, and `Rvalue::Use` copies either. `jr-mir` spills an
//! aggregate parameter to a slot the moment a field of it is read, so an aggregate
//! in a register is only ever moved wholesale — never indexed.
//!
//! # Why no type is stored alongside
//!
//! A [`Value`] is bits, and its type comes from `MirBody::value(id).ty` or
//! `Pool::type_of` for a constant. Carrying a copy here would be a second answer to
//! a question MIR and the pool already answer, and two answers are two chances to
//! disagree — the same argument `jr_mir::Operand::Constant` makes for not storing a
//! type beside a `PoolId`.
//!
//! # Where the arithmetic went
//!
//! ADR-0002's integer arithmetic used to live here, because the interpreter was the
//! only thing that evaluated it. `jr-mir`'s constant folder is a second, and
//! `jr-mir` cannot depend on this crate, so ADR-0022 §2 moved [`IntKind`] and the
//! checked operations into `jr-pool` — where `IntKind::of` was already reading
//! `Item::IntType`. [`IntKind`] is re-exported here so no consumer of `jr_vm` broke.
//!
//! What stayed is the mapping from `jr-pool`'s vocabulary to this crate's:
//! `interp.rs` turns a `jr_pool::IntTrap` into a [`Trap`], and
//! `IntTrap::Overflow` carries the same `&'static str` the message is built from, so
//! the move cannot have changed a single byte of a trap message — which
//! `differential.rs` compares.

pub use jr_pool::IntKind;

use crate::error::{Trap, VmError};

// ---------------------------------------------------------------------------
// Value
// ---------------------------------------------------------------------------

/// A byte address inside the VM's [`crate::Memory`].
///
/// Zero is never a valid allocation, so it is available as the null pointer — the
/// gap `jr-mir`'s `zero_value` records as the reason a default-initialised pointer
/// local is treated as uninitialised.
pub type Address = u64;

/// What one register holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// The single value of type `void`.
    Void,
    /// A `bool` (0 or 1), an integer's raw bits, or a pointer's [`Address`].
    ///
    /// Integer bits are always **normalised to the type's width**: the bits above
    /// `bits` are zero, for signed and unsigned alike. That invariant is what makes
    /// [`Value::as_int`] able to recover the mathematical value from the type alone,
    /// and it is restored after every arithmetic operation by [`IntKind::wrap`].
    Scalar(u64),
    /// An aggregate held by value, as its bytes in target layout.
    Aggregate(Vec<u8>),
    /// A value that was never assigned.
    ///
    /// Distinct from any bit pattern, because `Rvalue::Undef` is a well-typed value
    /// with no bits rather than a zero. Reading one traps
    /// ([`Trap::UninitialisedRead`]) instead of yielding a plausible number, so the
    /// bug E0227 reports statically does not become a wrong answer when the check
    /// is skipped.
    Undefined,
}

impl Value {
    /// A boolean.
    #[must_use]
    pub const fn bool(value: bool) -> Self {
        Self::Scalar(if value { 1 } else { 0 })
    }

    /// The scalar bits, or an error naming what was found instead.
    ///
    /// # Errors
    /// [`VmError::Trap`] for an undefined value, [`VmError::Internal`] otherwise —
    /// a non-scalar here means MIR's types and the bytecode disagree.
    pub fn scalar(&self) -> Result<u64, VmError> {
        match self {
            Self::Scalar(bits) => Ok(*bits),
            Self::Undefined => Err(VmError::Trap(Trap::UninitialisedRead)),
            Self::Void => Err(VmError::internal("expected a scalar, found void")),
            Self::Aggregate(_) => Err(VmError::internal("expected a scalar, found an aggregate")),
        }
    }

    /// The scalar interpreted as a `bool`.
    ///
    /// # Errors
    /// As [`Self::scalar`].
    pub fn boolean(&self) -> Result<bool, VmError> {
        Ok(self.scalar()? != 0)
    }

    /// The mathematical value of an integer of type `kind`.
    ///
    /// # Errors
    /// As [`Self::scalar`].
    pub fn as_int(&self, kind: IntKind) -> Result<i128, VmError> {
        Ok(kind.decode(self.scalar()?))
    }

    /// The aggregate's bytes.
    ///
    /// # Errors
    /// [`VmError::Trap`] for an undefined value, [`VmError::Internal`] otherwise.
    pub fn aggregate(&self) -> Result<&[u8], VmError> {
        match self {
            Self::Aggregate(bytes) => Ok(bytes),
            Self::Undefined => Err(VmError::Trap(Trap::UninitialisedRead)),
            Self::Void | Self::Scalar(_) => {
                Err(VmError::internal("expected an aggregate, found a scalar"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bool_is_a_normalised_scalar() {
        assert_eq!(Value::bool(true), Value::Scalar(1));
        assert_eq!(Value::bool(false), Value::Scalar(0));
        assert!(Value::bool(true).boolean().expect("a scalar is a bool"));
    }

    #[test]
    fn an_undefined_value_traps_rather_than_reading_as_zero() {
        // `Rvalue::Undef` is a well-typed value with no bits, not poison and not a
        // zero. Reading one must trap, or the bug E0227 reports statically becomes a
        // plausible wrong answer when the check is skipped.
        assert!(matches!(
            Value::Undefined.scalar(),
            Err(VmError::Trap(Trap::UninitialisedRead))
        ));
        assert!(matches!(
            Value::Undefined.aggregate(),
            Err(VmError::Trap(Trap::UninitialisedRead))
        ));
    }

    #[test]
    fn as_int_decodes_through_the_shared_kind() {
        // The arithmetic itself is `jr-pool`'s and tested there (ADR-0022 §2); what
        // is this crate's is that a register's bits go through the right kind.
        assert_eq!(
            Value::Scalar(u64::MAX)
                .as_int(IntKind::S64)
                .expect("a scalar"),
            -1
        );
    }
}
