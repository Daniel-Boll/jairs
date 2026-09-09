# ADR-0217: `String_Builder` uses captured-allocator buffer chains, and the Jai guide is a pinned local reference

- **Status:** Accepted
- **Date:** 2026-09-09
- **Deciders:** dboll
- **Research:** [`docs/research/way-to-jai-compatibility.md`](../research/way-to-jai-compatibility.md)
- **Reference:** `references/The_Way_to_Jai` at
  `19cb4b7acb0de2798c769f9ad73313a4d15f4056`

## Context

The six source-compatibility gaps that started with the
`ora_to_atlas_test` probe are closed by ADR-0214 through ADR-0216. The next
ordinary Jai example declares this:

```jai
builder: String_Builder;
defer free_buffers(*builder);
init_string_builder(*builder);
append(*builder, "One!");
print_to_builder(*builder, "value = %", value);
result := builder_to_string(*builder);
```

Jairs has the substrate: default and replaceable context allocators, procedure
pointers, byte pointers, `defer`, variadic `Any`, and one formatter used by
`print` and `format`. It has no `String_Builder`.

The linked guide is now a git submodule because the decider wants the examples
available locally for repeated compatibility audits. It is a secondary account
of a closed-beta language, not a specification or a build dependency. The
submodule commit and that evidence limit are recorded in the research inventory.

Four design forks matter:

1. contiguous storage or a chain of buffers;
2. the active allocator at each operation or an allocator captured by the
   builder;
3. transfer or copy when producing a `string`;
4. a second formatter, the bounded `format` API, or one formatter with another
   output sink.

The guide does not publish the real Basic module's implementation, but its
surface gives useful constraints: storage is plural in `free_buffers`,
conversion accepts a destination allocator separately from builder storage,
and examples defer `free_buffers` before converting. Sixteen local example
files use a builder and only two call `init_string_builder`, so a zero value
must be useful even though the explicit initializer remains part of the API.

## Decision

### §1. The guide is a pinned research submodule, never a build input

`references/The_Way_to_Jai` is a git submodule. `references/README.md` states
how to initialise it and the evidence rules for citing it.

Nothing under `references/` enters Cargo packaging, the bundled standard
library, module discovery, the corpus gates, or installed artefacts. ADR-0203's
nested-checkout exclusion is the mechanical boundary. Compatibility claims
name the submodule commit and distinguish example syntax from confirmed Jai
behaviour.

### §2. `String_Builder` lives in `Basic` and owns a chain of byte buffers

The guide imports only `Basic` for the builder, so a separate
`String_Builder` module would be source-incompatible at the first line.

The public value records:

- first and last buffer addresses;
- total byte count and the next buffer capacity;
- the allocator procedure, free procedure and state word captured at
  initialization;
- whether allocator capture has happened.

Each private allocation is a header followed by its byte capacity:

```text
next | used | capacity | bytes...
```

The first capacity is 256 bytes and later capacities double. An append larger
than the planned capacity gets a buffer large enough for that append. Existing
bytes never move, no `realloc` behaviour leaks into the VM/native comparison,
and `free_buffers` walks exactly the allocations the name promises.

The zero value initializes lazily on its first allocating operation. Explicit
`init_string_builder` remains available and resets an already-initialized
builder by freeing its buffers first.

### §3. Builder storage captures the context allocator triple

Initialization captures:

```text
context.allocator
context.allocator_free
context.allocator_data
```

Every builder allocation and release runs in a `push_context` that installs
that triple. After the call, the possibly changed `allocator_data` is copied
back into the builder. This is required for stateful arenas and counting
allocators; storing only the procedure pointer would call it with whichever
state happens to be active later.

Changing the caller's context after initialization therefore does not change
who owns existing or future builder buffers. `free_buffers` is idempotent,
empties the chain, and retains the captured allocator so the builder can be
reused.

