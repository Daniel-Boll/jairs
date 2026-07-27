//! Documentation attached to declarations: the `file_docs` query.
//!
//! # Why this is a side table rather than a field on each HIR item
//!
//! ADR-0027 §2. Documentation is not part of the typed program, and this module is
//! the place that statement is enforced rather than merely asserted: `jr-sema` has
//! no way to read a doc comment, because there is nothing on a [`jr_hir::Item`] to
//! read. Only the language server depends on this query.
//!
//! It also keeps the invalidation honest in the one direction that matters. This
//! query returns a value that implements [`PartialEq`], so salsa backdates it:
//! editing a procedure's *body* recomputes `file_docs` and then discovers the docs
//! are unchanged, and nothing downstream of the docs re-runs. The reverse — editing
//! only a comment — still invalidates `file_hir`, because every span after the edit
//! moves. That is a property of byte spans, not something this module can fix, and
//! claiming otherwise would be the kind of unchecked assertion this project keeps
//! having to retract.
//!
//! # Why attachment is computed here rather than during lowering
//!
//! Lowering walks the CST and could attach docs as it goes, which would save this
//! module's second traversal. It would also put prose in `jr-hir`'s output types,
//! which is what §2 rejected. The cost is one pass over the file's top-level
//! trivia, which is bounded by the file and happens only when an editor asks.

use std::sync::Arc;

use jr_hir::{FileHir, ItemId};
use jr_syntax::{
    SyntaxElement,
    SyntaxKind::{DOC_COMMENT, MODULE_DOC_COMMENT, WHITESPACE},
    SyntaxNode,
};
use rustc_hash::FxHashMap;

use crate::{Db, SourceFile, module_loader::file_hir};

/// Documentation harvested from a file's `///` and `//!` comments.
///
/// Empty for a file with no doc comments, which is every file in the corpus that
/// predates ADR-0027.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileDocs {
    /// Doc text per documented item. Items with no `///` above them are absent
    /// rather than present-and-empty, so that "undocumented" and "documented with
    /// nothing" cannot be confused.
    docs: FxHashMap<ItemId, String>,
    /// The file's own documentation, from a leading `//!` block.
    module: Option<String>,
}

impl FileDocs {
    /// The documentation for one item, if it has any.
    #[must_use]
    pub fn get(&self, item: ItemId) -> Option<&str> {
        self.docs.get(&item).map(String::as_str)
    }

    /// The file's own documentation, from `//!`.
    #[must_use]
    pub fn module(&self) -> Option<&str> {
        self.module.as_deref()
    }

    /// The number of documented items. Used by tests and by nothing else.
    #[must_use]
    pub fn len(&self) -> usize {
        self.docs.len()
    }

    /// Whether no item in the file is documented.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.docs.is_empty() && self.module.is_none()
    }
}

/// Harvests `///` and `//!` comments and attaches them to the items they precede.
///
/// A `///` that precedes no declaration is **silently dropped** (ADR-0027 §3): it
/// raises no diagnostic, so this query cannot fail and returns no diagnostics.
///
/// A blank line between a doc comment and the declaration breaks the attachment,
/// matching what a reader sees. So does any non-trivia token, which is what stops a
/// trailing `///` at the end of a file from attaching to nothing forever.
#[salsa::tracked(returns(clone))]
pub fn file_docs(db: &dyn Db, file: SourceFile) -> Arc<FileDocs> {
    let parse = crate::parse_file(db, file);
    let hir = file_hir(db, file);
    Arc::new(harvest(&parse.syntax(), hir.as_ref()))
}

/// The pure half, so it is testable without a database.
fn harvest(root: &SyntaxNode, hir: &FileHir) -> FileDocs {
    // Items are keyed by where they start, because that is the only thing a CST node
    // and a HIR item are guaranteed to agree on: `ItemId` is an index into
    // `FileHir::items` assigned by lowering, and the CST knows nothing about it.
    let starts: FxHashMap<u32, ItemId> = hir
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| (item.span.range.start().into(), ItemId::from_usize(i)))
        .collect();

    let mut out = FileDocs::default();
    let mut pending: Vec<String> = Vec::new();
    let mut module: Vec<String> = Vec::new();

    for element in root.children_with_tokens() {
        match element {
            SyntaxElement::Token(tok) => match tok.kind() {
                DOC_COMMENT => pending.push(strip_marker(tok.text(), "///")),
                MODULE_DOC_COMMENT => module.push(strip_marker(tok.text(), "//!")),
                WHITESPACE => {
                    // A blank line ends a doc block. Without this, a licence header
                    // separated from the first declaration by an empty line would
                    // become that declaration's documentation.
                    if tok.text().chars().filter(|&c| c == '\n').count() >= 2 {
                        pending.clear();
                    }
                }
                // Any other trivia — an ordinary comment between the docs and the
                // declaration — also breaks the block, because the reader sees it
                // break.
                _ => pending.clear(),
            },
            SyntaxElement::Node(node) => {
                let start: u32 = node.text_range().start().into();
                if let Some(&item) = starts.get(&start)
                    && !pending.is_empty()
                {
                    out.docs.insert(item, pending.join("\n"));
                }
                pending.clear();
            }
        }
    }

    if !module.is_empty() {
        out.module = Some(module.join("\n"));
    }
    out
}

