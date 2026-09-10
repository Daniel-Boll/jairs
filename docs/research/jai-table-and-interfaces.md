# Jai `Table` and structural interfaces: source audit

Date: 2026-09-10

## Scope and evidence

Jai is still distributed as a closed beta, and neither an official language specification nor
the current compiler-distributed `Hash_Table/module.jai` is available in this workspace or in an
official public repository found during this audit. There is no local `jai` executable with which
to probe edge cases. This report therefore separates evidence carefully:

1. **Direct current Jai usage:** [`focus-editor/focus` at
   `c6b3ead7`](https://github.com/focus-editor/focus/tree/c6b3ead7d4174527d0138e8a31f7c3c5663badec).
   Focus is not maintained by the Jai language team, so it is not an authoritative declaration
   source; it is nevertheless a substantial Jai program whose README requires Jai `0.2.029`
   ([README lines 24–28](https://github.com/focus-editor/focus/blob/c6b3ead7d4174527d0138e8a31f7c3c5663badec/README.md#L24-L28))
   and whose checked-in calls directly establish accepted contemporary source forms. For
   structural-interface usage, this report also cites
   [`corruptmemory/jai-wayland` at
   `49abb954`](https://github.com/corruptmemory/jai-wayland/tree/49abb954f522131e0ac8cb98e8884f2c2fc97731);
   it is likewise direct application source rather than language-owner documentation.
2. **Secondary fallback:** the local [`The_Way_to_Jai`](../../references/The_Way_to_Jai/README.md)
   submodule, pinned to
   [`19cb4b7a`](https://github.com/Ivo-Balbaert/The_Way_to_Jai/tree/19cb4b7acb0de2798c769f9ad73313a4d15f4056).
   Its README says that it grew from personal notes and other primers and only specifically dates
   testing through chapter 8 to beta `0.2.024`
   ([README lines 1–12](https://github.com/Ivo-Balbaert/The_Way_to_Jai/blob/19cb4b7acb0de2798c769f9ad73313a4d15f4056/README.md#L1-L12)).
   Chapters 23 and 34 are useful leads, not primary evidence.
3. **Not used as evidence for actual Jai:** `withlang-dev/open-jai` describes itself as a clean-room,
   incomplete Jai-style implementation
   ([README](https://github.com/withlang-dev/open-jai/blob/264ba53218bf0e55bbc328197b312fe704496224/README.md)).
   Its `Hash_Table` module is therefore useful only as a design comparison, not as a snapshot of
   Jai's shipped module.

## Findings in brief

- The current observed standard type spelling is `Table(Key_Type, Value_Type)`, with optional
  compile-time hash and equality customization named `given_hash_function` and
  `given_compare_function`.
- A zero-initialized table is usable; `table_resize` is the visible preallocation operation.
- The contemporary call surface includes `table_add`, `table_set`, `table_find`,
  `table_find_pointer`, `table_remove`, `table_resize`, `table_reset`,
  `table_reset_keeping_memory`, and `deinit`.
- Current Focus source receives `table_find` as `(found, value)`. The older secondary tutorial
  receives it as `(value, success)`, so return order is version-sensitive and the tutorial must not
  define Jairs compatibility.
- `Table(string, V)` is ordinary usage. The table stores string values, not owned deep copies of
  their bytes; callers copy persistent keys and free those owned strings separately.
- `$T/interface Some_Struct` is a structural field constraint, not a runtime vtable. The available
  secondary example establishes extra-field matching; field-order independence and `using`-field
  promotion are plausible and explicitly claimed by the supplied feature example, but no public
  first-party compiler test was found, so those details remain below high confidence.

## 1. The observed `Hash_Table` / `Table` interface

### 1.1 Type parameters

Normal declarations use exactly two required type arguments:

```jai
table: Table(string, Value);
```

Focus contains tables keyed by `string`, integers, and pointers, including
`Table(string, Loaded_Font)` and `Table(*Focus_Timer, *NSTimer)`
([config lines 613–631](https://github.com/focus-editor/focus/blob/c6b3ead7d4174527d0138e8a31f7c3c5663badec/src/config.jai#L613-L631),
[macOS lines 220–228](https://github.com/focus-editor/focus/blob/c6b3ead7d4174527d0138e8a31f7c3c5663badec/src/platform/macos.jai#L220-L228)).
Generic code can refer to the associated names `table.Key_Type` and `table.Value_Type`
([utility lines 648–653](https://github.com/focus-editor/focus/blob/c6b3ead7d4174527d0138e8a31f7c3c5663badec/src/utils/utils.jai#L648-L653)).

The `Hash_Table :: struct(K, V, N)` in The Way to Jai chapter 23 is an illustrative fixed-array
struct used to explain polymorphic restrictions, not the standard module's `Table`
([chapter 23 lines 156–173](https://github.com/Ivo-Balbaert/The_Way_to_Jai/blob/19cb4b7acb0de2798c769f9ad73313a4d15f4056/book/23A_Polymorphic_arrays_and_structs.md#L156-L173)).
It must not be used to infer a third required capacity parameter for the real table.

### 1.2 Hash and equality customization

Focus constructs a path-keyed table with exact named polymorphic arguments:

```jai
buffers_table: Table(
    string,
    s64,
    given_hash_function = x => Hash.get_hash(to_lower_copy(x,, temp)),
    given_compare_function = platform_path_equals
);
```

Source:
[`editors.jai` lines 5127–5130](https://github.com/focus-editor/focus/blob/c6b3ead7d4174527d0138e8a31f7c3c5663badec/src/editors.jai#L5127-L5130).

This establishes:

- hash and equality are customizable at the `Table` type-instantiation level;
- the custom equality may differ from ordinary `==`; and
- the hash function must agree with that equality. Focus lowercases before hashing while using a
  platform path comparator, so equal paths are intended to receive equal hashes.

The exact default hash algorithm, callback signatures, and complete list/order of optional
polymorphic arguments remain unverified without the distributed module source.

### 1.3 Initialization and allocation

A zero value is a valid starting state. Focus declares a local table and calls `table_add`
immediately, without an explicit initializer
([files lines 202–220](https://github.com/focus-editor/focus/blob/c6b3ead7d4174527d0138e8a31f7c3c5663badec/src/files.jai#L202-L220)).
Other call sites pre-size the same zero value with `table_resize` before adding entries
([Jai lexer lines 782–801](https://github.com/focus-editor/focus/blob/c6b3ead7d4174527d0138e8a31f7c3c5663badec/src/langs/jai.jai#L782-L801)).
This is strong evidence for lazy backing allocation and an optional explicit capacity reservation,
but not for the internal bucket layout or growth factor.

The secondary chapter-34 example heap-allocates the table header with
`New(Table(string, string))`
([example lines 1–12](https://github.com/Ivo-Balbaert/The_Way_to_Jai/blob/19cb4b7acb0de2798c769f9ad73313a4d15f4056/examples/34/34.2_hash_table.jai#L1-L12)).
That is consistent with ordinary Jai `New(T)` usage, but direct current source more commonly keeps
the `Table` value inline and passes `*table` to operations.

Final cleanup uses `deinit(*table)`. Clearing APIs also exist:

- `table_reset(*table)` is used when cursor-history state is reset
  ([history lines 1–6](https://github.com/focus-editor/focus/blob/c6b3ead7d4174527d0138e8a31f7c3c5663badec/src/history.jai#L1-L6));
- `table_reset_keeping_memory(*table)` explicitly preserves backing storage
  ([files lines 526–530](https://github.com/focus-editor/focus/blob/c6b3ead7d4174527d0138e8a31f7c3c5663badec/src/files.jai#L526-L530)); and
- `deinit(*table)` is used at final teardown
  ([editors lines 2269–2274](https://github.com/focus-editor/focus/blob/c6b3ead7d4174527d0138e8a31f7c3c5663badec/src/editors.jai#L2269-L2274)).

The exact difference between current `table_reset` and `deinit`, and whether a table captures the
allocator used for its first allocation, cannot be proved from public call sites alone.

### 1.4 Add, set, find, remove, and pointer lookup

The contemporary observed surface is:

```jai
table_add(*table, key, value);
slot := table_set(*table, key, value);       // observed return type: *Value
found, value := table_find(*table, key);
value_pointer := table_find_pointer(*table, key);
removed := table_remove(*table, key);
```

Evidence:

- Focus removes an existing buffer path, checks the returned boolean, and then uses `table_add`
  ([buffer lines 761–779](https://github.com/focus-editor/focus/blob/c6b3ead7d4174527d0138e8a31f7c3c5663badec/src/buffer.jai#L761-L779)).
- Cache-like code uses `table_set` where an existing value may be replaced
  ([config lines 615–624](https://github.com/focus-editor/focus/blob/c6b3ead7d4174527d0138e8a31f7c3c5663badec/src/config.jai#L615-L624)).
- A wrapper returning `*bool` directly returns `table_set(...)`, establishing that `table_set`
  returns a pointer to the stored value in this compiler generation
  ([files lines 519–524](https://github.com/focus-editor/focus/blob/c6b3ead7d4174527d0138e8a31f7c3c5663badec/src/files.jai#L519-L524)).
- `table_find_pointer` is used as a nullable presence/mutation lookup
  ([workspace lines 380–386](https://github.com/focus-editor/focus/blob/c6b3ead7d4174527d0138e8a31f7c3c5663badec/src/workspace.jai#L380-L386)).

The usage pattern strongly suggests `table_add` is the known-new-key operation and `table_set` is
upsert, but duplicate-key behavior for `table_add` is not proved without the declaration.

#### Return-order drift

Current Focus consistently writes:

```jai
found, value := table_find(*table, key);
```

Examples:
[config lines 615–624](https://github.com/focus-editor/focus/blob/c6b3ead7d4174527d0138e8a31f7c3c5663badec/src/config.jai#L615-L624) and
[macOS lines 266–277](https://github.com/focus-editor/focus/blob/c6b3ead7d4174527d0138e8a31f7c3c5663badec/src/platform/macos.jai#L266-L277).

The pinned secondary tutorial instead writes:

```jai
value, success := table_find(table, key);
```

Source:
[chapter-34 example lines 55–72](https://github.com/Ivo-Balbaert/The_Way_to_Jai/blob/19cb4b7acb0de2798c769f9ad73313a4d15f4056/book/34A_Other_useful_modules.md#L55-L72).

For compatibility planning, the current direct call sites outrank that older example:
`table_find` should be treated as `(found, value)` unless an actual targeted Jai compiler probe says
otherwise.

### 1.5 Iteration

Current Focus iterates a table as:

```jai
for key, value : table {
    // ...
}
```

It uses that form to free owned key strings before deinitializing the table
([editors lines 2269–2274](https://github.com/focus-editor/focus/blob/c6b3ead7d4174527d0138e8a31f7c3c5663badec/src/editors.jai#L2269-L2274)).

The older tutorial uses the implicit loop names, with `it_index` as key and `it` as value
([example lines 23–39](https://github.com/Ivo-Balbaert/The_Way_to_Jai/blob/19cb4b7acb0de2798c769f9ad73313a4d15f4056/examples/34/34.2_hash_table.jai#L23-L39)).

No inspected source promises stable iteration order. The tutorial's sample output is an observation,
not a contract. Jairs should therefore specify order as undefined and should treat insertion,
removal, resize, or rehash as iterator-invalidating until stronger evidence says otherwise.

### 1.6 String keys and ownership

`Table(string, V)` is pervasive in Focus, including token maps, file paths, fonts, colours, and
named captures. Default string hashing/equality therefore works without custom callbacks.

The table does not deep-own string bytes. Persistent path keys are copied before insertion
([buffer lines 761–779](https://github.com/focus-editor/focus/blob/c6b3ead7d4174527d0138e8a31f7c3c5663badec/src/buffer.jai#L761-L779)),
and final teardown explicitly frees every key before `deinit`
([editors lines 2269–2274](https://github.com/focus-editor/focus/blob/c6b3ead7d4174527d0138e8a31f7c3c5663badec/src/editors.jai#L2269-L2274)).
The table owns its bucket storage; ownership of memory referenced by keys and values remains with
the caller.

## 2. `$T/interface Some_Struct`

### 2.1 Exact observed syntax

The constraint spelling is:

```jai
Interface_Shape :: struct {
    required_name: Required_Type;
}

consume :: (value: $T/interface Interface_Shape) {
    // value has concrete type T
}
```

The pinned secondary example uses exactly
`discuss :: (x: $T/interface Matchable)`
([chapter 23 lines 176–204](https://github.com/Ivo-Balbaert/The_Way_to_Jai/blob/19cb4b7acb0de2798c769f9ad73313a4d15f4056/book/23A_Polymorphic_arrays_and_structs.md#L176-L204)).
Direct Jai application source also uses the pointer form
`target: *$T/interface Versionable`
([registry lines 128–136](https://github.com/corruptmemory/jai-wayland/blob/49abb954f522131e0ac8cb98e8884f2c2fc97731/modules/wayland/registry.jai#L128-L136)).
Its interface shape is a struct containing `id: u32`, explicitly documented there as accepting any
struct with that field
([types lines 52–56](https://github.com/corruptmemory/jai-wayland/blob/49abb954f522131e0ac8cb98e8884f2c2fc97731/modules/wayland/types.jai#L52-L56)).
The constraint therefore attaches to the polymorphic pointee type rather than introducing a boxed
interface.

This is distinct from `$T/Base`, which is described as a nominal/component-style restriction and
may match through `using`
([chapter 23 lines 149–154](https://github.com/Ivo-Balbaert/The_Way_to_Jai/blob/19cb4b7acb0de2798c769f9ad73313a4d15f4056/book/23A_Polymorphic_arrays_and_structs.md#L149-L154)).

### 2.2 Field-matching rules

The best available evidence supports these rules:

- every named field in the interface struct must exist on the concrete `T`;
- the field's resolved type must match;
- the concrete struct may contain additional fields; and
- the procedure remains specialized for concrete `T`; this is compile-time structural checking,
  not a runtime interface object or vtable.

The `Matchable`/`Thing` example directly demonstrates extra fields: `Thing` contains
`duration`, `tag`, and `type` in addition to the required `name` and `color`, and is accepted by
`$T/interface Matchable`
([chapter 23 lines 185–220](https://github.com/Ivo-Balbaert/The_Way_to_Jai/blob/19cb4b7acb0de2798c769f9ad73313a4d15f4056/book/23A_Polymorphic_arrays_and_structs.md#L185-L220)).
The prose immediately after the example contains a likely typo (“name and a type field”); the code
requires `name` and `color`, so the code is the stronger evidence.

The supplied feature example additionally claims:

- declaration order does not matter; and
- a promoted `using position: Vector3` can provide the required fields.

Those claims fit Jai's specialization and field-promotion model, but this audit found no public
first-party compiler test or official module source proving them. They should be treated as
**medium-confidence behavior to probe**, not as settled specification. Likewise unverified are
duplicate promoted names, visibility rules, implicit numeric conversions, procedure-valued fields,
and nested/parameterized interface shapes.

## 3. Implications for Jairs

### `Hash_Table` / `Table`

1. Ship the public type as `Table(K, V)`, not as another concrete `Map(s64, s64)` and not with a
   required capacity type parameter.
2. Preserve a zero-value usable state. Offer explicit reserve/resize and final `deinit`; distinguish
   clearing while retaining memory from releasing storage.
3. Include both value lookup and pointer lookup. Pointer lookup is important for in-place mutation
   and avoids a second search.
4. Make string keys work in the first release, with byte-content hash/equality and an explicit
   borrowed-key ownership rule. A separate owned-key convenience may copy strings, but the core
   table should not silently deep-copy arbitrary `K`.
5. Support custom hash and equality as a paired contract. Jai's type-level callback parameters are
   attractive, but Jairs still refuses imported value-polymorphic (`$N`) specialization. The
   practical first implementation should therefore store runtime procedure pointers in the table
   or take them in `init`; later language work can add Jai's exact type-level spelling.
6. Use the contemporary `(found, value)` ordering if source compatibility is the goal. If Jairs
   instead keeps its existing `(value, found)` convention, document that as an intentional
   divergence rather than citing the older tutorial as Jai's current API.
7. Do not block the module on `for table`. Until user-defined for-expansion exists, expose an
   explicit iterator/cursor or indexed live-entry walk and keep iteration order unspecified.
8. Capture the allocator used for bucket storage, as the existing Jairs plan proposes. Public
   sources do not settle Jai's allocator-capture details, and freeing through whichever allocator
   happens to be active later would be unsafe in Jairs.

### Structural interfaces

1. Plan `$T/interface Shape` as a compile-time constraint over existing `$T` specialization, not as
   trait objects, witness tables, or a new runtime representation.
2. Match required fields by resolved name and type, allow extras, and resolve each access against
   the concrete struct's real offset. Order-independent matching cannot reinterpret the value as
   the interface struct because the concrete offsets may differ.
3. Decide `using` promotion explicitly and test ambiguity. The useful test matrix is: exact shape,
   reordered fields, extra fields, missing field, wrong field type, promoted fields, ambiguous
   promoted fields, value parameter, and pointer parameter.
4. Keep this feature independent from `Table`. Structural data interfaces do not by themselves
   express a free-standing hash/equality protocol, and making the container wait for them would
   couple two separately useful features.

## Confidence and remaining gaps

| Claim | Confidence | Reason |
|---|---|---|
| `Table(K, V)`; string/integer/pointer keys | High | Repeated current Focus declarations |
| `given_hash_function`, `given_compare_function` | High | Exact current source spelling |
| `table_find` currently returns `(found, value)` | High | Repeated current call sites |
| `table_set` returns `*V`; `table_remove` returns `bool` | High | Typed wrapper and assigned result |
| zero-value use, `table_resize`, pointer lookup, reset/deinit names | High | Direct current call sites |
| caller owns string bytes referenced by keys | High | Explicit copy-on-insert and free-before-deinit |
| `table_add` means new-only; `table_set` means upsert | Medium | Strong usage pattern, declaration unavailable |
| iteration order is unspecified | Medium | No promise found; normal hash-table behavior |
| interface allows extra fields | Medium-high | Compiler-how-to-derived secondary example |
| interface field order is irrelevant | Medium | Supplied example and structural model; no first-party public test |
| `using`-promoted fields satisfy `/interface` | Medium-low | Supplied example; no independent primary proof |
| exact bucket layout, growth/load policy, default hash, allocator capture | Unknown | Distributed module source unavailable |

The next authoritative step, if access to a Jai beta installation becomes available, is to preserve
its exact `modules/Hash_Table/module.jai` revision and run focused probes for duplicate `table_add`,
allocator changes, reset behavior, iterator invalidation, reordered interface fields, wrong field
types, and `using` promotion.
