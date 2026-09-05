//! Implementation of `jr bench`: how long a language-server request actually takes.
//!
//! # Why this is a subcommand and not a `criterion` benchmark
//!
//! [ADR-0033](../../../../docs/adr/0033-latency-measurement.md) §1, and it is the whole
//! reason this file exists in this shape. A benchmark harness runs the same closure many
//! times and takes a distribution — and under salsa (ADR-0007) the *second* call to `hover`
//! on an unedited file does no work at all. It reads a memo. So a harness would report the
//! cost of a hash lookup with beautifully tight variance, and would answer ADR-0013's
//! question backwards: the invalidation cost that ADR is about is exactly what a warm cache
//! hides.
//!
//! Measuring it correctly means controlling the cache *per iteration* — a fresh database for
//! a cold number, a real edit before each after-edit one. That is a script, not a closure.
//!
//! # Why three regimes
//!
//! §2. **cold** is what the first request after opening a project pays. **warm** is the memo
//! hit, reported as a *control* — it is the number a naive benchmark would have presented as
//! the answer. **after-edit** applies a whitespace-only edit at the top of the file before
//! each iteration, which is ADR-0013 stated as an experiment: semantically nothing changed,
//! but every span below the edit moved, so every HIR node compares unequal and salsa cannot
//! backdate.
//!
//! # What it deliberately does not do
//!
//! No threshold, no assertion, no pass or fail (§4). A timing assertion on a shared machine
//! fails for reasons unrelated to the code, and this project's gates are meant to be
//! believable. `jr bench` is verified rather than gated, like `editors/nvim/verify.lua`.
//!
//! # Why compile throughput is a *mode* of this subcommand
//!
//! ADR-0146 §1. It is the same activity under the same contract — measure, report, never judge
//! — so a second subcommand would be a second place for that contract to be stated and a
//! second place for someone to add a threshold to it.
//!
//! It is the opposite *shape*, though, and the docs above are why: latency is one file in three
//! cache regimes, and throughput is one cold pass over many files. There is no warm throughput
//! at all, because a compiler run is a process and the second run starts with an empty database
//! by construction — so a warm figure here would measure a memo table, which is exactly the
//! misleading answer §1 above was written to avoid.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use jr_db::{Db as _, JairsDatabase, ModuleSearchPaths, SourceFile};
use jr_lsp::Encoding;

use crate::cli::{BenchArgs, GlobalArgs};
use crate::commands::check::bundled_module_dir;

/// Which cache state an operation was measured in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Regime {
    /// A fresh database per iteration: nothing memoized.
    Cold,
    /// The same database, unedited: every query is a memo hit.
    Warm,
    /// The same database, with a whitespace-only edit applied first.
    AfterEdit,
}

impl Regime {
    fn label(self) -> &'static str {
        match self {
            Regime::Cold => "cold",
            Regime::Warm => "warm",
            Regime::AfterEdit => "after-edit",
        }
    }
}

/// One operation's timings in one regime.
struct Row {
    operation: &'static str,
    regime: Regime,
    /// Every sample, so the summary can be computed without assuming a distribution.
    samples: Vec<Duration>,
}

impl Row {
    /// The smallest sample: the closest thing to the cost with no interference.
    fn min(&self) -> Duration {
        self.samples.iter().copied().min().unwrap_or_default()
    }

    /// The median, which is what a user experiences most of the time.
    fn median(&self) -> Duration {
        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        sorted.get(sorted.len() / 2).copied().unwrap_or_default()
    }

    /// The 95th percentile, because a tail an editor hits is a tail a user notices.
    ///
    /// Computed by rank rather than by interpolation: with the iteration counts this
    /// subcommand uses, interpolating between two samples would imply a precision the
    /// sample size does not support.
    fn p95(&self) -> Duration {
        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        if sorted.is_empty() {
            return Duration::ZERO;
        }
        let rank = ((sorted.len() as f64) * 0.95).ceil() as usize;
        let index = rank.saturating_sub(1).min(sorted.len() - 1);
        sorted[index]
    }
}

