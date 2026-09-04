# A build script written in Jairs — research and implementation plan

> **Status:** research complete, plan proposed, nothing implemented.
> **Date:** 2026-09-04. **Author:** this session. **Would become:** ADR-0195.

The ask: several Jai projects carry a `build.jai` and are built with `jai build.jai` rather than a
makefile. Jairs should have the same — a build script *written in Jairs* that drives the compiler.

This document is research first and a plan second, because the research changed the plan twice.

---

## 1. What Jai actually does

Every fact here comes from a real `build.jai` in a public repository, or from a file whose own header
says it is a verbatim copy of a compiler module. **Jai's `modules/Compiler` is not published anywhere**
— an authenticated code search for `Build_Options :: struct`, `compiler_begin_intercept ::`,
`add_build_file ::` and six other declarations returns nothing but one repo that says it is an
independent reimplementation. So signatures below are known from *call sites* across 23 real scripts
that agree with each other, plus one committed transcript of the compiler's own `print` of
`Build_Options`.

### The shape

`jai build.jai`. No flag, no subcommand. Nothing marks the file as a build script: it is an ordinary
Jai program with a top-level `#run` that calls the `Compiler` module. Workspace 1 is the compiler's own
default metaprogram, workspace 2 is the build file, and workspaces 3+ are the ones the build file
creates.

```jai
build :: () {
    w := compiler_create_workspace("Target Program");
    options := get_build_options(w);
    options.output_executable_name = "hitboxer";
    set_build_options(options, w);
    set_build_options_dc(.{do_output = false});      // or the build file becomes a binary too
    compiler_begin_intercept(w);
    add_build_file("./main.jai", w);
    while true {
        message := compiler_wait_for_message();
        if message.kind == { case .COMPLETE; break; }
    }
    compiler_end_intercept(w);
}
#run build();
```
— `valignatev/hitboxer/build.jai`, condensed; `focus-editor/focus/first.jai` and
`danieltan1517/chess-jai/build.jai` differ only in arrangement.

### The irreducible core

Counted across the 23-script corpus: `output_executable_name` is set by **23 of 23**, `do_output=false`
by 20, `output_path` and `import_path` and `backend` by 16 each, `output_type` by 13. Everything else is
a long tail mostly handled by one convenience call, `set_optimization`.

So a build script that only *compiles* needs: create a workspace, read its options, mutate, write them
back, add a root file, suppress output for the build file itself, and — if it wants to know whether the
build succeeded — intercept and read messages until `.COMPLETE`.

### The part that is not the compiler at all

**This is the finding that matters most.** Nearly everything interesting in those 23 scripts is *not*
the `Compiler` API:

- `chess-jai` clones and `make`s a C dependency when a library is missing.
- `focus` stamps `git rev-parse --short HEAD` into the binary, builds a macOS `.app`, and makes a `.dmg`.
- `jai_raylib` generates C bindings at build time and copies the `.lib` into the module.
- `theos-2` formats a **bootable GPT disk image**, and on Windows relaunches itself inside WSL.
- `trueno` compiles shaders by shelling out; `Jails` runs `codesign`; `PetEngine` copies DLLs.
- Several run the binary they just built, or run tests as a build target.

All of that is `Process`, `File`, `File_Utilities`, `String`, `Basic` — the ordinary standard library.
**Jai's build system is powerful because a build script is an ordinary program with the whole stdlib,
not because the `Compiler` module is rich.** Any plan that gives Jairs a rich `Compiler` module and a
script that cannot open a file has copied the wrong half.

---

## 2. Where Jairs actually stands

### The blocker Jai's model runs into, measured

Jai puts the script at **compile time**, in a `#run`. In Jairs a `#run` can reach *nothing*:

```
$ jr check probe.jr          # probe.jr: X :: #run pid();  where pid calls getpid via #foreign
error[E0230]: compile-time evaluation failed: `getpid` is a foreign procedure, and compile-time
code may not call one until `#foreign_at_comptime` arrives (ADR-0006)
```

Verified this session, for `getpid` and for `malloc`. Every `#foreign` call is refused under
`Mode::Comptime` unconditionally — `crates/jr-vm/src/interp.rs:1368`, the first statement of `fn
foreign`, before the libffi bridge is reached. So a `#run` cannot read a file, read the environment,
shell out, print, or **allocate**: `Basic.malloc` is `#foreign libc "malloc"`.

