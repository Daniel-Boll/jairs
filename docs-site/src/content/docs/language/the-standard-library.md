---
title: The standard library
description: The 25 in-Jairs modules — output and memory, containers, maths, files and processes, threads, JSON, and the game/graphics stack.
sidebar:
  order: 18
---

Jairs' standard library is written **in Jairs**, not inside the compiler. That is a design
commitment, not an accident: it forces the language to be expressive enough to write its own
library, and it means the library is something you can read to learn what the language means.
Every module here is a `.jr` file under `modules/`, and there are 25 of them.

`Array` and `Map` genuinely are parameterised structs — `Array :: struct($T)`,
`Map :: struct($K, $V)` — a parameterised struct crosses a module boundary now. What stays
concrete is the **procedures**: an imported polymorphic procedure is refused (E0268), so
`push :: (a: *Array($T), v: T)` would be uncallable by every importer, and
`push :: (a: *Array(s64), v: s64)` is what you actually get. Where you see `Array(s64)` or
`Map(s64, s64)`, that is why. It is the honest state of the library today, and the walkthroughs
in [Book III](/in-practice/) use exactly these concrete instances.

## Basic

The bottom of the library — imported by essentially every program. It reaches libc through
`#foreign` and provides output and raw memory:

```jr
print(fmt: string, args: ..Any) -> s64        // formats to stdout; % takes the next argument
print_error(fmt: string, args: ..Any) -> s64  // print, to standard error
format(buffer: []u8, fmt: string, args: ..Any) -> s64  // formats into a caller buffer, truncating
write(fd, buf, count)       // the #foreign syscall underneath print
exit(status: s64)           // terminate the process
malloc(size) -> *u8         // raw allocation; null on failure
free(p: *u8)                // release; free(null) is a no-op
talloc(n) -> *u8            // temporary-storage bump arena
reset_temporary_storage()   // rewind the arena
```

Older programs may still call `print_line` and `print_int`; both are compatibility wrappers.
New code uses `print("...\n")` and `print("%", n)`.

`Basic` also declares the `Type_Info` and `Any` structs that [reflection](/language/reflection/)
uses. Floats print: `print`'s `..Any` variadic renders any argument the formatter knows,
`float64` included — `print("%", 3.5)` writes `3.5` — though only one level deep, since a
`Type_Info_Field`'s type is an id rather than a nested `Type_Info`, so a struct prints one level
deep and a field that is itself an aggregate shows as `..` — `{i = 7, xs = ..}`.

## String

String operations are byte-wise and state their ownership. Read-only operations and borrowed views
allocate nothing:

```jr
equal(a, b) -> bool          starts_with(s, prefix) -> bool
compare(a, b) -> s64         ends_with(s, suffix) -> bool
find(haystack, needle) -> s64 (or -1)   contains(h, n) -> bool
byte_at(s, index) -> s64 (or -1)        is_empty(s) -> bool
slice(s, start, count) -> string         // borrowed, strict bounds
```

