# Examples

Small, runnable `.jr` programs. Each was verified with the command in its own header
comment, and each header records the expected output and exit code.

Run any of them with:

```sh
cargo run -q -p jr-cli -- run examples/<name>.jr -I modules
```

| File | Shows |
|---|---|
| [`01-hello.jr`](01-hello.jr) | The smallest program: one `#import`, one `print_line`. |
| [`02-struct-and-proc.jr`](02-struct-and-proc.jr) | A `struct`, and a procedure that takes one by value. |
| [`03-polymorphic-procedure.jr`](03-polymorphic-procedure.jr) | A `$T` polymorphic procedure, instantiated at two different types. |
| [`04-comptime-run.jr`](04-comptime-run.jr) | `#run` folds a procedure call to a constant at compile time. |
| [`05-target-os.jr`](05-target-os.jr) | `os()`, the compile-time target-operating-system value. |
| [`06-array.jr`](06-array.jr) | A fixed-size `[N]T` array: indexing, `.count`, bounds checks. |
| [`07-file-read.jr`](07-file-read.jr) | Installs an allocator, then writes and reads a whole file. |
| [`08-print-formatted.jr`](08-print-formatted.jr) | `%` placeholders over any type, and what a wrong argument count does. |
| [`09-language-utilities.jr`](09-language-utilities.jr) | Array literals, typed constants, `type_of`, and an enum printed by name. |

A drawing program (`Simp`, `Window`, `Input`) needs SDL2 and cannot run under `jr run` —
the compile-time VM reaches libc and nothing else. None is included here; see
[`docs/capabilities.md`](../docs/capabilities.md) for what the graphics modules can do
and how to `jr build` against them.

For the language itself, see [`../README.md`](../README.md) and
[`../docs/spec/`](../docs/spec/).
