# ADR-0062: `context.allocator` is a struct of procedure pointers

- **Status:** Accepted
- **Date:** 2026-07-31
- **Deciders:** dboll
- **Completes ADR-0057 §1**, which put an `s64` in `Context` as an explicit placeholder "deliberately
  not usable as an allocator" and said what an allocator *is* — a procedure pointer plus data — needs
  indirect calls. Those arrived in ADR-0059; `null` and a memory source in ADR-0060/0061. This is the
  placeholder becoming real.
- **Fifth feature of W3**, and the last before traps-with-backtraces.

## Context

The pieces an allocator needs all exist, and that was checked by *writing* one rather than reading
the handoff:

```jr
Allocator :: struct { alloc: (s64) -> *u8; }
my_alloc :: (n: s64) -> *u8 { return malloc(n); }
main :: () { a: Allocator; a.alloc = my_alloc; p := a.alloc(16); … }
```

**That program already runs to exit 0 in both engines.** A proc-pointer struct field, assignment into
one, and a call through it all work — the field fell out of ADR-0059 because a procedure type is a
scalar and field access is type-directed.

Writing it also surfaced three gaps the handoff did not record, all of which this wave must close
because an allocator needs each:

- **A void-returning procedure pointer is unspellable.** `free: (*u8)` is what an allocator's release
  half looks like, and there is no syntax for it: `(*u8)` demands `->` (E0111), `-> void` is E0212
  because `void` has no type name (ADR-0015 §3), and `-> ` with nothing after it is a parse error.
  **This is the blocking gap**, and §1 closes it.
- **A `#foreign` procedure cannot fill a proc-pointer field, and the diagnostic is unactionable.**
  `a.alloc = malloc` reports *"expected `(s64) -> *u8`, found `(s64) -> *u8`"* — the same text twice,
  because the two types differ only in the invisible `ContextKind` (ADR-0059 §3 makes every
  proc-pointer type `Jairs`; a `#foreign` one is `CCall`). The refusal is correct; the message is
  not. §3.
- **The context's fields are a `const &[PoolId]`**, so they can only name *well-known* pool ids.
  An allocator's proc-pointer types are not well-known, so they must be pre-interned to be nameable
  there — the same move `PTR_U8` already makes, and for the same stated reason: it is reached before
  any user code mentions one. §2.

## Decision

### 1. `(T)` in type position is a procedure pointer returning `void`

```jr
Sink :: struct {
    put:     (s64);          // takes an s64, returns nothing
    release: (*u8);          // an allocator's free half
}
```

`parse_proc_type` makes the `-> T` **optional**, and `TypeRef::Proc`'s `ret` is already an
`Option<TypeRefId>` that resolves to `PoolId::VOID` when absent (ADR-0059 §3 built it that way for a
malformed arrow; this makes the absence *legal*).

**Why the arrow is omitted rather than `-> void` spelled.** A declared procedure already means `void`
by omitting the arrow — `f :: () { }` — so the type syntax matching the declaration syntax is the
rule a reader already knows. Making `void` a type *name* would reverse ADR-0015 §3, which says `void`
is a real type with no spelling; a bare `-> ` is punctuation with nothing to read and would make
`(s64) ->` and `(s64)` two spellings of one type.

**The results-list ambiguity widens, and the `->` still decides.** `-> (s64)` in *return* position is
a one-element results list (ADR-0052 §1, normalised to `s64`). `(s64)` in a *parameter* or *field*
position is now a proc-pointer type. These do not collide, because they are different positions:
`ret_type` looks ahead for the `->` after the closing `)` and only then commits (ADR-0059 §3), and a
bare `(s64)` in return position keeps meaning the results list. What is new is that `(s64)` in type
position no longer *requires* an arrow, and that is only reachable where a type is expected.

### 2. The allocator is two procedure pointers and a data word

`Context` becomes:

```jr
Context :: struct {
    allocator:      (s64) -> *u8;    // allocate this many bytes
    allocator_free: (*u8);           // release a pointer it returned
    allocator_data: s64;             // the allocator's own state
}
```

**Three fields rather than a nested `Allocator` struct**, and this is a representation choice with a
reason: `CONTEXT_FIELD_TYPES` is a `const &[PoolId]`, so every field type must be a well-known id. A
nested struct type would need a `DeclId`, which a compiler-declared type has not got (ADR-0057 §1's
problem, solved there by going structural) — so the fields are flattened into the context and the two
proc-pointer types are pre-interned as `PoolId::ALLOC_FN` and `PoolId::FREE_FN`, exactly as `PTR_U8`
is pre-interned "because it is reached before any user code mentions a pointer".

**`allocator_data` is an `s64`, not a `*u8`.** An allocator's state is whatever it wants — a pointer
to a block, a bump offset, a handle — and `s64` is the one width that holds any of them without
asserting which. A pointer field would say "the state is a pointer", which a bump allocator's offset
is not.