/// Run `jr bench`.
///
/// Exits 0 whenever the measurement completed. There is no threshold to fail (ADR-0033 §4);
/// a non-zero exit here would mean the harness itself could not run.
pub fn run(args: BenchArgs, global: &GlobalArgs) -> Result<i32> {
    if args.throughput {
        return throughput(&args, global);
    }
    let path = args
        .file
        .canonicalize()
        .unwrap_or_else(|_| args.file.clone());
    let text = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;

    let mut search_paths = args.module_paths.clone();
    search_paths.push(bundled_module_dir());

    let at = cursor(&text);
    let iterations = args.iterations.max(1);
    let mut rows = Vec::new();

    // Cold: a fresh database every iteration. This is the only honest way to measure a
    // salsa query, and the reason this is not a `criterion` bench.
    for (operation, run_one) in operations() {
        let mut samples = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            let (db, file, input) = fresh(&path, &text, &search_paths)?;
            let start = Instant::now();
            run_one(&db, file, input, at);
            samples.push(start.elapsed());
        }
        rows.push(Row {
            operation,
            regime: Regime::Cold,
            samples,
        });
    }

    // Warm and after-edit share one database, built once.
    let (mut db, file, input) = fresh(&path, &text, &search_paths)?;
    for (operation, run_one) in operations() {
        // Prime the memo, so the first warm sample is not a cold one in disguise.
        run_one(&db, file, input, at);
        let mut samples = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            let start = Instant::now();
            run_one(&db, file, input, at);
            samples.push(start.elapsed());
        }
        rows.push(Row {
            operation,
            regime: Regime::Warm,
            samples,
        });
    }

    for (operation, run_one) in operations() {
        let mut samples = Vec::with_capacity(iterations);
        for index in 0..iterations {
            // A whitespace-only edit at the *top* of the file: semantically empty, and it
            // moves every span below it. ADR-0013's question in experimental form.
            //
            // A different number of leading newlines each time, so the text is genuinely
            // new on every iteration — re-setting identical text would be backdated by
            // salsa and would measure nothing.
            let edited = format!("{}{text}", "\n".repeat(index + 1));
            db.set_file_text(path.to_string_lossy().into_owned(), edited);
            let start = Instant::now();
            run_one(&db, file, input, at);
            samples.push(start.elapsed());
        }
        rows.push(Row {
            operation,
            regime: Regime::AfterEdit,
            samples,
        });
    }

    // Workspace discovery, cold only: the number ADR-0029 §3 promised the first caller
    // would pay, and warming it would measure a database the scan has already populated.
    let mut samples = Vec::with_capacity(iterations);
    let mut discovered = 0usize;
    for _ in 0..iterations {
        let mut db = JairsDatabase::default();
        let _ = db.set_module_search_paths(search_paths.clone());
        let roots: Vec<PathBuf> = search_paths
            .iter()
            .cloned()
            .chain(path.parent().map(Path::to_path_buf))
            .collect();
        db.set_workspace_roots(&roots);
        let start = Instant::now();
        discovered = db.load_workspace_files();
        samples.push(start.elapsed());
    }
    rows.push(Row {
        operation: "workspace_load",
        regime: Regime::Cold,
        samples,
    });

    report(&rows, iterations, &path, discovered, at, global.quiet);
    Ok(0)
}

/// The operations measured, each a call into `jr-lsp`'s pure handlers.
///
/// Handlers rather than the transport, deliberately: ADR-0024 §4 made them pure functions of
/// `(&db, params)` so they could be called without a client, and measuring over stdio would
/// time framing and thread scheduling alongside the query work under investigation
/// (ADR-0033 §1).
type Operation = (
    &'static str,
    fn(&JairsDatabase, SourceFile, ModuleSearchPaths, lsp_types::Position),
);

