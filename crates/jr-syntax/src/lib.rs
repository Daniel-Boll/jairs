//! Lexer, `SyntaxKind`, the error-recovering recursive-descent parser, the
//! lossless `rowan` CST, and the typed AST accessor layer.
//!
//! This crate is the single source of truth for Jairs syntax. The batch
//! compiler, the language server, and `jr fmt` all go through it -- there is
//! deliberately no second frontend, because two frontends always drift.
//!
//! The `tree-sitter` grammar in `tree-sitter-jairs/` is a separate artefact for
//! editor highlighting only. It is held in agreement with this parser by the
//! shared corpus in `tests/corpus/` and the `corpus-drift` CI job.

mod kind;
mod lexer;

pub use kind::{JairsLanguage, SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken};
pub use lexer::{LexOutput, Token, lex};
