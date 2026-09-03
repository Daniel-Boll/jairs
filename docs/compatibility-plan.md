# Remaining Jai-compatibility features — a plan grounded in probes, not in memory

**Status:** **Wave A is BUILT** (ADR-0183, ADR-0184, commit `e622f40`). Waves B, C and D remain proposals.

> [!IMPORTANT]
> **Wave A is delivered, and building it corrected §0's own correction.** This document already led with "the
> correction that reorders everything" — that OpenGL is unreachable because a per-OS library *name* is
> circular. That cycle is real and it is **second in line**. Two commands, which cost less than reading this
> paragraph:
>
> ```
> $ cc probe.c -o probe -lOpenGL           ld: library 'OpenGL' not found   (exit 1)
> $ cc probe.c -o probe -framework OpenGL                                   (exit 0)
> ```
>
> `jr-link`'s whole flag vocabulary was `-L` and `-l`, so **a perfect name mechanism would have emitted a name
> that does not link.** The first blocker was a missing *argument form* — smaller and far more tractable than
> what §0 describes. A2 was listed here as the *second* item of Wave A; it was the first.
>
> **A1 and A2 are both built, and A3 is done for the case that motivated it.** `modules/GL` exists and links,
> `modules/File`'s hedged `O_*` flags select per OS, and E0294 records the one shape a computed operand cannot
> generate. `Socket`'s constants and `Thread`'s pthread sizes are the remaining A3 work and **need no compiler
> change** — they are ordinary module edits now.
>
> **What A1 turned out to cost, versus what §A1 estimated.** §A1 guessed "one arm in the dispatcher plus
> lowering", and the arm was right. What it missed is that `checked_expanded` reused the **unexpanded**
> signatures under a comment reading *"because `#insert` adds no items"* — true only while an insert could not
> add declarations. A generated procedure therefore had no signature and the failure blamed its *caller* with
> an internal compiler error. Three further sites had to learn "a file insert is pending". **An estimate that
> counts the code to add and not the assumptions to break is the estimate this project keeps getting wrong.**
**Measured against:** `main` at the merge of ADR-0179–0182 (1073 tests, 262 corpus files, 182 ADRs).

---

## 0. Read this first: the correction that reorders everything

The Simp programme's plan (ADR-0179–0182) made **six** claims about platform support that did not survive
contact. Five are recorded in PLAN §7. The sixth is new, found while answering "where are the per-OS gates and
OpenGL?", and it is the one that reorders this document:

> **The plan said a per-OS library *name* is unreachable because of a query-order cycle.** Library resolution
> happens inside `file_signatures`, `file_consts` depends on `file_signatures`, so a *computed*
> `#system_library` operand cannot be evaluated before the library must be known.

That cycle is real — `#system_library NAME` is E0100, verified — **and it is not on the path.** Two probes:

```
$ cc probe.c -o probe -lOpenGL          → ld: library 'OpenGL' not found      (exit 1)
$ cc probe.c -o probe -framework OpenGL →                                     (exit 0)
```

```jai
// runs today, exits 6 on macOS — a #run that reads os() and emits source text
pick :: () -> string {
    if os() == Operating_System.MACOS { return "n := 6;"; }
    return "n := 1;";
}
main :: () { #insert #run pick(); exit(n); }
```

So:

1. **The first OpenGL blocker is not the library name at all.** OpenGL on macOS is a *framework*, and
   `jr-link`'s entire flag vocabulary is `-L` and `-l` (`crates/jr-link/src/lib.rs:138-143`). Even a perfect
   per-OS name emits `-lOpenGL` and fails. **One missing link form**, not a language feature.
2. **Comptime OS-driven code generation already works.** A `#run` reading `os()` can produce source text and
   `#insert` splices it. What it cannot do is produce a **declaration**: `#insert` is not one of the four arms
   of the file-scope directive dispatcher (`crates/jr-syntax/src/parser.rs:762-780`), so at file scope it is
   E0101, a stray token.
3. **Item-level `#insert` dissolves the cycle rather than breaking it.** Generated text contains a *literal*,
   so `foreign_library_of` sees exactly the bare `STRING_LITERAL` it demands. Nothing has to be evaluated out
   of order, because nothing is computed at the point of use.

**That is the whole reorientation.** The per-OS story is not "add `#if` to the compiler". It is **one parser arm
plus one linker flag**, after which per-OS support is *library code written in Jairs and run at compile time* —
which is what Jai actually does, and what this language's comptime machinery was built for.

