# ADR-0197: The build-script gaps this project actually had, closed

- **Status:** Accepted
- **Date:** 2026-09-04
- **Deciders:** dboll
- **Amends:** ADR-0195 §1 (the library-output gap), ADR-0196 §9 (`Compiler.command` is host-mediated)

## Context

ADR-0195 delivered a build script written in Jairs, and ADR-0196 established that a `#run` can allocate,
print and declare a build. Both were verified end to end. Neither answered the question a reader would ask
next: **is this actually capable of the same work as a real `build.jai`?**

That question is answerable by measurement rather than by opinion, and the measurement is what this ADR
exists for. Twenty-three build scripts across the vendored open-source Jai projects were inventoried
feature by feature against what this compiler could do.

**The honest answer was no, and it missed specific things.** The scoreboard, before this wave:

| Feature | Scripts using it | Jairs before |
|---|---|---|
| `Build_Options.output_type` — build a **library** | 13 of 23 | **executables only** |
| `Build_Options.output_path` / `output_executable_name` | 23 of 23 | had it |
| `Build_Options.module_paths` | 16 of 23 | had it |
| `Compiler.arguments` — read the command line | 18 of 23 | had it |
| `add_build_string` — inject generated source | several | **nothing** |
| `set_working_directory` | several | **nothing** |
| `provide_import` / custom link command | 1+ each | **nothing** |
| `additional_linker_arguments` | several | **nothing** |
| `read_entire_file` / `write_entire_file` in a `#run` | many | **nothing** |
| `compiler_begin_intercept` + message loop | all | declined, ADR-0153 |
| Icons, manifests, `Bindings_Generator`, `BuildCpp` | 1–2 each | **nothing** |

`output_type` is the one to notice: **13 of 23** scripts set it, and this compiler could not produce a
library at all. That is not a rough edge, it is the single most-used option in the corpus.

## Decision

### §1. A library is an output kind, and the entry point is a three-state policy

`jr-link` gained `OutputKind`: an executable, a static archive, or a dynamic library. `jr build
--output-kind` selects one, and `Build_Options.output_type` sets it from a script.

**The instructive part is what a bool could not express.** `build_object` took `wants_entry: bool`, and
three behaviours are needed:

- an **executable** *requires* a `main`; its absence is an error;
- a **library** must have no entry point at all, because a static archive containing one fails a C link
  with `duplicate symbol '_main'` — measured, not predicted;
- an **object** is the compiler's bytes either way, so it uses a `main` when the file declares one and
  does not insist.

The third case is what a library asked for as an object needs, and it is the one a bool cannot carry. So
`EntryPolicy` has three variants. **Found by a test**: the "a procedure without the attribute is not
exported" test asked for an object from a file with no `main` and got exit 2.

### §2. `#program_export` gives a procedure a C-visible symbol

A library that exports nothing is not a library, and every Jairs procedure is `jr$<file>$<proc>` by
ADR-0012 — a mangled name no C caller can write. Jai's spelling is `#program_export`, confirmed from
`onelivesleft/jai-cookbook`'s `directives.jai`, and it is the spelling used here.

**Two things had to learn about it beyond the obvious.** The parse tree, HIR and codegen wiring was
mechanical. Then the symbol was still absent from the archive, because the procedure was **dead-code
eliminated**: `jr-mir`'s reachability walks from `main`, and a library has none. An export must be a
**root**. And the symbol needs `Linkage::Export` rather than the `Local` every non-entry procedure gets,
so `ProcKind::Local` grew an `exported` flag — a change that made **every** construction site a compile
error, which is the house rule earning its keep again.

**Verified by linking C against both kinds**: `cc use.c -lthing` against the static archive and against
the dylib, both calling `add_two(40, 2)` and printing 42.

### §3. Four build options, because they are independent in the code

`additional_linker_arguments`, `emit_object`, `add_build_string`, `set_working_directory` and
`provide_import`. Each is a few lines once the request struct carries it, and each is a thing a real
script does.

`add_build_string` is the one with a design question: Jai injects text into a **shared global scope**, so a
generated `VERSION :: "abc";` is simply visible everywhere. Jairs has no shared global scope — a name crosses
a file boundary only through `#import` (ADR-0014 §2) — so the text becomes a module named `Build`, and the
target reads it with `#import "Build";`.

**Say the import out loud, because it is the usage contract and not an implementation detail.** Verified from
a clean directory: a script calling `add_build_string(t, "VERSION :: \"stamped\";")` alongside
`set_working_directory` produces a binary that prints `version stamped`. Without the `#import` the target
gets `error[E0201]: unresolved name VERSION` — which is the *correct* diagnostic under this language's rules
and a confusing one if the docs imply a bare name works.

