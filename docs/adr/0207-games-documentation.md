# ADR-0207: The documentation site had no games, and its absence inventory was sixty ADRs stale

- **Status:** Accepted
- **Date:** 2026-09-06
- **Deciders:** dboll
- **Amends:** the `docs-site/` claim that the text "names the wave that adds it", which is true of no
  page and cannot be true of anything now that all twelve waves are closed; `docs/jai-parity.md`'s
  probed rows for array literals, `type_of` and typed constants, and its §2 item 6, all of which
  expired; `examples/README.md`'s statement that no drawing program is included.

## Context

The report was one sentence: *the doc site is not updated with new changes — for instance there is
nothing about `Simp`*. Both halves were true, and the second is the more serious one, because Jairs
exists to be ergonomic for writing games the way Jai is, and the six modules that claim is made of —
`Window`, `Input`, `Simp`, `GL`, `Image`, `UI` — appeared in **no page of the documentation site at
all**. Nor did `Time`, `File`, `JSON`, `Process`, `Socket`, `Thread`, `Compiler` or `Bucket_Array`:
sixteen of twenty-four standard library modules were undocumented.

An audit against `AGENTS.md`, `docs/adr/README.md` and the module sources found **47 verified-false
statements**, concentrated where they do the most harm: `language/whats-absent.md` listed `#must`,
array literals, float printing, run-time reflection, DWARF, `#simd` and `#soa` as absent and the LLVM
back end as a later wave — every one of them shipped. The site's absence inventory was written at
roughly ADR-0140 and the project is at ADR-0206.

**The pattern behind that number matters more than the number.** A page that says a feature is absent
is read as a commitment that nobody has built it, so a reader plans around it and a maintainer files
work for it. This project has been bitten by exactly this three times before — ADR-0125 found the
README's "Absent" column listing three shipped features, ADR-0168 found three stale `[NOT DELIVERED]`
markers in `PLAN.md`, and ADR-0205 found a hand-maintained library claim that had been wrong on Linux
for waves. A documentation site is the largest hand-maintained claim in the repository.

## Decision

### 1. A fourth book, rather than chapters folded into the existing three

`docs-site/` gains **Book IV — Games with Jairs**, a twelve-page group under
`src/content/docs/games/`: an overview, project layout, the window and the event loop, drawing with
`Simp`, the game loop, textures and images, immediate-mode UI, maths for games, three walkthroughs, and
an inventory of what a game cannot do yet.

**Rejected: adding graphics chapters to Book I.** Its `sidebar.order` range 1–20 is contiguous and full,
so six new chapters would renumber the whole book — but that is the small objection. The real one is
that Book I is a language tour and every graphics chapter is a *library* chapter that additionally
requires a third-party dependency, a link flag, a display, and a build mode. Mixing that into the
narrative tour would make the tour's prerequisites conditional halfway through.

**Rejected: one long page.** The stack has six modules and two coordinate conventions that cannot be
mixed in one render target; a reader who needs the UI's obligation must not have to find it inside a
chapter about quads.

### 2. Three games, in `examples/games/`, and the split they exist to demonstrate

