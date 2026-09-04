# ADR-0158: `Process` and `Socket` — W7 closes, and the VM's pointer boundary is drawn

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- **PLAN §8.3 items 6 and 7**, the last two of W7's nine modules. **W7 — Stdlib is DONE.**
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Context

`Process` and `Socket` are the two modules PLAN §8.3 marked "the error model is the whole difficulty", and
they are also where this language's FFI reaches its edge. Between them they turned up one leaked internal
error, one fixed pool invariant, and a **boundary the comptime VM cannot cross** — which decides where a
module's test can live.

## Decision

### 1. `view_of` interns the element pointer, so no view constructor can forget

`modules/Process` builds a C `argv` as a `[]Argument` over `talloc` storage, and the comptime VM refused it
with the internal error **"a view's element pointer type was never interned"**.

Reading `view[i]` needs `*elem` in the pool: a view's element place is its `data` word indexed, and both back
ends *look the pointer type up* rather than construct one. `Pool::static_array` knew this and interned it,
with a comment saying it was the only constructor that had to, "because every other way of making a view goes
through a type annotation, which interns the pointer as a side effect of resolving `[]T`".

That was true until `view(p, n)` over a **struct** element type with no annotation anywhere. So the
obligation moved into **`Pool::view_of`**, the one constructor every view goes through, and `static_array`
now relies on it.

**Rejected: interning it in `check_view` too**, which fixes the second site and waits for the third. An
invariant enforced per-caller is one a caller will miss, and this is the proof.

### 2. `Process` binds `execvp`, and the choice is load-bearing

`execvp(const char *file, char *const argv[])` is **fixed-arity**. Its siblings `execl`, `execle` and
`execlp` are variadic, and ADR-0157 §2 established what a fixed-arity declaration of a variadic C function
does: it passes the extra arguments in the wrong place, silently. So `execvp` is not a style preference — it
is the only one of the six callable correctly today, and the SDK header was read rather than assumed.

`waitpid(pid_t, int *, int)` is fixed 3-arg; `fork(void)` and `_exit(int)` take no pointers. The whole module
is scalars and pointers.

**Rejected: `posix_spawn`**, the modern answer, which takes two attribute structs by pointer whose layouts
are opaque and platform-specific — this module would have to hard-code sizes and offsets, which is guessing
rather than portability, and E0286 refuses an aggregate at the boundary anyway (ADR-0150).

The child uses **`_exit`, not `exit`**, after a failed `execvp`. `exit` would flush the buffers the child
inherited from its parent, writing the parent's pending output a second time — a duplicated log line, and the
kind nobody suspects the child of.

**The status is a struct**, not `waitpid`'s integer. Those bits are decoded by the `W*` **macros**, which a
`#foreign` binding cannot reach, so this module decodes them itself and commits to the BSD/Linux layout
explicitly. Returning the raw integer would push the decoding onto every caller, and a caller comparing it
against an exit code directly gets the wrong answer for every non-zero status: `exit(1)` produces 256. A
struct with named fields removes the single most common bug in this area, and `succeeded()` exists because
`status.code == 0` reads as correct and is wrong for a killed process.

**No pipes, so no output capture**: capturing needs a read loop *while the child runs*, or a deadlock when
the pipe fills — a concurrency question, and concurrency is W11. A caller redirects to a file with `File` and
reads it after `wait`, which is deadlock-free by construction. **No environment control**: `execvpe` is not
POSIX and `environ`'s declaration differs per platform; `setenv` before `spawn` works, since the child
inherits. **No signal handling**: installing a Jairs handler means a procedure pointer called from a signal
context, and this language has said nothing about that reentrancy — offering only `kill` avoids inviting the
hard half.

### 3. The VM cannot pass a pointer to memory that itself contains pointers

**`Process.spawn` works in a compiled binary and fails under `jr run`.** This was probed, not deduced: a
minimal `fork`/`execvp` program prints 0 as a binary and the "execvp returned" marker in the VM.

The cause is structural. The VM satisfies allocation from its own linear region (ADR-0061), so a Jairs pointer
is an offset into that region, and a foreign call translates it to a host address on the way out. That
translation is **one level deep and can only ever be**: the VM knows a *parameter* is a pointer, and it cannot
know that the bytes behind it hold more pointers. `execvp`'s `argv` is exactly an array of pointers, so libc
gets a real address for the array and region-relative garbage for every string in it.

**Rejected: refusing a foreign call whose pointee type contains a pointer.** It is decidable, and it would
turn a silent failure into a diagnostic — which is normally this project's rule. It also refuses `strtod`'s
`char **end` out-parameter, which `JSON` uses and which **works**: the callee *writes* a pointer there rather
than reading one, and no type distinguishes those two uses. A refusal that breaks working code in order to
describe broken code is the wrong trade.

The real fix is marshalling the pointee recursively from its known type, and it is a `jr-vm` wave: it needs a
shadow copy per call, because the VM's own memory must keep its region-relative values, plus a decision about
what an out-parameter's pointers mean on the way back. Recorded in PLAN's known-defects list.

