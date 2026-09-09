//! Executable compatibility probes for the pinned Jai guide.
//!
//! ADR-0219 keeps the upstream checkout as provenance only. This test reads one
//! strict manifest and executes only repository-owned Jairs files, each from a
//! fresh temporary directory.

use std::{
    collections::BTreeSet,
    ffi::OsStr,
    fs,
    path::{Component, Path, PathBuf},
    process::{Command, Output},
};

use libtest_mimic::{Arguments, Failed, Trial};
use serde::Deserialize;
use tempfile::TempDir;

const SCHEMA: u32 = 1;
const UPSTREAM_REVISION: &str = "19cb4b7acb0de2798c769f9ad73313a4d15f4056";
const CHAPTERS: &[&str] = &[
    "03", "04", "05", "06", "07", "08", "09", "10", "11", "12", "13", "14", "15", "16", "17", "18",
    "19", "20", "21", "22", "23", "24", "25", "26", "27", "28", "29", "30", "31", "33", "34", "35",
    "50", "51", "52",
];

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    upstream: Upstream,
    probe: Vec<Probe>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Upstream {
    repository: String,
    revision: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Probe {
    id: String,
    chapter: String,
    upstream_path: PathBuf,
    jairs_path: PathBuf,
    status: Status,
    classification: Classification,
    reason: String,
    mode: Mode,
    formatted: bool,
    #[serde(default)]
    artifact: Option<Artifact>,
    expect: Expectation,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Status {
    SourceCompatible,
    Ported,
    Blocked,
    Divergent,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Classification {
    Lexical,
    Syntactic,
    Semantic,
    Library,
    Toolchain,
    Platform,
    Runtime,
    IntentionalDivergence,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Mode {
    Check,
    Run,
    Build,
    BuildRun,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Expectation {
    exit: i32,
    #[serde(default)]
    stdout: Option<String>,
    #[serde(default)]
    stderr: Option<String>,
    #[serde(default)]
    stdout_contains: Vec<String>,
    #[serde(default)]
    stderr_contains: Vec<String>,
    #[serde(default)]
    codes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Artifact {
    path: PathBuf,
    expect: Expectation,
}

fn main() {
    let mut args = Arguments::from_args();
    if args.test_threads.is_none() {
        args.test_threads = Some(4);
    }
    let root = workspace_root();
    let loaded = load_manifest(&root);

    let trials = match loaded {
        Ok(manifest) => {
            let mut trials = vec![Trial::test("manifest", || Ok(()))];
            trials.extend(manifest.probe.into_iter().map(|probe| {
                let name = format!("{}::{}", probe.chapter, probe.id);
                let root = root.clone();
                Trial::test(name, move || run_probe(&root, &probe))
            }));
            trials
        }
        Err(message) => vec![Trial::test("manifest", move || {
            Err(Failed::from(message.clone()))
        })],
    };

    libtest_mimic::run(&args, trials).exit();
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn load_manifest(root: &Path) -> Result<Manifest, String> {
    let path = root.join("tests/compatibility/probes.toml");
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let manifest: Manifest =
        toml::from_str(&text).map_err(|error| format!("invalid {}: {error}", path.display()))?;
    validate_manifest(root, &manifest)?;
    Ok(manifest)
}

fn validate_manifest(root: &Path, manifest: &Manifest) -> Result<(), String> {
    if manifest.schema != SCHEMA {
        return Err(format!(
            "manifest schema is {}, expected {SCHEMA}",
            manifest.schema
        ));
    }
    if manifest.upstream.repository != "https://github.com/Ivo-Balbaert/The_Way_to_Jai.git" {
        return Err(format!(
            "unexpected upstream repository {:?}",
            manifest.upstream.repository
        ));
    }
    if manifest.upstream.revision != UPSTREAM_REVISION {
        return Err(format!(
            "upstream revision is {}, expected {UPSTREAM_REVISION}",
            manifest.upstream.revision
        ));
    }

    let expected: BTreeSet<&str> = CHAPTERS.iter().copied().collect();
    let actual: BTreeSet<&str> = manifest
        .probe
        .iter()
        .map(|probe| probe.chapter.as_str())
        .collect();
    if actual != expected {
        return Err(format!(
            "manifest must cover every approved chapter and no others; expected {expected:?}, got {actual:?} across {} rows",
            manifest.probe.len()
        ));
    }

    let mut ids = BTreeSet::new();
    let mut upstream_paths = BTreeSet::new();
    let mut jairs_paths = BTreeSet::new();
    for probe in &manifest.probe {
        if !ids.insert(&probe.id) {
            return Err(format!("duplicate probe id {:?}", probe.id));
        }
        if !upstream_paths.insert(&probe.upstream_path) {
            return Err(format!(
                "duplicate upstream path {}",
                probe.upstream_path.display()
            ));
        }
        if !jairs_paths.insert(&probe.jairs_path) {
            return Err(format!(
                "duplicate Jairs path {}",
                probe.jairs_path.display()
            ));
        }
        if probe.reason.trim().is_empty() {
            return Err(format!("probe {} has an empty reason", probe.id));
        }

        validate_relative(&probe.upstream_path, &probe.id)?;
        validate_relative(&probe.jairs_path, &probe.id)?;
        require_prefix(
            &probe.upstream_path,
            &["examples", &probe.chapter],
            &probe.id,
        )?;
        require_prefix(&probe.jairs_path, &["probes", &probe.chapter], &probe.id)?;

        let owned = root.join("tests/compatibility").join(&probe.jairs_path);
        if !owned.is_file() {
            return Err(format!(
                "probe {} names missing owned source {}",
                probe.id,
                owned.display()
            ));
        }

        validate_codes(&probe.id, &probe.expect)?;

        match probe.status {
            Status::Blocked | Status::Divergent
                if matches!(probe.mode, Mode::Check) && probe.expect.codes.is_empty() =>
            {
                return Err(format!(
                    "refusal probe {} must pin at least one diagnostic code",
                    probe.id
                ));
            }
            Status::SourceCompatible | Status::Ported
                if matches!(probe.mode, Mode::Check) && probe.expect.exit != 0 =>
            {
                return Err(format!(
                    "working check probe {} must expect exit 0",
                    probe.id
                ));
            }
            _ => {}
        }

        if !probe.formatted && !matches!(probe.classification, Classification::Syntactic) {
            return Err(format!(
                "only a syntactic refusal may opt out of formatting: {}",
                probe.id
            ));
        }
        if let Some(artifact) = &probe.artifact {
            if !matches!(probe.mode, Mode::Build) {
                return Err(format!(
                    "only a build probe may name a post-build artifact: {}",
                    probe.id
                ));
            }
            validate_relative(&artifact.path, &probe.id)?;
            validate_codes(&probe.id, &artifact.expect)?;
        }
    }
    Ok(())
}

fn validate_relative(path: &Path, id: &str) -> Result<(), String> {
    if path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(format!(
            "probe {id} path must be relative and contain no `..`: {}",
            path.display()
        ));
    }
    Ok(())
}

fn require_prefix(path: &Path, prefix: &[&str], id: &str) -> Result<(), String> {
    let actual: Vec<&OsStr> = path.iter().take(prefix.len()).collect();
    let expected: Vec<&OsStr> = prefix.iter().map(OsStr::new).collect();
    if actual != expected {
        return Err(format!(
            "probe {id} path {} must begin with {}",
            path.display(),
            prefix.join("/")
        ));
    }
    Ok(())
}

fn is_diagnostic_code(code: &str) -> bool {
    code.len() == 5 && code.starts_with('E') && code.as_bytes()[1..].iter().all(u8::is_ascii_digit)
}

fn validate_codes(id: &str, expectation: &Expectation) -> Result<(), String> {
    for code in &expectation.codes {
        if !is_diagnostic_code(code) {
            return Err(format!("probe {id} has invalid diagnostic code {code:?}"));
        }
    }
    let mut unique_codes = BTreeSet::new();
    if !expectation
        .codes
        .iter()
        .all(|code| unique_codes.insert(code))
    {
        return Err(format!("probe {id} repeats a diagnostic code"));
    }
    Ok(())
}

fn run_probe(root: &Path, probe: &Probe) -> Result<(), Failed> {
    let compatibility = root.join("tests/compatibility");
    let owned = compatibility.join(&probe.jairs_path);
    let source_dir = owned
        .parent()
        .ok_or_else(|| Failed::from(format!("{} has no parent", owned.display())))?;
    let source_name = owned
        .file_name()
        .ok_or_else(|| Failed::from(format!("{} has no file name", owned.display())))?;

    let temp = TempDir::new().map_err(failed)?;
    let case_dir = temp.path().join("case");
    copy_dir(source_dir, &case_dir).map_err(failed)?;
    let source = PathBuf::from(source_name);

    if probe.formatted {
        let output = jr(&case_dir, ["fmt", "--check", source.to_str().unwrap()])?;
        assert_status(&probe.id, "format", &output, 0)?;
    }

    let output = match probe.mode {
        Mode::Check => jr(&case_dir, ["check", source.to_str().unwrap()])?,
        Mode::Run => jr(&case_dir, ["run", source.to_str().unwrap()])?,
        Mode::Build => jr(&case_dir, ["build", source.to_str().unwrap()])?,
        Mode::BuildRun => {
            let artifact = case_dir.join("compat-probe-bin");
            let build = jr(
                &case_dir,
                [
                    "build",
                    source.to_str().unwrap(),
                    "-o",
                    artifact.to_str().unwrap(),
                ],
            )?;
            assert_status(&probe.id, "build", &build, 0)?;
            if !build.stdout.is_empty() || !build.stderr.is_empty() {
                return Err(Failed::from(format!(
                    "{} build was not quiet:\n{}",
                    probe.id,
                    render_output(&build)
                )));
            }
            Command::new(&artifact)
                .current_dir(&case_dir)
                .output()
                .map_err(failed)?
        }
    };

    assert_expectation(&probe.id, &probe.expect, &output)?;
    if let Some(artifact) = &probe.artifact {
        let output = Command::new(case_dir.join(&artifact.path))
            .current_dir(&case_dir)
            .output()
            .map_err(failed)?;
        assert_expectation(
            &format!("{} post-build artifact", probe.id),
            &artifact.expect,
            &output,
        )?;
    }
    Ok(())
}

fn jr<const N: usize>(cwd: &Path, args: [&str; N]) -> Result<Output, Failed> {
    Command::new(env!("CARGO_BIN_EXE_jr"))
        .current_dir(cwd)
        .args(["--color", "never", "--quiet"])
        .args(args)
        .output()
        .map_err(failed)
}

fn assert_expectation(id: &str, expectation: &Expectation, output: &Output) -> Result<(), Failed> {
    assert_status(id, "command", output, expectation.exit)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if let Some(expected) = &expectation.stdout
        && stdout.as_ref() != expected
    {
        return Err(Failed::from(format!(
            "{id} stdout differed:\nexpected {expected:?}\nactual   {stdout:?}"
        )));
    }
    if let Some(expected) = &expectation.stderr
        && stderr.as_ref() != expected
    {
        return Err(Failed::from(format!(
            "{id} stderr differed:\nexpected {expected:?}\nactual   {stderr:?}"
        )));
    }
    for needle in &expectation.stdout_contains {
        if !stdout.contains(needle) {
            return Err(Failed::from(format!(
                "{id} stdout did not contain {needle:?}: {stdout:?}"
            )));
        }
    }
    for needle in &expectation.stderr_contains {
        if !stderr.contains(needle) {
            return Err(Failed::from(format!(
                "{id} stderr did not contain {needle:?}: {stderr:?}"
            )));
        }
    }

    if !expectation.codes.is_empty() {
        let actual = diagnostic_codes(&stderr);
        let expected: BTreeSet<String> = expectation.codes.iter().cloned().collect();
        if actual != expected {
            return Err(Failed::from(format!(
                "{id} diagnostic codes differed: expected {expected:?}, got {actual:?}\nstderr:\n{stderr}"
            )));
        }
    }
    Ok(())
}

fn assert_status(id: &str, operation: &str, output: &Output, expected: i32) -> Result<(), Failed> {
    let actual = output.status.code();
    if actual != Some(expected) {
        return Err(Failed::from(format!(
            "{id} {operation} exited {actual:?}, expected {expected}:\n{}",
            render_output(output)
        )));
    }
    Ok(())
}

fn diagnostic_codes(stderr: &str) -> BTreeSet<String> {
    let mut codes = BTreeSet::new();
    for line in stderr.lines() {
        let Some(rest) = line.strip_prefix("error[") else {
            continue;
        };
        let Some((code, _)) = rest.split_once("]:") else {
            continue;
        };
        if is_diagnostic_code(code) {
            codes.insert(code.to_owned());
        }
    }
    codes
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let destination = to.join(entry.file_name());
        if ty.is_dir() {
            copy_dir(&entry.path(), &destination)?;
        } else if ty.is_file() {
            fs::copy(entry.path(), destination)?;
        } else {
            return Err(std::io::Error::other(format!(
                "probe fixture {} is not a regular file or directory",
                entry.path().display()
            )));
        }
    }
    Ok(())
}

fn render_output(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn failed(error: impl std::fmt::Display) -> Failed {
    Failed::from(error.to_string())
}
