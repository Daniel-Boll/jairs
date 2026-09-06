//! The standard library, compiled into the binary.
//!
//! # Why the modules are in the binary rather than on disk
//!
//! `cargo install` cannot install data files. Cargo's documentation is explicit that only
//! packages with executable `[[bin]]` or `[[example]]` targets can be installed, and that
//! every executable goes to the installation root's `bin` directory — there is no mechanism
//! for anything else. So a `jr` installed the ordinary way has exactly one file to its name,
//! and a standard library that lives beside the source tree is a standard library an installed
//! compiler cannot find.
//!
//! Before this crate existed, the bundled module directory was `env!("CARGO_MANIFEST_DIR")`
//! joined with `../../modules` — the path of the *build* machine's source tree, baked into the
//! binary. That works exactly as long as the compiler runs from the tree it was built in, and
//! its own doc comment said so.
//!
//! # The alternatives, and why each was rejected
//!
//! **Walk up from the executable and look for `modules/`**, as Zig does for its own source
//! standard library. Zig's implementation is careful in a way worth copying — it validates
//! every candidate directory by opening a marker file, `std/std.zig`, rather than trusting a
//! derived path. Rejected here anyway, because it answers a question this design does not have
//! to ask: with the modules in the binary there is no directory to find, so there is no
//! candidate to validate and no way for the search to come back empty. It also divides badly
//! across platforms — `current_exe` returns the path *as invoked* on macOS and the *fully
//! resolved* target on Linux, so one symlinked install resolves to two different directories
//! depending on the host.
//!
//! **Extract to a user data directory on first run.** Rejected: it turns every first
//! invocation into a filesystem write, needs a version stamp to know when to re-extract, and
//! puts the compiler's correctness at the mercy of a directory the user can edit.
//!
//! **Ship a tarball or an installer**, as rustup does. A real answer at scale, and the one to
//! revisit when the standard library outgrows a binary. Rejected now because it gives up the
//! property that matters most today: `cargo install --path crates/jr-cli` is the whole
//! installation procedure, with nothing to unpack and no second step to get wrong.
//!
//! # The seam
//!
//! Project discovery includes every bundled module as a catalog entry under one synthetic root —
//! [`root`]. Keeping a synthetic path still matters: diagnostics can distinguish a built-in source
//! from a filesystem path, while the catalog gives every consumer the same module identity without
//! probing the filesystem from a tracked query (ADR-0213).

// Defines `BUNDLED`: every bundled module as `(name, source)`, sorted by name. The doc comment
// travels with the item in the generated file, which is why there is none here.
include!(concat!(env!("OUT_DIR"), "/bundled.rs"));

use std::path::Path;

/// The name of the file a directory-form module keeps its source in.
const MODULE_FILE: &str = "module.jr";

/// The synthetic directory the bundled modules answer for.
///
/// Deliberately **not** a path any filesystem can produce: the angle brackets make it
/// unmistakable in the list of searched candidates an E0210 prints, and they mean no directory
/// a user creates can shadow the bundled library or be shadowed by it.
const ROOT: &str = "<bundled>";

/// The synthetic catalog root used for bundled standard-library entries.
///
/// Operator `-I` entries and exact dependencies may override a bundled module; implicit local and
/// legacy path entries may not (ADR-0213 §4). This crate deliberately depends on nothing, so it
/// exposes only the path and source table rather than the catalog type that consumes them.
#[must_use]
pub fn root() -> &'static Path {
    Path::new(ROOT)
}

/// The source of the bundled module `path` names, or `None` if it names no bundled module.
///
/// Answers **only** for `<bundled>/<Name>/module.jr`. A real path on disk therefore never
/// reaches this table, which is what stops a user's own `modules/Basic` from being shadowed by
/// the bundled one — and the single-file spelling `<bundled>/<Name>.jr` is not answered either,
/// because every bundled module is in directory form and a second spelling would be a second
/// answer to one question.
#[must_use]
pub fn source(path: &Path) -> Option<&'static str> {
    let mut parts = path.components().rev();

    // `module.jr`, then the module name, then the root — read from the end, so this is a
    // suffix test and not an assumption about how the caller spelled the root.
    if parts.next()?.as_os_str() != MODULE_FILE {
        return None;
    }
    let name = parts.next()?.as_os_str().to_str()?;
    if parts.next()?.as_os_str() != ROOT {
        return None;
    }
    if parts.next().is_some() {
        return None;
    }

    BUNDLED
        .binary_search_by_key(&name, |(bundled, _)| *bundled)
        .ok()
        .map(|index| BUNDLED[index].1)
}

/// Every bundled module's name, sorted.
///
/// Exposed for the diagnostic that lists what a misspelled `#import` could have meant, and for
/// the test that asserts the table is not empty.
pub fn names() -> impl ExactSizeIterator<Item = &'static str> {
    BUNDLED.iter().map(|(name, _)| *name)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{names, root, source};

    #[test]
    fn the_library_is_not_empty() {
        // A build script that found no modules would produce a compiler that cannot compile
        // `#import "Basic"`, and every corpus program imports it. Worth an assertion, because
        // the failure otherwise appears as a resolution error in unrelated code.
        assert!(names().len() >= 20, "got {} modules", names().len());
        assert!(names().any(|name| name == "Basic"));
    }

    #[test]
    fn names_are_sorted() {
        // `source` binary-searches, so an unsorted table silently fails to find modules that
        // are present. Asserted here rather than trusted from `build.rs`.
        let mut sorted: Vec<&str> = names().collect();
        let original = sorted.clone();
        sorted.sort_unstable();
        assert_eq!(original, sorted);
    }

    #[test]
    fn a_bundled_module_resolves_under_the_root() {
        let basic = root().join("Basic").join("module.jr");
        let text = source(&basic).expect("Basic is bundled");
        assert!(text.contains("print"), "Basic should declare `print`");
    }

    #[test]
    fn every_named_module_resolves() {
        for name in names() {
            let path = root().join(name).join("module.jr");
            assert!(
                source(&path).is_some(),
                "{name} is listed but does not resolve"
            );
        }
    }

    #[test]
    fn a_real_path_is_never_answered() {
        // The whole point of the synthetic root: a user's own `modules/Basic` must reach the
        // filesystem, not this table. Were this to answer, an on-disk module would be silently
        // replaced by the bundled one of the same name.
        for path in [
            "modules/Basic/module.jr",
            "/usr/share/jairs/Basic/module.jr",
            "./Basic/module.jr",
            "Basic/module.jr",
        ] {
            assert!(source(Path::new(path)).is_none(), "answered for {path}");
        }
    }

    #[test]
    fn the_single_file_spelling_is_not_answered() {
        // Every bundled module is in directory form. Answering the single-file spelling too
        // would give one module two paths, and `module_file` would report whichever it probed
        // first as *the* location in a diagnostic.
        assert!(source(&root().join("Basic.jr")).is_none());
    }

    #[test]
    fn a_module_that_is_not_bundled_is_not_answered() {
        assert!(source(&root().join("Nonexistent").join("module.jr")).is_none());
    }

    #[test]
    fn a_deeper_path_under_the_root_is_not_answered() {
        // A suffix test alone would accept `<bundled>/x/Basic/module.jr`. It must not: that
        // path names a nested directory the bundled library does not have.
        let nested = PathBuf::from("<bundled>/extra/Basic/module.jr");
        assert!(source(&nested).is_none());
    }
}