**So `Process`'s test is a `jr-cli` integration test, not a corpus program.** `tests/corpus/valid/` exists on
the premise that both engines agree, and this program legitimately cannot hold it — the same conclusion
ADR-0126 reached when the VM trapped where native code wrote short. The test builds the binary and asserts
its exit status.

`Socket` is **unaffected**, and the contrast is worth stating because it inverts the intuition: a
`sockaddr_in` passed *by pointer* is fine, since it holds only integers and one level of translation is
enough. "Passes a struct by pointer" sounds like the harder case and is the easier one.

### 4. `Socket` is a separate type from `File`, and parses its own addresses

`Socket` wraps a descriptor exactly as `File` does and is deliberately a **different type**: a socket accepts
`send`/`recv`/`shutdown` and a file accepts `seek`, and the type is what stops a caller seeking a socket. The
cost is that `File`'s routines are unavailable on a socket, which is the right way round — the overlap is
smaller than it looks.

`listen_on` **binds and listens in one call**, because a bound socket that is not listening refuses every
connection and no caller wants that state; splitting them is how a caller forgets the second half.
`local_port` exists so binding **port 0** is usable — the OS picks a free port and the caller reads it back,
which is how `valid/129` opens a server without a hard-coded number that might be in use on a busy machine.
`set_reuse_address` is offered because a server restarting inside `TIME_WAIT` fails to bind, which every
server wants and every first implementation forgets.

**`parse_ipv4` is written here rather than bound to `inet_pton`**, and not to avoid the FFI: the point is that
**the refusals are ours**. `inet_aton` accepts `"1.2.3"` and `"0x7f.1"` as addresses — a documented historical
quirk, and not what a caller expects. This takes exactly four decimal octets and refuses trailing content,
because accepting a prefix is how a typo becomes a connection somewhere else.

**`to_network_port` is written by hand** for two reasons that both matter: `htons` is a *macro* on some
platforms so there may be no symbol to bind — the same reason `Process` decodes `waitpid` itself — and the
swap is two operations, so a binding would cost a foreign call to save nothing. It assumes a little-endian
host, which arm64 and x86-64 both are, and says so.

`setsockopt` gets a **`u32`** and a length of 4, because C wants a pointer to an `int`. A `*s64` would pass
eight bytes whose low four happen to be right on a little-endian host — an accident this project calls a
silent wrong answer.

**No name resolution**: `getaddrinfo` returns a linked list of `struct addrinfo`, pointers inside pointers,
which neither the FFI refusal nor §3's one-level translation can handle. **No IPv6**: `sockaddr_in6` needs a
second address type or a union, a real design question and the wrong one to answer while the aggregate
boundary is closed. **No `select` or non-blocking mode**: both exist to wait on several descriptors, which is
concurrency. **No TLS**: it is a protocol over a socket, needing primitives that do not exist here.

### 5. The layout check is a procedure, because a file-scope constant cannot compute it

`Sockaddr_In` must be the 16 bytes C's `sockaddr_in` is, and a drift would be wrong bytes on the wire rather
than a compile error. So the module answers it: `layout_is_c_compatible()`.

It went through two rejected forms. A **caller-side** `size_of(Sockaddr_In) == 16` fails in any context that
resolves the consumer without module paths — the `jr-sema` corpus harness does — for a reason unrelated to
the layout. And a **file-level constant** `LAYOUT :: size_of(Sockaddr_In) == …;` reports "a name failed to
resolve at file scope" from const-eval, because a file-level constant's MIR does not see a struct declaration
the way a body does. That is a real limit on what a file-scope constant can compute, recorded here rather
than worked around silently.

A procedure returning a `bool` rather than a build failure, because this language has no `#assert`.
`valid/129` reads it, so a drift is a failing corpus program.

## Consequences

- **`modules/Process` and `modules/Socket`** are new. **W7 — Stdlib is DONE**: nine of nine, with `Compiler`
  delivered inside W6 and `Thread` split out to W11.

  > **This sentence is false, and `docs/build-script-plan.md` §7 found it.** No module named `Compiler`
  > has ever been created — verified by three independent searches of `modules/`. ADR-0154, which closed
  > W6, does not claim one either: its Consequences list what W6 shipped and `Compiler` is not among
  > them. `PLAN.md`'s §8.3 table still carries the un-struck row saying the module "belongs to W6's
  > decision", which is where the claim came from — a *plan* to move it, read as a delivery. So W7 is
  > **eight of nine**. An ADR is immutable, so the correction is recorded here rather than by editing the
  > claim, which is how ADR-0168 handled the same shape.
- **`jr-pool`'s `view_of` interns `*elem`**, closing a leaked internal error class.
- **`valid/129` opens a real TCP connection to itself** over loopback on an OS-chosen port; all three engines
  print 32767 and exit 137. **`Process`'s test is native-only**, in `jr-cli`'s integration suite, with the
  reason in its doc comment.
- **One more defect recorded**: the VM's one-level pointer marshalling, with the refusal-based fix explicitly
  rejected and the real fix scoped.
- **W9 — Tooling and W10 — Graphics remain**, plus W11 — Concurrency. §8.1.2's aggregate-crossing change now
  blocks three named things at once: W10 entirely, `readdir`/`stat` in `File_Utilities`, and `getaddrinfo` in
  `Socket` — so one change discharges all three.
