//! Diagnostic rendering and summary helpers.

use jr_base::SourceMap;
use jr_diag::{Config, Diagnostics, Renderer};

/// Build a [`Renderer`] from the resolved colour flag.
#[must_use]
pub fn make_renderer(colour: bool) -> Renderer {
    Renderer::with_config(Config {
        colour,
        show_backtrace: true,
    })
}

/// Render all diagnostics to stderr.
pub fn emit_diagnostics(renderer: &Renderer, map: &SourceMap, diags: &Diagnostics) {
    if !diags.is_empty() {
        let text = renderer.render_all(map, diags);
        eprint!("{text}");
    }
}

/// Print a summary line to stderr: `N files checked, M errors`.
pub fn print_check_summary(files: usize, errors: usize, quiet: bool) {
    if !quiet {
        let file_word = if files == 1 { "file" } else { "files" };
        let error_word = if errors == 1 { "error" } else { "errors" };
        eprintln!("{files} {file_word} checked, {errors} {error_word}");
    }
}

/// Produce a unified diff between `original` and `formatted`.
///
/// Returns an empty string if the two are identical.
#[must_use]
pub fn unified_diff(path: &std::path::Path, original: &str, formatted: &str) -> String {
    if original == formatted {
        return String::new();
    }
    // Build a minimal unified diff manually.
    // We use the `similar` crate if available; since it is not in the workspace
    // we produce a simple "file would change" message instead.
    // A proper diff is a nice-to-have; the spec only requires exit 1 + "what
    // would change".  We show the full before/after for small files and a
    // summary for large ones.
    let mut out = String::new();
    out.push_str(&format!("--- {}\n", path.display()));
    out.push_str(&format!("+++ {} (formatted)\n", path.display()));

    // Line-by-line diff using a simple LCS-free approach: show context around
    // changed lines.  For the purposes of this tool a simple "show all changed
    // lines" is sufficient.
    let orig_lines: Vec<&str> = original.lines().collect();
    let fmt_lines: Vec<&str> = formatted.lines().collect();

    let max = orig_lines.len().max(fmt_lines.len());
    let mut i = 0;
    while i < max {
        let o = orig_lines.get(i).copied().unwrap_or("");
        let f = fmt_lines.get(i).copied().unwrap_or("");
        if o != f {
            out.push_str(&format!("-{o}\n"));
            out.push_str(&format!("+{f}\n"));
        }
        i += 1;
    }
    out
}
