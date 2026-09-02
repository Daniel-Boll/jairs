# ADR-0157: `File` and `File_Utilities` — and two silent divergences the filesystem found

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- **PLAN §8.3 item 5** of W7's nine modules, both halves. The last of the four modules the error model was
  holding up before `#must` (ADR-0151) existed.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Context

These are the first modules whose *correctness depends on something outside the program*. That changed what
the wave found: two defects, both **silent**, both invisible to any test that did not touch a real filesystem
and run the native binary. Neither is in the modules — one is an FFI miscompile and one is a VM/native
divergence — and finding them is the argument for `valid/128` writing to `/tmp` rather than mocking.

## Decision

### 1. Descriptors, not streams; paths as text, not a type

`File` is a **descriptor in a one-field struct**. `read` and `write` are the syscalls, byte counts and all.

**Rejected: a buffered `File`.** It has to answer where the buffer comes from, when it flushes, and what
happens to unflushed bytes when the handle goes out of scope. There are no destructors (ADR-0008), so the
last answer is "they are lost unless you remember to flush" — a worse bargain than "you called `write` and it
wrote". Buffering is a real feature that belongs to a module saying so in its name.

The struct rather than a bare `s64` is so a descriptor cannot be passed where a count is wanted; both are
integers and the compiler will not confuse them once one has a name.

`File_Utilities` takes **paths as `string`**. **Rejected: a `Path` type**, which buys exactly one thing —
refusing a string where a path is wanted — and costs a conversion at every boundary including every literal.
Worse, it would be a *lie about validity*: a `Path` that can hold `"\0/../"` proves nothing, so the type
suggests a check it does not perform.

### 2. A fixed-arity declaration of a variadic C function is a silent miscompile

`open(2)` is `open(const char *, int, ...)`, and it reads a third `mode_t` only when `O_CREAT` is set. The
obvious binding is `open_raw :: (path: *u8, flags: s64, mode: s64) -> s64 #foreign libc "open"`.

It is wrong, and **nothing says so**. On arm64 macOS a variadic argument goes on the stack while a fixed third
argument goes in a register, so the callee reads whatever was on the stack: the probe created a file with
permissions `---------x`. No diagnostic in either engine, a file that exists, and unreadable — the
silent-wrong-answer shape this project refuses everywhere else, arriving through the FFI.

So `File` calls `open` with **only its fixed arguments** — well-defined on every ABI — and routes creation
through **`creat(2)`**, which is genuinely fixed-arity and is exactly `open(path, O_WRONLY|O_CREAT|O_TRUNC,
mode)`. The three shapes fall out:

- no `CREATE`: one `open`;
- `CREATE` + `TRUNCATE`: one `creat`, plus a reopen only when the caller also wanted read access, since
  `creat` gives write-only;
- `CREATE` alone (an append): try `open` first, and `creat` only when the file is missing — so an existing
  file's contents survive, which is what `APPEND` means.

**Rejected: declaring the three-argument form and hoping.** It is what this wave wrote first and it is why the
probe happened. **Rejected: waiting for C-variadic support** (PLAN §8.5), which would block a module that has
a correct shape available.

Recorded in PLAN's known-defects list as two separable halves: *refusing* it needs the compiler to know which
symbols are variadic, which it cannot learn from a Jairs declaration — so the honest form is a `#c_variadic`
marker whose absence means "not variadic" — and *supporting* it is §8.5's item. `File` does not need the
support; a module that needs `printf` or `ioctl` will.

### 3. Every routine is `#must` except `close`

`open`, `read`, `write`, `seek`, `size`, `remove` and the whole-file three return a flag and it is `#must`. A
caller who ignores it reads zero bytes from a file that failed to open and processes a silently empty result,
which is the bug the marker exists for. `_ = f();` is the escape hatch, and writing it is the point.

`close` is **not** `#must`, and the asymmetry is deliberate. A failing `close` means buffered data was lost —
but this module does not buffer, so there is nothing to lose; and a caller who checks it has no recovery
available, because the descriptor is gone either way. It returns the status for a caller who wants to log it
and does not insist. It is also safe on an already-closed file, which is what lets a caller close on every
path without tracking whether they got one — the shape a `defer` would want.

`write_entire_file` **does** check the close status, because its promise is "the file now holds these bytes"
and a failed close is the one way that promise breaks after a successful write. Same mechanism, different
promise, and the difference is stated at both sites.

**A short read or write is success**, and zero bytes from `read` is end of file. That is the syscall's
contract and hiding it would be wrong: a caller reading a pipe has to loop either way, and a wrapper that
looped for them would block where the caller wanted a partial answer. `read_all` and `write_all` are the
looping versions, and `write_all` treats a zero-byte write with no error as a failure rather than spinning —
"should not happen" is exactly what a loop condition must not rely on.

