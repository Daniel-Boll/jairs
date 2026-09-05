# ADR-0202: A standard library that installs, a project manifest, and a third off `jr build`

- **Status:** Accepted
- **Date:** 2026-09-05
- **Deciders:** dboll
- **Amends:** ADR-0029 §1 (which rejected `jairs.toml` as a *prerequisite* and left it open as an
  override — this is that override), ADR-0014 §1 (the bundled tier is now synthetic rather than a
  build-machine path)

## Context

Five asks, one wave: make the compiler easy to install globally, stop requiring `-I modules`, add
`jr new`, make the formatter configurable, and take some cost out of the CLI.

The first two turned out to be one problem. `bundled_module_dir()` returned
`env!("CARGO_MANIFEST_DIR")/../../modules` — the path of the machine that *built* the compiler,
baked into the binary. Correct exactly as long as `jr` runs from its own source tree, and its own
doc comment said so: *"installing `jr` will need a real installation-relative lookup."*

## 1. The standard library is compiled into the binary

**`cargo install` cannot install data files.** Cargo's documentation is explicit: only packages with
executable `[[bin]]` or `[[example]]` targets can be installed, and every executable goes to the
installation root's `bin` directory. There is no data-file mechanism, so a `jr` installed the
ordinary way has exactly one file to its name.

That reduces the options to four, and three lose:

**An executable-relative search**, Zig's approach for its own source standard library. Zig's
implementation is worth copying in one respect — it validates each candidate directory by opening a
marker file, `std/std.zig`, rather than trusting a derived path. Rejected anyway, because embedding
does not have to ask the question: there is no directory to find, so no candidate to validate and no
way for the search to come back empty. It also divides badly across platforms. `current_exe` returns
the path *as invoked* on macOS (`_NSGetExecutablePath`) and the *fully resolved* target on Linux
(`readlink /proc/self/exe`), so one symlinked install resolves to two different directories depending
on the host — and on Linux it fails outright where `/proc` is not mounted.

**Extraction to a user data directory on first run.** Rejected: it makes every first invocation a
filesystem write, needs a version stamp to know when to re-extract, and puts the compiler's
correctness at the mercy of a directory the user can edit.

**A tarball or an installer**, as rustup ships. The right answer at scale and the one to revisit when
the standard library outgrows a binary. Rejected now because it gives up the property that matters
most today: `cargo install --path crates/jr-cli` is the entire installation procedure.

**A build-script-generated `include_str!` table wins, and not `include_dir`.** Measured, not assumed:

| | |
|---|---|
| Standard library | 24 modules, 483,923 bytes |
| Binary delta from embedding | +476,336 bytes |
| Generated table vs `include_dir` | **927,664** vs 929,200 bytes |
| Raw / zip-deflate / zstd-19 | 483,923 / 170,452 / 130,156 |

`include_dir` also stores `&[u8]` and re-validates UTF-8 on **every** access behind an `Option` every
caller must then answer for, where the table gives `&'static str` with neither cost — and it costs
`proc-macro2`, `quote` and `syn` to do it. **A compressed archive is `ty`'s shape** for its vendored
typeshed and is right there at 7.0 MB across 752 files; here it would save 318 KB in exchange for a
zip dependency, a decompression step and a virtual filesystem between the compiler and its own
standard library.

### The build script is load-bearing, not hygiene

`include_str!` registers a rustc dependency, so editing a module's *contents* rebuilds. **Adding or
removing** a module does not, and the failure is silent: the new module is simply absent from the
binary, and the compiler reports "module not found" on a program that is correct.
`include_dir`'s answer is `proc_macro::tracked_path`, which is nightly-only and a **no-op on
stable** — so it does not solve the problem this project has. `cargo::rerun-if-changed` on the
directory does, and needs a build script.

**Jairs-specific and mandatory:** `modules/` is at the workspace root, *outside* `crates/jr-stdlib/`,
so Cargo's default "scan the whole package directory" fallback does not cover it. `ty_vendored` gets
away with emitting no directive only because its `vendor/typeshed` is inside its own package.

### The seam is one function, and the tier is a synthetic path

Module resolution funnels through `jr_db::Db::read_module_file`, which both the existence probe and
the load go through — so the bundled library is reachable by answering there for paths under one
synthetic root, `<bundled>`. A synthetic path rather than a separate lookup tier is deliberate: it
keeps `module_file`'s first-hit-wins ordering, its list of searched candidates, and the E0210
diagnostic that prints them, working with no new concept. The root is spelled with angle brackets so
that no directory a user creates can shadow the bundled library or be shadowed by it, and so
`<bundled>/Nope/module.jr` in a diagnostic is unmistakably not somewhere to go and look.

