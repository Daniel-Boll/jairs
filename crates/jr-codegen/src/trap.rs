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
    /// A shift count was negative or `>=` the type's width (ADR-0042 §3).
    ///
    /// The third trapping operation, after overflow and division by zero. Cranelift's shift
    /// instructions mask the count rather than trapping, so this is an explicit
    /// compare-and-trap emitted before the shift — the masking would otherwise turn `x << 8`
    /// on an `s8` silently into `x << 0`.
    ShiftOutOfRange,
    /// `Terminator::Unreachable(Unreachable::Trap)` was reached.
    Deliberate,
    /// The stub a **refused** body gets ([`jr_mir::MirBody::refused`]) was reached.
    ///
    /// Its own kind rather than [`Self::Deliberate`], because the two mean opposite things:
    /// a deliberate trap is the program doing what it asked for, and this is the compiler
    /// admitting a gap that E0245 already named. Sharing one message told a user their
    /// program had deliberately trapped when it had done nothing of the kind.
    Refused,
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
    /// An array index was outside the array (ADR-0003, ADR-0039 §2).
    ///
    /// Raised by the comparison `Statement::BoundsCheck` lowers to. Unlike
    /// [`TrapKind::UninitialisedRead`], nothing reports this statically in general — a
    /// *literal* index out of range is E0236, but a computed one is only knowable here.
    IndexOutOfBounds,
    /// A call through a **null procedure pointer** (ADR-0110 §1).
    ///
    /// A language trap that both engines raise, so the differential harness compares them. Native code would
    /// otherwise jump to address zero and take a signal the compiler has nothing to say about; the VM would decode
    /// zero into an arbitrary real procedure and call it.
    NullCall,
    /// A `variant`'s case was read while its tag named a different one (ADR-0068 §4).
    ///
    /// Nothing reports this statically — which case is live is not decidable — so this is the runtime
    /// half of a check that has no compile-time half at all, unlike [`TrapKind::IndexOutOfBounds`]
    /// whose literal cases are E0236.
    WrongVariantCase,
}

