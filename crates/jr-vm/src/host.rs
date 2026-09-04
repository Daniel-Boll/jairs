//! The boundary a **build script** talks to the driver across (ADR-0195 §4).
//!
//! # Why this exists at all
//!
//! A build script is an ordinary Jairs program run in this VM, and it has to tell the driver what to
//! compile. Every other way of saying that was rejected for a recorded reason:
//!
//! - **A `#run` calling the compiler**, Jai's model, needs `#foreign_at_comptime` before it can read
//!   a file — so the script could not do the interesting half of its job (ADR-0195 §3A).
//! - **A separate program printing a manifest** is the text protocol ADR-0102 §3 rejected.
//! - **A new instruction** would make every engine learn a concept only the interpreter can serve,
//!   because a *native* build script would be compiling with a compiler that has already finished.
//!
//! What is left is a call the VM recognises and forwards. `#foreign compiler "set_output"` is that
//! call: the declaration form already exists, so no grammar, no HIR node, no MIR variant, and no
//! change to either native back end. A build script is not something you compile, so a `compiler`
//! library that cannot be linked is exactly right — `jr build` on a file calling one fails at link
//! with `symbol not found`, which is the honest answer.
//!
//! # Why the boundary is scalars and strings only
//!
//! `Build_Options` is a **struct in `modules/Compiler`**, and `set_options` decomposes it into
//! per-field calls that cross here as an integer or a string. The alternative — passing the struct —
//! would put field offsets, `[]string` views and the layout fold on this side, so the compiler would
//! have to know the shape of a library type. It does not, and a reader adding a field to
//! `Build_Options` should not have to touch Rust.
//!
//! That is the same reasoning ADR-0009 uses for the layout seam, applied to a library type: keep the
//! narrow thing narrow, and let Jairs handle Jairs' own aggregates.

/// One argument arriving from a build script.
///
/// Deliberately not `Value`: an implementor lives in `jr-driver` and has no business knowing about
/// the VM's memory regions or aggregate encoding. A `Str` has already been read out of the guest's
/// memory by the time it gets here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostArg {
    /// An integer, sign-extended to a full word. Also carries a `bool`, which arrives as 0 or 1.
    Int(i64),
    /// A `string`, copied out of the guest's memory.
    Str(String),
}

/// What a host call answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostValue {
    /// The procedure returns nothing.
    Void,
    /// An integer or a `bool`; a `bool` is 0 or 1.
    Int(i64),
    /// A `string`. The VM copies the bytes into its own memory and builds the `{data, count}` pair,
    /// so an implementor never allocates on the guest's behalf.
    Str(String),
}

/// Whatever the VM forwards a `#foreign compiler "…"` call to.
///
/// One method rather than one per procedure, because the set of procedures is a *library* decision
/// that will change more often than this trait should: adding `Compiler.set_backend` is a
/// `modules/Compiler` declaration plus an arm in the implementor, and nothing here.
pub trait Host {
    /// Performs one call.
    ///
    /// # Errors
    /// A sentence for the VM to raise as an unsupported-operation trap. Reaching this means the
    /// script asked for something the driver cannot do — an unknown symbol, or a target handle that
    /// was never created — which is a bug in the script or in `modules/Compiler`, so it stops the
    /// script rather than being reported and ignored.
    fn call(&mut self, symbol: &str, args: &[HostArg]) -> Result<HostValue, String>;
}
