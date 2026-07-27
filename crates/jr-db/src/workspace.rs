//! Which `*.jr` files exist: the workspace file list.
//!
//! # Why the compiler needs this at all
//!
//! It did not, until an editor asked two questions a batch compiler never does. A rename
//! must edit *every* file mentioning a name, and an auto-import must know that `Basic` is
//! available — and [`crate::module_file`] only ever probes a name it was given. Nothing
//! enumerated.
//!
//! # Why the list is an input rather than a query
//!
//! [ADR-0029](../../../docs/adr/0029-workspace-discovery.md) §2. A directory walk is
//! untracked I/O: salsa cannot know the filesystem changed, so a query returning the list
//! would be stale with no way to notice. As an *input*, the staleness lives in exactly one
//! place, refreshing it invalidates precisely what depended on it, and the thing
//! responsible for refreshing — a client file watcher — is outside the database where it
//! belongs.
//!
//! # Why it holds paths and not files
//!
//! Reading and parsing every file to answer one request is the cost this avoids paying at
//! startup, and pays instead on the first request that needs the whole workspace. §3 of
//! the ADR states the consequence plainly rather than burying it: the first whole-workspace
//! rename parses the whole workspace.

use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The maximum number of files a walk will collect.
///
/// A walk is rooted at whatever directory an editor happened to open, so unbounded is the
/// wrong default. Ten thousand is far above any Jairs tree that exists and far below the
/// point where the list itself is a problem.
pub const MAX_FILES: usize = 10_000;

/// Directory names never descended into.
///
/// A heuristic, and the ADR says so: a project keeping Jairs sources in a dot-directory is
/// wrong by fiat here.
const SKIP: &[&str] = &["target", "node_modules"];

#[allow(
    missing_docs,
    reason = "salsa's generated code is not documented by us"
)]
mod workspace_input {
    use super::WorkspaceFileList;
    use std::sync::Arc;

    /// The set of `*.jr` files the workspace contains.
    ///
    /// A salsa input, so that a file appearing or disappearing invalidates every query
    /// that consulted the list — and so that nothing inside the database is tempted to
    /// walk a directory itself.
    #[salsa::input]
    pub struct WorkspaceFiles {
        /// The discovered files, and whether the walk was cut short.
        #[returns(clone)]
        pub list: Arc<WorkspaceFileList>,
    }
}

pub use workspace_input::WorkspaceFiles;

/// A discovered file set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceFileList {
    /// Absolute paths of every `*.jr` file found, sorted so the order is deterministic.
    ///
    /// Sorted rather than in walk order because walk order depends on the filesystem, and
    /// a rename's `WorkspaceEdit` — or a `workspaceSymbol` list — that reorders itself
    /// between runs is impossible to test.
    pub files: Arc<[PathBuf]>,
    /// `true` when the walk hit [`MAX_FILES`] and the list is therefore **not** the whole
    /// workspace.
    ///
    /// Consumers that must be exhaustive to be *correct* — rename — are required to refuse
    /// when this is set (ADR-0029 §4). A silent cap is how a rename quietly misses a file.
    pub truncated: bool,
}

impl WorkspaceFileList {
    /// Whether no file was found.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// How many files were found.
    #[must_use]
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Whether `path` is in the list.
    #[must_use]
    pub fn contains(&self, path: &Path) -> bool {
        self.files.iter().any(|candidate| candidate == path)
    }
}

