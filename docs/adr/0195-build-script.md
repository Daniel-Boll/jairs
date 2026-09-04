# ADR-0195: A build script written in Jairs

- **Status:** Accepted
- **Date:** 2026-09-04
- **Deciders:** dboll

## Context

Several Jai projects carry a `build.jai` and are built with `jai build.jai`: the language builds
itself, and a makefile is not in the picture. The ask was that Jairs have this too.

[`docs/build-script-plan.md`](../build-script-plan.md) is the research. Jai's own `modules/Compiler`
is unpublished — an authenticated code search for nine of its declarations finds only a repository
whose README says it is a reimplementation — so the API was reconstructed from **23 real `build.jai`
files** whose call sites agree, plus one committed transcript of the compiler printing
`Build_Options` itself.

That research produced one finding that decided the whole design, and it is not the one the plan
expected to matter.

## §1 The driver becomes a value, because a build script needs it called twice

`jr build` was a `main`-shaped function: 147 lines and 25 top-level statements in `jr-cli`, every one
reading a field of `BuildArgs`. That is adequate for one compilation per process and impossible for
two — and a build script needs exactly two, because the script is compiled and run, and *then* the
targets it asked for are compiled.

So a compilation is a `jr_driver::BuildRequest`: data, constructible by a `clap` parser or by a
running script. `crates/jr-driver` had been a one-line doc comment promising "compilation
orchestration: workspaces, the compiler message queue, and build metaprograms" since the slice, and
it is now the crate that does it.

**The driver does not print.** `BuildOutcome` carries diagnostics and the caller renders them. Two
callers want different things from one failure — `jr build` renders to a terminal at the operator's
colour setting, and the script driver wants to say *which target* failed first — and a crate that
owns the rendering can serve only the first. Passing a renderer in was rejected: it makes every caller
supply a terminal concept to ask a question about a file, and the script driver has no terminal.

**Flag precedence stays in `jr-cli`.** `-o` beating a declared `BUILD_OUTPUT` and `-O` beating
`BUILD_OPT_LEVEL` (ADR-0102 §2) is a statement about a command line, which this crate cannot see. The
request carries the *decided* values, so a reader never has to reconstruct which source won.

**Moving `confined_output` dropped its seven unit tests, and the workspace count caught it**: 1082 →
1081 while the wave *added* six tests. Every one of the seven guards a real escape ADR-0122 found —
`.git/hooks/pre-commit`, a leading `-`, a NUL byte — so losing them silently would have been the worst
possible seven to lose. This is what §7's "track the test count" is for, and it is the second time in
two sessions that the number has found something.

## §2 The script is a program, not a `#run` — and Jai's own model could not work here

Jai puts the build script at compile time, in a `#run`. Copying that shape does not work, and the
reason is measured rather than argued: **compile-time code may call no `#foreign` procedure**
(`crates/jr-vm/src/interp.rs`, ADR-0006), so a `#run` cannot read a file, shell out, print, or even
allocate — `Basic.malloc` is itself `#foreign`. A build script that cannot open a file is not a build
script.

> **Two of those five are wrong, and ADR-0196 corrects them.** "Print" and "even allocate" were false:
> `ffi.rs` has served `malloc` from the VM's **own linear region** since ADR-0061 and `write` from its
> capture buffer, so neither reaches a host — the refusal was keyed on the `#foreign` *declaration*
> rather than on whether foreign code is actually called. A `#run` can allocate and print now, and can
> declare a build.
>
> "Read a file" and "shell out" stand: those do reach a host, and `#foreign_at_comptime` is what they
> need. So the *conclusion* of this section survives — a build script wanting `git rev-parse` has to be
> a program — but the reasoning was broader than the facts, and the decider caught it by asking whether
> allocation really needs a foreign library. It does not, and it does not in Jai either: Jai's
> compile-time `context.allocator` is an ordinary Jai module, and no allocator is compiler-provided.
>
> An ADR is immutable, so the correction is recorded here rather than by editing the claim.

`#foreign_at_comptime` is what would change that. `PLAN.md`'s locked decisions call it "non-negotiable
given build scripts must read files"; it has thirteen mentions in the repository and not one line of
code. This design **sidesteps** it rather than pretending to solve it, which also means it stays owed
for its own reason — a `#run` that reads a schema and generates code — rather than being half-built
here.

**And the deeper finding: Jai's build power is not in the `Compiler` module at all.** Across the
23-script corpus, what the scripts actually *do* is clone a C dependency, stamp a git hash into a
binary, build a `.dmg`, format a bootable disk image, run a shader compiler. Every one of those is
`Process`, `File`, `String` — the ordinary standard library. A plan that ports the compiler API and
leaves the script unable to open a file has copied the wrong half.

So the script runs as an **ordinary Jairs program in the bytecode VM**, where this library already
works. Verified before the design was accepted: a VM-hosted program writes and reads files, joins
paths, and reports the OS.

**Three consequences that fall out rather than being designed:**

- **No poll, and no salsa instability.** The script runs to completion *before* any target compiles,
  so nothing observes a compilation halfway through and ADR-0153 §1's objection does not apply.
