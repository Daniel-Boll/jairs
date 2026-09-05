//! Generates the table of bundled standard-library modules.
//!
//! # Why a build script rather than `include_dir!`
//!
//! Two reasons, both measured rather than assumed.
//!
//! The first is **correctness**. `include_str!` registers a rustc dependency on the file it
//! reads, so editing a module's *contents* does rebuild. But **adding or removing** a module
//! does not: nothing tells Cargo that the set of files in `modules/` is an input. The failure
//! is silent — a new module is simply absent from the binary, and the compiler reports it as
//! "module not found" on a program that is perfectly correct. `include_dir`'s answer to this
//! is `proc_macro::tracked_path`, which is nightly-only and a no-op on stable, so it does not
//! solve the problem this project has. `cargo::rerun-if-changed` on the *directory* does, and
//! it needs a build script.
//!
//! The second is that once a build script exists, the table it can emit is strictly better
//! than the proc macro's: an `include_str!` entry is a `&'static str`, where `include_dir`
//! stores `&[u8]` and re-validates UTF-8 on **every** access and hands back an `Option` that
//! every caller must then answer for. Measured on this stdlib the generated table also
//! produced a *smaller* binary (927,664 against 929,200 bytes), and it costs no dependency at
//! all — no `proc-macro2`, no `quote`, no `syn`, and no third-party crate to keep current.
//!
//! **Rejected: a compressed archive**, the shape `ty` uses for its vendored typeshed. That is
//! the right call at 7.0 MB across 752 files. Here it would save roughly 318 KB — the stdlib
//! is 484 KB raw against 170 KB deflated — in exchange for a zip dependency, a decompression
//! step on startup, and a virtual-filesystem abstraction between the compiler and its own
//! standard library. The saving does not buy the machinery.

use std::{fmt::Write as _, path::Path};

fn main() {
    // **This line is what makes add and remove take effect.** See the module docs above: it is
    // load-bearing, not hygiene. Cargo scans the whole tree when the path names a directory.
    println!("cargo::rerun-if-changed=../../modules");
    println!("cargo::rerun-if-changed=build.rs");

    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate directory has a workspace root two levels up")
        .join("modules");

    let mut names: Vec<String> = std::fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", root.display()))
        .filter_map(|entry| {
            let entry = entry.ok()?;
            entry
                .path()
                .join("module.jr")
                .is_file()
                .then(|| entry.file_name().to_string_lossy().into_owned())
        })
        .collect();

    // Sorted so the runtime lookup is a binary search rather than a scan. The order is also
    // what makes the generated file stable across machines, which a reviewer reading a diff of
    // it depends on.
    names.sort();

    assert!(
        !names.is_empty(),
        "no modules found under {} — the bundled standard library would be empty, which every \
         program importing `Basic` would then fail to compile against",
        root.display()
    );

    let mut out = String::from(
        "/// Every bundled module, as `(name, source)`, sorted by name.\n\
         pub(crate) static BUNDLED: &[(&str, &str)] = &[\n",
    );
    for name in &names {
        let source = root.join(name).join("module.jr");
        let source = source
            .canonicalize()
            .unwrap_or_else(|e| panic!("cannot canonicalise {}: {e}", source.display()));
        writeln!(out, "    ({name:?}, include_str!({source:?})),").expect("writing to a String");
    }
    out.push_str("];\n");

    let dest =
        Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR is set by Cargo")).join("bundled.rs");
    std::fs::write(&dest, out).unwrap_or_else(|e| panic!("cannot write {}: {e}", dest.display()));
}