**`load_module` duplicated `read_module_file`'s body**, which meant the probe and the load could in
principle disagree about whether a module is there — and would have had to learn about a third source
twice. Collapsed to one reader.

**`in_memory_modules` stays exclusive rather than becoming a fallback.** A test supplying an in-memory
module set is asserting that no filesystem access happens, and falling through on a miss would
quietly break that. The bundled tier does not conflict with it: it answers only for `<bundled>`,
which no filesystem can produce, so no path could reach both.

## 2. `jairs.toml`, as an override and never a prerequisite

ADR-0029 §1 rejected this file when it was proposed as a *prerequisite* for workspace discovery, and
that reasoning still holds. It also left the door open on exact terms worth quoting, because they are
this section's specification: a manifest "would need a fallback to exactly the rule above, at which
point **the rule is doing the work and the manifest is an optional override**."

So: every command works with no manifest. A missing file is not an error and not a warning. And an
explicit flag outranks the file, because a flag is an instruction and a file is a default — the same
asymmetry ADR-0102 §2 drew between `-O` and a declared `BUILD_OPT_LEVEL`.

Precedence is stated **once**, in `jr-cli`'s `project` module, because five of seven subcommands build
a module search path and three take an entry point. Inline answers per command would be six chances to
disagree, and precedence is exactly what a user infers from one command's behaviour and assumes
generalises.

### Unknown keys are an error

`deny_unknown_fields`. A configuration file whose typos are ignored is the worst of both worlds:
`indent_size = 2` beside a formatter that indents by four looks like a formatter bug, and nothing
distinguishes a key that does nothing from one that is not read yet.

That decision is also why **`max_width` is absent** rather than offered. It was declared in
`jr_fmt::Config`, defaulted to 100, and **never copied into the `Formatter`** — dead from the day it
was written. Offering it in a manifest would have been a setting that appears to work.

`indent_style` and `indent_width` mirror EditorConfig's vocabulary rather than inventing one. Their
interaction is documented in three places — the field, the crate docs and the scaffolded file —
because `indent_width` genuinely is not read under `indent_style = "tab"`: a formatter emitting a tab
does not decide how wide it looks.

### Tabs were impossible, and there was exactly one place to fix

`indent_str()` hard-coded `" ".repeat(..)` and was the **only** site that emitted indentation, called
only by `emit_indent`. So the whole feature is one enum and one `match`. It also allocated a fresh
`String` per output line, which the fix removes: the unit is resolved once and pushed `indent` times.

### The scaffold is tested against itself

Two tests that look like belt and braces and are not. The scaffolded manifest **must parse** — a
scaffold writing a file the tool then refuses is worse than no scaffold. And every *commented-out*
key in it must be a key the manifest accepts, because the reader's next move is to uncomment one.
A third asserts the scaffolded `.gitignore` covers the artefact the scaffolded manifest produces:
those are written by different functions from the same name, and ignoring `/demo` while building
`src/main` would be silently useless.

## 3. `[project] name` names the artefact, as a fallback and not an override

`jr build` wrote `src/main`, because the driver's default is the root file's stem. The manifest's name
belongs in that chain, but **not at the top**: `-o` is the operator's instruction and a declared
`BUILD_OUTPUT` is what the source chose, so the order is `-o` → `BUILD_OUTPUT` → project name → file
stem.

Implemented as a new `BuildRequest.default_output` field, consulted only at the last step. The
alternative — having the CLI pass `output` when it finds a manifest — would have made the manifest
outrank a declared `BUILD_OUTPUT`, which inverts the asymmetry the rest of this ADR rests on. The
field also keeps `jr-driver` manifest-ignorant, which it should be: a manifest is a command-line
concept.

Adding the field made `script.rs`'s `BuildRequest` literal a compile error — the house exhaustiveness
rule working as designed. `None` there is the correct value and not a placeholder: that site sets
`output: Some(confined)` unconditionally, so the fallback is unreachable by construction.

## 4. A third off `jr build`, and two measurements that closed off the alternatives

Every millisecond of `jr build` on a trivial program was accounted for before anything was changed.
Two results made most of the obvious work unnecessary:

**Startup has no recoverable overhead.** `jr --version` is 8.16 ms; an *empty* Rust binary on the same
machine is 8.24 ms, against 5.89 ms for `/usr/bin/true`. Nothing is eager before dispatch — no logging
init, no tracing subscriber, no panic hook — and the release profile was already `lto = "thin"`,
`codegen-units = 1`, `panic = "abort"`.

