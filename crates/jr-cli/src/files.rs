//! Path expansion and atomic file writing utilities.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

/// Expand a list of paths (files and/or directories) into a flat list of
/// `.jr` files.
///
/// Files are included as-is (regardless of extension, so the user can
/// explicitly pass a file with any name).  Directories are walked recursively
/// and every file whose name ends in `.jr` is included.
///
/// Returns an error if any path does not exist.
pub fn expand_paths(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for p in paths {
        if !p.exists() {
            anyhow::bail!("path does not exist: {}", p.display());
        }
        if p.is_dir() {
            collect_jr_files(p, &mut out)?;
        } else {
            out.push(p.clone());
        }
    }
    Ok(out)
}

/// Recursively collect all `*.jr` files under `dir`.
fn collect_jr_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries =
        fs::read_dir(dir).with_context(|| format!("cannot read directory: {}", dir.display()))?;
    let mut entries: Vec<_> = entries
        .map(|e| e.with_context(|| format!("cannot read entry in {}", dir.display())))
        .collect::<Result<_>>()?;
    // Sort for deterministic ordering.
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_jr_files(&path, out)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("jr") {
            out.push(path);
        }
    }
    Ok(())
}

/// Read a source file, returning its text.
pub fn read_file(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("cannot read file: {}", path.display()))
}

/// Write `content` to `path` atomically.
///
/// Writes to a temporary file in the same directory as `path`, then renames
/// it over `path`.  This ensures that an interrupted write cannot leave a
/// truncated file.
pub fn write_file_atomic(path: &Path, content: &str) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)
        .with_context(|| format!("cannot create temp file in {}", dir.display()))?;
    tmp.write_all(content.as_bytes())
        .with_context(|| format!("cannot write temp file for {}", path.display()))?;
    tmp.persist(path)
        .with_context(|| format!("cannot rename temp file to {}", path.display()))?;
    Ok(())
}
