# ADR-0216: Fresh contexts have a default allocator, and whole-file operations belong to `File`

- **Status:** Accepted
- **Date:** 2026-09-08
- **Deciders:** dboll
- **Amends:** ADR-0062 §4 (the default context is no longer null), ADR-0157 §§1 and 3
  (whole-file operations move from `File_Utilities` to `File`)

## Context

The source probe that motivated ADR-0214 and ADR-0215 now reaches its remaining library call:

```jr
File :: #import "File";
contents, ok := File.read_entire_file(path);
```

Jairs already has the implementation, but under `File_Utilities`, and it requires every caller to
install an allocator before using it. That is a poor default for ordinary programs: `String`,
`JSON`, `Compiler.arguments`, path utilities and whole-file reads all use `context.allocator`, yet a
fresh context still contains null procedure pointers because ADR-0062 §4 deliberately rejected an
automatic allocator.

That old rejection was reasonable when the only apparent implementation was “put libc `malloc`
into the field”. It is not a valid implementation. The field has the Jairs procedure type
`(s64) -> *u8`, so an indirect call prepends the hidden `*Context`; libc `malloc` has the C
signature `(s64) -> *u8` and would receive the context pointer as its size. A raw libc address is
therefore silently ABI-wrong.

The existing whole-file implementation exposes a second ownership defect. A successful empty read
returns the literal `""`, while its documented caller cleanup is `String.free_string`. An empty
literal has a real pointer into read-only program data and must not be freed. The common and safe
rule is that every successful `read_entire_file` result is owned, including an empty one.

The decider approved a working default allocator while preserving custom allocators, and moving the
three whole-file operations to `File`.

## Decision

### §1. Every fresh Jairs context starts with a working allocator pair

The compiler initializes these two fields:

```jr
context.allocator
context.allocator_free
```

`allocator_data`, `temp_data` and `temp_mark` remain zero. A program may overwrite either allocator
field exactly as before, and a `push_context` block copies the active pair with the rest of the
context. No global allocator is introduced.

The default pair has the semantics of libc `malloc` and `free`:

- allocation returns a pointer to at least the requested bytes, or `null`;
- releasing `null` is a no-op;
- the VM allocates from its own region and keeps its existing no-op release, as ADR-0061 requires;
- native code calls the platform libc.

This reverses ADR-0062 §4's null-by-default choice. A null procedure pointer remains representable
and still traps when called; a test or program that wants that condition clears the field
explicitly.

### §2. Native back ends emit compiler-owned Jairs-ABI thunks

Cranelift and LLVM each emit two local helper procedures:

```text
default_alloc(context, size) -> pointer
default_free(context, pointer)
```

Their externally visible type is the existing Jairs allocator type: the hidden context parameter is
present and ignored. Inside the helper, the libc call uses the platform C convention and receives
only its declared arguments.

After the entry shim zeroes `main`'s context, it stores the two helper addresses into the allocator
fields. Field offsets come from `jr-pool`'s ordinary `field_offset` calculation, never from hard-coded
0 and 8 assumptions.

The helpers are compiler-owned rather than declarations in `Basic`. A context exists independently
of the source import graph, and installing a default must not add an implicit module, a reachability
root, or a requirement that every program import one particular library file.

### §3. The VM uses reserved non-null handles and initializes every context slot

VM procedure pointers are biased `ProcRef` handles rather than machine addresses. Two values outside
the real packed-handle domain identify the default allocator and release builtins. Indirect-call
resolution recognizes them before decoding an ordinary `ProcRef`.

`Vm::new_context` zeroes the aggregate and installs those two handles. Contexts created for
file-scope `#run` calls do not pass through that method: MIR gives the thunk a fresh context-typed
slot. VM slot plans therefore retain the one fact execution needs — whether a slot is a context —
and frame allocation initializes such a slot through the same helper.

This covers runtime `main`, `#modify` predicates, direct VM test calls and file-scope comptime calls.
A `#c_call main` still receives no context at all.

### §4. Whole-file operations move to `File`, with no forwarding copies

These declarations move from `File_Utilities` to `File`:

```jr
read_entire_file   :: (path: string) -> (string, bool) #must
write_entire_file  :: (path: string, contents: string) -> bool #must
append_entire_file :: (path: string, contents: string) -> bool #must
```

`File` imports `String` under a qualified alias for `adopt`; its existing descriptor API and names
remain unchanged. `File_Utilities` keeps textual path operations only.

There are no compatibility wrappers in `File_Utilities`. Imports are flat, so exporting the same
three names from both modules would make a caller importing both receive ambiguous names. This is a
pre-alpha source move, and repository callers are updated in the same wave.

### §5. A successful empty read is owned and safely freeable

`read_entire_file` allocates `size + 1` bytes even when the size is zero, writes a terminating zero,
and returns `String.adopt(data, count)`. The caller may therefore call `String.free_string` after
every successful read without checking the count.

Failure returns use `String.adopt(null, 0), false`, which is likewise safe to clean up
unconditionally. They do not claim an allocation exists.

The allocation continues to belong to the active context allocator. As with every other allocating
`String` operation, changing allocator pairs between allocation and release is outside the contract;
this ADR does not add per-string allocator provenance.

## Rejected alternatives

- **Store raw libc `malloc` and `free` addresses in the context.** Their C signatures omit the
  hidden context. The call would compile and pass the wrong first argument.
- **Define the default wrappers in `Basic` and discover them by name.** Programs without that import
  would still have no default, and the compiler would acquire an implicit source dependency and two
  hidden reachability roots.
- **Change allocator fields to `#c_call` pointers.** A custom allocator would then have no context
  from which to read or update `allocator_data`, breaking ADR-0062's stateful protocol.
- **Redesign the protocol to pass `allocator_data` explicitly.** It can work, but it is a source and
  ABI break unrelated to making the existing protocol usable by default.
- **Keep forwarding wrappers in `File_Utilities`.** Two modules exporting the same flat names make
  the common `File` + `File_Utilities` import ambiguous.
- **Return the literal `""` for a successful empty read.** The documented cleanup would free memory
  the allocator does not own, aborting natively while the VM can appear to tolerate it.

## Consequences

Ordinary allocating library calls work without setup, while arena, counting and other custom
allocators remain ordinary context-field assignments and retain `push_context` scoping.

The compiler gains two small runtime helpers per native executable and two reserved VM handles.
Context layout itself does not change. Gate 7 is mandatory because both native back ends and the
three-way differential are affected.

`File.read_entire_file` now matches the source shape readers expect, and an empty file has the same
ownership rule as a non-empty one.