fn operations() -> Vec<Operation> {
    vec![
        ("diagnostics", |db, file, input, _at| {
            let _ = jr_lsp::diagnostics(db, file, input, Encoding::Utf8);
        }),
        ("hover", |db, file, input, at| {
            let _ = jr_lsp::hover(db, file, input, Encoding::Utf8, at);
        }),
        ("completion", |db, file, input, at| {
            // **With the workspace input**, so the number covers the unimported-symbol source
            // (ADR-0199 §7). Passing `None` here would measure the in-scope half alone and report
            // a latency the real server never has — the cost ADR-0033 §3 declined to guess at is
            // precisely this one, so measuring the cheap path would answer the wrong question.
            let _ = jr_lsp::completion(db, file, input, Encoding::Utf8, at, db.workspace_files());
        }),
        ("code_action", |db, file, input, at| {
            // Given the file's own diagnostics, which is what a client sends. An empty list
            // would measure a code-action request with nothing to offer — the cheap case,
            // and not the one ADR-0031 §5 is about.
            let diagnostics = jr_lsp::diagnostics(db, file, input, Encoding::Utf8);
            let workspace = db
                .workspace_files()
                .map(|files| files.list(db))
                .unwrap_or_default();
            let range = lsp_types::Range { start: at, end: at };
            let _ = jr_lsp::code_actions(
                db,
                file,
                input,
                Encoding::Utf8,
                range,
                &diagnostics,
                &workspace,
            );
        }),
        // The two rows below are **not** requests any client sends. They are the split that
        // turns "references is slow" into "parsing is slow" — see ADR-0034 §3, which is the
        // decision they produced. A benchmark that measures only end-user operations can say
        // *that* something is slow and never *what*.
        ("parse_all_files", |db, _file, _input, _at| {
            // Lex and parse every workspace file, nothing else. Measured at 31 ms of
            // `references`'s 55 ms on a 302-file tree: over half the cost is syntax, which no
            // index and no traversal change can touch.
            let workspace = db
                .workspace_files()
                .map(|files| files.list(db))
                .unwrap_or_default();
            for path in workspace.files.iter() {
                if let Some(source) = db.source_file_for_path(path.to_string_lossy().as_ref()) {
                    let _ = jr_db::parse_file(db, source);
                }
            }
        }),
        ("resolve_all_files", |db, _file, input, _at| {
            // The scan's whole *substrate*: the above plus lowering and resolution, with no
            // matching done. Measured at 55 ms — indistinguishable from `references` itself,
            // which is what proved the traversal costs ~1% and killed the reverse index
            // ADR-0030 had reserved and `PLAN.md` §7 had already promoted to "build it"
            // (ADR-0034 §1).
            let workspace = db
                .workspace_files()
                .map(|files| files.list(db))
                .unwrap_or_default();
            for path in workspace.files.iter() {
                if let Some(source) = db.source_file_for_path(path.to_string_lossy().as_ref()) {
                    let _ = jr_db::file_hir(db, source);
                    let _ = jr_db::resolved(db, source, input);
                }
            }
        }),
        ("references", |db, file, input, at| {
            let workspace = db
                .workspace_files()
                .map(|files| files.list(db))
                .unwrap_or_default();
            let _ = jr_lsp::find_references(
                db,
                file,
                input,
                Encoding::Utf8,
                at,
                true,
                &workspace.files,
            );
        }),
        ("rename", |db, file, input, at| {
            let workspace = db
                .workspace_files()
                .map(|files| files.list(db))
                .unwrap_or_default();
            // The result is deliberately discarded, refusal included: a refusal still costs
            // the whole workspace scan, which is the thing being timed.
            let _ = jr_lsp::rename(
                db,
                file,
                input,
                Encoding::Utf8,
                at,
                "renamed_by_bench",
                workspace.as_ref(),
            );
        }),
    ]
}

/// The position every operation is measured at: a top-level declaration's own name.
///
/// **Derived from the file, not hardcoded.** The first draft used a fixed line 20, column 8,
/// which in `024-hello.jr` lands inside the `return` keyword — so `references` and `rename`
/// took their "nothing here" early return and reported **0.002 ms**, a workspace scan that
/// never happened. The module docs of this file had warned about exactly that failure mode
/// one paragraph before committing it.
///
/// A declaration's name is the right choice for a reason beyond convenience: `references`
/// and `rename` are keyed on a `DefId` (ADR-0030 §1), and a declaration site is the position
/// that yields one for a name used elsewhere in the file — which is what makes the scan do
/// its work rather than refuse.
///
/// Falls back to the file's first non-blank, non-comment line if nothing matches, and the
/// caller reports which position was used so a suspiciously cheap row can be checked rather
/// than believed.
/// The benchmark cursor, exposed so a test can assert *where* the measurement happens.
///
/// Public because the position decides whether half the table means anything, and an
/// integration test cannot reach a private function. Named `_for_test` so it reads as what it
/// is rather than as API someone should build on.
#[must_use]
pub fn cursor_for_test(text: &str) -> lsp_types::Position {
    cursor(text)
}

fn cursor(text: &str) -> lsp_types::Position {
    for (index, line) in text.lines().enumerate() {
        // A top-level declaration: `name ::` or `name :=` at column 0. Column 0 matters —
        // an indented `::` is inside a body and may be a local rather than an item.
        if line.starts_with(char::is_alphabetic) && (line.contains(" :: ") || line.contains(" := "))
        {
            return lsp_types::Position {
                line: u32::try_from(index).unwrap_or(0),
                character: 0,
            };
        }
    }
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if !trimmed.is_empty() && !trimmed.starts_with("//") {
            return lsp_types::Position {
                line: u32::try_from(index).unwrap_or(0),
                character: 0,
            };
        }
    }
    lsp_types::Position {
        line: 0,
        character: 0,
    }
}

