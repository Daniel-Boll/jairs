//! The Jairs language server, implemented as a consumer of `jr-db` queries -- never a second frontend.
//!
//! [ADR-0024](../../../docs/adr/0024-language-server.md) is this crate's
//! specification, and ADR-0007 is the claim it exists to test: that the LSP is a
//! *consumer* of the same salsa queries as the batch compiler rather than a second
//! front end. Nothing in here analyses anything. [`diagnostics()`] calls
//! `jr_db::file_diagnostics`, [`hover()`] calls `jr_db::checked`, and
//! [`goto_definition()`] calls `jr_db::resolved`. If a capability ever needs a fact no
//! query produces, the fix belongs in `jr-db` and not here.
//!
//! # Shape
//!
//! - [`mod@position`] converts between LSP positions and byte offsets, under a negotiated
//!   encoding (ADR-0024 §3).
//! - [`mod@locate`] turns an offset into a HIR node by scanning spans (§1). This is the one
//!   thing no query does, because ADR-0013 deferred `AstIdMap`.
//! - [`mod@handlers`] answers the three capabilities §1.4 asks for, as pure functions of
//!   `(&db, params)` so that they can be tested without a transport (§4).
//! - [`mod@server`] is the stdio loop: the main thread writes, a worker reads a snapshot,
//!   and salsa cancels the worker when the next edit arrives (§2).
//!
//! # What it deliberately does not do
//!
//! Completion, rename, references, inlay hints and semantic tokens are wave W9's
//! (`PLAN.md` §2.1), and hover renders a type rather than documentation for the same
//! reason. §1.4's box names exactly three capabilities, and a server that quietly
//! grew a fourth would be a wave that did not finish the one it was scoped to.

pub mod handlers;
pub mod locate;
pub mod position;
pub mod server;
pub mod uri;

pub use handlers::{diagnostics, goto_definition, hover};
pub use locate::{Located, locate};
pub use position::{Encoding, Positions};
pub use server::{ServerOptions, capabilities, run_stdio};