`modules/File/module.jr:40` says so in the library's own words: *"**No comptime file reading.**
`#foreign` is refused at compile time (ADR-0006), so a `#run` cannot read a file — which is a real
limitation for a build script and is the compiler's decision, not this module's."*

`#foreign_at_comptime` — the gate ADR-0006 assigned to W6 — **was never implemented.** Thirteen
mentions in the repository, every one a comment, an ADR, a PLAN row, or a test asserting the refusal.
No token, no directive, no sema arm, no flag. Meanwhile the hard part is done: `libffi` is a declared
dependency *for this purpose*, the bridge is complete, and ADR-0018 says the remaining work "flips the
mode from §4 rather than adding a mechanism".

**And PLAN's §0 locked decisions already knew:** *"Comptime FFI: **Yes**, gated behind
`#foreign_at_comptime`. VM needs libffi-style dynamic calls. **Non-negotiable given build scripts must
read files.**"* W6 was nevertheless declared DONE without it.

### The thing a program in the VM *can* do — measured

A program run by `jr run` is in `Mode::Runtime`, so FFI is allowed. Verified this session:

```
$ jr run probe.jr -I modules
write true read true bytes 18 join src/main.jr
```

Writing a file, reading it back, joining a path — all work, once the program installs an allocator
(`context.allocator = libc_alloc`, the idiom `examples/07-file-read.jr` already documents).

**One thing does not, and it fails silently.** `Process.run` under the VM:

```
VM:      run ok = true, code = {exited = true, code = 127, signal = 0}
native:  spawned
         run ok = true, code = {exited = true, code = 0, signal = 0}
```

127 is "command not found". The child *is* spawned and `ok` is `true`, so nothing reports a failure —
the `argv` arrives corrupt because the VM translates a pointer argument one level deep and `argv` is an
array of pointers (ADR-0158 §3). A build script that shells out would appear to work and do nothing.

### Two recorded blockers have already dissolved

ADR-0154 §2 recorded that a `Build_Options` **struct** is blocked on struct literals (E0117), and
ADR-0102 recorded that a script adding a module path "wants a list-valued constant". Both are stale:

```
$ jr run opts.jr -I modules
output=app level=0 paths=2 first=modules
```

A struct with `string`, `s64`, `[]string` and `bool` fields, returned from a procedure, **read then
mutated**, its `[]string` filled from an array literal, passed by value to a consumer. Verified this
session in both engines.

Two reasons it works now. The **read-then-mutate** idiom needs no literal — and it is what every real
Jai script uses anyway (23 of 23 call `get_build_options` and mutate the copy), so the literal was never
the requirement. And the `[]string` is fillable because **array literals landed one wave ago**
(ADR-0194); ADR-0102 and ADR-0154 both predate them.

> **One caveat, and this sentence first claimed more than the probe showed.** It said the field was set
> "from `string.["modules", "vendor"]`" — direct assignment, which is **E0240**: Jairs has no implicit
> array-to-view conversion. The literal must be **named** first, then viewed: `paths := string.[…];`
> then `o.module_paths = paths[];`. A view of a temporary (`string.[…][]`) is also refused. So the
> blocker is genuinely gone and the ergonomics cost one extra line, which is why §4's example carries
> that line and a comment saying why. Caught by type-checking §4's example against a stub rather than by
> re-reading the probe — the probe had used `view(*paths[0], 2)` and the prose had generalised it.

### What exists today

Two constants, `BUILD_OUTPUT` and `BUILD_OPT_LEVEL`, read through `file_consts` and beaten by the
corresponding flag. A `noted_declarations("x")` reflection table. And `crates/jr-driver`, whose doc
comment reads *"Compilation orchestration: workspaces, the compiler message queue, and build
metaprograms"* and which contains **nothing else** — one line, no dependencies, depended on by no crate.
Every driver step lives in `jr-cli` instead: `commands/build.rs` is 323 lines, of which `pub fn run` is
**147 lines and 25 top-level statements** — measured, because a round number inherited from a summary is
the kind of claim this project keeps having to correct.

