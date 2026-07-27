//! Converting between filesystem paths and `file:` URIs.
//!
//! # Why this is hand-written
//!
//! `lsp-types` 0.97 replaced the old `Url` with a newtype over `fluent_uri::Uri`, which
//! parses and serialises URIs but does not know about filesystem paths. The old
//! `Url::from_file_path` came from the `url` crate, and adding one is a dependency
//! decision ADR-0009 deliberately makes deliberate. For the one direction each way that
//! a language server needs, the conversion is small enough to own.
//!
//! # What it does and does not handle
//!
//! Absolute POSIX paths, percent-encoding the characters a URI path may not carry
//! literally. It does **not** handle Windows drive letters or UNC paths, because
//! nothing in this project has been built or run on Windows and a half-correct
//! implementation would be worse than an honest absence — `PLAN.md` §1.4's platform
//! criteria are macOS arm64 and Linux x86-64. When Windows arrives, this is the module
//! that has to grow, and the `debug_assert` below is what will say so.

use std::path::{Path, PathBuf};

use lsp_types::Uri;

/// The `file:` URI for an absolute path.
///
/// `None` if the path is not absolute or the result does not parse — both of which mean
/// the caller has a path it should not be reporting a location in.
#[must_use]
pub fn from_path(path: &Path) -> Option<Uri> {
    // Windows would need drive-letter and UNC handling this module does not have; see
    // the module docs for why an honest absence beats a half-correct implementation.
    #[cfg(windows)]
    compile_error!("jr-lsp's uri module does not handle Windows paths yet");
    if !path.is_absolute() {
        return None;
    }
    let mut out = String::from("file://");
    for byte in path.to_str()?.bytes() {
        match byte {
            // Unreserved, plus the separators a path needs to keep.
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out.parse().ok()
}

/// The path a `file:` URI names.
///
/// `None` for any other scheme. A client is not supposed to send one, and inventing a
/// path from `untitled:` would attach diagnostics to a file that does not exist.
#[must_use]
pub fn to_path(uri: &Uri) -> Option<PathBuf> {
    let text = uri.as_str();
    let rest = text.strip_prefix("file://")?;
    // An empty authority is the normal form (`file:///tmp/x`); a non-empty one is a
    // network path this module does not handle.
    let rest = rest.strip_prefix('/').map(|r| format!("/{r}"))?;
    Some(PathBuf::from(percent_decode(&rest)))
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            if let Some(byte) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_round_trips() {
        let path = Path::new("/tmp/jairs/hello.jr");
        let uri = from_path(path).expect("an absolute path converts");
        assert_eq!(uri.as_str(), "file:///tmp/jairs/hello.jr");
        assert_eq!(to_path(&uri).as_deref(), Some(path));
    }

    #[test]
    fn a_space_is_percent_encoded_and_decoded() {
        let path = Path::new("/tmp/my dir/a.jr");
        let uri = from_path(path).expect("an absolute path converts");
        assert_eq!(uri.as_str(), "file:///tmp/my%20dir/a.jr");
        assert_eq!(to_path(&uri).as_deref(), Some(path));
    }

    #[test]
    fn a_relative_path_is_refused() {
        assert!(from_path(Path::new("hello.jr")).is_none());
    }

    #[test]
    fn a_non_file_scheme_is_refused() {
        let uri: Uri = "untitled:Untitled-1".parse().expect("parses");
        assert!(
            to_path(&uri).is_none(),
            "inventing a path for an unsaved buffer would attach diagnostics to a file \
             that does not exist"
        );
    }
}