/// A database with the file loaded, its modules loaded, and its workspace discovered.
///
/// Everything a language server would have done before the first request arrives, so that a
/// cold measurement times the *request* rather than the setup.
fn fresh(
    path: &Path,
    text: &str,
    search_paths: &[PathBuf],
) -> Result<(JairsDatabase, SourceFile, ModuleSearchPaths)> {
    let mut db = JairsDatabase::default();
    let input = db.set_module_search_paths(search_paths.to_vec());
    let key = path.to_string_lossy().into_owned();
    let _ = db.set_file_text(key.clone(), text.to_owned());
    let file = db
        .source_file(&key)
        .ok_or_else(|| anyhow::anyhow!("internal error: {key} was not registered"))?;
    db.load_modules_transitively(file);

    let roots: Vec<PathBuf> = search_paths
        .iter()
        .cloned()
        .chain(path.parent().map(Path::to_path_buf))
        .collect();
    db.set_workspace_roots(&roots);
    db.load_workspace_files();

    Ok((db, file, input))
}

/// Prints the table.
///
/// Plain text on stdout, because the audience is a person deciding whether to build an index
/// (ADR-0033) and not a machine. `--quiet` suppresses the preamble, not the numbers: a
/// measurement subcommand that printed nothing would have no purpose.
fn report(
    rows: &[Row],
    iterations: usize,
    path: &Path,
    discovered: usize,
    at: lsp_types::Position,
    quiet: bool,
) {
    if !quiet {
        // The cursor is printed because it decides whether half these rows mean anything:
        // a position on a keyword makes `references` and `rename` return immediately, and
        // the resulting sub-microsecond row looks like a fast scan rather than no scan.
        println!(
            "{} — {iterations} iterations, {discovered} files discovered, cursor at {}:{}",
            path.display(),
            at.line,
            at.character
        );
        println!(
            "\n{:<16} {:<11} {:>10} {:>10} {:>10}",
            "operation", "regime", "min", "median", "p95"
        );
    }
    for row in rows {
        println!(
            "{:<16} {:<11} {:>10} {:>10} {:>10}",
            row.operation,
            row.regime.label(),
            millis(row.min()),
            millis(row.median()),
            millis(row.p95()),
        );
    }
    if !quiet {
        println!(
            "\nNo thresholds: this reports, it does not judge (ADR-0033 §4).\n\
             `warm` is the control — it is what a `criterion` benchmark would have measured,\n\
             because a memoized query does no work the second time it is asked."
        );
    }
}

/// A duration as milliseconds with three decimals.
///
/// Milliseconds throughout rather than a unit chosen per row, because the point of the table
/// is comparing rows to each other and a column with mixed units defeats that.
fn millis(duration: Duration) -> String {
    format!("{:.3}ms", duration.as_secs_f64() * 1000.0)
}

// ---------------------------------------------------------------------------
// Compile throughput (ADR-0146)
// ---------------------------------------------------------------------------

/// One operation's throughput over a file set.
struct Throughput {
    /// What was measured: `check` or `build`.
    operation: &'static str,
    /// Every sample, one per iteration over the whole set.
    samples: Vec<Duration>,
    /// How many files were compiled, so a reader can see the set was not empty.
    files: usize,
    /// How many source lines, and how many bytes — see ADR-0146 §1 on why both.
    lines: usize,
    /// Total bytes of source.
    bytes: usize,
}

impl Throughput {
    /// The fastest pass over the set: the closest thing to the cost with no interference.
    ///
    /// The *minimum* rather than the median, unlike the latency table, and for a different
    /// reason than taste: a throughput figure is quoted as a capability ("this compiler does
    /// N lines a second"), so the honest version of that claim is the best the machine
    /// managed rather than the middle of a distribution that includes other processes.
    fn best(&self) -> Duration {
        self.samples.iter().copied().min().unwrap_or_default()
    }

    /// Lines per second at [`Self::best`].
    fn lines_per_second(&self) -> f64 {
        let seconds = self.best().as_secs_f64();
        if seconds <= 0.0 {
            return 0.0;
        }
        self.lines as f64 / seconds
    }

    /// Bytes per second at [`Self::best`].
    fn bytes_per_second(&self) -> f64 {
        let seconds = self.best().as_secs_f64();
        if seconds <= 0.0 {
            return 0.0;
        }
        self.bytes as f64 / seconds
    }
}