Jairs cannot expose Jai's exact `builder.allocator = temp` spelling. It has no
first-class `Allocator` value, imported struct fields are not visible, and
`Context` is not a spellable source type. That is a language compatibility
item, not something this library should imitate with an unsafe half-shape.

### §4. The common Jai API is preserved; overload-only forms get explicit names

`Basic` exports:

```jairs
String_Builder
init_string_builder(*builder)
append(*builder, text: string) -> bool
append_bytes(*builder, data: *u8, count: s64) -> bool
append_byte(*builder, byte: u8) -> bool
print_to_builder(*builder, format_string: string, args: ..Any) -> bool
builder_string_length(*builder) -> s64
builder_to_string(*builder) -> string
free_buffers(*builder)
```

The guide's common `append(*builder, string)` spelling remains exact. Its byte
and pointer-length forms cannot share that name because Jairs has no general
procedure overloading (ADR-0053 §5); explicit suffixes make the incompatibility
local and searchable.

Append returns `false` only when a required allocation fails. The guide does
not state its return type, and every inspected call discards it, so this adds
recoverability without changing those call sites.

The guide's optional destination allocator and `extra_bytes_to_prepend` are not
implemented from secondary prose alone. Their ownership and pointer semantics
need primary source or a real compiler probe.

### §5. `builder_to_string` copies and does not reset the builder

Conversion allocates one contiguous result through the caller's current context
allocator and copies the chain in order. The returned string remains valid
after `free_buffers`, and the builder remains available until explicitly
cleared or freed.

This is the only ownership rule consistent with a separate destination
allocator plus the guide's `defer free_buffers` examples. Transferring one
buffer would fail for a multi-buffer builder, couple result ownership to
builder ownership, and make the deferred cleanup invalidate the result.

### §6. `print_to_builder` is another sink for the one formatter

The formatter keeps one rendering implementation. Its output state gains a
builder destination and an allocation-success flag. `out_byte` sends bytes
directly to the builder when that destination is active; it does not pass
through the 4096-byte file/buffer staging array.

Calling public `format` and appending its result is rejected: `format` is
deliberately bounded and silently truncates at its caller's buffer size.
Duplicating number, aggregate and reflection formatting is also rejected,
because two renderers will disagree.

The same wave adds one-based `%1`, `%2`, … argument selection. The pinned
builder examples use repeated and reordered placeholders in generated code,
and the current formatter knows only sequential `%` and escaped `%%`.

The formatter remains module-global and not thread-safe, exactly as `print`
already documents. `#add_context` is the later language feature that can make
the sink state per-thread.

## Rejected alternatives

- **A contiguous growable buffer.** Simpler state, but every growth may move
  earlier bytes, conversion still must copy to satisfy independent ownership,
  and plural `free_buffers` plus a separate destination allocator point toward
  distinct builder storage.
- **Use `[..]u8` or `List`.** The public dynamic array owns contiguous storage,
  and `List` is concrete `s64` and allocates through libc rather than the
  captured context protocol.
- **Use the caller's active allocator for every operation.** A context change
  between append and cleanup would free through the wrong allocator.
- **Transfer storage from `builder_to_string`.** A chain is not contiguous,
  the destination allocator may differ, and deferred `free_buffers` would
  invalidate or double-free the result.
- **Implement formatted append through a 4096-byte temporary.** It would make
  the first long generated source silently incomplete.
- **Wait for procedure overloading before shipping any builder.** The common
  string append spelling needs no overload, and the distinct byte names are a
  bounded compatibility gap rather than a reason to leave all builder users
  unsupported.

## Consequences

Ordinary Jai-shaped builder examples can be ported with no spelling change for
string append, formatting, length, conversion or cleanup. Byte append uses two
temporary explicit names until general overload sets arrive.

The standard formatter gains an unbounded destination and indexed arguments,
so generated source longer than one page no longer needs a second renderer or
manual chunking.

This is a library and documentation wave. It changes no parser, HIR, MIR, pool
layout, VM instruction, or native back end, so gate 7 is not required.