### Where the library stands today, measured

`os()` shipped in ADR-0180. The entire standard library uses it **once**:

```
$ grep -rn "Operating_System" modules/ | grep -v modules/Basic
modules/Time/module.jr:57:    if os() == Operating_System.MACOS {
modules/Time/module.jr:60:    if os() == Operating_System.LINUX {
```

Every `#system_library` in 22 modules is `"c"` or `"SDL2"` — both of which are one name on all three targets,
which is *why* the graphics stack needed no gating and got none. That is a defensible choice for a renderer and
a poor state for a standard library: `File`'s `O_*` flags, `Socket`'s constants and `Thread`'s pthread sizes are
all hedged with comments admitting they are one platform's numbers.

---

## 1. ~~Wave A~~ — the comptime platform layer — **BUILT** (ADR-0183, ADR-0184)

**Everything else in this document is cheaper after this wave, and three later items become library edits
rather than compiler work.** It is two compiler changes and then Jairs code.

### ~~A1~~ — `#insert` at file scope, producing declarations — **BUILT** (ADR-0184)

The one capability gap. `#insert` works in a body (ADR-0072), takes a computed operand (ADR-0073), and is
already driven by `os()` — probed above. It is absent from the file-scope dispatcher only.

```jai
// modules/GL — what this unlocks, written in Jairs
gl_library :: () -> string {
    if os() == Operating_System.MACOS  { return "gl :: #system_library \"OpenGL\" #framework;"; }
    if os() == Operating_System.LINUX  { return "gl :: #system_library \"GL\";"; }
    return "gl :: #system_library \"opengl32\";";
}
#insert #run gl_library();
```

> [!NOTE]
> **What shipped differs from this sketch in one place, and the difference is a decision.** A framework is
> `#framework "OpenGL"` — a **separate directive** — not `#system_library "OpenGL" #framework`, a modifier on
> the library form. ADR-0183 §1 argues it: a framework is a different kind of linkable thing rather than a
> style of naming one, and interning the form *into* the library value makes `#system_library "X"` and
> `#framework "X"` two different values, so a program that wrote one meaning the other cannot silently be
> handed the other's `PoolId`. The modifier spelling would have made that collision representable.
>
> The four forks in the table below were settled as recommended, all four. See `modules/GL/module.jr` for the
> shipped version of this sketch.

**Design forks this wave must settle, each with a real cost:**

| Fork | Options | Recommendation |
|---|---|---|
| **When does the splice happen?** | Before name resolution (generated names are ordinary declarations) vs. after (generated names are second-class) | **Before.** The existing body-level insert already expands before resolution and ADR-0073 built the operand pre-pass for exactly this ordering. |
| **Can generated text contain another `#insert`?** | Yes, bounded by ADR-0073 §3's depth budget vs. no | **Yes, reuse the budget.** A separate rule for items would be a second thing to keep in step, and the budget already exists because a computed operand can quine. |
| **Does a generated declaration get a real span?** | The directive's span (ADR-0072 §2's rule) vs. a synthetic file | **The directive's**, unchanged. ADR-0072 §2 records that rewriting spans afterwards missed `Expr::Name`'s own field; the override is the only version that cannot be incomplete. |
| **`#if` sugar on top?** | Add `#if` as sugar for the common case vs. `#insert` only | **`#insert` only, this wave.** A feature designed by an inconvenience is the wrong feature (ADR-0167 §4). Ship the mechanism, then see whether the library actually wants sugar. |

