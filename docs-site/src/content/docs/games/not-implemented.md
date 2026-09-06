---
title: What a game cannot do yet
description: An honest, maintained inventory of what Jai gives a game developer that Jairs does not — organised by what you go looking for, not by compiler subsystem.
sidebar:
  order: 11
---

The examples in this book can open a window, run deterministic rules, draw coloured and textured
quads, and build a small immediate-mode UI. This page marks the edge of that working slice.

`works` means the feature is usable now. `partial` means the foundation exists but a common
game-facing layer is missing. `absent` means there is no reusable implementation yet.

## Game-development capabilities

| Area | Status | What works now | What is still missing |
| --- | --- | --- | --- |
| Window and rendering | partial | `Game.App` owns startup, close handling, one event drain, delta time, presentation and teardown; OpenGL renders through `Simp` | The facade does not yet own held input, drawing helpers or resources, and Jairs exposes a Simp-shaped subset rather than Jai's complete Simp surface |
| Shapes | partial | Coloured triangles and quads, textured quads, blending, and y-up or y-down projection | Lines, circles, scissoring, render textures, and broader 2D/3D drawing helpers |
| Text and fonts | absent | Scores can be represented with simple shapes | Font loading, glyph preparation, text measurement, and string drawing |
| Images | partial | BMP load/save, in-memory surfaces, fills, GL upload, sprite-sheet UV selection | PNG and other common formats, public pixel editing, and pre-upload cropping |
| Audio | absent | — | Sound loading, playback, mixing, and volume control |
| Keyboard and mouse | partial | Event polling, key edges, mouse motion/buttons, quit events | A complete key table, text input, named resize handling, and gamepads |
| Held input state | partial | A game can fold key-down/key-up events into caller-owned state | A standard `is_key_down`/pressed/released helper |
| UI | partial | One immediate-mode button with hover, active, and click behavior | Labels, layout, focus, sliders, text fields, scrolling, and themes |
| Math | partial | Integer helpers, `float64` vectors, matrices, quaternions, and core trig | Common 2D helpers such as float clamp, `atan2`, 2D rotation, reflection, easing, and approximate equality |
| Geometry and collision | partial | `Window.Rect` mirrors SDL's integer rectangle | Gameplay shapes, overlap/containment tests, ray casts, and collision response |
| Storage and lookup | partial | Dynamic arrays, `Map(s64, s64)`, and stable-address `Bucket_Array` | Reusable generic maps and resource registries across module boundaries |
| Build and assets | partial | Build scripts can read files and invoke external tools | A standard asset cooker, atlas pipeline, and hot reload |

Caller-owned held-key state is a design choice, not a language workaround. Jairs supports
file-scope mutable state; `Input` simply does not currently choose to own a global keyboard
snapshot.

## Language edges visible in game code

| Need | Current state | Practical workaround |
| --- | --- | --- |
| Computed array lengths | `[WIDTH * HEIGHT]T` is E0233 | Use a literal constant and a folded consistency check |
| Procedure overloading | Ordinary procedures cannot share a name by parameter type | Use explicit names such as `dot2`, `dot3`, and `dot4` |
| Cross-file generic procedures | Imported polymorphic procedures cannot be instantiated generally | Export concrete wrappers for the types a module supports |
| Pointer iteration | Jai's `for *item` form is not available | Iterate by index and take the element address |
| `Code` values and for-expansion macros | Not available | Write the loop body explicitly |
| Item-level `#if` | Conditional declarations are not supported | Generate a declaration with `#insert #run` when needed |
| Thread-local module context | No `#add_context` equivalent | Keep rendering on one thread or pass state explicitly |

None of these blocks the single-window examples in this book. They become important when building
reusable engine modules rather than one game.

## The rendering-stack distinction

Jai's `Simp` renders with OpenGL and uses platform-specific window/context plumbing. Jairs keeps
the same general immediate-rendering shape but reaches the platform through SDL2:

```text
Jairs game
  → Window / Input / Image (SDL2 platform services)
  → Simp / GL (OpenGL drawing)
```

That is why “Jairs uses SDL” and “Jairs uses OpenGL” are both true, but describe different layers.

For language-wide status, use [What's absent](/language/whats-absent/). This page stays deliberately
short: when a capability ships, its row should change in the same wave.

Next: [Book I — The Jairs Language](/language/introduction/) — the language underneath these
examples.
