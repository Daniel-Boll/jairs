# ADR-0103: `String` is a module of non-allocating byte operations — W7 opens

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W7 sub-wave 1.** W6 is open but its remaining work (a compiler-emitted static-data table) is a wave-sized
  architectural decision. `String` is the standing goal's first piece and has a caller already waiting, so it
  goes first. This is the project's **second module**, and therefore the first program to import two.

## Context

**A refusal in the previous wave named this module as its own fix.** ADR-0099 §4 refused `==` on two strings:
a `string` is `{data: *u8, count: s64}` (ADR-0004), so "the same storage" and "the same contents" are both
plausible, and picking one silently would make the other a bug that looks like working code. Its stated reason
was that comparing contents needs a byte loop, *which is `String`'s job in W7*.

That is a better reason to write a module than "a string library usually has one", and it is why `String` comes
before `Sort`, `Math`, or a dynamic array: it is the only W7 module with a consumer already asking.

An `==` that looped would also be the only implicitly-looping operator in the language — a much larger thing to
introduce than a procedure called `equal`.

## Decision

### 1. Its own module, not more of `Basic`

`modules/String/module.jr`, imported separately. `Basic` is the module *every* program imports, so anything put
there is a tax on every program — and more importantly, adding to `Basic` would mean **nothing ever tested that
two modules can be imported at once**. Every module test to date imports `Basic` alone, so a second module is
the first real exercise of ADR-0014's flat merge with more than one in play. `valid/084` imports both.

The cost is that `String` cannot use `Basic`'s `#scope_module` helpers, which is the correct consequence of a
module boundary rather than a problem with this one.

### 2. Nine procedures, chosen by what asks for them

`equal`, `compare`, `starts_with`, `ends_with`, `find`, `contains`, `byte_at`, `is_empty`. Each is here because
something asks for it, not because a string library usually has it:

- **`equal`** — the reason the module exists (E0278's help says "compare `.count`, or compare fields one at a
  time"; this is the real answer it was pointing at).
- **`compare`** → `s64` — the same loop as `equal`, and what sorting will need. **The sign is specified, not the
  magnitude**: a caller must not read a byte difference out of it, because that would pin an implementation
  detail no other comparison routine promises.
- **`starts_with` / `ends_with`** — the two common predicates `equal` cannot express. An empty pattern is
  `true`, so `starts_with(s, "")` composes instead of being a case a caller guards.
- **`find`** → index or `-1`, and **`contains`** as its predicate form. `-1` rather than a second return value
  (which ADR-0008 would allow) because a caller almost always feeds the result straight into a comparison:
  `if find(h, n) >= 0` reads better than naming an index it discards. The error model earns its keep where the
  *value* is meaningless on failure; here the sentinel is outside the domain of valid indices.
- **`byte_at`** — because **`s.data[i]` does not compile**: `data` is a `*u8` and a pointer is not indexable
  (E0234), so reading a byte takes `(s.data + i).*` plus a cast. This is that expression with a name, and it is
  honestly a workaround — when pointer indexing arrives it becomes a one-line wrapper rather than the only way.
  Out of range answers **`-1` rather than trapping**, unlike an array index (ADR-0003): an array's bound is
  known to the compiler and indexing past it is a *mistake*, while scanning until the bytes run out is an
  ordinary way to write a loop.
- **`is_empty`** — `count == 0` with a name that says what it means rather than how it is measured.

### 3. Nothing allocates, and that is a decision rather than an omission

`concat`, `substring`, `to_upper`, `split` are all absent. Each needs somewhere to put a result, which means an
allocator argument and a decision about who frees it. The **mechanism is not missing** — `context.allocator`
(ADR-0057) and temporary storage (ADR-0065) both exist. What is missing is a *choice* between "always the
context allocator", "an explicit parameter" and "always temporary", and that choice has real consequences for
every caller.

Settling it in passing, by whichever one this file happened to use, is exactly how a library acquires an
accidental convention. A non-allocating module is a complete and useful thing on its own, and shipping it first
means the allocation decision is made with a working baseline to compare against.

## Consequences

- **E0278 now has a real answer.** Its help said "compare `.count`, or compare fields one at a time", which was
  honest and unhelpful; `equal(a, b)` is what it meant.
- **Two modules import cleanly**, which nothing had tested. `valid/084` is the proof.
- **Teeth-checked twice, precisely.** Making `equal` compare only lengths clears bit 1 (255 → 254); deleting
  `compare`'s trailing length check — the prefix case — clears bit 2 (255 → 253). Each group in `valid/084`
  contributes **one bit** and folds its negative cases in with `&&`, so a wrong answer clears a bit rather than
  pushing the total past 255 where an exit code wraps and could coincide with a passing value. That mattered:
  the first draft summed to 721 and exited 209.
- **Written in the language, so it is a test of the language.** `find`'s inner loop sets its bound instead of
  breaking, because `while` has no unlabelled `break` out of a nested loop (ADR-0049) — the kind of thing only
  writing a real library surfaces.
- **What W7 still owes**: the allocating half of `String`; `Sort` (which `compare` was shaped for); a dynamic
  array and a hash table, both of which want the polymorphic structs W5 delivered; `Math`, `Random`, `File`,
  `Process`, `Thread`, `Time`, `Socket`, `JSON`.
