# ADR-0210: `Game.App` owns the first playable-loop foundation

- **Status:** Accepted
- **Date:** 2026-09-06
- **Deciders:** dboll
- **Amends:** ADR-0208's deliberately unresolved `Game` proposal by deciding its six forks and
  authorising the first implementation slice.

## Context

ADR-0208 established that real game programs repeatedly compose `Window`, `Input`, `Simp`, `Image`
and `Time`, and that a thin Jairs-native facade is justified. It intentionally stopped before code:
state ownership, resource identity, coordinates, naming, loop policy and the first-release threshold
were all expensive choices.

The existing Pong, Snake and sprite programs make the first repeated cluster concrete:

1. start SDL;
2. create a window;
3. create and verify the Simp render target;
4. drain one frame of events;
5. compute elapsed time from a monotonic clock;
6. present the frame;
7. destroy the render target before the window, then stop SDL.

Deleting a `Game` module would put that ordering and its failure cleanup back into every caller. That
is the depth the module earns: a small interface hides a lifecycle whose mistakes are use-after-free,
resource leaks or a silently absent renderer.

## Decision

### 1. The facade has an explicit `App`

Callers own an `App` value and pass `*App` to the interface. The first slice exposes:

```text
open(width, height, title) -> (App, bool) #must
close(*App)
begin_frame(*App) -> bool
end_frame(*App)
should_close(*App) -> bool
delta_time(*App) -> float64
window(*App) -> *Window.Window
```

`App` owns its window and its caller-sized `Input.Events` buffer. Copying an open `App` is
unsupported: Jairs has no move or linear-ownership type with which to forbid it.

**Rejected: a raylib-like module singleton.** Calls are shorter, but lifetime and cleanup become
implicit, tests share state by construction, and a hidden global becomes the permanent public model.

Simp itself still has one process-wide GL context and batch. `Game` therefore permits at most one
open `App` at a time and enforces that invariant privately. An explicit value remains useful even
when the substrate is singular: ownership and cleanup are visible, and the interface need not remain
singleton-shaped when the renderer eventually grows another context.

The App's entire lifecycle stays on the thread that calls `open`. The private one-App guard is
deliberately non-atomic, and the underlying OpenGL context is thread-bound.

### 2. The foundation owns lifecycle, close state and frame timing

`open` takes `width, height, title`, preserving `Window.create_window`'s order rather than the
illustrative title-first spelling in the audit. It starts SDL, creates a top-left/y-down Simp target,
checks `Simp.is_ready`, and unwinds every completed step on failure.

`begin_frame` drains the event queue exactly once, latches any close request, and records an
unclamped non-negative delta in seconds from `Time.monotonic`. It returns `false` once closing has
been requested. `end_frame` presents with vsync. `close` is idempotent and tears down in dependency
order: Simp target, window, SDL.

`Game` does not install or replace `context.allocator`; this slice allocates nothing. It does not
treat Escape as close until the input-state slice owns a complete named key table.

**Rejected: Game-owned fixed ticks or delta clamping.** Pong clamps a long frame while Snake runs a
fixed-step accumulator; either policy inside the facade would make one ordinary game fight the
other. The module reports elapsed time and the caller chooses simulation semantics.

### 3. Coordinates and names are Jairs-native

The facade uses top-left, y-down coordinates by selecting `Simp.LEFT_HANDED` once during `open`.
Input, `UI`, image editors and raylib use that convention, so mouse positions and drawing agree
without per-call conversion.

Public names use Jairs `snake_case`: `open`, `begin_frame`, `draw_texture`, not raylib's PascalCase.

**Rejected: Simp's bottom-left, y-up default.** It is convenient for the existing Pong and Snake
worlds, but every UI and mouse caller must convert.

**Rejected: raylib-compatible naming.** The module is not a raylib binding, so matching its spelling
would imply a compatibility promise the implementation does not make.

### 4. Resources will be owned through generation-tagged handles

The resource slice will return an integer texture handle containing an index and generation. `App`
will own the registry and automatic teardown; stale handles will be rejected rather than silently
referring to a newly allocated GPU object.

**Rejected: expose raw `Simp.Texture` as the game-facing identity.** It is the thinnest wrapper, but
it leaves ownership, stale copies and cleanup order with every caller.

This decision does not add the registry to the foundation slice. The public identity is decided now
so later storage can change without changing callers.

### 5. Implementation lands in slices; “game-ready v1” waits for PNG and text

Foundation, input, primitives and BMP-backed resource ownership may land independently. The facade
is not advertised as game-ready v1, and the beginner tutorial does not cut over, until PNG and text
exist beneath it.

**Rejected: call a BMP-only, no-text facade v1.** Its first ordinary project immediately needs asset
conversion and cannot label a button.

**Rejected: block every facade change on PNG and text.** That couples reviewable lifecycle and input
work to two separate substrate projects.

The earlier source inventory also listed sound in a broad raylib-like wish list. ADR-0208's later,
approved boundary omitted audio and no audio substrate exists. Sound is therefore deferred rather
than smuggled into this first release threshold.

### 6. Struct-field opacity is a convention, not a false guarantee

Jairs can hide declarations with `#scope_module`, but cannot make selected fields of a public struct
private. `App` therefore remains caller-owned and its fields are technically reachable. Callers use
the interface and the explicit `window` escape hatch; directly closing the embedded window or
destroying Simp invalidates the `App`.

**Rejected: allocate an opaque `App` handle solely to enforce field privacy.** It would force
allocation and another ownership protocol into a slice whose implementation otherwise allocates
nothing. The type system cannot enforce the convention yet, so the interface states it honestly.

## First slice and verification

This ADR authorises only the foundation:

- `modules/Game/module.jr`;
- lifecycle, close-event draining, monotonic delta time and presentation;
- one successful real-driver integration test, including a synthetic close event and idempotent
  cleanup;
- one dummy-driver failure test proving a missing GL context unwinds cleanly;
- no drawing helpers, held-input table, textures, PNG, text or audio.

The SDL/OpenGL checks belong in `jr-cli` integration tests, not the corpus: the comptime VM cannot
resolve SDL2. They join nextest's serial graphics group because Simp and SDL hold process-global
state.

## Consequences

- A caller can open, frame and close through one small interface without reproducing teardown order.
- Only one `Game.App` may be open; this is enforced rather than left as a renderer-corruption trap.
- Delta time is seconds, zero before the first frame, non-negative, and never clamped by the facade.
- The module count becomes 25.
- Later slices remain: input snapshots/key table, primitives, generation-tagged resources/sprites,
  PNG and text, then tutorial cutover.
