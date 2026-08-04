//! High-level IR (HIR) for the Jairs compiler.
//!
//! This crate lowers the lossless rowan CST (via the typed AST accessors in
//! `jr-syntax`) into a desugared, arena-based representation that downstream
//! passes (`jr-sema`, `jr-mir`) can work with efficiently.
//!
//! # Design
//!
//! ## Purity
//!
//! `jr-hir` is a **pure function** over a parsed syntax tree. It has no
//! filesystem access, no salsa dependency, and no cross-file loading. The
//! `jr-db` crate wraps these functions as tracked salsa queries (ADR-0007).
//! This makes the crate directly testable without a database.
//!
//! ## One file at a time
//!
//! [`lower_file`] processes exactly one file. Cross-file information (imported
//! scopes) is passed in as an explicit parameter; the crate never loads files
//! itself.
//!
//! ## Spans — a deliberate tradeoff
//!
//! Every HIR node carries a [`jr_base::Span`]. This is simple and gives good
//! diagnostics, but it means a whitespace-only edit changes spans and therefore
//! invalidates downstream salsa queries. The correct long-term answer is
//! rust-analyzer's `AstIdMap` (stable per-file IDs, spans looked up on demand).
//!
//! // TODO(AstIdMap): Replace per-node `Span` storage with stable `AstId`s
//! // (rust-analyzer style). Each node gets a stable index into an `AstIdMap`
//! // that is computed once per file; spans are looked up from the map on demand
//! // rather than stored in every node. This makes whitespace-only edits
//! // non-invalidating for downstream queries. Revisit when the salsa query
//! // graph shows span-churn as a bottleneck (likely during W4 comptime work).
//!
//! ## Diagnostic codes
//!
//! The lexer uses E0001–E0006 and the parser uses E0100–E0199. HIR uses
//! **E0200+**:
//!
//! | Code  | Message |
//! |-------|---------|
//! | E0200 | duplicate declaration of `<name>` |
//! | E0201 | unresolved name `<name>` |
//! | E0202 | use of local `<name>` before its declaration |
//! | E0203 | procedure has neither a body nor a `#foreign` attribute |
//! | E0204 | *(moved to `jr-sema`: a literal's fit depends on its contextual type)* |
//! | E0205 | unknown string escape `\<c>` |
//! | E0206 | invalid unicode escape |
//! | E0207 | declaration or `#run` inside a procedure body |
//! | E0208 | `#import` outside file scope |
//! | E0209 | directive used where it is not valid |
//! | E0210 | module not found (owned by `jr-db`, not this crate) |
//! | E0211 | ambiguous name provided by multiple imported modules |

pub mod dump;
pub mod hir;
pub mod instantiate;
pub mod lower;
pub mod resolve;

pub use hir::{
    AssignOp, BinOp, Body, BodyId, ConstValue, Enum, EnumId, EnumMember, Expr, ExprId, Field,
    FieldId, FileHir, ForIterable, ForeignInfo, InsertOperands, Item, ItemId, ItemKind, ItemScope,
    Literal, Local, LocalId, Param, ParamId, Proc, ProcId, Res, Stmt, StmtId, Struct, StructId,
    SwitchArm, TypeRef, TypeRefId, UnOp,
};
pub use instantiate::{Instantiation, expand_instantiations};
pub use lower::{lower_file, lower_file_with_inserts};
pub use resolve::{ExprScope, ResolveMap, resolve};
