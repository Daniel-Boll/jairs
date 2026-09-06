# Games

Three complete programs, each built and run before being written up. The documentation for them is
[Book IV — Games with Jairs](../../docs-site/src/content/docs/games/), and this file is the
operational half: what is here, how to build it, and what a machine needs.

| Directory | What it shows |
|---|---|
| [`pong/`](pong) | A game split into a **simulation module and a driver**. `modules/Pong` imports only `modules/Math`, so `sim.jr` plays a whole match inside the compile-time VM; `main.jr` is the only file that needs SDL2. |
| [`snake/`](snake) | A grid game: a ring-buffer body in a fixed array, buffered turns, seeded apples, and a **fixed-tick accumulator** loop whose speed does not depend on the frame rate. |
| [`sprites/`](sprites) | Textures and widgets: a sprite sheet built in memory with `modules/Image`, animated by UV sub-rectangle, over two `modules/UI` buttons. |

## Building them

One command builds all five artefacts, through a build script written in Jairs:

```sh
cargo run -q -p jr-cli -- build examples/games/build.jr -I modules
```

That produces `build/pong-sim`, `build/snake-sim`, `build/pong`, `build/snake` and `build/sprites`.
Pass `-- release` for `-O1` with no bounds checks. Edit `SDL2_DIRECTORY` at the top of
[`build.jr`](build.jr) if SDL2 is not in `/opt/homebrew/lib` on your machine.

By hand, one at a time:

```sh
# The rules, with no window — these run under `jr run` as well.
cargo run -q -p jr-cli -- run examples/games/pong/sim.jr \
    -I modules -I examples/games/pong/modules
cargo run -q -p jr-cli -- run examples/games/snake/sim.jr \
    -I modules -I examples/games/snake/modules

# The drawing programs, which need SDL2 on the link line.
cargo run -q -p jr-cli -- build examples/games/pong/main.jr -o /tmp/pong \
    -I modules -I examples/games/pong/modules -L /opt/homebrew/lib
/tmp/pong
```

## What a machine needs

**SDL2.** `brew install sdl2`, or your distribution's `libsdl2-dev`. It is not on the C driver's
default search path, so a build needs `-L /opt/homebrew/lib` or `JR_LIBRARY_PATH` pointing at it —
deliberately a machine property rather than something a source file can state (ADR-0163 §2).

**A display, for the three drawing programs.** They cannot run under `jr run` at all: the
compile-time VM resolves a foreign symbol from the compiler's own process image, so it reaches libc
and nothing else. And SDL's `dummy` video driver has no OpenGL, so a headless run opens a window and
then `Simp.is_ready()` is false.

## Running them without a person at the keyboard

Each drawing program reads a frame budget from the environment and stops when it is reached, so a
checker can run it:

```sh
PONG_FRAMES=120 ./build/pong                      # prints "frames=120 score 0-0", exits with the left score
SNAKE_DEMO=1 SNAKE_FRAMES=120 ./build/snake       # SNAKE_DEMO plays it with Snake.autopilot
SPRITES_FRAMES=120 ./build/sprites
```

Every frame ends with a `GL.error_code()` check, and an observed error exits 74. A clean exit means
that check observed no queued GL error; it does not prove shader success or correct pixels.
**That is the extent of what has been verified.** Nobody has compared the pixels to a reference
image: the window opens on a different macOS Space from a fullscreen terminal, so a screenshot
could not be taken from the session that wrote these.

## Controls

| Program | Keys |
|---|---|
| `pong` | `W`/`S` move the left paddle. The right paddle plays itself. `Escape` or the close box quits. |
| `snake` | `W`/`A`/`S`/`D` turn. `Escape` quits. |
| `sprites` | Click **PAUSE** and **RESET**. `Escape` quits. |

The keys are ASCII literals rather than arrow keys because a keycode below 128 *is* its ASCII value,
and `modules/Input` deliberately carries one keycode constant (`KEY_ESCAPE`) instead of a table. The
arrow keys are above 128 (`SDLK_UP` is `0x40000052`), so a game that wants them declares its own four
constants.

## The one thing worth copying from these

Keep the rules in a module that imports no graphics module. `modules/Pong` imports `modules/Math`;
`modules/Snake` imports `modules/Random`. Both therefore run in the compile-time VM, which means
`jr run` plays a whole game and a rule can be tested with no display, no SDL2 and no window —
[`pong/sim.jr`](pong/sim.jr) counts the rally, [`snake/sim.jr`](snake/sim.jr) asserts that one seed
replays identically. Everything that cannot be tested that way is confined to `main.jr`.
