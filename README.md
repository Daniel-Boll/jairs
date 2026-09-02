# Jairs

Jairs is a Jai-inspired systems language with compile-time execution, explicit
allocators, and no GC, RAII, or exceptions — compiled by a hand-written,
error-recovering compiler written in Rust.

> **Status: pre-alpha.** Jairs source runs in the compile-time VM *and* compiles to
> a native binary, and the two agree byte for byte — including where a trap
> happened, and now including every construct either of them accepts — ADR-0051
> and ADR-0056 closed the two cases where one compiled what the other refused. The language it agrees about is deliberately tiny. The tables below say
> exactly how tiny, and are updated at the end of every wave; if they and the code
> disagree, the code is right and the tables are a bug.
>
> See [`PLAN.md`](PLAN.md) §1.5 for per-crate status, §2.1 for the wave order, and §7
> for what happens next.

---

## Status, honestly

**Both compilers now show a local variable at its real place on the stack.** The second one needed a small piece
of care worth mentioning: it reports a variable's position measured from the bottom of the stack frame, while the
debug format measures from a reference point near the top. Reconciling those is one subtraction, and getting it
wrong produces a location that looks perfectly valid and reads the wrong memory — so the test checks the sign of
the result rather than merely that a location is present.

It also corrected something written an hour earlier. An earlier note concluded, from a single test program, that
a structure-typed local could never be shown by name. That was too strong: it depends on how the variable is
used. One passed to a function by value is shown; one only assigned field by field is not. A negative result from
one program is evidence about that program — generalising it needs a second program that differs in the suspected
way, and here that program was one line away. This is the second time in one sitting that the project's habit of
writing the thing before planning around it caught its own freshly written conclusion.

**Both compilers now describe a struct to a debugger, and they agree** — the same field names at the same byte
offsets, produced by two implementations with nothing in common. One asks LLVM to write the description; the
other writes the bytes itself. The offsets in both come from the single function the compiler uses to generate a
field access, so neither can drift from the running code, and the test checks that the two *agree* rather than
that each one produces something.

Reaching that also uncovered a step the plan had skipped. The remaining debug-info item needs variable
descriptions; a variable description needs a function description to live inside and a type description to point
at; and one of the two compilers had neither — only line numbers pointing into a section that did not exist. The
general lesson: when a plan item looks like it needs one new thing, check what that thing needs to live in.

**A local variable can be printed in a debugger** — by the name the programmer wrote, with its type and its
place on the stack. Half of that item is delivered, and the other half is now precisely described rather than
vaguely owed: a variable that lives entirely in registers has no stack address to point at, so describing it
needs a different kind of location entry. Writing the test is what established the boundary, and the test now
asserts the absence as well as the presence — so whoever closes the gap is told by a failing test to update it,
rather than having to rediscover why the case was missing.

The name lookup got this wrong once in an instructive way. It first identified a variable by the source position
recorded on its storage, which worked for simple values and silently failed for structures, whose storage records
the position of the expression that created it instead. A lookup that names some variables and not others is
worse than one that names none, because the gap looks like a fact about the program rather than a gap in the
compiler.

**A struct's layout is visible to a debugger** — its field names, and the byte offset of each one. Those offsets
come from the very function the compiler uses to generate a field access, so a debugger cannot disagree with the
running code about where a field is. Computing them separately for the debug info would have been a second
implementation of layout, which is exactly the kind of duplication that drifts into a wrong answer.

Getting there turned up something the plan had wrong. The type descriptions were written and correct, and the
debugger tools showed the basic types and no struct at all — which looks precisely like the new code being
broken. It was not: LLVM discards a type description that nothing *declares*, and a function signature mentioning
a type does not count as declaring it. What retains a type is a variable of that type. So each parameter is now
declared, and two items the plan listed separately turn out to be one piece of work: a type nothing declares is
not emitted, and a declared variable with no type has nothing to point at.

**Both compilers can name a source line, and they agree** — which took two entirely separate implementations. One
back end's line table is written by hand, byte by byte, including the relocations that let a linker fill in
function addresses. The other back end's is written by LLVM itself, from metadata attached to each instruction.
The two share exactly one thing: the single lookup that turns an internal position into a file, a line and a
column. That they then agree about which lines exist is the property worth having, and it is why each has its own
test rather than one shared one — a shared test would only check the part they have in common.

One mistake is worth recording. At first every function's debug scope pointed at the main source file, so
statements from an imported library were attributed to the program that imported it. A line table naming the
wrong file is worse than no line table: it sends a reader to a line that has different code on it. A test that
only checked the main file would have passed it, so both tests now check that the imported module has its own
entry too.

**A native binary can now name a source line** — the first debug information this compiler has ever produced. A
built object carries a real DWARF line table, and its rows point at actual statements rather than at the top of
the file. The plan had claimed line tables already existed; checking found none at all, so this started from
nothing.

The part worth describing is not the DWARF. Trap messages already knew how to turn an internal location into
`file:line:column`, but only as finished text — and a line table needs the pieces. So rather than writing a second
lookup, the existing one was split: it now returns the pieces and the text is formatted from them. A trap and a
debugger therefore cannot disagree about where a statement is, because there is one answer and two renderings of
it. The same reasoning had been applied once before to keep two compilers from drifting; this applies it to two
consumers of one fact.

Two false starts are recorded, and both were a single string. On macOS the section is called `__debug_line`, not
`.debug_line` — get that wrong and the tools silently ignore it, which looks exactly like producing nothing. And a
debug section placed outside the segment reserved for it does not merely get ignored: it breaks the link, because
the linker lays it out among real pointers.

Last updated with **W10 — Graphics DONE** (W6 — Metaprogram, W7 — Stdlib, W8 — Performance and W9 — Tooling
depth were already), 1066 tests green — 1070 with the LLVM back end compiled in, and nineteen library modules.
**Two waves remain: W11 — Concurrency, and W12 — Debug info, which is now under way.**

Graphics arrived in four steps, on a foundation the plan had wrong: a window and a 2D renderer, an event loop,
an immediate-mode UI, and image loading. It rests on SDL2's C API rather than on Cocoa, because every Cocoa call
goes through a variadic function this compiler's back end cannot express — a blocker that turned out to live
upstream, not here. The cost is a third-party dependency where the plan imagined system frameworks; the gain is
that it works today and works on Linux.

The plan predicted graphics would need *zero* compiler changes. That was wrong in one direction and then right
in another: passing a rectangle to a C function needed two compiler waves of its own first, and once those
landed the four graphics waves needed nothing at all.

**An aggregate crosses a `#foreign` boundary** (ADR-0160, ADR-0161), which was the project's
highest-leverage open item: it blocked graphics entirely — every windowing and GPU call passes a rectangle by
value — and also blocked `readdir`, `stat` and `getaddrinfo` in the standard library. All three are now
unblocked.

**Where an aggregate's pieces go is answered in exactly one place**, because three engines cross that boundary
and a struct in the wrong register is a silent wrong answer with no diagnostic. The compile-time VM describes
the struct to libffi and lets it place the pieces; Cranelift emits one machine value per register; LLVM emits
the same separate scalars rather than using its own `byval` lowering, so the two native back ends produce the
same call and the differential harness compares like with like. The diagnostic that refuses the rest asks the
same function, so it cannot drift from what the engines can do.

Two shapes are supported — at most two words in general registers, and a homogeneous float aggregate of at most
four members in floating-point registers — and everything else is refused with a message naming what works. A
homogeneous float aggregate has **no size limit**, which is the point rather than an oversight: a `CGRect` is
four doubles and thirty-two bytes, so a byte test would reject precisely the type graphics needs most.

The refusal is argued rather than temporary. It covers a small *mixed* struct, and the two supported targets
genuinely disagree about one of those: System V classifies each eight bytes independently, putting a `double`
and a `long` in two different register files, while AArch64 puts both in general registers. One case with two
correct answers gets refused until it is split — an honest narrower rule beats a wrong wider one, which is the
same call this project made about `sqrt`.

**A window opens and gets drawn on** (ADR-0164). `modules/Window` creates a window, makes a 2D renderer, sets
a colour, clears the surface, fills and outlines a rectangle, draws a line, presents it and tears everything
down — ten steps, in a compiled binary, against real SDL2. It is the seventeenth module and the first that
`jr run` cannot execute: the compile-time interpreter resolves a foreign symbol from the compiler's own
process image, so it reaches the C library and nothing else.

**And closing that wave found a bug in something else** (ADR-0168). The plan's table of what each wave delivered
carried three "not delivered" notes that were no longer true, and they had been added in the first place to
correct a *different* set of stale claims. Two documents disagreed about one of them, so rather than pick a side
the construct was simply run — and it turned out that half of it worked, half of it had never been tried, and the
untried half produced an internal compiler error on a program that looked perfectly legal.

It is now a diagnostic. The construct in question decorates a parameter to say "this argument is a compile-time
constant"; written on a *return* type it cannot mean anything at all, because a return has no argument. So it is
refused rather than implemented, which is the strongest case a refusal can have.

The correction then made the same class of mistake one screen later: a note claimed a file count was unchanged
because a test fixture had moved between directories. The reasoning was plausible and the count was wrong. Running
the count caught it.

**An image loads and draws** (ADR-0167). A BMP is decoded, uploaded to the renderer as a texture, and drawn
scaled into a rectangle. BMP rather than PNG because SDL decodes it in the library already installed, where PNG
would mean a second dependency or an inflate implementation — the largest single thing this standard library
would contain. The test builds its own image file rather than committing one, so the decoding path is genuinely
exercised instead of trusting a blob generated once.

Writing it turned up a real limitation, in the good way. Its routines were first called `fill`, `free`, `destroy`
— and a program importing it alongside two other modules got four separate ambiguity errors, because an import
here brings names in flat and there is no way to qualify them. The compiler caught every one. Every routine is
now prefixed, and qualified imports are written down as owed rather than bolted on to make the inconvenience go
away.

**A button works** (ADR-0166), which is the wave that shows the pieces compose rather than merely exist: one
test holds a window, an event queue and a renderer open together and drives a real click through all three.

It is immediate-mode, so there is no widget tree and nothing is allocated — a caller re-declares every widget
every frame and keeps three pieces of state on the stack. A click completes on **release inside after press
inside**, never on press, because pressing a button and then dragging away to cancel is an escape hatch every
user expects to work, and the implementation that fires on press passes every positive test while breaking it.

One bug in it is worth recording, because the tests caught it and review would not have. Asking "is this widget
hovered" compared the widget's id against the id of whatever was under the cursor — and when nothing was under
the cursor, that field held a reserved "nothing" id. So asking about the reserved id itself answered **yes**, on
every frame, about a widget that does not exist. A sentinel meaning nothing must not be answerable through the
same accessor as a real value.

**And the window can be closed** (ADR-0165) — which is worth reading because the previous wave recorded, in an
accepted decision record, that it could not be. The reasoning was that reading an event means reading an
`SDL_Event`, which is a union, and this compiler refuses a union at a foreign boundary. The refusal is real. It
is also beside the point: what is refused is an aggregate passed **by value**, and the SDL call takes a
**pointer** — the same shape as the rectangle that same module had been passing successfully all along.

The project has a written habit for this: confirm a premise by *writing* the thing before planning around it.
It has now paid seven times, and this was its most valuable catch — against this project's own accepted record,
from the same sitting. The correction cost one probe and no compiler change at all. A decision record is
evidence of a decision, not evidence of a fact.

Two smaller things came out of building it, both by writing rather than reasoning. SDL does not promise that one
pushed event means one polled event — a test that polled once per push passed the first time and failed the
second — so the library drains the queue instead. And a fabricated keypress is accepted by SDL and then
silently discarded, so the keyboard tests read an event built locally rather than one round-tripped.

**Graphics is unblocked, on a different foundation than the plan named** (ADR-0163). The plan said "Cocoa via
`#foreign`", and every Cocoa call goes through `objc_msgSend`, which is variadic — and the blocker for that is
upstream, in Cranelift. So that removes an option rather than delaying the wave, and the wave is built on
**SDL2's C API** instead. Proven rather than proposed: a Jairs program opens a window, creates a renderer, sets
a colour, clears the surface, fills a rectangle, presents it and tears everything down. Six calls, six
successes, none of them needing Objective-C or a struct passed by value.

The cost is stated: SDL2 is a third-party library where the plan imagined system frameworks, so a drawing
program needs it installed. In exchange the wave starts now, on an API that also works on Linux — which this
project calls a target and which Cocoa never was. The probe also found the last missing piece, and it was
small and exact: `ld: library 'SDL2' not found`. A `#system_library` declaration says *what* to link and never
*where*, so `jr build` gained `-L` and reads `JR_LIBRARY_PATH` — as a flag rather than a directive, because a
source file naming `/opt/homebrew/lib` is unbuildable anywhere else.

**The other gate in front of graphics is a stated limitation** (ADR-0162). A `#foreign` declaration can
carry `#c_variadic`, meaning its parameters are the *fixed* ones and the C declaration ended in `...` — which
nothing can infer, since a Jairs signature cannot say that C permits more. Declaring one is legal and
**calling** one is a diagnostic, so a library author can annotate `printf` today and get an error rather than
corruption. That matters because the alternative was measured: a fixed-arity declaration of a variadic function
put the mode argument in the wrong place and created a file with permissions `---------x`, silently.

Refusing it in all three engines was chosen over supporting it in the two that could — the compile-time VM and
LLVM both can — because a build failing where the interpreter succeeds breaks the premise the two-engine
harness rests on. The blocker is upstream: Cranelift has no variadic calling convention at all.

**And it is verified against a real C compiler, not against itself.** A test that called a Jairs procedure
using the C convention would pass with both sides wrong, since one classification would emit the call and read
it. So the corpus calls libc's `ldiv`, which returns a sixteen-byte struct and whose convention was fixed years
ago, and checks the quotient and remainder separately so that reading two result registers in the wrong order
shows up. A second test compiles a C shim with `cc`, links it, and runs it — covering an aggregate *argument*,
a return whose fields are deliberately swapped, and a nested four-`double` rectangle that a byte-count test
would have rejected.

**Semantic tokens ship** (ADR-0159), the fourteenth and last LSP capability — and the only one whose whole
value is information the parser does not have. The tree-sitter grammar colours this language well and cannot
tell one identifier from another: `Point` and `count` are both `IDENT` to it, and so are a parameter, a field,
a procedure and a module. The provider classifies by syntactic **context** first and asks the resolver only
about a bare name, which is what makes it work in a file that does not parse — the state an editor is in most
of the time.

**And W9's other item turned out to be mis-described, which is the wave's second deliverable.** The plan said
"line tables exist; locals and layouts do not". There is no DWARF at all: an empty `.debug_line`, no `__DWARF`
segment, nothing consuming the `gimli` dependency the workspace already declares. This README's own capability
table said "Not started — no DWARF at all" and was right the whole time. So a debug-info writer is now a named
wave of its own rather than a line in a wave described as "small, and mostly already done".

