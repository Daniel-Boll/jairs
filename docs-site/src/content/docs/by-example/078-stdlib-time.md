---
title: "Time: a monotonic clock"
description: Reading a monotonic and a wall clock through clock_gettime, one nanosecond unit throughout, and why Time has no sleep and no formatting.
sidebar:
  order: 78
---

`Time` wraps one syscall, `clock_gettime`, into two readings and a set of truncating unit conversions.
Everything is `s64` nanoseconds — there is no `Duration` struct and no `float64` seconds — because a
single exact integer unit is arithmetic a caller can do without a library, and it is the one property a
benchmark actually needs (ADR-0155).

## Selecting the clock id at compile time

```jr
/// `CLOCK_MONOTONIC`, selected for the target (ADR-0181 §1).
///
/// **This constant is the reason `os()` exists.** It was `6` — macOS's number — with a comment saying so
/// and adding *"the day the CI matrix runs is the day this needs a `#if`-shaped answer, which this
/// language does not have either"*. It has one now, and it is not `#if`: `os()` is a compile-time **value**
/// (ADR-0180), so a per-OS number is selected by an ordinary procedure that a `#run` evaluates — the
/// file-scope idiom `modules/Random`'s `GOLDEN :: #run golden_seed();` already used.
///
/// Windows gets **0**, which is not a guess: Windows has no `clock_gettime` at all, so there is no correct
/// number, and `0` is `CLOCK_REALTIME`'s value — the reading a caller would get if the binding somehow
/// resolved, and otherwise the failure `clock_gettime`'s `-1` already reports. Naming a plausible-looking
/// constant for a call that cannot happen would be the worse answer.
monotonic_clock_id :: () -> s64 {
    if os() == Operating_System.MACOS {
        return 6;
    }
    if os() == Operating_System.LINUX {
        return 1;
    }
    return 0;
}

CLOCK_MONOTONIC :: #run monotonic_clock_id();

/// `CLOCK_REALTIME`, which is 0 on both macOS and Linux — so this constant, unlike the one above, is
/// genuinely portable.
CLOCK_REALTIME :: 0;
```

`monotonic_clock_id` is a public procedure, not a private helper, purely so the `#run` beside it has
something to call — the same file-scope idiom `File.CREATE` uses for a per-OS value (ADR-0184 §6).
There is no `#if` anywhere in this module: `os()` is an ordinary compile-time value, so a per-OS
number is an ordinary branch a `#run` evaluates once, at compile time, and the constant it produces
is indistinguishable from a literal to every caller.

**On Windows, `CLOCK_MONOTONIC` is `0`, and that is a placeholder rather than a fact.** Windows has no
`clock_gettime` at all, so there is no correct number to put here — the module deliberately does not
invent one that would let a wrong call *succeed*.

## Two clocks, one syscall shape

```jr
/// A `timespec` as C lays it out: two 64-bit words on both supported targets.
Timespec :: struct {
    /// Whole seconds.
    seconds: s64;
    /// Nanoseconds within the second, 0 to 999_999_999.
    nanoseconds: s64;
}

/// POSIX `clock_gettime(2)`: writes the named clock's reading into `out`.
clock_gettime :: (clock: s64, out: *Timespec) -> s64 #foreign libc "clock_gettime";

/// Nanoseconds since an unspecified origin, from a clock that never goes backwards.
///
/// The only correct thing to measure a duration with. Two readings subtract to a nanosecond count.
monotonic :: () -> s64 {
    return now(CLOCK_MONOTONIC);
}

/// Nanoseconds since the Unix epoch, from the wall clock.
///
/// Comparable between machines and across a reboot, and therefore what a *timestamp* wants — but it can
/// jump, so a duration computed from two of these can be negative. Use `monotonic` to measure.
wall :: () -> s64 {
    return now(CLOCK_REALTIME);
}

/// One clock's reading, as nanoseconds. Shared by the two above so the seconds-to-nanoseconds
/// conversion exists once.
now :: (clock: s64) -> s64 {
    t: Timespec;
    if clock_gettime(clock, *t) != 0 {
        return 0;
    }
    return t.seconds * NANOSECONDS_PER_SECOND + t.nanoseconds;
}
```

`clock_gettime` is scalars and one pointer to a plain struct of two `s64`s, which is exactly the FFI shape
that already worked before any of the graphics or concurrency waves — it is why `Time` was the first
module built in W7. `monotonic` and `wall` differ only in *which* clock id they hand to `now`, so a bug in
the seconds-to-nanoseconds arithmetic cannot exist in one and not the other.

**Use `monotonic` to measure a duration, and `wall` to timestamp a moment.** A game loop's delta time is
`monotonic() - previous_monotonic`; a save file's "last played" field is `wall()`. Mixing them up is a
live hazard: the wall clock can jump backwards when the system clock is corrected, and a frame-time
calculation built on it can go negative.

## Nanoseconds, exactly, and truncating on the way out