**Risk, named:** this changes the *item tree* during lowering, which ADR-0072 §5 deferred deliberately. The
item-count invariant nested-item hoisting relies on (ADR-0134: "no other item is allocated between now and the
drain") is the thing most likely to break, and it is asserted rather than merely assumed — so it will fail
loudly rather than miscompile.

### ~~A2~~ — `jr-link` learns the framework form — **BUILT** (ADR-0183), and it was the *first* blocker, not the second

`-framework NAME`, macOS-only, from a `#system_library "X" #framework` marker. Two lines in the flag loop and
one attribute in the parser.

**Why a marker rather than inference:** the compiler cannot know whether `"OpenGL"` means a framework or a
dylib, and guessing by platform would make `#system_library "SDL2"` on macOS try `-framework SDL2` first. The
declaration says which, and A1 means the *declaration itself* is per-OS generated — so no source file carries a
flag that is wrong on another platform, which is ADR-0163 §2's rule about `-L`.

**Also close the sibling hole this exposes:** `jr-link` has no full-path form either, so a library outside the
`-L` search path is unreachable. Out of scope here; recorded so it is not rediscovered.

### A3 — Then use it: stop the standard library hedging — **partly done**, and the rest needs no compiler change

Pure Jairs after A1/A2, one module at a time, each a corpus file that asserts the *host's* numbers:

- ~~**`modules/File`** — the `O_*` flags~~ **DONE** (ADR-0184 §6). `CREATE`, `TRUNCATE` and `APPEND` select
  per OS through `#run` and `os()`. The corpus program that uses them exits **124 before and after**, which is
  the measurement that matters for a change like this: the mechanism changed and the behaviour did not.
  Notably it did **not** need a generated *declaration* at all — a per-OS **value** (ADR-0181) was enough, and
  this section had assumed otherwise. A per-OS number is cheaper than a per-OS declaration, so check which one
  a case actually wants before reaching for the bigger tool.
- **`modules/Socket`** — `AF_INET`, `SOCK_STREAM` and the `sockaddr_in` layout differ; `sockaddr_in` has a
  `sin_len` byte on macOS that Linux does not have, so the struct is genuinely a different shape.
- **`modules/Thread`** — ADR-0177 chose a spin lock because `pthread_mutex_t` is "64 opaque platform-sized
  bytes and this language cannot spell that without hard-coding N per platform". **A1 removes that
  objection** — but check the cheaper route first, as `modules/File` turned out to need: a per-OS **constant**
  is a `#run` (ADR-0181) and needs no insert, and a size is a number. Whether to then swap the spin lock for a
  real mutex is a separate decision with its own tradeoffs.

**Both remaining items are ordinary module edits now.** Neither is blocked, and neither needs a compiler
change — which is the whole point of Wave A having landed.

---

## 2. Wave B — Simp on OpenGL, which is what Jai's Simp actually is

Only reachable after Wave A. Jai's `Simp` has a **GL backend**; this project's has an SDL2 renderer backend,
which was the right call when a per-OS library name was believed unreachable and is now a choice rather than a
constraint.

- **B1 — `modules/GL`**: the bindings, with the library name generated per OS (A1) and linked per OS (A2).
  Loading is the interesting part: on Windows every function past 1.1 needs `wglGetProcAddress`, so this is not
  a flat binding list. `SDL_GL_GetProcAddress` is the portable answer and keeps SDL2 as the platform layer,
  which is exactly the division Jai draws.
- **B2 — a second `Simp` backend behind the unchanged API**. ADR-0182 promised this is "a later swap behind an
  unchanged API"; that promise is testable, and the test is that `Simp`'s six integration tests pass against
  both backends unchanged.
- **B3 — shaders**, which is what a GL backend buys that `SDL_RenderGeometry` cannot: ADR-0163 §4 deferred the
  GPU question until "whichever item needs a shader", and this is it.

**Honest cost:** two renderers that must agree, and the differential harness does not cover them (it compares
the *engines*, not two library backends). That needs a decision about what "agree" means for pixels.

---

## 3. Wave C — the language gaps this programme found and did not fix

Each was found by building, and each is named where it bit. Ordered by value per unit of work.

| # | Gap | Evidence | Cost |
|---|---|---|---|
| C1 | **A typed constant.** `QUIT : u32 : 256` does not parse; nor does `OUTLINE_THICKNESS : float32 : 1.0`. Every constant crossing a C boundary is `cast` at the call site — ~a dozen sites in `Window`, `Image`, `UI`. | ADR-0165 §5 owed it; ADR-0182 added five more call sites. | **Small.** A parser rule and a type on `ConstValue`. Highest value per unit of work in this document. |
| C2 | **A file-scope mutable variable.** E0245 with an honest trapping stub (ADR-0178). | Shaped `Simp` and `Input` around its absence (ADR-0182 §1). | **Medium-large.** A `.data`/`.bss` section, static initialisation, three engines. No longer urgent: the caller-owned-struct shape is better. |
| C3 | **`"literal".data`** does not lower — *"a memory reference has no place"* — while a local bound to the literal works. | Cost one confused build in ADR-0182. Pre-existing tests already used the workaround. | **Small.** One lowering arm. A one-line surprise for whoever writes the obvious thing. |
| C4 | **`#c_variadic` calls** are E0289, so `printf` and `objc_msgSend` are unreachable — which is why Cocoa and Metal are out. | ADR-0162 §2: Cranelift's `Signature` has no variadic boundary. | **Blocked upstream**, not by effort. Track cranelift; the VM (libffi) and LLVM both *can* do it, so a partial answer would make `jr build` and `jr run` disagree — which ADR-0162 refused for good reason. |
| C5 | **Aggregates by value at a `#foreign` boundary** work for `Class::Integer`/`Float` (ADR-0160/0161); `Class::Memory` is E0286. A union by value is refused outright. | ADR-0164 §5 hit it, ADR-0165 routed around it. | **Medium.** The case has two correct answers (System V and AAPCS64 disagree), which is why it was split rather than guessed. |
| C6 | **Cross-file `$T` instantiation** is E0268; the workaround is a wrapper the declaring module writes. | ADR-0104 §2. `modules/Sort` is shaped by it. | **Medium.** Needs instantiation to cross a module boundary, which touches the query graph. |
| C7 | **Qualified-import leftovers**: `using p: Window.Point` promotes nothing; a bare alias is `unresolved name`. | ADR-0179's own consequences, asserted as boundaries. | **Small**, and both are arguably correct as-is. |

---

## 4. Wave D — the compiler as a host, which is what "Windows support" really means

Currently **source-portable and unrun**, and one of these is not merely untested:

- **D1 — `jr-vm` cannot be built for Windows.** `crates/jr-vm/src/ffi.rs:69` is
  `use libloading::os::unix::Library`. This is a hard compile error on a Windows host, not a runtime gap.
- **D2 — the link step assumes a Unix `cc`.** `jr-link` shells out to the first of `cc`/`clang`/`gcc` and
  emits Unix flags. Whether `clang` on Windows resolves `-lSDL2` to `SDL2.lib` is **unverified** and is the
  first thing to check, because it decides whether D2 is a small change or a real one.
- **D3 — no CI has ever run**, on any platform. PLAN §1.5 records it. Everything above is verified on macOS
  arm64 *locally*, which means the Linux claims in this document are all inference.

**D3 is the honest first move for this wave**, because per-OS code that no CI runs is per-OS code nobody has
tested on the other OS — and Wave A's whole output is per-OS code.

---

## 5. What is deliberately not here

- **Item-level `#if` as a compiler feature.** Wave A gets the same result from comptime code, which is the
  Jai-native answer and reuses machinery that exists. Sugar can follow the mechanism if the library wants it.
- **Cocoa and Metal.** Blocked on C4, which is blocked upstream.
- **Fonts and text in `Simp`.** Needs `SDL_ttf` (a second library's version skew) or a glyph table as data —
  a scope decision, not a capability gap.
- **A retained-widget-tree UI.** ADR-0166 rejected it with reasons that still hold.

---

## 6. Recommended order, and why

1. ~~**A1 + A2**~~ — **DONE** (ADR-0183, ADR-0184). One parser arm and one linker argument form, in that
   order of *difficulty* and the reverse of the order this list put them in: A2 was the first blocker. `File`'s
   half of A3 came with it.
2. **C1** (typed constants) — **now the top item**, and its case grew rather than shrank: `modules/GL`'s
   constants are all untyped, so every one crossing a C boundary is `cast` at the call site. Smallest item with
   real daily value.
3. **A3's remainder** — `Socket`'s constants and `Thread`'s pthread sizes. Ordinary module edits now, not
   compiler work.
4. **D3** (CI) — before trusting any of A3's Linux numbers. **Its priority rose**: `modules/GL` and `File`'s
   flags both now contain Linux and Windows branches that **no machine has ever executed**, so the library's
   per-OS claims are untested rather than merely unverified. This is the first work in this project whose
   correctness genuinely depends on a second platform running it.
5. **B** (OpenGL) — the largest, and the one with a real unanswered question about what "two backends agree"
   means. Unblocked now: the library links.

**One habit to carry:** this document's own §0 exists because the plan it corrects was written from memory —
**and §0 was itself wrong**, corrected by two `cc` invocations at the top of this file. The score for probing
before planning is now **fifteen for fifteen**, and its two most valuable catches were both against
conclusions written minutes earlier in the same session. Every claim above that could be probed, was, and the
probe commands are in §0 so the next reader can re-run them rather than trust them.

**A second habit this wave adds: check whether the cheaper mechanism suffices.** §A3 assumed `modules/File`
needed a generated *declaration*; it needed a per-OS **value**, which already existed (ADR-0181). The plan
reached for the tool it was building rather than the one already there.