**The standard library is complete as scoped** (ADR-0158): `Basic`, `String`, `Sort`, `Array`, `List`, `Map`,
`Math`, `Random`, `Generic_Types`, `Time`, `Bucket_Array`, `JSON`, `File`, `File_Utilities`, `Process`,
`Socket` — all written in Jairs, and `Compiler` delivered inside W6. `Thread` was one line in a stdlib list
and is really a wave of its own, so it is W11 and said so rather than being quietly dropped.

`Process` and `Socket` drew a boundary worth knowing about. **The compile-time VM cannot pass a pointer to
memory that itself contains pointers**: it translates a foreign call's pointer argument from its own region to
a host address, one level deep, and one level is all a type can support — the VM knows a parameter is a
pointer and cannot know the bytes behind it hold more. `execvp`'s argument vector is exactly an array of
pointers, so spawning a process works in a compiled binary and fails under `jr run`. Refusing such a call was
considered and rejected, because the same test would refuse `strtod`'s out-parameter, which works. So that one
module's test builds a binary, while the socket module — whose address struct holds only integers — is a
corpus program that opens a real TCP connection to itself in all three engines.

**Files open, read, write and append** (ADR-0157), with paths joined, split and normalised on top. These are
the first modules whose correctness depends on something outside the program, and that is where the wave got
interesting: **it found two silent defects, and neither was in the modules.**

A fixed-arity `#foreign` declaration of a **variadic** C function passes the extra argument in the wrong
place. Declaring `open(path, flags, mode)` created a file with permissions `---------x` on arm64 — variadic
arguments travel on the stack, a fixed third argument in a register — with no diagnostic from either engine
and a file that existed and could not be read. Creation now goes through `creat`, which is genuinely
fixed-arity.

And freeing a **string literal** aborts as a native binary while running perfectly under `jr run`, because
the compile-time VM serves `malloc`/`free` from its own region and quietly drops a pointer it does not
recognise. The shape that found it — `out := "";` in a loop that later frees `out` — is one any
accumulate-into-a-string routine has. That is exactly the divergence the two-engine harness exists to catch,
and it only catches it when a program actually does it: which is why the file module's test writes to a real
`/tmp` rather than mocking a filesystem. A mocked test would have passed in both engines and shipped the
abort.

**`JSON` parses** (ADR-0156) — the module the plan called the most valuable for proving the language, and
the first one here that is not a utility: it has a data model, a grammar, a failure mode and two kinds of
allocation. A value is an **index** into one flat node array rather than a pointer in a recursive type, so
freeing a document is one call and a handle carries no ownership question — including on the error path,
where a half-built pointer tree would have to be unwound. A number's *extent* comes from JSON's grammar and
its *value* from `strtod`, because `strtod` alone accepts `0x1p3`, `inf` and a leading `+`; integers are
converted in Jairs, since a `float64` cannot hold 2^53 + 1 and returning the wrong one would be a silently
wrong answer. **Serialisation is deliberately absent**: writing a `float64` back out needs a correct `dtoa`,
and an approximate one would emit numbers the parser could not read back, which is worse than emitting none.

Two of the plan's own guesses about that module were wrong, and are corrected where they were written: a
`variant` is not the right JSON value, and `Map` cannot be an object — it is `Map(s64, s64)` and cannot key
on a string, and a member chain preserves source order anyway, which a hash table destroys.

**`Time`, `Bucket_Array` and a stable merge sort landed** (ADR-0155), the first three of W7's nine
remaining modules. `Time` is nanoseconds as an `s64` with a monotonic and a wall clock, and deliberately
no formatting: rendering a timestamp needs a calendar, and a calendar needs leap seconds, time zones and a
locale that this project has decided nothing about. `Bucket_Array` keeps element **addresses** stable by
appending fixed-size buckets to a movable spine — the promise `List` cannot make, because it copies on
growth — which is what a UI retains handles into. And `stable_sort` closes the debt ADR-0104 §3 opened,
taking its scratch from the **arena**, making that ADR-0065's first real customer.

**The sort would not compile, and four polymorphism defects came out of finding out why.** All four were
silent: a template that allocated its own scratch, or called another template, type-checked and then
reached an engine as `no routine for file N proc M`. `typed(T, …)` refused a bound type variable while
`size_of(T)` beside it accepted one; an instantiation's pointer views were never threaded into the
mid-end; a template calling a template was refused for an inference it did not need; and one call
**deleted** a shadowed type binding instead of restoring it, so a `size_of(T)` after an inner call failed
in a body where `T` was bound throughout. That last one was on the known-defects list, described as masked
— it was not, and it hid only because both existing callers happened to put the inner call last.

**W6 is closed, and the last three waves' worth of decisions are made.** PLAN §8.6 laid out seven steps to
finish the programme; the first three are done. A `#foreign` signature that cannot be lowered is now a
diagnostic instead of an internal compiler error (ADR-0150). **`#must` exists** (ADR-0151) — the error
marker ADR-0008 chose in the vertical slice and nothing built for the whole programme — so a fallible
operation returns a value beside a flag and ignoring the flag has to be *written*, as `_ = f();`. The
obligation lives in the procedure's *type*, in ADR-0008's reserved effect-row slot, which is why it works
across module boundaries and through a procedure pointer. And the compiler can now **emit static data**
(ADR-0152), which delivered `Type_Info.fields` — owed since ADR-0078 — and let a metaprogram *iterate* the
declarations carrying a note instead of unrolling to a guessed bound (ADR-0153).

**Two W6 items were declined rather than deferred**, with the reason stated: plugin hooks and Jai-style
workspaces are both the *poll* model, and a poll's behaviour would depend on compilation order, which
salsa's re-execution makes unstable. What a plugin would want is already available in two reproducible
pieces.

**What happens next is planned, and the plan named its own blockers.** PLAN §8 is the completion plan
for the four waves that remain — W6 Metaprogram, W7 Stdlib, W9 Tooling, W10 Graphics — and writing it
changed three things. Five of W7's seven remaining modules turn out to wait on one decision this
language has never made: there is no error-handling model, so `File`, `Process`, `Socket` and the useful
half of `JSON` have no way to say that ignoring a failure is an error. `Thread` + atomics was one item
in a stdlib list and is really a wave of its own, so it is now W11. And W10 was described as "all
library work, no compiler changes", which is false: no aggregate crosses a `#foreign` boundary today, and
every windowing and GPU API passes structs by value — so W10 is **blocked**, not merely unstarted.

**Probing the plan found a live defect.** Calling a `#foreign` procedure with a struct by value produces
an internal compiler error in both engines rather than a diagnostic — the ninth occurrence of this
project's most-recorded failure shape, found by writing the thing rather than reading about it. It is
also the cheapest fix on the list, because one back end already refuses it in words.

**One of W8's eight sub-waves shipped a measurement instead of a feature, on purpose.** Parallel
semantic analysis was written, produced byte-identical output at every thread count, and was then
measured: 1.20x on a clean 119-file tree and 1.01x on one with errors, against a ceiling of 2.5x set by
the 40% of a check that runs inside the type pool's exclusive critical sections (ADR-0149). It was
reverted. A 1.2x speedup is not worth a deadlock mode that appears only under threads, and the honest
deliverable of a performance wave is the number — including when the number says no. The blockers are
recorded and neither is a scheduling change.

**There is a vector type, at the width the machine actually has.** `v: #simd [4]s32` is one register,
and `a +% b` adds four lanes at once (ADR-0148) — one instruction natively, a loop in the VM, and the
differential harness asserts the three agree byte for byte. The legal shapes are exactly the six a
128-bit register holds, and that is a deliberate machine fact in the language: a wider vector would
have to be split or quietly turned into a loop, and a directive that is silently ignored is worse than
one that is refused. Integer division is refused for the same reason, because no machine has one.
Integer lanes take the *wrapping* operators, because no vector add can trap and one spelling should not
mean two things.

**A struct can be stored as arrays.** `Entities :: struct #soa(4) { x: s64; hp: u8; }` lays out as one
array per field, and `e[i].x` means `e.x[i]` (ADR-0147) — so a loop over one field is contiguous
instead of striding over every other. The whole feature is a change in the type checker *before*
layout runs, so layout, reflection, the VM and both back ends needed no line: after resolution there
is nothing special about the type. A bare `e[i]` is refused, because with the fields in separate arrays
there is no single element to name.

**There is a compile-throughput number, and it is published rather than claimed.** On an Apple M2 Pro
with a `--release` compiler, over `tests/corpus/valid` (116 files, 9 203 lines, 360 982 bytes) with
`modules/` on the search path, best of ten: **`check` 113 103 lines/s** and **`build` 25 864 lines/s**
(ADR-0146). The debug compiler every gate runs manages 87 460 and 19 230. `jr bench --throughput`
took it, it reports and never judges, and the machine is quoted beside the figure because a throughput
number without one is not a number. The most useful thing the table says is that **`build` costs 4.4×
`check`** — the front end is not where the time goes.

That number is also what `modules/Sort` was waiting for: **`heap_sort`** now sits beside `sort` — in
place, no allocation, `O(n log n)` always, and *unstable*, which is why it is a second name rather
than a faster `sort`. Stability is observable behaviour, so swapping the algorithm silently would
change what an existing program computes. The choice is proved by a **comparison count** rather than a
timing: deterministic, machine-independent, identical in all three engines, so it is a test in the
differential harness instead of a number needing a footnote.

**The inliner takes a non-leaf callee**, so the `sort_ints` → `sort` → `less_int` shape a standard
library is full of collapses instead of stopping at one level (ADR-0145). `024-hello.jr`'s optimized
MIR shows it: `print_line` is inlined *two levels*, through `print` to the `write` call. Store-to-load
forwarding also crosses blocks now, along a single-predecessor chain — sound because one predecessor
both ran first and dominates the load.

**A recursive callee is refused, and a test decided that rather than a review.** The first version
unrolled recursion three levels, which is correct, and it broke the four-frame backtrace test: an
inlined callee has no frame, inline-provenance backtraces are deferred, and in a recursive trap the
*depth* is the message — so flattening three of four frames is a backtrace that lies about what
happened. A plausible optimisation traded against a documented promise, caught by the corpus.

**A struct can control its own layout.** `x: s64 #align 16;` raises a field's alignment and
`y: s64 #place 32;` puts one at an exact byte, so an overlay of several fields on the same bytes —
the hardware-register and file-format case a `union` cannot express when only *some* fields overlap
— is now spellable (ADR-0144). The whole feature is a change to the one place layout is computed
plus the syntax to reach it: **no engine changed for it**, and three independently written engines
agree on the resulting offsets because all three read the same numbers from the same place.

Building it decided two things the plan had wrong. `#align` is a **minimum** rather than a rule that
refuses a lowering value, because a field's natural alignment is not always knowable while
signatures are being resolved — so the refusal would have been enforced only sometimes. And probing
for a "misaligned `#place`" refusal found something worse than the case it was meant to prevent: the
LLVM back end was already promising `align 8` on addresses it computes itself and has proved nothing
about. It now claims `align 1` everywhere but an `alloca`, which is sound for every field and free.

**There is a third execution engine.** `jr build --backend llvm` compiles through LLVM 21 via
`inkwell` (ADR-0143), so the differential harness now holds **VM ≡ Cranelift ≡ LLVM**. All 114
executable corpus programs, and every trap tried by hand, agreed with the VM on the *first* run —
reason, location and two-frame backtrace, byte for byte. That is the return on two old decisions
being decisions rather than habits: a back end that computes no layout of its own (ADR-0018 §2) and
consumes SSA it did not build (ADR-0017) has almost nothing left to disagree about. It is also worth
saying that agreeing immediately means the third engine has *found* nothing — its value is
prospective, a second witness for every future change to MIR or layout.

It is behind a **default-off cargo feature and a seventh gate**, because `llvm-sys` needs an LLVM 21
installation it can find and an unconditional dependency would wall off the whole compiler for a
back end that is not the default. Three things in it differ from the Cranelift translation, each
forced by LLVM: a block parameter becomes a `phi`, every offset is a byte `getelementptr` with **no
Jairs aggregate ever acquiring an LLVM struct type** (which would put LLVM's padding rules in charge
of where a field sits), and poison has to be *avoided* — a shift past the width, a division by zero
and an out-of-range `fptosi` are all undefined in LLVM where Jairs traps or saturates.

**W8 opened with an optimisation level** (ADR-0142): `-O0` and `-O1` on `jr run` and `jr build`,
closing the deferral ADR-0058 §6 handed this wave. The level's real payload is a check the mid-end
never had — **every corpus program's whole observable behaviour is now asserted identical at both
levels**, so "optimisation preserves meaning" is a test rather than an argument. The one legitimate
difference is a backtrace: at `-O1` an inlined leaf's trap names the call site, at `-O0` it names the
leaf's own line, and both halves are pinned.

The **eight-wave programme to keep the promises ADR-0127 found unkept** is **fully done**
(ADR-0128 through ADR-0139), and both of its owed follow-ups have landed: **ADR-0140** converted
`modules/List` to operate on the native `[..]s64` (the hand-rolled `List :: struct($T)` deleted) and
added `Type_Info_Kind.DYNAMIC_ARRAY`, and **ADR-0141** landed a `..Any` variadic — `f(*a, *b, *c)`
packs arguments of arbitrary types into a `[]Any` (`print(fmt, ..)`). All six unkept promises are kept: instantiation backtraces,
enum-member-from-constant, `Math` vec/mat/quat, `it`/`it_index` in a nameless `for`, nested
procedures, `[..]T` dynamic-array syntax, `$$T`, and `..T` variadic parameters — including the
call-site packing sugar (`sum(1, 2, 3)` packs a stack view).

**The four most recent waves were driven by an audit rather than by a feature**
([`docs/assessment-2026-08-07.md`](docs/assessment-2026-08-07.md)).

**Instantiation diagnostics now name the call that demanded them** (ADR-0128) — the first of the six
unkept promises below to be kept. The frame machinery had existed since the vertical slice, defined early
so the feature "would not need retrofitting", and W5 shipped without ever constructing one; the real gap
was that an instantiation carried no call span. A multi-level chain is still owed.

**Expired deferrals have been swept out of the code** (ADR-0127), and this one was found by a user
reading a single diagnostic rather than by a gate. `E0207` told people that nested procedures "arrive in
wave W2" **six waves after W2 shipped** — and W2's scope never included them, so the note named a wave
that had both passed and never owned the feature. Eleven such places now say what is *owed* instead of
when it arrives. `E0212` also stopped claiming "`void` is not a type name in Jairs" while
`size_of(void)` folds to 0. §2.1 of [`PLAN.md`](PLAN.md) now records **six features a completed wave
promised and did not deliver**, including instantiation backtraces, whose machinery exists and has no
production caller at all.

