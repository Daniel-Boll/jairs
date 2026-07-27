//! Foundational types shared by every other crate: source spans, [`FileId`],
//! string interning, arenas, and the newtype ID conventions.
//!
//! Nothing in this crate knows anything about the Jairs language. It is
//! deliberately boring infrastructure, because everything else depends on it and
//! churn here is expensive.

mod id;
mod intern;
mod source;
mod span;
mod trap;

pub use intern::{Interner, Symbol};
pub use source::{FileId, LineCol, SourceFile, SourceMap};
pub use span::Span;
pub use trap::{render_location, trap_message};

/// Re-exported so downstream crates agree with `rowan` on offset types without
/// each depending on `text-size` directly.
pub use text_size::{TextLen, TextRange, TextSize};
