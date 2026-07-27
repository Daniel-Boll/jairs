//! A deterministic textual dump of a [`MirBody`], for tests and for reading.
//!
//! # Why a hand-rolled printer
//!
//! `jr-hir` already establishes the precedent: `dump_hir` is a hand-written
//! two-space-indent pretty-printer, and its own docs call it "indispensable when
//! debugging `jr-sema` and `jr-mir`". This is that, for MIR, and it plays the
//! same role for lowering that `jr fmt` plays for the CST — the cheapest possible
//! proof that the phase is *total*, because a dump of every corpus file either
//! renders or does not.
//!
//! Two alternatives were rejected. Deriving `Debug` and snapshotting that couples
//! every snapshot to Rust's field-by-field formatting, so adding a field churns
//! every file; a purpose-written printer renders the IR the way a compiler
//! engineer reads it and changes only when the IR's *meaning* changes. And
//! `insta`, which the plan named, is a declared dev-dependency of `jr-hir` and
//! `jr-db` that **nothing in the workspace actually uses** — there are no snapshot
//! directories at all — so reaching for it here would set a new precedent rather
//! than follow one. It remains a reasonable thing to layer *over* this function
//! later, which is why this returns a `String` rather than asserting anything.
//!
//! # Why determinism is a correctness property
//!
//! The next wave snapshots this output over the whole corpus, so an unstable
//! rendering is a permanently failing test rather than a cosmetic annoyance.
//! Nothing here iterates a hash map, prints an address, or depends on insertion
//! order that is not itself deterministic — [`FileMir`] is a `Vec` in `ProcId`
//! order for exactly this reason.
//!
//! # Why spans are not printed
//!
//! Every statement carries a [`MirSpan`], and none of them are rendered. A
//! `MirSpan` is an HIR arena index, so printing it would make a snapshot depend on
//! the *numbering* of HIR nodes — and inserting an unrelated expression earlier in
//! a file renumbers everything after it, churning snapshots that have nothing to
//! do with the change. The provenance is in the IR for diagnostics, which resolve
//! it on demand; it is not part of what a dump is asserting. [`dump_body_spans`]
//! exists for the debugging case where it is exactly what you want.

use jr_base::{FileId, Interner};
use jr_hir::{ConstValue, FileHir, ItemKind, ProcId};
use jr_pool::{Item, Pool, PoolId};
use jr_sema::FileSignatures;

use crate::mir::{
    BinOp, BlockId, Callee, FileMir, MirBody, MirSpan, Operand, Place, PlaceBase, Poisoned,
    ProcRef, Projection, Rvalue, SlotId, Statement, Target, Terminator, UnOp, Unreachable, ValueId,
};

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Dumps one body, without provenance.
#[must_use]
pub fn dump_body(body: &MirBody, pool: &Pool, signatures: &FileSignatures) -> String {
    let mut out = String::new();
    Dumper {
        pool,
        signatures,
        out: &mut out,
        indent: 0,
        spans: false,
        home: None,
    }
    .body(body, None);
    out
}

/// Dumps one body, annotating each statement with the HIR node it came from.
///
/// Not used by snapshots — see the module docs — but invaluable when a lowering
/// bug needs to be traced back to the source construct that produced it.
#[must_use]
pub fn dump_body_spans(body: &MirBody, pool: &Pool, signatures: &FileSignatures) -> String {
    let mut out = String::new();
    Dumper {
        pool,
        signatures,
        out: &mut out,
        indent: 0,
        spans: true,
        home: None,
    }
    .body(body, None);
    out
}

/// Dumps every body in a file, lowered or refused, in [`ProcId`] order.
///
/// A refused body renders a `poisoned:` line rather than being omitted, so that a
/// snapshot records the refusal. A body that silently vanished from a dump would
/// look identical to a body that was never there.
#[must_use]
pub fn dump_file(
    file: &FileMir,
    hir: &FileHir,
    pool: &Pool,
    signatures: &FileSignatures,
    interner: &Interner,
) -> String {
    let mut out = String::new();
    let mut dumper = Dumper {
        pool,
        signatures,
        out: &mut out,
        indent: 0,
        spans: false,
        home: None,
    };
    for (proc, outcome) in file.iter() {
        let name = proc_name(hir, interner, proc);
        match outcome {
            Ok(body) => dumper.body(body, Some(&name)),
            Err(poison) => dumper.poisoned(&name, *poison),
        }
    }
    out
}