**A foreign call's pointer span is now bounded by the VM's own check** (ADR-0126), the first of the three
narrow security dispatches that audit said it still owed. Translating a pointer argument for libffi validated
**one byte** — all a C signature tells you — and the `write` capture path then dereferenced the program's own
`count` bytes through it. From an ordinary POSIX declaration, `write(1, s.data, 4_000_000)` on a two-byte
string exited 0 having written **4,000,000 bytes**, ~3 MB past the end of the VM's memory region; a count of
2e9 killed the compiler with **`SIGBUS`**; and the *native* build of the same program wrote 114,688 bytes, so
the two engines disagreed. It is now a trap with a source location. The bound is the **region, not the
buffer** — within one linear region an address is just an offset, which is the documented model — and the
`unsafe` block was deleted rather than corrected.

The same pass **verified two things sound**, which is half of what a security review is for: the comptime FFI
gate holds structurally, so a hostile file *merely opened in an editor* cannot reach libffi and run native
code inside the language server; and the heap/frame fix from ADR-0107 is complete. Two dispatches remain
owed — forging an `Any` or a procedure pointer, and `jr-lsp` path handling.

**A declared `BUILD_OUTPUT` is confined to the working directory** (ADR-0122). ADR-0102 let a program name
its own artefact, and nothing checked the value — which is computed by arbitrary compile-time code *in the file
being compiled*, so it is attacker-controlled whenever the source is, and for a compiler that is the ordinary
case. `BUILD_OUTPUT :: "../../.git/hooks/pre-commit";` made `jr build` write an executable to a path git runs
on the next commit, which turns "I compiled a file someone sent me" into "I ran their code". A leading `-` was
read by `cc` as a flag. An explicit `-o` is deliberately still unconfined: that is the operator's instruction
rather than the artefact's, the same asymmetry that already makes `-o` win.

**Compile-time execution now has a step budget** (ADR-0121) — 10 million instructions, after which a `#run`
reports E0230. It had none: only recursion was bounded, so `HANG :: #run spin();` with a `while true` inside
hung `jr check` with no diagnostic and no way out but a signal. That mattered more than a slow compile,
because `file_consts` calls the VM inside a salsa query and the loop reads no database, so cancellation could
never reach it — under `jr lsp` it wedged the worker thread on a file the user had merely **opened**. `jr run`
is deliberately unmetered: there a long loop is the user's program working, and the two engines must agree on
what a program computes, not on how patient they are.

**And ADR-0120 closed four programs that reported `internal compiler error: no routine for file N proc M`
while `jr check` reported "0 errors"**: a template calling a template, a computed `#insert` sharing a file
with any polymorphic call, and a `#run` or a `typed(…)` inside a template body. One shape behind all three —
*a key computed against one tree and read against another*. An instantiation's body is a **clone** with its
own `BodyId`, so redirects built from the base check could not name its call sites; expansion now iterates to
a fixed point and reads the **final** check. A computed `#insert` used to disable instantiation for the whole
file, on a justification its own comment described far more narrowly than the code implemented. And a clone
now inherits its template body's folded values, which is a scope substitution rather than a remap because the
body arena is cloned whole. Two things are refused rather than guessed: an unbounded instantiation family
(E0280), and a `$N` call in a file whose `#insert` operand is computed (E0281) — there the argument's value is
keyed to the unexpanded tree, so a call before the splice keeps its key while one after it shifts, and the
pairing could deliver *another expression's* value.

The audit's remaining findings are open and listed in `PLAN.md` §7. Its **security scope is only partly
covered** and that is worth stating rather than implying: the assessor responsible for it failed twice, so the
VM's memory region, forging an `Any` or a procedure pointer, comptime-FFI-gate bypasses and `jr-lsp` path
handling are **unexamined**. A second pass is owed.

W7's modules so far are **`String`** (ADR-0103): `equal`, `compare`, `starts_with`, `ends_with`, `find`, `contains`,
`byte_at`, `is_empty`, **none of which allocate**. It exists because the *previous* wave named it — ADR-0099 §4
refused `==` on two strings, since a `string` is `{data, count}` and so "the same storage" and "the same
contents" are both plausible readings, and its stated reason was that comparing contents needs a byte loop,
which is a library's job rather than an operator's. So `equal(a, b)` is what E0278's help was pointing at.

It is **its own module rather than more of `Basic`**, and the deciding argument is not size: `Basic` is imported
by every program, so anything in it is a tax on all of them — but more importantly, adding to `Basic` would mean
nothing ever tested that **two modules can be imported at once**. Every module test to date imported `Basic`
alone. Nothing allocates on purpose: `concat` and friends need somewhere to put a result, and while the
mechanism exists (`context.allocator`, temporary storage), the *choice* between "always the context allocator",
"an explicit parameter" and "always temporary" does not — and settling it in passing is how a library acquires
an accidental convention.

`byte_at` is there because `s.data[i]` **does not compile** — a `*u8` is not indexable — so reading a byte takes
`(s.data + i).*` and a cast. It is honestly a workaround, and out of range answers `-1` rather than trapping,
unlike an array index: an array's bound is known to the compiler so passing it is a mistake, while scanning
until the bytes run out is an ordinary loop.

**And `Sort`** (ADR-0104) is the second: `sort(xs, less)` orders a view in place for any element type, given a
comparison. The **caller** supplies the ordering rather than the module requiring `<`, and that is a language
fact rather than a taste — resolving an *operator* inside a `$T` template against the instantiated type is a
lookup instantiation does not do. `operator <` exists and `#modify` can *reject* an instantiation, but nothing
can *select* an implementation per instantiated type; that is operator-bounded polymorphism, and it belongs to
whichever wave decides how a template states its requirements. The algorithm is **insertion sort**, `O(n²)` said
plainly: it is *stable*, which quicksort is not, it needs no allocation, which is the decision `String` declined
to make, and it is short enough to read. **W8 delivered the faster one** (ADR-0146): `heap_sort` sits
beside it — `O(n log n)` always, no allocation, and unstable, which is why it is a second name rather
than a replacement — with a comparison count rather than a timing behind the choice. And **W7 completed the
family** (ADR-0155) with `stable_sort`: `O(n log n)` *and* stable, at the cost of scratch space, which is
the trade the other two do not offer. Three names rather than one clever default, because each of the three
is the right answer to a different question — smallest, fastest without allocating, fastest while stable.

**Writing that module found two leaked internal errors**, which is the argument for a standard library written
in the language paying out twice in one sub-wave. Passing an **imported procedure as a value** reported "this
compiler has a gap — please report it" for a legal program: the value had been representable all along, and
what was missing was a three-line bridge. And calling an **imported template** leaked "no routine for file 2
proc 0" — cross-file instantiation is deferred, but the refusal did not exist, because a `$T` parameter's type
is `ERROR` and `ERROR` matches anything, so the call type-checked. It is now a diagnostic that **names the
workaround**, and a corpus file checks that the workaround works, since a refusal is only as good as its escape
route. Both bugs were hiding behind a stale comment that said something checkable nobody had checked; one of
them recorded the refusal as *intended* while users saw a bug report.

**And `Array`** (ADR-0105) is the third: a **fixed-capacity** array with `push`, `pop`, `get`, `set`. W7's plan
names a *dynamic* array, and this is not one — for three reasons that were **probed rather than assumed**, each
a refusal the language already makes on purpose. A `malloc`'d region **cannot be typed**, because a general
pointer cast makes a wrong pointee type a silent wrong read and is refused; so `data: *T` is declarable and
nothing can produce a `*T` from an allocator returning `*u8`, which puts heap storage out of reach until a
*typed allocation* primitive exists. Inference *through* a parameterised struct is deferred. And a parameterised
struct **cannot cross a module boundary at all** — the one that decided the shape, found by importing the
module: the first draft compiled cleanly inside it and failed at the importer's first use, so a polymorphic
struct in a module is unusable by everyone.

Routing around those was considered and rejected. A `*u8`-backed array with hand-computed byte offsets *is*
expressible, and every read would need the element size as a literal while every write reinterpreted bytes —
exactly the silent wrong read the cast refusal exists to prevent. The standard library, where a reader looks to
learn what the language means, is the worst possible place to route around a deliberate refusal.

`push` answers `false` when full rather than trapping, because filling a fixed buffer is something a correct
program does and then handles, while indexing past a compiler-known bound is a program error. `pop` and `get`
return two values rather than a sentinel, the opposite call from `find`'s `-1`, and the difference is that an
index has values outside its domain while an element does not.

**And typed allocation** (ADR-0106) is the fourth piece, which is a *language* change rather than a module:
`size_of(T)`, `typed(T, p)` and `untyped(p)` make heap storage reachable — `d := typed(s64, malloc(n *
size_of(s64)))`, then ordinary pointer arithmetic, then `free(untyped(d))`. That is the first of the three things
`Array` named as blocking a real dynamic array.

**`cast` is unchanged**, and that is the design: it still refuses `cast(*s64, p)`, because a general pointer cast
makes a wrong pointee type a silent wrong read. `typed` is not *safer* — `typed(s64, p)` on four bytes is still
wrong — it is **visible**: the target type is a type argument at a named boundary a reader can grep for, the same
shape that lets an erasing conversion happen only at an `Any` boundary. It takes a `*u8` specifically, since
`*T` → `*U` would be the refused cast reached by another spelling.

Building it found a **pre-existing miscompile**. Retyping is a store-then-load through a slot, and store-to-load
forwarding deleted exactly that step, producing a use whose source and destination types differed. The verifier
caught it, which is the good outcome, but the *pass* was wrong: there the store and load **are** the conversion
rather than a redundant pair. The first fix was too broad and lost a real optimisation on struct fields — caught
by the optimized-MIR snapshot, which is precisely the job a snapshot has, since an optimisation quietly not
happening is invisible to every other check.

**And `List`** (ADR-0107) is the genuinely growable array typed allocation unblocked: heap storage, doubling from
four, `push`/`pop`/`free_data`. It is a **separate module rather than a rewrite of `Array`**, because the two have
different contracts — an `Int_Array` needs no cleanup while a growable list **owns** memory a caller must free, and
with no destructors in the language that is something read in a type's name or never learnt. As of **ADR-0140** the
list *type* is the native `[..]s64` (ADR-0136): the hand-rolled `List :: struct($T)` is gone and the module is now
the operations over the compiler's own dynamic array — a caller declares `xs: [..]s64` and calls `push(*xs, v)`.

**Writing it produced the corpus differential's first real catch.** The test exited 247 in the bytecode VM and
255 natively, and bisecting gave thirteen lines: a callee that allocates, writes, and hands the pointer back,
where the write succeeded *inside* the callee and read back zero outside. The VM satisfies `malloc` from its own
linear region — so a pointer stays a bounds-checked offset — and that region's cursor **was the frame bump mark,
restored on return**. Heap memory allocated inside a callee was therefore reclaimed when it returned, and read
back as zero rather than garbage precisely because release zeroes for determinism, which made the symptom a clean
wrong answer instead of a crash. The heap now grows downward from the top of the region.

Every earlier catch was a construct **both** engines got wrong together, or a leaked internal error. This was one
engine right and the other wrong — the failure two independent implementations exist to expose, and the reason
the corpus asserts exit codes rather than agreement. Nothing had found it because a growable array is the first
construct whose whole point is memory outliving the call that made it.

**And an imported module's own errors are now reported** (ADR-0108), which the previous sub-wave found and left
alone because fixing it changes what every command reports. A root whose imported module was broken used to pass
every gate — `jr check` printed "0 errors" — and then fail inside an engine with a message naming a `FileId`.
Resolution was never wrong: checking the module alone always reported the unresolved name. Nothing *asked* it.

All three commands now walk the reachable set they already use to assemble MIR, and each diagnostic keeps the
module's **own** file and span, because attributing it to the `#import` line would read better for someone using a
module they cannot edit while discarding the only thing that locates the bug. This does make the compiler reject
programs it used to accept — every one of which was going to fail anyway, later and less comprehensibly.

**And the library now composes** (ADR-0109): `view(p, n)` builds a `[]T` from a pointer and a count, so
`sort_ints(elements(*l))` sorts a growable list **in place** — `List`, `Sort` and the language's own view type
cooperating on one buffer with no copy. That was the previous sub-wave's closing gap: a slice takes an *array*, so
nothing could turn a pointer and a count into a view, and a growable array and a sorting routine sat side by side
unable to be combined.

Getting there meant revisiting a refusal whose **stated reason had expired**. A view's `.data` was refused because
it "would hand out an unbounded `*T` … and there is no pointer arithmetic to use it with" — and both halves are now
false. The answer was not to expose `.data` but to add the constructor a caller actually wanted, so the refusal
stands for a better reason. The element type comes from the pointer, so nothing is asserted; the count is
**unchecked**, and that is said plainly, because a pointer's allocation size is tracked nowhere and a checked
version would need a registry the native back end could not share with the VM.

**And calling a null procedure pointer now traps** (ADR-0110), found while probing what allocator `String`'s
allocating half should use: the first thing tried — `context.allocator(8)` before installing an allocator — leaked
an internal compiler error, and `context.allocator` is null until installed, so that is a mistake a reader will
actually make. Both engines were wrong differently: the VM's packed proc-pointer handle decoded null to file 0
procedure 0 (an arbitrary real procedure, giving a message about an arity nobody wrote), while native would have
jumped to address zero. It is now one trap in both, exit 4 with a source location. The VM's handle is biased by
one so that zero can mean null — `valid/048` calls the first procedure in the file, which packed to the same
handle as null and proved the bias necessary.

**And `String` grew an allocating half** (ADR-0111): `concat`, `substring`, `to_upper`, `to_lower`,
`free_string`. Each produces a new string through `context.allocator`, and the caller frees it — the convention
`String` deferred when it shipped its non-allocating half. Not temporary storage, because a result that expired
on an unrelated reset would be a trap; not an explicit allocator parameter, because the context exists to carry
exactly this, so a caller who wants arena behaviour installs an arena and gets it for every routine at once.
Forgetting to install one is not silent — a null allocator traps, which is why that trap was built first. This
was the first W7 sub-wave in several to touch no compiler crate at all: built entirely on what the language
already had, which is what a maturing language should let a library do.

**And `Math`** (ADR-0112) is the exact, closed-form functions — `abs`, `min`, `max`, `sign`, `clamp`, `pow`,
`gcd`, `floor`/`ceil`/`round` — with **no `sqrt`, `sin` or `log`**, which is the surprising part and the whole
design. The obvious `Math` wraps libm, but a float cannot cross the FFI boundary yet, so libm is unreachable and
the module is pure Jairs; and a transcendental approximated in Jairs would be wrong in a way this project cannot
tolerate, because its last bits depend on evaluation order and the two engines could disagree on the last ulp,
the one thing the differential harness treats as a failure. So it ships only what it can make exact — the line
between `floor` (in) and `sqrt` (out) is exactness, not difficulty — and says so at the top of the module rather
than surprising a reader who reaches for `sqrt`.