`equal` is what `==`-on-strings points you to (recall strings don't compare with `==`).
`byte_at` exists because `s.data[i]` doesn't compile — reading a byte from a `*u8` needs help.
`slice` preserves pointer identity into its source and traps on a negative or out-of-range extent.
Trims, split pieces and parse remainders are borrowed for the same reason.

Owned operations produce independent storage through `context.allocator`, which the caller frees:

```jr
concat(a, b) -> string       substring(s, start, count) -> string
copy_string(s) -> string
to_upper_copy(s) -> string   to_lower_copy(s) -> string
free_string(s: string)       // free one you got from the above
```

The pinned Way-to-Jai guide's non-overload names are also available: `begins_with`,
`compare_strings`, `find_index_from_left`, `find_index_from_right`, `replace_chars`, `is_any`,
`string_to_int`, `string_to_float`, and consuming `parse_int(*string)`. They delegate to the
existing implementations. The richer prefix parsers are `to_integer` and `to_float`, which also
return the borrowed remainder. Byte/string overload families wait for general procedure
overloading rather than being faked with a second set of algorithms.

Two related routines mutate bytes you already have rather than allocating —
`to_upper_in_place(s: string)` and `to_lower_in_place(s: string)` — and are the ones to reach for
when the string is already yours to change.

A fresh context supplies a default allocator. A caller who wants arena behaviour replaces the pair
and gets it for every routine at once.

## Sort

A generic sort, where the **caller** supplies the ordering:

```jr
sort(xs: []$T, less: (T, T) -> bool, comparisons: *s64)        // stable insertion sort, in place
sort_ints(xs: []s64)                                            // the concrete s64 wrapper
heap_sort(xs: []$T, less: (T, T) -> bool, comparisons: *s64)    // unstable, O(n log n) worst case
heap_sort_ints(xs: []s64)
stable_sort(xs: []$T, less: (T, T) -> bool, comparisons: *s64)  // stable merge sort, talloc scratch
stable_sort_ints(xs: []s64, less: (s64, s64) -> bool, comparisons: *s64)
is_sorted(xs: []$T, less) -> bool
ints_sorted(xs: []s64) -> bool
```

Three algorithms, not one. `sort` is insertion sort — `O(n²)`, said plainly — chosen because it
is stable, needs no allocation, and is short enough to read. `heap_sort` is unstable but
`O(n log n)` even in the worst case, in place. `stable_sort` is a bottom-up merge sort that is
both stable and `O(n log n)`, taking its scratch buffer from `talloc` and falling back to
insertion sort when the arena has no room, so the answer never depends on memory pressure.
`comparisons: *s64` is mandatory on `sort`, `heap_sort` and `stable_sort`, so there is no null
check in the comparison loop and no second copy of any algorithm. `is_sorted` is polymorphic
too but needs no counter — it only reads.

The caller passing `less` rather than the module requiring `<` is a language fact: selecting an
operator implementation per instantiated type — operator-bounded polymorphism — is something the
language cannot yet do, so the comparison is a parameter. `sort_ints`, `heap_sort_ints` and
`stable_sort_ints` exist as concrete wrappers because a `$T` template can't be called across a
module boundary — they are not conveniences, they are the only way an importing file can use this
module at all.

## Array and List

Two container shapes with different contracts:

**`Array(s64)`** — a **fixed-capacity** array (16 elements), no heap, no cleanup:

```jr
push(a, v) -> bool    // false when full
pop(a) -> (s64, bool)     get(a, i) -> (s64, bool)     set(a, i, v) -> bool
clear(a)   is_empty(a) -> bool   is_full(a) -> bool
```

`[..]s64` — the native dynamic array (see [Arrays and views](/language/arrays-and-views/)) — is
**growable**, heap-backed, and **owns** its memory. **`List` is not a type**; it is the eight
procedures that operate on one:

```jr
push(a: *[..]s64, v: s64) -> bool    // false only on out-of-memory; capacity doubles from 4
pop(a: *[..]s64) -> (s64, bool)      get(a, i) -> (s64, bool)     set(a, i, v) -> bool
clear(a)   is_empty(a) -> bool
elements(a: *[..]s64) -> []s64       // a view over the live elements — feed it to sort_ints
free_data(a: *[..]s64)               // YOU must call this; there are no destructors
```

`List` is a separate module from `Array`, not a rewrite, precisely because their contracts
differ: an `Array` needs no cleanup, a `[..]s64` owns memory you must `free_data`. `elements`
returns a view, so `sort_ints(elements(list))` sorts a list in place with no copy — the library
composing with itself.

## Map

An open-addressed hash table, `s64 -> s64`:

```jr
put(m, key, value) -> bool     get(m, key) -> (s64, bool)
has(m, key) -> bool            remove(m, key) -> bool
size(m) -> s64                 free_map(m)
```

Linear probing with tombstone deletion, grown at 3/4 load. Its hash uses **wrapping** `u64`
arithmetic (`*%`) — the overflow is the mixing working — so both engines compute the same
bucket, which a differential-tested hash table depends on absolutely.

## Math

Exact, closed-form functions plus libm wraps:

```jr
abs min max sign clamp pow gcd          // integer, exact
floor ceil round fabs                    // float, exact
sqrt sin cos exp ln powf                 // via libm through the FFI
```

`Math` deliberately shipped its transcendentals as **libm wraps** rather than in-language
approximations: libm is correctly rounded and both engines call the same libm, so `sqrt(2.0)`
is bit-identical in the VM and native code. An in-language approximation's last bit could
differ between engines — the one thing the differential harness treats as a failure.

It also has **vector math**: `Vector2`, `Vector3`, `Vector4` with `+ - * /` and `==` operators
(scalar multiply in both orders), plus `dot*`, `cross`, `length*`, `normalize*`, `distance*`,
`lerp*`, and a `Matrix4` with the usual transforms. The operators cross the module boundary, so
`a + b` on an imported `Vector3` just works. A `Quaternion` is present too — `{x, y, z, w}`,
scalar-last — with `quat_identity`, `quat_from_axis_angle`, `quat_conjugate`, `quat_dot`,
`quat_normalize`, `quat_inverse`, `quat_rotate`, `quat_to_matrix4`, and `quat_slerp` for
interpolating between two rotations. Multiplication deliberately does not auto-normalise.

## Random

A deterministic xorshift64 generator whose state the **caller owns**:

```jr
Random :: struct { state: u64; }

seed(rng, value)             // a zero seed is replaced with a golden constant
next(rng) -> u64             // the next 64 random bits
below(rng, low, high) -> s64 // a value in [low, high)
coin(rng) -> bool
```

State is caller-owned rather than a hidden global so it is testable, and its `u64` arithmetic
agrees bit-for-bit between the engines — a sequence that differed would fail the harness on its
first call.

## Time

A monotonic clock and a wall clock, both in nanoseconds:

```jr
monotonic() -> s64      // nanoseconds since an unspecified origin, from a clock that never goes back
wall() -> s64           // nanoseconds since the Unix epoch — comparable across machines, but can jump
to_milliseconds(ns) -> s64   to_microseconds(ns) -> s64   to_seconds(ns) -> s64   // truncating
is_after(a, b) -> bool  // whether a is a later reading than b on the same clock
```

`monotonic` is what a frame timer measures a duration with; `wall` is what a timestamp wants.
There is deliberately **no formatting** — rendering a timestamp needs a calendar, time zones and
a locale this project has decided nothing about — and **no sleeping**, because a blocking call in
the comptime VM would mean compilation that pauses. A game loop that wants to yield between
frames reaches for `Window.delay` instead (see [Book IV](/games/the-game-loop/)), since that
module already cannot run in the VM.

## File and File_Utilities

`File` is a descriptor and the syscalls around it — every routine but `close` is `#must`:

```jr
open_read(path) -> (File, bool)     open_write(path) -> (File, bool)     open_append(path) -> (File, bool)
read(f, buffer: []u8) -> (s64, bool)        write(f, bytes: []u8) -> (s64, bool)
read_all(f, buffer) -> (s64, bool)          write_all(f, bytes) -> bool
seek(f, offset, whence) -> (s64, bool)      size(f) -> (s64, bool)
close(f) -> s64          // not #must — a descriptor that does not buffer has nothing to lose
exists(path) -> bool        remove(path) -> bool
read_entire_file(path) -> (string, bool)      write_entire_file(path, contents) -> bool
append_entire_file(path, contents) -> bool
```

Whole-file reads allocate through the context's default allocator and return an owned string that
`String.free_string` releases, including a successful empty read.

`File_Utilities` contains path operations — paths are just `string`s:

```jr
path_join(left, right) -> string        // exactly one separator between them; an absolute right wins
base_name(path) -> string   directory_name(path) -> string
extension(path) -> string   stem(path) -> string
is_absolute(path) -> bool
normalise(path) -> string               // textual only — . and .. collapsed; symlinks not resolved
```

There is no directory listing and no file metadata (size, modification time, permissions) — both
need `stat`, and a `struct stat` crossing the `#foreign` boundary by value is refused (E0286).

## Process and Socket

`Process` starts a child, waits for it, and decodes its exit status:

```jr
spawn(argv: []string) -> (Process, bool)
wait(p: *Process) -> (Exit_Status, bool)
run(argv: []string) -> (Exit_Status, bool)     // spawn then wait, in one call
succeeded(status: Exit_Status) -> bool
```

It is `fork` plus `execvp`, and it is **native-only**: `execvp`'s argument vector is an array of
pointers, one level of indirection deeper than the comptime VM's foreign-call marshalling can
translate, so the same program that works as a binary reports `EXEC_FAILED` under `jr run`.

`Socket` is a TCP client and server over `AF_INET`:

```jr
make() -> (Socket, bool)
connect_to(s, address, port) -> bool        listen_on(s, address, port, backlog) -> bool
accept_one(s) -> (Socket, bool)
send(s, bytes: []u8) -> (s64, bool)     send_all(s, bytes) -> bool     send_string(s, text) -> bool
receive(s, buffer: []u8) -> (s64, bool)     // zero means the peer closed, which is success
parse_ipv4(text) -> (u32, bool)
```

The address layout (`Sockaddr_In`) is macOS's; a Linux build needs its first two bytes widened.
There is no DNS resolution, no IPv6, and no TLS — the first two need a pointer-to-pointer shape
the FFI boundary refuses, and TLS is a protocol this library has no primitives for.

## Thread

A thin binding over `pthread`, plus a spin lock:

```jr
spawn(body: (*u8) -> *u8 #c_call, argument: *u8) -> (Thread, bool)
join(thread: *Thread) -> bool       joinable(thread: *Thread) -> bool
acquire(lock: *s64)     release(lock: *s64)     is_locked(lock: *s64) -> bool
```

There is no `Mutex` — a real one is 64 opaque platform-specific bytes this language has no way to
name — so the substitute is a spin lock that burns CPU while contended, stated rather than
hidden. The atomics (`atomic_add`, `atomic_load`, `atomic_store`, `atomic_compare_exchange`) are
language intrinsics rather than procedures here, usable without importing this module at all. A
thread body has no `context`, so it cannot allocate; it takes what it needs through `argument`.

## JSON

A flat, index-addressed node store — parsing produces a `Json_Document`, and every value is read
back by index rather than as a nested structure:

```jr
parse(text: string) -> (Json_Document, bool)
kind_of(doc, at) -> Json_Kind      string_of(doc, at) -> string      number_of(doc, at) -> float64
integer_of(doc, at) -> s64         is_integer_at(doc, at) -> bool    is_true(doc, at) -> bool
array_count(doc, at) -> s64        array_at(doc, at, index) -> s64
member_count(doc, at) -> s64       member(doc, at, name) -> s64      has_member(doc, at, name) -> bool
member_at(doc, at, index) -> s64   key_of(doc, at) -> string
free_document(doc: *Json_Document)
```

There is no `#scope_module` in this file, so its 27 internal parsing helpers are technically
public alongside the surface above — read them, don't call them. There is **no serialisation**:
writing a document back out needs a correctly-rounded `float64`-to-decimal conversion this
library does not have yet, so `JSON` can only read.

## Bucket_Array

A growable sequence of `s64` whose element **addresses never move**, once pushed:

```jr
make() -> Bucket_Array
push(b: *Bucket_Array, value: s64) -> *s64     // a stable pointer, valid for the array's life
get(b: *Bucket_Array, index: s64) -> *s64       value_at(b, index) -> s64
bucket_count(b: *Bucket_Array) -> s64
free_all(b: *Bucket_Array)
```

It buys that stability by never compacting: there is **no removal**, because a hole's fate — move
elements and break the address promise, or leave a tombstone every read must check — has no
answer this module can give without knowing what the caller wants. It is the entity-storage
answer `List` cannot give, since reallocating a `[..]s64` invalidates every pointer into it.

## Generic_Types

The one library module that exists to *exercise* a language capability rather than to be used
directly — two parameterised structs, generic across a module boundary:

```jr
Box :: struct($T) { value: T; }
Pair :: struct($A, $B) { first: A; second: B; }
```

Kept deliberately separate from `Array`, `List` and `Map`: converting those three concrete
containers to real `$T` / `$K, $V` procedures is a library rewrite, and proving that a
parameterised struct now crosses a module boundary at all is a language change — keeping them
apart is what makes a regression in either attributable.

## Compiler

The vocabulary a build script writes in: `jr build --script build.jr` compiles this file's
caller, runs it in the bytecode VM, and performs the compilations it asks for.

```jr
create_target(name: string) -> Target
options(t: Target) -> Build_Options       set_options(t: Target, o: Build_Options)
add_file(t: Target, path: string)
build(t: Target) -> bool                  // #must
command(program: string) -> Command       argument_of(c: Command, value: string)
run(c: Command) -> s64                    output(c: Command) -> string
shell(program: string, args: []string) -> s64
```

`Build_Options` covers eight fields — output name and path, optimisation level, bounds checks,
backend, module and library search paths, and output kind (`EXECUTABLE`, `DYNAMIC_LIBRARY`,
`STATIC_LIBRARY`, `OBJECT`) — against Jai's roughly sixty; these are the ones real build scripts
actually set. A script that shells out uses `command` / `run` / `output` rather than
`modules/Process`, because the **driver** spawns the process with ordinary strings, sidestepping
the pointer-marshalling limit that makes `Process` native-only.

## The game and graphics stack

Six low-level modules put a window, input, and 2D drawing on OpenGL. `Game` has begun wrapping their
lifecycle into a smaller Jairs-native interface. The low-level separation and immediate-mode
vocabulary are Simp-shaped, but the API is a Jairs subset rather than an exact port of one
unspecified Jai beta. [Book IV — Games with Jairs](/games/) is the full walkthrough; here is the
two-line map.

### Game

`Game.App` owns window startup, one per-frame event drain, close handling, monotonic delta time,
presentation, and teardown order. The foundation deliberately stops there: held/pressed/released
input, shape helpers, generation-tagged textures, PNG, text, and audio remain later slices. The
current games book therefore still teaches the lower-level modules directly until the facade reaches
its documented game-ready threshold (ADR-0210).

### Simp

An immediate-mode 2D renderer over OpenGL, with **no state argument** — the render state lives in
file-scope globals, following the common shape of the public Jai snapshots ADR-0208 inspected.
`set_render_target`, `clear_render_target`,
`immediate_quad` and `immediate_flush` draw quads and colours; there is no text and no fonts. See
[Drawing with Simp](/games/drawing-with-simp/).

### Window

Window creation over SDL2 — Jai's `Window_Creation`, narrowed to just the window once `Simp` and
`Input` took the renderer and the event queue. It does not run under `jr run`: the comptime VM
resolves foreign symbols from the compiler's own process image, not a link line. See [A window and
an event loop](/games/window-and-events/).

