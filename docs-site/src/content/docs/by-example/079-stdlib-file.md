---
title: File and paths
description: Opening, reading, writing and closing a descriptor with every failure receivable, plus path joining, splitting and whole-file reads from File_Utilities.
sidebar:
  order: 79
---

`File` is a plain `s64` descriptor in a one-field struct — not a buffered stream — and every routine that
can fail returns a success flag marked `#must` (ADR-0151, ADR-0157). `File_Utilities` sits on top of it
and knows about **paths**, which are ordinary strings rather than a `Path` type: a `Path` that could still
hold `"\0/../"` would prove nothing about validity, so it would be a type that suggests a check it does
not perform.

## Opening, reading, writing

```jr
/// An open file.
///
/// A struct rather than a bare `s64` so that a descriptor cannot be passed where a count is wanted: the two
/// are both integers and the compiler will not confuse them once one has a name.
File :: struct {
    /// The OS descriptor. Negative when the file is not open.
    fd: s64;
}

/// Open for reading only.
READ :: 0;

/// Open for writing only.
WRITE :: 1;

/// Open for reading and writing.
READ_WRITE :: 2;

/// Opens `path` with `flags`, creating it with `CREATE_MODE` when `CREATE` is set.
///
/// `#must`: a caller who ignores the flag holds a closed file and every later call fails quietly.
open :: (path: string, flags: s64) -> (File, bool) #must { ... }

/// Opens `path` for reading.
open_read :: (path: string) -> (File, bool) #must {
    return open(path, READ);
}

/// Opens `path` for writing, creating it and truncating it.
open_write :: (path: string) -> (File, bool) #must {
    return open(path, WRITE + CREATE + TRUNCATE);
}

/// Opens `path` for appending, creating it if it does not exist.
open_append :: (path: string) -> (File, bool) #must {
    return open(path, WRITE + CREATE + APPEND);
}

/// Closes `f` and marks it closed. Returns the OS status.
///
/// Not `#must` — see the module docs. Safe on an already-closed file, so a caller can close on every path
/// without tracking whether they already did, which is what makes it usable where a `defer` would go.
close :: (f: *File) -> s64 { ... }

/// Reads up to `buffer.count` bytes into `buffer`, returning how many and whether the call succeeded.
///
/// **A short read is success, not failure**, and zero bytes means end of file rather than an error.
read :: (f: *File, buffer: []u8) -> (s64, bool) #must { ... }

/// Writes `bytes`, returning how many were written and whether the call succeeded.
///
/// A short write is success here too, for the same reason, and `write_all` is the looping version.
write :: (f: *File, bytes: []u8) -> (s64, bool) #must { ... }

/// Writes every byte of `bytes`, looping over short writes. Returns whether all of them landed.
write_all :: (f: *File, bytes: []u8) -> bool #must { ... }

/// Fills `buffer` as far as it can, looping over short reads, and returns how many bytes it got.
read_all :: (f: *File, buffer: []u8) -> (s64, bool) #must { ... }
```

`close` is the one routine here that is **not** `#must` (ADR-0157 §3), and the asymmetry is deliberate
rather than an oversight: a failing close means buffered data was lost, and this module does not buffer,
so there is nothing to lose. Every other routine can fail for a reason a caller has to handle — an empty
read from a file that never opened is exactly the bug `#must` exists to catch — and before ADR-0151 there
was no way to say that ignoring the flag was an error rather than a style choice.

## The flags a `#run` computes per target

```jr
/// Create the file if it does not exist. Combine with `WRITE` or `READ_WRITE`.
///
/// **Selected for the target** (ADR-0184 §6), where it used to be macOS's number under a comment admitting
/// so: `O_CREAT` is 0x200 on macOS and 0x40 on Linux. The three access modes above *are* portable — 0, 1, 2
/// everywhere — which is why only these three flags need selecting.
CREATE :: #run create_flag();

/// `O_CREAT` for the target (ADR-0184 §6).
///
/// **Windows gets Linux's number and that is a placeholder, not a fact.** Windows has no POSIX `open`
/// flags at all — its `_open` uses `_O_CREAT` = 0x100 — and this module's `#foreign` bindings name POSIX
/// symbols that a Windows libc does not export under these names. So a Windows build fails to *link*
/// rather than opening a file with the wrong flag, which is the failure mode to prefer.
create_flag :: () -> s64 {
    if os() == Operating_System.MACOS {
        return 512;
    }
    return 64;
}
```

`CREATE`, `TRUNCATE` and `APPEND` are each a `#run` of a per-OS procedure rather than a `#if`-guarded
literal, because Jairs has no `#if` on a target — `os()` is a compile-time **value** (ADR-0180), so the
per-OS number is an ordinary branch evaluated once. The three flags actually differ between macOS and
Linux; the three access modes `READ`/`WRITE`/`READ_WRITE` do not, which is why only the flags need this
treatment.