**And `Random`** (ADR-0113) is a deterministic xorshift64 generator whose state the caller owns: `seed`,
`next`, `below`, `coin`. Its `u64` arithmetic agrees bit-for-bit between the engines, which a generator depends
on absolutely — a sequence that differed would fail the harness on its first call. The state is caller-owned
rather than a hidden global (untestable and usually clock-seeded, which the differential harness cannot use) or
context-carried (a callee facility). xorshift64 because its correctness is obvious, which beats better
statistics for a baseline. Writing it surfaced a real language gap: a `u64`-range named constant has no
`name : T : value` form, so the golden-ratio seed is declared through `#run` of a `-> u64` procedure whose
return type gives the too-large literal its context — recorded rather than worked around silently.

**And `Time`, `Bucket_Array` and `stable_sort`** (ADR-0155) are W7's next three. `Time` is one integer unit
— nanoseconds in an `s64`, exact for ±292 years, where a `float64` of seconds loses nanosecond resolution
in the 2030s and would make two runs of one benchmark differ in their last digits. Two clocks, because
using the wrong one is a quiet bug: `monotonic` never goes backwards and is the only correct thing to
*measure* with, `wall` is what a timestamp wants. `Bucket_Array` is the container whose element addresses
never move — `push` returns the stable pointer, and there is no `remove`, because compacting would break
the promise and a tombstone would stop `get` being pointer arithmetic. `stable_sort` merges bottom-up
through arena scratch and **falls back to insertion sort when the arena has no room**; both paths are
stable, so the answer never depends on memory pressure, which would be the worst kind of bug to chase.
Its one load-bearing line is `less(right, left)` rather than `less(left, right)` — that is what stability
*is*, and no test of sortedness can see the difference, so the corpus program sorts by one key and inspects
another.

**And a float can now cross the FFI boundary** (ADR-0114), the language unblocker two library sub-waves named:
a `#foreign` procedure may take and return a float, so `sqrt`, `pow` and friends are callable. A float is passed
in a floating-point register on every real ABI, not as a machine word — so the VM's libffi path describes the
argument and return as a float type (which places it in the right register) and native code gives the foreign
signature a float parameter. Passing the bits as an integer would call the routine on a float register that was
never written: a plausible-looking wrong number, silently, which is why the register placement is load-bearing
rather than an optimisation. Both engines call the same libm, which is correctly rounded, so `sqrt(16.0) == 4.0`
is an exact comparison — the exactness `Math` said an in-language approximation could not have, and the reason
its transcendentals belong behind this boundary. That unblocks them as a libm wrap.

**And `Math` grew its transcendentals** (ADR-0115): `sqrt`, `sin`, `cos`, `exp`, `ln`, `powf`, as libm wraps
now that a float can cross the FFI boundary. `Math` had shipped without them and named FFI floats as the reason;
they arrive the right way — libm is correctly rounded and both engines call the same libm, so `sqrt(2.0)` is
bit-identical in the VM and native code, the exactness an in-language approximation could not have. That is a
three-sub-wave arc worth noting: a library named a language feature it needed, the language delivered it, and
the library collected. **`Math` is now complete in the sense ADR-0115 tried to claim** — the `vec/mat/quat` W7 promised were
all absent when the audit looked, which ADR-0127 §3 caught. ADR-0130 added `Vector2/3/4`, ADR-0131
added `Matrix4` (column-major, right-handed, with `operator *` for matrix×matrix, matrix×vector and
matrix×scalar), and ADR-0132 added `Quaternion` (`{x, y, z, w}` layout matching `Vector4`, right-handed
to match `cross` and `Matrix4`, no auto-normalisation on multiply). The audit's third finding is
closed by three consecutive all-library sub-waves.

**And a hash table** (ADR-0116): `Int_Map`, `s64 -> s64`, open-addressed with linear probing and tombstone
deletion, grown at 3/4 load — a heap array of structs, the module that most exercises typed allocation and
`List`-style growth. Concrete, for the same cross-file-generics reason `Array` is. Its hash is
FFI-free `u64` arithmetic, so both engines compute the same bucket — and writing it caught the project's
**second** engine divergence: the wrapping operators (`*%` and friends) decoded their operands to `i128` and
computed `wrap(a * b)`, and two large `u64`s overflowed `i128` *itself*, panicking the compile-time evaluator
before the wrap could take the low bits — while native code, which multiplies in a 64-bit register, was correct.
Both of the differential's real catches have been in arithmetic or memory the native path did in hardware while
the VM modelled it in Rust, where the model was subtly off. Fixed to wrap on the truncated `u64` values, which
is what `*%` always promised.

**And a parameterised struct can now cross a module boundary** (ADR-0117) — the biggest language unblocker the
wave had left, named by *three* library sub-waves: `Array` and `Map` are concrete `Int_*` types only
because a `struct($T)` declared in a module was unusable by every importer (`List` was too until ADR-0140
moved it onto the native `[..]s64`, whose routines stay concrete `s64` for a *different* reason — an imported
polymorphic procedure is still refused, E0268).

It was not a lookup change. A parameterised struct's fields are resolved *per instance, under the caller's type
arguments*, and its own file cannot do that — it does not know what an importer will supply. So the **importer**
resolves them, which needs the field type tree, and that tree is indexed into the *declaring* file's arena. The
check phase now receives the imported HIR, which the database already holds, rather than copying those types onto
the signatures as a second representation of the same thing. Identity stays the declaring file's, so `Box(s64)` is
the same type in two importers — which is what lets a value of it pass between them. The pool needed nothing: the
instance-keyed field map built for local parameterised structs already keyed on an instance carrying its declaring
file, so a cross-file one was representable from the day that map existed.

Building it found three things by running, the sharpest being that a field naming the *declaring* module's own type
was resolved in the **importer's** signatures — which, had the importer declared a same-named type, would have
resolved silently to a different type rather than failing.

Before it, wave **W6 — Metaprogram**, five sub-waves in. Its headline claim is met — a metaprogram can find
declarations by note and generate code for each — and a build script can name its own artefact. A declaration can
carry **`@note` metadata** for a metaprogram to read (ADR-0098). `@deprecated` and `@requires "x"` sit in the
same attribute loop as `#c_call`/`#expand`/`#modify`, so notes and directives interleave freely — but a note
is its own node kind, because a note is *data for a metaprogram* while a directive is an *instruction to the
compiler*, and a consumer collecting notes must not have to filter directives out of the same list. A note
affects **no code**: the noted program's MIR is exactly what it would be without them, which is the point. What
notes still lack is a **reader**, and that is deliberate — the next sub-wave is the compiler message loop, the
mechanism that lets a metaprogram *ask* for the declarations carrying a note, and it is worth designing against
data that already exists. The formatter dropped every note on its first run, so a build script collecting `@X`
would have silently found nothing; gate 5 caught it.

**And a metaprogram can now read them** (ADR-0099): `has_note(f, "x")` answers `bool` and
`note_value(f, "x")` answers `string`, both **folded while checking, with no VM and no new query** — unlike
`type_info`, which folds later because it needs a layout; a note's answer is in the HIR the checker is already
holding. The first argument is the **declaration itself** rather than its name as text, so a misspelling is an
unresolved name instead of a silent `false` — the same silence the dropped notes had. An absent note answers
`false` and `""` and is *not* an error: asking whether a note is present is the point, which is the opposite
call from `any_as`, and the difference is that `any_as` would otherwise return garbage while this returns the
truth. **And it can query them** (ADR-0100): `noted_count("serialise")` answers how many declarations in the file
carry that note and `noted_name("serialise", 0)` names them, in **declaration order** — the one order a reader
can predict from the source, since sorting by name would renumber every index when a declaration is inserted
and a hash order would make one program answer differently between runs. An out-of-range index answers `""`
rather than being refused, because unrolling to a fixed bound is the intended use and its tail has to be quiet.

**And it can generate code for each of them** (ADR-0101): `#insert noted_insert("serialise", "write(#);")`
emits the template once per noted declaration, with `#` standing for each name — so one line of source
generates a call to every `@serialise` procedure in the file. That is the metaprogram loop for the case that
matters, and it needed no new machinery: the query was already there, the fold channel was already there, and
`#insert` of a computed string has been there since ADR-0073.

**The loop lives inside the fold, and that is the right place rather than a workaround.** A run-time loop
could not do this job at all — generated code has to exist before checking, so a loop running after the
program was compiled could not declare a procedure, add a field, or emit a statement. What is still genuinely
missing is *inspection*: a run-time loop reading declarations as values, which needs the compiler-emitted
table and is bundled with `Type_Info`'s variable-length field list. Stating the split that way is a
correction — the earlier claim deferred generation and inspection as one thing.

Building it found a **latent miscompile** that predates it: a folded value is keyed by expression id, a
computed `#insert` renumbers every id after its splice, and so with *two* computed inserts in one body the
second's value landed on a different expression — a `string` on an arithmetic operand. It surfaced as the MIR
verifier panicking rather than as any diagnostic, which makes it the sharpest well-typed-placeholder this
project has had: the value is genuine and merely in the wrong place, so nothing in the type system can see
that it is wrong.

**And a build script can name its own artefact** (ADR-0102): `BUILD_OUTPUT :: #run choose_name();` in the
program decides what `jr build` writes, which is the makefile's most basic job and the first time anything in a
Jairs file has influenced the *build* rather than the program. A **declared constant** rather than a
`set_build_output("app")` call, because a call has to happen — its effect would depend on evaluation order and
on the script being reached — while a constant is a fact about the file, and order-dependent configuration is
the failure mode makefiles are notorious for. An explicit `-o` still wins: a person at a terminal is overriding
on purpose. This is emphatically **not** a build system — no dependency graph, no incremental rule, no multiple
artefacts — so the honest claim is that "a build script replaces the makefile" is now true of *something*
rather than true in general.

**Iteration at run time is still missing, and the reason is worth stating plainly** rather than filed as a
limitation: all four of these are answered while *checking*, so every argument must be readable then — and a
`for` variable is not, because it exists only at run time. `for i: 0..noted_count(…)` cannot be made to work by
folding whatever it is called — which is why *generation* takes the fold route above instead. Reading
declarations as run-time values needs the query to lower to real code reading a **compiler-emitted table**:
static data a back end emits and the VM can also read, which Jairs has never had, and which is the same
mechanism `Type_Info`'s variable-length field list has been deferred for since ADR-0078. So notes can be
counted and named, and cannot yet be looped over. That makes the message loop a wave about static data rather
than a wave about notes, which is a better-shaped wave — and getting there is what the
data-then-reader-then-query ordering bought.

Writing that sub-wave's corpus file **found a shipped leaked internal error**: `a == "x"` on two strings
reached the VM as `expected a scalar, found an aggregate`, for a program any reader would expect to compile. A
`string` is `{data, count}`, so its `==` has exactly a view's two plausible meanings — same storage or same
contents — and a view's `==` was already refused for that reason; it is now refused for every aggregate
(E0278), by a *structural* test, since size and alignment cannot tell an `s64` from a two-field struct of
`s32`s. Comparing contents needs a byte loop, which is `String`'s job in W7 rather than an operator this wave
invents.

