---
title: A window and an event loop
description: Open one window, drain its events, and understand the two sizes a drawable window can report.
sidebar:
  order: 2
---

The first useful graphical program is a window that closes correctly:

```jr
if !Window.start() return;

window := Window.create_window(800, 600, "My game");
if !Window.is_open(*window) {
    Window.stop();
    return;
}

while !Input.wants_to_close(Input.MAX_EVENTS_PER_FRAME) {
    Window.delay(16);
}

Window.close(*window);
Window.stop();
```

Build it with `jr build`, not `jr run`, and link SDL2 as shown in the
[overview](/games/). The important part is not the empty window. It is the lifecycle:
start the platform, verify the window, poll until quit, then close and stop.

## A real game drains once per frame

`wants_to_close` is convenient for a program that needs only one answer. A game usually needs the
events themselves, so it keeps one buffer and fills it at the start of each frame:

```jr
events: Input.Events;

while true {
    _ = Input.update_window_events(
        *events,
        Input.MAX_EVENTS_PER_FRAME,
    );
    if Input.frame_wants_to_close(*events) {
        break;
    }

    seen := Input.events_this_frame(*events);
    // Fold `seen` into this frame's controls.
}
```

Choose one polling shape per frame. Polling consumes events: after
`update_window_events` has stored them, asking `wants_to_close` would inspect what remains in
SDL's queue, not the frame you just captured.

Both polling helpers process **up to `limit` events**. They stop when the limit is reached or as soon
as a poll finds the queue empty. Events beyond the limit remain for a later call. This keeps one busy
frame from spending unbounded time on input, but it also means `limit` should normally be
`MAX_EVENTS_PER_FRAME`.

## Turn edges into held controls

SDL gives the program edges: `KEY_DOWN` and `KEY_UP`. Pong wants a level—“is W down now?”—so its
driver folds those edges into a tiny struct:

```jr
Keys :: struct {
    up: bool;
    down: bool;
    quit: bool;
}
```

The fold is ordinary game code:

```jr
if kind == Input.KEY_DOWN {
    if code == KEY_W { keys.up = true; }
    if code == KEY_S { keys.down = true; }
}
if kind == Input.KEY_UP {
    if code == KEY_W { keys.up = false; }
    if code == KEY_S { keys.down = false; }
}
```

`Input.should_close(*event)` covers both the application quit event and a window-close event.
`Input.pressed(*event, keycode)` is useful for one-shot actions because it excludes key-repeat
events.

The held-state struct is caller-owned because that keeps the example explicit and supports separate
state for separate windows or players. It is not forced by missing language support: modules can
hold mutable globals. A future input facade could keep the same state internally.

## Window size is not render size

`Window.get_window_size` reports the size the window manager knows. The renderer asks
`Simp.get_render_dimensions` for the OpenGL backing-store size. On a high-DPI display those can
differ, so use the latter when building a viewport or projection.

The full SDL event overlay, constants, constructors and test helpers live in
`modules/Input/module.jr`; the window declarations live in `modules/Window/module.jr`. Most games
should not need to know their ABI layout—only when to drain, what the event means and who owns the
result.

Next: [Drawing with Simp](/games/drawing-with-simp/) — turn that empty window into a frame.
