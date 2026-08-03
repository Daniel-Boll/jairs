# ADR-0075: `type_info` returns a `Type_Info` declared in `Basic`, and a constant may hold a string

- **Status:** Accepted
- **Date:** 2026-08-03
- **Deciders:** dboll
- **W4 sub-wave 7.** ADR-0074 closed with "this is the last thing between W4 and RTTI". Probing before
  writing this ADR found that it is not — §0. Two decisions ship together here because the first is
  RTTI's blocker and neither is worth a wave alone.

## Context

### 0. What running found, and the claim it corrects

ADR-0074 §0's four facts were each checked. Its *closing* sentence was not, and it is wrong:

```
Named :: struct { name: string; size: s64; }
mk :: () -> Named { n: Named; n.name = "s64"; n.size = 8; return n; }
V :: #run mk();
// error[E0230]: a compile-time aggregate holding a string arrives with a later wave
```

A `Type_Info` that carries a type's **name** is exactly this shape, so RTTI was still blocked — on a
refusal ADR-0074 *wrote itself*, in `intern_element`. That is the fourth time a scheduled dependency has
turned out not to hold (ADR-0067 §0, ADR-0070 §0, ADR-0072 §5, ADR-0073 §0), and the first time the false
claim was **this project's own ADR about the very next wave**. The habit that catches it is the same one:
type the program.

Four further facts, each checked rather than assumed:

- **The gap is comptime-only.** The identical struct works at run time — it prints `s64` and exits 8. So
  layout, field offsets and both back ends already handle a string inside an aggregate; only the path from
  the VM into the pool does not.
- **The cause is structural, not a missing branch.** `reduce` copies the result out as a flat
  `Raw::Aggregate(Vec<u8>)` byte image, and a `string` field's eight-plus-eight bytes are a
  `{data, count}` pair pointing **into the VM's memory**. `intern_aggregate` runs *after* the VM is
  dropped, so by then the pointer is dangling. Refusing was right; the fix is to resolve the text while
  the VM is still alive, which means `Raw` has to stop being flat.
- **No compiler-declared type is spellable.** `t: Type;` and `c: Context;` both report E0212. `Context`'s
  fields are the Rust `const`s `CONTEXT_FIELD_TYPES`/`CONTEXT_FIELD_NAMES` precisely because a
  compiler-declared type has no `DeclId` (ADR-0057 §1), and `resolve_type_name` reads a
  `SigEntry::type_value` no declaration sets to either.
- **The compiler never looks a name up in `Basic`.** There is no such mechanism anywhere in `crates/`;
  `#import "Basic"` is an ordinary module load. So "the compiler finds `Type_Info` in the standard
  library" is not a small step from where the code is.

Those last two are a vice, and §2 is the way out of it.

## Decision

### 1. `Raw` becomes a tree, so a string element is resolved while the VM is alive

`Raw::Aggregate(Vec<u8>)` becomes `Raw::Aggregate(Vec<Raw>)`: `reduce` walks the aggregate's fields
*inside* the VM's lifetime, reducing each element recursively and turning a `string` element into the
`Raw::Str` variant that already exists. Interning then consumes a tree in which every string is already
owned text, and `intern_element`'s refusal goes away rather than being special-cased.

This is not a new mechanism. `reduce` **already** does exactly this for a top-level string —
`Value::Aggregate(_) if ty == PoolId::STRING => Raw::Str(vm.read_string(value)?)` — using the VM's own
`read_string` while it is alive. The decision is only to apply it one level down, which is where the
existing code stopped.

**Rejected: keeping the byte image and reading strings out of a saved copy of VM memory.** The image
could be paired with a snapshot of the VM's heap, and pointers resolved against that afterwards. It keeps
`reduce` a one-liner and it is how a debugger would do it — and it means carrying the VM's whole address
space to intern two words of text, then re-deriving which bytes are pointers, which is the field walk
this decision does directly and with types in hand. It also puts a *host* pointer width in the saved
image, the target-specific trap ADR-0074 §1 rejected bytes to avoid.