Before it, **W5 — Polymorphism completed** in fifteen sub-waves: `$T` procedures *and* polymorphic
structs work. `Box :: struct($T) { value: T; }` is a **type constructor**, not a type — `Box(s64)` applies
it to the type argument `s64`, and `Box(s64)` and `Box(bool)` are **distinct types** from one declaration,
with distinct field types and distinct layouts. They are told apart in the pool by the argument in the key,
exactly the way `[2]s64` and `[3]s64` are (ADR-0085); each instance's fields are the declaration's,
substituted per argument, keyed on the instance rather than the declaration, so both engines compute the
right layout independently. The change touched the pool's most load-bearing invariant — a struct's identity
was its declaration site — and it was landed in two commits: a *zero-behaviour-change* representation
refactor (proven by an unchanged snapshot and test count), then the parameterised behaviour on top, so a
half-built type-identity change could never hide a miscompile. Deferred with by-design refusals, not gaps:
inferring a struct's argument through a `$T` parameter (`(b: Box($T))`), `using` on a parameterised struct,
and a cross-file one (E0269). Before it, `$T` procedures went end to end.
`id :: (x: $T) -> T` is declared as a template (no concrete signature, no MIR), a call `id(42)` infers `$T`
from its argument, and the compiler appends a **concrete procedure** — one per distinct structural tuple of
bound types (ADR-0005), deduped across call sites — that both engines run like any other. Nothing
polymorphic survives to the back end, which is what lets the differential harness check a polymorphic
program at all. The body is checked *per instantiation*, so `add(a, b)` on a struct with no `+` is a
diagnostic rather than a miscompile. It handles several type variables (`pair :: (a: $A, b: $B)`) and
inference through a pointer or view (`deref :: (p: *$T)`, `sort :: (items: []$T)`), by a one-layer
structural match rather than a full unifier. On top of that, **comptime-value parameters work** (ADR-0087 surface, ADR-0088 instantiation): `make :: ($N: s64)`
marks a parameter polymorphic over a compile-time-known value, the value-side mirror of `$T`. A call
`make(5)` evaluates the argument to a constant via the same acyclic pre-pass `#insert` uses, and appends a
concrete procedure with `N` **baked** into the body — the instantiation's parameter list drops the `$N`
params, and each reference to `N` becomes a literal. `make(5)` twice dedupes to one instantiation, `make(7)`
is a distinct one, and mixed comptime+runtime params (`scaled :: ($N: s64, factor: s64)`) pass only the
runtime ones at the call. A non-constant argument is refused with E0271. And `[N]T` sized by a `$N` parameter works (ADR-0089), which is what the feature is *for*: two
instantiations get genuinely different array types from one declaration. And **`#expand` macros have their surface** (ADR-0090): a macro parses, formats and checks like any
procedure, and **a call splices** (ADR-0091): the macro's body lands in the
caller's scope, so it sees and can modify the caller's locals — deliberately unhygienic, matching Jai. A
generated prelude binds each argument **once** (substituting it per use would re-evaluate a side-effecting
argument), and expression position gets a generated result local so one mechanism serves both. The MIR shows
no calls at all — every body inlined. Refused by design: an early `return` (E0273), a void macro in
expression position, and a cross-file call (E0272 — which had been reaching the VM as an internal error).
And **`type_info(T)` reflects a bound type variable** (ADR-0092): a `$T` procedure can ask its own bound
type's size, field count, or identity — each instantiation reflecting its own type. That was found missing
while designing `#modify`, whose predicate needs exactly it, and fixing it also turned a leaked internal
error into working code. And **`#modify` has its surface** (ADR-0093): a compile-time predicate over an instantiation — `#modify {
return type_info(T).id == type_info(s64).id; }` guards a template in code rather than in a comment. The
block parses and formats; a call is refused (E0274) pending evaluation, because a parsed-and-ignored
predicate would accept calls the author rejected. And **the predicate now runs** (ADR-0095): a `false` refuses that instantiation, so a template enforces its
own constraints in code — `#modify { return type_info(T).id == type_info(s64).id; }` rejects every other
instantiation, with the rejection pointing at the guarded procedure. A predicate that fails to *run* is
deliberately not a rejection. And **`#bake_arguments` has its surface** (ADR-0096): `add_five :: #bake_arguments add(a = 5)` parses, with
its operand a *call* so the named-argument spelling is the ordinary one. Its specialisation — a clone with
the baked parameters dropped, which is literally the machinery `$N` instantiation already uses — is refused
(E0276) pending the last W5 sub-wave. And **the specialisation works** (ADR-0097): the declaration lowers to a *real procedure* — a clone with the
baked parameters dropped and their literals substituted, which is the same machinery `$N` instantiation uses.
**W5 — Polymorphism is complete**, in fifteen sub-waves; **W6 — Metaprogram is open**, then W7 — Stdlib. On top of **`#code`** (ADR-0080), which **completed wave W4 — Comptime** as scoped: `#code { n := 7; }`
is `#insert "n := 7;"` written without quotes, spliced into the enclosing scope. It is deliberately *sugar* —
`#insert` of a named constant already worked, so what `#code` adds is no quoting and a body parsed where it
is written, not a new capability. There is no `Code` *value*, and that is **declined rather than deferred**: a
quoted syntax tree is worth representing only once something can inspect or transform it, and a value that
can only be spliced is what a `string` already is. The same sub-wave refused a **shipped silent miscompile**
found while probing — a pointer or view inside a compile-time aggregate interned the evaluator's own address
as a plain integer, so reading it gave 48 in one engine and a segfault in the other, with no diagnostic — and
turned a third leaked "internal compiler error" into a sentence a reader can act on. On top of **`Type_Info`'s per-kind facts** (ADR-0078): `type_info(T)` now also reports a struct's
field `count` and an array's/pointer's `element` type. The trick that made it a small wave is that these
are *fixed-size* — a count is a number, an element type is a pool id (an `s64` since ADR-0077) — so they
need none of the memory-ownership decision the variable-length field *list* does, and that list stays
deferred. On top of **`Any`** (ADR-0076, ADR-0077): reflection's second half. `any_of(*x)` erases a value —
building an `Any` that carries a `*Type_Info` and a pointer to the value — and `any_as(a, T)` reads it back,
**trapping** unless the type matches. The erasing conversion is allowed only at that boundary: a bare
`cast(*u8, p)` stays refused, because a general pointer cast would make a wrong pointee type a silent wrong
read, the reinterpretation Jairs confines to `union`. Nothing is reinterpreted, because a pointer's bits do
not depend on its pointee, so the conversion emits no code — and neither engine's back end needed a single
line, because `Any` reuses the aggregate, slot and trap machinery already there. The checked read needed a
runtime type identity, and the four-field `Type_Info` had none a *sound* check could use: two `type_info(T)`
calls have different addresses, `size` and `alignment` collide, and `name` is unsound because a local
`Point` and an imported one are different types with one spelling. So `Type_Info` gained a stable `id` — the
type's pool id, the identity the whole compiler already uses and identical in both engines because they
share one pool — and `any_as` compares that. On top of **`type_info(T)`** (ADR-0075): reflection's first half. `type_info(Point)` returns a
`Type_Info` giving a type's kind, name, size and alignment — the numbers coming from the same `layout_of`
every real layout decision uses, so reflection cannot disagree with the layout it describes. The struct is
declared **in `modules/Basic`, in Jairs, not inside the compiler**, because it has to be *spellable*: a
program that reflects must be able to write `info: Type_Info`, and no compiler-declared type can be named at
all — `t: Type;` and `c: Context;` both report "unknown type name". The price is a compiler dependency on a
declaration it does not own, and it is paid honestly: the field names, types and order are validated on
lookup, so editing that struct produces a diagnostic naming the mismatch rather than a read of whatever now
sits at the old offset. Getting here first needed **a constant that may hold a string**, which ADR-0074's own
closing claim said was already done and was not: a compile-time aggregate was copied out of the VM as a flat
byte image, and a `string` field's bytes are a pointer into memory that is gone by the time it is interned.
The image is now a tree reduced *while the VM is alive*. Per-kind detail is still owed — a
struct's field list, an array's element type — each of which needs a memory-ownership decision of its own. On top of **an aggregate compile-time value** (ADR-0074): `V :: #run mk();` where `mk` returns a
struct or an array now works. It interns as its **element values**, not as the
byte image the compile-time VM already had, because the type pool is target-independent and a byte image is
not: interning bytes would put one target's padding and pointer width into a shared table, and a
cross-compile would then read plausible wrong values rather than fail. Each engine turns the values into
bytes itself, at the point that knows which target is meant. A *union* constant is refused, because untagged
storage makes "which field is valid" unanswerable. On top of **`#insert` of a computed string** (ADR-0073): `#insert CODE;` and
`#insert #run build();` evaluate the operand's text at compile time and splice it into the enclosing
scope. This is the point W4 called its top risk — sema and the VM become mutually recursive, because
lowering cannot finish until the operand is evaluated and the evaluator runs on lowered code — and the
cycle is broken by an acyclic pre-pass (`insert_operands`) that reuses the constant evaluator and re-lowers
only the affected bodies, not by fixed-point recovery. The operand is held as an ordinary expression, so
`#insert undefined;` is an unresolved-name error and a non-string operand is a type error, each at the
operand's own span; a pending insert the evaluator has not reached is *refused*, never lowered to nothing,
so a computed insert is diagnosed rather than miscompiled at every step. Building it caught the formatter
silently dropping a computed operand — `#insert CODE;` → `#insert;` — the same lossy-CST failure the
literal wave guarded against. On top of **`#insert` of a literal string** (ADR-0072): `#insert "n := 2 + 3;";`
parses its operand as Jairs source and lowers the statements **where the directive is written** — same
scope, so the next line can read `n`. Every synthesized node's span is the `#insert` itself, because
inserted code has no position in any file and `jr-diag` *clamps* an out-of-range offset rather than
rejecting it, so a synthesized span would silently underline source the user never wrote. Nesting works and
needed no code; it cannot run away because escaping doubles the text at every level, so a written insert is
bounded by its file. On top
of **a type as a compile-time value** (ADR-0071): `T :: Point;` binds a type to a name,
and using a type where a *runtime* value is expected is now an error rather than a silent miscompile.
`t := Point;` used to type-check cleanly and exit 0 in both engines while storing an undefined value into
a slot of a type that has no runtime layout at all — the project's first named failure mode, and only a
MIR dump would have shown it. `type_info()` and `Any` are deliberately a later sub-wave: both make a type
into *runtime data*, which is a different size of problem than a type that only the compiler ever sees.
On top of **an array length that names a constant** (ADR-0070): `N :: 4;  buf: [N]s64;` now
resolves, which ADR-0039 refused for thirty ADRs on an argument that turned out to cover only *part* of
what it forbade — a length needing evaluation still waits for the comptime sub-wave, but one that is
already a literal one name away needs none. That sub-wave's scheduled work, "aggressive const folding",
was found already delivered by const-prop. On top of **`#run` across files and in a body** (ADR-0069),
which **opened wave W4 — Comptime**,
the wave PLAN §5 calls the project's top risk and which is therefore delivered in sub-waves. A `#run` may
now call an imported procedure — the first time this compiler executes a library procedure *while
compiling* — and appear inside a procedure body, where the body receives the computed value. Two internal
compiler errors became actionable messages in the process. On top of `variant`, a tagged union with a
checked read (ADR-0068), which **completed wave W4.5 — Pattern matching**: a write sets the tag, reading a different case *traps* instead of
reinterpreting bits, and `switch` destructures it by case. `union` is untouched and still reinterprets,
which is what makes the variant's check a choice rather than a language-wide cost. On top of `switch`
with exhaustiveness checking (ADR-0067), which **opened W4.5 a wave earlier than planned**: PLAN placed
it after W4 "because exhaustiveness diagnostics want comptime type info" — checking showed that was a
want rather than a need, so the wave moved forward and §2.1 records the amendment. And on top of
traps with backtraces (ADR-0066), which **completed wave W3 — Runtime core**: a
trap now names the procedure frames that were live beneath it, innermost first, and both engines emit
byte-identical bytes — the VM from a shadow stack it resolves against the HIR, native from name pointers
its generated helper walks at trap time. Inlined frames do not appear, because at run time they did not
exist, and ADR-0066 §4 says so rather than reconstructing them. On top of temporary storage (ADR-0065):
`talloc(n)` hands out bytes from a per-context bump arena, valid until `reset_temporary_storage()` — a
feature that composes three prior waves rather than adding machinery. And pointer arithmetic
(ADR-0064): `p + n`, `n + p` and `p - n` on a `*T`, element-scaled and unchecked. And `push_context`
(ADR-0063): a block gets its own copy of the context, restored on exit — the isolation ADR-0057 §2
claimed but never had. And the allocator protocol (ADR-0062): `context.allocator` is a procedure
pointer a program installs in one line, and a callee allocates through it without knowing
which. All on top of `null` and a memory source
(ADR-0060/0061), indirect calls (ADR-0059), the implicit `context` (ADR-0057) and the bounds-check
build setting (ADR-0058, which finished ADR-0003) — on top of
imported constant values (ADR-0055) and a float-constant codegen fix (ADR-0056), `#scope_module`
(ADR-0054) **completing wave W2**, named and default arguments
(ADR-0053), multiple return values (ADR-0052), aggregate returns (ADR-0051),
`using` (ADR-0050), `for` with labelled `break`/`continue` and `defer` (ADR-0049), and the completed
wave W1: operator overloading (ADR-0048), imported enum
members and a refused body that reports instead of crashing (ADR-0047), `xx` autocast with bare
`.RED` (ADR-0046), `union` (ADR-0045), `[]T` views (ADR-0044), `enum_flags` (ADR-0043), the bitwise
operators (ADR-0042), `enum` (ADR-0041), `float32`/`float64` (ADR-0040), `[N]u8` fixed arrays and
bounds checks (ADR-0039), negative literals (ADR-0038) and the integer tower, `cast` and
`print_int` (ADR-0037). 1010 workspace tests; six gates green on macOS arm64 — **locally**, since CI
has never run — plus 166 Neovim checks that are verified rather than gated.

### What you can actually do

| You can | How | Caveat |
|---|---|---|
| Compile and run a program in the comptime VM | `jr run file.jr` | Register bytecode interpreter, no JIT tier |
| Control a struct's layout | `x: s64 #align 16;`, `y: s64 #place 32;` | Raise a field's alignment, or put it at an exact byte offset (ADR-0144). `#align` is a *minimum*, a power of two up to 4096; `#place` takes any non-negative offset, may be unaligned, and **may overlap another field** — that is the point, and nothing checks for it, exactly as an untagged `union` reinterprets bits. A placed field never moves the ones after it. The operand is a literal or a named constant; arithmetic needs the compile-time evaluator, which runs after a struct is laid out |
| Choose a code generator | `jr build file.jr --backend llvm` | Cranelift by default and LLVM 21 on request (ADR-0143). The LLVM path needs a compiler built with `--features llvm`; without it the flag is refused with a message naming the feature, rather than reported as unknown. The three engines are held to agreement by the differential harness — all 114 corpus programs and every hand-tried trap matched the VM on the first run |
| Compile to a native executable | `jr build file.jr -o out` | arm64 macOS verified. x86-64 Linux is **configured in CI and has never run** — the workflow exists, and no CI run has ever happened on this repository, so Linux is entirely unverified. A declared `BUILD_OUTPUT` is confined to the working directory (ADR-0122) |
| Build without bounds checks | `jr build file.jr --no-bounds-check`, or `jr run` | ADR-0003's build setting, finally wired (ADR-0058). An out-of-range index is then undefined behaviour, which is the trade. `#no_abc` on a procedure does the same locally, whatever the build says; compile-time execution checks regardless |
| Choose an optimisation level | `jr build file.jr -O0`, or `jr run -O0` | Two levels, `0` and `1` (the default, and what every build did before the flag). `-O0` runs no mid-end pass, so the code executed is exactly what lowering produced — which is how a wrong answer becomes attributable to lowering rather than to a pass. A level may **not** change what a program computes, and the differential harness now sweeps every corpus program at both levels to check it (ADR-0142). The one thing `-O0` does change is a backtrace: nothing is inlined, so a trap inside a leaf names the leaf's own line. There is no `-O2` yet and no `--release` — deliberately, since a level with no pass behind it is a promise rather than a flag |
| Get rustc-grade diagnostics | `jr check file.jr` | 115 codes across lexer, parser, HIR, sema, MIR and const-eval, with cross-crate uniqueness enforced by a test (ADR-0123). E0218 and E0212 suggest a near name; E0231 and E0245 are *warnings* — an unused `#import`, and a body the compiler could not lower |
| Format source canonically | `jr fmt [--check] paths…` | The corpus is canonical under it, enforced by gate 5 — locally, since CI has never run |
| Inspect tokens or the CST | `jr parse file.jr` | Debug aid |
| Measure language-server latency | `jr bench file.jr` | Reports min/median/p95 cold, warm and after an edit. **Reports, never judges** — no threshold, not a gate (ADR-0033), so a performance regression is invisible to CI by construction |
| Measure compile throughput | `jr bench --throughput paths… ` | Lines and bytes per second for `check` and `build`, cold only — a compiler is a process, so there is no warm throughput to report (ADR-0146). Same contract: reports, never judges. The published figure is above, with the machine beside it |
| Print a number | `print_int(n)` from `modules/Basic` | Written in Jairs, and still recursive — both the `[N]u8` buffer and the `[]u8` view it wanted now exist, so nothing in the language is missing; converting it is its own change. Traps on the most negative `s64`, which cannot be negated (ADR-0002). Executed by `valid/101`, which until ADR-0125 nothing did |
| Call libc from Jairs | `#foreign` / `#system_library` | Through libffi at run time (refused at comptime, ADR-0006). `modules/Basic` binds `write`, `exit`, `malloc`, `free`; the VM satisfies `malloc`/`free` from its own region (ADR-0061) so a pointer round-trips there too |
| Fold a compile-time call | `COMPUTED :: #run add(2, 3)`, or `n := #run add(2, 3)` in a body | Nested calls, arithmetic around a call, a loop in the callee and an **imported** callee all work (ADR-0069). Still refused: a `#foreign` call (ADR-0006), an operator overload, a default or named argument, and reading another file's constant — all because const-eval precedes the check phase |
| Import a module | `#import "Basic";` | One module = one file, flat imports, cycles legal. Procedures, types, enum members and **constants' values** all cross the boundary; an imported struct's *fields* do not, so `using` on one is refused. `#scope_module` hides a declaration from importers, and `modules/Basic` uses it for its own internals |
| Edit in Neovim, with highlighting, diagnostics, hover, goto-definition, completion, rename, code actions, signature help and inlay hints | `editors/nvim/` | Two lines in `init.lua` and one build script; no plugin manager. Neovim **0.11+** — every capability is on a stock 0.11 default binding (`K`, `gd`, `gra`, `grn`, `grr`, `gO`, `<C-s>`), so there are no keymaps to add. Works on a standalone `.jr` file too, not only inside a checkout. See [`editors/nvim/README.md`](editors/nvim/README.md) |
| Use any other LSP editor | `jr lsp` | Speaks LSP 3.17 over stdio. The repository packages for Neovim only and **will not ship a VS Code extension** (ADR-0036) — point your client at the command yourself |