/// Strips the marker and one following space from one doc-comment line.
///
/// One space only: further indentation is the author's, and a doc comment holding an
/// indented code block would be ruined by trimming all leading whitespace.
fn strip_marker(text: &str, marker: &str) -> String {
    let body = text.strip_prefix(marker).unwrap_or(text);
    body.strip_prefix(' ')
        .unwrap_or(body)
        .trim_end()
        .to_string()
}

#[cfg(test)]
mod tests {
    use jr_base::{FileId, Interner};

    use super::*;

    fn docs_of(src: &str) -> FileDocs {
        let parse = jr_syntax::parser::parse(src, FileId::from_usize(0));
        let interner = Interner::new();
        let (hir, _) = jr_hir::lower_file(&parse, FileId::from_usize(0), &interner);
        harvest(&parse.syntax(), &hir)
    }

    /// The `ItemId` of the item declared with `name`.
    fn item_named(src: &str, name: &str) -> ItemId {
        let parse = jr_syntax::parser::parse(src, FileId::from_usize(0));
        let interner = Interner::new();
        let (hir, _) = jr_hir::lower_file(&parse, FileId::from_usize(0), &interner);
        let sym = interner.intern(name);
        hir.items
            .iter()
            .position(|i| i.name == Some(sym))
            .map(ItemId::from_usize)
            .expect("item declared")
    }

    #[test]
    fn a_doc_comment_attaches_to_the_declaration_below_it() {
        let src = "/// Adds two numbers.\nadd :: (a: s64) -> s64 { return a; }\n";
        let docs = docs_of(src);
        assert_eq!(docs.get(item_named(src, "add")), Some("Adds two numbers."));
    }

    #[test]
    fn consecutive_lines_join_with_newlines() {
        let src = "/// First.\n/// Second.\nX :: 1;\n";
        let docs = docs_of(src);
        assert_eq!(docs.get(item_named(src, "X")), Some("First.\nSecond."));
    }

    #[test]
    fn a_blank_line_breaks_the_attachment() {
        // Otherwise a file header becomes the first declaration's documentation.
        let src = "/// A header, not documentation.\n\nX :: 1;\n";
        assert_eq!(docs_of(src).get(item_named(src, "X")), None);
    }

    #[test]
    fn an_ordinary_comment_breaks_the_attachment() {
        let src = "/// Documentation.\n// an aside\nX :: 1;\n";
        assert_eq!(docs_of(src).get(item_named(src, "X")), None);
    }

    #[test]
    fn an_ordinary_comment_is_never_documentation() {
        let src = "// Not documentation.\nX :: 1;\n";
        let docs = docs_of(src);
        assert_eq!(docs.get(item_named(src, "X")), None);
        assert!(docs.is_empty());
    }

    #[test]
    fn a_stray_doc_comment_is_silently_dropped() {
        // ADR-0027 §3: no diagnostic, and nothing attached. This query has no way to
        // report one, which is the point.
        let src = "X :: 1;\n/// documents nothing at all\n";
        let docs = docs_of(src);
        assert_eq!(docs.get(item_named(src, "X")), None);
        assert_eq!(docs.len(), 0);
    }

    #[test]
    fn module_docs_are_collected_separately() {
        let src = "//! The Basic module.\n//! Second line.\n\n/// An item.\nX :: 1;\n";
        let docs = docs_of(src);
        assert_eq!(docs.module(), Some("The Basic module.\nSecond line."));
        assert_eq!(docs.get(item_named(src, "X")), Some("An item."));
    }

    #[test]
    fn only_one_leading_space_is_stripped() {
        let src = "///     indented code\nX :: 1;\n";
        assert_eq!(
            docs_of(src).get(item_named(src, "X")),
            Some("    indented code")
        );
    }

    #[test]
    fn four_slashes_are_not_documentation() {
        let src = "//// ------------\nX :: 1;\n";
        assert!(docs_of(src).is_empty());
    }

    #[test]
    fn each_item_gets_its_own_docs() {
        let src = "/// The first.\nA :: 1;\n/// The second.\nB :: 2;\n";
        let docs = docs_of(src);
        assert_eq!(docs.get(item_named(src, "A")), Some("The first."));
        assert_eq!(docs.get(item_named(src, "B")), Some("The second."));
        assert_eq!(docs.len(), 2);
    }

    #[test]
    fn a_documented_struct_and_proc_are_both_reached() {
        // Both are `ItemKind::Const` holding a `ConstValue`, so this checks the
        // keying works for the kinds hover is actually asked about.
        let src = "/// A point.\nPoint :: struct {\n    x: s64;\n}\n/// A proc.\nf :: () {}\n";
        let docs = docs_of(src);
        assert_eq!(docs.get(item_named(src, "Point")), Some("A point."));
        assert_eq!(docs.get(item_named(src, "f")), Some("A proc."));
    }
}
