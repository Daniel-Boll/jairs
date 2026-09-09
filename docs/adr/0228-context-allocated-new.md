# ADR-0228: `New(T)` allocates one zeroed value through the active context

- **Status:** Accepted
- **Date:** 2026-09-09
- **Decider:** dboll

## Context

The requested container shape is:

```text
Node :: struct {
  name: string;
  children: [..]*Node;
  properties: Table(string, string);
}

node := New(Node);
stack: [..]*Node;
```

The first probe changed the planned wave. A same-file parameterised `Table($K, $V)`, the
`Table(string, string)` field, `children: [..]*Node`, and a separate `[..]*Node` stack already check
with no diagnostics. Jairs has predeclared nominal identity and representation-indirect recursion
since ADR-0015; `jr-pool` already lays out a pointer without asking for the pointee's body, and a
dynamic array is three words regardless of its element. There is no recursive-struct substrate to
add for this program.

What is missing is the allocation expression. `typed(T, context.allocator(size_of(T)))` can produce
the pointer manually, but it is both noisy and incomplete: allocator memory is not promised to be
zeroed, while a fresh Jai `New(T)` value is expected to begin as the zero value.

## Decision

### 1. `New(T)` is an unresolved-name intrinsic with one type argument

`New(Node)` returns `*Node`; `New(Table(string, string))` returns
`*Table(string, string)`. The argument uses the same type-position resolver as `size_of`, `typed`,
`type_info`, and fixed-array literals, so pointers, arrays, dynamic arrays, and parameterised
nominal types compose without a second type grammar.

The name is not reserved. A source declaration named `New` shadows the intrinsic, matching every
other unresolved-name intrinsic.

### 2. Allocation uses the active context, with the protocol's 16-byte alignment

The intrinsic calls `context.allocator(size_of(T))`, passing the current context by the ordinary
Jairs calling convention. A custom allocator therefore sees and may update `context.allocator_data`
exactly as it does when called explicitly.

The allocator protocol carries only a byte count. Its default native implementation is libc
`malloc`, and the VM deliberately matches its 16-byte alignment; therefore `New(T)` accepts layouts
aligned to at most 16 bytes and reuses E0266 for a stricter `#align`. Silently accepting an
over-aligned type would make the same source well-defined only under a specially chosen custom
allocator, with no way for the call to state that requirement.

If the allocator returns `null`, `New` returns `null`. Allocation failure is a resource outcome, not
a program error, so the intrinsic does not add a trap. The VM's bounded linear-memory exhaustion
remains a VM resource error, as it is for a direct call to its default allocator.

### 3. A successful allocation is zero-initialised before it is returned

MIR branches on the returned address. The non-null edge emits the existing whole-place
`Statement::Zero` against `p.*`; the null edge performs no write. Both edges join with the same
pointer value.

This reuses the one zeroing operation the VM, Cranelift, and LLVM already implement for local
aggregates. No back end receives a new allocation or initialization concept.

### 4. Ownership remains explicit

`New` allocates one object and returns only its pointer. It captures no hidden allocator metadata,
installs no destructor, and recursively frees nothing. A caller that still has the matching active
allocator releases it explicitly:

```text
context.allocator_free(untyped(node));
```

Container modules that must outlive allocator changes remain responsible for capturing an
allocator triple themselves; `New` does not silently invent that ownership policy.

### 5. The existing sema-to-MIR facts channel carries the decision

Semantic analysis records, per `New` call, the result pointer type and an interned `s64` byte-count
constant. MIR loads the canonical `allocator` field from the current `Context`, emits an indirect
call, retypes the byte pointer through the existing pointer-view mechanism, and zeroes the pointee
on the success edge.

A dedicated MIR `Allocate` node is rejected. Allocation is already an ordinary indirect call and
zeroing is already a statement; a new variant would make every optimization and all three engines
answer a question whose answer already exists.

### 6. Existing diagnostics describe the refusals

- A non-type argument reuses E0261, as the other type-bearing intrinsics do.
- A type without a runtime layout, or requiring alignment above the allocator protocol's 16 bytes,
  reuses E0266.
- File scope and `#c_call` bodies reuse E0254: `New` needs an implicit context and those locations
  have none.
- Wrong arity reuses E0216.

No new diagnostic code is needed.

## Rejected alternatives

- **A polymorphic `New :: ($T) -> *T` in `Basic`.** Jairs does not pass a type as a first-class
  runtime argument, and imported polymorphic procedures are still E0268. The library spelling would
  claim a call model that does not exist.
- **Call `malloc` directly.** That ignores the active allocator and breaks arenas, test allocators,
  and every ownership policy carried by `context`.
- **Leave the bytes uninitialised.** This makes `New(Node).children.count` nondeterministic and
  disagrees with the zero-value model used by ordinary declarations.
- **Trap on allocation failure.** It removes the caller's only recovery path and contradicts the
  existing `List.push`/`Map.put` resource-failure policy.

## Consequences

- The exact recursive `Node` shape is pinned by an executable corpus program rather than by adding
  redundant type-system machinery.
- `New(T)` works at run time and comptime wherever an active context exists.
- Gate 7 is mandatory for this wave because MIR construction changes, even though the three back
  ends consume only existing nodes.
- Generic list and hash-table operations still require the separate cross-file polymorphic
  instantiation wave; allocation does not disguise that blocker.