---

## 3. The decision: where the build script runs

This is the whole design, and it is a fork with three answers.

### (A) At compile time, as a `#run` — Jai's model. **Rejected.**

Needs `#foreign_at_comptime` before it can do anything at all, and then needs an answer to a question
Jai never faces: **salsa**. `file_consts` models no external dependency, so a `#run` that read a file
would be memoised against a file it does not know it read, and a second build would use a stale value
silently. Solving that means teaching salsa about filesystem inputs the compiler did not open — a real
piece of work whose cost has nothing to do with build scripts.

Jairs has also rejected the *poll* twice, on its own grounds: ADR-0153 §1 refused
`compiler_wait_for_message()` because a poll's observable behaviour depends on compilation order, which
salsa's re-execution makes unstable by design; ADR-0154 §3 and §4 declined plugin hooks and workspaces
for the same reason. Those decisions are sound and this plan honours them.

### (B) In the VM, as an ordinary program, with driver intrinsics. **Recommended.**

The driver compiles `build.jr` and runs it in the bytecode VM in `Mode::Runtime`. The script calls a
`modules/Compiler` whose procedures are **intrinsics the VM implements** — they record a build request
into driver state. When the script returns, the driver performs the compilations it asked for.

What this buys, each verified above rather than assumed:

- **The stdlib works.** File IO, paths, strings, sorting, JSON — because the script is a program, not a
  `#run`. This is the half of Jai's design that actually matters (§1).
- **No `#foreign_at_comptime` needed.** The keystone blocker is *sidestepped*, not solved. It stays owed
  for its own reason — a `#run` that reads a file — which is honest.
- **No poll and no salsa instability.** The script runs start to finish *before* the target compilation
  begins. There is no interleaving to be unstable about, so ADR-0153's objection does not apply.
- **No text protocol.** ADR-0102 §3 rejected "the driver running the script as a separate program that
  prints a manifest" — but on the *text protocol*, explicitly, not on the two phases. An in-process
  intrinsic call has no protocol to design.
- **The `-I` circularity dissolves.** A source-level module path is circular today because reading any
  constant needs `file_consts(db, root, search_paths)`, which takes the paths as an argument. Here the
  script's own imports resolve under paths the operator gave, and the *target's* paths are data the
  script produced. Nothing is read before it exists.
- **`do_output = false` disappears.** The single most-written line in real Jai build scripts (20 of 23)
  exists to stop the build file becoming a junk binary. Here the script is never a target, so there is
  nothing to suppress. A whole class of mistake is unavailable.
- **ADR-0154 §4 sanctioned exactly this.** It named what a revisit would need: *"a compilation unit that
  is a **value** — created, configured and built by a `#run` — which is a very different thing from a
  poll and would need its own ADR."* A `Target` handle is that value.

What it costs: **the script cannot shell out** (§2), so no shader compilation, no `codesign`, no running
the built binary — until deep pointer marshalling lands. That is the honest limit of v1, and §6 prices
it.

### (C) Compile the script natively and let it drive `jr` itself. **The baseline, and it works today.**

Nothing stops anyone writing a Jairs program that calls `Process.run(["jr", "build", …])`, compiling it
with `jr build`, and running it. Natively `Process.run` works — verified above. Two commands instead of
one, and the script has no access to anything the compiler knows.

This is worth stating because it is the *floor*: the feature is not "impossible today", it is
"unpleasant today". A plan has to beat the floor, and (B) beats it by making it one command and by
giving the script the compiler's own vocabulary instead of a string of flags.

---

## 4. The surface for v1

Mapped from Jai's irreducible core (§1) onto Jairs' idioms, with the read-then-mutate shape verified in
§2.

