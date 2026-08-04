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

Last updated with **wave W7 — Stdlib open** and **W6 — Metaprogram still open**, 984 tests green. W7's modules so far are **`String`** (ADR-0103): `equal`, `compare`, `starts_with`, `ends_with`, `find`, `contains`,
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
to make, and it is short enough to read. A faster one is W8's job, with a benchmark behind it.

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
four, `push`/`pop`/`free_data`. It is a **new module rather than a rewrite of `Array`**, because the two have
different contracts — an `Int_Array` needs no cleanup while an `Int_List` **owns** memory a caller must free, and
with no destructors in the language that is something read in a type's name or never learnt.

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

Before it, wave **W6 — Metaprogram**, five sub-waves in, with 984 tests green. Its headline claim is met — a metaprogram can find
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
`print_int` (ADR-0037). 981 workspace tests; six CI gates green on macOS arm64, plus 166 Neovim
checks that are verified rather than gated.

### What you can actually do

| You can | How | Caveat |
|---|---|---|
| Compile and run a program in the comptime VM | `jr run file.jr` | Register bytecode interpreter, no JIT tier |
| Compile to a native executable | `jr build file.jr -o out` | arm64 macOS verified; x86-64 Linux configured in CI but **never run** |
| Build without bounds checks | `jr build file.jr --no-bounds-check`, or `jr run` | ADR-0003's build setting, finally wired (ADR-0058). An out-of-range index is then undefined behaviour, which is the trade. `#no_abc` on a procedure does the same locally, whatever the build says; compile-time execution checks regardless |
| Get rustc-grade diagnostics | `jr check file.jr` | 95 codes across lexer, parser, HIR, sema, MIR and const-eval. E0218 and E0212 suggest a near name; E0231 and E0245 are *warnings* — an unused `#import`, and a body the compiler could not lower |
| Format source canonically | `jr fmt [--check] paths…` | The corpus is canonical under it, CI-enforced |
| Inspect tokens or the CST | `jr parse file.jr` | Debug aid |
| Measure language-server latency | `jr bench file.jr` | Reports min/median/p95 cold, warm and after an edit. **Reports, never judges** — no threshold, not a gate (ADR-0033) |
| Print a number | `print_int(n)` from `modules/Basic` | Written in Jairs, and still recursive — both the `[N]u8` buffer and the `[]u8` view it wanted now exist, so nothing in the language is missing; converting it is its own change. Traps on the most negative `s64`, which cannot be negated (ADR-0002) |
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
| `struct { … }`, one level, nominal | |
| `union { … }`, nominal, **untagged** — every field at offset 0, so a cross-field read reinterprets | |
| `variant { … }` — a tagged union: a write sets the tag, reading another case **traps**, `switch` destructures it (ADR-0068) | a recursive variant; one in a `#foreign` signature; eliding the check inside a matching arm |
| `enum { RED; GREEN :: 5; }`, nominal, namespaced members, and bare `.RED` from context — including as a `switch` case (ADR-0067) | |
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
| `#run` at file scope or in a body, calling local or **imported** procedures, with loops and nested calls | `type_info()`, `Any`, `#code` (**W4**, in sub-waves) |
| a `#run` returning a **struct or array**, interned as its element values and materialised by both engines (ADR-0074), including one holding a **string** (ADR-0075) | a `#run` returning a **union** — untagged storage makes "which field is valid" unanswerable; a struct or array *literal* (`P.{1, 2}`), which is a separate syntax question |
| **`type_info(T)`** — a type's kind, name, size, alignment, a stable `id`, and the fixed-size per-kind facts `count` (a struct's field count / array length) and `element` (an array's element / pointer's pointee, as a type id); `Type_Info` is declared in `Basic` and validated on lookup (ADR-0075, ADR-0077, ADR-0078) | the variable-length **field list** — the elements need the program's lifetime, so it is a static-data-vs-comptime-table decision; following an `element` id back to a `Type_Info`; `type_info([4]s64)`, blocked on structural type aliases |
| **`Any`** — `any_of(*x)` erases a value to a `{*Type_Info, *u8}` pair, `any_as(a, T)` reads it back and traps unless the type's `id` matches; the erasing pointer conversion is allowed only at that boundary (ADR-0076) | a bare **value** coercing to `Any` implicitly (a literal has no address, so it needs a materialised temporary — the *pointer* form `takes(*x)` is done); an `Any` in a compile-time constant |
| `#insert "…"` of a **string literal**, lowered where it is written — same scope, so a local it declares is visible after it; nesting works, and every diagnostic points at the directive and names its offset into the inserted text (ADR-0072) | `#insert` at file scope, which would change the item tree; `#code` and the `Code` type |
| `#insert <expr>;` of a **computed** operand — a constant or a `#run` whose text is evaluated at compile time and spliced (ADR-0073). The operand resolves and type-checks like any expression (`#insert undefined;` → E0201; a non-string → E0214), and a pending insert the evaluator has not reached is refused, never miscompiled. This is where sema and the VM become mutually recursive; the cycle is broken by an acyclic pre-pass | a **cross-file** `#run` value (its own decision, ADR-0073 §4); expansion past 16 levels (E0264) |
| **`#code { … }`** — unquoted source spliced into the enclosing scope; the body is parsed where it is written, so no quoting and no escaping (ADR-0080). Deliberately *sugar* over `#insert`, reusing its depth bound and its refusal of a pending splice | a `Code` **value** — **declined**, not deferred: a quoted syntax tree is worth representing only once something can inspect or transform one (ADR-0080 §3); spans into the body's real source, so a fault inside it points at the `#code` |
| **`$T` polymorphic procedures** — inferred from the argument (directly or through `*$T`/`[]$T`), instantiated once per distinct tuple of bound types, checked per instantiation, run as ordinary procedures in both engines (ADR-0081–0084) | comptime-value params (`$$T`); macros (`#modify`/`#bake_arguments`/`#expand`); two-way unification and explicit type arguments |
| **polymorphic structs** — `Box :: struct($T) { value: T; }` used as `Box(s64)`; the instance is keyed on `(decl, args)` so `Box(s64)` and `Box(bool)` are distinct types with substituted fields and layouts, told apart in the pool the way `[2]s64` and `[3]s64` are (ADR-0085). Both engines compute each instance's layout from its substituted fields | inferring a struct's argument through a `$T` parameter (`(b: Box($T))`); `using` on a parameterised struct; a **cross-file** parameterised struct (E0269); recursive `List($T)` |
| **`[N]T` sized by a `$N` comptime parameter** (ADR-0089) — `buf: [N]s64` inside a `$N` procedure; each instantiation gets its own array type and layout from the baked value, so two calls at 4 and 3 give a `[4]s64` and a `[3]s64` from one declaration. The value reaches sema through the HIR already interned, so sema still runs no evaluator | a length needing *arithmetic* (`[2 + 2]u8`), or one naming a constant from another file — both ADR-0070's own deferrals |
| **`$N` comptime-value parameter and instantiation** (ADR-0087, ADR-0088): `make :: ($N: s64)` called as `make(5)` evaluates the argument to a compile-time constant and appends a concrete procedure with `N` baked into the body; two calls at the same value dedupe, distinct values instantiate separately (ADR-0005 extended to values). Mixed comptime and runtime parameters — `scaled :: ($N: s64, factor: s64)` — pass only the runtime one at the call site | `[N]T` where `N` is a `$N` parameter (small, next); a non-constant argument is refused E0271; a mixed `$T`+`$N` template falls through with an honest mismatch |
| a **type as a compile-time value**: `T :: Point;` binds one, and `T` is usable wherever `Point` is — as an annotation, a parameter, a field, an array element, a pointee; an enum alias carries its members (ADR-0071) | a chain (`B :: A`); comparing types (`T == U`); a `Type` parameter; `Type` as an annotation, which does not parse |
| using a type where a **runtime** value is expected is refused (E0261) — it has no runtime representation, so there is nothing to store | — |
| `#import`, `#foreign`, `#system_library`; `#expand` macros that splice; `#modify` predicates; `#bake_arguments` specialisations | — |
| `@note` metadata on a declaration — `@deprecated`, `@requires "x"` (ADR-0098) — read at compile time by `has_note` / `note_value` (ADR-0099), and queried without naming by `noted_count` / `noted_name` (ADR-0100), and used to **generate code** for each noted declaration by `noted_insert` (ADR-0101) | run-time **inspection**: a loop reading declarations as values, which needs a compiler-emitted table and lifts `Type_Info`'s field list (**W6**) |
| overflow traps with a source location (ADR-0002, ADR-0020), and a **call chain** of the frames that were live (ADR-0066) | a per-frame line number; inlined frames, which have no runtime existence |
| `context` — a hidden parameter passed by pointer, so a callee reads what its caller wrote; `#c_call` opts out and gets none | — |
| `push_context { … }` — a block with its own copy of the context, so a write inside it is restored on exit (ADR-0063) | — |
| `context.allocator` / `.allocator_free` / `.allocator_data` — install an allocator, and a callee allocates through it without knowing which | a `#foreign` procedure installed directly (wrap it) |
| `p + n`, `n + p`, `p - n` on a `*T` — element-scaled, unchecked; a bump allocator advances a pointer (ADR-0064) | `p - q` (deferred); `p[n]` sugar; pointer ordering `< >` |
| `talloc(n)` / `reset_temporary_storage()` — a per-context bump arena, valid until reset, no per-piece free (ADR-0065) | hands out `*u8` only (a wider store needs a pointer cast) |

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
| Sema (signatures, checking, inference) | **Works** | E0212–E0278; a union's diagnostics are a struct's unchanged, deliberately, and a bare `.RED`'s "no such member" is the qualified form's; no const-eval here — ADR-0018 §3 puts it in the VM, which is why an array length must be a literal. Float literals are context-typed with **no** fit check, because IEEE-754 saturates (ADR-0040 §5) |
| MIR (typed SSA, Braun construction) | **Works** | Block parameters, not phis (ADR-0017); CFG diagnostics E0227–E0229, the last of which now also reports a `break`/`continue` naming an unknown label (ADR-0049 §2); an explicit `bounds_check` statement and an explicit `zero`, both ADR-0039. `for` reuses the `while` shape with a synthesised induction variable and needs no new node; `defer`'s statements appear once per exit path |
| Mid-end | **Four passes** | Inliner, store-to-load forwarding, const-prop, DCE, to a bounded fixed point (ADR-0021 – ADR-0023). Forwarding is block-local, so a value read across a loop stays in memory, and it refuses two unequal array indices as possibly-aliasing; no SROA; the SSA value arena is never compacted |
| Bytecode VM + libffi | **Works** | Per-instruction spans, so a trap names its line. Floats need no new value variant, but are dispatched *before* the bit-compare fallback that would answer `NaN == NaN` and `-0.0 == 0.0` backwards. No JIT |
| Cranelift back end + linker | **Works** | Returns an aggregate through a caller-allocated `sret` pointer, uniform by size (ADR-0051) — a register fast path is W8's, because the size threshold and field classification are platform-specific and a wrong guess is garbage with no diagnostic. Carries the context as a second hidden parameter, after `sret` and before the declared ones, so one shared predicate computes an offset of 0, 1 or 2 (ADR-0057 §4). Calls through a procedure pointer with `func_addr` + `call_indirect` (ADR-0059 §4). Still refuses an aggregate crossing a `#foreign` boundary in either direction |
| salsa incremental database | **Works** | Built *and* optimized MIR staged (ADR-0021 §1); invalidation is at file grain |
| Differential harness | **Works** | Compares stdout, stderr and exit status of both engines as subprocesses |
| LLVM back end | **Not started** | Wave W8 |
| Language server | **Works** | `jr lsp`, twelve capabilities: diagnostics, hover, goto-definition, completion + resolve, references, documentHighlight, rename (workspace-wide, refuses rather than half-renaming), documentSymbol, workspaceSymbol, code actions, `signatureHelp`, inlay hints (ADR-0024, ADR-0028, ADR-0030, ADR-0031). Dispatches a read only after every write, because the reverse silently lost `didOpen`'s diagnostics (ADR-0032). No semantic tokens |
| Neovim integration | **Works** | `editors/nvim/` (ADR-0025), verified against the real editor by a 151-check script — **not** by CI, which has no Neovim |
| VS Code integration | **Will not be built** | ADR-0036: the maintainer does not use it, and a packaging target for an unused editor rots. `jr lsp` is editor-agnostic, so any LSP client works |
| Compilation driver / workspaces | **Partly** | `jr-driver` is still a one-line stub; the workspace *file list* exists in `jr-db::workspace` (ADR-0029): the search paths plus the root tree, walked and watched, bounded at 10 000 files |
| Debug info | **Not started** | No DWARF at all; a native binary is not debuggable |
| Optimisation levels | **Barely started** | No `--release` and no `opt_level`; one code path, plus exactly one build setting — `--no-bounds-check`, which is a *configuration* rather than an optimisation level (ADR-0058 §2). `BuildConfig` has one field deliberately: designing the level surface around a single boolean would mean redesigning it in W8 |

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
- **A cross-file `#run` does not work**, and ADR-0021 §2 now depends on that. Enabling
  it requires more than removing the refusal.
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
| `jr-codegen-llvm` | LLVM backend for optimised release builds — feature-gated, lands in wave W8 |
| `jr-link` | Object-file emission and system linker driver, including macOS ad-hoc codesigning |
| `jr-db` | salsa query database — single source of truth shared by the batch driver and the LSP |
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

macOS arm64 is the primary development target. Linux x86-64 is kept green in CI
as a sanity oracle.

---

## Licence

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT licence ([LICENSE-MIT](LICENSE-MIT) or
  <https://opensource.org/licenses/MIT>)

at your option.
