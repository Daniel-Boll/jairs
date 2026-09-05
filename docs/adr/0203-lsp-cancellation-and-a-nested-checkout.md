# ADR-0203: The language server aborted on every cancellation, and a repository inside the repository

- **Status:** Accepted
- **Date:** 2026-09-05
- **Deciders:** dboll
- **Amends:** ADR-0024 §2 and ADR-0032 (whose cancellation contract the release profile silently
  revoked), ADR-0029 §2 (discovery now refuses a nested checkout), ADR-0199 §3 (`module_index` claims
  a name later than it did)

## Context

The report was "completion doesn't appear in Zed, it used to in Neovim". Three separate defects were
behind it, and **two of them were invisible in Neovim for the same reason**: that editor's
configuration picks the *newer* of `target/debug/jr` and `target/release/jr`, and the Zed extension
takes `target/release/jr` unconditionally. So a developer editing this compiler had usually been
talking to a debug build, and the release build had a bug the debug build cannot have.

Nothing here was found by reading code. Each was found by *asking the running system a question it
could answer with a number*: an LSP client written for the purpose, a screenshot, the wrapper script
that logged the traffic, the exit status of a process.

## 1. `panic = "abort"` made a cancellation fatal

**salsa signals cancellation by panicking.** `jr-lsp` is built on that: `catch` is
`salsa::Cancelled::catch`, a `catch_unwind`, and `answer` turns the caught cancellation into
`ContentModified`. `server.rs`'s own module docs state the contract twice — "drop the snapshot on
unwind", "a handler that panics is caught as a cancellation and answered".

`[profile.release] panic = "abort"` makes `catch_unwind` unreachable. So the first keystroke that
landed behind an in-flight request did not cancel a query — it **killed the process**: exit 134,
SIGABRT, and *no message*, because with abort the panic payload is never handled and no crash report
was written either. From the editor's side the server simply stopped answering, and Zed logged
`Get completion via jairs failed: server shut down`.

Reproduced deterministically outside any editor, by interleaving a `didChange` behind each
`textDocument/completion` — 25 rounds:

| Build | Panic strategy | Result |
|---|---|---|
| `target/release/jr` before | `abort` | **`returncode=-6`** (SIGABRT), stderr holds only the startup banner |
| `target/debug/jr` | `unwind` | survives |
| `target/release/jr` after | `unwind` | survives 40 rounds |

**The fix is the profile**, because Cargo has no per-binary panic strategy and cancellation cannot be
avoided: an editor writes while requests are outstanding, by definition. Abort in a *compiler* was a
defensible intent; its price was an editor that dies while you type. The binary grows 6.51 MB →
7.72 MB (+18.6%) for the unwinding tables, and that is recorded here rather than discovered later.

**No behavioural test can defend this**, and that is why the guard is a test about the manifest.
`cargo test` compiles with the test profile, which inherits `dev` and unwinds, so a test exercising
cancellation passes under both settings and proves nothing. `crates/jr-cli/tests/profile.rs` asserts
the setting instead, with a second test asserting the `[profile.release]` scan finds *another* key —
the same "guard against a silently-empty search" that `codes.rs` and `differential.rs` already use.

## 2. Zed's own grammar clone emptied the module index

`module_index` (ADR-0199 §3) is what lets completion offer a name the file has not imported. Its loop
marked a module name as *seen* **before** running the round-trip that decides whether the candidate is
that module at all:

```rust
if name.is_empty() || !seen.insert(name.clone()) { continue; }   // claimed here
let Some(found) = lookup.found else { continue };                // ... and rejected here
```

A file that produced a name and then failed took the name with it, so the real module below was
skipped as a duplicate. That is not hypothetical, and the trigger was *installing the extension*:
**Zed builds a dev extension by cloning the grammar's repository into the extension directory**, and
`grammars.jairs.repository` is this repository — so `editors/zed/grammars/jairs/` is a second copy of
the whole tree, 328 `.jr` files, `modules/` included. Its paths sort before the real ones, claimed
every module name, failed every round-trip, and left the index **empty**. Completion offered no
unimported name anywhere in the workspace.

Measured rather than argued, with the real server under Zed's exact invocation:

| Open file | Workspace | Unimported offers |
|---|---|---|
| inside the repository | repository root | **0** |
| in `/tmp` | `modules/` only | 571 |
| inside the repository, clone moved aside | repository root | 571 |
| inside the repository, clone present, after the fix | repository root | **571** |

Neither the name nor the round-trip was wrong; the **order** of the two was. A repeated `module_file`
probe costs nothing, because it is a query memoised on its name.

The auto-import *quick fix* kept working throughout, which is what made this confusing: it walks the
workspace list and does its own round-trip, so it never consults `module_index`. Two features
answering "which module exports this" by two routes disagreed, and the one nobody suspected was the
one a person uses on every keystroke.

## 3. Discovery descends into other projects' checkouts

Fixing §2 leaves the clone *indexed*: 328 extra files in the workspace list, so `rename` would edit a
build artefact, `references` would report matches inside it, and `workspaceSymbol` would list every
symbol twice. All three are the confident-wrong-answer failure ADR-0029 §3 is written against.

So `walk` no longer descends into a subdirectory holding a `.git` of its own. Three points make it a
rule rather than a patch for one directory:

- **A nested checkout is another project's source**, whoever put it there — Zed, a vendoring script,
  a submodule. `SKIP`'s existing entries (`target`, `node_modules`) are the same judgement made by
  name; this one is made by evidence.
- **`.git` is tested with `exists`, not `is_dir`**, because a worktree and a submodule spell it as a
  *file* holding a `gitdir:` line and both are still separate checkouts.
- **The root the caller named is never subject to it.** Roots go straight into the queue, so opening a
  repository still walks it — asserted, because getting that backwards would index nothing at all.