/// The declared name of a procedure.
///
/// `Proc` carries no name: procedures are constants (ADR-0012), so the name lives
/// on the `Item` whose `ItemKind::Const` holds `ConstValue::Proc`. When no item
/// claims it — which nothing in the corpus does, but recovery could produce — the
/// `ProcId` itself is the name, because a dump must never panic.
fn proc_name(hir: &FileHir, interner: &Interner, proc: ProcId) -> String {
    for item in &hir.items {
        let ItemKind::Const {
            value: ConstValue::Proc(id),
        } = &item.kind
        else {
            continue;
        };
        if *id == proc {
            return match item.name {
                Some(name) => interner.resolve(name).to_owned(),
                None => format!("<anon proc {}>", proc.index()),
            };
        }
    }
    format!("<proc {}>", proc.index())
}

// ---------------------------------------------------------------------------
// Dumper
// ---------------------------------------------------------------------------

struct Dumper<'a> {
    pool: &'a Pool,
    signatures: &'a FileSignatures,
    out: &'a mut String,
    indent: usize,
    spans: bool,
    /// The file the body being dumped belongs to, so that [`Dumper::proc_ref`] can
    /// tell a local call from a cross-file one. `None` until a body starts.
    home: Option<FileId>,
}

impl Dumper<'_> {
    fn line(&mut self, text: &str) {
        for _ in 0..self.indent {
            self.out.push_str("  ");
        }
        self.out.push_str(text);
        self.out.push('\n');
    }

    // -------------------------------------------------------------------
    // Bodies
    // -------------------------------------------------------------------

    fn poisoned(&mut self, name: &str, poison: Poisoned) {
        let reason = match poison {
            Poisoned::Here(reason) => reason.to_owned(),
            Poisoned::Transitive(proc) => format!("depends on proc {}", proc.index()),
        };
        self.line(&format!("proc {name} {{"));
        self.indent += 1;
        self.line(&format!("poisoned: {reason}"));
        self.indent -= 1;
        self.line("}");
    }

    fn body(&mut self, body: &MirBody, name: Option<&str>) {
        self.home = Some(body.file());
        let header = match name {
            Some(name) => format!("proc {name} -> {} {{", self.ty(body.ret())),
            None => format!(
                "proc <{}> -> {} {{",
                body.proc().proc.index(),
                self.ty(body.ret())
            ),
        };
        self.line(&header);
        self.indent += 1;

        if !body.params().is_empty() {
            let params: Vec<String> = body
                .params()
                .iter()
                .map(|value| self.value_decl(body, *value))
                .collect();
            self.line(&format!("params: {}", params.join(", ")));
        }

        if body.slot_count() > 0 {
            self.line("slots:");
            self.indent += 1;
            for index in 0..body.slot_count() {
                let id = SlotId::from_usize(index);
                let slot = body.slot(id);
                let origin = match slot.local {
                    Some(local) => format!("  // local {}", local.index()),
                    None => String::from("  // temporary"),
                };
                self.line(&format!("s{index}: {}{origin}", self.ty(slot.ty)));
            }
            self.indent -= 1;
        }

        // Reachable blocks in execution order, then anything unreachable, so that
        // a dump shows the order the back ends will use but never quietly drops a
        // block that lowering left dangling.
        let order = body.reverse_postorder().to_vec();
        for block in &order {
            self.block(body, *block, false);
        }
        for index in 0..body.block_count() {
            let id = BlockId::from_usize(index);
            if !order.contains(&id) {
                self.block(body, id, true);
            }
        }

        self.facts(body);

        self.indent -= 1;
        self.line("}");
    }

    fn facts(&mut self, body: &MirBody) {
        let facts = body.facts();
        if facts.undefined_reads.is_empty() && facts.stray_jumps.is_empty() {
            return;
        }
        self.line("facts:");
        self.indent += 1;
        for read in &facts.undefined_reads {
            self.line(&format!(
                "read of possibly-undefined local {}",
                read.local.index()
            ));
        }
        for _ in &facts.stray_jumps {
            self.line("break or continue outside a loop");
        }
        self.indent -= 1;
    }

    fn block(&mut self, body: &MirBody, block: BlockId, unreachable: bool) {
        let params: Vec<String> = body
            .block(block)
            .params
            .iter()
            .map(|value| self.value_decl(body, *value))
            .collect();
        let marker = if unreachable { "  // unreachable" } else { "" };
        self.line(&format!(
            "bb{}({}):{marker}",
            block.index(),
            params.join(", ")
        ));
        self.indent += 1;
        for stmt in &body.block(block).stmts {
            let text = self.stmt(body, stmt);
            self.line(&text);
        }
        let term = self.terminator(&body.block(block).term);
        self.line(&term);
        self.indent -= 1;
    }

    fn value_decl(&self, body: &MirBody, value: ValueId) -> String {
        format!("v{}: {}", value.index(), self.ty(body.value(value).ty))
    }

    // -------------------------------------------------------------------
    // Statements and terminators
    // -------------------------------------------------------------------

    fn stmt(&self, body: &MirBody, stmt: &Statement) -> String {
        match stmt {
            Statement::Assign { dest, rvalue, span } => {
                format!(
                    "v{}: {} = {}{}",
                    dest.index(),
                    self.ty(body.value(*dest).ty),
                    self.rvalue(rvalue),
                    self.span(*span)
                )
            }
            Statement::Store { place, value, span } => {
                format!(
                    "store {} <- {}{}",
                    self.place(place),
                    self.operand(*value),
                    self.span(*span)
                )
            }
            Statement::Discard { rvalue, span } => {
                format!("discard {}{}", self.rvalue(rvalue), self.span(*span))
            }
            Statement::Nop => String::from("nop"),
        }
    }

    fn span(&self, span: MirSpan) -> String {
        if !self.spans {
            return String::new();
        }
        match span {
            MirSpan::Expr(_, expr) => format!("  // expr {}", expr.index()),
            MirSpan::Local(_, local) => format!("  // local {}", local.index()),
            MirSpan::Stmt(_, stmt) => format!("  // stmt {}", stmt.index()),
            MirSpan::Param(_, index) => format!("  // param {index}"),
            MirSpan::Synthetic => String::from("  // synthetic"),
        }
    }

    fn terminator(&self, term: &Terminator) -> String {
        match term {
            Terminator::Goto(target) => format!("goto {}", self.target(target)),
            Terminator::Branch { cond, then_, else_ } => format!(
                "branch {} ? {} : {}",
                self.operand(*cond),
                self.target(then_),
                self.target(else_)
            ),
            Terminator::Return(operand) => match operand {
                Some(operand) => format!("return {}", self.operand(*operand)),
                None => String::from("return"),
            },
            Terminator::Unreachable(why) => {
                let why = match why {
                    Unreachable::Trap => "trap",
                    Unreachable::StrayJump => "stray jump",
                    Unreachable::FellOffEnd => "fell off the end",
                };
                format!("unreachable // {why}")
            }
        }
    }

    fn target(&self, target: &Target) -> String {
        let args: Vec<String> = target.args.iter().map(|arg| self.operand(*arg)).collect();
        format!("bb{}({})", target.block.index(), args.join(", "))
    }

    fn rvalue(&self, rvalue: &Rvalue) -> String {
        match rvalue {
            Rvalue::Use(operand) => self.operand(*operand),
            Rvalue::Binary { op, lhs, rhs } => {
                format!(
                    "{} {} {}",
                    self.operand(*lhs),
                    bin_op(*op),
                    self.operand(*rhs)
                )
            }
            Rvalue::Unary { op, operand } => format!("{}{}", un_op(*op), self.operand(*operand)),
            Rvalue::Call { callee, args } => {
                let args: Vec<String> = args.iter().map(|arg| self.operand(*arg)).collect();
                let callee = match callee {
                    Callee::Direct(target) => self.proc_ref(*target),
                    Callee::Indirect(operand) => format!("({})", self.operand(*operand)),
                };
                format!("call {callee}({})", args.join(", "))
            }
            Rvalue::Load(place) => format!("load {}", self.place(place)),
            Rvalue::Address(place) => format!("addr {}", self.place(place)),
            Rvalue::Undef => String::from("undef"),
        }
    }

    fn place(&self, place: &Place) -> String {
        let mut text = match &place.base {
            PlaceBase::Slot(slot) => format!("s{}", slot.index()),
            PlaceBase::Deref(operand) => format!("({}).*", self.operand(*operand)),
        };
        for step in &place.projection {
            match step {
                Projection::Field(index) => text.push_str(&format!(".{index}")),
                Projection::Deref => text.push_str(".*"),
                Projection::StringData => text.push_str(".data"),
                Projection::StringCount => text.push_str(".count"),
            }
        }
        text
    }

    fn operand(&self, operand: Operand) -> String {
        match operand {
            Operand::Value(value) => format!("v{}", value.index()),
            Operand::Constant(id) => self.constant(id),
        }
    }

    /// Renders a callee, marking it when it lives in another file.
    ///
    /// A same-file call stays `proc3`, which keeps the common case terse and keeps
    /// every pre-ADR-0018 dump line unchanged. A cross-file call is `extern proc3`.
    ///
    /// The callee's [`FileId`] is deliberately **not** printed, even though
    /// [`ProcRef`] carries it. A `FileId` is an index assigned in database load
    /// order, so printing one would make an unrelated new corpus file renumber
    /// every cross-file call in the snapshot — and a snapshot whose diff is mostly
    /// churn is a snapshot nobody reads, which is the only thing it is for. The id
    /// is still in the IR for whoever needs it; the VM's own errors name files by
    /// path.
    fn proc_ref(&self, target: ProcRef) -> String {
        if Some(target.file) == self.home {
            format!("proc{}", target.proc.index())
        } else {
            format!("extern proc{}", target.proc.index())
        }
    }

    /// Renders an interned constant.
    ///
    /// Total, and deliberately non-panicking: an id from a foreign pool would make
    /// [`Pool::item`] panic, and a dump that crashes is useless exactly when it is
    /// most needed.
    fn constant(&self, id: PoolId) -> String {
        if id.index() >= self.pool.len() {
            return format!("<foreign {}>", id.index());
        }
        match self.pool.item(id) {
            Item::VoidValue => String::from("void"),
            Item::BoolValue(value) => value.to_string(),
            Item::IntValue { ty, bits } => format!("{bits}_{}", self.ty(*ty)),
            Item::StrValue(str_id) => format!("{:?}", self.pool.resolve_str(*str_id)),
            Item::TypeValue(ty) => format!("type({})", self.ty(*ty)),
            Item::ProcValue { ty: _, decl } => format!("proc({decl:?})"),
            Item::ForeignLibraryValue(str_id) => {
                format!("library({:?})", self.pool.resolve_str(*str_id))
            }
            // A *type* used as a constant operand is not something lowering
            // produces, but rendering it is cheaper than deciding it cannot happen.
            Item::VoidType
            | Item::BoolType
            | Item::IntType { .. }
            | Item::StringType
            | Item::TypeType
            | Item::ErrorType
            | Item::ForeignLibraryType
            | Item::PointerType(_)
            | Item::StructType { .. }
            | Item::ProcType { .. } => format!("type({})", self.ty(id)),
        }
    }

    // -------------------------------------------------------------------
    // Types
    // -------------------------------------------------------------------

    /// Renders a type.
    ///
    /// A local equivalent of `jr-sema`'s `describe`, which is `pub(crate)` there
    /// and so cannot be called from here. Duplication is the lesser evil: widening
    /// `jr-sema`'s API to export a debug-rendering helper would make a formatting
    /// choice part of its public contract.
    fn ty(&self, id: PoolId) -> String {
        if id.index() >= self.pool.len() {
            return format!("<foreign {}>", id.index());
        }
        if id == PoolId::ERROR {
            return String::from("<unknown>");
        }
        match self.pool.item(id) {
            Item::VoidType => String::from("void"),
            Item::BoolType => String::from("bool"),
            Item::IntType { signed, bits } => {
                format!("{}{bits}", if *signed { 's' } else { 'u' })
            }
            Item::StringType => String::from("string"),
            Item::TypeType => String::from("type"),
            Item::ErrorType => String::from("<unknown>"),
            Item::ForeignLibraryType => String::from("<library>"),
            Item::PointerType(pointee) => format!("*{}", self.ty(*pointee)),
            Item::StructType { decl } => match self.signatures.type_name(id) {
                Some(name) => name.to_owned(),
                None => format!("struct{decl:?}"),
            },
            Item::ProcType {
                params,
                ret,
                context: _,
                effects: _,
            } => {
                let params: Vec<String> = params.iter().map(|ty| self.ty(*ty)).collect();
                format!("({}) -> {}", params.join(", "), self.ty(*ret))
            }
            // The type *of* a value is what a caller wanted; a value where a type
            // was expected is a bug elsewhere, rendered rather than hidden.
            Item::VoidValue
            | Item::BoolValue(_)
            | Item::IntValue { .. }
            | Item::StrValue(_)
            | Item::TypeValue(_)
            | Item::ProcValue { .. }
            | Item::ForeignLibraryValue(_) => format!("<value {}>", id.index()),
        }
    }
}

