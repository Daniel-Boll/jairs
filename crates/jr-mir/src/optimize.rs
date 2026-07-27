//! The optimisation pipeline: which passes run, in what order, and how many times.
//!
//! [ADR-0022](../../../docs/adr/0022-dce-constprop-shared-arithmetic.md) §3 and §6.
//!
//! # Why the order lives here and not in the query
//!
//! `jr-db`'s `optimized_file_mir` used to call [`crate::inline_body`] directly, and
//! adding a second pass forced the question. The split ADR-0022 §3 took is that
//! `jr-db` owns the **policy** — ADR-0021 §2's decision about *which bodies* may be
//! rewritten at all, which is a query's business because the `#run` closure is — and
//! this module owns the **sequence**.
//!
//! That is not tidiness. The frozen-set check is the one rule in the mid-end whose
//! violation is a silent comptime/runtime divergence rather than a visibly wrong
//! answer, and it should not be one item in a growing list of calls that a future pass
//! gets appended to in the wrong place.
//!
//! # Why it iterates, and why the iteration is capped
//!
//! The cascade between the two new passes is real. [`crate::const_prop`] folds a
//! branch on a constant condition, which costs the untaken arm its last predecessor;
//! [`crate::dce`] then deletes it; that can leave a surviving block with one
//! predecessor, whose parameter `const_prop` can now collapse. A single pass over the
//! three would leave that on the floor.
//!
//! The cap follows `jr-db`'s `file_consts`, which iterates lower-then-evaluate under
//! its own `MAX_ROUNDS` and gives the reason this one borrows: a bound rather than
//! "until stable" means a bug in a pass's change-reporting is a diagnosable stop
//! instead of a hang. This loop runs inside a salsa query, so a hang is a hung editor.
//!
//! Reporting a change that did not happen wastes a round. Failing to report one loses
//! an optimisation. Neither is a wrong answer, which is what makes the bound safe to
//! rely on rather than merely convenient.

use jr_pool::Pool;

use crate::constprop::const_prop;
use crate::dce::dce;
use crate::forward::forward_stores;
use crate::inline::{Callees, inline_body};
use crate::mir::MirBody;

/// How many times the pipeline may run before it gives up on reaching a fixed point.
///
/// A bound rather than "until stable", for the reason the module docs give. Sized so
/// that a genuine cascade — fold a branch, delete a block, collapse a parameter, fold
/// again — has room several times over, while a pass that lies about having changed
/// something stops rather than hangs.
pub const MAX_OPT_ROUNDS: usize = 8;

/// What one call to [`optimize`] did.
///
/// Returned rather than logged because a test wants to assert on it and a future `-Z`
/// flag will want to print it. Counting *rounds* as well as splices is what makes a
/// pass that never converges visible: a body that used every round is either genuinely
/// deep or has a pass reporting spurious change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OptStats {
    /// How many call sites were inlined.
    pub inlined: usize,
    /// How many rounds ran, including the final one that changed nothing.
    pub rounds: usize,
    /// Whether the pipeline stopped because it hit [`MAX_OPT_ROUNDS`].
    ///
    /// Not an error: the body is correct either way, merely less optimised than it
    /// might be. It is reported so that "we ran out of rounds" is distinguishable from
    /// "we converged", which is the difference between a missing optimisation and a
    /// misbehaving pass.
    pub exhausted: bool,
}

/// Runs every optimisation pass over `body` until it stops changing.
///
/// The caller decides whether `body` may be optimised at all: ADR-0021 §2 freezes
/// every body compile-time evaluation can reach, and this function will happily
/// rewrite one it is handed.
///
/// # Panics
/// In a debug build, if any pass leaves the body malformed. Each pass verifies its own
/// output, so the panic names the pass rather than the pipeline.
pub fn optimize(body: &mut MirBody, callees: &Callees<'_>, pool: &mut Pool) -> OptStats {
    let mut stats = OptStats::default();
    for round in 1..=MAX_OPT_ROUNDS {
        stats.rounds = round;
        // Inlining first, and every round rather than once: a call exposed by folding
        // a branch away is a call the first round never saw.
        let inlined = inline_body(body, callees, pool);
        stats.inlined += inlined;

        let mut changed = inlined > 0;
        // Forwarding before const-prop, because it is what turns a value that lives in
        // memory into an operand const-prop can see. ADR-0023's whole point is that
        // without it `024-hello.jr` folds nothing: its `Point` is a slot, and a slot is
        // opaque to every other pass here.
        changed |= forward_stores(body, pool);
        // Const-prop before DCE, because it is the one that *creates* dead code —
        // a folded branch is what makes a block unreachable.
        changed |= const_prop(body, pool);
        changed |= dce(body, pool);

        if !changed {
            return stats;
        }
    }
    stats.exhausted = true;
    stats
}
