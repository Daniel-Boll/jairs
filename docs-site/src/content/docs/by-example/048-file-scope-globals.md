---
title: File-scope variables
description: A real, mutable, process-wide global — the construct modules/Simp needs so a renderer can hold its own state instead of threading a handle through every call.
sidebar:
  order: 48
---

Until ADR-0186, `counter: s64 = 5;` at file scope was legal to *write* and impossible to *use*: any body that touched it was refused with E0245, "a file-level item has no value until jr-vm." A file-scope variable is now a real global, with the consequences a real global always has — shared storage, a defined initial value, and no thread isolation.

## A scalar, written in one procedure, read in another

```jr
// A scalar with a literal initialiser — the simplest form.
counter: s64 = 5;

// A *different* procedure from the one that reads it below. That separation is the test: a global
// exists so that two procedures share storage.
bump :: () {
    counter = counter + 1;
}

read_counter :: () -> s64 {
    return counter;
}
```

`bump` and `read_counter` share one `counter` — a write in one is visible to the other with no parameter or return value carrying it across, which is the entire point of the construct and the property a per-frame or per-body simulation of it would get wrong while still looking correct.

## Initialised by a compile-time call, and left uninitialised

```jr
// An initialiser that is a **call**, evaluated at compile time. There is no moment before `main` at
// which arbitrary code could run, so the value has to exist by then; this proves it does.
computed: s64 = triple(7);

// Explicitly uninitialised. ADR-0186 §2 promises **zero**, not undefined bytes, so that the native
// path and the VM cannot differ about a program that never assigns it first.
untouched: s64 = ---;
```

A global's initialiser can be a call, evaluated at compile time — there is no moment before `main` runs at which arbitrary code could execute, so the value has to already exist by then. `---` reads as zero, a promise rather than an accident of the loader: three engines back a global (a bump-allocated region in the VM, a Cranelift data object, an LLVM internal mutable global) and all three must agree on what an untouched one reads as.

## Why the feature exists: Simp's own state

```jr
/// The one GL context, created on the first `set_render_target`.
///
/// Named as Jai names it (`the_gl_context`, `gl.jai:85-92`), and one per process for the same reason:
/// a GL program, a buffer and a texture binding all belong to a context, so a second context would
/// need a second copy of everything below.
the_gl_context: *u8 = ---;

/// The window the context is current on.
the_window: *u8 = ---;

/// Which corner the origin is in — `RIGHT_HANDED` or `LEFT_HANDED`.
coordinate_system: s64 = RIGHT_HANDED;

/// How many vertices the open batch holds.
vertex_count: s64 = 0;
```

The public Jai `Simp` snapshots inspected for ADR-0208 share a no-state call shape — for example `set_render_target(window, coords)` and `immediate_quad(x0, y0, x1, y1, color)` — because their immediate state lives in `#add_context simp: *Immediate_State` and their GL backend keeps process-wide context state. The snapshots disagree on several exact declarations, so Jairs does not claim signature identity. Before file-scope globals, though, it could not follow even that shared shape: the old module threaded a `*Renderer` through every call because a file-scope `var` was refused. `modules/Simp` now keeps its GL context, current program and open batch as file-scope globals, giving it a recognisably Simp-shaped no-state surface.

## What a real global costs

**A comptime `#run` may not read a global's initialised value.** A `#run` at file scope executes during signature resolution, before a global's own initialiser has run in the same engine — so a `#run` that calls a procedure reading a global sees its *pre-initialisation* zero, not the value the declaration names. This is <span class="jairs-status absent">absent</span> rather than refused: it compiles cleanly, in both engines, and silently returns the wrong answer, which is a sharper trap than an error would be.

**A module's global is not thread-local.** Jai's own `print` keeps its buffer in the context's temporary storage, which is per-thread; `modules/Simp`'s state and `modules/Basic`'s output buffer are both plain file-scope globals, so two threads calling into either share one copy. The fix Jai's own model implies is `#add_context`, and it does not exist: writing `#add_context foo: s64;` is <span class="jairs-status refused">refused</span> as an unexpected token, E0101 — the directive was never added, so there is no way for a global to travel per-thread through the context the way an allocator does. Until it exists, a threaded program shares this module's globals across every thread that touches them, and a drawing or printing program stays single-threaded on purpose.

See also [Book I — The Jairs Language](/language/introduction/).
