# ADR-0060: `null` is a context-typed pointer literal, and `malloc`/`free` reach libc

- **Status:** Accepted
- **Date:** 2026-07-31
- **Deciders:** dboll
- **Fourth feature of W3.** ADR-0059 made a procedure a value; this makes a *pointer* writable as a
  literal and gives a program a way to get memory — the two things an allocator needs that ADR-0059
  left, once the proc-pointer struct field it named turned out already to work (see Context).

## Context

`PLAN.md` §7 said "a proc-pointer struct field... is absent". **It is not** — `a.fn = add;
a.fn(20, 22)` computes 42 in both engines, because a `ProcType` is a scalar and field access is
type-directed, so ADR-0059 gave it for free. That is the handoff rot the process warns about, in the
pessimistic direction: a claim of absence, contradicted by running it. What an allocator *actually*
lacks was found the same way — by writing one and seeing what the compiler refused.

Five facts, established by running rather than reading:

- **A proc-pointer struct field already works** (above), so it is not this wave's job.
- **`null` is the last remaining reserved keyword**, and its refusal still reads "arrives in wave
  W1" — a message eight waves stale. It is in `is_reserved_keyword`'s range and the parser turns it
  into E0121. **This is the fact that shapes §1**: `null` is the only keyword left in that block, so
  making it real empties the block.
- **There is no memory source anywhere.** `modules/Basic` binds `write`, `exit` and prints; no
  `malloc`, no `mmap`, nothing that returns storage. A program cannot get a byte it did not declare.