## 4. Two things the investigation corrected on the way

**`editors/zed/verify.sh`'s last check could never pass.** `jr lsp advertises formatting and
completion` reported FAIL while the capability was in the reply all along: under `set -uo pipefail`
the server exits non-zero once its stdin closes, and `grep -q` closes the pipe early, so the
*pipeline* failed. It is the same trap as `d03a0f0` ("the reachability check was a piped while, so it
always failed"). The reply is captured first now, and the check finally asserts **completion** too,
which its name had always promised and its body never checked.

**The README's settings snippet named the wrong key.** It said `"lsp": { "Jairs": … }`; Zed keys `lsp`
settings by the server *id* from `[language_servers.jairs]`, which is `jairs` — the name its own log
messages use. A wrong key is not reported, so the override silently never applies. Verified both
ways: with `Jairs` the default command kept running, with `jairs` the wrapper replaced it. This is
what made the traffic log possible, and it would have made every documented override fail for a user.

## 5. `Alias.` offered nothing, because an alias is not a value

Reported straight after the above, on this program:

```jai
Window :: #import "Window";
main :: () {
    Window.create_window(10, 10, "oi", 10, 10);
    Window.
}
```

The call on line 3 checks. The dot on line 4 offered **nothing**, and the reason is a fork taken in
one place: `context_at` classifies a `.` as a field access, and `fields_at` then asks what *type* the
receiver has. An alias has none — `jr-hir` deliberately keeps it out of `hir.scope`, its own comment
saying "a bare `Simp` is not a value" — so the answer was an empty list. **The one spelling that
reaches an aliased module was the one the editor was silent about**, which is ADR-0199 §5's complaint
about unimported names, in the other half of the same surface.

`Alias.` now answers with the module's [`file_exports`]: 21 items for `modules/Window`, each with its
signature, its documentation and a call snippet, and none needing an import edit because the import is
already written.

**The fork is decided the way lowering decides it, not by whether the field path found anything.**
`Lower::field_receiver_alias` treats `Alias.member` as a qualified name *unless a local or parameter
of that name is in scope* (ADR-0014 §3), so completion asks the same question in the same order: an
alias with nothing shadowing it answers with module members, and a shadowed one falls through to
fields. Falling back on "the field path returned an empty list" would have been shorter and wrong
twice — a struct with no fields would offer module members, and a shadowed alias would disagree with
the compiler about a program that checks.

Two properties are asserted rather than assumed: a `#scope_module` name is **not** offered (the
exports scope, not the raw items — the divergence ADR-0199 §6 closed for the bare form), and the list
holds *only* the module's names, no keyword and nothing from the file being edited, because a
qualified receiver admits nothing else.

A **type** position is not distinguished: `w: Window.` offers procedures too. Narrowing it needs the
CST of a file that does not parse, and the qualified type form (ADR-0179 §5) reads the same scope, so
every name offered is at least reachable through that receiver. Recorded rather than fixed.

## 6. The diagnostic on a half-typed `Window.` named a symbol nobody wrote

Fixing §5 made this visible on every keystroke after a dot: `no exported name `<error>` in module
`Window``. With no name token, lowering interns a placeholder and resolution reported *it* — the
**twelfth** internal identifier in this project to reach a place a person reads (ADR-0200 counts the
eleventh).

It was also a second complaint about one problem: the parser has already said `expected a field name
after `.``. The `None` arm of the very same function had already written that rule down — "E0210
already says so, and a second complaint about one problem is the wrong one for a reader to act on" —
so the fix is that judgement applied one arm over.

The placeholder is now `jr_hir::ERROR_NAME`, shared between the producer and the consumer **so the
two cannot drift**: a comparison against a re-spelled `"<error>"` literal would go quietly false, and
that is exactly the failure this project keeps finding in hand-maintained claims. The guard sits in
`resolve_qualified_name`, and the test runs the *binary*, because what matters is what a reader sees.

## Consequences

- `jr lsp` survives cancellation in a release build, which is the build every editor and every
  `cargo install` uses.
- Completion offers unimported names inside this repository again — verified in Zed itself, menu and
  documentation card, with the clone present.
- `Alias.` offers the aliased module's names, also verified in Zed on the reported program.
- A half-typed qualified access reports the parser's error and nothing else.
- A workspace-wide rename cannot reach a vendored checkout.
- The workspace release binary is ~19% larger.
- **11 tests, 1181 → 1192** — four of them in the second round: three for `Alias.`, one running the
  binary for §6. The baseline moved from 1178 while this was in progress, because a concurrent session
  was finishing ADR-0202 in the same working tree. No new corpus file: none of this is something a
  `.jr` program can observe.

## Rejected alternatives

**Keep `panic = "abort"` and make the server not rely on unwinding.** There is no way to: salsa's
cancellation *is* a panic, and the alternative — never writing while a snapshot job is outstanding —
is the opposite of what an editor does. ADR-0024 §2 chose the snapshot-per-job design precisely so a
write can cancel a read.

**Have the Zed extension prefer the newer of `target/debug` and `target/release`, as Neovim does.**
This was tempting because it would have made the symptom vanish. It would also have left `jr` broken
for anybody who installed it, and hidden §1 again — the editor asking for the *release* build is
right, and the release build was wrong.

**Skip `editors/zed/grammars/` by name** in `SKIP`. Four lines, and it describes one tool's choice of
directory rather than the property that matters. The next vendored copy would arrive somewhere else.

**Ask Zed to clone the grammar elsewhere.** Not available: the clone location is Zed's, derived from
the extension directory. What *is* in this project's hands is the two behaviours above, plus
`.gitignore` — the clone is a build input, so it is ignored now, which `target/` and `*.wasm` already
were.
