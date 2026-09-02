//! Integration tests: lower every corpus file and check diagnostics.
//!
//! Valid corpus files must lower with zero HIR lowering diagnostics.
//! (Resolution diagnostics are not checked here because the corpus test
//! cannot provide real import scopes — `print` and `print_int` come from
//! `#import "Basic"` which is not available in the test environment.)
//!
//! Invalid corpus files must lower without panicking.
//!
//! The `imports_*` tests hand-wire the module map from `tests/corpus/modules/`
//! and verify the full resolution pipeline against the import corpus fixtures.

use std::collections::HashMap;
use std::path::Path;

use jr_base::{FileId, Interner};
use jr_hir::{FileHir, ItemScope, lower_file, resolve};
use jr_syntax::parse;

#[test]
fn valid_corpus_files_lower_with_zero_lowering_diagnostics() {
    let corpus_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/valid");

    let mut failures = Vec::new();

    let mut entries: Vec<_> = std::fs::read_dir(&corpus_dir)
        .unwrap_or_else(|e| panic!("failed to read corpus dir: {e}"))
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("jr"))
        .collect();
    entries.sort_by_key(|e| e.path());

    for entry in entries {
        let path = entry.path();
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        let interner = Interner::new();
        let file = FileId::from_usize(0);
        let parsed = parse(&source, file);

        // Lowering diagnostics only (not resolution — we can't provide imports).
        let (_hir, diags) = lower_file(&parsed, file, &interner);

        if !diags.is_empty() {
            let msgs: Vec<String> = diags.iter().map(|d| d.message.clone()).collect();
            failures.push(format!(
                "{}: {} lowering diagnostic(s): {}",
                path.file_name().unwrap().to_string_lossy(),
                diags.len(),
                msgs.join("; ")
            ));
        }
    }

    if !failures.is_empty() {
        panic!(
            "Valid corpus files with lowering diagnostics:\n{}",
            failures.join("\n")
        );
    }
}

#[test]
fn invalid_corpus_files_do_not_panic() {
    let corpus_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/invalid");

    let mut entries: Vec<_> = std::fs::read_dir(&corpus_dir)
        .unwrap_or_else(|e| panic!("failed to read corpus dir: {e}"))
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("jr"))
        .collect();
    entries.sort_by_key(|e| e.path());

    for entry in entries {
        let path = entry.path();
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        let interner = Interner::new();
        let file = FileId::from_usize(0);
        let parsed = parse(&source, file);
        // Must not panic:
        let (hir, mut diags) = lower_file(&parsed, file, &interner);
        let (_, resolve_diags) = resolve(&hir, &[], &interner);
        diags.extend(resolve_diags.into_vec());
        // We don't assert anything about the number of diagnostics for invalid files.
        let _ = diags;
    }
}