- **`null` is the first pointer *constant*.** `cast(*u8, 0)` is refused (E0214), and nothing else
  produces a pointer value — so `null` is not joining an existing path, it is the first thing to
  intern an `IntValue` of pointer type. Both engines already *classify* a pointer as a scalar
  (`Repr::of`, the VM's scalar shape), so the representation exists; nothing had reached it.
- **A `#foreign` procedure with a pointer return already works** — `write` returns `s64`, but
  ADR-0051's machinery passes a pointer in a register like any scalar, so `malloc`'s `-> *u8` needs
  no new ABI.

## Decision

### 1. `null` is a literal that takes its type from context

```jr
p: *u8 = null;         // ok — the annotation gives *u8
if p == null { }       // ok — the other operand's type
f(null);               // ok — the parameter's type
q := null;             // error: a bare null has no type to take
```

`null` becomes a `Literal::Null`, parsed into a `LITERAL_EXPR` beside `true`/`false`, and typed
exactly as an integer literal is (ADR-0016 §1): it has no intrinsic type, takes its context's, and is
an **error with no context** — because unlike an integer, there is no sensible default pointer type to
fall back to. It interns to `IntValue { ty: <the pointer type>, bits: 0 }`, which both engines already
handle as a scalar.

**`null`'s context must be a pointer type.** `n: s64 = null` is an error — E0257 — the same shape as
"expected `s64`, found a float literal": the literal is fine, the context is wrong for it.

**Rejected: a distinct `*void` type that coerces to any `*T`.** It would make a bare `null`
typeable and is closer to C. Rejected because it introduces the language's **first implicit
coercion**, which ADR-0016's no-coercion rule exists to forbid and which ADR-0015 already refused for
floats. A context-typed literal reuses machinery that exists and adds no coercion; the cost is that
`q := null` needs a type, which is the same cost `q := 1` does not pay only because integers have a
default and pointers have none.

**The keyword leaves `is_reserved_keyword` — the block is now empty of anything unimplemented.**
`cast`, `enum`, `union`, `xx`, `for`, `defer`, `using` each made this trip; `null` is the last, and
the reserved-keyword refusal (E0121) now has no keyword left to fire on. It stays as the mechanism,
because a future reserved word will use it, but its range shrinks to nothing.

### 2. `malloc` and `free` bind libc in `modules/Basic`

```jr
malloc :: (size: s64) -> *u8 #foreign libc "malloc";
free   :: (p: *u8) #foreign libc "free";
```

Two `#foreign` declarations beside `write` and `exit`, reached the same way. No new machinery: a
pointer return is a register scalar (ADR-0051), and `#foreign` already resolves a libc symbol.

**Rejected: `mmap`, for a page allocator.** Lower-level and what a real bump allocator sits on, but
`mmap` takes flag arguments whose values are platform-specific integers Jairs has no way to name —
they would need a per-platform set of `::` constants, which is a portability question this wave will
not open. `malloc` is portable and is the honest bottom of a standard library until W7 replaces it.

**`free` returns `void`**, so a program that allocates can release. Both are the minimum that makes a
pointer round-trip observable: allocate, use, free.

### 3. Comptime FFI stays refused — a host pointer is not a VM value

`#run malloc(16)` is refused, and this wave changes nothing about that. ADR-0006 gates comptime FFI
behind `#foreign_at_comptime` (wave W6), so a compile-time `malloc` call already fails, and it must:

**The VM's `Memory` is its own address space with its own bounds checks.** A host pointer read
through it would fault or read unrelated VM memory — a *plausible wrong value*, which is this
project's first named failure mode. Runtime `malloc` works because a native pointer is a native
address; comptime `malloc` cannot, because the VM is not the host.

**The corpus file must not put `malloc` in a `#run`.** It allocates at *runtime*, in `main`, and the
differential harness runs it as a subprocess where the pointer is a real address in both the built
binary and — through libffi — the VM's runtime mode. §4 records why that is the one place a host
pointer is legitimate in the VM.

### 4. What the corpus must observe, and where

A `malloc`'d pointer's *value* is not observable (it is undefined which address the OS returns), so
the corpus proves the round-trip through what *is* observable:

- **`null` compares equal to `null` and unequal to a real pointer** — `p == null` after
  `p = malloc(…)` is false, and `null == null` is true. This is the whole point of a null pointer:
  a sentinel a program can test.
- **A byte written through a `malloc`'d pointer reads back** — `p[0] = 42; p[0] == 42` — proving the
  memory is real and writable, without depending on its address.
- **Both engines agree**, run as subprocesses. The VM reaches `malloc` through libffi in runtime
  mode, so the pointer is a genuine host address there too — the one place ADR-0006's gate does not
  apply, because it is runtime, not comptime.

**A `#run` calling `malloc` is deliberately absent** (§3), and a `type-errors/` file records that a
bare `null` with no context is E0257.

### 5. What is deliberately absent

- **Pointer arithmetic.** `p + 1` on a `*u8` is still refused (E0223), and the README calls that a
  design position, not a gap. A *bump* allocator needs it and so is still not writable; a `malloc`
  wrapper is. Changing the arithmetic rule is its own ADR with its own argument.
- **The allocator protocol in `context`.** `context.allocator` is still an `s64` placeholder
  (ADR-0057 §1). Putting a real allocator there — a struct of the proc pointers this wave's `malloc`
  and `free` could fill — is the next wave, and it is now unblocked: `null`, a memory source, and a
  proc-pointer struct field all exist.
- **`null` as a default argument.** ADR-0053 §2 admits only literal defaults; `null` is now a
  literal, so `p: *u8 = null` as a *parameter default* falls out — but it is not exercised this wave
  and is noted rather than claimed.

## Consequences

- **`Literal` gains a `Null` variant**, so every exhaustive match over it is a compile error until
  taught — `jr-hir`'s lowering, `jr-sema`'s `check_literal`, and the const-folder. House style.
- **One new diagnostic code, E0257**, for `null` in a non-pointer context. **E0258 is the first free
  code.**
- **`is_reserved_keyword`'s range collapses to empty of implemented keywords.** The one test that
  asserted `null` produces E0121 must flip to asserting `null` is now real, and the reserved-keyword
  machinery keeps its last user gone — recorded so the next wave that adds a reserved word knows the
  block is dormant, not deleted.
- **`jr-fmt` needs `null`**, and the formatter has lost a construct in eight of the last ten waves. A
  literal is the easy case — it round-trips as its own text — but a test must still pin it.
- **The tree-sitter grammar needs `null`** as a literal. It is a keyword the lexer already produces,
  so the grammar change is adding it to the literal choice; a missing arm is an ERROR node gate 6
  catches.
- **`modules/Basic` grows two `#foreign` bindings**, and its MIR snapshot changes. The differential
  harness gains a program that allocates, writes, reads back and frees.
- **A pointer is now writable without `cast`.** `cast(*u8, 0)` stays refused — this wave does not
  touch it — so `null` is the *only* way to write a null pointer, which is deliberate: one spelling,
  and it is the readable one.
