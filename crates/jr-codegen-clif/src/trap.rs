//! The traps native code can raise, and the words it raises them in.
//!
//! # Why the wording matters
//!
//! `PLAN.md` §1.4 asks for a differential harness: every corpus program's output
//! must match under the VM and native. A *failing* program's output is its trap
//! message, so if the two disagree about wording they disagree about output, and the
//! harness can only ever compare programs that succeed — which is the half where
//! comptime and runtime are least likely to differ.
//!
//! So each message here is the wording `jr-vm`'s `Trap` produces, and
//! [`TrapKind::EXIT_STATUS`] is the status `jr run` uses. That is a coupling, and it
//! is a deliberate one: it is the coupling being tested.
//!
//! # Why there is no `BadAddress`
//!
//! It is a property of the VM's memory model — a bounds-checked linear region — and
//! not of a machine. Native code dereferencing a dangling pointer faults, and no
//! amount of message-matching would make the two comparable. `jr-vm`'s own docs note
//! that a dangling pointer into a released frame is reachable from a valid program,
//! so this is a real and known limit of the differential rather than an oversight.

/// A trap native code can raise.
///
/// One variant per `jr-vm` `Trap` that a machine can reproduce; see the module docs
/// for the one that it cannot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TrapKind {
    /// An addition overflowed. ADR-0002: `+`, `-`, `*` and unary `-` trap rather
    /// than wrap, and `+%`, `-%`, `*%` are the opt-out.
    ///
    /// There is a variant per operation, rather than one `Overflow`, because
    /// `jr-vm`'s trap names the operation — `"addition overflowed"` — and a
    /// differential that compares a failing program's output compares that
    /// sentence. One shared message would have made every overflow trap
    /// *look* like a disagreement.
    OverflowAdd,
    /// A subtraction overflowed.
    OverflowSub,
    /// A multiplication overflowed.
    OverflowMul,
    /// A division overflowed, which is `MIN / -1` and nothing else.
    OverflowDiv,
    /// A remainder overflowed, which is `MIN % -1` and nothing else.
    OverflowRem,
    /// A negation overflowed, which is `-MIN` and nothing else.
    OverflowNeg,
    /// A divisor was zero.
    DivideByZero,
    /// `Terminator::Unreachable(Unreachable::Trap)` was reached.
    Deliberate,
    /// A `break` or `continue` outside a loop was reached at run time.
    ///
    /// E0229 reports this statically, so reaching it means the program was run
    /// without being checked.
    StrayJump,
    /// Control fell off the end of a procedure that must return a value.
    ///
    /// E0228 reports this statically, for the same reason.
    FellOffEnd,
    /// A value that was never assigned was read.
    ///
    /// E0227 reports this statically. `Rvalue::Undef` is *not* poison — it is a
    /// well-typed value with no bits — so native traps on use rather than reading a
    /// zero, which would hide the bug the diagnostic exists to report.
    UninitialisedRead,
}

impl TrapKind {
    /// Every kind, so the driver can emit one message object per kind up front.
    ///
    /// Listed rather than derived, and the array's length is checked by a test, so
    /// that adding a variant without emitting its message is caught.
    pub const ALL: [Self; 11] = [
        Self::OverflowAdd,
        Self::OverflowSub,
        Self::OverflowMul,
        Self::OverflowDiv,
        Self::OverflowRem,
        Self::OverflowNeg,
        Self::DivideByZero,
        Self::Deliberate,
        Self::StrayJump,
        Self::FellOffEnd,
        Self::UninitialisedRead,
    ];

    /// The status a trapped program exits with.
    ///
    /// `jr run` uses 4, on the grounds that a program which compiled and then
    /// trapped is a different outcome from one that never compiled. Native matches
    /// it so that a script driving the compiler cannot tell the two execution
    /// engines apart.
    pub const EXIT_STATUS: i32 = 4;

    /// The message this trap reports, byte for byte as `jr run` reports it.
    ///
    /// That includes the `error: ` prefix and the trailing newline, because the
    /// comparison the differential harness makes is between two processes'
    /// **stderr**, not between two Rust strings. `jr-cli`'s `report::error` produces
    /// exactly this shape, and the wording after the prefix is `jr-vm`'s `Trap`
    /// rendering.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::OverflowAdd => "error: addition overflowed\n",
            Self::OverflowSub => "error: subtraction overflowed\n",
            Self::OverflowMul => "error: multiplication overflowed\n",
            Self::OverflowDiv => "error: division overflowed\n",
            Self::OverflowRem => "error: remainder overflowed\n",
            Self::OverflowNeg => "error: negation overflowed\n",
            Self::DivideByZero => "error: division by zero\n",
            Self::Deliberate => "error: reached a deliberate trap\n",
            Self::StrayJump => "error: a `break` or `continue` outside a loop was reached\n",
            Self::FellOffEnd => {
                "error: control reached the end of a procedure that must return a value\n"
            }
            Self::UninitialisedRead => "error: read a value that was never assigned\n",
        }
    }

    /// The symbol name of the data object holding this message.
    #[must_use]
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::OverflowAdd => "jr$trap$overflow_add",
            Self::OverflowSub => "jr$trap$overflow_sub",
            Self::OverflowMul => "jr$trap$overflow_mul",
            Self::OverflowDiv => "jr$trap$overflow_div",
            Self::OverflowRem => "jr$trap$overflow_rem",
            Self::OverflowNeg => "jr$trap$overflow_neg",
            Self::DivideByZero => "jr$trap$divide_by_zero",
            Self::Deliberate => "jr$trap$deliberate",
            Self::StrayJump => "jr$trap$stray_jump",
            Self::FellOffEnd => "jr$trap$fell_off_end",
            Self::UninitialisedRead => "jr$trap$uninitialised_read",
        }
    }
}

/// The name of the runtime helper a trap calls.
///
/// It takes a message pointer and a length and does not return. `jr-link` supplies
/// it; ADR-0019 §2 explains why a call rather than a bare machine trap.
pub const TRAP_HELPER: &str = "jr_trap";

#[cfg(test)]
mod tests {
    use super::TrapKind;

    #[test]
    fn every_kind_is_listed_in_all() {
        // `ALL` is what the driver iterates to emit message objects, so a variant
        // missing from it would produce a `CodegenError::Internal` at the first trap
        // site instead of a compile error here.
        assert_eq!(TrapKind::ALL.len(), 11);
        let mut sorted = TrapKind::ALL;
        sorted.sort_unstable();
        sorted.windows(2).for_each(|pair| {
            assert_ne!(pair[0], pair[1], "a kind is listed twice");
        });
    }

    #[test]
    fn messages_and_symbols_are_distinct() {
        for (index, kind) in TrapKind::ALL.iter().enumerate() {
            for other in &TrapKind::ALL[index + 1..] {
                assert_ne!(kind.message(), other.message());
                assert_ne!(kind.symbol(), other.symbol());
            }
        }
    }
}