## Why `creat`, and not a three-argument `open`

C declares `open(const char *, int, ...)`, reading a third `mode_t` only when `O_CREAT` is set. That makes
it variadic, and this project measured what a **fixed-arity declaration of a variadic C function** does:

```jr
/// libc `open(2)`, called with **only its fixed arguments**.
///
/// **This is the flag whose being wrong is worst.** ADR-0157 §2 measured a mis-declared `open` producing a
/// file with permissions `---------x`: no diagnostic, a plausible-looking result, unreadable. A wrong
/// `O_CREAT` is the same failure shape one step earlier — the call succeeds and does something else.
open_raw :: (path: *u8, flags: s64) -> s64 #foreign libc "open";

/// libc `creat(2)`: `open(path, O_WRONLY | O_CREAT | O_TRUNC, mode)`, and **not variadic**.
///
/// The one portable way to create a file with a known mode without a variadic call. Its fixed shape is the
/// whole reason it is used, which is why it is here rather than `open` with three arguments.
creat_raw :: (path: *u8, mode: s64) -> s64 #foreign libc "creat";
```

Declaring `open(path, flags, mode)` and calling it created a file with permissions `---------x` on arm64
macOS: variadic arguments go on the stack while a fixed third argument goes in a register, so the mode
never arrives where `open` looks for it. No diagnostic, a plausible-looking file, and unreadable — exactly
the silent-wrong-answer shape this project refuses everywhere else. So `open` decides which of three
shapes to take (a plain `open_raw`, a `creat_raw` when creation and truncation are both wanted, or an
`open_raw` that falls back to `creat_raw` when the file is missing) and never passes a mode to the
variadic call.

## Paths, as text

```jr
/// Joins `left` and `right` with exactly one separator between them.
///
/// **An absolute `right` wins**: `path_join("a", "/b")` is `"/b"`, and so is `path_join("a/", "/b")`. That
/// matches Python's `os.path.join` and Rust's `PathBuf::push`, because a caller who supplied an absolute
/// path meant it.
///
/// Allocates through `context.allocator`; `String.free_string` releases it.
path_join :: (left: string, right: string) -> string { ... }

/// Everything after the last separator, or the whole path when there is none.
base_name :: (path: string) -> string { ... }

/// Everything before the last separator, or `""` when there is none.
directory_name :: (path: string) -> string { ... }

/// The extension, without its dot, or `""`.
extension :: (path: string) -> string { ... }

/// The base name without its extension.
stem :: (path: string) -> string { ... }

/// Resolves `.` and `..` **textually**, and collapses repeated separators.
normalise :: (path: string) -> string { ... }

/// Reads the whole file at `path`.
read_entire_file :: (path: string) -> (string, bool) #must { ... }

/// Writes `contents` to `path`, replacing whatever was there.
write_entire_file :: (path: string, contents: string) -> bool #must { ... }

/// Appends `contents` to `path`, creating it if it does not exist.
append_entire_file :: (path: string, contents: string) -> bool #must { ... }
```

`path_join`, not `join`: `String` gained its own `join` that concatenates a `[]string` with a separator —
Jai's own name for that — and `#import` is a flat merge (ADR-0014), so a file importing both modules got
E0211 on every unqualified use. `path_join` is Jai's name for *this* routine, so the collision resolved
into the naming the language being followed already had (ADR-0197 §7). A games program that loads
levels from disk imports both `File_Utilities` and `String` in the same file, so this collision is one it
will hit on day one if it reaches for the wrong name.

`base_name`, `directory_name`, `extension` and `stem` all **borrow** — they slice their argument rather
than allocate — so nothing here needs freeing except `path_join`'s result and the whole-file reads.

## What is absent, and why

`File` has no `errno`: a failure says *that* it failed, not *why*. Reading `errno` needs a thread-local,
which this language has no concept of, and the two platform functions that expose it differ by name and
shape between macOS and Linux — a wrong `errno` names a cause that is not the cause, which is worse than
none.

There is <span class="jairs-status absent">absent</span> comptime file reading: `#foreign` is refused at
compile time (ADR-0006), so a `#run` cannot call anything in this module. A build script that needs to
read a file at compile time uses `modules/Compiler`'s own `read_file`/`write_file` instead — see
[A build script in Jairs](/by-example/120-build-scripts/) — because those calls are host-mediated by the
driver rather than `#foreign`.

`File_Utilities` has <span class="jairs-status absent">absent</span> directory listing and file
metadata — no `readdir`, no size-by-path, no modification time. `readdir` returns a `struct dirent` by
value or by pointer, whose layout differs per platform, and reading through it needs field offsets this
module would have to hard-code; E0286 also refuses an aggregate at a `#foreign` boundary today
(ADR-0150). `File.size` sidesteps the whole question by seeking on an *open* file rather than asking the
filesystem.

See also [Book I — The Jairs Language](/language/introduction/).
