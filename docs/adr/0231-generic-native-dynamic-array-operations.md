# ADR-0231: Native dynamic-array operations are generic and caller-owned

- **Status:** Accepted
- **Date:** 2026-09-10
- **Decider:** dboll

## Context

ADR-0136 made `[..]T` a native three-word dynamic array and ADR-0140 moved `List` onto that
representation, but its operations remained concrete `s64`. The reason was no longer the type:
ADR-0230 now specialises imported pure `$T` procedures in their declaration file.

The requested program needs both of these to be ordinary library use:

```text
Node :: struct {
  name: string;
  children: [..]*Node;
  properties: Table(string, string);
}

node := New(Node);
stack: [..]*Node;
push(*stack, node);
```

Three design forks were settled before implementation: keep the existing module name, retain
fallible reads rather than trapping on an empty stack, and leave allocation ownership visible in
the native array instead of introducing an owning wrapper before `Hash_Table` establishes the
shared container policy.

Writing the first generic `pop` exposed two compiler gaps. A template returning `(T, bool)` was
diagnosed as returning two values from `<unknown>` before it could specialise. Once that was
withheld correctly, a default-initialised pointer local lowered to `undef` even though only
`= ---` requests uninitialised storage.

## Decision

### 1. `List` remains the one module and its operations become pure `$T` templates

`push`, `pop`, `get`, `set`, `clear`, `free_data`, `is_empty`, `elements` and the private `grow`
operate on `*[..]$T`. `last` is added as the non-removing stack-top operation.

No concrete compatibility wrappers are needed. A call with `[..]s64` specialises the same source
procedure to `s64`, preserving every existing call while avoiding two implementations of growth.

### 2. Empty reads return a zero placeholder and `false`

`pop`, `last` and `get` return `(T, bool)`. When no element exists, the `T` is the ordinary
zero-initialised value and must be ignored because `false` is the only absence signal. Null pointers,
zero integers and zero-valued aggregates all remain legal stored elements.

This keeps failure explicit and does not turn an ordinary empty stack into a trap. `pop` retains
capacity, `clear` retains storage, and `free_data` releases and resets it.

### 3. The native array remains caller-owned

The public value is still exactly `{ data: *T, count: s64, capacity: s64 }`. The module allocates on
growth and `free_data` releases that backing storage; it does not add an opaque wrapper, destructor
or allocator metadata.

This keeps `[..]T` directly usable by language operations and views. A future owning container may
capture an allocator, but that must be a different type with a visible ownership contract.

### 4. An unbound generic results aggregate defers arity checking to its clone

In the template body, a return type containing `T` intentionally resolves to `ERROR` because no
concrete type exists yet. `return a, b` still checks both expressions but withholds E0251 only when
the current body is that unbound template. Its concrete clone is checked again with real bindings.

Scalar procedures and concrete clones retain the existing E0251 arity rule.

### 5. A default-initialised pointer is the typed null constant

MIR lowering represents pointer zero exactly as a `null` literal does: an integer value of zero at
the pointer type. A local written `p: *T;` is therefore defined and null; only
`p: *T = ---;` is uninitialised.

This is a general language repair, not a `List` special case. The generic empty-result path merely
made the old contradiction observable.

## Rejected alternatives

- **A new generic namespace beside concrete `List`.** It duplicates the public vocabulary and
  growth implementation even though specialisation preserves all concrete callers.
- **`pop -> T` with non-empty input required.** Empty is an ordinary state and there is no universal
  sentinel for `T`.
- **An owning `List(T)` wrapper.** It would duplicate the native layout or hide it behind another
  allocation before the allocator and cleanup policy shared by containers has been decided.
- **Return uninitialised storage with `false`.** Callers can accidentally inspect it, native and VM
  behaviour may differ, and the language already promises default declarations are zeroed.
- **Suppress every E0251 when the return type is poisoned.** That would hide real arity errors caused
  by unrelated type failures; the exception is restricted to an unbound polymorphic template.

## Consequences

- `[..]*Node` is a working caller-owned stack with append, top inspection, pop, indexed access,
  mutation, clear, views and explicit cleanup.
- Existing `[..]s64` programs use the generic implementations without source changes.
- Generic growth copies `size_of(T)` bytes per element and preserves pointer and aggregate elements.
- The next container wave can focus on `Table(K,V)` policy and ownership rather than generic
  specialisation or dynamic-array substrate.
- MIR changed, so this wave requires gate 7 in addition to the six ordinary gates.