### The language today

Everything in the left column is implemented end to end — parsed, formatted,
type-checked, lowered, executed in the VM, compiled natively, and asserted equal in
both engines. Everything in the right column is absent, with the wave that adds it.
The authoritative version of this list is
[`docs/spec/00-overview.md`](docs/spec/00-overview.md).

| Works | Absent (wave) |
|---|---|
| `s8 s16 s32 s64`, `u8 u16 u32 u64`, `bool`, `string`, `*T`, `null` | pointer *difference* `p - q`; unchecked, so past-end is UB |
| `float32`, `float64` — plain IEEE-754, no traps | `%` on floats, `is_nan`, math intrinsics (**W7**) |
| `cast(T, x)` between any two numeric types, and `xx` where the context gives the type | pointer conversions — `xx` is no more powerful than `cast` |
| `#simd [N]T` — a vector at one of the six register widths, with elementwise `+% -% *%` (integers) and `+ - * /` (floats), lane indexing and `.count` | wider or narrower than one register; integer `/`; comparisons, which need a mask type; swizzles |
| `struct { … }`, one level, nominal, with `#soa(N)` for one-array-per-field storage and `e[i].x` access (ADR-0147), and per-field `#align N` (a minimum, power of two up to 4096) and `#place N` (an exact byte offset, may overlap and may be unaligned) — ADR-0144 | a struct-level `#align`; any packing form; `#align` on a local or a procedure; an operand needing evaluation; a bare `e[i]` on an `#soa` struct, and `using` inside one |
| `union { … }`, nominal, **untagged** — every field at offset 0, so a cross-field read reinterprets | |
| `variant { … }` — a tagged union: a write sets the tag, reading another case **traps**, `switch` destructures it (ADR-0068) | a recursive variant; one in a `#foreign` signature; eliding the check inside a matching arm |
| `enum { RED; GREEN :: 5; }`, nominal, namespaced members, and bare `.RED` from context — including as a `switch` case (ADR-0067). A member's value may **name a constant** whose initialiser is a literal, and auto-numbering continues from it (ADR-0129) | a value needing evaluation (`2 + 2`, a `#run`, another file's constant); a member naming a **sibling** member |
| `enum_flags { READ; WRITE; }` — powers of two, combines with `& \| ^ ~` | building one from a computed integer (`cast(Perm, 3)` is refused) |
| procedures, one result or several: `-> (s64, bool)`, `q, ok := f();`, `_` to discard | `#must` (its own ADR); a multi-result call as a `return` operand |
| a procedure as a **value**: `f := add`, a `(s64, s64) -> s64` parameter or **struct field**, `f(...)` calls through it; `(T)` with no arrow for a void return | a cross-file or `#foreign` procedure value; comparing or printing one; a `#c_call` proc-pointer type |
| named arguments `f(b = 2, a = 1)` and literal defaults `(b: s64 = 10)` | a non-literal default; a named argument on a cross-file call, or in a `#run` |
| `::` constant, `:=` inferred, `: T = v` typed, `---` uninit | |
| `if` / `else if` / `else`, `while`, `return` | |
| `switch e { case v; … else; … }` over an enum or an integer, **exhaustiveness-checked** for an enum, no fallthrough (ADR-0067) | patterns, ranges, guards; a multi-value `case`; `switch` as an expression |
| `for x: buf`, `for x, i: buf`, `for i: 0..n`, `for < x: buf`; over arrays, views and ranges | iterate-by-reference `for *x`, a range as a value, `for` over a user type (**a later wave**) |
| `break` / `continue`, labelled (`break outer`) or not; `defer` at every scope exit | |
| `using p: Point` promotes a struct's fields; `using base: Point;` embeds them, transitively | `using` on an enum, a module, or an **imported** struct |
| blocks and block scope, shadowing | |
| `#scope_module` / `#scope_export` — module-private declarations, exported by default | `#scope_file` (indistinguishable while a module is one file); re-export |
| `+ - * / %` trapping, `+% -% *%` wrapping, unary `-` | |
| `& \| ^ ~ << >>`, **non-C precedence**, trapping shift count | `transmute` — though a `union { f: float64; bits: u64; }` reads a float's bits |
| `== != < <= > >=`, `&& \|\| !` short-circuiting | |
| `operator + :: (a: Vec2, b: Vec2) -> Vec2` — arithmetic and comparison, one operand local, and it may return a struct | unary, `[]`, `()` and compound-assignment overloading; an overload in a `#run` |
| `=` and compound `+= -= *= /= %= +%= -%= *%= &= \|= ^= <<= >>=` | |
| `a.b.c` field access, auto-deref through pointers | dynamic arrays `[..]T` (**a later wave**) |
| `[]T` views: `buf[]`, `xs[i]`, `xs.count`, writes through to the array, **returned from a procedure** | sub-slicing `buf[1..3]`, `==` on views |
| `[N]T` fixed arrays: `a[i]`, `.count`, zeroed by default, bounds-checked — and `#no_abc` or `--no-bounds-check` to stop checking. `N` may be a literal or a **named constant** (ADR-0070) | a length needing evaluation — arithmetic, `#run`, a chain, or another file's constant; array literals `[1, 2, 3]`; a per-*index* `#no_abc` |
| calls, nested; a discarded call is a statement | |
| integer literals (dec/hex/bin/oct, `_`), string literals + escapes | |
| float literals: `1.5`, `1e9`, `1.5e-3`, `1_000.5` | float *printing* — `print_int` has no counterpart |
| nesting block comments; `///` and `//!` doc comments, shown on hover | doc generation (`jr doc`) — nothing consumes docs but the language server |
| `#run` at file scope or in a body, calling local or **imported** procedures, with loops and nested calls; bounded by a **step budget** (ADR-0121), so a non-terminating one reports E0230 rather than hanging the compiler | a `#run` reading **another file's constant**; a `#foreign` call (ADR-0006); an operator overload, a default or a named argument |
| a `#run` returning a **struct or array**, interned as its element values and materialised by both engines (ADR-0074), including one holding a **string** (ADR-0075) | a `#run` returning a **union** — untagged storage makes "which field is valid" unanswerable; a struct or array *literal* (`P.{1, 2}`), which is a separate syntax question |
| **`type_info(T)`** — a type's kind, name, size, alignment, a stable `id`, and the fixed-size per-kind facts `count` (a struct's field count / array length) and `element` (an array's element / pointer's pointee, as a type id); `Type_Info` is declared in `Basic` and validated on lookup (ADR-0075, ADR-0077, ADR-0078) | the variable-length **field list** — the elements need the program's lifetime, so it is a static-data-vs-comptime-table decision; following an `element` id back to a `Type_Info`; `type_info([4]s64)`, blocked on structural type aliases |
| **`Any`** — `any_of(*x)` erases a value to a `{*Type_Info, *u8}` pair, `any_as(a, T)` reads it back and traps unless the type's `id` matches; the erasing pointer conversion is allowed only at that boundary (ADR-0076) | a bare **value** coercing to `Any` implicitly (a literal has no address, so it needs a materialised temporary — the *pointer* form `takes(*x)` is done); an `Any` in a compile-time constant |
| `#insert "…"` of a **string literal**, lowered where it is written — same scope, so a local it declares is visible after it; nesting works, and every diagnostic points at the directive and names its offset into the inserted text (ADR-0072) | `#insert` at file scope, which would change the item tree; `#code` and the `Code` type |
| `#insert <expr>;` of a **computed** operand — a constant or a `#run` whose text is evaluated at compile time and spliced (ADR-0073). The operand resolves and type-checks like any expression (`#insert undefined;` → E0201; a non-string → E0214), and a pending insert the evaluator has not reached is refused, never miscompiled. This is where sema and the VM become mutually recursive; the cycle is broken by an acyclic pre-pass | a **cross-file** `#run` value (its own decision, ADR-0073 §4); expansion past 16 levels (E0264) |
| **`#code { … }`** — unquoted source spliced into the enclosing scope; the body is parsed where it is written, so no quoting and no escaping (ADR-0080). Deliberately *sugar* over `#insert`, reusing its depth bound and its refusal of a pending splice | a `Code` **value** — **declined**, not deferred: a quoted syntax tree is worth representing only once something can inspect or transform one (ADR-0080 §3); spans into the body's real source, so a fault inside it points at the `#code` |
| **`$T` polymorphic procedures** — inferred from the argument (directly or through `*$T`/`[]$T`), instantiated once per distinct tuple of bound types, checked per instantiation, run as ordinary procedures in both engines (ADR-0081–0084). A template may call another template: expansion iterates to a fixed point, so a clone body's own polymorphic calls are redirected too (ADR-0120) | two-way unification and explicit type arguments; a **cross-file** instantiation (E0268 — the workaround is a wrapper the module instantiates itself) |
| **polymorphic structs** — `Box :: struct($T) { value: T; }` used as `Box(s64)`; the instance is keyed on `(decl, args)` so `Box(s64)` and `Box(bool)` are distinct types with substituted fields and layouts, told apart in the pool the way `[2]s64` and `[3]s64` are (ADR-0085). Both engines compute each instance's layout from its substituted fields. A parameterised struct **crosses a module boundary** (ADR-0117): the importer resolves its fields from the declaring file's HIR, and identity stays the declaring file's | inferring a struct's argument through a `$T` parameter (`(b: Box($T))`, E0212); `using` on a parameterised struct; recursive `List($T)` |
| `talloc(n)` / `reset_temporary_storage()` — a per-context bump arena, valid until reset, no per-piece free (ADR-0065) | — its `*u8` is now storable at a wider type through `typed(T, p)` (ADR-0106), so the old "needs a pointer cast the language does not have" no longer applies; aligned `talloc` and a configurable region size are still owed |
| **`[N]T` sized by a `$N` comptime parameter** (ADR-0089) — `buf: [N]s64` inside a `$N` procedure; each instantiation gets its own array type and layout from the baked value, so two calls at 4 and 3 give a `[4]s64` and a `[3]s64` from one declaration. The value reaches sema through the HIR already interned, so sema still runs no evaluator | a length needing *arithmetic* (`[2 + 2]u8`), or one naming a constant from another file — both ADR-0070's own deferrals |
| **`$N` comptime-value parameter and instantiation** (ADR-0087, ADR-0088): `make :: ($N: s64)` called as `make(5)` evaluates the argument to a compile-time constant and appends a concrete procedure with `N` baked into the body; two calls at the same value dedupe, distinct values instantiate separately (ADR-0005 extended to values). Mixed comptime and runtime parameters — `scaled :: ($N: s64, factor: s64)` — pass only the runtime one at the call site | `[N]T` where `N` is a `$N` parameter (small, next); a non-constant argument is refused E0271; a mixed `$T`+`$N` template falls through with an honest mismatch |
| a **type as a compile-time value**: `T :: Point;` binds one, and `T` is usable wherever `Point` is — as an annotation, a parameter, a field, an array element, a pointee; an enum alias carries its members (ADR-0071) | a chain (`B :: A`); a `Type` parameter; `Type` as an annotation, which does not parse. Comparing types has an idiom rather than an operator — `type_info(T).id == type_info(s64).id`, which is what `#modify` predicates use — so `T == U` is now sugar nobody has argued for rather than the open design question ADR-0071 §5 called it |
| using a type where a **runtime** value is expected is refused (E0261) — it has no runtime representation, so there is nothing to store | — |
| `#import`, `#foreign`, `#system_library`; `#expand` macros that splice; `#modify` predicates; `#bake_arguments` specialisations | — |
| `@note` metadata on a declaration — `@deprecated`, `@requires "x"` (ADR-0098) — read at compile time by `has_note` / `note_value` (ADR-0099), and queried without naming by `noted_count` / `noted_name` (ADR-0100), and used to **generate code** for each noted declaration by `noted_insert` (ADR-0101) | run-time **inspection**: a loop reading declarations as values, which needs a compiler-emitted table and lifts `Type_Info`'s field list (**W6**) |
| overflow traps with a source location (ADR-0002, ADR-0020), and a **call chain** of the frames that were live (ADR-0066) | a per-frame line number; inlined frames, which have no runtime existence |
| `context` — a hidden parameter passed by pointer, so a callee reads what its caller wrote; `#c_call` opts out and gets none | — |
| `push_context { … }` — a block with its own copy of the context, so a write inside it is restored on exit (ADR-0063) | — |
| `context.allocator` / `.allocator_free` / `.allocator_data` — install an allocator, and a callee allocates through it without knowing which | a `#foreign` procedure installed directly (wrap it) |
| `p + n`, `n + p`, `p - n` on a `*T` — element-scaled, unchecked; a bump allocator advances a pointer (ADR-0064) | `p - q` (deferred); `p[n]` sugar; pointer ordering `< >` |
| `talloc(n)` / `reset_temporary_storage()` — a per-context bump arena, valid until reset, no per-piece free (ADR-0065) | — its `*u8` is now storable at a wider type through `typed(T, p)` (ADR-0106), so the old "needs a pointer cast the language does not have" no longer applies; aligned `talloc` and a configurable region size are still owed |

ADR-0008 chose Jai's **error model** — several return values plus `#must` — and the first half now
exists: a procedure returns a value and a flag, and the caller must name both. `#must`, which makes
ignoring the flag a compile error, is owed its own ADR. There is no GC and no RAII, which is a design value rather than a missing feature.

### Compiler internals

