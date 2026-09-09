---
title: A build script in Jairs
description: "A build script as an ordinary Jairs program that talks to the compiler through #compiler_library, and the narrower #run shape that declares a build rather than performing one."
sidebar:
  order: 120
---

Jairs supports two build-script shapes. A `main`-shaped script can read arguments, run commands and
call `Compiler.build` immediately. A file-scope `#run` can use the driver-backed file and command
procedures too, but it declares targets with `Compiler.request_build` because the compiler cannot start
a second compilation while the script's own query is active. This chapter uses `main` because it wants
an immediate success flag and a `release` argument, not because compile-time code cannot do useful
build work (ADR-0195–0198).

## A script that shells out and configures a target

```jr
#import "Basic";
Compiler :: #import "Compiler";
String :: #import "String";

main :: () {
    // `Compiler.arguments()` allocates through the default context allocator. A script can still
    // replace the pair when it wants an arena or another policy.
    args := Compiler.arguments();
    is_release := args.count > 0 && String.equal(args[0], "release");

    // Shell out, which is what a build script is *for*: a git hash to stamp in, a shader to compile,
    // a bundle to sign. The driver spawns the process, so there is no `argv` to marshal.
    git := Compiler.command("git");
    Compiler.argument_of(git, "rev-parse");
    Compiler.argument_of(git, "--short");
    Compiler.argument_of(git, "HEAD");
    status := Compiler.run(git);
    if status == 0 {
        print("at commit %", Compiler.output(git));
    }
    if status != 0 {
        print("not a git checkout, carrying on\n");
    }

    if is_release {
        print("release build\n");
    }
    if !is_release {
        print("debug build\n");
    }

    // Per-OS decisions are ordinary code here, not a directive.
    if Compiler.os() == Compiler.Operating_System.MACOS {
        print("building for macOS\n");
    }

    level: s64 = 0;
    if is_release {
        level = 1;
    }

    // A literal must be named before it is viewed: there is no implicit array-to-view conversion.
    paths := string.["modules"];

    t := Compiler.create_target("hello");
    o := Compiler.options(t);
    o.output = "hello-from-a-build-script";
    o.opt_level = level;
    o.bounds_checks = !is_release;
    o.module_paths = paths[];
    Compiler.set_options(t, o);
    Compiler.add_file(t, "examples/01-hello.jr");

    if !Compiler.build(t) {
        Compiler.report("the target did not build");
        exit(1);
    }
    exit(0);
}
```

There is no `--script` flag here: a file that imports `modules/Compiler` **is** a build script, because
that import is what gives it the driver's vocabulary at all — `jr build examples/10-build-script.jr -I
modules` from the repository root prints what the script learned, then `note: built
hello-from-a-build-script`. `git rev-parse` runs through `Compiler.command`, not `Process.run`, because
the **driver** spawns the process with ordinary Rust strings — a Jairs build script cannot use
`Process` to shell out while running under `jr run` (see [Processes](/by-example/100-processes/)), since
the VM's one-level pointer translation cannot marshal `execvp`'s array of pointers correctly. There is
nothing to marshal here because the host, not the VM, is the one making the call.

## `#compiler_library`, and why a build script cannot compile itself

```jr
// **No `#import "Basic"` here**, deliberately: everything this file uses from outside itself —
// `size_of`, `view`, `typed`, `context` — is a compiler intrinsic rather than a library name, so the
// import would be dead weight and E0231 says so.

// The compiler this script is running inside. Not a library: `#compiler_library` is what tells the
// VM to forward these calls to the driver, and it takes no name because there is nothing to name.
compiler :: #compiler_library;

create_target_raw :: (name: string) -> s64 #foreign compiler "create_target";
build_raw :: (target: s64) -> bool #foreign compiler "build";
request_build_raw :: (target: s64) #foreign compiler "request_build";

/// A compilation the script is describing.
Target :: struct {
    /// The driver's handle. Opaque; a script never needs to read it.
    id: s64;
}

/// Compiles `t`, and reports whether it succeeded.
///
/// The whole of what a script needs from Jai's four-call intercept-and-poll dance: with no message
/// loop there is nothing to interleave, so there is nothing for a script to do between "start" and
/// "finished" except find out which it got.
build :: (t: Target) -> bool #must {
    return build_raw(t.id);
}