Pong, Snake, and a sprite-and-widget demo. Each is built and run; the two games are **multi-module**,
with their rules in a module of their own — which is what the report asked for ("by example of some
simple games using jai multi-module even").

**The structural decision the examples are for:** a game's rules go in a module that imports no
graphics module. `modules/Pong` imports only `modules/Math` and `modules/Snake` only `modules/Random`,
so both run in the **compile-time VM** — `jr run examples/games/pong/sim.jr` plays a whole match with no
display attached, and `snake/sim.jr` asserts that one seed replays identically. Everything that cannot
be tested that way is confined to `main.jr`.

That is not a stylistic preference. A drawing program cannot run under `jr run` at all: the comptime VM
resolves a foreign symbol from the compiler's own process image, so it reaches libc and nothing else
(ADR-0158 §3). The boundary already exists; the only question is which side a game's rules sit on, and
putting them on the testable side costs nothing.

**Each drawing program reads a frame budget from the environment** (`PONG_FRAMES`, `SNAKE_FRAMES`,
`SPRITES_FRAMES`) and stops when it is reached, and each checks `GL.error_code()` every frame and exits
74 if a call failed. A game that can only be checked by looking at it cannot be checked.

**Rejected: shipping a `.bmp` asset.** `modules/Image` reads BMP and nothing else, and a binary blob in
the repository is a file no reader can review. The sprite sheet is built in memory with
`Image.create_surface` and four `Image.fill_surface` calls instead, which also documents the surface API
and the ABGR8888 packing that would otherwise silently swap red and blue.

**Rejected: a fourth, larger game.** Breakout or a platformer would exercise no module these three do
not, and every additional program is a maintenance obligation whose cost is paid at every future
library change.

### 3. Absent and refused are different, and the site now says which

`docs-site/src/styles/jairs.css` has defined three status badges since the site was written and only one
had ever been used: `absent`. So a by-design refusal — cross-file `$T` instantiation, `cast(Perm, 3)`,
`Point.{1, 2}`, a `Code` value, an item-level `#if` — read as a gap somebody was going to fill.

Every page now distinguishes <span>absent</span> (not built yet) from <span>refused</span> (the
compiler rejects it, or the design declined it), and the games inventory additionally names the
**blocker** where a gap is blocked rather than merely unbuilt: Cocoa on `#c_variadic` and Cranelift's
signature model, a generic string-keyed table on E0268, text-bearing widgets on fonts, JSON
serialisation on a correct `dtoa`, a per-thread trap backtrace on thread-local storage in both back
ends. A gap with a named blocker is schedulable; a gap without one is a wish.

### 4. What Jai has and Jairs does not is documented rather than omitted

`games/not-implemented.md` is the games-facing view of `docs/jai-parity.md`, organised by what a game
developer goes looking for — text and fonts, audio, input devices, widgets, images, maths, collision,
entity storage, language features, platform, build — rather than by compiler subsystem. Each row names
Jai's feature with its real name where a primary source establishes one, says what a Jairs game writes
instead today, and carries the badge that fits.

This is what the report asked for in its second paragraph, and the maintenance rule is stated on the
page: a row is deleted when the feature ships.

### 5. The documentation found a compiler defect, and it is fixed here rather than recorded

Writing up Book III's note-serialiser page meant running its program, which nothing had done: it
**checked clean and then crashed the compiler in both engines** — `internal compiler error: edge to
block 2 supplies 2 arguments for 1 parameters` under `jr run`, and a Cranelift verifier failure under
`jr build`. The shape is ordinary: `#insert noted_insert("task", …)` generating a call, in a body that
also calls `print("%", total)`.

**The root cause is the third instance of a shape this repository has recorded twice and warned about
both times.** A computed `#insert` renumbers every expression id after its splice, and `file_consts`
records its tables against the *unexpanded* tree. ADR-0101 §3 found that in `folded_calls`, cleared
that one map, and wrote *"a stale entry the expanded check does not replace is exactly the wrong value
at a live id"*. ADR-0188 §1 hit the same thing one map over — a constant's value keyed by `ItemId` —
and its comment ends by saying every map keyed by that identity is suspect.

Six maps are keyed that way. **One was being cleared.** So a stale variadic record sat at an id the
splice had moved, lowering packed trailing arguments for a call that takes none, and the block argument
counts stopped matching. Clearing that record alone then exposed the second half: the *fresh* records
were never threaded either, so the `any_of` coercion at the live id had no lowering and the failure
became `expected an aggregate, found a scalar`.

**Fixed by clearing the whole body scope and re-recording it through the one function `file_consts`
itself uses** — `record_checked_folds`, which ADR-0196 §6 created for exactly this reason ("two paths
populating one `ConstValues` differently is the defect under all of this"). Per-key clearing cannot
work: the caller does not know which ids the expanded check will record, only which scope moved.
`ConstValues::clear_body_scope` is therefore scope-wide and clears all six maps, so the next map added
there is covered without anybody remembering.

**Why nothing caught it.** `tests/corpus/valid/082-note-driven-codegen.jr`'s tagged procedures contain
only a `return`. The corpus had a splice and it had variadics, and no program had both in one body.
`tests/corpus/valid/150-splice-beside-a-variadic.jr` is that program, and it asserts the printed line as
well as the exit status, because the printed line is the half that was broken.

**The MIR snapshot gained 91 lines and lost none**, which is the evidence that the fix changed no
existing program's lowering: every other corpus program either has no computed `#insert` or records
nothing in those maps.

## Consequences

- **The corpus gains one file and the test count does not move**: 1216 tests (1222 under gate 7), 283
  corpus files. The regression program is iterated by harnesses that already exist, which is the pattern
  every wave whose deliverable a `.jr` program can observe follows.
- **A documentation wave changed the compiler**, which is worth noting for what it says about coverage:
  the defect in §5 was reachable by an ordinary program and had been unreachable only because no program
  in the tree wrote that shape. Writing prose about a feature meant running it, and running it is what
  found it. Three of this project's library modules were written the same way and found four compiler
  defects between them; this is the first time documentation did.
- **The four ADRs whose features were documented as absent are now documented as shipped**, and
  `docs/jai-parity.md`'s own probed table carries a paragraph saying that three of its rows expired
  between being written and being re-read. That document's caveat already said so in general; it now
  says so about itself.
- **A fifth instance of the stale-claim shape is now on record**, and the interesting thing about this
  one is its size: 47 false statements is not an oversight, it is what happens when a hand-maintained
  document has no gate. Nothing here changes that — the site has no test — so the honest mitigation is
  the one `docs-site/README.md` now carries: never paste a signature from memory, and treat a page
  documenting a signature the compiler does not have as worse than a page that omits it.
- **`build/` is a new ignored directory**, written by `examples/games/build.jr`'s
  `Build_Options.output_path`.
- **Visual verification was not achieved and is recorded as not achieved.** The three drawing programs
  build, link, run, and pass a per-frame `GL.error_code()` check; nobody has compared their pixels to a
  reference image, because an SDL window opens on a different macOS Space from a fullscreen terminal and
  a screenshot could not be taken from the session that wrote them. Every page that describes a game
  says exactly that. A screenshot-based check is owed and is a real piece of work: `modules/GL` binds no
  `glReadPixels`, so the readback path does not exist yet either.
- **The graphics stack's own documentation has a known rot** that this wave records rather than fixes:
  `modules/GL`'s module doc still claims "OpenGL 1.1 only. Every entry point below is in the 1.1 core",
  and the file binds twenty post-1.1 symbols beneath that sentence — GL 2.0 shaders, GL 1.5 buffers,
  `glActiveTexture`. The site documents the real surface and does not reproduce the claim. Correcting
  the module's own header is owed.
- **Two smaller gaps in `modules/GL` are now written down** because a games book had to explain the
  failure modes: only `glDeleteTextures` is bound, so a shader, program or buffer created through the
  module cannot be released through it; and `glGetProgramInfoLog` is not bound at all, so
  `program_linked` can report *that* a link failed and nothing can report *why*.
