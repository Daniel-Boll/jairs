---
title: Processes
description: Starting a child process with fork and execvp, decoding waitpid's status into a struct, and why spawn works in a built binary and fails under jr run.
sidebar:
  order: 100
---

`Process` binds `fork` and `execvp` rather than `posix_spawn`: the modern answer takes two attribute
structs by pointer whose layouts are opaque and platform-specific, and this language would have to
hard-code sizes and offsets to use them — guessing rather than portability, and E0286 refuses an
aggregate by value at a `#foreign` boundary in any case. `fork` plus `execvp` is scalars and pointers
throughout, which is the FFI shape that works (ADR-0158).

## Starting a child, and decoding what it did

```jr
#import "Basic";

/// A child process.
Process :: struct {
    /// The child's process id. Negative when the spawn failed.
    pid: s64;
}

/// How a child finished.
Exit_Status :: struct {
    /// Whether the child exited normally, rather than being killed.
    exited: bool;
    /// The exit code, when `exited`. Zero otherwise.
    code: s64;
    /// The signal that killed it, when not `exited`. Zero otherwise.
    signal: s64;
}

/// Starts `argv[0]` with `argv` as its arguments, searching `PATH`.
///
/// `#must`: a caller who ignores the flag holds a `Process` with a negative pid, and `wait` on it fails
/// forever.
///
/// `argv` is the whole argument vector **including the program name**, which is C's convention: a program
/// reads its own name from `argv[0]`, and a wrapper that inserted it would take that choice away from a
/// caller who wants `argv[0]` to differ from the path.
spawn :: (argv: []string) -> (Process, bool) #must { ... }

/// Waits for `p` to finish and decodes its status.
///
/// `#must`: the status is the whole point of waiting, and a caller who drops it has started a process and
/// learned nothing.
///
/// Blocks. There is no `try_wait`, because `WNOHANG` returns "not finished yet" as a *third* outcome and a
/// caller polling in a loop is busy-waiting.
wait :: (p: *Process) -> (Exit_Status, bool) #must { ... }

/// Runs `argv` to completion and returns its status — `spawn` then `wait`.
run :: (argv: []string) -> (Exit_Status, bool) #must { ... }

/// Whether `status` means "finished normally with code 0".
///
/// The question almost every caller actually has, named so that nobody writes `status.code == 0` and
/// forgets that a killed process also has `code == 0`. That mistake reads as correct and is the reason
/// this exists.
succeeded :: (status: Exit_Status) -> bool { ... }
```

`waitpid` writes a single `int` that packs "exited with code N", "killed by signal N" and "stopped" into
bit patterns the C `W*` macros decode — and macros are not something a `#foreign` binding can reach, so
this module decodes the bits itself. Returning the raw integer would push that decoding onto every
caller, and a caller who compares it against an exit code directly gets the wrong answer for *every*
non-zero status: `exit(1)` produces the raw value `256`, not `1`. A struct with named fields removes the
single most common bug in this area, and `succeeded` exists because `status.code == 0` reads as correct
and is wrong for a process that was killed.

`Process` decodes `waitpid`'s status against the BSD/macOS layout, which Linux shares for the two cases
this module reports: the low seven bits are the terminating signal and `0x7f` means stopped, so `status &
0x7f == 0` means a normal exit and `(status >> 8) & 0xff` is the code.

## `execvp`, and why it is the only one of the six that works

```jr
/// libc `execvp(3)`: `execvp(const char *file, char *const argv[])`, **fixed-arity**.
///
/// Returns only on failure — on success the process image is replaced and there is nothing to return to.
/// That inverted contract is why `spawn` treats a *return* as the error case.
execvp_raw :: (file: *u8, argv: *Argument) -> s64 #foreign libc "execvp";

/// libc `_exit(2)` — exits **without** running atexit handlers or flushing.
///
/// The child uses this after a failed `execvp`, and the distinction from `exit` matters: `exit` would flush
/// the buffers this process inherited from its parent, writing the parent's pending output a second time.
underscore_exit_raw :: (status: s64) #foreign libc "_exit";

/// The exit code `spawn` gives a child whose `execvp` failed.
///
/// 127 is the shell's convention for "command not found", so a caller who sees it recognises it.
EXEC_FAILED :: 127;
```

C's `exec` family has six members. `execl`, `execle` and `execlp` are variadic, and a fixed-arity
declaration of a variadic C function passes the extra arguments in the wrong place, silently — the exact
hazard `File`'s `open` binding hit. So `execvp` is not a style preference; it is the only one of the six
that can be called correctly today. It also searches `PATH`, which is what a caller passing `"ls"`
expects — a caller who wants no search passes an absolute path.

Between `fork` and `execvp` the child is a byte-for-byte copy of this process, and in the comptime VM that
means a copy of the **interpreter** — correct, but why `spawn` does as little as possible in the child,
and why the child calls `_exit` rather than `exit` on a failed `execvp`: `exit` would flush the buffers
inherited from the parent, duplicating the parent's pending output.

## Why `spawn` fails under `jr run`

**`Process.spawn` works in a built binary and fails under `jr run`.** This is a property of the comptime
VM's memory model, not a bug this module could fix by trying harder. The VM satisfies allocation from its
own linear region, so a Jairs pointer is an offset into that region, and a foreign call translates it to
a host address on the way out — one level deep, and only ever one level deep: the VM knows a
*parameter* is a pointer, and it cannot know that the bytes behind it contain more pointers.

`execvp`'s second argument is exactly that: an array of pointers. libc receives a real host address for
the array itself and region-relative garbage for every string inside it, so `execvp` fails and the child
exits `EXEC_FAILED` (127). Natively there is no translation and nothing to get wrong, so the same program
works. A minimal `fork`/`execvp` program run under `jr run` exits **127**, the same status the child gives
when `execvp` cannot find a command at all — probed, not assumed.

This is why `Process`'s own test is a `jr-cli` integration test rather than a corpus program:
`tests/corpus/valid/` exists on the premise that both engines agree, and this module's defining behaviour
is exactly the case where they cannot. `Socket`, covered on the [next page](/by-example/101-sockets/), is
unaffected by the same limitation, because a `sockaddr_in` passed by pointer holds only integers and one
level of translation is enough — "passes a struct by pointer" sounds like the harder case and is the
easier one.

**A build script hits the same wall.** [`Compiler.command`](/by-example/120-build-scripts/) exists
because a build script needs to shell out — `git rev-parse`, a shader compiler — and `Process.run` cannot
do it while the script itself runs under `jr run`. The driver spawns the process with ordinary Rust
strings instead, so there is no pointer array to marshal.

## What is absent, and why

There is <span class="jairs-status absent">absent</span> output capture: `Process` has no pipes, so a
caller cannot read a child's stdout directly. Capturing output needs a read loop while the child runs, or
risks a deadlock when the pipe fills — a concurrency question, and this module predates `Thread`. A
caller who wants a child's output today redirects it to a file with `File` and reads the file after
`wait`, which is deadlock-free by construction.

There is also <span class="jairs-status absent">absent</span> environment control (`execvpe` is not
POSIX; `setenv` before `spawn` works, since the child inherits) and <span class="jairs-status
absent">absent</span> signal handling — sending one with `kill` is easy, but *installing* a Jairs handler
means a procedure pointer called from a signal context, and this language has said nothing about that
reentrancy.

See also [Book I — The Jairs Language](/language/introduction/).