| Stage | Status | Honest note |
|---|---|---|
| Lexer, parser, CST, typed AST | **Works** | Hand-written, error-recovering, trivia-preserving. Doc comments are trivia, so they cannot change what parses (ADR-0027) |
| Formatter | **Works** | Pure function over the CST |
| HIR, name resolution, module loader | **Works** | Flat import merge (ADR-0014) |
| InternPool (types, comptime values, layout, arithmetic) | **Works** | One layout computation and one integer evaluator, shared (ADR-0018 §2, ADR-0022 §2) |
| Sema (signatures, checking, inference) | **Works** | E0212–E0279; a union's diagnostics are a struct's unchanged, deliberately, and a bare `.RED`'s "no such member" is the qualified form's; no const-eval here — ADR-0018 §3 puts it in the VM, which is why an array length must be a literal. Float literals are context-typed with **no** fit check, because IEEE-754 saturates (ADR-0040 §5) |
| MIR (typed SSA, Braun construction) | **Works** | Block parameters, not phis (ADR-0017); CFG diagnostics E0227–E0229, the last of which now also reports a `break`/`continue` naming an unknown label (ADR-0049 §2); an explicit `bounds_check` statement and an explicit `zero`, both ADR-0039. `for` reuses the `while` shape with a synthesised induction variable and needs no new node; `defer`'s statements appear once per exit path |
| Mid-end | **Four passes** | Inliner, store-to-load forwarding, const-prop, DCE, to a bounded fixed point (ADR-0021 – ADR-0023), and all four are skipped at `-O0` (ADR-0142). The inliner takes **non-leaf** callees and refuses recursive ones so their backtraces survive (ADR-0145); forwarding follows a **single-predecessor chain** across blocks, but a join still ends it, so a value read across a loop stays in memory. It refuses two unequal array indices as possibly-aliasing. No SROA — that needs a new `Rvalue` extracting a field from an operand, not a pass; the SSA value arena is never compacted |
| Bytecode VM + libffi | **Works** | Per-instruction spans, so a trap names its line. Floats need no new value variant, but are dispatched *before* the bit-compare fallback that would answer `NaN == NaN` and `-0.0 == 0.0` backwards. No JIT |
| Cranelift back end + linker | **Works** | Returns an aggregate through a caller-allocated `sret` pointer, uniform by size (ADR-0051) — a register fast path is W8's, because the size threshold and field classification are platform-specific and a wrong guess is garbage with no diagnostic. Carries the context as a second hidden parameter, after `sret` and before the declared ones, so one shared predicate computes an offset of 0, 1 or 2 (ADR-0057 §4). Calls through a procedure pointer with `func_addr` + `call_indirect` (ADR-0059 §4). Still refuses an aggregate crossing a `#foreign` boundary in either direction |
| salsa incremental database | **Works** | Built *and* optimized MIR staged (ADR-0021 §1); invalidation is at file grain |
| Differential harness | **Works** | Compares stdout, stderr and exit status of the engines as subprocesses; each corpus program against **itself** at both optimisation levels — the check that says the mid-end preserves meaning (ADR-0142 §3) — and, under gate 7, **three-way**: VM ≡ Cranelift ≡ LLVM (ADR-0143 §8) |
| LLVM back end | **Works** | `jr build --backend llvm` (ADR-0143), behind a default-off `llvm` cargo feature and gate 7. MIR → LLVM IR directly: block parameters become `phi`s, every offset is a byte GEP so no Jairs aggregate gets an LLVM struct type, and overflow/shift/division/float→int all go through checks or saturating intrinsics because LLVM's plain operators are poison or UB where Jairs traps. No LLVM optimisation passes: `-O` selects the mid-end only |
| Language server | **Works** | `jr lsp`, twelve capabilities: diagnostics, hover, goto-definition, completion + resolve, references, documentHighlight, rename (workspace-wide, refuses rather than half-renaming), documentSymbol, workspaceSymbol, code actions, `signatureHelp`, inlay hints (ADR-0024, ADR-0028, ADR-0030, ADR-0031). Dispatches a read only after every write, because the reverse silently lost `didOpen`'s diagnostics (ADR-0032). No semantic tokens |
| Neovim integration | **Works** | `editors/nvim/` (ADR-0025), verified against the real editor by a **166**-check script — **not** by CI, which has no Neovim |
| VS Code integration | **Will not be built** | ADR-0036: the maintainer does not use it, and a packaging target for an unused editor rots. `jr lsp` is editor-agnostic, so any LSP client works |
| Compilation driver / workspaces | **Partly** | `jr-driver` is still a one-line stub; the workspace *file list* exists in `jr-db::workspace` (ADR-0029): the search paths plus the root tree, walked and watched, bounded at 10 000 files |
| Debug info | **Not started — now W12** | No DWARF at all; a native binary is not debuggable. This row was right and the plan's was wrong, which ADR-0159 §7 corrected: a `gimli` writer is owed in *both* back ends, and locals need `ValueLabel`s the Cranelift lowering does not emit |
| Compile throughput | **Measured** | `jr bench --throughput` (ADR-0146). 113 k lines/s checking and 26 k building, on the machine named in the status section — `build` is 4.4× `check`. Not a gate, and not compared against anything: this is the first number, so there is no trend yet |
| Optimisation levels | **Two** | `--opt-level` takes `0` or `1` (short `-O`) on `jr run` and `jr build`, defaulting to 1 = the pipeline (ADR-0142). `-O0` runs no mid-end pass and is asserted to leave every body byte-identical to what lowering produced. No `-O2` and no `--release`, both deliberately: a level with no pass behind it is a promise, and `--release` is a bundle that would re-couple the safety setting ADR-0058 unbundled. The level does not reach a *back end* yet — Cranelift's own optimisation level is untouched, and selecting a back end is the LLVM sub-wave's business |

### Things it is easy to over-read

- **A flags enum's combination names no member, and that is the design.** `Perm.READ |
  Perm.WRITE` is 3, which no member has. The type's job is keeping a *set* distinguishable from
  an integer — so a `Perm` stays a `Perm` through `& | ^ ~` — not naming every subset. Testing a
  flag is `(f & Perm.READ) == Perm.READ`, which is the idiom Jai uses and which composes:
  `f & (A|B)` tests two at once, where a binary `has` operator would not.
- **`enum_flags` numbers by powers of two, and the continue-from-here rule has two ways to go
  wrong.** After an explicit `B :: 8` the next flag is 16 — the next power of two above the
  *value*, not above the member's index. And that holds when the previous value is not itself a
  power of two: after a named mask `AB :: 3` the next flag is 4, not 6. An explicit `NONE :: 0;`
  leaves the sequence undisturbed, and zero is never created for you.
- **A plain `enum` still refuses `|`**, deliberately (ADR-0043 §4). If bitwise worked on both
  forms the declaration would carry no information, and the numbering difference alone would
  separate a set from an alternative — which is how `READ|WRITE` silently colliding with a
  member becomes possible. The diagnostic names `enum_flags`, because a reader who has not met
  the form cannot find it.
- **There is no way to build a flags value from a computed integer.** `cast(Perm, 3)` is
  refused, and the hole it closes is wider for flags than for a plain enum: *most* integers are
  valid flag sets, so a wrong one would look right. Members are combined with `|` instead.
- **Bitwise operators bind tighter than comparison, which is *not* C's ordering.**
  `flags & MASK == 0` means `(flags & MASK) == 0`. C reads it as `flags & (MASK == 0)` —
  something Ritchie described as a mistake kept only for compatibility with pre-`&&` C, and
  which Go, Rust and Zig all changed. Shifts sit between `+` and `*`, so `a + b << c` is
  `a + (b << c)`; C puts them below `+`. Under C's ordering Jairs would *refuse* a line that
  reads correctly, because `flags & bool` is a type error here rather than a wrong answer.
- **An out-of-range shift count traps.** `x << 8` on an `s8` traps, and so does a negative
  count. This is ADR-0002's rule applied to a new operator: masking to the width is what x86
  does natively and would silently turn `<< 8` into `<< 0`, and saturating to 0 costs the same
  branch while turning a likely bug into an answer. The shift's *result* is not checked —
  `1 << 7` in an `s8` is -128, because that is exactly the bits requested.
- **`>>` is arithmetic for a signed type and logical for an unsigned one**, decided by the
  type exactly as `/` chooses between `sdiv` and `udiv`. There is no `>>>`: a program that
  wants the bits without the sign casts to the unsigned type of the same width.
- **Bitwise operators are integers only.** `1.5 & 2.5` is refused, because a float's bits are
  a sign, an exponent and a mantissa — ANDing two of them is the AND of nothing meaningful.
  There is also **no way to read a float's bits**: `cast` converts values, not
  representations, so a bit-level float inspection needs an operation Jairs does not have.
  `Colour.RED | Colour.GREEN` is refused too, and that refusal is what `enum_flags` will lift.
- **An enum is nominal, and `Colour.RED` is the only way to name a member.** `Colour` is not
  `s64`: a bare integer cannot be passed where an enum belongs, and `cast(s64, c)` is how the
  number is obtained. Members are namespaced and never enter the enclosing scope, so adding one
  cannot shadow an existing name — C's rule would be worse here than in C, because ADR-0014's
  flat import merge would let an imported enum's members enlarge the name space every
  identifier resolves against.
- **Bare `.RED` works, and its last owed decision is now taken.** `c: Colour = .RED;` landed with
  `xx` autocast (ADR-0046); ADR-0041 §2 listed five steps it needed, and the fifth — "a decision
  about `switch`" — was owed until ADR-0067 made `case .RED` legal. It asks the context for a
  *namespace to resolve a name in* rather than a type to give an untyped value, which is why it was
  a resolution rule rather than a new literal. `Colour.RED` stays valid, so both spellings work and
  the corpus proves they compile to identical MIR.
- **An enum's numbering is Jai's, including the part that surprises people.** Members
  auto-number from 0, an explicit value is allowed, and **later members continue from it** —
  `enum { A; B :: 10; C; }` is 0, 10, 11, not 0, 10, 2. Duplicate values are legal. Ordering
  and arithmetic are refused: with auto-numbering `Colour.RED < Colour.GREEN` would be true by
  an accident of declaration order, which is a fact about the source file rather than about
  colours.
- **An enum declared in an imported module cannot be used from another file yet.** The member
  lookup handles a local declaration only, because an imported enum's arena index belongs to
  the other file — the same cross-file restriction an imported *constant* has (ADR-0017 §3).
- **Floats do not trap, and that is a scoping of ADR-0002 rather than an exception to it.**
  `1.0/0.0` is `inf`, `0.0/0.0` is `NaN`, and an overflowing multiply saturates. Integer
  overflow traps because an overflowing `+` produces a result the program did not ask for;
  IEEE-754 *defines* `inf` as the answer, so there is nothing to refuse (ADR-0040 §1). The
  consequence that surprises people: `==` is not reflexive, because `NaN == NaN` is false.
  There is no `is_nan` yet, so the check is spelled `x != x`.
- **`NaN == NaN` and `-0.0 == 0.0` are the two answers a raw bit compare gets wrong**, in
  opposite directions — identical bits for the first, different bits for the second. The VM
  has a bit-compare fallback for `bool` and pointer equality, and a float reaching it would
  answer both backwards. That is a *plausible wrong answer* rather than an error, which is
  why floats are dispatched before it and why a corpus file pins both values in both engines.
- **A `float32` operation is computed at `float64` precision in the VM.** `jr-pool` does the
  arithmetic in `f64` and rounds once at the end, while Cranelift emits native `f32`
  instructions throughout. That is a double rounding and it is visible in the last bit of
  some results. The two engines are held equal by `differential.rs` rather than by
  construction, so a case that disagreed would be a real finding rather than a surprise.
- **There is no implicit conversion between an integer and a float**, in either direction.
  `1 + 1.5` is a type error and so is `some_s64 + some_float64`; `cast` is the only way
  across, exactly as it is between integer widths. Stricter than C, and the same strictness
  ADR-0016's rules already had — one implicit conversion would make the float the only type
  that silently changes another's meaning. The exception that is not a conversion: an untyped
  *literal* takes its context's type, so `1.5 + f32_value` works while `1 + f64_value` does
  not, because `1` is an integer literal.
- **A float→int cast saturates rather than wrapping or trapping.** `cast(s8, 1000.0)` is 127
  and `NaN` is 0. C makes this undefined behaviour and Cranelift offers both a trapping and a
  saturating instruction; saturation is chosen because it is total, so every float has an
  answer in every integer type and there is no trap to add to a path that has none
  (ADR-0040 §4). Rust made the same change for the same reason.
- **A float literal that does not fit `float32` is not an error.** `x: float32 = 1e300;` is
  `inf`. This differs from `x: u8 = 300;`, which *is* E0204, and the difference is that there
  is no integer `inf` to saturate to — an integer literal that does not fit has no answer,
  while a float literal always has one.
- **A write to `context` is visible downward and not upward, and there is no scoped form.**
  The context is passed *by pointer*, so a callee reads what its caller wrote — that is the whole
  point of it (ADR-0057 §2). But `f` setting `context.allocator` and returning leaves the value set
  from its caller's view too, because they share one object. Jai's `push_context` is the form that
  isolates a callee, and it does not exist here: it introduces a scope, which interacts with `defer`
  and deserves its own decision. `tests/corpus/valid/046-context.jr` asserts the *current* behaviour
  rather than the intended one, and says so.
- **`context.allocator` is an allocator now, and it starts null.** ADR-0062 replaced ADR-0057's `s64`
  placeholder with two procedure pointers and a state word. `main`'s context is zeroed, so an
  uninstalled allocator is a **null procedure pointer and calling through it traps** — the honest
  failure for a configuration error, where returning null would make every allocation site check for
  a mistake that is not an out-of-memory one. A program installs one in a line:
  `context.allocator = my_alloc;`. Installing libc's automatically in the entry stub was rejected:
  it would make `modules/Basic` a dependency of the runtime, which a freestanding target cannot
  satisfy.
- **A `#foreign` procedure cannot be installed directly** — `context.allocator = malloc` is E0256,
  because a `#foreign` type is `ContextKind::CCall` and a proc-pointer type is always `Jairs`. The
  wrapper is one line and is the required shape. Before this wave the imported case reported
  *"expected `(s64) -> *u8`, found `(s64) -> *u8`"* — two identical types, because the difference is
  invisible.
- **A `#c_call` procedure cannot call a Jairs one.** It has no context to pass, so the body is
  refused with a message rather than having one invented for it: a boundary that silently
  manufactured a context would hide where it came from. The other direction works.
- **The bounds check can be turned off, and `#no_abc` is on the procedure rather than the
  index.** ADR-0003 decided in the *slice* that bounds checking is a build setting carried as
  an explicit MIR operation strippable by one pass, with a local opt-out **at an individual
  index**. The operation landed with arrays (ADR-0039) and the pass and flag never did, which
  §1.5 said in the same words for eleven waves. ADR-0058 built both, and moved the opt-out to
  the procedure header — a per-index flag would have to reach `Projection::Index` through
  eleven passes and both back ends, and one some of them ignored would be a check silently
  restored or silently dropped. `--no-bounds-check` is on `jr run` and `jr build`, not on
  `jr check`, because checking reports diagnostics from *built* MIR that the pass never
  touches.
- **An out-of-range index with the checks off is undefined behaviour, by construction.** That
  is what the flag buys, and it is why no corpus program exercises it: a test asserting what
  `buf[9]` produces would be asserting a fact about this machine's stack. What *is* tested is
  that a valid program's answer is identical either way, in both engines — a build setting
  that changed an answer would be a miscompile.