```jr
// build.jr
Compiler :: #import "Compiler";
String :: #import "String";

build :: () {
    args := Compiler.arguments();               // what followed `--` on the command line
    release := args.count > 0 && String.equal(args[0], "release");

    level: s64 = 0;
    if release { level = 1; }

    paths := string.["modules"];                // a literal must be named before it is viewed

    t := Compiler.create_target("jairs-demo");
    o := Compiler.options(t);                   // read the defaults, never construct from zero
    o.output = "demo";
    o.output_path = "build";
    o.opt_level = level;
    o.module_paths = paths[];
    o.bounds_checks = !release;
    Compiler.set_options(t, o);

    Compiler.add_file(t, "src/main.jr");
    if !Compiler.build(t) {
        Compiler.report("the target failed to build");
        return;
    }
}
```

> **The first draft of that example did not compile, and the two reasons belong in Wave 2.** It was
> written in Jai's idiom and checked afterwards. `args[0] == "release"` is **E0278** — `==` on a `string`
> is refused, because same storage and same contents are both plausible for a `{data, count}` pair
> (ADR-0099 §4), so a script compares with `String.equal`. And `ifx release then 1 else 0` produced
> **seven** errors: Jairs has no `ifx`, so a conditional value is a `var` plus an `if`.
>
> Everything in the corrected version except the `Compiler` calls was then **run**: `String.equal` over a
> `string.[…]` literal, the `&&`, the `var`-plus-`if`, exit code 1. A plan shipping uncompilable example
> code teaches the wrong idiom to whoever implements it, and the module in that example does not exist
> yet, so nothing would have caught it later either.

Invoked as `jr build --script build.jr -- release`, or `jr build build.jr` when the file declares
`build` and no `main` — the second spelling is the one to aim for and the first is what makes it
testable before the detection rule is settled.

**The whole surface above was type-checked and run before this plan was accepted.** `modules/Compiler`
does not exist, so it was **stubbed** — the eight signatures verbatim from the table below, the
`Build_Options` struct with its seven fields, placeholder bodies that print — and `build.jr` was checked
and then run against it:

```
$ jr check build.jr -I <stub-path>
1 file checked, 0 errors
$ jr run build.jr -I <stub-path>
set: output=demo path=build level=0 paths=1 bounds=true
add: src/main.jr
build: target 10
```

That is worth more than it looks. It proves the read-then-mutate flow, the view assignment, the string
comparison and the by-value struct round trip all compose *in this language* — and it found three defects
in this document's own example, two of which no later gate would have caught, because a plan's code block
is not compiled by anything.

**Eight procedures and one struct.** Every one is a request recorded against driver state; none of them
performs a compilation except `build`.

| Jairs | Jai counterpart | Notes |
|---|---|---|
| `Compiler.create_target(name) -> Target` | `compiler_create_workspace` | `Target`, not `Workspace`: Jairs declined workspaces (ADR-0154 §4) and reusing the word would claim the poll model |
| `Compiler.options(t) -> Build_Options` | `get_build_options` | returns the *defaults*, so a script mutates rather than constructs |
| `Compiler.set_options(t, o)` | `set_build_options` | write-back is separate, exactly as in Jai, because mutating a copy must not act at a distance |
| `Compiler.add_file(t, path)` | `add_build_file` | |
| `Compiler.build(t) -> bool` | the message loop's `.COMPLETE` | **collapses Jai's four-call intercept into one boolean**, because with no poll there is nothing to interleave |
| `Compiler.arguments() -> []string` | `compile_time_command_line` | 18 of 23 real scripts read it; `- release` is the idiom |
| `Compiler.report(message)` | `compiler_report` | routes through the compiler's diagnostic renderer, so a script's complaint looks like every other error |
| `Build_Options` | `Build_Options` | `output`, `output_path`, `opt_level`, `module_paths: []string`, `library_paths: []string`, `bounds_checks`, `backend` — the seven that matter, against Jai's ~60 |

`Build_Options` is declared in `modules/Compiler` and validated by the compiler against a field table,
exactly as `Type_Info`, `Any` and `Declaration` already are — so editing it is E0265 rather than a
silent wrong read. That mechanism exists and has three clients; this is the fourth.

