# Examples

Small, runnable `.jr` programs. Each was verified with the command in its own header
comment, and each header records the expected output and exit code.

Run any of them with:

```sh
cargo run -q -p jr-cli -- run examples/<name>.jr -I modules
```

| File | Shows |
|---|---|
| [`01-hello.jr`](01-hello.jr) | The smallest program: one `#import`, one `print`. |
| [`02-struct-and-proc.jr`](02-struct-and-proc.jr) | A `struct`, and a procedure that takes one by value. |
| [`03-polymorphic-procedure.jr`](03-polymorphic-procedure.jr) | A `$T` polymorphic procedure, instantiated at two different types. |
| [`04-comptime-run.jr`](04-comptime-run.jr) | `#run` folds a procedure call to a constant at compile time. |
| [`05-target-os.jr`](05-target-os.jr) | `os()`, the compile-time target-operating-system value. |
| [`06-array.jr`](06-array.jr) | A fixed-size `[N]T` array: indexing, `.count`, bounds checks. |
| [`07-file-read.jr`](07-file-read.jr) | Writes and reads a whole file through `File` and the default allocator. |
| [`08-print-formatted.jr`](08-print-formatted.jr) | `%` placeholders over any type, and what a wrong argument count does. |
| [`09-language-utilities.jr`](09-language-utilities.jr) | Array literals, typed constants, `type_of`, and an enum printed by name. |
| [`10-build-script.jr`](10-build-script.jr) | A **build script**: `jr build examples/10-build-script.jr -I modules` runs it, and it compiles another program. Shells out for a git hash, reads `-- release`, chooses per OS. |
| [`11-run-build-script.jr`](11-run-build-script.jr) | The same, as a **`#run`** with no `main` — the shape a Jai `build.jai` has. Prints and allocates at compile time; declares its target with `request_build`, because compiling from inside a query is not possible. |

## Games

[`games/`](games) holds three complete programs — Pong, Snake, and a sprite-and-widget demo — with
their own README and their own build script. Two of them split the game's rules into a module that
imports no graphics module, so `jr run` plays a whole match with no display attached; the drawing
halves need SDL2 and a `jr build`.

A drawing program cannot run under `jr run` at all: the compile-time VM resolves a foreign symbol
from the compiler's own process image, so it reaches libc and nothing else. See
[`games/README.md`](games/README.md) for what a machine needs, and
[Book IV of the documentation site](../docs-site/src/content/docs/games/) for the chapters.

For the language itself, see [`../README.md`](../README.md) and
[`../docs/spec/`](../docs/spec/).