/// Asks the driver to build `t` once the script has finished.
///
/// # When to use this rather than `build`
///
/// In a `#run`. A `#run` is evaluated *during* the script's own compilation, and starting another
/// compilation from inside one is not something the query engine permits — it needs a second database
/// while the first is mid-query. So a `#run` **declares** what it wants and the driver builds it
/// afterwards, which is exactly the shape Jai's `build.jai` has.
request_build :: (t: Target) {
    request_build_raw(t.id);
}
```

`#foreign compiler "create_target"` reuses the ordinary `#foreign` declaration form rather than inventing
a new grammar rule: `compiler` is not a library and is never linked — the VM recognises `#compiler_library`
and forwards the call to the driver instead of a dynamic loader (ADR-0195 §3). Running a script with
plain `jr run` makes that visible rather than silent: the call says by name that it needs the driver,
rather than quietly resolving to nothing.

`build(t)` cannot be called from inside a `#run`: a `#run` is evaluated *while* its own file is still
being compiled, and asking the driver to compile a second target mid-query is not something this
compiler's memoising query engine permits — it panics with "Cannot change database mid-query." So a
`#run`-shaped build script, the one that matches Jai's own model most closely, uses `request_build`
instead, which only **declares** the target; the driver performs the compilation once const-evaluation
has finished, the same two-phase shape Jai's own `build.jai` has under `add_build_file`.

## The `#run` shape, and what it can and cannot do

```jr
#import "Basic";
Compiler :: #import "Compiler";

build :: () {
    print("configuring at compile time\n");

    if Compiler.os() == Compiler.Operating_System.MACOS {
        print("target is macOS\n");
    }

    // Allocation at compile time uses the comptime VM's default allocator and its own region.
    paths := string.["modules"];

    t := Compiler.create_target("hello");
    o := Compiler.options(t);
    o.output = "hello-from-a-run";
    o.module_paths = paths[];
    Compiler.set_options(t, o);
    Compiler.add_file(t, "examples/01-hello.jr");

    // Declares rather than compiles — see the header.
    Compiler.request_build(t);
    print("target declared\n");
}

#run build();
```

There is **no `main`** in this file, and `jr build examples/11-run-build-script.jr -I modules` prints
what the `#run` printed, then `note: built hello-from-a-run`. Inside a `#run`, comptime code can compute,
allocate, build strings and print — allocation works because the VM serves `malloc` from its own linear
region rather than calling libc, and `print` works because `write` fills the VM's capture buffer, which
the driver emits, so neither reaches a real host and neither is the comptime FFI refusal ADR-0006 still
enforces. It **can** shell out through `Compiler.command`, for the reason the previous section gives —
the driver, not the VM, spawns the process. What a `#run` **cannot** do is use `modules/File`: that
module's flags (`CREATE`, `TRUNCATE`, `APPEND`) are themselves `#run` constants of *their own*, and a
module's own compile-time evaluation is the one thing this project's compile-time evaluation cannot
reach into. A `#run` that needs to read or write a file at compile time uses `Compiler.read_file` and
`Compiler.write_file` instead, which are host-mediated by the driver rather than `#foreign`.

## Why a game reaches for this

A build script is where a game stamps a git commit hash into its binary for a crash report, compiles a
shader with a shelled-out tool before linking, or picks `--opt-level` and bounds checks per target
(debug build with checks on, release build with them off) from one file rather than a shell script
wrapping `jr build` with different flags per platform. `Build_Options.kind`, an `Output_Kind` (an
executable, a static or dynamic library, or a bare object), is what lets a script build a game's core
logic as a library and link a second, thinner executable against it for a level editor or a headless
simulation runner sharing the same code.

## What is absent, and why

There is <span class="jairs-status refused">refused</span>, not merely absent, message-loop
interception: `compiler_begin_intercept`, `.TYPECHECKED`, answering `provide_import` in reply to a
`FAILED_IMPORT` message mid-compilation. Jai's compiler is threads and a queue, so a script can watch a
compilation proceed and answer questions as they arrive; this compiler is a memoising query engine, where
a compilation observed halfway through is precisely what the architecture cannot allow — the same reason
a `#run` cannot call `build` on itself. `provide_import` still exists, supplying the mapping **up front**
rather than in reply to a failure, and `Output_Kind.OBJECT` hands a script the bare object so it can run
its own linker — both are the useful half of the message loop, reached by a different route.

AST inspection and modification (`compiler_modify_procedure` and friends) are also <span
class="jairs-status refused">refused</span>, for the same architectural reason plus one more:
`noted_declarations` already answers the note-driven question a real build script asks, without needing
to rewrite a procedure's body from outside it.

Windows icons and manifests, and a C-bindings generator, are <span class="jairs-status absent">absent</span>
rather than refused: this compiler has never run on Windows, so a portability claim about editing a PE
resource section would be untestable, and a bindings generator is a C header parser and a project of its
own. A script can shell out to an existing tool for either through `Compiler.command` today.

See also [Book I — The Jairs Language](/language/introduction/).
