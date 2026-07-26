//! Signatures: everything about a file that another file is allowed to see.
//!
//! # Why signatures are a separate thing from a check
//!
//! ADR-0016 §5 requires that typing a call into an imported module read only
//! that module's *signatures*, and that computing signatures depend only on the
//! other file's HIR — never on the other file's full type-check. Two files that
//! import each other otherwise make the query graph cyclic, and
//! `tests/corpus/imports/valid/005-import-cycle-is-legal.jr` stops terminating.
//!
//! The consequence, stated so it is not rediscovered later: a procedure's
//! signature must be typeable **from syntax alone**. That holds in Jairs-0
//! because parameter and return types are always written out
//! (`docs/spec/02-declarations.md`), and it stops holding the day return-type
//! inference is added.

use jr_hir::{ItemId, ProcId};
use jr_pool::{DeclId, Field, Pool, PoolId};
use rustc_hash::FxHashMap;

// ---------------------------------------------------------------------------
// SigKind
// ---------------------------------------------------------------------------

/// What kind of declaration a name came from.
///
/// The HIR's `Res::Item` is unclassified — it says "a file-level item" and
/// nothing more — so the checker has to look the item's kind up anyway. Carrying
/// it on the signature entry means that lookup happens once, in the phase that
/// already has the item in hand, rather than at every use site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SigKind {
    /// `name :: value` where the value is neither a procedure nor a struct.
    Const,
    /// `name := value` or `name: T` at file scope.
    Var,
    /// `name :: (params) -> T { … }`, or a `#foreign` declaration.
    Proc,
    /// `name :: struct { … }`.
    Struct,
}

// ---------------------------------------------------------------------------
// SigEntry
// ---------------------------------------------------------------------------

/// What one exported name means.
#[derive(Debug, Clone, Copy)]
pub struct SigEntry {
    /// The type of the name *used as a value*.
    ///
    /// For a name that denotes a type this is [`PoolId::TYPE`], because a type
    /// is a value of type `type` (ADR-0012). It is never `None`: an entry that
    /// could not be typed carries [`PoolId::ERROR`], so that consumers poison
    /// instead of branching.
    pub ty: PoolId,
    /// The type this name *denotes*, when it denotes one.
    ///
    /// `Some` exactly for [`SigKind::Struct`]. This is the field a type
    /// annotation reads; `ty` is the field an expression reads.
    pub type_value: Option<PoolId>,
    /// Which kind of declaration this was.
    pub kind: SigKind,
    /// The declaring item, for diagnostics that want to point at it.
    pub item: ItemId,
}

impl SigEntry {
    /// Returns `true` if assigning to this name is legal — that is, if it is a
    /// variable rather than a constant, a procedure, or a type.
    #[must_use]
    pub fn is_assignable(&self) -> bool {
        match self.kind {
            SigKind::Var => true,
            SigKind::Const | SigKind::Proc | SigKind::Struct => false,
        }
    }
}

// ---------------------------------------------------------------------------
// ProcSig
// ---------------------------------------------------------------------------

/// A procedure's resolved signature.
///
/// Held separately from [`SigEntry`] because checking a *body* needs the
/// parameter types individually, while checking a *call* needs only the
/// interned procedure type.
#[derive(Debug, Clone)]
pub struct ProcSig {
    /// The parameter types, in order. A parameter whose type could not be
    /// resolved is [`PoolId::ERROR`].
    pub params: Vec<PoolId>,
    /// The return type. [`PoolId::VOID`] when the source omitted the arrow —
    /// never `None`, per ADR-0015 §3.
    pub ret: PoolId,
    /// The interned procedure type.
    pub ty: PoolId,
}

// ---------------------------------------------------------------------------
// FileSignatures
// ---------------------------------------------------------------------------

/// The signature-level view of one file.
///
/// Construct with [`file_signatures`](crate::file_signatures). Read by the check
/// phase of the same file and by the check phase of every file that imports it.
#[derive(Debug, Clone, Default)]
pub struct FileSignatures {
    /// Every named file-level declaration.
    names: FxHashMap<jr_base::Symbol, SigEntry>,
    /// Each procedure's resolved signature, keyed by its HIR id.
    procs: FxHashMap<ProcId, ProcSig>,
    /// The resolved field list of every struct declared in this file.
    ///
    /// Kept here as well as in the pool so that the pool dependency is
    /// explicit: a consumer calls [`FileSignatures::record_in`] rather than
    /// relying on some earlier phase having happened to intern them.
    struct_bodies: Vec<(DeclId, Vec<Field>)>,
    /// Display names for the nominal types declared in this file.
    ///
    /// The pool keys struct types on a [`DeclId`], which is deliberately just a
    /// file and an index — it cannot render `Rect`. Diagnostics need to, so the
    /// name is recorded on the side.
    type_names: FxHashMap<PoolId, String>,
}

impl FileSignatures {
    /// Creates an empty set of signatures.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Looks up a name.
    #[must_use]
    pub fn lookup(&self, name: jr_base::Symbol) -> Option<SigEntry> {
        self.names.get(&name).copied()
    }

    /// Returns a procedure's resolved signature.
    #[must_use]
    pub fn proc_sig(&self, proc: ProcId) -> Option<&ProcSig> {
        self.procs.get(&proc)
    }

    /// Returns the display name of a nominal type declared in this file.
    #[must_use]
    pub fn type_name(&self, ty: PoolId) -> Option<&str> {
        self.type_names.get(&ty).map(String::as_str)
    }

    /// The number of named declarations. Non-zero for any file that declares
    /// anything, which is what makes an empty result distinguishable from a
    /// failed one.
    #[must_use]
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Returns `true` if the file declares no names at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Records this file's struct bodies in `pool`.
    ///
    /// Idempotent, and safe to call for a file whose fields are already
    /// recorded: [`Pool::set_struct_fields`] replaces rather than appends.
    /// Call it for the file being checked *and* for every file it imports,
    /// before any field access is typed.
    pub fn record_in(&self, pool: &mut Pool) {
        for (decl, fields) in &self.struct_bodies {
            pool.set_struct_fields(*decl, fields.clone());
        }
    }

    // -----------------------------------------------------------------------
    // Construction (crate-internal)
    // -----------------------------------------------------------------------

    /// Records a named declaration.
    pub(crate) fn insert(&mut self, name: jr_base::Symbol, entry: SigEntry) {
        self.names.insert(name, entry);
    }

    /// Records a procedure's resolved signature.
    pub(crate) fn insert_proc(&mut self, proc: ProcId, sig: ProcSig) {
        self.procs.insert(proc, sig);
    }

    /// Records a struct's resolved field list.
    pub(crate) fn insert_struct_body(&mut self, decl: DeclId, fields: Vec<Field>) {
        self.struct_bodies.push((decl, fields));
    }

    /// Records the display name of a nominal type.
    pub(crate) fn insert_type_name(&mut self, ty: PoolId, name: String) {
        self.type_names.insert(ty, name);
    }
}
