//! The `Backend` trait and the lowering helpers shared by every native backend.
//!
//! # What this crate is for
//!
//! ADR-0009 pins `cranelift-*` with `=` because its API is not semver-stable, and
//! requires that **all** Cranelift contact live inside `jr-codegen-clif` behind a
//! trait. This crate is that trait. Nothing here depends on Cranelift, on LLVM, or
//! on any object-file format: the vocabulary is [`jr_mir`], [`jr_pool`] and
//! [`jr_sema`], all of which are ours.
//!
//! That is not merely tidiness. It is what makes wave W8's LLVM back end an
//! addition rather than a rewrite, and what makes an ADR-0009 dependency bump touch
//! one crate.
//!
//! # Why the lifecycle is three phases and not one call
//!
//! ADR-0019 §1 settles this. A [`Backend`] is driven as
//! *declare → define → finalise*:
//!
//! 1. [`Backend::declare`] every procedure that will exist, with its signature.
//! 2. [`Backend::define`] one body at a time.
//! 3. [`Backend::finalise`] into an artifact.
//!
//! The declare phase is what makes a **forward or cross-file call representable at
//! all**. ADR-0018 §5 widened `Callee::Direct` to a [`jr_mir::ProcRef`], so a body
//! may call a procedure defined later in the same file or in another file entirely.
//! A back end that first learns of a callee when it reaches the call must either
//! walk every body up front to collect callees — the declare phase, unnamed and
//! un-sequenceable — or patch references afterwards. Making it a phase puts the
//! ordering in the interface, where a reader can see it and a driver can honour it.
//!
//! The rejected alternative was a single `compile_body`, which is smaller while
//! there is one back end but does not remove the two-phase requirement, only hides
//! it. ADR-0019 §1 argues it at length.
//!
//! # What a back end must not do
//!
//! **Compute layout.** Not size, not alignment, not a field offset, not the offset
//! of a string's `data` or `count`. Every one of those comes from
//! [`jr_pool::layout_of`] and its siblings, which the VM already calls (ADR-0018 §2). This is stated
//! here, in the trait's own crate, because it is the one mistake in this area that
//! produces a *silent* comptime/runtime divergence — a struct whose field sits at a
//! different offset in a `#run` than at runtime is two different programs from one
//! source, with no diagnostic and no verifier complaint — and because no test can
//! be relied on to catch it in general. A [`TargetLayout`] is handed to
//! [`Backend::define`] for exactly this reason: the back end passes it back into
//! `jr-pool` rather than interpreting it.

use jr_base::FileId;
use jr_hir::FileHir;
use jr_mir::{MirBody, ProcRef};
use jr_pool::{Pool, PoolId, TargetLayout};
use jr_sema::FileSignatures;

mod error;
mod plan;
mod trap;

pub use error::CodegenError;
pub use plan::{ForeignSymbol, ProcDecl, ProcKind, declarations, symbol_for};
pub use trap::{TRAP_HELPER, TrapKind};

/// One native back end.
///
/// Implemented by `jr-codegen-clif`, and by `jr-codegen-llvm` in wave W8. See the
/// crate docs for why the lifecycle is three phases, and for the prohibition on
/// computing layout.
///
/// # Driving one
///
/// A driver calls [`declare`](Backend::declare) for every procedure in the program
/// — including `#foreign` ones, which become imports rather than definitions —
/// then [`define`](Backend::define) for each body it has MIR for, then
/// [`finalise`](Backend::finalise) exactly once. Defining a procedure that was
/// never declared is a bug in the driver, and an implementation should say so with
/// [`CodegenError::Undeclared`] rather than silently declaring it late.
pub trait Backend {
    /// Declares a procedure's existence and signature.
    ///
    /// Every procedure the program can call must be declared before any body is
    /// defined, so that a call to a procedure defined later — or in another file —
    /// resolves to a real reference rather than to a patch-up list.
    ///
    /// `pool` and `layout` are passed for the same reason
    /// [`define`](Backend::define) takes them: a signature's parameter and return
    /// types are [`PoolId`]s, and turning one into a machine type is a layout
    /// question that only `jr-pool` may answer.
    ///
    /// # Errors
    /// [`CodegenError`] when the signature cannot be represented, most often
    /// because a parameter or return type has no layout ([`PoolId::ERROR`], or a
    /// comptime-only type such as a `#system_library` handle).
    fn declare(
        &mut self,
        decl: &ProcDecl,
        pool: &Pool,
        layout: TargetLayout,
    ) -> Result<(), CodegenError>;

    /// Generates code for one procedure body.
    ///
    /// `layout` is passed rather than assumed so that the back end hands it back to
    /// [`jr_pool::layout_of`]; see the crate docs on why a back end must not compute
    /// its own.
    ///
    /// # Errors
    /// [`CodegenError`] when the body cannot be generated — an undeclared callee, a
    /// type with no layout, or an unsupported construct.
    fn define(
        &mut self,
        proc: ProcRef,
        body: &MirBody,
        pool: &Pool,
        layout: TargetLayout,
        locations: &dyn TrapLocations,
    ) -> Result<(), CodegenError>;

    /// Produces the artifact, consuming the back end.
    ///
    /// The bytes are an object file in the host's native format. Turning them into
    /// an executable — including any runtime object and, on macOS, an ad-hoc
    /// signature — belongs to `jr-link`, so that this crate stays free of linker
    /// and platform concerns.
    ///
    /// # Errors
    /// [`CodegenError`] when the module cannot be emitted, which at this point is a
    /// back end or target configuration fault rather than a program one.
    fn finalise(self: Box<Self>) -> Result<Vec<u8>, CodegenError>;