**Zeroed by default (ADR-0057 §5), which means a null allocator.** `main`'s context has
`allocator == null` until something sets it, so **calling through it traps** rather than silently
doing nothing — the null-pointer call is a real trap in both engines. §4 records why that is the
right default rather than installing a libc allocator automatically.

**Rejected: a separate global allocator.** ADR-0001's whole argument for a context is that an
allocator travels *with the call*, so a caller can change what its callees allocate from. A global
defeats that and makes `push_context` (ADR-0057 §6) meaningless.

**Rejected: leaving the `s64` placeholder and adding a free-standing `Allocator` type.** Two ways to
reach an allocator is one too many, and ADR-0057 §1 put the `s64` there *for this*.

### 3. A `#foreign` procedure in a proc-pointer field is E0256, not a type mismatch

`a.alloc = malloc` now reports E0256 — "a `#foreign` procedure cannot be used as a value yet" —
the code ADR-0059 §5 already defined for taking one as a value. Assignment into a proc-pointer field
is the same objection reached by a different route, so it gets the same diagnostic.

**Why not just improve the type-mismatch message.** Because "expected `(s64) -> *u8`, found
`(s64) -> *u8`" is not a message that can be improved into an actionable one: the reader cannot see
the difference, and the difference is not something they can *change* — a `#foreign` procedure has the
C convention and that is the whole point of `#foreign`. The actionable answer is the one E0256 already
gives: wrap it. `my_alloc :: (n: s64) -> *u8 { return malloc(n); }` is the fix, and the corpus shows it.

**A `#c_call` proc-pointer type is the general answer, and is deferred.** It needs a syntax for an
attribute inside a type — `#c_call (s64) -> *u8` — which is its own decision about where attributes
may appear. Recorded in §5.

### 4. The default context has a null allocator, and calling it traps

`main`'s context is zeroed (ADR-0057 §5), so `context.allocator` is `null` and calling through it
traps. **This is deliberate rather than an oversight**, and the alternative was considered:

**Rejected: installing a libc allocator in the entry stub.** Every program would then have a working
`context.allocator` for free, which sounds strictly better. Rejected because it makes `modules/Basic`
a *dependency of the runtime* rather than a library — the entry stub would have to know libc's
`malloc` symbol, and a `#c_call main` or a freestanding target would carry a dependency it cannot
satisfy. A program that wants an allocator installs one in a line:

```jr
context.allocator      = my_alloc;
context.allocator_free = my_free;
```

**A null-allocator call traps rather than returning null**, which is the honest failure: the program
asked to allocate from an allocator that does not exist, and a trap names the line. Returning null
would make every allocation site need a check for a mistake that is a *configuration* error, not an
out-of-memory one.

### 5. What is deliberately absent

- **Temporary storage** — W3's last data structure, and it wants this. Its own wave.
- **`push_context`** (ADR-0057 §6), so an allocator set by a callee is visible to *its* callees and
  not restored on return. This is exactly the form a scoped allocator wants, and it is still absent.
- **A `#c_call` proc-pointer type** (§3), so a `#foreign` allocator must be wrapped.
- **`alloc`/`free` wrappers in `modules/Basic`** that read the context. They would be one line each
  and they are *not* added: a wrapper that reads `context.allocator` would make `Basic` the thing
  that defines the protocol, and the protocol belongs to the language. A program calls
  `context.allocator(n)` directly, which is the whole surface.
- **Pointer arithmetic**, so a bump allocator is still not writable — only a `malloc` wrapper
  (ADR-0060 §5).

## Consequences

- **`Context` grows from one field to three**, so its layout changes and every corpus MIR snapshot
  that mentions a context offset moves. `CONTEXT_FIELD_TYPES`/`_NAMES` stay the single source both
  engines read.
- **Two new well-known pool ids**, `ALLOC_FN` and `FREE_FN`, and `WELL_KNOWN_COUNT` grows to 14. The
  `debug_assert_eq!` chain in `Pool::new` is what keeps the indices honest.
- **`046-context.jr` changes meaning**: `context.allocator` is no longer an `s64` a program can set to
  5. That file's assertions must be rewritten, and this is the first time a corpus file's *feature*
  was replaced rather than extended — recorded because a reader of that file will wonder.
- **No new diagnostic code.** §3 reuses E0256 and §1 removes a refusal. **E0258 is still the first
  free code**, which is worth stating because a wave usually adds one.
- **`jr-fmt` needs `(T)` with no arrow**, and the formatter has lost a construct in nine of the last
  eleven waves. The proc-type emitter currently always writes `") -> "`; it must not when there is no
  return type.
- **The tree-sitter grammar needs the optional arrow too**, and it interacts with the results-list
  conflict GLR currently resolves — so gate 6 is the check that matters here.
- **An allocator is now writable end to end**, which is what W3 was for: a program can install one,
  allocate through the context, and free — and the corpus proves it in both engines.
