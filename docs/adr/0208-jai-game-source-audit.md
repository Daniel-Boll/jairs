# ADR-0208: Public Simp copies disagree, so the games book teaches a pinned shape rather than an “exact Jai API”

- **Status:** Accepted
- **Date:** 2026-09-06
- **Deciders:** dboll
- **Amends:** ADR-0187's unqualified “Jai's real API” wording; ADR-0207's claims that a clean
  `GL.error_code()` check proves every GL call succeeded and that the first games-book draft was
  source-accurate; current documentation that repeated ADR-0195's already-amended claim that a
  Jairs `#run` cannot read files or run commands through `modules/Compiler`.

## Context

ADR-0207 added a twelve-page games book and three examples, but the report on that work was immediate:
the pages read like a signature catalogue and an implementation postmortem rather than a tutorial.
They also made the strongest possible parity claim — that Jairs exposed Jai's exact or real `Simp`
API — while naming no version.

The requested audit read public game source rather than relying on remembered declarations. The
primary inventory is in `docs/research/jai-games-primary-sources.md`, pinned by commit, and the
Jairs-facing analysis is in `docs/jai-game-development-audit.md`.

The source changed the answer in three ways:

1. **Jai's observed Simp stack is not SDL-backed.** Its public vendored GL backends use WGL, GLX,
   NSOpenGL and EGL around native `Window_Creation`/`Input` modules.
2. **Jairs is not SDL-rendered either.** SDL2 supplies windows, events, context creation, buffer
   swapping and BMP surfaces; `modules/GL` and `modules/Simp` perform the rendering through OpenGL.
3. **There is no one public Simp signature set to call exact.** The inspected Focus, Hitboxer and
   Voronoi copies differ on render-target parameters, `immediate_begin`, font construction and text
   colour. They are snapshots or forks of a closed-beta distribution, so a public copy establishes
   only its own revision.

The local audit then found concrete false claims beyond the parity wording: imported library handles
were said to be unusable by a foreign declaration even though `Time` uses `Basic.libc`; event helpers
were said to drain beyond their limit; `texture_from` was said to free a surface it deliberately
leaves alive; a released button was said to draw active after `active` had been cleared; caller-owned
input was attributed to missing globals after ADR-0186 shipped them; and a clean GL error sample was
presented as proof of successful shaders and pixels.

## Decision

### 1. “Simp-shaped subset” is the compatibility claim

The games book, capability inventory and README describe Jairs as a **Simp-shaped subset**. They name
the architectural resemblance — immediate batches, colour/image shaders, render targets and the same
recognisable procedure family — without claiming one canonical closed-beta signature set.

`docs/research/jai-games-primary-sources.md` records the source revisions and their disagreements.
A future compatibility wave must choose and name the Jai beta or public snapshot it targets before
changing an API.

**Rejected: choose whichever public copy is newest and call it canonical.** Repository recency does
not establish that a vendored module is unmodified or that it matches the current private distribution.

**Rejected: remove every Jai comparison.** The shared shape is useful to a reader and was an explicit
goal of ADR-0187. The correction is to qualify the claim, not pretend there is no relationship.

### 2. The backend is described layer by layer

The one-sentence contract is:

> Jairs uses SDL2 for platform windows, events, GL-context plumbing and BMP surfaces, then OpenGL for
> rendering. The inspected Jai Simp stack uses native platform windows/context plumbing and OpenGL.

That sentence replaces both misleading shortcuts, “Simp uses SDL” and “Jairs is implementation-parity
with Jai”.

### 3. The twelve pages become a tutorial sequence

Every games page is rewritten around an outcome:

1. run Pong headlessly and graphically;
2. separate a rulebook from its platform driver;
3. open and close one window;
4. draw one frame;
5. choose variable or fixed-step timing;
6. carry one image through surface, upload, draw and teardown;
7. build one working button;
8. use the math needed by one bounce;
9. assemble Pong, Snake and the sprite demo;
10. finish with a compact capability matrix.

Long signature catalogues, SDL overlays and GL implementation details move to module source or short
“under the hood” notes. The book quotes the lines a reader changes and links to the full examples for
the rest.

**Rejected: retain the reference chapters and add a shorter tutorial in front.** That leaves two
sources describing the same API, the exact maintenance failure ADR-0207 was written to correct.

### 4. The examples and source comments are part of the audit

The examples lose the same false claims the prose did:

- `examples/games/build.jr` distinguishes a `main` script from a `#run` script without denying the
  driver-backed compile-time file/command surface.
- Pong calls caller-owned input a design choice, not a language limitation.
- Snake's aggregate replay check is evidence rather than proof of every trajectory.
- The sprite demo no longer wraps `draw_button` in an empty outer batch.
- A GL check is described as observing no queued error, not proving every operation or pixel.

The underlying `Input`, `UI` and `Compiler` module comments are corrected too. `UI`'s outline constant
uses the typed-constant syntax that has existed since ADR-0190 rather than documenting it as absent.

### 5. A smaller game facade is planned, not implemented

The condition for considering a separate facade is met. Public game projects manually assemble
window, input, timing, drawing and teardown; `dbechrd/jai-simpler` exists specifically as a wrapper
above Simp; community raylib bindings expose the alternative flat workflow. No built-in Jai
raylib-level facade was found.

`docs/jai-game-development-audit.md` therefore proposes a Jairs-native `Game` module implemented over
`Window`, `Input`, `Simp`, `Image`, `Time` and ordinary Jairs storage. No code is added here because
six design forks remain for the decider: explicit `App` versus singleton state, resource handles,
coordinate system, naming, loop policy and whether text/PNG gate the first release.

## Consequences

- The games book shrinks from 2917 lines to roughly 1150 while retaining the three complete examples.
- The compatibility claim is weaker in wording and stronger in evidence: every external source is
  commit-pinned, and disagreements remain visible.
- `docs/jai-game-development-audit.md` is the language/library backlog extracted from real games:
  computed array lengths, pointer iteration, `ifx`, inferred literals, procedure overloading,
  item-level `#if`, `#load`, module parameters, `Code` values, and the missing input/text/image/audio/
  math/resource layers.
- No facade API is frozen by this ADR. Implementing one before the forks are decided would violate the
  repository's “design forks before code” rule.
- Test and corpus counts do not move: **1216** workspace tests (**1222** under gate 7), **283** corpus
  files. The ADR count becomes **208**.
