//! The release profile's panic strategy, which the language server's correctness depends on.
//!
//! # Why a test asserts a build setting
//!
//! salsa signals cancellation by **panicking**, and `jr-lsp` catches it — `salsa::Cancelled::catch`
//! is a `catch_unwind` — to answer the cancelled request with `ContentModified` (ADR-0024 §2,
//! ADR-0032). `panic = "abort"` makes that catch unreachable, so the first keystroke that lands
//! behind an in-flight request kills the server: **exit 134, SIGABRT, and no message at all**,
//! because with abort the panic payload is never handled.
//!
//! It shipped that way and hid for waves, because the two editors disagree about which build they
//! talk to. `editors/nvim/lsp/jairs.lua` picks the *newer* of `target/debug/jr` and
//! `target/release/jr`, so a developer editing this compiler was usually served by an unwinding
//! build; `editors/zed/src/jairs.rs` takes `target/release/jr` unconditionally, and there completion
//! never appeared, because the server died while answering it.
//!
//! # Why it is stated here and not as a behavioural test
//!
//! `cargo test` compiles with the *test* profile, which inherits `dev` and therefore unwinds. **No
//! test can observe the release profile's abort**, so a behavioural test would pass in both worlds
//! and defend nothing. The setting itself is the invariant, and this is the same argument
//! `codes.rs` makes for reading source text: the claim is only stateable at the level where it
//! lives.
//!
//! The cost of unwinding is recorded rather than implied: the binary grows about 19% (6.5 MB →
//! 7.7 MB on arm64 macOS). A change made for size that flips this back has to fail here first.

use std::path::{Path, PathBuf};

/// The workspace root, found by walking up from this test's manifest.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root exists")
}

/// The value of `key` inside the `[profile.release]` table.
fn release_profile_key(key: &str) -> Option<String> {
    let manifest = std::fs::read_to_string(workspace_root().join("Cargo.toml"))
        .expect("the workspace manifest is readable");
    let mut in_release = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_release = line == "[profile.release]";
            continue;
        }
        if !in_release || line.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim() == key {
            return Some(value.trim().trim_matches('"').to_owned());
        }
    }
    None
}

#[test]
fn the_release_profile_unwinds_so_the_language_server_survives_a_cancellation() {
    assert_eq!(
        release_profile_key("panic").as_deref(),
        Some("unwind"),
        "`jr-lsp` answers a cancelled request by catching salsa's unwind; with `panic = \"abort\"` \
         the server aborts on the first keystroke that cancels a request (SIGABRT, silently)"
    );
}

/// The guard against this test silently reading nothing.
///
/// A renamed table or a reformatted manifest would make the scan above return `None` for every key,
/// and a `None` that means "not found" is indistinguishable from a `None` that means "absent on
/// purpose". Asserting a *different* key of the same table is present keeps the parser honest —
/// the same reason `codes.rs` asserts its walk still finds declarations.
#[test]
fn the_release_profile_table_is_found_at_all() {
    assert_eq!(
        release_profile_key("codegen-units").as_deref(),
        Some("1"),
        "the `[profile.release]` scan found nothing, so the assertion above proves nothing"
    );
}