    /// Every library a `#foreign` declaration named, for the link line.
    ///
    /// On the trait rather than on an implementation, because it is a question every back
    /// end must answer — a `#foreign` declaration names a library whatever generates the
    /// code — and because a driver that has to name a concrete back end to ask it can only
    /// ever drive one (ADR-0143 §6). It was an inherent method on `ClifBackend` until the
    /// second back end made that impossible.
    ///
    /// Collected during the declare phase, so this is meaningful from then on and complete
    /// once every file has been declared.
    fn libraries(&self) -> &[String];
}

/// How a back end learns where a trap is, without seeing the front end.
///
/// A back end holds a [`jr_mir::MirSpan`] at every trap site and can do nothing with
/// it: resolving one needs the file's `FileHir`, and rendering it needs a `SourceMap`.
/// Neither is available here, and ADR-0009 confines the back end so that neither
/// *should* be — a back end whose signature mentions `FileHir` has the front end in
/// it, which is what the confinement exists to prevent.
///
/// So the driver, which has both, implements this and the back end asks. ADR-0020 §3
/// made it a parameter of [`Backend::define`] rather than a setter on the back end,
/// because a setter is order-dependent hidden state: forget the call and every trap
/// silently loses its location, which is the class of quiet degradation this project
/// keeps being bitten by.
pub trait TrapLocations {
    /// Where `span` is, as a path plus a one-based line and column.
    ///
    /// `None` when there is nothing to point at — a compiler-invented value, where
    /// [`jr_mir::MirSpan::Synthetic`] is the honest answer. A back end must then
    /// report without a location rather than substituting a nearby one.
    ///
    /// **The structured form is the one implementors define**, and [`location`](TrapLocations::location)
    /// formats it. Debug info needs the pieces — a DWARF line program stores a file index and a line
    /// number, not a rendered string — and having the two come from one method is what keeps a trap
    /// message and a `.debug_line` row from ever disagreeing about where a statement is. That is
    /// ADR-0020 §2's argument for a single trap-message formatter, applied one level down.
    fn position(&self, span: jr_mir::MirSpan) -> Option<SourcePosition>;

    /// The location of `span`, rendered as `path:line:col`.
    ///
    /// A provided method over [`position`](TrapLocations::position), so an implementor cannot supply one
    /// without the other and the rendering exists once.
    fn location(&self, span: jr_mir::MirSpan) -> Option<String> {
        let at = self.position(span)?;
        Some(format!("{}:{}:{}", at.path, at.line, at.column))
    }
}

/// Where something is in a source file: a path, and a one-based line and column.
///
/// Owned `String` rather than a borrowed path, because the only implementor builds it from a
/// `SourceMap` behind a database and the lifetime would leak salsa into this crate's signature — the
/// confinement the trait's own docs exist to protect. A location is asked for once per trap site and
/// once per instruction with a new span, so the allocation is not on any hot path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcePosition {
    /// The file's path, as the source map records it.
    pub path: String,
    /// The line, counting from 1 — which is what both a diagnostic and DWARF want.
    pub line: u32,
    /// The column, counting from 1.
    pub column: u32,
}

/// A [`TrapLocations`] that never knows a location.
///
/// For a caller with no source map — a test that only wants to check that a body
/// generates — so that "no locations available" is stated rather than achieved by
/// passing something misleading.
pub struct NoLocations;

impl TrapLocations for NoLocations {
    fn position(&self, _span: jr_mir::MirSpan) -> Option<SourcePosition> {
        None
    }
}

/// Everything a back end needs about one file, without a database.
///
/// `jr-codegen` and its implementations are pure functions over these, exactly as
/// `jr-mir` is a pure function over HIR plus `jr-sema`'s output. That is what keeps
/// a back end testable on a single file with no salsa, no filesystem and no module
/// loader — the same argument `jr-mir`'s and `jr-sema`'s test harnesses make, and
/// worth keeping for the same reason.
pub struct FileInput<'a> {
    /// The file's stable id, which is half of every [`ProcRef`]'s identity.
    pub file: FileId,
    /// The file's HIR, for procedure declarations and `#foreign` info.
    pub hir: &'a FileHir,
    /// The file's signatures, which carry parameter and return types and the
    /// resolved `#foreign` library (ADR-0019 §4).
    pub signatures: &'a FileSignatures,
    /// Each procedure's **source** name, for a backtrace frame (ADR-0066 §3), indexed by
    /// [`jr_hir::ProcId`]'s own index.
    ///
    /// Supplied by the caller rather than read from `hir` here, because resolving a `Symbol` to text
    /// needs the interner and this crate has no database to ask — the same split that puts a trap's
    /// *location* on the caller's side (ADR-0020 §3).
    ///
    /// A slice rather than a map, because [`plan::declarations`] already walks `0..hir.procs.len()` and
    /// a parallel slice is the shape that cannot disagree about which procedure an entry describes. An
    /// entry is `None` for a procedure no item binds; its frame is omitted rather than given a
    /// placeholder.
    pub names: &'a [Option<String>],
}

/// The type of a procedure's parameter or result, as a back end sees it.
///
/// A thin alias over [`PoolId`] so that a signature's fields read as types rather
/// than as opaque indices. A back end resolves one to a machine type through
/// [`jr_pool::layout_of`] and never by inspecting the pool item itself.
pub type Ty = PoolId;