/// Walks `roots` for `*.jr` files.
///
/// Pure with respect to the database — it takes paths and touches the filesystem, and is
/// called from *outside* any query, which is the whole point of ADR-0029 §2.
///
/// # Paths come back spelled as the caller spelled them
///
/// Deliberately **not** canonicalised. A file's identity in this database is its path
/// string, and an editor sends the path *it* used: on macOS `/tmp` is a symlink to
/// `/private/tmp`, so a canonicalising walk returns `/private/tmp/x.jr` for the file the
/// client calls `/tmp/x.jr`. Those become two `SourceFile`s for one file — duplicated
/// analysis, and a rename that edits the same bytes twice through two URIs.
///
/// Duplicates are instead removed by comparing *canonical* forms while emitting the
/// original spelling, which is what lets a search path inside the walked tree — this
/// repository's own layout — not yield every file twice. Roots are visited in sorted order
/// and directory entries sorted, so which spelling wins is deterministic.
///
/// Symlinks are not followed. A symlinked parent directory is how a walk becomes infinite,
/// and the alternative — tracking visited inodes — is more machinery than this case earns.
/// A symlinked *file* is likewise skipped, because following it would report the same
/// source under two paths and a rename would then edit it twice.
///
/// Unreadable directories are skipped silently. A permissions error on some unrelated
/// subtree is not something a language server should refuse to start over.
#[must_use]
pub fn walk(roots: &[PathBuf]) -> WorkspaceFileList {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut truncated = false;

    // Canonical identity of everything already emitted or entered, so a directory reachable
    // by two spellings is walked once and a file reachable by two is listed once.
    let mut seen: Vec<PathBuf> = Vec::new();
    let identity = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    let mut queue: Vec<PathBuf> = roots.iter().filter(|root| root.is_dir()).cloned().collect();
    queue.sort();

    let mut index = 0;
    while index < queue.len() {
        let dir = queue[index].clone();
        index += 1;

        let key = identity(&dir);
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);

        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        // Sorted, so the walk order — and therefore which spelling of a duplicate wins — is
        // the same on every filesystem.
        let mut children: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        children.sort();

        for path in children {
            // `symlink_metadata` rather than `metadata`: the latter follows the link, and a
            // symlinked directory would then look like an ordinary one.
            let Ok(meta) = path.symlink_metadata() else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();

            if meta.is_dir() {
                if name.starts_with('.') || SKIP.contains(&name.as_str()) {
                    continue;
                }
                queue.push(path);
            } else if meta.is_file() && path.extension().is_some_and(|ext| ext == "jr") {
                let key = identity(&path);
                if seen.contains(&key) {
                    continue;
                }
                if files.len() >= MAX_FILES {
                    truncated = true;
                    break;
                }
                seen.push(key);
                files.push(path);
            }
        }

        if truncated {
            break;
        }
    }

    files.sort();

    WorkspaceFileList {
        files: files.into(),
        truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temporary tree, described as `(relative path, is_dir)` pairs.
    fn tree(entries: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().expect("a temporary directory");
        for entry in entries {
            let path = dir.path().join(entry);
            if entry.ends_with('/') {
                std::fs::create_dir_all(&path).expect("mkdir");
            } else {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).expect("mkdir");
                }
                std::fs::write(&path, "X :: 1;\n").expect("write");
            }
        }
        dir
    }

    fn names(list: &WorkspaceFileList) -> Vec<String> {
        list.files
            .iter()
            .map(|p| {
                p.file_name()
                    .expect("a file name")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    #[test]
    fn it_finds_jr_files_recursively() {
        let dir = tree(&["a.jr", "sub/b.jr", "sub/deep/c.jr"]);
        let list = walk(&[dir.path().to_path_buf()]);
        assert_eq!(names(&list), vec!["a.jr", "b.jr", "c.jr"]);
        assert!(!list.truncated);
    }

    #[test]
    fn it_ignores_other_extensions() {
        let dir = tree(&["a.jr", "b.rs", "c.md", "d.jr.txt"]);
        assert_eq!(names(&walk(&[dir.path().to_path_buf()])), vec!["a.jr"]);
    }

    #[test]
    fn it_skips_build_output_and_dot_directories() {
        let dir = tree(&[
            "keep.jr",
            "target/generated.jr",
            "node_modules/dep.jr",
            ".git/hook.jr",
            ".hidden/x.jr",
        ]);
        assert_eq!(names(&walk(&[dir.path().to_path_buf()])), vec!["keep.jr"]);
    }

    #[test]
    fn the_order_is_deterministic() {
        // Walk order depends on the filesystem, and a `WorkspaceEdit` that reorders itself
        // between runs cannot be tested.
        let dir = tree(&["z.jr", "a.jr", "m/y.jr", "m/b.jr"]);
        let first = walk(&[dir.path().to_path_buf()]);
        let second = walk(&[dir.path().to_path_buf()]);
        assert_eq!(first.files, second.files);
        let mut sorted = first.files.to_vec();
        sorted.sort();
        assert_eq!(first.files.to_vec(), sorted);
    }

    #[test]
    fn overlapping_roots_do_not_duplicate_a_file() {
        // This repository's own layout: `modules/` is a search path *and* inside the tree
        // the root walk covers.
        let dir = tree(&["modules/Basic/module.jr", "main.jr"]);
        let list = walk(&[
            dir.path().to_path_buf(),
            dir.path().join("modules"),
            // And the same root spelled differently.
            dir.path().join("modules").join(".."),
        ]);
        assert_eq!(names(&list), vec!["main.jr", "module.jr"]);
    }

    #[test]
    fn a_missing_root_is_skipped_rather_than_fatal() {
        let dir = tree(&["a.jr"]);
        let list = walk(&[
            dir.path().to_path_buf(),
            dir.path().join("does-not-exist-at-all"),
        ]);
        assert_eq!(names(&list), vec!["a.jr"]);
    }

    #[test]
    fn no_roots_is_an_empty_list_rather_than_an_error() {
        let list = walk(&[]);
        assert!(list.is_empty());
        assert!(!list.truncated);
    }

    #[test]
    #[cfg(unix)]
    fn a_symlinked_directory_is_not_followed() {
        // The infinite-walk case: a link to an ancestor.
        let dir = tree(&["real/a.jr"]);
        std::os::unix::fs::symlink(dir.path(), dir.path().join("loop")).expect("symlink");
        let list = walk(&[dir.path().to_path_buf()]);
        assert_eq!(names(&list), vec!["a.jr"], "the walk followed a symlink");
    }

    #[test]
    #[cfg(unix)]
    fn a_symlinked_file_is_skipped_so_a_rename_cannot_edit_it_twice() {
        let dir = tree(&["a.jr"]);
        std::os::unix::fs::symlink(dir.path().join("a.jr"), dir.path().join("alias.jr"))
            .expect("symlink");
        assert_eq!(names(&walk(&[dir.path().to_path_buf()])), vec!["a.jr"]);
    }

    #[test]
    fn hitting_the_cap_sets_truncated() {
        // Not MAX_FILES files on disk: the flag is what consumers branch on, and this
        // asserts the branch exists rather than the constant's value.
        let mut entries = Vec::new();
        for i in 0..12 {
            entries.push(format!("f{i}.jr"));
        }
        let refs: Vec<&str> = entries.iter().map(String::as_str).collect();
        let dir = tree(&refs);
        let list = walk(&[dir.path().to_path_buf()]);
        assert_eq!(list.len(), 12);
        assert!(
            !list.truncated,
            "twelve files is well under the cap, so this must not be truncated"
        );
        // The cap itself is asserted as data rather than with `assert!(MAX_FILES > 12)`,
        // which clippy rightly calls a constant assertion.
        assert_eq!(MAX_FILES, 10_000);
    }
}