```jr
/// Nanoseconds in a second.
NANOSECONDS_PER_SECOND :: 1000000000;

/// Nanoseconds in a millisecond.
NANOSECONDS_PER_MILLISECOND :: 1000000;

/// Nanoseconds in a microsecond.
NANOSECONDS_PER_MICROSECOND :: 1000;

/// `nanoseconds` as whole milliseconds, truncating.
///
/// Truncating rather than rounding, and stated because the choice is invisible at the call site: a reader
/// comparing `to_milliseconds(x) == 5` wants to know whether 5_999_999 counts. It does not.
to_milliseconds :: (nanoseconds: s64) -> s64 {
    return nanoseconds / NANOSECONDS_PER_MILLISECOND;
}

/// `nanoseconds` as whole microseconds, truncating.
to_microseconds :: (nanoseconds: s64) -> s64 {
    return nanoseconds / NANOSECONDS_PER_MICROSECOND;
}

/// `nanoseconds` as whole seconds, truncating.
to_seconds :: (nanoseconds: s64) -> s64 {
    return nanoseconds / NANOSECONDS_PER_SECOND;
}

/// Whether `a` is a later reading than `b` on the same clock.
///
/// A named comparison rather than leaving callers to write `a > b`, because on the *wall* clock the
/// comparison is the thing that can surprise: a reading taken later can be numerically smaller.
is_after :: (a: s64, b: s64) -> bool {
    return a > b;
}
```

`s64` nanoseconds throughout is a deliberate refusal of two easier-looking designs. A `Duration` struct —
a seconds/nanos pair, which is what C's `timespec` already is — cannot be subtracted or compared without
a helper of its own, and its only advantage over a bare integer is range this project does not need.
Microseconds or milliseconds as the base unit is refused for the opposite reason: a coarser unit cannot
express what the finer one measures. And a `float64` of seconds loses nanosecond resolution somewhere in
the 2030s, which would make two runs of the same benchmark differ in their last digit for no reason a
reader could see.

`is_after` exists because on the *wall* clock a later reading can be numerically smaller — a clock
correction can step it backwards — so a named comparison is a place a reader can find every ordering
decision this module makes, rather than trusting every call site to write `a > b` and mean it.

## What the checksum proves

```jr
#import "Basic";
#import "Time";

main :: () {
    total := 0;

    first := monotonic();
    if first > 0 {
        total = total + 1;
    }

    // **Not strictly greater**: two readings can land in the same nanosecond on a fast machine, and a test
    // that demanded strict increase would fail rarely and unreproducibly — the worst kind of flake. What
    // the clock actually promises is that it never goes *backwards*.
    second := monotonic();
    if second >= first {
        total = total + 2;
    }

    // The wall clock is past the epoch by a lot. Asserted loosely for the same reason: a tighter bound
    // would be a date this file would eventually fail on.
    if wall() > NANOSECONDS_PER_SECOND {
        total = total + 4;
    }

    // The conversions, on a *constant* rather than a reading — which is what makes this part of the
    // checksum reproducible at all.
    one_and_a_half := 1500000000;
    if to_seconds(one_and_a_half) == 1 {
        if to_milliseconds(one_and_a_half) == 1500 {
            total = total + 8;
        }
    }

    // Truncation, stated in the module docs and asserted here because the choice is invisible at a call
    // site: a reader comparing `to_milliseconds(x) == 2` wants to know whether 1_999_999 counts.
    if to_milliseconds(1999999) == 1 {
        total = total + 16;
    }

    if is_after(second, first) == (second > first) {
        total = total + 32;
    }

    exit(total);
}
```

Every check here holds on *any* machine — that is the whole difficulty of testing a clock, since a reading
is not reproducible and nothing above compares one against a literal. What is asserted is the arithmetic
(`1_500_000_000` ns really is `1` s and `1500` ms) and the invariants (monotonic never goes backwards, the
wall clock is well past the epoch). The exit code is **63**, one bit per property.

## What is absent, and why

`Time` has <span class="jairs-status absent">absent</span> sleeping and <span class="jairs-status
absent">absent</span> formatting, and both are refusals rather than gaps still to fill.

There is no `nanosleep` binding because sleeping is a syscall that *blocks*, and a blocking call inside
the comptime VM would mean compilation that pauses — a decision about compile-time execution rather than
about time, and ADR-0121 gave the VM a step budget precisely so it cannot run away. A caller writing a
frame limiter in the VM has no route to it; a caller in a built binary can call `#foreign libc
"nanosleep"` directly, but this module does not offer the shortcut.

There is no `to_string`, no strftime, and no calendar of any kind. Rendering a timestamp needs leap
seconds, time zones and a locale, none of which this project has decided anything about — and a module
that offered a *wrong* rendering would be worse than one that offers none. A game's on-screen clock has to
be drawn some other way — as a bar or a count of pips built from raw nanoseconds, never as formatted text.

See also [Book I — The Jairs Language](/language/introduction/).