### Input

An SDL2 event queue with no module-owned state — a caller declares an `Events` buffer and drains
SDL into it every frame. There is no held-key state and no keycode table beyond `KEY_ESCAPE`; a
keycode under 128 is its ASCII value. See [A window and an event loop](/games/window-and-events/).

### UI

Immediate-mode widgets — a button is one call that both draws and reports whether it was clicked
— built on `Input` and `Simp` through aliased imports. It hit-tests **y-down**, so a program with
widgets must render with `Simp.LEFT_HANDED`. See [Immediate-mode UI](/games/immediate-mode-ui/).

### Image

Loads and saves **BMP only** — `SDL_image` or an inflate implementation would be needed for PNG —
and turns a `Surface` into a `Simp.Texture` that `GL` can bind. See [Textures and
images](/games/textures-and-images/).

### GL

A hand-written binding to 36 GL entry points across versions 1.1 through 2.0 — enough for a
shader-based 2D renderer, not a general GL wrapper. Only `glDeleteTextures` is bound for cleanup;
a shader, program or buffer this module creates cannot be released through it. See [Drawing with
Simp](/games/drawing-with-simp/).

## Reading the source

Every one of these is a readable `.jr` file, and reading them is a good way to learn idiomatic
Jairs — how the allocator protocol is used, how a two-value return is spelled, how a view
borrows a buffer. The [Book III](/in-practice/) programs put several of them together.

Next: [Concurrency](/language/concurrency/) — threads, atomics, and the memory model.
