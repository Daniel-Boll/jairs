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
    /// Whether it is defined here or imported.
    pub kind: ProcKind,
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
        /// constants — so this comes from the item that declares it, and is
        /// `"jr$<file>$<proc>"` for anything the linker does not need to find by a
        /// fixed name.
        symbol: String,
        /// Whether this is the program's entry point.
        ///
        /// The entry point needs the name the system linker looks for, and it is the
        /// one procedure whose symbol is not ours to choose.
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
/// file, and `entry_symbol` is the name the system linker expects for it.
#[must_use]
pub fn declarations(
    input: &FileInput<'_>,
    pool: &Pool,
    entry: Option<ProcId>,
    entry_symbol: &str,
) -> Vec<ProcDecl> {
    let mut out = Vec::new();
    for index in 0..input.hir.procs.len() {
        let proc = ProcId::from_usize(index);
        let Some(sig) = input.signatures.proc_sig(proc) else {
            continue;
        };
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
            None => {
                let is_entry = entry == Some(proc);
                ProcKind::Local {
                    symbol: if is_entry {
                        entry_symbol.to_owned()
                    } else {
                        symbol_for(input.file, proc)
                    },
                    entry: is_entry,
                }
            }
        };

        out.push(ProcDecl {
            proc: ProcRef::new(input.file, proc),
            params: sig.params.clone(),
            ret: sig.ret,
            kind,
        });
    }
    out
}

/// The symbol name for a procedure the linker need not find by a fixed name.
///
/// Built from the [`ProcRef`] rather than from the source name because a procedure
/// has no name of its own (ADR-0012) and two files may declare the same one. The
/// `jr$` prefix keeps it out of the way of any C symbol.
#[must_use]
pub fn symbol_for(file: jr_base::FileId, proc: ProcId) -> String {
    format!("jr${}${}", file.index(), proc.index())
}