/// Measures compile throughput over `args`'s paths (ADR-0146 §1).
///
/// # Errors
/// When a path cannot be read or expands to no `.jr` file at all — the second being an error
/// rather than a zero, because a throughput number over nothing is the most misleading output
/// this subcommand could produce.
fn throughput(args: &BenchArgs, global: &GlobalArgs) -> Result<i32> {
    let mut roots = vec![args.file.clone()];
    roots.extend(args.paths.iter().cloned());
    let files = crate::files::expand_paths(&roots)?;
    if files.is_empty() {
        anyhow::bail!("no `.jr` files to measure");
    }

    let mut sources = Vec::with_capacity(files.len());
    let mut lines = 0usize;
    let mut bytes = 0usize;
    for path in &files {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;
        lines += text.lines().count();
        bytes += text.len();
        sources.push((path.clone(), text));
    }

    let mut search_paths = args.module_paths.clone();
    search_paths.push(bundled_module_dir());

    let iterations = args.iterations.max(1);
    let mut rows = Vec::new();

    // **A fresh database per iteration, for both operations.** Not an optimisation to skip:
    // reusing one would make every iteration after the first a memo hit, which is the
    // measurement ADR-0033 §1 exists to refuse.
    for (operation, compile) in [
        (
            "check",
            check_all as fn(&mut JairsDatabase, &[(PathBuf, String)], ModuleSearchPaths),
        ),
        ("build", build_all),
    ] {
        let mut samples = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            let mut db = JairsDatabase::default();
            let input = db.set_module_search_paths(search_paths.clone());
            // Registering the text is *setup*, not compilation: a real command reads its
            // files before it starts, so the clock starts after.
            for (path, text) in &sources {
                let _ = db.set_file_text(path.to_string_lossy().into_owned(), text.clone());
            }
            let start = Instant::now();
            compile(&mut db, &sources, input);
            samples.push(start.elapsed());
        }
        rows.push(Throughput {
            operation,
            samples,
            files: files.len(),
            lines,
            bytes,
        });
    }

    report_throughput(&rows, iterations, global.quiet);
    Ok(0)
}

/// Every diagnostic for every file, which is `jr check`'s work (ADR-0146 §1).
fn check_all(db: &mut JairsDatabase, sources: &[(PathBuf, String)], input: ModuleSearchPaths) {
    for (path, _) in sources {
        let Some(file) = db.source_file(&path.to_string_lossy()) else {
            continue;
        };
        db.load_modules_transitively(file);
        let _ = jr_db::file_diagnostics(db, file, input);
    }
}

/// Through MIR and a back end into an object, which is `jr build`'s work minus the link.
///
/// The link is excluded because it is `cc` rather than this compiler (ADR-0146 §1). A file
/// with no `main` contributes its *check* cost and no object, which is honest: `build_object`
/// refuses it, and skipping the refusal would be measuring a different program.
fn build_all(db: &mut JairsDatabase, sources: &[(PathBuf, String)], input: ModuleSearchPaths) {
    let config = db.build_config();
    for (path, _) in sources {
        let Some(file) = db.source_file(&path.to_string_lossy()) else {
            continue;
        };
        db.load_modules_transitively(file);
        let _ = jr_db::file_diagnostics(db, file, input);
        // `Required`: the benchmark measures compiling a *program*, which is what the corpus contains.
        let _ = jr_db::build_object(
            db,
            file,
            input,
            config,
            jr_db::BackendChoice::Cranelift,
            jr_db::EntryPolicy::Required,
        );
    }
}

/// Prints the throughput table.
fn report_throughput(rows: &[Throughput], iterations: usize, quiet: bool) {
    if let Some(first) = rows.first()
        && !quiet
    {
        println!(
            "compile throughput — {} files, {} lines, {} bytes, {iterations} iterations",
            first.files, first.lines, first.bytes
        );
        println!(
            "\n{:<10} {:>12} {:>16} {:>16}",
            "operation", "best", "lines/s", "bytes/s"
        );
    }
    for row in rows {
        println!(
            "{:<10} {:>12} {:>16.0} {:>16.0}",
            row.operation,
            millis(row.best()),
            row.lines_per_second(),
            row.bytes_per_second(),
        );
    }
    if !quiet {
        println!(
            "\nNo thresholds: this reports, it does not judge (ADR-0033 §4, ADR-0146 §2).\n\
             Cold only — a compiler is a process, so there is no warm throughput to report.\n\
             `build` excludes the link, which is `cc` rather than this compiler."
        );
    }
}