**Rejected: interning the string in the VM and storing its `StrId` in the byte image.** The bytes would
then be pool-relative and safe to read later. But it needs `&mut Pool` during execution, and the VM holds
`&Pool` — which is the precise reason `Raw` exists at all ("interning needs `&mut Pool` and the VM holds
`&Pool`"). Making the pool mutable during comptime execution to save a field walk would undo the
arrangement that keeps evaluation and interning separable.

### 2. `Type_Info` is a **compiler-built nominal struct**, and `type_info` is a compiler intrinsic

`type_info(T)` takes a type and returns a **`Type_Info` by value**. The struct is declared **in `modules/Basic`**, as
ordinary Jairs source — and the compiler finds it by the mechanism §0 says does not exist yet, which this
wave adds narrowly: a single by-name lookup of `Type_Info` in the loaded module graph, resolved once and
cached, failing with a diagnostic if it is absent or shaped wrongly.

**By value rather than by pointer, and this section first said pointer.** The MIR verifier caught it
within minutes of the first working build: `info := type_info(Point)` produced `deref of a non-pointer`,
because the value `type_info` folds to is an `Item::AggregateValue` — a *constant*, which has no address.
A `*Type_Info` therefore needs somewhere for the pointee to live, and the honest options are a stack slot
(which dangles the moment the frame returns, so `return type_info(T)` would hand back a dead pointer) or
static data the back end emits per described type (real, but it is a *storage* decision, and a second one
in a wave that already has two). By value needs neither: an aggregate return is ADR-0051's `sret`
mechanism, which already works for every other struct-returning procedure.

Nothing is lost, because §4 had already declined to promise that two `type_info(T)` calls yield the same
pointer — so no pointer identity was on offer to give up. A program that wants one writes
`info := type_info(Point); p := *info;` and gets a pointer to its own copy, which is honest about the
lifetime. The verifier's objection is the reason this is recorded as a correction rather than as the
original plan: a placeholder that type-checked would have been the silent-miscompile failure mode, and
this project's rule is that such a case refuses instead.

**Why in `Basic` rather than as a `Context`-style compiler `const`.** A `Type_Info` must be *spellable*:
a program that reflects has to write `info: Type_Info` and read `info.name`. §0 established that no
compiler-declared type is spellable and that fixing that in general would mean giving compiler-declared
types a `DeclId` and a resolvable `type_value` — a change to name resolution for one type's benefit.
Declaring the struct in Jairs makes it spellable **for free**, because it is then an ordinary nominal
declaration with an ordinary `DeclId`, and every existing mechanism (field access, layout, `using`,
pointers) applies with no new case. `Context` could not take this route: it is threaded through *every*
call as a hidden parameter (ADR-0057), so it must exist before any module loads.

**The cost, stated plainly:** the compiler now depends on a declaration it does not own. If someone edits
`Basic`'s `Type_Info`, the compiler's field indices are wrong. This is mitigated, not eliminated — the
lookup **validates the shape** (field names, types and order) and refuses with a diagnostic naming the
mismatch rather than reading whatever is at offset 8. A wrong offset would be a silent wrong value, which
is this project's named failure mode; a refusal is not.

**Rejected: `Type_Info` as a `CONTEXT_FIELD_TYPES`-style structural type.** Symmetric with `Context` and
needs no lookup — and it would be **unspellable**, so a program could obtain a `Type_Info` and have no
way to declare a variable of that type or name it in a signature. Reflection you cannot write down is not
reflection.

**Rejected: `#type_info` as a directive rather than a call.** It is what `#run` and `#insert` are, and a
directive cannot be passed as a value or composed. `type_info(T)` reads as the call it is, and ADR-0071
already makes a type an argument-position value; the receiver-of-a-type allowlist in sema is where this
hooks in.

### 3. The schema: what a `Type_Info` says

```
Type_Info :: struct {
    kind: Type_Info_Kind;   // an enum: INTEGER, FLOAT, BOOL, STRING, POINTER, ARRAY, STRUCT, ENUM, …
    name: string;           // the type's source name, or a built-in's spelling
    size: s64;              // runtime size in bytes
    alignment: s64;         // runtime alignment in bytes
}
```

Four fields, and the `name` is why §1 had to happen first. `kind` is an enum rather than an integer so a
`switch` over it is exhaustiveness-checked (ADR-0067), which is the whole point of having a tag.

**Rejected for this wave: per-kind detail** — a struct's field list, an array's element type, a
procedure's signature. Each is a *variable-length* member, so it wants a view or a pointer-plus-count,
and a view of `Type_Info_Field` needs the field structs to live somewhere with a lifetime as long as the
program. That is a memory-ownership decision (static data the back end emits, versus a comptime-built
table), and folding it into the wave that also lifts the string blocker and adds the lookup would put
three decisions in one ADR. `size` and `alignment` are enough to be *useful* — they are what `size_of`
would give, which the language does not have at all — and the shape extends by adding fields, which does
not break a reader that only names the four.

**Rejected: `Any`.** `Any` is a `{type, pointer}` pair and it needs `Type_Info` to exist first; it also
raises implicit-conversion questions (does every value coerce to `Any`?) that are their own decision.
ADR-0073 §0 named the two together and they are not one problem.

### 4. What is deliberately absent

- **`Any`** (§3), which this unblocks rather than delivers.
- **Per-kind detail** (§3): field lists, element types, signatures.
- **A `Type_Info` for a comptime-only type.** `Item::TypeType` has no runtime layout at all
  (`LayoutError::ComptimeOnly`), so `size` and `alignment` have no answer; `type_info(Type)` is refused
  rather than reporting zeroes, for `type-errors/063`'s reason — a plausible wrong number is worse than a
  refusal.
- **Comparing two `Type_Info` pointers for type identity.** Tempting, and it would work if the compiler
  guaranteed one `Type_Info` per type. It does not guarantee that yet, so the ADR does not promise it.

## Consequences

- **`Raw` stops being flat**, and the change is confined to `jr-db`'s `consts.rs`: `reduce` recurses,
  `intern_aggregate` consumes a tree instead of slicing bytes at offsets, and `intern_element`'s
  string refusal is deleted. Reading fields *by offset* goes away with it, which removes a place where
  the pool's layout and the VM's could disagree.
- **A standing user-visible gap closes**, independent of RTTI: `#run` returning any struct with a string
  field now works, which is the shape any configuration-at-compile-time program has.
- **The compiler gains one dependency on `Basic`'s source**, validated on lookup (§2). This is the first
  such dependency and it deserves the scrutiny: the validation is what keeps a mismatch a diagnostic
  rather than a wrong offset.
- **Two new diagnostic codes**, E0265 and E0266: `Type_Info` missing or wrongly shaped, and `type_info`
  applied to something with no runtime layout. E0267 becomes the first free code.
- **This does not complete W4.** `#code`/`Code`, `Any`, per-kind detail and a cross-file `#run` value all
  remain, and the plan should say so rather than repeating ADR-0074's mistake of declaring the path clear.