/// Test that resolution with no imports reports unresolved imported names.
#[test]
fn resolution_without_imports_reports_unresolved_names() {
    let source = r#"
#import "Basic";
main :: () {
    print("hello");
}
"#;
    let interner = Interner::new();
    let file = FileId::from_usize(0);
    let parsed = parse(source, file);
    let (hir, lower_diags) = lower_file(&parsed, file, &interner);
    assert!(
        lower_diags.is_empty(),
        "unexpected lowering diagnostics: {:?}",
        lower_diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );

    // With no imports, `print` should be unresolved
    let (_, resolve_diags) = resolve(&hir, &[], &interner);
    assert!(
        resolve_diags.iter().any(|d| d.message.contains("print")),
        "expected unresolved `print` diagnostic, got: {:?}",
        resolve_diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// Import corpus integration tests
//
// These tests hand-wire the module map from `tests/corpus/modules/` and
// verify the full resolution pipeline against the import corpus fixtures.
// We cannot use `jr-db` here (different crate), so we lower each module
// file ourselves and use `FileHir::export_scope()` to build the map.
// ---------------------------------------------------------------------------

/// Lower a `.jr` source file and return its `FileHir`.
///
/// Panics if lowering produces any diagnostics (modules must be clean).
fn lower_module(source: &str, interner: &Interner, file_id: FileId) -> FileHir {
    let parsed = parse(source, file_id);
    let (hir, diags) = lower_file(&parsed, file_id, interner);
    assert!(
        diags.is_empty(),
        "module lowering produced diagnostics: {:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
    hir
}

/// Read a corpus module file. Tries `<name>/module.jr` first, then `<name>.jr`.
fn read_module_source(modules_dir: &Path, name: &str) -> String {
    let dir_form = modules_dir.join(name).join("module.jr");
    if dir_form.exists() {
        return std::fs::read_to_string(&dir_form)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", dir_form.display()));
    }
    let file_form = modules_dir.join(format!("{name}.jr"));
    std::fs::read_to_string(&file_form)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", file_form.display()))
}

/// Build the module map for the corpus: lower every module in `modules/` and
/// collect their export scopes.
///
/// Returns `(module_hirs, module_scopes)` where `module_hirs` owns the
/// `FileHir` values and `module_scopes` maps module name → `&ItemScope`.
/// The `FileHir` values must outlive the `ItemScope` references.
fn build_module_map(interner: &Interner) -> (HashMap<String, FileHir>, HashMap<String, ItemScope>) {
    let modules_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/modules");

    // **Read from the directory rather than listed by hand** (ADR-0104). This was a literal array of eight
    // names, and adding `Generic.jr` for the cross-file-instantiation refusal made `imports/valid/017` report
    // `unresolved name` — the module existed on disk and not in the list. That is exactly the drift the
    // comment below warns about for the *file* count, one level over: a hand-maintained list of what is in a
    // directory is a list that goes stale, and the failure it produces blames the test file rather than the
    // list. A directory walk cannot go stale.
    //
    // Both layouts are accepted, because the corpus uses both: `Colors.jr` and `Shapes/module.jr`.
    let mut module_names: Vec<String> = std::fs::read_dir(&modules_dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", modules_dir.display()))
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                if !path.join("module.jr").exists() {
                    return None;
                }
                return Some(path.file_name()?.to_string_lossy().into_owned());
            }
            if path.extension().and_then(|e| e.to_str()) != Some("jr") {
                return None;
            }
            Some(path.file_stem()?.to_string_lossy().into_owned())
        })
        .collect();
    // Sorted so a `FileId` is a function of the directory's contents rather than of the filesystem's
    // enumeration order — the same reason every other corpus walk sorts (AGENTS.md on `FileId` churn).
    module_names.sort();

    let mut hirs: HashMap<String, FileHir> = HashMap::new();
    let mut scopes: HashMap<String, ItemScope> = HashMap::new();

    for (i, name) in module_names.iter().enumerate() {
        let source = read_module_source(&modules_dir, name);
        let file_id = FileId::from_usize(i + 1); // 0 is reserved for the importing file
        let hir = lower_module(&source, interner, file_id);
        // Clone the scope before moving hir into the map.
        let scope = hir.export_scope();
        hirs.insert(name.to_string(), hir);
        scopes.insert(name.to_string(), scope);
    }

    (hirs, scopes)
}

/// Resolve a single import corpus file against the module map.
///
/// Returns the list of diagnostics (empty = clean).
fn resolve_import_file(
    source: &str,
    scopes: &HashMap<String, ItemScope>,
    interner: &Interner,
) -> jr_diag::Diagnostics {
    let file_id = FileId::from_usize(0);
    let parsed = parse(source, file_id);
    let (hir, lower_diags) = lower_file(&parsed, file_id, interner);
    assert!(
        lower_diags.is_empty(),
        "import file lowering produced diagnostics: {:?}",
        lower_diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );

    // Build the imports slice from the #import items in the file.
    let mut imports: Vec<jr_hir::ImportedModule<'_>> = Vec::new();
    for item in &hir.items {
        if let jr_hir::ItemKind::Import { path, alias, .. } = &item.kind
            && let Some(scope) = scopes.get(path.as_str())
        {
            imports.push(jr_hir::ImportedModule {
                path: path.as_str(),
                alias: *alias,
                scope,
            });
        }
        // If the module is not in the map, we skip it (E0210 is jr-db's job).
    }

    let (_, resolve_diags) = resolve(&hir, &imports, interner);
    resolve_diags
}

/// All 8 `imports/valid/` files must resolve with zero diagnostics.
#[test]
fn imports_valid_all_resolve_cleanly() {
    let interner = Interner::new();
    let (_hirs, scopes) = build_module_map(&interner);

    let valid_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/imports/valid");

    let mut entries: Vec<_> = std::fs::read_dir(&valid_dir)
        .unwrap_or_else(|e| panic!("failed to read imports/valid dir: {e}"))
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("jr"))
        .collect();
    entries.sort_by_key(|e| e.path());

    // A **floor**, not an exact count. This was `assert_eq!(.., 9)` and before that `8`, and it had
    // to be edited by hand in each of the last two waves that added a file — a count that only ever
    // grows is a count that only ever needs updating, and the assertion it makes ("nobody deleted the
    // corpus") is served just as well by a minimum. The same drift the CLI harness had, where three
    // refusal files went unexercised for two waves.
    assert!(
        entries.len() >= 9,
        "expected at least 9 files in imports/valid/, found {}",
        entries.len()
    );

    let mut failures = Vec::new();
    for entry in &entries {
        let path = entry.path();
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        let diags = resolve_import_file(&source, &scopes, &interner);
        if !diags.is_empty() {
            let msgs: Vec<String> = diags
                .iter()
                .map(|d| format!("[{}] {}", d.code.unwrap_or("?"), d.message))
                .collect();
            failures.push(format!(
                "{}: {} diagnostic(s): {}",
                path.file_name().unwrap().to_string_lossy(),
                diags.len(),
                msgs.join("; ")
            ));
        }
    }

    if !failures.is_empty() {
        panic!(
            "imports/valid/ files with unexpected diagnostics:\n{}",
            failures.join("\n")
        );
    }
}

