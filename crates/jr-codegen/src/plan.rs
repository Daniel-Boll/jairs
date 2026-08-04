//! What a back end is told about a procedure before any code is generated.
//!
//! # Why a declaration is a separate type from a signature
//!
//! `jr-sema`'s `ProcSig` answers "what are this procedure's types". A back end also
//! needs to know **what kind of thing it is linking**: a procedure defined in this
//! program, an entry point, or a symbol imported from a library. Those are linkage
//! questions, not typing questions, and `jr-sema` has no business answering them.
//!
//! Keeping them in one type also means the declare phase of ADR-0019 §1 takes a
//! single argument that is complete: an implementation never has to reach back into
//! HIR to discover that the procedure it is declaring is actually `#foreign`.

use jr_hir::ProcId;
use jr_mir::ProcRef;
use jr_pool::{Pool, PoolId};

use crate::FileInput;

/// A procedure's declaration, as the declare phase of ADR-0019 §1 receives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcDecl {
    /// Which procedure this is.
    pub proc: ProcRef,
    /// Parameter types, in order.
    pub params: Vec<PoolId>,
    /// The return type. [`PoolId::VOID`] when the source omitted the arrow, never
    /// absent, per ADR-0015 §3.
    pub ret: PoolId,
    /// Whether it receives the implicit context as a leading hidden parameter (ADR-0057 §3).
    ///
    /// `false` for a `#c_call` or `#foreign` procedure. The back end reads this rather than
    /// recomputing it, so the signature and the MIR body agree about the parameter count — the
    /// argument shift ADR-0053 §1 records is what a disagreement produces.
    pub receives_context: bool,
    /// Whether it is defined here or imported.
    pub kind: ProcKind,
    /// The procedure's **source** name, for a backtrace frame (ADR-0066 §3).
    ///
    /// Distinct from [`ProcKind::Local::symbol`], which is the mangled `jr$<file>$<proc>` a linker
    /// sees: a reader of a backtrace wants `countdown`, not `jr$0$3`. A procedure carries no name of
    /// its own (ADR-0012), so this is the name of the *item* that binds it — found here, where the HIR
    /// is at hand, rather than by a back end that has no HIR to ask.
    ///
    /// `None` for a procedure no item binds, whose frame is then omitted from a backtrace rather than
    /// printed as a placeholder.
    pub name: Option<String>,
}

/// Whether a procedure is defined in this program or imported from a library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcKind {
    /// Defined in this program; [`Backend::define`](crate::Backend::define) will be
    /// called for it.
    Local {
        /// The symbol to emit.
        ///
        /// A procedure carries no name of its own — ADR-0012 makes procedures
        /// constants — so this is `"jr$<file>$<proc>"`, built from the identity
        /// instead. *Every* Jairs procedure gets a mangled name, the entry point
        /// included: see the `entry` field.
        symbol: String,
        /// Whether this is the program's entry point.
        ///
        /// The entry point does **not** take the linker's name for itself. A Jairs
        /// `main` returns `void`, and a C `main` that returns nothing leaves the
        /// process status to whatever happened to be in the return register — the
        /// first native run of `024-hello.jr` printed both its lines correctly and
        /// then exited 1. So a back end emits a small `main` shim that calls this
        /// procedure and returns a real status, and this flag is how it learns which
        /// procedure the shim should call.
        entry: bool,
    },
    /// Imported through `#foreign`; no body will be defined.
    Foreign(ForeignSymbol),
}

/// A `#foreign` procedure's symbol and library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignSymbol {
    /// The symbol as written in the `#foreign` declaration, e.g. `"write"`.
    pub symbol: String,
    /// The library it lives in, e.g. `"c"`, resolved once by `jr-sema` and read
    /// from the pool (ADR-0019 §4).
    ///
    /// `None` when the declaration named no library, or named something that is not
    /// a `#system_library` — already an E0225. A back end must **refuse** rather
    /// than default to a likely library: guessing emits a link against a library the
    /// source never named, which is a build that succeeds for the wrong reason.
    pub library: Option<String>,
}

/// Every procedure declaration one file contributes.
///
/// The shared fold ADR-0019 §1's declare phase needs, so that `jr-codegen-clif` and
/// the eventual `jr-codegen-llvm` cannot disagree about what a program contains.
/// Both `#foreign` imports and locally defined procedures are returned, because both
/// must be declared before any body is defined.
///
/// A procedure whose signature `jr-sema` never recorded is skipped: it failed to
/// type-check, so ADR-0017 §4 refused its body and there is nothing to define.
///
/// `entry` names the procedure that is the program's entry point, if it is in this
/// file. It is flagged rather than renamed; see [`ProcKind::Local`].
#[must_use]
pub fn declarations(input: &FileInput<'_>, pool: &Pool, entry: Option<ProcId>) -> Vec<ProcDecl> {
    let mut out = Vec::new();
    for index in 0..input.hir.procs.len() {
        let proc = ProcId::from_usize(index);
        let Some(sig) = input.signatures.proc_sig(proc) else {
            continue;
        };
        // **A polymorphic procedure is not declared** (ADR-0081 §2): its `$T` parameters have no concrete
        // type — `sig.params` holds `PoolId::ERROR` for each — so building a native signature for it would
        // try to lay out a poisoned type and fail. It is a *template*; the instantiation sub-wave declares
        // a concrete copy per call. Skipped exactly as `jr-mir` skips its body and as a `#foreign` with no
        // symbol is skipped here, and keyed on the same `poly_vars` the body skip and the call refusal use.
        if !sig.poly_vars.is_empty() {
            continue;
        }
        let data = &input.hir.procs[index];

        let kind = match &data.foreign {
            Some(info) => {
                let Some(symbol) = info.symbol.clone() else {
                    // `#foreign` with no symbol string is a syntax error already
                    // reported; leaving it undeclared means a call to it fails to
                    // link by name rather than linking to something arbitrary.
                    continue;
                };
                ProcKind::Foreign(ForeignSymbol {
                    symbol,
                    library: input
                        .signatures
                        .foreign_library(proc)
                        .and_then(|id| pool.foreign_library_name(id))
                        .map(str::to_owned),
                })
            }
            None => ProcKind::Local {
                symbol: symbol_for(input.file, proc),
                entry: entry == Some(proc),
            },
        };

        out.push(ProcDecl {
            proc: ProcRef::new(input.file, proc),
            params: sig.params.clone(),
            ret: sig.ret,
            // From the HIR's own flags, the same two `jr-mir` reads — a `#c_call` or `#foreign`
            // procedure takes no context (ADR-0057 §3).
            receives_context: !(data.c_call || data.foreign.is_some()),
            kind,
            // The source name for a backtrace frame (ADR-0066 §3), resolved by the caller because
            // turning a `Symbol` into text needs the interner.
            name: input.names.get(index).cloned().flatten(),
        });
    }
    out
}

/// The symbol name for a Jairs procedure.
///
/// Built from the [`ProcRef`] rather than from the source name because a procedure
/// has no name of its own (ADR-0012) and two files may declare the same one. The
/// `jr$` prefix keeps it out of the way of any C symbol — including `main`, which a
/// back end emits as a shim rather than as a Jairs procedure.
#[must_use]
pub fn symbol_for(file: jr_base::FileId, proc: ProcId) -> String {
    format!("jr${}${}", file.index(), proc.index())
}
