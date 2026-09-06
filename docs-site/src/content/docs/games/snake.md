---
title: Snake
description: Snake's ring-buffer body, buffered turns and seeded apples, played headless and then with a window.
sidebar:
  order: 9
---

By the end of this chapter you will have one Snake rules module used in two ways: a deterministic
headless simulation and an SDL2 window that draws the same state.

Start with the simulation:

```sh
cargo run -q -p jr-cli -- run examples/games/snake/sim.jr -I modules -I examples/games/snake/modules
```

The rules module imports `Random`, not `Math`, and knows nothing about windows or rendering. That
separation is what makes the command above possible.

The runner plays one seed twice and compares the score, tick count, and ending event. Those
aggregates are useful evidence that replay is deterministic; they are not proof that every
per-tick trajectory matched. A full proof would record and compare the whole trace.

## Build the rules around ticks

The most important input rule is to buffer a turn until the next tick:

```jr
turn :: (state: *State, direction: s64) {
    if direction == DIR_NONE {
        return;
    }
    if is_reverse(state.direction, direction) {
        return;
    }
    state.next_direction = direction;
}
```

If two key events arrive before the snake moves, this prevents the second event from reversing
into the neck. Rendering can run at any frame rate because the game advances only when the fixed
tick accumulator owes a step:

```jr
while pending >= state.step_seconds && !Snake.is_over(*state) {
    pending = pending - state.step_seconds;
    if demo {
        Snake.turn(*state, Snake.autopilot(*state));
    }
    _ = Snake.step(*state);
}
```

Use a `while`, not an `if`: after a slow frame the simulation may owe more than one tick.

## Store the body without allocating

The body is a ring buffer in a fixed array. Advancing the head is constant-time, and the module
stays allocator-free:

```jr
GRID_WIDTH :: 24;
GRID_HEIGHT :: 18;
GRID_CELLS :: 432;
GRID_CELLS_IS_CONSISTENT :: GRID_CELLS == GRID_WIDTH * GRID_HEIGHT;

State :: struct {
    body: [GRID_CELLS]Cell;
    head: s64;
    length: s64;
    // ...
}
```

The literal `432` is a current language workaround. `[GRID_WIDTH * GRID_HEIGHT]Cell` is E0233
because computed array lengths are not accepted yet. The folded consistency constant keeps the
literal honest.

Two small details keep the ring correct:

- Jairs `%` follows C, so a negative index needs explicit wrapping rather than
  `-1 % GRID_CELLS`.
- The collision test excludes the tail cell, because that cell is vacated by the same tick.

Apple placement first tries random empty cells, then falls back to scanning the grid. The bounded
fallback matters near a win, when random retries could otherwise run indefinitely.

## Put the same rules in a window

```sh
cargo run -q -p jr-cli -- build examples/games/snake/main.jr -o /tmp/snake \
    -I modules -I examples/games/snake/modules -L /opt/homebrew/lib
SNAKE_DEMO=1 SNAKE_FRAMES=120 /tmp/snake
```

`SNAKE_DEMO=1` lets the autopilot drive, which is useful for a smoke run. The program checks the
OpenGL error queue; a clean exit means no GL error was observed during those frames, not that the
rendered pixels or every possible graphics failure were proved correct.

Next: [Sprites and widgets](/games/sprites/) — textures built in memory, and the `LEFT_HANDED`
cost of using `modules/UI`.