`size` uses **two seeks, not `stat`**, because a `struct stat` cannot cross the `#foreign` boundary today
(E0286, ADR-0150; PLAN §8.1.2's hard gate). Recorded because `stat` is what a reader expects. For the same
reason there is **no directory listing** and **no metadata by path**: `readdir` needs `struct dirent` field
offsets this module would have to hard-code per platform, which is guessing rather than portability.

**No `errno`.** A failure says *that* it failed, not *why*. Reading `errno` needs a thread-local, which this
language has no concept of (`Thread` is W11), and `__error` / `__errno_location` differ by platform in name
and shape. A wrong `errno` names a cause that is not the cause, which is worse than none.

### 4. `String.adopt` and `String.borrow` — one construction, two obligations

`String.make_string` was module-private, and ADR-0156 exported it as `adopt` for JSON's unescaper. This wave
found the other half: `File_Utilities.base_name` returns a **slice of its argument**, which must not be freed.

So `String` exports both names over the same construction: **`adopt`** takes ownership, **`borrow`** does not.
The operation is identical and the obligation is not, so the call site says which one it is.

**Rejected: one `make`.** Every reader would work out the ownership from context, which is how a double free
gets written — and this wave wrote one (§5) with the names available, which is the argument for making the
distinction loud rather than quiet. **Rejected: `base_name` copying**, which would make every caller free
something they did not ask to own, for a routine whose whole job is to point at part of a path.

Nothing checks a borrow's lifetime — there is no lifetime system here — so the rule is the call site's to
keep. Naming it is the most the language can do today, and the doc says so rather than implying a guarantee.

### 5. Freeing a string literal runs in the VM and aborts natively

`normalise` accumulates into a string, and its first draft started with `out := "";` and freed `out` a few
lines later when replacing it. That is a shape **any** accumulate-into-a-string routine has.

`String.free_string`'s doc said it is "safe on a `""` result", which is true and was read as covering the
`""` **literal**, which it does not: an allocating routine returns `null` data for a zero count, while a
literal's `data` is a real pointer into the program's own read-only data. Freeing it hands the allocator
something it never allocated.

Natively that aborts (SIGABRT). Under `jr run` it is **clean**, because the comptime VM satisfies
`malloc`/`free` from its own region (ADR-0061) and quietly drops a pointer it does not recognise. So the
program ran, printed the right answer, passed every check — and died as a binary.

Two things came out of it. The module starts with `substring("", 0, 0)` — a computed empty, safe to free —
and that one line is the whole fix. And `free_string`'s doc now draws the distinction explicitly, names the
divergence, and says what to write instead.

Making the VM **trap** on a foreign pointer is the real fix and is its own decision, recorded rather than
taken: it would also refuse a pointer that a `#foreign` `malloc` produced at run time, which is legal.

**This is the divergence class the differential harness exists for**, and it only catches it when a corpus
program does it — which is the argument for `valid/128` writing to a real `/tmp` rather than mocking a
filesystem. A mocked test would have passed in both engines and shipped the abort.

### 6. Path rules, chosen and stated

`join` inserts **exactly one** separator, and an **absolute right-hand side wins** — matching Python's
`os.path.join` and Rust's `PathBuf::push`, because a caller who supplied an absolute path meant it. The first
draft also stripped leading separators from the right, which contradicted the absolute rule two paragraphs
apart: `join("a/", "/b")` cannot be both `"a/b"` and `"/b"`. The absolute rule is the one worth keeping and
the stripping is gone.

`base_name("/a/b/")` is `""`, because a path ending in a separator names a directory and has no base name;
returning `"b"` would be inventing one. `directory_name("c.txt")` is `""`, not `"."`, because `""` is what
the path *says* — a caller who wants the interpretation can test for empty, while a caller handed `"."`
cannot tell it from a path that really said `"."`.

`extension` takes the **last** dot, so `"a/b.tar.gz"` is `"gz"`: `"tar.gz"` is a convention, not a filename
part. A leading dot is not an extension (`.bashrc`), and a dot in a *directory* name is not one either
(`a.b/c`) — the two cases a naive implementation gets wrong. `stem` uses the same rule, so `stem` and
`extension` partition the base name between them, which is worth having since the two are almost always used
together.

`normalise` is **textual and says so**. It never touches the filesystem, so it is deterministic and cheap —
and wrong for symlinks, since `a/link/..` is `a` textually and is whatever `link` sits beside in reality. The
filesystem-resolving version is `realpath`, which needs a `PATH_MAX` buffer this module would have to invent,
and is deferred. A `..` that would escape a **relative** path is kept, because `../x` means something; a `..`
at the root is dropped, because `/..` is `/`; and a relative path that normalises to nothing is `"."`, not
`""`, because `""` names nothing a caller could open.

## Consequences

- **`modules/File` and `modules/File_Utilities`** are new; `modules/String` exports `borrow` beside `adopt`
  and its `free_string` doc is corrected.
- **`valid/128` touches a real filesystem** under `/tmp`, with names no other test uses, and removes what it
  creates — so a second run sees the first run's state. Twenty-four independent bits; all three engines print
  16777215 and exit 124.
- **Two defects recorded in PLAN**: the variadic-FFI miscompile and the VM's tolerance of a foreign `free`.
- **W7 is six of nine.** `Process` and `Socket` remain, both unblocked by the error model; `Compiler` shipped
  inside W6 and `Thread` is W11.
