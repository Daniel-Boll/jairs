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
//! - [`mod@render`] is the **one** place a declaration becomes text, so a hover card, a
//!   completion item, an outline entry and a signature-help label cannot disagree about the
//!   same procedure (ADR-0028 §1).
//! - [`mod@completion`] answers what can be written at the cursor (ADR-0028 §5).
//! - [`mod@defs`] and [`mod@navigate`] find a declaration and everywhere it is used, and
//!   turn that into rename edits (ADR-0030).
//! - [`mod@actions`] offers to *change* the code, keyed on the diagnostic the user is
//!   already looking at (ADR-0031 §4).
//! - [`mod@hints`] tells the user what the source does not say: the active parameter of a
//!   call, an inferred `:=` type, and the value a `#run` produced (ADR-0031 §6, §7).
//! - [`mod@server`] is the stdio loop: the main thread writes, a worker reads a snapshot,
//!   and salsa cancels the worker when the next edit arrives (§2).
//!
//! # What it deliberately does not do
//!
//! **Semantic tokens**, and any code action that would need to invent something the source
//! does not contain — `#foreign` needs a library name, so E0203's quick fix offers only a
//! body (ADR-0031 §4).
//!
//! Nothing here works on a **type annotation's own span**, because `jr_hir::TypeRef` carries
//! none. That is why the E0212 quick fix replaces the range the *diagnostic* points at
//! rather than one found from the cursor, and it is pinned by
//! `hovering_a_type_annotation_returns_nothing_today` — a test written to fail the day the
//! limitation is lifted.
//!
//! Twelve capabilities is more than §1.4's box asks for. Completion, references, rename and
//! now code actions and hints were wave W9's by `PLAN.md` §2.1 and were pulled forward
//! because each needed machinery the one before it built: rename needs reference-finding,
//! reference-finding needs the workspace list, and a code action needs somewhere to put the
//! edit. Building them apart would have built the traversal three times.

pub mod actions;
pub mod completion;
pub mod defs;
pub mod handlers;
pub mod hints;
pub mod locate;
pub mod navigate;
pub mod position;
pub mod render;
pub mod server;
pub mod tokens;
pub mod uri;

pub use actions::code_actions;
pub use completion::{completion, resolve_completion};
pub use defs::{DefId, Reference, definition_at, references};
pub use handlers::{diagnostics, formatting, goto_definition, hover};
pub use hints::{EnclosingCall, enclosing_call, inlay_hints, signature_help};
pub use locate::{DeclSite, Located, locate, locate_declaration};
pub use navigate::{
    RenameRefusal, document_highlight, document_symbol, find_references, prepare_rename, rename,
    workspace_symbol,
};
pub use position::{Encoding, Positions};
pub use render::{Card, Decl, binding_card, container_of, type_name};
pub use server::{ServerOptions, capabilities, run_stdio};
