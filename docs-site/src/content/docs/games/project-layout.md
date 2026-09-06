---
title: Laying out a game
description: Module resolution, qualified imports, jairs.toml, and why a game's rules and its renderer live in separate files.
sidebar:
  order: 1
---

Start by running Pong without Pong's window:

```sh
cargo run -q -p jr-cli -- run examples/games/pong/sim.jr \
    -I modules -I examples/games/pong/modules
```

That command plays a complete deterministic match. It works because the example has one clean
seam: the rules know nothing about SDL2, input devices or pixels.

## Put the rulebook behind a module boundary

The example is deliberately small:

```text
examples/games/pong/
├── main.jr
├── sim.jr
└── modules/
    └── Pong/
        └── module.jr
```

- `modules/Pong/module.jr` owns the ball, paddles, scores and collisions.
- `sim.jr` drives those rules with fixed input and a fixed time step.
- `main.jr` translates keyboard events into movement, measures real time and draws.

The useful test is simple: if a rule needs `Window`, `Input` or `Simp`, it is probably in the
wrong file. The simulation should accept plain values and return plain results:

```jr
event := Pong.update(*state, dt, left, right);
```

Neither caller has to be special. The windowed driver supplies human input; the headless runner
supplies `Pong.follow`. Both exercise the same `update`.

This split also prevents pixels from leaking into the rules. Pong's field is `200 × 150` world
units. Only `main.jr` decides that one world unit becomes four pixels.

## Keep imports qualified

Jairs imports are flat by default, which becomes awkward as soon as a game combines several
modules. Alias graphics imports so every call says where it came from:

```jr
Pong :: #import "Pong";
Window :: #import "Window";
Input :: #import "Input";
Simp :: #import "Simp";
Time :: #import "Time";
#import "Basic";
```

Now `Window.start`, `Time.monotonic` and `Pong.update` remain readable even when modules export
similar names. The full import rules are covered in [Modules](/language/modules/).

## Tell Jairs where project modules live

An import such as `#import "Pong"` searches each module path for:

```text
Pong/module.jr
Pong.jr
```

The in-repository examples pass their paths explicitly:

```sh
jr build examples/games/pong/main.jr \
    -I modules \
    -I examples/games/pong/modules
```

For a standalone project, keep them in `jairs.toml` instead:

```toml
[build]
module_paths = ["modules", "vendor"]
```

`jr build`, `jr run`, `jr check` and the language server then resolve the same project.

## Own state where it is easiest to reason about

The examples keep `Pong.State`, the frame's `Input.Events` and held keys in `main`. That is a
design choice, not a language restriction: Jairs supports file-scope mutable state, and `Simp`
uses it internally. Caller-owned state is preferable here because it is explicit, easy to reset,
and lets a test create more than one simulation.

The same rule scales beyond Pong: keep durable game state in a struct, keep platform details in a
thin driver, and make the boundary a small set of values such as input directions, elapsed time and
renderable positions.

Read the complete files under `examples/games/pong/`; the next chapter builds the driver side of
that seam.

Next: [A window and an event loop](/games/window-and-events/).