- **No text protocol.** ADR-0102 §3 rejected "the driver running the script as a separate program that
  prints a manifest" — on the *protocol*, explicitly, not on the two phases. An in-process call has no
  protocol.
- **`do_output = false` disappears.** The single most-written line in real Jai build scripts (20 of 23)
  exists to stop the build file becoming a junk binary. Here the script is never a target, so a whole
  class of mistake is unavailable.

ADR-0154 §4 named what a revisit would need: *"a compilation unit that is a **value** — created,
configured and built by a `#run` — which is a very different thing from a poll."* A `Target` handle is
that value, with the `#run` replaced by a program for §2's reason.

## §3 `#compiler_library`, and why the dispatch keys on a kind rather than a name

A script's `Compiler.set_output` is declared `#foreign compiler "set_output"`, and the VM forwards the
call to the driver. Reusing the `#foreign` declaration form is what made this cheap: **no grammar
rule, no HIR node, no MIR variant, and no change to either native back end.** A build script is not
something you compile, so a library that cannot be linked is exactly the right shape.

`compiler :: #compiler_library;` declares it. A separate directive rather than
`#system_library "compiler"`, for two reasons:

1. That spelling would emit `-lcompiler`, so the source would say something false.
2. **A name is forgeable.** Keying the VM's dispatch on the string `"compiler"` would hand the
   driver's vocabulary to any program that declared a library with that name. `jr_pool::LinkKind`
   gained a `Compiler` variant instead, and it can only come from the one directive that produces it.
   `ForeignProc` carries the kind beside the name — which is also ADR-0018 §4's prediction that a
   third consumer of a `#foreign` declaration would want the answer interned rather than re-derived.

Probed: `#system_library "compiler"` plus a `#foreign compiler "create_target"` takes the **C** route
and is refused by the library loader. The host is never reached.

Adding the variant made exactly **one** site a compile error — the driver's translation to `jr-link`'s
vocabulary — which is where the decision belongs, and the answer is that a `Compiler` library
contributes no linker argument at all.

## §4 The boundary is scalars and strings, and `Build_Options` is a library type

`Build_Options` is declared in `modules/Compiler`, in Jairs, and `set_options` decomposes it into one
boundary call per field. Passing the struct was rejected: it would put field offsets, `[]string` views
and the layout fold on the compiler's side, so **the compiler would have to know the shape of a
library type** — and adding an option would mean editing Rust.

That is ADR-0009's layout seam applied to a library struct: keep the narrow thing narrow, and let
Jairs handle Jairs' own aggregates. The `Host` trait therefore has one method and three value kinds
(`Int`, `Str`, and `Void`/`Int`/`Str` back), and an implementor never touches a `Value` or an
`Address`.

**A unit test asserts the two default lists agree**, by reading `modules/Compiler`'s source. A script
that reads the options and writes them straight back must not *change* the build, and the two lists
are written in different languages with nothing checking one against the other.

## §5 Shelling out: the call site knows what no type can

A build script must be able to run `git rev-parse`. `modules/Process` cannot do it under `jr run`: the
VM translates a pointer argument one level deep, so `execvp`'s `argv` — an array of pointers — arrives
with a real address for the array and region-relative garbage for every string in it (ADR-0158 §3).
Measured this session: **exit code 127 while reporting success.**

Fixing that *in the VM* needs information no **type** carries. `char **` is `argv` here and `strtod`'s
out-parameter there, and the second one **works** — the callee writes a pointer rather than reading
one. A rule keyed on "the pointee contains a pointer" would break working code in order to describe
broken code, which is the trade `modules/Process` already recorded and rejected.

A `view`-style intrinsic that built a host-side C array was considered and rejected on cost: it would
touch sema, `ConstValues`, MIR **and** `file_consts`' unenforced early-out list, which AGENTS.md
records biting four times for four distinct reasons.

So the knowledge stays where it actually is — at the call site, which says "run this list" — and the
**driver** spawns with ordinary Rust strings. `Compiler.command`, `argument_of`, `run` and `output`,
plus a one-line `shell`. Nothing to marshal.

This leaves the VM's marshalling defect standing for a general program, which is honest: it is a
separate fix with a separate argument, and a build script does not have to wait for it.

Two details that are decisions rather than details. **Standard error is inherited, not captured** — it
is the channel a tool uses to explain itself, and capturing it would make a build script responsible
for printing a compiler's error message, which most would forget. And **a signal is not exit code 0**:
`ExitStatus::code()` is `None` for a killed process, and `unwrap_or(0)` would report a crashed tool as
a success, which is how a build "succeeds" having produced nothing.

## §6 Detection, and the boundary of a cheap check

`jr build build.jr` runs the script with no flag, because **importing `modules/Compiler` is what makes
a file a build script**. Detecting on that fact rather than on a shape — "declares `build` and no
`main`" was the alternative — means the detection and the refusal for compiling one as a program
cannot disagree about what a build script is.

