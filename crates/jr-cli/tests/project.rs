//! Integration tests for the project surface: `jr new`, `jr init`, `jairs.toml`, and the bundled
//! standard library.
//!
//! # Why these run the binary rather than calling the command functions
//!
//! `crates/jr-cli/tests/integration.rs` calls `jr_cli::commands::*::run` directly, which is faster
//! and gives better failure messages. It cannot work here: every behaviour in this file depends on
//! the **working directory**, because that is how a project is discovered, and `set_current_dir`
//! is process-global. Two of these tests running in parallel — which is the default — would see
//! each other's directory. A subprocess has its own.
//!
//! So the rule for this file: if the assertion involves where the command was run from, it goes
//! through `Command`. If it does not, it belongs in `integration.rs` with the cheaper shape.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

/// The `jr` binary under test, built by Cargo for this integration test.
fn jr() -> Command {
    Command::new(env!("CARGO_BIN_EXE_jr"))
}

/// Runs `jr` with `args` in `dir`, returning `(exit code, stdout, stderr)`.
fn run_in(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let out = jr()
        .args(args)
        .current_dir(dir)
        .output()
        .expect("the jr binary must be runnable");
    (
        // A signal-killed child has no code. Reported as -1 rather than unwrapped, so a crash
        // fails the assertion it was going to fail anyway instead of panicking here.
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A scaffolded project in a fresh temporary directory, returning the guard and the project root.
fn scaffolded() -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().expect("tempdir");
    let (code, _, stderr) = run_in(dir.path(), &["new", "demo"]);
    assert_eq!(code, 0, "jr new failed: {stderr}");
    let root = dir.path().join("demo");
    (dir, root)
}

// ---------------------------------------------------------------------------
// The bundled standard library
// ---------------------------------------------------------------------------

#[test]
fn a_program_importing_basic_needs_no_module_path() {
    // The reason the standard library is compiled into the binary. Before that, this program
    // needed `-I <the repo>/modules` — a path an installed `jr` has no way to know.
    let dir = TempDir::new().expect("tempdir");
    let file = dir.path().join("p.jr");
    std::fs::write(
        &file,
        "#import \"Basic\";\nmain :: () {\n    print(\"ok\\n\");\n}\n",
    )
    .expect("write");

    let (code, stdout, stderr) = run_in(dir.path(), &["run", "p.jr"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout, "ok\n");
}

#[test]
fn a_missing_module_names_the_bundled_root_in_the_diagnostic() {
    // The synthetic root appears in user-facing output, so its spelling is part of the contract.
    // A reader seeing `<bundled>/Nope/module.jr` must be able to tell it is not a real directory
    // they should go and look in.
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(
        dir.path().join("p.jr"),
        "#import \"Nope\";\nmain :: () {}\n",
    )
    .expect("write");

    let (code, _, stderr) = run_in(dir.path(), &["check", "p.jr"]);
    assert_eq!(code, 1);
    assert!(stderr.contains("E0210"), "stderr: {stderr}");
    assert!(stderr.contains("<bundled>/Nope"), "stderr: {stderr}");
}

#[test]
fn a_module_path_shadows_the_bundled_library() {
    // Precedence, asserted from the outside: an explicit `-I` is searched before the bundled
    // library (ADR-0014 §1), so a project can ship its own `Basic`. The positive and negative
    // halves are both here, because the positive alone passes even if `-I` is ignored and the
    // bundled `Basic` happens to satisfy the program.
    let dir = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("mods").join("Basic")).expect("mkdir");
    std::fs::write(
        dir.path().join("mods").join("Basic").join("module.jr"),
        "SHADOWED :: 7;\n",
    )
    .expect("write");
    std::fs::write(
        dir.path().join("p.jr"),
        "#import \"Basic\";\nmain :: () {\n    n := SHADOWED;\n}\n",
    )
    .expect("write");

    let (with, _, stderr) = run_in(dir.path(), &["check", "p.jr", "-I", "mods"]);
    assert_eq!(with, 0, "the on-disk module must win: {stderr}");

    let (without, _, stderr) = run_in(dir.path(), &["check", "p.jr"]);
    assert_eq!(
        without, 1,
        "without -I the bundled Basic applies, which has no SHADOWED: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// `jr new` and `jr init`
// ---------------------------------------------------------------------------

#[test]
fn new_scaffolds_a_project_that_builds_and_runs() {
    let (_guard, root) = scaffolded();

    for expected in ["jairs.toml", "build.jr", ".gitignore"] {
        assert!(root.join(expected).is_file(), "missing {expected}");
    }
    assert!(root.join("src").join("main.jr").is_file());

    // The scaffold's whole promise: it compiles on the first try, with no arguments.
    let (code, stdout, stderr) = run_in(&root, &["run"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("Hello"), "stdout: {stdout}");
}

#[test]
fn new_refuses_an_existing_directory() {
    let dir = TempDir::new().expect("tempdir");
    std::fs::create_dir(dir.path().join("taken")).expect("mkdir");

    let (code, _, stderr) = run_in(dir.path(), &["new", "taken"]);
    assert_ne!(code, 0);
    assert!(stderr.contains("already exists"), "stderr: {stderr}");
    // The message has to say what to do instead, or the reader's next move is to delete a
    // directory they may not have meant to.
    assert!(stderr.contains("jr init"), "stderr: {stderr}");
}

#[test]
fn init_scaffolds_in_place_and_names_the_project_after_the_directory() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().join("my-thing");
    std::fs::create_dir(&root).expect("mkdir");

    let (code, _, stderr) = run_in(&root, &["init"]);
    assert_eq!(code, 0, "stderr: {stderr}");

    let manifest = std::fs::read_to_string(root.join("jairs.toml")).expect("read");
    assert!(
        manifest.contains("name = \"my-thing\""),
        "manifest: {manifest}"
    );
}

#[test]
fn init_refuses_to_replace_an_existing_manifest() {
    let dir = TempDir::new().expect("tempdir");
    let manifest = dir.path().join("jairs.toml");
    std::fs::write(&manifest, "[project]\nname = \"mine\"\n").expect("write");

    let (code, _, stderr) = run_in(dir.path(), &["init"]);
    assert_ne!(code, 0, "stderr: {stderr}");
    assert!(stderr.contains("already exists"), "stderr: {stderr}");
    assert_eq!(
        std::fs::read_to_string(&manifest).expect("read"),
        "[project]\nname = \"mine\"\n",
        "the existing manifest must be untouched"
    );
}

// ---------------------------------------------------------------------------
// The manifest
// ---------------------------------------------------------------------------

#[test]
fn a_bare_command_finds_the_entry_point_from_a_subdirectory() {
    let (_guard, root) = scaffolded();

    // The reason discovery walks up rather than reading only the working directory: the same
    // command has to mean the same thing anywhere inside the project.
    let (code, stdout, stderr) = run_in(&root.join("src"), &["run"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("Hello"), "stdout: {stdout}");
}

#[test]
fn the_project_name_names_the_binary_and_an_output_flag_still_wins() {
    let (_guard, root) = scaffolded();

    let (code, _, stderr) = run_in(&root, &["build"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(
        root.join("demo").is_file(),
        "the artefact should be named after the project, at its root"
    );

    // `-o` is an instruction from the operator and outranks the manifest.
    let (code, _, stderr) = run_in(&root, &["build", "-o", "chosen"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(root.join("chosen").is_file());
}

#[test]
fn the_scaffolded_gitignore_covers_the_artefact_the_scaffold_produces() {
    // These two are written by different functions from the same name. A scaffold that ignores
    // `/demo` while building `src/main` would be silently useless.
    let (_guard, root) = scaffolded();
    let (code, _, stderr) = run_in(&root, &["build"]);
    assert_eq!(code, 0, "stderr: {stderr}");

    let ignore = std::fs::read_to_string(root.join(".gitignore")).expect("read");
    assert!(ignore.lines().any(|line| line == "/demo"), "{ignore}");
}

#[test]
fn the_manifest_sets_the_indent_width() {
    let (_guard, root) = scaffolded();
    std::fs::write(root.join("jairs.toml"), "[fmt]\nindent_width = 2\n").expect("write");
    let src = root.join("src").join("main.jr");
    std::fs::write(&src, "main :: () {\nn := 1;\n}\n").expect("write");

    let (code, _, stderr) = run_in(&root, &["fmt"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let formatted = std::fs::read_to_string(&src).expect("read");
    assert!(
        formatted.contains("\n  n := 1;"),
        "expected two spaces, got: {formatted:?}"
    );
}

#[test]
fn the_manifest_selects_tabs() {
    // Tabs were impossible before this wave: the formatter hard-coded `" ".repeat(..)`.
    let (_guard, root) = scaffolded();
    std::fs::write(root.join("jairs.toml"), "[fmt]\nindent_style = \"tab\"\n").expect("write");
    let src = root.join("src").join("main.jr");
    std::fs::write(&src, "main :: () {\nn := 1;\n}\n").expect("write");

    let (code, _, stderr) = run_in(&root, &["fmt"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let formatted = std::fs::read_to_string(&src).expect("read");
    assert!(
        formatted.contains("\n\tn := 1;"),
        "expected a tab, got: {formatted:?}"
    );
}

#[test]
fn a_malformed_manifest_is_reported_and_nothing_is_formatted() {
    // The argument for `deny_unknown_fields`: the alternative is a formatter that appears to
    // ignore the file the user just edited. The file must also be left alone — reporting the
    // problem and then reformatting with the defaults would be the worst of both.
    let (_guard, root) = scaffolded();
    std::fs::write(root.join("jairs.toml"), "[fmt]\nindent_size = 2\n").expect("write");
    let src = root.join("src").join("main.jr");
    let unformatted = "main :: () {\nn := 1;\n}\n";
    std::fs::write(&src, unformatted).expect("write");

    let (code, _, stderr) = run_in(&root, &["fmt"]);
    assert_ne!(code, 0);
    assert!(
        stderr.contains("not a valid manifest") && stderr.contains("indent_size"),
        "the error must name the file and the key: {stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(&src).expect("read"),
        unformatted,
        "a broken manifest must not cause a rewrite"
    );
}

#[test]
fn the_manifest_can_add_a_module_search_path() {
    let (_guard, root) = scaffolded();
    std::fs::create_dir_all(root.join("vendor").join("Extra")).expect("mkdir");
    std::fs::write(
        root.join("vendor").join("Extra").join("module.jr"),
        "ANSWER :: 42;\n",
    )
    .expect("write");
    std::fs::write(
        root.join("jairs.toml"),
        "[project]\nname = \"demo\"\n\n[build]\nmodule_paths = [\"vendor\"]\n",
    )
    .expect("write");
    std::fs::write(
        root.join("src").join("main.jr"),
        "#import \"Extra\";\nmain :: () {\n    n := ANSWER;\n}\n",
    )
    .expect("write");

    let (code, _, stderr) = run_in(&root, &["check"]);
    assert_eq!(code, 0, "stderr: {stderr}");

    // And the path is relative to the manifest, not to the working directory — so the same
    // command works from a subdirectory.
    let (code, _, stderr) = run_in(&root.join("src"), &["check"]);
    assert_eq!(code, 0, "from a subdirectory: {stderr}");
}

#[test]
fn manifest_projects_import_direct_children_of_src_without_a_modules_directory() {
    let (_guard, root) = scaffolded();
    std::fs::write(root.join("src").join("Numbers.jr"), "ANSWER :: 40;\n").expect("write");
    std::fs::create_dir_all(root.join("src").join("More")).expect("mkdir");
    std::fs::write(
        root.join("src").join("More").join("module.jr"),
        "EXTRA :: 2;\n",
    )
    .expect("write");
    std::fs::write(
        root.join("src").join("main.jr"),
        "#import \"Basic\";\n#import \"Numbers\";\n#import \"More\";\nmain :: () {\n    exit(ANSWER + EXTRA);\n}\n",
    )
    .expect("write");

    let (code, _, stderr) = run_in(&root, &["check"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let (code, _, stderr) = run_in(&root, &["run"]);
    assert_eq!(code, 42, "stderr: {stderr}");
}

#[test]
fn exact_dependencies_name_a_file_or_only_a_directories_module_file() {
    let (_guard, root) = scaffolded();
    std::fs::create_dir_all(root.join("vendor").join("Geometry")).expect("mkdir");
    std::fs::write(
        root.join("vendor").join("Geometry").join("module.jr"),
        "POINTS :: 40;\n",
    )
    .expect("write");
    // This sibling must not become importable merely because Geometry's directory was named.
    std::fs::write(
        root.join("vendor").join("Geometry").join("Hidden.jr"),
        "HIDDEN :: 99;\n",
    )
    .expect("write");
    std::fs::write(root.join("vendor").join("Noise.jr"), "NOISE :: 2;\n").expect("write");
    std::fs::write(
        root.join("jairs.toml"),
        "[project]\nname = \"demo\"\n\n\
         [dependencies]\n\
         Geometry = { path = \"vendor/Geometry\" }\n\
         Noise = { path = \"vendor/Noise.jr\" }\n",
    )
    .expect("write");
    std::fs::write(
        root.join("src").join("main.jr"),
        "#import \"Basic\";\n#import \"Geometry\";\n#import \"Noise\";\nmain :: () {\n    exit(POINTS + NOISE);\n}\n",
    )
    .expect("write");

    let (code, _, stderr) = run_in(&root, &["run"]);
    assert_eq!(code, 42, "stderr: {stderr}");

    std::fs::write(
        root.join("src").join("main.jr"),
        "#import \"Hidden\";\nmain :: () {}\n",
    )
    .expect("write");
    let (code, _, stderr) = run_in(&root, &["check"]);
    assert_ne!(
        code, 0,
        "a sibling of an exact dependency leaked into the catalog"
    );
    assert!(stderr.contains("Hidden"), "stderr: {stderr}");
}

#[test]
fn two_local_spellings_of_one_module_are_a_configuration_error() {
    let (_guard, root) = scaffolded();
    std::fs::write(root.join("src").join("Same.jr"), "A :: 1;\n").expect("write");
    std::fs::create_dir_all(root.join("src").join("Same")).expect("mkdir");
    std::fs::write(root.join("src").join("Same").join("module.jr"), "B :: 2;\n").expect("write");

    let (code, _, stderr) = run_in(&root, &["check"]);
    assert_ne!(code, 0);
    assert!(
        stderr.contains("Same") && stderr.contains("declared twice"),
        "stderr: {stderr}"
    );
}

#[test]
fn implicit_local_modules_cannot_accidentally_shadow_the_standard_library() {
    let (_guard, root) = scaffolded();
    std::fs::write(root.join("src").join("Basic.jr"), "MINE :: 1;\n").expect("write");

    let (code, _, stderr) = run_in(&root, &["check"]);
    assert_ne!(code, 0);
    assert!(
        stderr.contains("Basic") && stderr.contains("exact dependency"),
        "stderr: {stderr}"
    );
}

#[test]
fn an_exact_dependency_can_explicitly_replace_a_bundled_module() {
    let (_guard, root) = scaffolded();
    std::fs::write(root.join("replacement.jr"), "MINE :: 42;\n").expect("write");
    std::fs::write(
        root.join("jairs.toml"),
        "[project]\nname = \"demo\"\n\n\
         [dependencies]\nBasic = { path = \"replacement.jr\" }\n",
    )
    .expect("write");
    std::fs::write(
        root.join("src").join("main.jr"),
        "#import \"Basic\";\nmain :: () {\n    n := MINE;\n}\n",
    )
    .expect("write");

    let (code, _, stderr) = run_in(&root, &["check"]);
    assert_eq!(code, 0, "stderr: {stderr}");
}

#[test]
fn an_entry_the_manifest_names_but_does_not_exist_is_an_error() {
    // Rather than falling back to `src/main.jr`, which would hide the mistake in the manifest.
    let (_guard, root) = scaffolded();
    std::fs::write(
        root.join("jairs.toml"),
        "[project]\nname = \"demo\"\nentry = \"src/gone.jr\"\n",
    )
    .expect("write");

    let (code, _, stderr) = run_in(&root, &["run"]);
    assert_ne!(code, 0);
    assert!(stderr.contains("gone.jr"), "stderr: {stderr}");
    assert!(stderr.contains("does not exist"), "stderr: {stderr}");
}

#[test]
fn outside_a_project_a_path_is_still_required() {
    // The manifest is an override, never a prerequisite — and its absence must not turn into a
    // command that guesses. Both messages have to name a way forward.
    let dir = TempDir::new().expect("tempdir");

    for (args, hint) in [
        (["run"].as_slice(), "jr new"),
        (["build"].as_slice(), "jr new"),
        (["check"].as_slice(), "jr new"),
        (["fmt"].as_slice(), "jr init"),
    ] {
        let (code, _, stderr) = run_in(dir.path(), args);
        assert_ne!(code, 0, "{args:?} should have failed");
        assert!(
            stderr.contains("jairs.toml"),
            "{args:?} must name the manifest: {stderr}"
        );
        assert!(
            stderr.contains(hint),
            "{args:?} must suggest `{hint}`: {stderr}"
        );
    }
}

#[test]
fn an_explicit_path_still_works_with_no_manifest_anywhere() {
    // The compatibility assertion. Every existing invocation must behave exactly as it did, or
    // this wave broke the gates and every script anyone has written.
    let dir = TempDir::new().expect("tempdir");
    let file = dir.path().join("p.jr");
    std::fs::write(&file, "main :: () {\nn := 1;\n}\n").expect("write");

    let (code, _, stderr) = run_in(dir.path(), &["check", "p.jr"]);
    assert_eq!(code, 0, "stderr: {stderr}");

    let (code, _, stderr) = run_in(dir.path(), &["fmt", "p.jr"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(
        std::fs::read_to_string(&file)
            .expect("read")
            .contains("    n := 1;"),
        "the default is four spaces with no manifest"
    );
}

// ---------------------------------------------------------------------------
// The manifest reaches the remaining project-aware command surfaces
// ---------------------------------------------------------------------------

/// Sends one LSP session over stdio and returns every diagnostic message published for `file`.
///
/// A real subprocess in `dir`, because the server discovers its project from the initialized
/// workspace root — the whole point of the tests below.
fn lsp_diagnostics(dir: &Path, file: &Path) -> Vec<String> {
    use std::io::Write as _;

    let text = std::fs::read_to_string(file).expect("read the source");
    let uri = format!("file://{}", file.display());
    let frame = |value: &serde_json::Value| {
        let body = serde_json::to_vec(value).expect("serialise");
        let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        out.extend_from_slice(&body);
        out
    };

    let mut input = Vec::new();
    for value in [
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"processId":null,"rootUri":format!("file://{}", dir.display()),
                      "capabilities":{}}}),
        serde_json::json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        serde_json::json!({"jsonrpc":"2.0","method":"textDocument/didOpen",
            "params":{"textDocument":{"uri":uri,"languageId":"jairs","version":1,"text":text}}}),
    ] {
        input.extend_from_slice(&frame(&value));
    }

    let mut child = jr()
        .args(["lsp", "-q"])
        .current_dir(dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the jr binary must be runnable");
    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(&input)
        .expect("write the session");
    let out = child.wait_with_output().expect("the server must exit");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();

    // Split on the framing header and parse each body. Cheaper than a full LSP client, and the
    // only thing asserted on is whether a diagnostic was published at all.
    let mut messages = Vec::new();
    for chunk in stdout.split("Content-Length") {
        let Some(start) = chunk.find('{') else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&chunk[start..]) else {
            continue;
        };
        if value.get("method").and_then(|m| m.as_str()) != Some("textDocument/publishDiagnostics") {
            continue;
        }
        for diagnostic in value["params"]["diagnostics"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            if let Some(message) = diagnostic.get("message").and_then(|m| m.as_str()) {
                messages.push(message.to_owned());
            }
        }
    }
    messages
}

/// A module the manifest declares resolves **in the editor**, not only from a terminal.
///
/// This was a real defect, found by auditing which subcommands go through the one resolver rather
/// than by a failure: `jr check` resolved a `[build] module_paths` entry and `jr lsp` reported
/// E0210 on the same file. That reads as the code being wrong rather than the tool, and it is the
/// second time this exact shape appeared — `jr fmt` honoured the manifest's style while the server
/// ignored it. **A setting with two surfaces is half-wired until both read it.**
#[test]
fn the_language_server_honours_the_manifests_module_paths() {
    let (_guard, root) = scaffolded();
    std::fs::create_dir_all(root.join("vendor").join("Vend")).expect("mkdir");
    std::fs::write(
        root.join("vendor").join("Vend").join("module.jr"),
        "VEND :: 5;\n",
    )
    .expect("write");
    std::fs::write(
        root.join("jairs.toml"),
        "[project]\nname = \"demo\"\n\n[build]\nmodule_paths = [\"vendor\"]\n",
    )
    .expect("write");
    let source = root.join("src").join("main.jr");
    std::fs::write(
        &source,
        "#import \"Basic\";\n#import \"Vend\";\nmain :: () {\n    exit(VEND);\n}\n",
    )
    .expect("write");

    // The terminal half must pass, or the test proves nothing about the server.
    let (code, _, stderr) = run_in(&root, &["check"]);
    assert_eq!(code, 0, "jr check must resolve it first: {stderr}");

    let diagnostics = lsp_diagnostics(&root, &source);
    assert!(
        diagnostics.is_empty(),
        "the server must resolve a manifest-declared module, got {diagnostics:?}"
    );
}

/// The negative half: without the manifest entry the module genuinely does not resolve.
///
/// Without this, the test above passes even if the server resolved `Vend` for some unrelated
/// reason — which is exactly how a search-path bug hides.
#[test]
fn the_language_server_still_reports_a_module_that_nothing_declares() {
    let (_guard, root) = scaffolded();
    std::fs::create_dir_all(root.join("vendor").join("Vend")).expect("mkdir");
    std::fs::write(
        root.join("vendor").join("Vend").join("module.jr"),
        "VEND :: 5;\n",
    )
    .expect("write");
    // No `[build] module_paths` this time.
    std::fs::write(root.join("jairs.toml"), "[project]\nname = \"demo\"\n").expect("write");
    let source = root.join("src").join("main.jr");
    std::fs::write(
        &source,
        "#import \"Basic\";\n#import \"Vend\";\nmain :: () {\n    exit(VEND);\n}\n",
    )
    .expect("write");

    let diagnostics = lsp_diagnostics(&root, &source);
    assert!(
        diagnostics.iter().any(|m| m.contains("Vend")),
        "an undeclared module must still be reported, got {diagnostics:?}"
    );
}

/// A malformed project file is an editor diagnostic, not only stderr from a terminal command.
#[test]
fn the_language_server_publishes_a_malformed_manifest_diagnostic() {
    let (_guard, root) = scaffolded();
    std::fs::write(
        root.join("jairs.toml"),
        "[project]\nname = \"demo\"\nunknown_setting = true\n",
    )
    .expect("write");
    let source = root.join("src").join("main.jr");

    let diagnostics = lsp_diagnostics(&root, &source);
    assert!(
        diagnostics
            .iter()
            .any(|message| message.contains("not a valid manifest")
                && message.contains("unknown_setting")),
        "the manifest error must be published through LSP, got {diagnostics:?}"
    );
}

/// `jr bench` measures the module set the project actually builds with.
///
/// A benchmark taken against a different search path than the real build is a measurement of
/// something nobody runs.
#[test]
fn bench_honours_the_manifests_module_paths() {
    let (_guard, root) = scaffolded();
    std::fs::create_dir_all(root.join("vendor").join("Vend")).expect("mkdir");
    std::fs::write(
        root.join("vendor").join("Vend").join("module.jr"),
        "VEND :: 5;\n",
    )
    .expect("write");
    std::fs::write(
        root.join("jairs.toml"),
        "[project]\nname = \"demo\"\n\n[build]\nmodule_paths = [\"vendor\"]\n",
    )
    .expect("write");
    std::fs::write(
        root.join("src").join("main.jr"),
        "#import \"Basic\";\n#import \"Vend\";\nmain :: () {\n    exit(VEND);\n}\n",
    )
    .expect("write");

    // `--throughput` compiles the files it is given, so a search path it cannot resolve shows up
    // as a failure rather than as a slower number.
    let (code, stdout, stderr) = run_in(
        &root,
        &["bench", "--throughput", "src", "--iterations", "1"],
    );
    assert_eq!(code, 0, "stdout: {stdout}\nstderr: {stderr}");
}