**Deliberately absent from v1**, each with its reason:

- **The message loop and any poll.** ADR-0153 §1's objection stands. `build(t) -> bool` is the whole of
  what 23 real scripts get from `.COMPLETE`.
- **AST inspection and modification** (`Message_Typechecked`, `compiler_modify_procedure`). A large
  surface with no relevance to compiling, and `noted_declarations` already covers note-driven
  inspection at run time.
- **A custom link command.** `additional_linker_arguments` covers most of what it is used for at a
  fraction of the cost; the escape hatch can come when a target needs it.
- **Plugins.** Strictly layered on top; Jai's own protocol is seven callbacks.
- **`provide_import`.** Only needed when `module_paths` is not enough.

---

## 5. The staged plan

Five waves. Each ends with all seven gates green, an ADR, and a corpus or integration test — the
project's usual rhythm. Sizes are relative, not calendar.

### Wave 1 — `jr-driver` stops being empty, and the driver becomes callable twice · **small**

Move `jr build`'s ordered steps — `pub fn run`, 147 lines and 25 top-level statements — out of
`crates/jr-cli/src/commands/build.rs` into `jr-driver` as
a function taking a **request** rather than parsed CLI args:

```rust
pub struct BuildRequest { root: PathBuf, output: Option<PathBuf>, opt_level: Option<OptLevel>,
                          module_paths: Vec<PathBuf>, library_paths: Vec<PathBuf>,
                          backend: BackendChoice, bounds_checks: bool, emit_object: bool }
pub fn build(request: &BuildRequest) -> Result<BuildReport, BuildError>
```

`jr-cli` then becomes what it should be: argument parsing plus one call. **Nothing about build scripts
yet** — and that is the point. This wave is verifiable on its own (every existing test still passes,
because the behaviour is identical), and it is the prerequisite the plan cannot skip: a build script
needs the driver to be callable more than once with different requests, and today it is a `main`-shaped
function reading `clap` structs.

PLAN.md:277 already says this crate "should consume `jr_db::workspace` rather than invent a second".

### Wave 2 — the script runs, and can name its artefact · **medium**

`jr build --script build.jr`. The driver compiles the script with its own `BuildRequest`, runs it in the
VM, and executes the requests it recorded. `modules/Compiler` with the eight procedures; the intrinsics;
`Build_Options` with its field-table contract.

Acceptance: a `build.jr` that names its output and its optimisation level produces the same binary as
the equivalent flags, proved by comparing the two artefacts.

The interesting risk is **not** the intrinsics — it is that `modules/Compiler`'s procedures are the first
in this project whose *implementation* is the driver rather than Jairs or libc. `noted_declarations` is
the closest precedent and it folds to a constant, which these cannot.

### Wave 3 — the script reads the world · **small**

Nothing to build in the compiler: this wave is a corpus program and an example proving a script can read
a file, join paths, inspect `os()`, and branch on `Compiler.arguments()`. It exists because §2's
measurements were taken by hand and a claim this load-bearing needs a test that runs every time.

Acceptance: a `build.jr` that reads a version string out of a file and passes it to the target as a
generated constant. That last part needs `add_build_string`-equivalent, so either this wave adds
`Compiler.add_constant(t, name, value)` or it defers and says so.

### Wave 4 — deep pointer marshalling, so a script can shell out · **medium, and independently valuable**

The VM translates a pointer argument one level deep, so `argv` arrives corrupt and `Process.run` returns
127 while reporting success (§2). Fixing it unlocks shelling out for build scripts **and** fixes
`Process.spawn` under `jr run`, which is a documented wart today and the reason `Process`'s test is a
native integration test rather than a corpus program (ADR-0158 §3).

Do it as its own wave because its value does not depend on build scripts, and because the silent-127
failure deserves a test of its own regardless.

### Wave 5 — `jr build build.jr`, and the doc corrections · **small**

Detect a build script rather than requiring `--script`: a file that declares `build` and no `main`. The
rule needs a decision about ambiguity (both declared, neither declared) and a refusal for each.

