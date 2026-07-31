# ADR-0065: temporary storage is a bump arena in two context fields, lazily allocated

- **Status:** Accepted
- **Date:** 2026-07-31
- **Deciders:** dboll
- **W3's last data structure.** ADR-0057 §6 listed "no temporary storage" as deliberately absent
  because it "wants an allocator first"; ADR-0062 gave the allocator, ADR-0064 gave the pointer
  arithmetic a bump allocator needs. This assembles them.
- **Composes the last two waves rather than adding machinery.** The only compiler change is two more
  context fields; `talloc` and `reset_temporary_storage` are ordinary Basic procedures.

## Context

Jai's temporary storage is a per-context scratch arena: `talloc(n)` hands out `n` bytes that stay
valid until the next `reset_temporary_storage()`, with no individual `free`. It is the allocator you
reach for when a computation needs scratch space it will throw away wholesale — building a string to
print, say — and freeing each piece would be noise.

Everything it needs now exists and composes, which was checked by running before designing:

```jr
p := malloc(64);
off := 0;
q := p + off;   q.* = 7;   off = off + 8;
r := p + off;   r.* = 9;
```

runs to the expected result in both engines. So temporary storage is not new machinery — it is a
region from `malloc` (ADR-0060), a cursor advanced with pointer arithmetic (ADR-0064), and two fields
on the context (ADR-0057) to hold them.

## Decision

### 1. Two flattened context fields: `temp_data` and `temp_mark`

`Context` grows from three fields to five:

```jr
Context :: struct {
    allocator:      (s64) -> *u8;
    allocator_free: (*u8);
    allocator_data: s64;
    temp_data:      *u8;    // the scratch region, or null before first use
    temp_mark:      s64;    // bytes handed out so far — the bump cursor
}
```

**Flattened, not nested**, for the reason ADR-0062 §2 flattened the allocator: `CONTEXT_FIELD_TYPES`
is a `const &[PoolId]`, so each field's type must be a well-known id. Here that costs *nothing* new —
`temp_data` is `PoolId::PTR_U8` and `temp_mark` is `PoolId::S64`, both already well-known (unlike the
allocator's proc-pointer types, which had to be pre-interned). So `WELL_KNOWN_COUNT` does not move and
no new pool id is added.

**`temp_mark` is the count of bytes handed out, not a pointer.** The next allocation is at
`temp_data + temp_mark`, and a reset is `temp_mark = 0`. Storing the cursor as an offset rather than a
pointer means the reset is one integer store and the overflow check is one comparison against the
region size — both simpler than pointer forms, and neither needs `temp_data` to be non-null to be
meaningful.

### 2. The region is a fixed size, `malloc`'d lazily on first use

`temp_data` is null in a fresh context (zeroed, ADR-0057 §5). The first `talloc` sees null and
`malloc`s a fixed region — 64 KiB, a constant in Basic — then hands out from it. A program that never
calls `talloc` never allocates the region, which is why it is lazy rather than allocated in the entry
stub.

**Rejected: allocating the region in the entry stub.** Every program would then pay 64 KiB whether or
not it uses temporary storage, and — the deciding reason — the entry stub would have to call `malloc`,
which makes `modules/Basic` a *dependency of the runtime*. ADR-0062 §4 rejected exactly this for the
allocator, and the same argument holds: a `#c_call main` or a freestanding target could not satisfy it.
Lazy allocation keeps the entry stub knowing only how to zero a context.

**Rejected: a growable region.** Growing means either a `realloc` that moves the arena — invalidating
every pointer `talloc` already returned, which is the one thing an arena must never do — or a linked
list of blocks, which is more machinery than W3 needs. A fixed region that returns null on overflow is
honest about its limit, and 64 KiB is enough for the scratch uses the slice has. A later wave can make
it configurable through a context field.

### 3. Overflow returns null, matching `malloc`

`talloc(n)` where `temp_mark + n` exceeds the region returns `null`, exactly as `malloc` does on
failure (ADR-0060 §2). A caller checks for null the same way, so the two allocators have one failure
convention. It does **not** trap: running out of scratch space is a resource condition a program may
want to handle (fall back to `malloc`, or reset and retry), not a bug that should end the process.

### 4. `talloc` and `reset_temporary_storage` live in Basic, reading the context

Both are ordinary Jairs procedures in `modules/Basic`, and this is a *different* call from ADR-0062
§5, which kept the allocator protocol out of Basic. The distinction: the allocator protocol is *how a
callee reaches whatever allocator its caller installed* — a language mechanism, so a wrapper in Basic
would wrongly claim to define it. Temporary storage is a *specific allocator*, one concrete arena with
one policy, and a concrete allocator is exactly what a library provides. `talloc` reads
`context.temp_data`/`temp_mark`, so it still travels with the context — a callee's `talloc` uses its
caller's arena — but the *code* is a library's, not the language's.

```jr
talloc :: (n: s64) -> *u8 { … reads context.temp_data / temp_mark, bumps, returns … }
reset_temporary_storage :: () { context.temp_mark = 0; }
```

**`reset` does not free the region**, it rewinds the cursor. The 64 KiB stays `malloc`'d for the
program's life — reuse is the whole point of an arena, and freeing it would defeat the "no per-piece
free" convention that makes temporary storage cheap.

### 5. What is deliberately absent

- **Alignment of `talloc`'s result.** It hands out byte-aligned pointers; a caller that needs an
  aligned block over-allocates and rounds, as C code does with `malloc`. Aligned `talloc` is a later
  refinement, not a slice need.
- **`push_context` around temporary storage does not get a fresh arena** automatically — it copies the
  context, so the *fields* (the pointer and the cursor) are copied, and a `talloc` inside the block
  bumps the copy's cursor while sharing the same region. Resetting inside the block and having the
  outer cursor restored on exit is a real and useful interaction (ADR-0063), but a *separate* arena per
  push is not something this wave adds.
- **A configurable region size.** 64 KiB is a Basic constant; making it a context field is a later
  decision.

## Consequences

- **`Context` grows from three fields to five**, so its layout changes and every corpus MIR snapshot
  that mentions a context offset moves. `CONTEXT_FIELD_TYPES`/`_NAMES` stay the single source both
  engines read, and both size the context from its layout, so neither entry path changes.
- **No new well-known pool id**, unlike ADR-0062: `PTR_U8` and `S64` are already well-known, so
  `WELL_KNOWN_COUNT` stays 14 and `Pool::new`'s `debug_assert` chain is unchanged.
- **No new diagnostic code and no new MIR node.** The whole feature is two context fields plus Basic
  code built from `malloc`, pointer arithmetic and field access — all of which already lower.
  **E0258 is still the first free code.**
- **`modules/Basic` gains its first stateful allocator**, and it is the first Basic code to *read* the
  context rather than only take syscalls. A corpus program allocates several times from `talloc`,
  resets, and allocates again — proving the cursor bumps, the region is reused, and a reset rewinds,
  in both engines.
- **The Neovim `verify.lua` context-field checks may count fields**, and if they assert the count they
  move from three to five — the kind of number a capability check pins deliberately.
