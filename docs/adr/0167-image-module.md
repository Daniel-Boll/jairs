# ADR-0167: `Image` — BMP, surfaces, textures, and four ambiguous names

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- **PLAN §8.5's image-decode item**, and the last graphics-shaped one — the remaining §8.5 entry is audio,
  which is not. Closes W10's content.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Decision

### 1. BMP, because SDL2 decodes it and nothing else costs nothing

`SDL_LoadBMP_RW` is in the base library. So this module adds **no dependency** on top of the one ADR-0163 §1
already accepted as a stated cost.

**Rejected: PNG via `SDL_image`.** A separate library with its own version skew, for a format this wave does not
need to prove a texture path works. A caller who wants it binds it the way `Window` binds SDL2, and this module
is the shape to copy — which is a better outcome than this module having a PNG path nobody asked for.

**Rejected: a PNG decoder written in Jairs.** It needs zlib's inflate, which is the largest single thing the
standard library would contain and belongs beside a `Compress` module rather than inside `Image`.

**Rejected: deferring images entirely** and calling §8.5's item owed. A texture path that has never carried a
real decoded image is a texture path nobody has tested, and the decode is the step with the *interesting*
failure — a missing file, a corrupt header — which is exactly what a caller needs a flag for. The last two
assertions in this wave's test are that failure.

So the scope is honest rather than grudging: **loading an image works, in the format that is free.**

### 2. Surface and texture both exist, because SDL's shape is real

A surface is pixels in memory; a texture is pixels the renderer draws from, which on an accelerated renderer
means pixels on the GPU. Loading gives a surface, drawing needs a texture, so a caller does both.

This module does **not** hide that. Hiding it means either keeping every surface alive forever or deciding for
the caller when to convert. `load_texture` does the whole sequence for the common case; `load_bmp` plus
`texture_from` are there for a caller who wants the pixels.

**`load_texture` frees the surface on both paths**, including when the upload fails. That is the leak a caller
writing the sequence by hand gets wrong, because the failure path is the one nobody runs.

**`texture_from` does not free**, deliberately: SDL copies out of the surface, and freeing it there would make
`load_bmp` + `texture_from` unusable for a caller who wanted both.

### 3. `Surface_Data` is a `#place` overlay, and its guarantee is weaker than `SDL_Event`'s

Same mechanism as ADR-0165 §1, legal for the same reason: SDL hands back a pointer, and a pointer to a struct is
just a pointer.

**The difference is stated rather than glossed.** `SDL_Event`'s offset 0 is documented in SDL's own header
("shared with all events"). `SDL_Surface`'s `w` at 16 and `h` at 20 are **ABI**, read from `offsetof` rather than
guessed, stable across SDL2's 2.x series but not written down as a contract. `surface_layout_is_sdl2` checks the
96-byte size so a disagreeing platform fails a test instead of reading rubbish.

A reader who saw both overlays would otherwise assume they were equally solid. They are not, and the weaker one
should say so.

**`pitch` is exposed** even though nothing here reads pixels, because a caller who ever does needs it and its
absence is the single most common cause of a skewed image. **`pixels` is exposed and deliberately unread**:
reading it needs the surface's pixel *format*, a second pointer into a second SDL struct, and a caller who wants
to poke pixels wants a format-conversion API rather than one accessor.

### 4. Every export is prefixed — four ambiguous names made the case

Written unprefixed first. A file importing `Window`, `Basic` and this module got **four E0211 ambiguous-name
errors at once**: `fill` and `destroy` collide with `Window`'s, `free` with `Basic`'s, and `layout_is_sdl2` with
`Window`'s.

So the exports are `fill_surface`, `free_surface`, `destroy_texture`, `draw_texture`, `create_surface`,
`load_bmp`, `save_bmp`, `surface_layout_is_sdl2`.

**The compiler catching this is the good outcome** — E0211 is exactly right, and it fired at the first file that
imported all three. ADR-0166 §7 had recorded the flat-namespace hazard one wave earlier as a note; this is it
arriving as four errors.

**The general rule: in a flat namespace a module must prefix as though the namespace were its own**, because
there is no qualification to fall back on and a short exported name is a claim on every importer. `Window` gets
away with `fill` and `close` only because it was first, which is not a principle.

**Rejected: adding qualified imports to fix this.** `Window.fill` is a language change, it is the right one
eventually, and reaching for it mid-wave to avoid renaming eight procedures would be a feature designed by an
inconvenience. Recorded as owed instead.

### 5. `draw_texture` requires a destination, and takes the whole source

SDL accepts a null destination meaning "fill the window". **Rejected**: it reads well and it makes the one
argument that matters optional, so a caller who forgot it gets a full-window stretch instead of a diagnostic.

A null *source* is used, and means "the whole texture" — which is what the routine's name says. The asymmetry is
deliberate: a source has an obvious whole, a destination has none.

### 6. `create_surface` and `save_bmp` exist for the test, and that is a good reason

The test builds a 24x16 surface, fills a rectangle, saves it, and loads it back — so **no binary file lives in
the repository** and the decode is genuinely exercised rather than trusted. A caller building an image
procedurally wants the same two routines, so this is not test-only scaffolding.

## Consequences

- **PLAN §8.5's image item is done**, and W10's content with it. Audio remains, and is not graphics.
- **`modules/Image` is the nineteenth module**, and the third `jr run` cannot execute (ADR-0164 §6's reason).
- **1059 tests**, 253 corpus files.
- **Qualified imports are owed**, promoted from ADR-0166 §7's note by four real collisions.
- **A second `#place` overlay of somebody else's struct**, with its guarantee explicitly weaker than the first's.
  If a third arrives, the pattern deserves a helper — a way to *assert* an offset rather than only a size.
