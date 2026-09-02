# ADR-0165: The event loop ADR-0164 said was impossible — amending ADR-0164 §5

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- **Amends ADR-0164 §5**, which is wrong. ADRs are immutable, so this is a new one that says so, exactly as
  ADR-0018 §5 amended ADR-0017.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Context

### ADR-0164 §5 was wrong, and it was wrong for the one reason this project keeps writing down

That section said `modules/Window` could not have an event loop:

> `SDL_PollEvent` fills an `SDL_Event`: a **union** of every event type, 56 bytes. E0286 refuses a union at a
> `#foreign` boundary … The refusal is right.

The refusal *is* right. It is also **irrelevant**, because **E0286 refuses an aggregate crossing by value, and
`SDL_PollEvent` takes an `SDL_Event *`.** A pointer to a union is just a pointer — the same shape as the `*Rect`
the same module had already been passing successfully for the whole of ADR-0164.

`AGENTS.md` states the habit that would have caught this: *confirm a wave's premise by writing the thing before
planning around it.* ADR-0164 §5 planned around a premise it never wrote. It then compounded the error by
building a story on it — "four waves at one boundary", and a claim that settling this also settles the
Objective-C question — which was a *pattern* assembled out of one unverified fact.

**The correction cost one probe.** A 56-byte struct, a `*Event` argument, push a synthetic `SDL_QUIT`, poll it
back, read the type: four assertions, all four passing, no compiler change of any kind.

**So this is the seventh time the habit has paid** (ADR-0067 §0, ADR-0070 §0, ADR-0072 §5, ADR-0073 §0,
ADR-0075's closing claim, ADR-0140's dump, ADR-0141's coercion) — and the first time it paid by *contradicting
this project's own accepted ADR from the same session* rather than an assumption inherited from a plan.

## Decision

### 1. `Event` is a `#place` overlay, which is what `#place` is for

`#place` (ADR-0144) puts a field at an explicit byte offset, and ADR-0144's own corpus example overlays a
`s64` on two `u8`s. **Two fields at one offset is what a union is**, so a union is not a thing this language
lacks — it is a thing `#place` already expresses.

`key_sym` and `mouse_x` genuinely share offset 20; `window_event`, `key_state` and `mouse_button` share 12 and
16. Which field is meaningful is decided by `kind`, exactly as in C. The test asserts the sharing rather than
tolerating it: writing `mouse_x = 40` must make `key_sym` read 40, because if it does not, the overlay is not
an overlay.

**What is guaranteed, stated exactly, because an overlay is a claim about somebody else's ABI:**

- **Offset 0 is contractual.** SDL's header documents `Uint32 type` as *"Event type, shared with all events"*.
- **The others are SDL2 ABI offsets**, read from `offsetof` on this platform, stable across the 2.x series by
  the same promise that makes `SDL_Event` a fixed 56 bytes.
- **`layout_is_sdl2()` checks the size**, so a platform that disagrees fails a test rather than corrupting a
  stack.

**Rejected: a C shim exposing per-type accessors.** It is what ADR-0164 §5 assumed was necessary, and it needs
a C compiler invoked during a Jairs build plus a standard library that carries compiled C. Unnecessary for a
mechanism `#place` covers.

**Rejected: hiding the shared offsets behind per-event structs.** `Key_Event`, `Mouse_Event` and so on would
read better, and getting from an `Event` to one needs a pointer cast, which E0232 refuses on purpose (a general
pointer cast makes a wrong pointee a silent wrong read). `typed` (ADR-0106) would do it from a `*u8`, at the
cost of two conversions per event and an erasure at exactly the boundary where the type matters. One struct
whose overlaps are *documented* is more honest than several whose relationship is a comment.

**`tail: [28]u8 #place 28` is not decoration.** SDL writes the whole 56-byte union through the pointer. A
smaller struct is a stack overwrite that surfaces as a corrupted unrelated local — the worst class of bug this
project has, and one no verifier catches because the write is a legitimate write to a legitimate address.

### 2. Fields are widened to `s64`, never constants narrowed — and there is no typed-constant syntax

Every comparison casts the *field* up: `cast(s64, event.kind) == QUIT`. A `#place` overlay's fields are C
widths and a Jairs constant is `s64`, and **there is no syntax for a typed constant** — probed: `QUIT : u32 :
256` does not parse.

Widening a `u32` to `s64` is lossless in every case, so this direction cannot be wrong. Narrowing the constant
could be. The asymmetry is worth stating because the alternative reads better and is the one a tired author
picks.

**A typed constant is now an owed language item.** It is not invented here: this module wants nine of them and
works without any, so the gap is real but not blocking, which is exactly the kind of thing this project records
rather than opportunistically fixes mid-wave.

### 3. `wants_to_close(limit)` drains, because SDL does not promise one-push-one-poll

**Found by writing it, and it is the useful half of this wave.** A test that polled exactly once per push
**passed on the first push and failed on the second**. `SDL_PollEvent` pumps device state as a side effect, and
the pump can deliver on a later call, so a single poll returning nothing does not mean the queue is empty.

A caller who polls once per frame and acts on the result silently misses events. `wants_to_close` drains
everything and reports whether any of it asked to stop, so the queue is empty when it returns and the next
frame starts clean. `limit` bounds the work, because mouse motion during a drag genuinely produces events
faster than a frame drains them.

**Not `#must`**, and neither is `next_event`: `false` is the ordinary answer on almost every frame, and
ADR-0151's marker is for a failure a caller would otherwise miss — "nothing happened yet" is not one.

### 4. A synthetic keyboard event is not testable through SDL's queue, and that is SDL's business

`SDL_PushEvent` returns **success** for a fabricated `KEY_DOWN` and SDL then drops it. Found by instrumenting —
push returned 1, the poll returned nothing — rather than assumed either way.

So the keyboard assertions build an `Event` locally and read it back through `pressed`, testing the overlay's
offsets and the auto-repeat filter. That is the part this project owns. Whether SDL delivers a fabricated
keypress is not, and a test that asserted it would be testing SDL's filter policy.

**`pressed` checks `key_repeat`**, because a held key produces a stream of `KEY_DOWN`s and a caller asking "was
this pressed" almost never wants forty of them.

### 5. `should_close` accepts both `QUIT` and a window-close

A user clicking the close box of the only window means the same thing as Cmd-Q, and a caller who checked only
`QUIT` would ship a window that refuses to shut. Both, therefore — and a caller who needs to distinguish them
still reads `kind` directly.

## Consequences

- **`modules/Window` has a working event loop.** A window opened by this library can be closed by the user,
  which ADR-0164 said it could not.
- **ADR-0164 §5 is superseded in full**, including its "four waves at one boundary" reading and its claim that
  this fork also settles the Objective-C question. Those were built on the unverified premise. **The
  Objective-C question is untouched and still open** — `objc_msgSend` is variadic, which is a genuinely
  different blocker (ADR-0162), and it was never the same fork.
- **1058 tests**, 253 corpus files. No corpus file, for ADR-0164 §6's reason: the VM cannot reach SDL2.
- **A typed constant is owed** as a language item, with nine callers in this one module.
- **`size_of` of an imported struct is not reachable from a file-scope constant** (E0230). `Socket` hit it and
  worked around it; this module hit it and the workaround is recorded in both places. Also owed.
- **The habit is now seven for seven**, and its most valuable catch was against this project's own accepted
  ADR from the same session. An ADR is evidence of a decision, not evidence of a fact.