Then the corrections §7 lists.

---

## 6. What this does not solve, priced

- **`#foreign_at_comptime` stays owed**, and this plan does not need it. Its own value is a `#run` that
  reads a file — code generation from a schema, for instance. Its real cost is not the mode flip (one
  `if`) but **salsa**: a memoised `#run` that touched the filesystem goes stale silently. Whoever picks
  it up should read that as the wave's actual content.
- **A source-level `-I` for the *program*** is still circular if anyone wants `BUILD_MODULE_PATHS` as a
  constant in the program's own source. Inside a build script it is not circular, because the script is
  a separate compilation — which is the argument for the script rather than more `declared_*` queries.
- **Cross-compilation.** `theos-2` sets `os_target` and an LLVM triple from its build script. Jairs'
  whole notion of a target is `TargetLayout` plus `TargetOs::host()`, and `jr-link` shells to the host
  `cc`. A `Build_Options.os_target` would be a lie until that changes.
- **Running the built binary** needs Wave 4.

---

## 7. Documentation defects this research found

Each verified this session, and each is a claim the repository makes about itself that is false.

1. **`modules/Compiler` does not exist.** ADR-0158's Consequences say *"W7 — Stdlib is DONE: nine of
   nine, with `Compiler` delivered inside W6"*. Nothing named `Compiler` was ever created; ADR-0154,
   which closed W6, does not claim one. `PLAN.md:1217` is the un-struck row that says so.
2. **PLAN's W6 row claims `#run build()` build scripts "replacing makefiles" as DONE.** What shipped is
   two constants. The row's own summary is accurate — *"Build scripts name the artefact and choose the
   optimisation"* — so the overclaim is the word DONE against the item, not the description.
3. **`tests/corpus/valid/020-run-directive.jr:11`** says a bare `#run` is *"executed for its side
   effects during compilation"*. It has none that anything can observe: no FFI, no globals, no
   allocation. The comment describes a capability that does not exist.
4. **ADR-0154 §2's `Build_Options` blocker is stale** — read-then-mutate needs no struct literal, and
   ADR-0102's "wants a list-valued constant" is answered by ADR-0194's array literals.
5. **`crates/jr-driver`'s doc comment** promises workspaces, a message queue and build metaprograms from
   an empty file. Wave 1 makes it true or it should say what it is.
6. **`AGENTS.md` called `editors/nvim/parser/jairs.so` "a checked-in `.so`".** It is gitignored
   (`.gitignore:35`) and `git log --all` on it is empty — it was never tracked. That changes the advice:
   it rots on a machine rather than in the repository, so nothing a reviewer sees can catch it.
7. **`jairs-dashboard.pdf` was a commit behind `jairs-dashboard.typ`**, so the artifact a reader opens
   still said "ALL TWELVE WAVES DONE" after the commit correcting that claim. Found by
   `git log -1 -- <artifact>` against its source, which is the whole check. It is the only *tracked*
   generated file in the tree; the other two are gitignored.
8. **This document's own §4 example did not compile, in three ways** — see the note there. `==` on a
   `string` is E0278, `ifx` does not exist, and an array literal does not implicitly become a view. Two
   of the three would have survived any review that read the code instead of running it, and §2's prose
   had generalised its own probe from `view(*paths[0], 2)` to a direct assignment that is refused.

---

## 8. The recommendation in one paragraph

Do **not** copy Jai's `#run`-plus-poll model: it needs `#foreign_at_comptime` to do anything, and then
needs salsa to model filesystem dependencies, and Jairs has twice rejected the poll on grounds that
still hold. Instead run the build script as an **ordinary Jairs program in the VM**, with a small
`modules/Compiler` whose procedures record a build request the driver then executes. That gets the half
of Jai's design that actually matters — a script with the whole standard library — sidesteps the
keystone blocker rather than pretending to solve it, honours ADR-0153's objection by having nothing to
interleave, and is what ADR-0154 §4 said a revisit would need. Start by making `jr-driver` real, because
a build script needs a driver that can be called twice and today there isn't one.