/// The spelling of a binary operator. Wrapping forms keep the `%` suffix so that
/// ADR-0002's distinction between trapping and wrapping is visible in a dump.
fn bin_op(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Rem => "%",
        BinOp::WrapAdd => "+%",
        BinOp::WrapSub => "-%",
        BinOp::WrapMul => "*%",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
    }
}

/// The spelling of a unary operator.
fn un_op(op: UnOp) -> &'static str {
    match op {
        UnOp::Neg => "-",
        UnOp::Not => "!",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::{Facts, UndefinedRead};

    fn signatures() -> FileSignatures {
        FileSignatures::new()
    }

    /// A diamond: `bb0` branches, both arms jump to a join block that takes the
    /// merged value as a parameter. This is the shape a phi becomes.
    fn diamond(pool: &mut Pool) -> MirBody {
        let mut mir = MirBody::new(
            ProcRef::new(FileId::from_usize(0), ProcId::from_usize(0)),
            PoolId::S64,
        );
        let one = Operand::Constant(pool.int_value(PoolId::S64, 1));
        let two = Operand::Constant(pool.int_value(PoolId::S64, 2));

        let then_ = mir.push_block();
        let else_ = mir.push_block();
        let join = mir.push_block();
        let merged = mir.push_block_param(join, PoolId::S64, MirSpan::Synthetic);

        let cond = mir.push_value(PoolId::BOOL, MirSpan::Synthetic);
        mir.stmts_mut(mir.entry()).push(Statement::Assign {
            dest: cond,
            rvalue: Rvalue::Binary {
                op: BinOp::Lt,
                lhs: one,
                rhs: two,
            },
            span: MirSpan::Synthetic,
        });
        mir.set_terminator(
            mir.entry(),
            Terminator::Branch {
                cond: Operand::Value(cond),
                then_: Target::new(then_),
                else_: Target::new(else_),
            },
        );
        mir.set_terminator(then_, Terminator::Goto(Target::with_args(join, vec![one])));
        mir.set_terminator(else_, Terminator::Goto(Target::with_args(join, vec![two])));
        mir.set_terminator(join, Terminator::Return(Some(Operand::Value(merged))));
        mir
    }

    #[test]
    fn a_diamond_renders_blocks_in_execution_order_with_a_block_parameter() {
        let mut pool = Pool::new();
        let mir = diamond(&mut pool);
        let text = dump_body(&mir, &pool, &signatures());
        assert_eq!(
            text,
            "\
proc <0> -> s64 {
  bb0():
    v1: bool = 1_s64 < 2_s64
    branch v1 ? bb1() : bb2()
  bb1():
    goto bb3(1_s64)
  bb2():
    goto bb3(2_s64)
  bb3(v0: s64):
    return v0
}
"
        );
    }

    #[test]
    fn dumping_twice_produces_identical_text() {
        let mut pool = Pool::new();
        let mir = diamond(&mut pool);
        let first = dump_body(&mir, &pool, &signatures());
        let second = dump_body(&mir, &pool, &signatures());
        assert_eq!(
            first, second,
            "a snapshot of an unstable dump is a permanently red test"
        );
    }

    #[test]
    fn spans_are_omitted_by_default_and_present_on_request() {
        let mut pool = Pool::new();
        let mir = diamond(&mut pool);
        assert!(!dump_body(&mir, &pool, &signatures()).contains("synthetic"));
        assert!(dump_body_spans(&mir, &pool, &signatures()).contains("synthetic"));
    }

    #[test]
    fn an_unreachable_block_is_shown_and_marked() {
        let pool = Pool::new();
        let mut mir = MirBody::new(
            ProcRef::new(FileId::from_usize(0), ProcId::from_usize(0)),
            PoolId::VOID,
        );
        mir.set_terminator(mir.entry(), Terminator::Return(None));
        let orphan = mir.push_block();
        mir.set_terminator(orphan, Terminator::Unreachable(Unreachable::Trap));
        let text = dump_body(&mir, &pool, &signatures());
        assert!(text.contains("bb1():  // unreachable"), "got:\n{text}");
        assert!(text.contains("unreachable // trap"));
    }

    #[test]
    fn slots_render_with_the_local_they_stand_for() {
        let pool = Pool::new();
        let mut mir = MirBody::new(
            ProcRef::new(FileId::from_usize(0), ProcId::from_usize(0)),
            PoolId::VOID,
        );
        mir.set_terminator(mir.entry(), Terminator::Return(None));
        let slot = mir.push_slot(
            PoolId::S64,
            Some(jr_hir::LocalId::from_usize(2)),
            MirSpan::Synthetic,
        );
        mir.stmts_mut(mir.entry()).push(Statement::Store {
            place: Place::slot(slot),
            value: Operand::Constant(PoolId::VOID_VALUE),
            span: MirSpan::Synthetic,
        });
        let text = dump_body(&mir, &pool, &signatures());
        assert!(text.contains("s0: s64  // local 2"), "got:\n{text}");
        assert!(text.contains("store s0 <- void"));
    }

    #[test]
    fn a_place_renders_its_projections_in_order() {
        let mut pool = Pool::new();
        let ptr = pool.pointer_to(PoolId::S64);
        let mut mir = MirBody::new(
            ProcRef::new(FileId::from_usize(0), ProcId::from_usize(0)),
            PoolId::VOID,
        );
        mir.set_terminator(mir.entry(), Terminator::Return(None));
        let value = mir.push_value(ptr, MirSpan::Synthetic);
        let place = Place::deref(Operand::Value(value))
            .project(Projection::Field(1))
            .project(Projection::StringCount);
        let loaded = mir.push_value(PoolId::S64, MirSpan::Synthetic);
        mir.stmts_mut(mir.entry()).push(Statement::Assign {
            dest: value,
            rvalue: Rvalue::Address(Place::deref(Operand::Value(value))),
            span: MirSpan::Synthetic,
        });
        mir.stmts_mut(mir.entry()).push(Statement::Assign {
            dest: loaded,
            rvalue: Rvalue::Load(place),
            span: MirSpan::Synthetic,
        });
        let text = dump_body(&mir, &pool, &signatures());
        assert!(text.contains("load (v0).*.1.count"), "got:\n{text}");
        assert!(text.contains("addr (v0).*"), "got:\n{text}");
    }

    #[test]
    fn recorded_facts_render_so_a_snapshot_pins_them() {
        let pool = Pool::new();
        let mut mir = MirBody::new(
            ProcRef::new(FileId::from_usize(0), ProcId::from_usize(0)),
            PoolId::VOID,
        );
        mir.set_terminator(mir.entry(), Terminator::Return(None));
        mir.set_facts(Facts {
            undefined_reads: vec![UndefinedRead {
                local: jr_hir::LocalId::from_usize(0),
                span: MirSpan::Synthetic,
            }],
            stray_jumps: vec![MirSpan::Synthetic],
        });
        let text = dump_body(&mir, &pool, &signatures());
        assert!(
            text.contains("read of possibly-undefined local 0"),
            "got:\n{text}"
        );
        assert!(text.contains("break or continue outside a loop"));
    }

    #[test]
    fn a_refused_body_renders_its_reason_rather_than_vanishing() {
        let pool = Pool::new();
        let interner = Interner::new();
        let hir = FileHir {
            items: Vec::new(),
            scope: jr_hir::ItemScope::default(),
            procs: Vec::new(),
            structs: Vec::new(),
            bodies: Vec::new(),
            exprs: Vec::new(),
            expr_spans: Vec::new(),
            type_refs: Vec::new(),
        };
        let mut file = FileMir::new();
        file.push(
            ProcId::from_usize(0),
            Err(Poisoned::Here("a type failed to check")),
        );
        file.push(
            ProcId::from_usize(1),
            Err(Poisoned::Transitive(ProcId::from_usize(0))),
        );
        let text = dump_file(&file, &hir, &pool, &signatures(), &interner);
        assert!(
            text.contains("poisoned: a type failed to check"),
            "got:\n{text}"
        );
        assert!(text.contains("poisoned: depends on proc 0"), "got:\n{text}");
    }
}