`is_build_script` reads one file's **own** import list, which needs `file_hir` and no module loading at
all. So an ordinary build pays one parse to learn it is not a script, rather than a second module tree.

**A file reaching the module indirectly is not detected**, and gets the refusal naming `--script`. That
is the stated boundary of the cheap check, and paying for a transitive walk on every ordinary build
would be the wrong trade. The refusal test now pins exactly that case, because detection made its
original construct unreachable.

## §7 What running it found that reviewing it did not

Four defects, none visible in review:

1. **The default output was `file.with_extension("")`**, so `add_file("/tmp/p/main.jr")` was refused as
   "an absolute path" — confinement blamed for a default the driver had chosen. It is the source's
   *basename* now, in `output_path` or the working directory: a script's artefact belongs to the
   project rather than beside whichever file happened to be a root, and the script is code the operator
   may not have written, so the confinement has to be able to pass for the ordinary case.
2. **Compiling a script gave a wall of linker output** — `Undefined symbols: _add_file, referenced from
   _jr$2$17`, naming the compiler's own mangling instead of the operator's actual mistake. Now a
   diagnostic naming `--script`.
3. **A tool pipeline ate a Rust line continuation**, so a diagnostic read "a program<14 spaces>to
   compile". Scanning for the same shape found **two pre-existing** diagnostics with the same defect,
   in `jr-sema`, which are fixed here.
4. **`Compiler.arguments()` needed `context.allocator`** and the first version did not say so. It does
   now, in the same words `File_Utilities.read_entire_file` uses.

And one thing that did *not* happen, worth recording because it has happened in thirteen of fifteen
waves: **the formatter did not drop `#compiler_library`, and tree-sitter parsed it first time.** Both
because it reuses the generic `DIRECTIVE_EXPR` node shape — the same reason ADR-0184's file-scope
`#insert` was clean. A construct that needs no new node kind needs no new emitter arm and no new
grammar rule.

## Decision

1. `crates/jr-driver` owns one compilation as a `BuildRequest` → `BuildOutcome`; `jr-cli` decides what
   to ask for and renders the answer.
2. A build script is an **ordinary Jairs program**, run in the bytecode VM, whose `Compiler` calls the
   VM forwards to the driver through a `Host` trait; targets are compiled after it returns.
3. `#compiler_library` declares the boundary, and `LinkKind::Compiler` — not a library name — is what
   the dispatch keys on.
4. The boundary carries integers and strings only; `Build_Options` is a library struct that
   `set_options` decomposes.
5. Shelling out is a driver-side spawn, because the type system cannot distinguish `argv` from an
   out-parameter.
6. A file importing `modules/Compiler` is detected as a build script; `--script` remains for the
   indirect case.

## Rejected

- **A `#run` build script, Jai's model** (§2): needs `#foreign_at_comptime` before it can do anything
  at all, and then needs salsa to model filesystem dependencies.
- **A message loop and a poll** (§2): ADR-0153 §1's objection stands, and `build(t) -> bool` is the
  whole of what 23 real scripts get from `.COMPLETE`.
- **A separate program printing a manifest** (§2): the text protocol ADR-0102 §3 rejected.
- **`#system_library "compiler"`** (§3): would emit `-lcompiler`, and a library *name* is forgeable.
- **Passing `Build_Options` across the boundary** (§4): puts a library type's layout in the compiler.
- **Recursive pointer marshalling in the VM** (§5): would refuse `strtod`'s working `char **end`.
- **A host-array intrinsic** (§5): sema, MIR, `ConstValues` and `file_consts`' early-out list, for
  something a driver-side spawn does with none of them.
- **Detecting a script by its shape** (§6): would let detection and the refusal disagree.
- **AST inspection, plugins, `provide_import`, a custom link command**: layered on top, and no real
  script in the corpus needs them to build.

## Consequences

`jr build build.jr` compiles and runs a Jairs build script, which then compiles real Mach-O
executables. `examples/10-build-script.jr` does it end to end: a git hash by subprocess, `-- release`
read from the command line, a per-OS branch, and a target built at a chosen optimisation level.

Tests **1082 → 1090**, and the arithmetic is worth spelling out because it is not simply "+8": seven new
`jr-cli` integration tests for the build script, one `jr-driver` unit test asserting the two default lists
agree, and `confined_output`'s seven tests **moved** from `jr-cli` to `jr-driver` with the function —
which is the move that briefly made the total go *down* (§1). No new corpus file: a build script cannot be one, because
`tests/corpus/valid/` exists on the premise that the VM and native code agree about a program, and a
build script has no native form at all — the same reasoning ADR-0164 used for SDL2.

No new diagnostic code. `#compiler_library`'s two refusals reuse **E0293**, which already means "this
directive does not name a linkable library" — and one of them is that `#compiler_library` takes no
name, which is the same question from the other side. **E0296 is still the first free code**, which `crates/jr-cli/tests/codes.rs` enforces.

Still owed, and now with a reason rather than a plan: `#foreign_at_comptime` (§2), and the VM's deep
pointer marshalling (§5), which a build script no longer waits on.