That costs one line in the target and buys something Jai does not have: a generated name cannot silently
collide with or shadow one the program wrote, because it is in its own scope.

### §4. Comptime file IO is host-mediated, and that closes ADR-0196's remaining gap

ADR-0196 §9 recorded that `Compiler.command` is host-mediated: the driver spawns the process, not the VM.
`read_file` and `write_file` take the same route for the same reason, and a `#run` now reads and writes
real files. `#foreign_at_comptime` remains undelivered and remains unnecessary.

### §5. Icons, manifests, `Bindings_Generator` and `BuildCpp` are refused with reasons

Recorded in `modules/Compiler/module.jr` beside the procedures that exist, rather than left as an absence
a reader has to notice. Each names what it would need. A script can shell out to any of them today
through `Compiler.command`, which is why none of them blocks anything.

The message loop stays declined — ADR-0153's reasoning is unchanged, and it is a **refusal** rather than
a gap: Jai can interleave because its compiler is threads and a queue, while this one is a memoised query
engine, and a script here observes a compilation halfway through only if that engine grows a concept it
does not have.

### §6. `modules/String` grew the algorithm surface it was missing

The inventory of Jai's `modules/String` was recovered from source — 131 signatures across the vendored
copies — and this library had **`trim` missing entirely**, which a build script needed the moment one
tried to strip a newline off `git rev-parse`'s output. Trimming, searching, splitting, joining,
replacing, case-insensitive comparison and parsing all landed, with byte classification going into
`Basic` because that is Jai's split.

### §7. Two procedures were renamed, and the flat namespace forced both

**`String.to_upper` → `to_upper_copy`** and **`File_Utilities.join` → `path_join`**.

Neither was a preference. `Basic` gained a `to_upper` taking a **`u8`** — where Jai puts byte
classification and where a lexer wants it — and `String` gained a `join` that concatenates a `[]string`
with a separator, which is Jai's own name. `#import` is flat (ADR-0166 §7, ADR-0167), so a file importing
both modules unqualified got **E0211 on every use**, and two existing corpus programs stopped checking.

**E0211 firing is the good outcome.** The rule it enforces is AGENTS.md's: in a flat namespace a module
must prefix as though the namespace were its own.

**What makes this worth an ADR section is that both collisions resolved into Jai's own naming.**
`to_upper_copy` and `path_join` are what Jai calls these exact procedures — the byte version is the
unqualified one in both languages, and the allocating string version carries the suffix. So the fix was
not a compromise invented to dodge an error; it was the naming the language being followed already had,
and the collision is what pointed at it. The suffix earns its keep independently: it says at the call
site that the routine allocates, which the in-place twin beside it does not.

Qualified imports (ADR-0179) exist, so `String.to_upper` would have worked. Renaming was still right: a
module whose short name is only usable qualified has made a claim on every importer it cannot honour.

## Consequences

- A Jairs build script can produce a library, inject generated source, set a working directory, pass
  linker arguments, provide an import, emit an object and link it itself, and read and write files at
  compile time. Thirteen of twenty-three real scripts needed the first of those and could not have run.
- The remaining gaps are named with reasons rather than absent. One is a refusal.
- Two library procedures changed name. Both callers migrated; no alias, no deprecation path.
- **1103 workspace tests by default, 1109 under gate 7**, all seven green. Gate 7's clippy caught the
  wave's one cross-back-end omission: `ProcKind::Local` grew `exported`, and `jr-codegen-llvm` is not
  compiled by the six gates, so the exhaustive-match rule found it exactly where that gate exists to look.
- No new diagnostic code. **E0296 is still the first free one** — every refusal in this wave is a
  library note or an existing code.

## Rejected alternatives

- **`wants_entry: bool` with the library case handled at the call site.** The three behaviours are a
  property of the artefact, and pushing the third into two callers is how the two disagree later.
- **A `#program_export` that is inferred** — every procedure in a file built as a library. It makes the
  library's surface an accident of what happens to be declared, and there is no way to keep a helper
  private.
- **Renaming `Basic.to_upper` to `to_upper_byte`** instead of §7's direction. It resolves the same
  collision and it is *not* Jai's naming, so it would have made the unqualified `to_upper` the allocating
  one — the opposite of both languages' convention, and the more surprising call site.
- **Leaving both `to_upper`s and telling callers to qualify.** E0211 would fire for every future importer
  of both modules, and the error names the compiler's ambiguity rather than the library's mistake.
- **A `Bindings_Generator`.** It is a C parser. Shelling out to Jai's, or to `bindgen`, is available now.