- **Compile-time execution always checks, whatever the build says.** `#run` on an out-of-range
  index is an error even under `--no-bounds-check`. This falls out of const-eval reaching MIR
  by a path that never runs the strip pass, and it is also the right answer: a trap at compile
  time is a *diagnostic* rather than a program behaviour, so stripping the check there would
  fold garbage into a well-typed constant instead of reporting it (ADR-0058 §4).
- **An array is zeroed; a scalar declared without an initialiser is not the same thing.**
  `buf: [20]u8;` zeroes, `buf: [20]u8 = ---;` does not. The difference from a scalar is
  deliberate: MIR tracks definedness per *slot*, so treating an array like a scalar would
  make the first partial write an uninitialised read of the whole array (ADR-0039 §4).
- **A default-initialised `struct` used to read stack garbage natively.** `p: Point;` emitted
  no zeroing at all, above a comment saying that was codegen's job. Neither back end did it:
  the VM zeroes a fresh frame, so it looked right there, while Cranelift's stack slot is
  uninitialised — the same program exited 0 in the VM and 184, then 200, natively. Fixed by
  ADR-0039 §4a. It hid because `differential.rs` compares observable output and nothing in
  the corpus observed one.
- **An index trap names the line but not the index.** `TrapKind::reason()` is a
  `&'static str` and native code raises a trap by handing a helper a pointer to a constant
  string, so there is no formatting step to interpolate a runtime value into. Naming the
  value means a formatting trap helper, which applies to every trap kind at once and is a
  better change than a special case for this one (ADR-0039 §2).
- **An array length must be a literal.** `[20]u8` works and `[COUNT]u8` does not, and it is
  not a preference: constant evaluation lives in `jr-db` over the bytecode VM (ADR-0018 §3),
  *downstream* of where a type annotation is resolved, so sema cannot ask for `COUNT`'s
  value without inverting that dependency. E0233 says so rather than resolving it wrongly.
  It becomes possible in W4, the wave that makes sema and comptime mutually recursive.
- **There is still no published *compile-throughput* number.** ADR-0019 §6 says a number
  taken without a mid-end measures the missing mid-end; the mid-end now exists, so one is
  finally honest to take, and it has not been taken. What *has* been measured is
  language-server latency (`jr bench`, ADR-0033) — a different question, and no substitute.
- **The latency numbers, so they are not overstated.** On a synthetic 36 000-line, 302-file
  workspace: every operation is under **1 ms** cold except `references` and `rename`, which
  cost **55 ms** because they parse the workspace, and `workspace_load` at **41 ms**. A
  40-line corpus file puts everything under 0.6 ms. These are one machine, one synthetic
  tree, and a floor rather than a promise. `jr bench` also reports two rows that are not
  client requests — `parse_all_files` and `resolve_all_files` — because they are what turned
  "references is slow" into "parsing is slow" (ADR-0034).
- **The two engines agreeing is *tested*, not assumed.** They share MIR, which makes
  agreement likely; `crates/jr-cli/tests/differential.rs` is what makes it checked.
  Both of this project's silent miscompiles were places where a plausible argument
  stood in for a check.
- **Only two of twenty executable corpus programs print anything**, so the corpus
  differential largely compares silence with silence. That is why it also drives
  computations out through `exit` — arithmetic, precedence, loops, block parameters,
  pointers, struct offsets and both traps.
- **A cross-file `#run` reading another file's *constant* does not work** — the callable half
  shipped in ADR-0069, so a `#run` may call an imported procedure. It is only reading an
  imported constant's value that stays refused, and ADR-0021 §2 depends on that narrower
  fact. This bullet used to say a cross-file `#run` did not work at all, which stopped being
  true ten ADRs before anyone corrected it.
- **The integer tower cost almost nothing, and that is a fact about the code rather than
  luck.** `jr-pool`'s `IntKind` was already generic over width and signedness, both back ends
  already read it that way, and interning is structural — so `s8`..`u64` is eight names mapped
  onto an existing representation (ADR-0037 §1). `float32/64` is the part that is genuinely
  missing, because it needs a new value representation everywhere.
- **`cast` truncates and does not trap.** ADR-0002 makes integer *overflow* trap, because an
  overflowing `+` produces a result the program did not ask for; a narrowing cast is the program
  asking for the low bits. A narrowing cast of a *literal* is still a compile error, reusing
  E0204 (ADR-0037 §2).
- **A signed minimum is now writable, and was not before.** `a: s8 = -128;` used to be
  rejected by a diagnostic that printed "the range of `s8` is -128 to 127". A leading `-` is now
  folded into the literal during lowering (ADR-0038), which is the only way the minimum of a
  two's-complement type can exist: negating 128 in an `s8` overflows, so `-128` has to *be* a
  literal rather than a negation of one. `-x` on a value still negates, and still traps.
- **Optimisation is real but shallow.** Four passes run, and `024-hello.jr` now folds
  its struct away entirely, collapses an `if` and deletes the dead arm. But forwarding is
  one walk per basic block, so anything read across a loop boundary stays in memory, and
  a whole-struct store never feeds a field read — which is why `modules/Basic`'s `print`
  still keeps its slot.
- **ADR-0002's arithmetic has two implementations, not one.** `jr-pool` owns the one
  both *evaluators* share; `jr-codegen-clif` keeps its own because it emits code rather
  than evaluating. The pair is held equal by `differential.rs` and nothing else.
- **Neovim integration is verified on one machine, not gated.** The 166 checks need an
  editor, and Neovim is not a build dependency of this workspace, so `cargo test` cannot
  run them. No other editor is packaged for, deliberately (ADR-0036). They also need the
  *installed* parser to be current: `editors/nvim/build.sh` is a separate artefact from the
  grammar, and gate 6 regenerates one without rebuilding the other.
- **The tree-sitter parser must be rebuilt after a grammar change**, and highlighting
  fails *silently* if you forget — `ftplugin` starts tree-sitter under `pcall`, because a
  missing parser is an ordinary state rather than an error.
- **Hover on an `#import` shows which file it resolved to**, because `#import "Basic"` does
  not say *which* `Basic` — the module search-path order decides, so the answer depends on how
  the server was configured. It also shows the module's `//!` documentation. Both were
  unreachable before ADR-0035, behind an `ItemKind::Import` arm whose comment claimed
  otherwise.
- **Hover does not work on a type annotation.** The `Point` in `p: Point` gets nothing,
  and no care in the language server can fix it: `jr_hir::TypeRef::Name` carries a symbol
  and no span, so there is no position to match a cursor against. A test pins the
  limitation and fails the day it stops being one (ADR-0028 §4).
- **Completion's idea of scope is "declared earlier in this body"**, not block scope. It
  over-offers — a local from a sibling block that has already closed — and never
  under-offers, which is the direction that would make the list feel broken.
- **`references` and `rename` cost 55 ms on their first call, and a reverse index would not
  help.** Both scan every workspace file, because ADR-0029 §3 discovers paths rather than
  loading them — but the split says where the time goes: **31 ms parsing, 24 ms lowering and
  resolving, 0.5 ms actually searching**. It is a cold-start cost paid once per session, and
  an index would have optimised the last 1% (ADR-0034) — which is what the previous handoff
  had already promised to build. Warm it is 0.53 ms, and 0.10 ms after an edit. The live lead,
  if this ever matters, is parsing the files in parallel.
- **A rename can refuse, and it will.** It refuses on a name collision, on a syntax error in
  any file it would edit, on a non-identifier, and when the workspace exceeded 10 000 files.
  That is deliberate (ADR-0030 §3) — a rename that half-completes leaves a broken build, and
  one that resolves a collision by shadowing leaves code that compiles and means something
  else — but it does mean the feature says no more often than a Rust user expects.
- **Completion's scope, and rename's, are not the same notion.** Completion offers locals
  "declared earlier in this body"; rename resolves them properly through `ResolveMap`. The
  first is an approximation, the second is not.
- **Nothing checks that a doc comment is true**, and nothing but the language server reads
  one. There are no doc tests and no `jr doc`.
- **A "did you mean" suggestion is a guess, and stays silent rather than guessing badly.**
  E0218 and E0212 offer the nearest field or type name within an edit distance that scales
  with length — and *nothing* for a name under three characters, because at that length every
  identifier is within reach of every other and the suggestion would carry no information.
  A missing suggestion is the common case.
- **The unused-import warning is a language-design position, not a lint.** Jai does not warn
  about one; Jairs does, because ADR-0014's flat import merge means an unused import silently
  enlarges the name space every identifier resolves against, and can turn a later declaration
  into an ambiguity error from a module the file never uses. It is deliberately conservative:
  an import is reported only when nothing in the file uses a name it provides, in either
  expression *or* type position.
- **A "flaky test" turned out to be a real bug that lost your diagnostics.** For several
  waves `opening_a_broken_file_publishes_diagnostics` hung intermittently and was recorded as
  flaky. It was not: the server queued the diagnostics job and *then* re-walked the workspace,
  and that write cancelled the job, which published nothing because a comment claimed the
  canceller would queue a replacement — true of an edit, false of a re-walk. Any client
  without a file watcher, which includes a plain `nvim`, silently got no diagnostics on open.
  Fixed and pinned by ADR-0032: **11 failures in 16 loaded runs before, 0 in 16 after**. It
  stayed hidden because it never reproduced on an idle machine, and because a test with no
  timeout does not fail — it waits.
- **Nothing here is self-hosted.** The compiler is Rust; only `modules/Basic` is Jairs.

---

## What it looks like

```jr
#import "Basic";                       // module system: one module, one file

Point :: struct { x: s64; y: s64; }   // structs, one level

add :: (a: s64, b: s64) -> s64 {      // procs, single return
    return a + b;
}

MESSAGE :: "hello from Jairs\n";      // constants
COMPUTED :: #run add(2, 3);           // one trivial comptime call

main :: () {
    p: Point;                         // decls: typed, and inferred below
    p.x = 4;
    sum := add(p.x, COMPUTED);        // := inference
    if sum > 5  print(MESSAGE);       // if
    i := 0;
    while i < 3 { i = i + 1; }        // while
    ptr := *sum;                      // pointer take + deref
    if ptr.* == 9  print_int(9);      // `print_int` works as of ADR-0037
}
```

---

## Strategy

The compiler is built as a **vertical tracer-bullet slice** ("Jairs-0") that
drives one tiny language subset all the way through every component — lexer,
parser, CST, HIR, Sema, MIR, VM, Cranelift, linker, FFI, stdlib module, LSP,
tree-sitter, formatter — until `hello.jr` is a signed native arm64 binary and
the LSP gives hover on it. Everything works, badly. Then the language is
thickened one feature wave at a time.

See [`PLAN.md`](PLAN.md) for the full roadmap, wave order, and architecture
decisions.

---

## Architecture

```
Source .jr
  → Lexer (hand-written, trivia-preserving)
  → Parser (hand-written, error-recovering, recursive descent)
  → Lossless CST (rowan)          ← jr fmt consumes this directly
  → Typed AST accessors
  → HIR (desugar, module graph, #import resolution, scopes)
  → Sema (lazy on-demand: types, inference, const-eval, polymorphs)
      ↔ InternPool (canonical IDs for every type and comptime value)
      ↔ Bytecode VM (#run / #insert / comptime FFI)
  → MIR (typed SSA, monomorphized)
  → Mid-end (inliner, mem2reg, DCE, const-prop)
  → Cranelift backend  →  object file  →  cc driver + codesign  →  native binary
  → LLVM backend (W8, behind --release)
  → salsa DB  →  LSP server (diagnostics, hover, goto-def)

tree-sitter-jairs  — separate editor grammar, CI-gated against drift
```

The LSP is a **consumer of the same salsa queries** as the batch compiler, not
a second frontend. The VM and Cranelift both consume the same MIR so `#run` and
runtime cannot silently disagree — and the mid-end is required to keep that literally
true, not merely approximately: the inliner refuses to rewrite any body compile-time
evaluation can reach (ADR-0021 §2), so every body both engines might execute is
bit-identical in each.

---

## Crate layout

| Crate | Responsibility |
|---|---|
| `jr-base` | Foundational types: source spans, `FileId`, string interning, arenas, newtype IDs |
| `jr-diag` | Diagnostic model (severity, spans, notes, instantiation backtraces) and rustc-identical renderer |
| `jr-syntax` | Lexer, `SyntaxKind`, error-recovering recursive-descent parser, lossless `rowan` CST, typed AST accessors |
| `jr-fmt` | Canonical formatter — a pure function over the lossless CST |
| `jr-hir` | Desugared high-level IR: module graph, `#import` resolution, scopes, name binding |
| `jr-pool` | `InternPool`: canonical identities for every type and every compile-time value, plus the layout both back ends share (ADR-0018 §2) |
| `jr-sema` | Lazy on-demand semantic analysis: type checking, inference, const-evaluation, polymorph instantiation |
| `jr-mir` | Typed SSA mid-level IR and optimisation passes, including the inliner Cranelift does not provide |
| `jr-vm` | Bytecode compile-time execution engine: lowering from MIR, interpreter, comptime FFI bridge |
| `jr-codegen` | `Backend` trait and lowering helpers shared by every native backend |
| `jr-codegen-clif` | Cranelift backend — all Cranelift API contact is confined here (ADR-0009) |
| `jr-codegen-llvm` | LLVM back end via `inkwell`, behind a default-off `llvm` cargo feature and gate 7 (ADR-0143). The third execution engine the differential harness compares |
| `jr-link` | Object-file emission and system linker driver, including macOS ad-hoc codesigning |
| `jr-db` | salsa query database — single source of truth shared by the batch driver and the LSP; the type pool is an `RwLock` whose read half is `Db::read_pool` (ADR-0149 §1) |
| `jr-driver` | Compilation orchestration: workspaces, compiler message queue, build metaprograms |
| `jr-lsp` | Language server — a consumer of `jr-db` queries, never a second frontend |
| `jr-cli` | The `jr` binary (`jr build`, `jr run`, `jr fmt`, `jr check`) |

---

## Building and testing

```sh
# Requires Rust stable (pinned via rust-toolchain.toml).
cargo test --workspace

# Check formatting and lints before pushing:
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

macOS arm64 is the primary development target and the only one anything has ever been verified on.
The workflow in `.github/workflows/ci.yml` configures a macOS + Linux x86-64 matrix, and **no CI run
has ever happened on this repository** — `main` has never been pushed. So "kept green in CI" would be
false in both halves: Linux is unverified, and the six gates are green *locally*. The tree-sitter
corpus job, which is the only check that can detect a **wrong parse tree** rather than an error count,
has therefore never run either.

---

## Licence

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT licence ([LICENSE-MIT](LICENSE-MIT) or
  <https://opensource.org/licenses/MIT>)

at your option.
