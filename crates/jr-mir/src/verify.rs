//! The validator that makes a bad lowering loud.
//!
//! # Why this exists at all
//!
//! ADR-0017 §4 refuses to lower a poisoned body, and a refusal is only as good as
//! the check behind it. The failure this module exists to prevent is the one named
//! in that ADR: MIR silently built from poison, which is a miscompile — a wrong
//! binary rather than an error message. A `Result` at the query boundary stops a
//! *caller* from ignoring poison; this stops *lowering* from manufacturing it.
//!
//! The check is affordable because types are interned (ADR-0015). Asking whether
//! a value is poisoned is one integer comparison against [`PoolId::ERROR`], not a
//! structural walk, so verifying every value, slot, operand and terminator in a
//! body is linear in the body with a tiny constant.
//!
//! # Why findings are returned rather than panicked on
//!
//! [`verify`] never panics and never emits a diagnostic. It returns what it
//! found, which lets a test assert on a *specific* violation instead of catching
//! a panic and hoping it was the right one — and it lets [`assert_valid`] compose
//! all the findings into one message rather than aborting on the first. The
//! rejected alternative was to assert inline at each check, which reports one
//! violation per run and makes a genuinely broken lowering an exercise in
//! whack-a-mole.
//!
//! # Why it is not a dominance checker
//!
//! This wave checks that every used value is defined *somewhere* and that block
//! parameters and edge arguments agree in arity. It does **not** check that a
//! definition dominates its uses. That needs a dominator tree, which ADR-0017 §2
//! deliberately avoids building because Braun's construction does not need one.
//! The claim made here is therefore weaker than "this MIR is well-formed SSA",
//! and it is stated rather than implied so nobody later mistakes a passing verify
//! for a dominance guarantee.
//!
//! Layout-dependent checks are absent for the reason ADR-0017 §5 gives: nothing
//! in this crate knows a size, an alignment or an offset, so there is nothing
//! here to check them against.

use jr_pool::{Item, Pool, PoolId};

use crate::mir::{
    BinOp, BlockId, Callee, MirBody, MirSpan, Operand, Place, PlaceBase, Projection, Rvalue,
    Statement, Terminator, UnOp,
};

// ---------------------------------------------------------------------------
// Findings
// ---------------------------------------------------------------------------

/// One way in which a body is malformed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Finding {
    /// The block the violation was found in, when it belongs to one.
    pub(crate) block: Option<BlockId>,
    /// A short, stable description of the rule that was broken.
    pub(crate) rule: &'static str,
    /// The specifics, for a human reading an assertion message.
    pub(crate) detail: String,
}

impl Finding {
    fn new(block: Option<BlockId>, rule: &'static str, detail: String) -> Self {
        Self {
            block,
            rule,
            detail,
        }
    }
}

impl core::fmt::Display for Finding {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.block {
            Some(block) => write!(f, "bb{}: {}: {}", block.index(), self.rule, self.detail),
            None => write!(f, "{}: {}", self.rule, self.detail),
        }
    }
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Checks `body` and returns every violation found.
///
/// Never panics, even on a body whose ids are out of range — the range checks
/// run before anything is dereferenced, precisely so that a malformed body
/// produces a finding rather than an index panic.
#[must_use]
pub(crate) fn verify(body: &MirBody, pool: &Pool) -> Vec<Finding> {
    let mut v = Verifier {
        body,
        pool,
        findings: Vec::new(),
    };
    v.run();
    v.findings
}