/// `imports/invalid/002` must produce exactly one E0211 for `blend`,
/// and `BLACK` and `PALETTE_SIZE` must still resolve (no E0201).
#[test]
fn imports_invalid_002_ambiguous_name_is_e0211() {
    let interner = Interner::new();
    let (_hirs, scopes) = build_module_map(&interner);

    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/imports/invalid/002-ambiguous-imported-name.jr");
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));

    let diags = resolve_import_file(&source, &scopes, &interner);

    // Exactly one E0211.
    let e0211: Vec<_> = diags.iter().filter(|d| d.code == Some("E0211")).collect();
    assert_eq!(
        e0211.len(),
        1,
        "expected exactly one E0211, got: {:?}",
        diags
            .iter()
            .map(|d| format!("[{}] {}", d.code.unwrap_or("?"), d.message))
            .collect::<Vec<_>>()
    );

    // The E0211 must mention both modules.
    let msg = &e0211[0].message;
    assert!(msg.contains("Colors"), "E0211 must name `Colors`: {msg}");
    assert!(msg.contains("Palette"), "E0211 must name `Palette`: {msg}");

    // No E0201 (BLACK and PALETTE_SIZE must still resolve).
    let e0201: Vec<_> = diags.iter().filter(|d| d.code == Some("E0201")).collect();
    assert!(
        e0201.is_empty(),
        "BLACK and PALETTE_SIZE must resolve; got E0201: {:?}",
        e0201.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

/// `imports/invalid/003` must produce exactly one E0201 for `not_exported`.
#[test]
fn imports_invalid_003_unresolved_after_import_is_e0201() {
    let interner = Interner::new();
    let (_hirs, scopes) = build_module_map(&interner);

    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/imports/invalid/003-unresolved-after-import.jr");
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));

    let diags = resolve_import_file(&source, &scopes, &interner);

    let e0201: Vec<_> = diags.iter().filter(|d| d.code == Some("E0201")).collect();
    assert_eq!(
        e0201.len(),
        1,
        "expected exactly one E0201 for `not_exported`, got: {:?}",
        diags
            .iter()
            .map(|d| format!("[{}] {}", d.code.unwrap_or("?"), d.message))
            .collect::<Vec<_>>()
    );
    assert!(
        e0201[0].message.contains("not_exported"),
        "E0201 must name `not_exported`: {}",
        e0201[0].message
    );
}
