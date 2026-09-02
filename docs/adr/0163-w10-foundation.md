# ADR-0163: W10's real path — SDL2 over Cocoa, and a library search path

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- **PLAN §8.5's correction.** That section names Cocoa and Metal, and ADR-0162 established that
  `objc_msgSend` cannot be called. This decides what W10 is built on instead, and delivers the one missing
  piece that decision needed.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Context

§8.5 lists W10's prerequisites as E0286, then FFI aggregates and C-variadics, then `File`. Three of those are
now done: ADR-0161 opened the aggregate boundary, `File` shipped in ADR-0157, and ADR-0162 established that a
C-variadic **call** is blocked upstream — Cranelift's `Signature` has no variadic boundary.

That last one is not a delay. It removes an option. §8.5's plan was "Cocoa via `#foreign`", and every Cocoa
call goes through `objc_msgSend`, which is variadic. So the wave as planned is not startable, and the question
is not *when* but *what instead*.

This ADR answers that by probing rather than arguing, and the probe changed the answer.

## Decision

### 1. W10 is built on SDL2's C API, not on Cocoa directly

**Proven, not proposed.** A Jairs program now opens a window, creates a renderer, sets a colour, clears the
surface, fills a rectangle through a `*SDL_Rect`, presents it, and tears everything down — six calls, all six
succeeding, under SDL2 2.0 with the dummy video driver. Every one of those is a plain C function taking scalars
and pointers.

That is the whole argument. SDL2 needs **no `objc_msgSend`**, so ADR-0162's blocker does not apply; it needs
**no aggregate by value**, because it passes rects and colours by pointer; and it is the shape this compiler
can already call, today, with no further language work.

**Rejected: Cocoa and Metal directly**, which is what §8.5 assumed. It is blocked on a variadic call whose
blocker is *in Cranelift*, not here — so it is not a matter of doing more work in this project. Keeping it as
the plan would make W10 permanently "next" behind an upstream dependency, which is the shape of estimate §5
exists to catch.

**Rejected: an Objective-C shim compiled at build time.** `objc_msgSend` could be wrapped in fixed-arity C
functions, which is what every non-Objective-C language binding does. It needs a C (or Objective-C) compiler
invoked *during* a Jairs build — `jr-link` already shells out to `cc`, so the machinery half exists — and it
makes the standard library carry compiled C, which is a decision about what "stdlib in Jairs" means (§0's
decision #5). Worth revisiting for W10's *later* items; wrong as the thing the wave rests on.

**Rejected: writing a windowing layer on the raw `objc_` runtime C API.** `objc_getClass`, `sel_registerName`
and `class_getMethodImplementation` are all fixed-arity, so a message *could* be sent by fetching the IMP and
calling it through a procedure pointer — except that an IMP's signature varies per selector, so each call needs
its own `#foreign` declaration cast, and `#foreign` procedure values are refused (E0256, ADR-0059 §5). Two
language features deep for a path SDL2 already covers.

**What this costs, stated plainly.** SDL2 is a third-party dependency, where §2.1 imagined system frameworks.
A Jairs program that draws will need SDL2 installed, and the graphics module will be a binding to somebody
else's library rather than to the platform. In exchange the wave becomes startable now, on a portable API that
also works on Linux — which the project's own §0 decision #6 says is a target and which Cocoa never was.

### 2. `--library-path` and `JR_LIBRARY_PATH`, because a `#system_library` never said *where*

The probe failed first, and the failure was small and exact: `ld: library 'SDL2' not found`. `-lSDL2` was on
the link line and there was nowhere to look. `-lc` had always resolved from the driver's own defaults, so no
program had ever needed a search path.

`LinkRequest` gains `library_paths`, `jr build` gains `-L`/`--library-path`, and `JR_LIBRARY_PATH` is read
after the flags. **The `-L`s are emitted before the `-l`s**, which `ld` requires: a search path affects only
the libraries that follow it, so the other order would look right and find nothing.

**Not a directive in the source**, and this is the load-bearing part. A path is a property of the *machine
compiling*, not of the program: a source file naming `/opt/homebrew/lib` is unbuildable on any machine that
puts its libraries elsewhere. That is the same asymmetry that makes `-o` outrank a declared `BUILD_OUTPUT`
(ADR-0102 §2) and that made ADR-0122 confine a declared output to the working directory — a *declared* name is
a value the artefact chose, and a path is an instruction from the operator.

**Rejected: hard-coding Homebrew's prefix.** It would work on this machine and on no CI runner, and it would
make the compiler wrong about a machine it cannot see. **Rejected: `pkg-config`.** It is the right answer for a
build system and this is not one; shelling out to a third tool to discover a path the operator already knows
adds a dependency to every link.

The flag comes **before** the environment variable, because a flag is the more specific instruction and `ld`
takes the first match — the same precedence `-o` has.

### 3. The test builds its own library rather than using SDL2

The gap was found with SDL2 and is tested with a one-function archive compiled by `cc` into a temporary
directory. Depending on a Homebrew package would make the suite unrunnable on a clean machine, and *whether a
particular library is installed* is not something this compiler owns; *whether `-L` reaches the link line* is.

**The negative half runs first and is the half that matters**: without the flag, the link must fail. A test
that only checked the success case would pass even if `-L` were ignored, because a driver that found the
library some other way would look identical. That is the same trap ADR-0055 recorded as "a test that passes
without the code it tests is worse than no test".

### 4. What W10 now is, and what it still is not

**Startable.** Window creation and a 2D drawing surface are demonstrated. §8.5's remaining items — image
decode, an immediate-mode UI, audio — are library work on top, and image decode wants `File`, which exists.

**Still absent: a GPU layer.** §2.1 names Metal then Vulkan. Metal is Objective-C, so it inherits §1's
refusal; Vulkan is a C API and would work the way SDL2 does, on a machine with a loader installed. SDL2's own
renderer covers 2D, which is what §8.5's first two items ask for, so the GPU question is deferred to whichever
item actually needs a shader — and it is now a *choice between two C APIs* rather than a blocked path.

## Consequences

- **W10 — Graphics is unblocked and startable**, on a different foundation than §8.5 planned, with the
  substitution argued.
- **`jr build -L DIR`** and `JR_LIBRARY_PATH` exist; `LinkRequest` carries the paths.
- **1055 tests**, one of them the library-path mechanism with its negative half.
- **`objc_msgSend` remains uncallable**, and that is now a *design input* rather than a task: Cocoa, Metal and
  any Objective-C API are out of reach until either Cranelift grows a variadic convention or this project
  decides to compile C shims.
- **§8.5 needs rewriting around SDL2**, which this ADR authorises and PLAN records.