/// Panics if `body` is malformed, listing every violation.
///
/// Called by lowering so that a violation is a test failure. The work is gated
/// on `debug_assertions`, so a release build of the compiler pays nothing.
pub(crate) fn assert_valid(body: &MirBody, pool: &Pool) {
    if cfg!(debug_assertions) {
        let findings = verify(body, pool);
        assert!(
            findings.is_empty(),
            "malformed MIR for {:?}:\n{}",
            body.proc(),
            findings
                .iter()
                .map(|f| format!("  {f}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

// ---------------------------------------------------------------------------
// Verifier
// ---------------------------------------------------------------------------

struct Verifier<'a> {
    body: &'a MirBody,
    pool: &'a Pool,
    findings: Vec<Finding>,
}

impl Verifier<'_> {
    fn report(&mut self, block: Option<BlockId>, rule: &'static str, detail: String) {
        self.findings.push(Finding::new(block, rule, detail));
    }

    fn run(&mut self) {
        self.check_signature();
        self.check_arenas();
        self.check_definitions();
        self.check_edges();
        self.check_types();
        self.check_spans();
    }

    // -------------------------------------------------------------------
    // Provenance
    // -------------------------------------------------------------------

    /// Every [`MirSpan`] that names a procedure names *this* one.
    ///
    /// This is the checkable fragment of ADR-0021 §3. A span copied out of an
    /// inlined callee names arenas belonging to the callee's file, and
    /// [`crate::resolve_span`] is handed the caller's `FileHir`, so a survivor
    /// resolves to a plausible wrong line rather than to nothing — the worst
    /// available outcome for a diagnostic.
    ///
    /// Only [`MirSpan::Param`] can be checked, and the ADR says so rather than
    /// implying more: an `ExprId`, a `BodyId` and an `ExprScope` carry no `FileId`,
    /// and a [`MirBody`] does not store the `BodyId` it was lowered from, so a
    /// foreign `Expr` span is indistinguishable from a native one here. The real
    /// guarantee for those is structural — `inline.rs` writes every copied span
    /// through one nullary function — and this check is the cheap corroboration,
    /// not the mechanism.
    fn check_spans(&mut self) {
        let own = self.body.proc().proc;
        let mut foreign: Vec<String> = Vec::new();
        for index in 0..self.body.value_count() {
            let span = self.body.value(crate::mir::ValueId::from_usize(index)).span;
            if let MirSpan::Param(proc, _) = span
                && proc != own
            {
                foreign.push(format!("v{index} is a parameter of {proc:?}"));
            }
        }
        for index in 0..self.body.slot_count() {
            let span = self.body.slot(crate::mir::SlotId::from_usize(index)).span;
            if let MirSpan::Param(proc, _) = span
                && proc != own
            {
                foreign.push(format!("s{index} is a parameter of {proc:?}"));
            }
        }
        for detail in foreign {
            self.report(None, "foreign span", detail);
        }

        for index in 0..self.body.block_count() {
            let block = BlockId::from_usize(index);
            let mut details: Vec<String> = Vec::new();
            for (position, stmt) in self.body.block(block).stmts.iter().enumerate() {
                let span = match stmt {
                    Statement::Assign { span, .. }
                    | Statement::Store { span, .. }
                    | Statement::Discard { span, .. }
                    | Statement::Zero { span, .. }
                    | Statement::BoundsCheck { span, .. }
                    | Statement::TagCheck { span, .. } => *span,
                    Statement::Nop => continue,
                };
                if let MirSpan::Param(proc, _) = span
                    && proc != own
                {
                    details.push(format!("statement {position} is spanned by {proc:?}"));
                }
            }
            for detail in details {
                self.report(Some(block), "foreign span", detail);
            }
        }
    }

    // -------------------------------------------------------------------
    // Structure
    // -------------------------------------------------------------------

    /// The body's own header: return type, entry block, and parameters.
    fn check_signature(&mut self) {
        if self.body.ret() == PoolId::ERROR {
            self.report(
                None,
                "poisoned return type",
                "the return type is <unknown>".to_owned(),
            );
        }
        if self.body.entry().index() >= self.body.block_count() {
            self.report(
                None,
                "entry out of range",
                format!("entry is bb{}", self.body.entry().index()),
            );
            return;
        }
        let entry_params = &self.body.block(self.body.entry()).params;
        if entry_params != self.body.params() {
            self.report(
                Some(self.body.entry()),
                "entry parameters disagree",
                format!(
                    "body declares {:?} but the entry block declares {entry_params:?}",
                    self.body.params()
                ),
            );
        }
    }

    /// Value and slot types, and the range of every id mentioned anywhere.
    fn check_arenas(&mut self) {
        for index in 0..self.body.value_count() {
            let ty = self.body.value(crate::mir::ValueId::from_usize(index)).ty;
            if ty == PoolId::ERROR {
                self.report(
                    None,
                    "poisoned value",
                    format!("v{index} has type <unknown>"),
                );
            }
            self.check_pool_id(None, ty, &format!("v{index}"));
        }
        for index in 0..self.body.slot_count() {
            let ty = self.body.slot(crate::mir::SlotId::from_usize(index)).ty;
            if ty == PoolId::ERROR {
                self.report(
                    None,
                    "poisoned slot",
                    format!("s{index} has type <unknown>"),
                );
            }
            self.check_pool_id(None, ty, &format!("s{index}"));
        }

        for (index, block) in self.body.blocks().iter().enumerate() {
            let at = BlockId::from_usize(index);
            for stmt in &block.stmts {
                self.check_statement_ids(at, stmt);
            }
            self.check_terminator_ids(at, &block.term);
        }
    }

    fn check_pool_id(&mut self, at: Option<BlockId>, id: PoolId, what: &str) {
        if id.index() >= self.pool.len() {
            self.report(
                at,
                "pool id out of range",
                format!("{what} names pool entry {}", id.index()),
            );
        }
    }

    fn check_statement_ids(&mut self, at: BlockId, stmt: &Statement) {
        match stmt {
            Statement::Assign {
                dest,
                rvalue,
                span: _,
            } => {
                if dest.index() >= self.body.value_count() {
                    self.report(Some(at), "value out of range", format!("v{}", dest.index()));
                }
                self.check_rvalue_ids(at, rvalue);
            }
            Statement::Store {
                place,
                value,
                span: _,
            } => {
                self.check_place_ids(at, place);
                self.check_operand_ids(at, *value);
            }
            Statement::Discard { rvalue, span: _ } => self.check_rvalue_ids(at, rvalue),
            Statement::Zero { place, span: _ } => self.check_place_ids(at, place),
            Statement::BoundsCheck {
                index,
                len,
                span: _,
            } => {
                self.check_operand_ids(at, *index);
                self.check_operand_ids(at, *len);
            }
            Statement::TagCheck { place, .. } => self.check_place_ids(at, place),
            Statement::Nop => {}
        }
    }

    fn check_rvalue_ids(&mut self, at: BlockId, rvalue: &Rvalue) {
        match rvalue {
            Rvalue::Atomic {
                op: _,
                address,
                value,
                expected,
            } => {
                self.check_operand_ids(at, *address);
                if let Some(value) = value {
                    self.check_operand_ids(at, *value);
                }
                if let Some(expected) = expected {
                    self.check_operand_ids(at, *expected);
                }
            }
            Rvalue::Use(operand) => self.check_operand_ids(at, *operand),
            Rvalue::Binary { op: _, lhs, rhs } => {
                self.check_operand_ids(at, *lhs);
                self.check_operand_ids(at, *rhs);
            }
            Rvalue::Unary { op: _, operand } => self.check_operand_ids(at, *operand),
            Rvalue::Convert { operand, from: _ } => self.check_operand_ids(at, *operand),
            Rvalue::Call { callee, args } => {
                match callee {
                    Callee::Direct(_) => {}
                    Callee::Indirect(operand) => self.check_operand_ids(at, *operand),
                }
                for arg in args {
                    self.check_operand_ids(at, *arg);
                }
            }
            Rvalue::Load(place) | Rvalue::Address(place) => self.check_place_ids(at, place),
            Rvalue::Undef => {}
        }
    }

    fn check_place_ids(&mut self, at: BlockId, place: &Place) {
        match &place.base {
            PlaceBase::Slot(slot) => {
                if slot.index() >= self.body.slot_count() {
                    self.report(Some(at), "slot out of range", format!("s{}", slot.index()));
                }
            }
            PlaceBase::Deref(operand) => self.check_operand_ids(at, *operand),
            // **A global carries no id this body can range-check** (ADR-0186 §3). A slot index is
            // checked against `slot_count` because a stale index reads another slot; a `GlobalRef`
            // is a `(FileId, ItemId)` pair, and whether that item really is a global is decided when
            // the place is *built*, against the declaring file's HIR. Re-deciding it here would need
            // this crate to hold every file's items, which is the cross-body dependency ADR-0017 §3
            // keeps out of the built-MIR query.
            PlaceBase::Global(_) => {}
        }
        // An index operand is a `ValueId` like any other and can dangle like any other —
        // which is the whole reason `dce.rs`, `ssa.rs` and `inline.rs` all had to learn to
        // walk it. This is the check that would catch one of them forgetting.
        for projection in &place.projection {
            match projection {
                Projection::Index(operand) => self.check_operand_ids(at, *operand),
                Projection::Field(_)
                | Projection::Deref
                | Projection::StringData
                | Projection::StringCount
                | Projection::ViewData
                | Projection::ViewCount
                | Projection::DynamicArrayData
                | Projection::DynamicArrayCount
                | Projection::DynamicArrayCapacity
                | Projection::VariantTag => {}
            }
        }
    }

    fn check_operand_ids(&mut self, at: BlockId, operand: Operand) {
        match operand {
            Operand::Value(value) => {
                if value.index() >= self.body.value_count() {
                    self.report(
                        Some(at),
                        "value out of range",
                        format!("v{}", value.index()),
                    );
                }
            }
            Operand::Constant(id) => {
                if id == PoolId::ERROR {
                    self.report(
                        Some(at),
                        "poisoned constant",
                        "a constant is <unknown>".to_owned(),
                    );
                }
                self.check_pool_id(Some(at), id, "a constant");
            }
        }
    }

    fn check_terminator_ids(&mut self, at: BlockId, term: &Terminator) {
        match term {
            Terminator::Goto(_) | Terminator::Branch { .. } => {
                if let Terminator::Branch { cond, .. } = term {
                    self.check_operand_ids(at, *cond);
                }
                for target in term.targets() {
                    if target.block.index() >= self.body.block_count() {
                        self.report(
                            Some(at),
                            "block out of range",
                            format!("bb{}", target.block.index()),
                        );
                    }
                    for arg in &target.args {
                        self.check_operand_ids(at, *arg);
                    }
                }
            }
            Terminator::Return(operand) => {
                if let Some(operand) = operand {
                    self.check_operand_ids(at, *operand);
                }
            }
            Terminator::Unreachable(_) => {}
        }
    }

    // -------------------------------------------------------------------
    // SSA
    // -------------------------------------------------------------------

    /// Every value that is *used* is defined, and no value is defined twice.
    ///
    /// A value that is declared, never defined and never used is deliberately
    /// **not** a finding. Collapsing a trivial block parameter (see `ssa.rs`)
    /// removes it from its block and rewrites its uses, but cannot reclaim its
    /// index without renumbering the whole arena — so a dead declaration is a
    /// normal residue of construction, exactly as it is in rustc and Cranelift.
    /// What must never happen is a *use* of such a value, which is what this
    /// catches: it is the failure mode of holding an operand across a seal.
    ///
    /// Deliberately weaker than dominance — see the module docs.
    fn check_definitions(&mut self) {
        let mut defined = vec![0u32; self.body.value_count()];
        let mut used = vec![false; self.body.value_count()];

        for block in self.body.blocks() {
            for param in &block.params {
                if let Some(count) = defined.get_mut(param.index()) {
                    *count += 1;
                }
            }
            for stmt in &block.stmts {
                match stmt {
                    Statement::Assign {
                        dest,
                        rvalue,
                        span: _,
                    } => {
                        if let Some(count) = defined.get_mut(dest.index()) {
                            *count += 1;
                        }
                        mark_rvalue(rvalue, &mut used);
                    }
                    Statement::Discard { rvalue, span: _ } => mark_rvalue(rvalue, &mut used),
                    Statement::Store {
                        place,
                        value,
                        span: _,
                    } => {
                        mark_place(place, &mut used);
                        mark_operand(*value, &mut used);
                    }
                    Statement::Zero { place, span: _ } => mark_place(place, &mut used),
                    Statement::BoundsCheck {
                        index,
                        len,
                        span: _,
                    } => {
                        mark_operand(*index, &mut used);
                        mark_operand(*len, &mut used);
                    }
                    Statement::TagCheck { place, .. } => mark_place(place, &mut used),
                    Statement::Nop => {}
                }
            }
            match &block.term {
                Terminator::Goto(_) | Terminator::Branch { .. } => {
                    if let Terminator::Branch { cond, .. } = &block.term {
                        mark_operand(*cond, &mut used);
                    }
                    for target in block.term.targets() {
                        for arg in &target.args {
                            mark_operand(*arg, &mut used);
                        }
                    }
                }
                Terminator::Return(operand) => {
                    if let Some(operand) = operand {
                        mark_operand(*operand, &mut used);
                    }
                }
                Terminator::Unreachable(_) => {}
            }
        }

        // A parameter of the entry block is the procedure's parameter: it is
        // defined by the call, so it counts as used for this purpose.
        for param in self.body.params() {
            if let Some(flag) = used.get_mut(param.index()) {
                *flag = true;
            }
        }

        for (index, count) in defined.iter().enumerate() {
            match count {
                0 => {
                    if used[index] {
                        self.report(
                            None,
                            "value never defined",
                            format!("v{index} is used but nothing defines it"),
                        );
                    }
                }
                1 => {}
                many => self.report(
                    None,
                    "value defined more than once",
                    format!("v{index} is defined {many} times"),
                ),
            }
        }
    }

    // -------------------------------------------------------------------
    // Edges
    // -------------------------------------------------------------------

    /// Edge arity, and ADR-0017 §1's no-critical-edges invariant.
    fn check_edges(&mut self) {
        let predecessors: Vec<usize> = self
            .body
            .predecessors()
            .iter()
            .map(std::vec::Vec::len)
            .collect();

        for (index, block) in self.body.blocks().iter().enumerate() {
            let at = BlockId::from_usize(index);
            let successors = block.term.targets().len();
            let edges: Vec<(BlockId, usize)> = block
                .term
                .targets()
                .iter()
                .map(|target| (target.block, target.args.len()))
                .collect();

            for (target, args) in edges {
                if target.index() >= self.body.block_count() {
                    continue; // already reported by check_arenas
                }
                let wanted = self.body.block(target).params.len();
                if args != wanted {
                    self.report(
                        Some(at),
                        "edge arity disagrees",
                        format!(
                            "the edge to bb{} supplies {args} argument(s) but that block \
                             declares {wanted} parameter(s)",
                            target.index()
                        ),
                    );
                }
                let target_predecessors = predecessors.get(target.index()).copied().unwrap_or(0);
                if successors > 1 && target_predecessors > 1 {
                    self.report(
                        Some(at),
                        "critical edge",
                        format!(
                            "bb{} has {successors} successors and bb{} has {target_predecessors} \
                             predecessors; the edge must be split",
                            index,
                            target.index()
                        ),
                    );
                }
            }
        }
    }

    // -------------------------------------------------------------------
    // Types
    // -------------------------------------------------------------------

    /// The type of an operand, or `None` if it cannot be determined safely.
    fn operand_type(&self, operand: Operand) -> Option<PoolId> {
        match operand {
            Operand::Value(value) => {
                (value.index() < self.body.value_count()).then(|| self.body.value(value).ty)
            }
            Operand::Constant(id) => (id.index() < self.pool.len()).then(|| self.pool.type_of(id)),
        }
    }

    fn is_pointer(&self, ty: PoolId) -> bool {
        if ty.index() >= self.pool.len() {
            return false;
        }
        match self.pool.item(ty) {
            Item::PointerType(_) => true,
            Item::VoidType
            | Item::BoolType
            | Item::IntType { .. }
            | Item::StringType
            | Item::TypeType
            | Item::ErrorType
            | Item::ForeignLibraryType
            | Item::ArrayType { .. }
            | Item::VectorType { .. }
            | Item::ViewType { .. }
            | Item::DynamicArrayType { .. }
            | Item::ResultsType { .. }
            | Item::ContextType
            | Item::FloatType { .. }
            | Item::EnumType { .. }
            | Item::StructType { .. }
            | Item::UnionType { .. }
            | Item::VariantType { .. }
            | Item::ProcType { .. }
            | Item::VoidValue
            | Item::BoolValue(_)
            | Item::IntValue { .. }
            | Item::FloatValue { .. }
            | Item::StaticArray { .. }
            | Item::StrValue(_)
            | Item::TypeValue(_)
            | Item::ProcValue { .. }
            | Item::ForeignLibraryValue(_, _)
            // A value, not a type: this predicate asks about types (ADR-0074 §1).
            | Item::AggregateValue { .. } => false,
        }
    }

    /// Whether `ty` is an `enum_flags` type (ADR-0043 §3).
    ///
    /// Kept separate from [`Self::is_integer`], which several checks still want on its own —
    /// a bounds check's operands must be integers specifically (ADR-0039 §1), and a plain
    /// enum must *not* pass here.
    fn is_flags_enum(&self, ty: PoolId) -> bool {
        if ty.index() >= self.pool.len() {
            return false;
        }
        matches!(self.pool.item(ty), Item::EnumType { flags: true, .. })
    }

    /// Whether `ty` is an integer or a float type.
    ///
    /// Separate from [`Self::is_integer`], which several checks still want on its own — a
    /// bounds check's operands must be integers specifically (ADR-0039 §1).
    fn is_numeric(&self, ty: PoolId) -> bool {
        self.is_integer(ty) || jr_pool::FloatKind::of(self.pool, ty).is_some()
    }

    fn is_integer(&self, ty: PoolId) -> bool {
        if ty.index() >= self.pool.len() {
            return false;
        }
        match self.pool.item(ty) {
            Item::IntType { .. } => true,
            Item::VoidType
            | Item::BoolType
            | Item::StringType
            | Item::TypeType
            | Item::ErrorType
            | Item::ForeignLibraryType
            | Item::PointerType(_)
            | Item::ArrayType { .. }
            | Item::VectorType { .. }
            | Item::ViewType { .. }
            | Item::DynamicArrayType { .. }
            | Item::ResultsType { .. }
            | Item::ContextType
            | Item::FloatType { .. }
            | Item::EnumType { .. }
            | Item::StructType { .. }
            | Item::UnionType { .. }
            | Item::VariantType { .. }
            | Item::ProcType { .. }
            | Item::VoidValue
            | Item::BoolValue(_)
            | Item::IntValue { .. }
            | Item::FloatValue { .. }
            | Item::StaticArray { .. }
            | Item::StrValue(_)
            | Item::TypeValue(_)
            | Item::ProcValue { .. }
            | Item::ForeignLibraryValue(_, _)
            // A value, not a type: this predicate asks about types (ADR-0074 §1).
            | Item::AggregateValue { .. } => false,
        }
    }

    /// The type checks that need no layout knowledge.
    fn check_types(&mut self) {
        for (index, block) in self.body.blocks().iter().enumerate() {
            let at = BlockId::from_usize(index);
            for stmt in &block.stmts {
                match stmt {
                    Statement::Assign {
                        dest,
                        rvalue,
                        span: _,
                    } => {
                        let dest_ty = (dest.index() < self.body.value_count())
                            .then(|| self.body.value(*dest).ty);
                        self.check_rvalue_types(at, dest_ty, rvalue);
                    }
                    Statement::Discard { rvalue, span: _ } => {
                        self.check_rvalue_types(at, None, rvalue);
                    }
                    Statement::Zero { place, span: _ } => self.check_place_types(at, place),
                    // Both operands must be integers. This is a real check rather than a
                    // formality: the comparison is unsigned (ADR-0039 §1), so a `bool` or a
                    // pointer reaching here would be compared as though it were a width
                    // the back end picked, and the check would pass or fail for reasons
                    // unrelated to the index.
                    Statement::BoundsCheck {
                        index,
                        len,
                        span: _,
                    } => {
                        for (operand, which) in [(index, "index"), (len, "length")] {
                            if let Some(ty) = self.operand_type(*operand)
                                && !self.is_integer(ty)
                            {
                                self.report(
                                    Some(at),
                                    "bounds check on a non-integer",
                                    format!("the {which} of a bounds check must be an integer"),
                                );
                            }
                        }
                    }
                    Statement::Store {
                        place,
                        value,
                        span: _,
                    } => {
                        self.check_place_types(at, place);
                        let _ = value; // the stored type needs layout to check; see module docs
                    }
                    // The place must be well-typed; the *case* is an index into the variant's field
                    // list, which needs the layout this module deliberately does not have (ADR-0017
                    // §5), so an out-of-range case is `jr-pool`'s `NotAType` at the offset lookup
                    // rather than a check here.
                    Statement::TagCheck { place, .. } => self.check_place_types(at, place),
                    Statement::Nop => {}
                }
            }
            self.check_terminator_types(at, &block.term);
        }
    }

    fn check_rvalue_types(&mut self, at: BlockId, dest: Option<PoolId>, rvalue: &Rvalue) {
        match rvalue {
            // The one check that matters for a conversion: `from` is *recorded* rather than
            // recovered (see `Rvalue::Convert`), so nothing but this stops it drifting from
            // the operand it describes. A wrong `from` sign-extends where it should
            // zero-extend and produces a wrong *number* — no poison, no type error, exactly
            // the silent miscompile shape `PLAN.md` §5 names.
            Rvalue::Convert { operand, from } => {
                if let Some(source) = self.operand_type(*operand)
                    && let Some(actual) = crate::mir::NumKind::of(self.pool, source)
                    && actual != *from
                {
                    self.report(
                        Some(at),
                        "convert disagrees about its source",
                        format!(
                            "recorded `from` is {} but the operand is {}",
                            from.name(),
                            actual.name()
                        ),
                    );
                }
                if let Some(dest) = dest
                    && crate::mir::NumKind::of(self.pool, dest).is_none()
                {
                    self.report(
                        Some(at),
                        "convert to a non-integer",
                        "a conversion's destination must be an integer type".to_owned(),
                    );
                }
            }
            // **An atomic's shapes, checked here because nothing downstream can** (ADR-0176 §2). Both
            // back ends and the VM take the address as a pointer and the value as a 64-bit integer; a
            // wrong lowering would hand one of them a `bool` or a struct and produce a wrong *store*
            // rather than a type error, which is the silent-miscompile shape §5 names.
            Rvalue::Atomic {
                op,
                address,
                value,
                expected,
            } => {
                if let Some(ty) = self.operand_type(*address)
                    && !matches!(self.pool.item(ty), jr_pool::Item::PointerType(_))
                {
                    self.report(
                        Some(at),
                        "atomic on a non-pointer",
                        "an atomic operates through a pointer".to_owned(),
                    );
                }
                // The operand set is decided by the operation, and a mismatch means the lowering built a
                // shape no engine has an arm for.
                let wants_value = !matches!(op, crate::mir::AtomicOp::Load);
                let wants_expected = matches!(op, crate::mir::AtomicOp::CompareExchange);
                if wants_value != value.is_some() || wants_expected != expected.is_some() {
                    self.report(
                        Some(at),
                        "atomic with the wrong operands",
                        format!("`{}` does not take these operands", op.name()),
                    );
                }
                // A store yields `void`; every other operation yields something, and a compare-exchange
                // yields a `bool` rather than a number.
                if let Some(dest) = dest {
                    let expected_dest = match op {
                        crate::mir::AtomicOp::Store => jr_pool::PoolId::VOID,
                        crate::mir::AtomicOp::CompareExchange => jr_pool::PoolId::BOOL,
                        crate::mir::AtomicOp::Load | crate::mir::AtomicOp::Add => dest,
                    };
                    if dest != expected_dest {
                        self.report(
                            Some(at),
                            "atomic result of the wrong type",
                            format!("`{}` does not produce this type", op.name()),
                        );
                    }
                }
            }
            Rvalue::Use(operand) => {
                if let (Some(dest), Some(source)) = (dest, self.operand_type(*operand))
                    && dest != source
                {
                    self.report(
                        Some(at),
                        "use changes type",
                        "a plain use must not change an operand's type".to_owned(),
                    );
                }
            }
            Rvalue::Binary { op, lhs, rhs } => {
                // A **shift** is the one binary form whose operands need not share a type: the
                // count is a separate integer, so `x << 1` has an `s8` value and an `s64`
                // count and that is correct rather than a coercion (ADR-0042 §2). Every other
                // operator still requires one type, which is ADR-0015's no-coercion rule.
                let operands_must_match = !matches!(op, BinOp::Shl | BinOp::Shr);
                if operands_must_match
                    && let (Some(lhs), Some(rhs)) =
                        (self.operand_type(*lhs), self.operand_type(*rhs))
                    && lhs != rhs
                {
                    self.report(
                        Some(at),
                        "mixed operand types",
                        "ADR-0015 forbids coercion, so both operands must have one type".to_owned(),
                    );
                }
                self.check_binary_result(at, *op, dest);
            }
            Rvalue::Unary { op, operand } => {
                let operand_ty = self.operand_type(*operand);
                match op {
                    UnOp::Not => {
                        if let Some(ty) = operand_ty
                            && ty != PoolId::BOOL
                        {
                            self.report(
                                Some(at),
                                "not on a non-bool",
                                "`!` wants a bool".to_owned(),
                            );
                        }
                    }
                    // `~` is integers only (ADR-0042 §5), unlike `-` — so this is the one
                    // unary check that must *not* accept a float.
                    UnOp::BitNot => {
                        // A flags enum is integer-shaped for bitwise purposes (ADR-0043 §3):
                        // `~Perm.READ` is well-formed MIR and keeps the flags type.
                        if let Some(ty) = operand_ty
                            && !self.is_integer(ty)
                            && !self.is_flags_enum(ty)
                        {
                            self.report(
                                Some(at),
                                "complement of a non-integer",
                                "`~` wants an integer".to_owned(),
                            );
                        }
                    }
                    UnOp::Neg => {
                        // Integers *and* floats: negating a float flips its sign bit and
                        // cannot fail, where negating the most negative integer traps
                        // (ADR-0002 versus ADR-0040 §1). Both are well-formed MIR; the
                        // difference is in the arithmetic, not the shape.
                        if let Some(ty) = operand_ty
                            && !self.is_numeric(ty)
                        {
                            self.report(
                                Some(at),
                                "negation of a non-number",
                                "unary `-` wants an integer or a float".to_owned(),
                            );
                        }
                    }
                }
            }
            Rvalue::Call { callee, args } => {
                let _ = (callee, args); // signature arity needs the signatures, not the body
            }
            Rvalue::Load(place) => self.check_place_types(at, place),
            Rvalue::Address(place) => {
                self.check_place_types(at, place);
                if let Some(dest) = dest
                    && !self.is_pointer(dest)
                {
                    self.report(
                        Some(at),
                        "address is not a pointer",
                        "taking an address must produce a pointer".to_owned(),
                    );
                }
            }
            // An undefined value has whatever type its declaration gave it; there
            // is nothing about it to check that the arena walk has not already.
            Rvalue::Undef => {}
        }
    }

    fn check_binary_result(&mut self, at: BlockId, op: BinOp, dest: Option<PoolId>) {
        let Some(dest) = dest else { return };
        match op {
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                if dest != PoolId::BOOL {
                    self.report(
                        Some(at),
                        "comparison is not a bool",
                        "a comparison must produce a bool".to_owned(),
                    );
                }
            }
            BinOp::Add
            | BinOp::Sub
            | BinOp::Mul
            | BinOp::Div
            | BinOp::Rem
            | BinOp::WrapAdd
            | BinOp::WrapSub
            | BinOp::WrapMul
            // A bitwise operator's and a shift's result is the operand's type, not a `bool`.
            | BinOp::BitAnd
            | BinOp::BitOr
            | BinOp::BitXor
            | BinOp::Shl
            | BinOp::Shr => {}
        }
    }

    fn check_place_types(&mut self, at: BlockId, place: &Place) {
        match &place.base {
            // A global's type comes from its declaration, exactly as a slot's comes from
            // `SlotData` — neither is a claim this pass can contradict (ADR-0186 §3).
            PlaceBase::Slot(_) | PlaceBase::Global(_) => {}
            PlaceBase::Deref(operand) => {
                if let Some(ty) = self.operand_type(*operand)
                    && !self.is_pointer(ty)
                {
                    self.report(
                        Some(at),
                        "deref of a non-pointer",
                        "ADR-0011's postfix `.*` wants a pointer".to_owned(),
                    );
                }
            }
        }
        // Projections past the base need field types, and a field's type needs the
        // struct's fields from the pool plus a walk this wave does not do. ADR-0017
        // §5 keeps layout out of the crate, and without it there is nothing further
        // to check here that would not be guesswork.
    }

    fn check_terminator_types(&mut self, at: BlockId, term: &Terminator) {
        match term {
            Terminator::Branch {
                cond,
                then_: _,
                else_: _,
            } => {
                if let Some(ty) = self.operand_type(*cond)
                    && ty != PoolId::BOOL
                {
                    self.report(
                        Some(at),
                        "branch on a non-bool",
                        "a conditional branch wants a bool".to_owned(),
                    );
                }
            }
            Terminator::Return(operand) => match operand {
                Some(_) => {
                    if self.body.ret() == PoolId::VOID {
                        self.report(
                            Some(at),
                            "void body returns a value",
                            "a body returning void must use `Return(None)`".to_owned(),
                        );
                    }
                }
                None => {
                    if self.body.ret() != PoolId::VOID {
                        self.report(
                            Some(at),
                            "non-void body returns nothing",
                            "a body with a return type must return a value".to_owned(),
                        );
                    }
                }
            },
            Terminator::Goto(_) | Terminator::Unreachable(_) => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Use marking
// ---------------------------------------------------------------------------

fn mark_operand(operand: Operand, used: &mut [bool]) {
    match operand {
        Operand::Value(value) => {
            if let Some(flag) = used.get_mut(value.index()) {
                *flag = true;
            }
        }
        Operand::Constant(_) => {}
    }
}

fn mark_place(place: &Place, used: &mut [bool]) {
    match &place.base {
        // Neither marks an SSA value as used: a slot is an index and a global is a symbol
        // (ADR-0186 §3).
        PlaceBase::Slot(_) | PlaceBase::Global(_) => {}
        PlaceBase::Deref(operand) => mark_operand(*operand, used),
    }
    for projection in &place.projection {
        match projection {
            Projection::Index(operand) => mark_operand(*operand, used),
            Projection::Field(_)
            | Projection::Deref
            | Projection::StringData
            | Projection::StringCount
            | Projection::ViewData
            | Projection::ViewCount
            | Projection::DynamicArrayData
            | Projection::DynamicArrayCount
            | Projection::DynamicArrayCapacity
            | Projection::VariantTag => {}
        }
    }
}

fn mark_rvalue(rvalue: &Rvalue, used: &mut [bool]) {
    match rvalue {
        Rvalue::Atomic {
            op: _,
            address,
            value,
            expected,
        } => {
            mark_operand(*address, used);
            if let Some(value) = value {
                mark_operand(*value, used);
            }
            if let Some(expected) = expected {
                mark_operand(*expected, used);
            }
        }
        Rvalue::Use(operand) => mark_operand(*operand, used),
        Rvalue::Binary { op: _, lhs, rhs } => {
            mark_operand(*lhs, used);
            mark_operand(*rhs, used);
        }
        Rvalue::Unary { op: _, operand } => mark_operand(*operand, used),
        Rvalue::Convert { operand, from: _ } => mark_operand(*operand, used),
        Rvalue::Call { callee, args } => {
            match callee {
                Callee::Direct(_) => {}
                Callee::Indirect(operand) => mark_operand(*operand, used),
            }
            for arg in args {
                mark_operand(*arg, used);
            }
        }
        Rvalue::Load(place) | Rvalue::Address(place) => mark_place(place, used),
        Rvalue::Undef => {}
    }
}

#[cfg(test)]
mod tests {
    use jr_base::FileId;
    use jr_hir::ProcId;

    use super::*;
    use crate::mir::{MirSpan, ProcRef, Target, ValueId};

    fn void_body() -> MirBody {
        let mut mir = MirBody::new(
            ProcRef::new(FileId::from_usize(0), ProcId::from_usize(0)),
            PoolId::VOID,
        );
        mir.set_terminator(mir.entry(), Terminator::Return(None));
        mir
    }

    fn rules(findings: &[Finding]) -> Vec<&'static str> {
        findings.iter().map(|f| f.rule).collect()
    }

    #[test]
    fn a_minimal_void_body_is_valid() {
        let pool = Pool::new();
        assert_eq!(verify(&void_body(), &pool), Vec::new());
    }

    #[test]
    fn a_diamond_with_matching_edge_arity_is_valid() {
        let pool = Pool::new();
        let mut mir = MirBody::new(
            ProcRef::new(FileId::from_usize(0), ProcId::from_usize(0)),
            PoolId::S64,
        );
        let then_ = mir.push_block();
        let else_ = mir.push_block();
        let join = mir.push_block();
        let merged = mir.push_block_param(join, PoolId::S64, MirSpan::Synthetic);
        mir.set_terminator(
            mir.entry(),
            Terminator::Branch {
                cond: Operand::Constant(PoolId::TRUE),
                then_: Target::new(then_),
                else_: Target::new(else_),
            },
        );
        let one = Operand::Constant(PoolId::VOID_VALUE);
        mir.set_terminator(then_, Terminator::Goto(Target::with_args(join, vec![one])));
        mir.set_terminator(else_, Terminator::Goto(Target::with_args(join, vec![one])));
        mir.set_terminator(join, Terminator::Return(Some(Operand::Value(merged))));
        // The join block's parameter type is s64 while the arguments are the void
        // value; only arity is checked on an edge, which this asserts on purpose.
        assert!(!rules(&verify(&mir, &pool)).contains(&"edge arity disagrees"));
        assert!(!rules(&verify(&mir, &pool)).contains(&"critical edge"));
    }

    #[test]
    fn a_poisoned_value_is_reported() {
        let pool = Pool::new();
        let mut mir = void_body();
        let value = mir.push_value(PoolId::ERROR, MirSpan::Synthetic);
        mir.stmts_mut(mir.entry()).push(Statement::Assign {
            dest: value,
            rvalue: Rvalue::Use(Operand::Constant(PoolId::VOID_VALUE)),
            span: MirSpan::Synthetic,
        });
        assert!(rules(&verify(&mir, &pool)).contains(&"poisoned value"));
    }

    #[test]
    fn a_poisoned_constant_is_reported() {
        let pool = Pool::new();
        let mut mir = void_body();
        mir.stmts_mut(mir.entry()).push(Statement::Discard {
            rvalue: Rvalue::Use(Operand::Constant(PoolId::ERROR)),
            span: MirSpan::Synthetic,
        });
        assert!(rules(&verify(&mir, &pool)).contains(&"poisoned constant"));
    }

    #[test]
    fn an_edge_arity_mismatch_is_reported() {
        let pool = Pool::new();
        let mut mir = void_body();
        let target = mir.push_block();
        let _param = mir.push_block_param(target, PoolId::S64, MirSpan::Synthetic);
        mir.set_terminator(mir.entry(), Terminator::Goto(Target::new(target)));
        mir.set_terminator(target, Terminator::Return(None));
        assert!(rules(&verify(&mir, &pool)).contains(&"edge arity disagrees"));
    }

    #[test]
    fn a_critical_edge_is_reported() {
        let pool = Pool::new();
        let mut mir = void_body();
        let other = mir.push_block();
        let join = mir.push_block();
        mir.set_terminator(
            mir.entry(),
            Terminator::Branch {
                cond: Operand::Constant(PoolId::TRUE),
                then_: Target::new(join),
                else_: Target::new(other),
            },
        );
        // `other` also jumps to `join`, so `join` has two predecessors while the
        // entry has two successors: the entry-to-join edge is critical.
        mir.set_terminator(other, Terminator::Goto(Target::new(join)));
        mir.set_terminator(join, Terminator::Return(None));
        assert!(rules(&verify(&mir, &pool)).contains(&"critical edge"));
    }

    #[test]
    fn a_value_defined_twice_is_reported() {
        let pool = Pool::new();
        let mut mir = void_body();
        let value = mir.push_value(PoolId::VOID, MirSpan::Synthetic);
        let assign = Statement::Assign {
            dest: value,
            rvalue: Rvalue::Use(Operand::Constant(PoolId::VOID_VALUE)),
            span: MirSpan::Synthetic,
        };
        mir.stmts_mut(mir.entry()).push(assign.clone());
        mir.stmts_mut(mir.entry()).push(assign);
        assert!(rules(&verify(&mir, &pool)).contains(&"value defined more than once"));
    }

    #[test]
    fn a_used_value_that_is_never_defined_is_reported() {
        let pool = Pool::new();
        let mut mir = void_body();
        let orphan = mir.push_value(PoolId::VOID, MirSpan::Synthetic);
        mir.stmts_mut(mir.entry()).push(Statement::Discard {
            rvalue: Rvalue::Use(Operand::Value(orphan)),
            span: MirSpan::Synthetic,
        });
        assert!(rules(&verify(&mir, &pool)).contains(&"value never defined"));
    }

    #[test]
    fn a_declared_but_unused_value_is_not_reported() {
        let pool = Pool::new();
        let mut mir = void_body();
        // Collapsing a trivial block parameter leaves exactly this residue.
        let _dead = mir.push_value(PoolId::VOID, MirSpan::Synthetic);
        assert_eq!(verify(&mir, &pool), Vec::new());
    }

    #[test]
    fn an_out_of_range_value_is_reported_without_panicking() {
        let pool = Pool::new();
        let mut mir = void_body();
        mir.stmts_mut(mir.entry()).push(Statement::Discard {
            rvalue: Rvalue::Use(Operand::Value(ValueId::from_usize(99))),
            span: MirSpan::Synthetic,
        });
        assert!(rules(&verify(&mir, &pool)).contains(&"value out of range"));
    }

    #[test]
    fn a_void_body_returning_a_value_is_reported() {
        let pool = Pool::new();
        let mut mir = void_body();
        mir.set_terminator(
            mir.entry(),
            Terminator::Return(Some(Operand::Constant(PoolId::VOID_VALUE))),
        );
        assert!(rules(&verify(&mir, &pool)).contains(&"void body returns a value"));
    }

    #[test]
    fn a_branch_on_a_non_bool_is_reported() {
        let pool = Pool::new();
        let mut mir = void_body();
        let target = mir.push_block();
        mir.set_terminator(
            mir.entry(),
            Terminator::Branch {
                cond: Operand::Constant(PoolId::VOID_VALUE),
                then_: Target::new(target),
                else_: Target::new(target),
            },
        );
        mir.set_terminator(target, Terminator::Return(None));
        assert!(rules(&verify(&mir, &pool)).contains(&"branch on a non-bool"));
    }

    #[test]
    fn entry_parameters_must_match_the_bodys_parameters() {
        let pool = Pool::new();
        let mut mir = void_body();
        let stray = mir.push_value(PoolId::S64, MirSpan::Synthetic);
        mir.set_params(vec![stray]);
        assert!(rules(&verify(&mir, &pool)).contains(&"entry parameters disagree"));
    }

    #[test]
    fn assert_valid_accepts_a_well_formed_body() {
        assert_valid(&void_body(), &Pool::new());
    }

    #[test]
    #[should_panic(expected = "malformed MIR")]
    fn assert_valid_panics_on_poison() {
        let mut mir = void_body();
        let value = mir.push_value(PoolId::ERROR, MirSpan::Synthetic);
        mir.stmts_mut(mir.entry()).push(Statement::Assign {
            dest: value,
            rvalue: Rvalue::Use(Operand::Constant(PoolId::VOID_VALUE)),
            span: MirSpan::Synthetic,
        });
        assert_valid(&mir, &Pool::new());
    }
}