impl TrapKind {
    /// Every kind, as a fixture for the invariants no single site can state.
    ///
    /// **Its doc used to say "so the driver can emit one message object per kind up front", and
    /// that driver does not exist** — a back end interns a message lazily at each trap site
    /// (`Backend::report`), so nothing outside this file's tests reads `ALL` at all. The stale
    /// sentence mattered: it made the list look load-bearing, so nobody checked it, and **four
    /// kinds were missing** — `ShiftOutOfRange`, `IndexOutOfBounds`, `NullCall` and
    /// `WrongVariantCase`. Found by ADR-0178 replacing a length assertion with an exhaustive match.
    ///
    /// The omission was not cosmetic. [`Self::reason`]'s distinctness test iterates this list, and
    /// its whole purpose is that no two kinds share a sentence — because the corpus differential
    /// compares *rendered messages*, so two kinds with one wording would make a genuine
    /// disagreement between the engines invisible. Four kinds were never checked for it.
    pub const ALL: [Self; 16] = [
        Self::OverflowAdd,
        Self::OverflowSub,
        Self::OverflowMul,
        Self::OverflowDiv,
        Self::OverflowRem,
        Self::OverflowNeg,
        Self::DivideByZero,
        Self::ShiftOutOfRange,
        Self::IndexOutOfBounds,
        Self::NullCall,
        Self::WrongVariantCase,
        Self::Deliberate,
        Self::Refused,
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

    /// The sentence this trap reports, without a prefix, a location or a newline.
    ///
    /// The *shape* of the message — the `error: ` prefix, the `  --> ` location line,
    /// the trailing newline — belongs to `jr_base::trap_message`, which the VM also
    /// calls. ADR-0020 §2 put it there so that two engines rendering at different
    /// times cannot drift; this function supplies only the part that is this back
    /// end's to decide, and the wording matches `jr-vm`'s `Trap` rendering exactly
    /// because `differential.rs` compares the finished bytes.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::OverflowAdd => "addition overflowed",
            Self::OverflowSub => "subtraction overflowed",
            Self::OverflowMul => "multiplication overflowed",
            Self::OverflowDiv => "division overflowed",
            Self::OverflowRem => "remainder overflowed",
            Self::OverflowNeg => "negation overflowed",
            Self::DivideByZero => "division by zero",
            Self::ShiftOutOfRange => "shift count out of range",
            Self::IndexOutOfBounds => "index out of bounds",
            Self::NullCall => "call through a null procedure pointer",
            Self::WrongVariantCase => "read the wrong variant case",
            Self::Deliberate => "reached a deliberate trap",
            Self::Refused => {
                "this procedure could not be compiled; the compiler reported a gap in it"
            }
            Self::StrayJump => "a `break` or `continue` outside a loop was reached",
            Self::FellOffEnd => "control reached the end of a procedure that must return a value",
            Self::UninitialisedRead => "read a value that was never assigned",
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
        // `ALL` is what the driver iterates to emit message objects, so a kind missing from it
        // produces a `CodegenError::Internal` at the first trap site rather than a failure here.
        //
        // **An exhaustive match rather than a count.** This test used to assert
        // `ALL.len() == 11`, which is a *proxy*: a new variant left out of `ALL` keeps the length
        // right and the test green, and `[Self; 11]` compiles perfectly well beside an enum with
        // twelve variants. It fired for ADR-0178 only because that wave happened to bump the array
        // length first — luck, not enforcement. Naming every variant here makes adding one a
        // **compile error** in this file, which is the same rule `AGENTS.md` states for match arms
        // and the reason it is stated.
        for kind in [
            TrapKind::OverflowAdd,
            TrapKind::OverflowSub,
            TrapKind::OverflowMul,
            TrapKind::OverflowDiv,
            TrapKind::OverflowRem,
            TrapKind::OverflowNeg,
            TrapKind::DivideByZero,
            TrapKind::ShiftOutOfRange,
            TrapKind::IndexOutOfBounds,
            TrapKind::NullCall,
            TrapKind::WrongVariantCase,
            TrapKind::Deliberate,
            TrapKind::StrayJump,
            TrapKind::FellOffEnd,
            TrapKind::UninitialisedRead,
            TrapKind::Refused,
        ] {
            // The exhaustiveness: a variant added to the enum and not to the list below is a
            // non-exhaustive-match error, and one added here but not to `ALL` fails the assert.
            let named = match kind {
                TrapKind::OverflowAdd
                | TrapKind::OverflowSub
                | TrapKind::OverflowMul
                | TrapKind::OverflowDiv
                | TrapKind::OverflowRem
                | TrapKind::OverflowNeg
                | TrapKind::DivideByZero
                | TrapKind::ShiftOutOfRange
                | TrapKind::IndexOutOfBounds
                | TrapKind::NullCall
                | TrapKind::WrongVariantCase
                | TrapKind::Deliberate
                | TrapKind::StrayJump
                | TrapKind::FellOffEnd
                | TrapKind::UninitialisedRead
                | TrapKind::Refused => kind,
            };
            assert!(
                TrapKind::ALL.contains(&named),
                "{named:?} is not in `ALL`, so no message object is emitted for it"
            );
        }

        let mut sorted = TrapKind::ALL;
        sorted.sort_unstable();
        sorted.windows(2).for_each(|pair| {
            assert_ne!(pair[0], pair[1], "a kind is listed twice");
        });
    }

    #[test]
    fn reasons_are_distinct() {
        // Two kinds sharing a sentence would make a real disagreement between the
        // engines invisible: the differential compares the rendered message, so an
        // overflow reported as a division-by-zero would still match.
        for (index, kind) in TrapKind::ALL.iter().enumerate() {
            for other in &TrapKind::ALL[index + 1..] {
                assert_ne!(kind.reason(), other.reason());
            }
        }
    }

    #[test]
    fn a_reason_carries_no_shape_of_its_own() {
        // `jr_base::trap_message` owns the prefix and the newline (ADR-0020 §2). A
        // reason that carried either would render as `error: error: ...`.
        for kind in TrapKind::ALL {
            let reason = kind.reason();
            assert!(!reason.starts_with("error:"), "{reason:?} has a prefix");
            assert!(!reason.ends_with('\n'), "{reason:?} has a newline");
        }
    }
}
