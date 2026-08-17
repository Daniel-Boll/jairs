---
title: A generated task runner
description: Tag procedures with a note and have the compiler generate a call to each one.
sidebar:
  order: 6
---

The last program shows the feature Jai calls its superpower, and Jairs inherits: a
metaprogram that **finds every declaration tagged with a note and generates code for each
one**. Here we tag a set of "tasks" and let one line of source generate a call to all of
them — a registry with no registration boilerplate.

```jr
#import "Basic";

// Three tasks, each tagged @task. A metaprogram generates a call to every one —
// including `seed_admin`, which is declared *after* the splice, because the query
// walks the file's declarations rather than what is in scope at that point.
migrate_db :: () -> s64 @task {
    print("  running: migrate_db\n");
    return 1;
}

warm_cache :: () -> s64 @task {
    print("  running: warm_cache\n");
    return 2;
}

// Not a task, so the generated code skips it.
internal_helper :: () -> s64 {
    return 100;
}

main :: () {
    print("running all @task procedures:\n");

    total := 0;
    // One line generates `total = total + <name>();` for every @task procedure,
    // with `#` standing for each task's name.
    #insert noted_insert("task", "total = total + #();");

    print("done; checksum = ");
    print_int(total);
    print("\n");
}

seed_admin :: () -> s64 @task {
    print("  running: seed_admin\n");
    return 4;
}
```

Output:

```
running all @task procedures:
  running: migrate_db
  running: warm_cache
  running: seed_admin
done; checksum = 7
```

## How it works

**Tagging with a note.** `@task` is a [note](/language/metaprogramming/#note-metadata) — pure
metadata that affects no generated code on its own. Three procedures carry it; `internal_helper`
does not.

**Generating a call to each.** The single line

```jr
#insert noted_insert("task", "total = total + #();");
```

does the whole job. `noted_insert` walks every declaration tagged `@task`, in **declaration
order**, and emits the template once per match, with `#` replaced by that declaration's name.
So this one line expands to three statements:

```jr
total = total + migrate_db();
total = total + warm_cache();
total = total + seed_admin();
```

`internal_helper` is skipped because it has no `@task`. The checksum is therefore `1 + 2 + 4
= 7`, and it would change the moment a task were added, removed, or mis-tagged — which is what
makes the generation load-bearing rather than decorative.

**Order, not scope.** Notice `seed_admin` is declared *after* `main`, yet it is still called.
`noted_insert` walks the file's declarations, not the names in scope at the splice point — so
adding a new `@task` anywhere in the file is enough to wire it in.

## Why this happens at compile time

The generation runs *while the program is being checked* — it has to, because the generated
statements must exist before type-checking can see them. A run-time loop could never do this:
by the time the program is running, its code is already compiled. This is why the metaprogram
loop [lives inside the fold](/language/metaprogramming/#generating-code-for-each-noted-declaration)
rather than being an ordinary `for`. What is still missing is the reverse — *inspecting*
declarations as run-time values — which needs machinery Jairs does not yet have.

## What it demonstrates

- `@note` metadata and `noted_insert` for note-driven code generation.
- `#insert` splicing generated statements into the enclosing scope.
- A registry pattern with zero registration boilerplate, resolved at compile time.

That's the end of Book III. To go deeper on any piece, [Book I](/language/introduction/) is
the narrative reference and [Book II](/by-example/) is the feature-by-feature catalogue.
