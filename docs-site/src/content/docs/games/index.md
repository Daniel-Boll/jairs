---
title: Games with Jairs
description: The Game foundation and the lower-level Window, Input, Simp, GL, Image and UI stack.
sidebar:
  order: 0
  label: Overview
---

You can play Pong in Jairs today. First, run the rules without opening a window:

```sh
cargo run -q -p jr-cli -- run examples/games/pong/sim.jr \
    -I modules -I examples/games/pong/modules
```

```text
steps=15153 walls=95 paddle hits=219 points=9
final score 4-5
```

Then build the graphical version:

```sh
cargo run -q -p jr-cli -- build examples/games/pong/main.jr -o /tmp/pong \
    -I modules -I examples/games/pong/modules -L /opt/homebrew/lib
/tmp/pong
```

Use `W` and `S`; the right paddle plays itself. On a machine where SDL2 lives somewhere else,
replace `/opt/homebrew/lib` with that library directory.

That pair of commands is the theme of this book: keep the game rules small, deterministic and
headless, then put the window around them.

## The stack in one minute

Jairs offers a **Simp-shaped subset**, not an exact copy of Jai's graphics API:

|Module|Job|
|---|---|
|`Game`|Own one app's window lifecycle, close handling, event drain, frame timing and presentation.|
|`Window`|Create and close a window.|
|`Input`|Poll keyboard, mouse and window events.|
|`GL`|Expose the OpenGL calls used by the renderer.|
|`Simp`|Batch and draw coloured or textured 2D geometry.|
|`Image`|Load BMP pixels and upload them as textures.|
|`UI`|Build immediate-mode widgets over `Input` and `Simp`.|

`Game` is a foundation, not yet the teaching surface for this book. It does not yet own held input,
primitive helpers, textures, PNG, text or audio, so the complete examples below continue to show the
lower-level modules they actually need. ADR-0210 reserves the beginner-facing cutover for the point
where PNG and text exist beneath the facade.

The implementation detail that explains the build setup is this: **Jairs uses SDL2 for windows,
events and OpenGL-context plumbing, then OpenGL for rendering. Jai's Simp uses native OpenGL
backends instead of SDL2.** The APIs have the same family resemblance, but Jairs currently covers
a smaller 2D surface and has its own pointer-based window integration.

For exhaustive declarations, read the corresponding files under `modules/`; these chapters show
the handful of calls that make a game.

## Why graphical programs are built, not interpreted

`jr run` executes inside the compiler's VM. It can run the pure Pong and Snake simulations, but it
cannot load the SDL2 and OpenGL libraries required by a graphical driver. Use:

- `jr run` for headless rules and simulations;
- `jr build` for programs importing `Game`, `Window`, `Input`, `Simp`, `Image` or `UI`.

That restriction becomes an advantage once the rules are separated from the driver: a collision
bug can fail a deterministic run instead of waiting for somebody to notice a bad frame.

## What you will build

The chapters move in one direction:

1. Put rules and rendering in different files.
2. Open a window and fold events into useful input state.
3. Clear the screen and draw a batch with `Simp`.
4. Advance continuous and fixed-tick games safely.
5. Assemble those pieces into Pong.
6. Add Snake, textures, widgets and sprites.

Three complete examples live under `examples/games/`:

- `pong/`: a continuous simulation with a headless opponent;
- `snake/`: a fixed-tick, seeded simulation;
- `sprites/`: textured drawing and immediate-mode UI.

## Before you start

Install SDL2 and make its library directory visible at link time:

```sh
jr build my_game.jr -o my_game -I modules -L /path/to/sdl2/lib
```

`JR_LIBRARY_PATH` can provide the same path. The source names the library with
`#system_library "SDL2"`; the machine-specific location remains a build setting.

The examples also check `GL.error_code()` after presenting a frame. A clean run means **no GL
error was observed by that check**. It does not prove shader output or pixels were visually
correct, so rendering still deserves a real look.

Current limitations—text, richer image formats, held-key helpers and other missing conveniences—
are kept in [What a game cannot do yet](/games/not-implemented/). They should inform the size of
the game you choose, not stop you from drawing the first frame.

Next: [Laying out a game](/games/project-layout/) — the small architectural choice that makes the
examples testable.