**The link is dominated by a tool we do not own.** `cc <obj> -o <out>` alone is 85 ms of the 135 ms
link step. No `lld` is installed here, so a faster-linker option could not be measured, let alone
recommended.

What was left was real, and it was all process spawns:

| | before | after |
|---|---|---|
| `cc --version`, spawned **only to test that `cc` exists** | 40 ms | gone |
| `codesign --verify`, when `ld64` has already signed the output | 16 ms | gone |
| `is_build_script`'s throwaway database, re-reading and re-parsing the root file | a few ms | gone |
| **`jr build`, trivial program** | **170.8 ms** | **114.0 ms** |

**Those two numbers are a controlled A/B and the first draft's were not.** The before figure was
originally measured early in the session and the after figure an hour later, which gave 178.6 → 106.7
and an inviting "40%". Re-measured properly — the parent commit built into a `git worktree`, both
binaries benchmarked in one `hyperfine` run with 10 warmups and 40 runs each — it is **170.8 → 114.0
ms, a third off**. The *recovered* milliseconds were right (56.8 measured against 56 estimated); the
percentage was inflated by a baseline that drifted 8 ms between two separate measurements. **A
before-and-after taken at two moments is not a measurement of a change**, and this one was caught only
because the audit re-ran it rather than trusting the number already written down.

**The driver probe is gone, and the distinction that replaced it is the whole correctness content.**
`Command::output()` can only return `Err` before the child ever ran, so a spawn failure unambiguously
means "try the next driver". A driver that *ran* and exited non-zero is a real link error and returns
immediately — falling through would replace a precise "undefined symbol `foo`" with a bogus "no C
driver found".

**The signature check is now in-process**: `LC_CODE_SIGNATURE` read via `object`, which costs a file
read instead of 16 ms of process spawn. An unparseable output falls through to signing rather than
being treated as signed, because failing to parse is not evidence.

**`strip = true`** saves 1.29 MB, which more than pays for the 476 KB the embedded standard library
adds: the release binary is **6.3 MB**, down from 6.8 MB before this wave.

## 5. A setting with two surfaces is half-wired until both read it — twice

Two subcommands were left reading the bundled directory directly instead of going through the one
resolver, and both were found by auditing *which call sites go through it* rather than by any
failure.

**`jr lsp` was the one that mattered.** In a project declaring `[build] module_paths = ["vendor"]`,
`jr check` resolved `#import "Vend"` and the **editor** reported `E0210: module Vend not found` on
the same file. That reads as the code being wrong rather than the tool, which is the worst way for a
configuration bug to present. `jr bench` had the same gap with a smaller consequence: a benchmark
taken against a different search path than the real build measures something nobody runs.

**This is the second instance of one shape in this ADR.** §2 already records the formatter half —
`jr fmt` honoured `jairs.toml`'s style while the server ignored it. Same server, same manifest, same
two surfaces, and the module-path half was missed *while fixing the style half*. So the rule is
worth stating flatly rather than as a note: **when a setting has two surfaces, wiring one is half a
feature, and the half that ships is usually the one nobody uses.** The mechanical form of the check
is cheap — grep for the direct call and confirm the resolver is its only caller — and it is now
true: `bundled_module_dir()` has exactly one call site.

**The regression test was verified by reverting the fix**, not by passing. Without it the assertion
fails with the E0210 quoted above; with it, nothing is published. A test that passes without the
code it tests is worse than no test (ADR-0055), and a search-path bug is exactly the kind that hides
behind a test which resolved the module for an unrelated reason — hence the negative twin, which
asserts an *undeclared* module is still reported.

## Consequences

`cargo install --path crates/jr-cli` produces a working compiler — verified by installing it, copying
the binary away from the tree, and compiling a program that imports `Basic` with no `-I`.

`jr new hello` produces a project that runs on the first try, and `jr build` / `jr run` / `jr check` /
`jr fmt` need no arguments inside one, from any subdirectory.

**Every existing invocation is unchanged**, which is asserted rather than assumed: an explicit path
with no manifest anywhere still checks, still formats, and still indents by four.

### Owed

- **`max_width` is not implemented**, only removed from the surface. Line wrapping is a real feature
  and a wave of its own; what this ADR fixes is that it was advertised.
- **`run_script` still does its own read** of the root file. `is_build_script` no longer duplicates it,
  but the script path reads once more than it needs to — a few milliseconds on a command that spends
  85 of them in `cc`.
- **No faster linker is offered.** `lld` is not installed on this machine, so `-fuse-ld=lld` could not
  be measured, and an unmeasured optimisation is not one.
