# ADR-0164: `Window` — a window and a 2D surface, and the union that closes it

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- **PLAN §8.5 items 1 and 2**, on the foundation ADR-0163 chose. The first W10 wave.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Context

ADR-0163 established that SDL2's C API is reachable and Cocoa's is not, and proved it with six raw calls.
This wave turns that probe into a module: `modules/Window`, exercising ten steps in a compiled binary.

Everything worked. The interesting result is not that it worked — the probe had already shown that — but
**what the wave found it could not do**, and that it is the same refusal three earlier waves hit.

## Decision

### 1. Opaque C handles are named one-field structs

`SDL_Window` and `SDL_Renderer` are opaque in C; a caller only ever holds a pointer. `Window` and `Renderer`
wrap one `*u8` each, for the reason `File` wraps a descriptor (ADR-0157 §1): a handle and a count are both
machine words, and the compiler will not confuse them once one has a name.

The *raw* `#foreign` bindings take `*u8`. **Rejected: giving them the wrapper type.** A `#foreign` signature
naming a Jairs struct SDL2 never dereferences would be describing a layout the library does not have — and it
would work, silently, which is worse: it makes the declaration a claim about SDL2's internals that happens to
be unread.

### 2. `Rect` is four `s32`s, and there is a constructor

C's `int`, on both supported targets. **Not `s64`**, and this is the field-width trap ADR-0157 §2 hit from the
other side: SDL reads sixteen bytes through the pointer, so a wider field puts the second rectangle's data in
the first's height — a wrong *drawing*, with no diagnostic anywhere.

`rect(x, y, w, h)` takes `s64`s and narrows, because the four `cast(s32, …)`es are noise at every call site and
because narrowing is exactly the place a caller forgets one.

**A rect crosses as a `*Rect`**, which is why SDL2 was reachable before Cocoa at all: ADR-0161's
aggregate-by-value work is not needed here, because SDL's own API passes pointers.

### 3. Every failable routine is `#must`, and `present` is not

`start`, `open`, `renderer_for`, `set_color`, `clear`, `fill`, `outline` and `line` all carry `#must`
(ADR-0151): each can fail for a reason the caller can act on, and each failure is otherwise invisible until a
later call produces nothing. A null window read as "not yet" rather than "never" is how a draw loop spins
forever.

**`present` and `stop` are deliberately not `#must`.** `SDL_RenderPresent` and `SDL_Quit` return `void`, so
there is no failure to report, and inventing a `bool` would be a lie about what the library says. That is the
same line `File` drew at `close` — the marker describes what the *callee knows*, not what a caller might like.

**`close` and `destroy` are safe on an already-closed handle**, and null it. That is what lets a caller close
on every path without tracking whether they got one — the property that makes `File.close` usable where a
`defer` would go (ADR-0157 §3).

### 4. `delay` lives here and not in `Time`

`Time` deliberately has no sleep (ADR-0155 §1: a blocking call in the comptime VM means compilation that
pauses). This module **cannot run in the VM at all**, so the objection does not apply, and a drawing program
must yield the CPU between frames. Putting it here keeps `Time`'s invariant intact instead of carving an
exception into it.

### 5. No event loop, because `SDL_Event` is a union — the refusal's fourth appearance

**This is the wave's real finding.** A window opened here cannot be closed by clicking its close box, and that
is the honest headline of the module's state.

`SDL_PollEvent` fills an `SDL_Event`: a **union** of every event type, 56 bytes. E0286 refuses a union at a
`#foreign` boundary, and ADR-0160 §3 explains why in a sentence this wave cannot argue with — a union's
members overlap, so every C ABI treats its bytes as opaque and there is no register classification to
implement. The refusal is right.

So the shape recurs: ADR-0157 hit it with `stat`, ADR-0158 with `sockaddr`, ADR-0161 opened it for *structs*
specifically, and here it is for a union. **Four waves, one boundary.** Reading an `SDL_Event` needs either a
per-type accessor written in C — which is the Objective-C-shim question ADR-0163 §1 deferred, arriving from a
second direction — or a `#place`-based overlay whose byte offsets this module hard-codes per SDL version.

**Rejected: hard-coding the offsets now.** `event.type` is at 0 and reading just that would give a close-box
check in four lines. It also silently breaks on any SDL2 point release that reorders a member, and it would
put a layout assumption in a *library* where the compiler has spent sixteen ADRs keeping layout in the pool.
Recorded as owed rather than done, which is the same call ADR-0158 made about `sockaddr`'s nested pointers.

### 6. The test is native-only, and skips rather than fails

`tests/corpus/valid/` asserts the VM and native engines agree, and **the VM cannot reach SDL2**: it resolves a
foreign symbol from the compiler's own process image, and `jr` is not linked against SDL2. So this is a
`jr-cli` integration test, exactly as `Process` was and `Socket` was not (ADR-0158 §3) — the boundary is what
the VM can call.

It searches four directories for the library and returns if none has it, because ADR-0163 §1 accepted SDL2 as
this foundation's stated cost. **The skip is narrow on purpose**: it checks for the library, and every
assertion after that point is unconditional. `HIDDEN` and `SDL_VIDEODRIVER=dummy` make it pass with no
display; `SOFTWARE` rather than `ACCELERATED`, because acceleration *fails* on a machine with no GPU driver
and the default is the surprising one.

Ten steps, one bit each, so a failure names which step broke.

## Consequences

- **PLAN §8.5 items 1 and 2 are done**: a window is created and a 2D surface is drawn on, in all the engines
  that can reach the library (native Cranelift and LLVM; the VM cannot, by construction).
- **1056 tests**, 253 corpus files — no corpus file, because no `.jr` program in `valid/` can exercise this.
- **`modules/Window` is the seventeenth module** and the first that a `jr run` cannot execute. That is a new
  category and it is worth naming: the standard library now has a member whose tests are native-only.
- **An event loop is owed**, and its blocker is a union at an FFI boundary — the fourth wave to meet that
  boundary and the first for which E0286 is not merely correct but *load-bearing on a design decision this
  project has twice deferred*. Whichever way it is settled — C shims or `#place` overlays — settles the
  Objective-C question too.
