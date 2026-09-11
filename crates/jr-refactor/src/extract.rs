//! Extraction implementation.

use std::{collections::BTreeSet, sync::Arc};

use jr_base::{TextRange, TextSize};
use jr_db::{Db, ModuleCatalog, SourceFile};
use jr_hir::{
    Body, BodyId, Expr, ExprId, FileHir, ForIterable, ItemKind, LocalId, ParamId, ProcId, Res,
    Stmt, StmtId, StructLitEntry, UnOp,
};
use jr_pool::{ContextKind, Item as PoolItem, PoolId};
use jr_sema::{FileSignatures, TypeMap};

use crate::{
    ExtractRequest, Extraction, ExtractionKind, ExtractionReport, Refusal, SourceChange, SourceEdit,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Binding {
    Local(LocalId),
    Param(ParamId),
}

#[derive(Debug, Clone)]
struct ImportTypes {
    alias: Option<String>,
    signatures: Arc<FileSignatures>,
}

struct Analysis<'a> {
    db: &'a dyn Db,
    text: Arc<str>,
    hir: Arc<FileHir>,
    resolutions: Arc<jr_hir::ResolveMap>,
    types: Arc<TypeMap>,
    signatures: Arc<FileSignatures>,
    imports: Vec<ImportTypes>,
}

#[derive(Debug, Clone)]
struct StatementSelection {
    body_id: BodyId,
    proc_id: ProcId,
    roots: Vec<StmtId>,
    range: TextRange,
    reaches_block_tail: bool,
}

#[derive(Debug, Clone, Copy)]
struct ExpressionSelection {
    body_id: BodyId,
    expr: ExprId,
    statement: StmtId,
}

#[derive(Debug, Clone)]
struct Rewrite {
    range: TextRange,
    replacement: String,
}

#[derive(Debug, Clone)]
enum Flow {
    Return,
    Break(Option<String>),
    Continue(Option<String>),
}

#[derive(Debug, Clone)]
struct FlowCase {
    code: usize,
    flow: Flow,
}

/// Computes every requested extraction action.
#[must_use]
pub fn extract(
    db: &dyn Db,
    file: SourceFile,
    catalog: ModuleCatalog,
    request: ExtractRequest,
) -> ExtractionReport {
    let mut report = ExtractionReport::default();
    if jr_db::file_diagnostics(db, file, catalog).has_errors() {
        refuse_requested(&mut report, &request, Refusal::ExistingErrors);
        return report;
    }

    let analysis = Analysis::new(db, file, catalog);
    if request.kinds.contains(ExtractionKind::Local) {
        match extract_local(&analysis, &request) {
            Ok(candidate) => report.candidates.push(candidate),
            Err(reason) => report.refusals.push((ExtractionKind::Local, reason)),
        }
    }
    if request.kinds.contains(ExtractionKind::Procedure) {
        match extract_procedure(&analysis, &request) {
            Ok(candidate) => report.candidates.push(candidate),
            Err(reason) => report.refusals.push((ExtractionKind::Procedure, reason)),
        }
    }
    report
}

impl<'a> Analysis<'a> {
    fn new(db: &'a dyn Db, file: SourceFile, catalog: ModuleCatalog) -> Self {
        let text = file.text(db);
        let hir = jr_db::file_hir(db, file);
        let resolutions = jr_db::resolved(db, file, catalog).map;
        let checked = jr_db::checked(db, file, catalog);
        let signatures = jr_db::file_signatures(db, file, catalog).signatures;
        let mut imports = Vec::new();
        for item in &hir.items {
            let ItemKind::Import { path, alias, .. } = &item.kind else {
                continue;
            };
            let Some(imported_path) =
                jr_db::module_file(db, catalog, Arc::from(path.as_str())).found
            else {
                continue;
            };
            let Some(imported) = db.source_file_for_path(&imported_path.to_string_lossy()) else {
                continue;
            };
            imports.push(ImportTypes {
                alias: alias.map(|name| db.interner().resolve(name).to_owned()),
                signatures: jr_db::file_signatures(db, imported, catalog).signatures,
            });
        }
        Self {
            db,
            text,
            hir,
            resolutions,
            types: checked.types,
            signatures,
            imports,
        }
    }

    fn slice(&self, range: TextRange) -> &str {
        &self.text[usize::from(range.start())..usize::from(range.end())]
    }

    fn binding_name(&self, body: &Body, proc_id: ProcId, binding: Binding) -> &str {
        let symbol = match binding {
            Binding::Local(local) => body.local(local).name,
            Binding::Param(param) => self.hir.proc(proc_id).params[param.index()].name,
        };
        self.db.interner().resolve(symbol)
    }

    fn render_type(&self, ty: PoolId) -> Option<String> {
        let pool = self.db.read_pool();
        render_type(&pool, self.signatures.as_ref(), &self.imports, ty)
    }
}

fn refuse_requested(report: &mut ExtractionReport, request: &ExtractRequest, refusal: Refusal) {
    for kind in [ExtractionKind::Local, ExtractionKind::Procedure] {
        if request.kinds.contains(kind) {
            report.refusals.push((kind, refusal));
        }
    }
}

fn extract_local(analysis: &Analysis<'_>, request: &ExtractRequest) -> Result<Extraction, Refusal> {
    let selection =
        normalized_selection(&analysis.text, request.selection).ok_or(Refusal::InvalidSelection)?;
    let found =
        find_expression_selection(&analysis.hir, selection).ok_or(Refusal::InvalidSelection)?;
    let body = analysis.hir.body(found.body_id);
    if !expression_moves_immediately_before_statement(body, found.statement, found.expr) {
        return Err(Refusal::UnsafeEvaluationOrder);
    }

    let statement_span = stmt_span(body.stmt(found.statement));
    let indent = line_indent(&analysis.text, usize::from(statement_span.start()));
    let name = fresh_name(&analysis.text, "extracted");
    let selected = analysis.slice(selection);
    // The statement span starts at its first token, after the existing line indentation. Reuse
    // those bytes for the new declaration and explicitly restore them before the original statement.
    let insertion = format!("{name} := {selected};\n{indent}");

    Ok(Extraction {
        title: format!("extract expression to local `{name}`"),
        kind: ExtractionKind::Local,
        change: SourceChange {
            edits: vec![
                SourceEdit {
                    range: TextRange::empty(statement_span.start()),
                    replacement: insertion,
                },
                SourceEdit {
                    range: selection,
                    replacement: name,
                },
            ],
        },
    })
}

fn extract_procedure(
    analysis: &Analysis<'_>,
    request: &ExtractRequest,
) -> Result<Extraction, Refusal> {
    let selection = find_statement_selection(&analysis.hir, &analysis.text, request.selection)
        .ok_or(Refusal::NonContiguousStatements)?;
    let body = analysis.hir.body(selection.body_id);
    let selected = selected_statement_set(body, &selection.roots);

    if selected.iter().any(|id| {
        matches!(
            body.stmt(*id),
            Stmt::Item(..) | Stmt::Insert { .. } | Stmt::Error(..)
        )
    }) {
        return Err(Refusal::GeneratedSource);
    }
    if !selection.reaches_block_tail
        && selection
            .roots
            .iter()
            .any(|id| matches!(body.stmt(*id), Stmt::Defer(..)))
    {
        return Err(Refusal::DeferLifetimeWouldChange);
    }

    let stmt_parents = statement_parents(body);
    let expr_parents = expression_parents(body);
    let mut declared = BTreeSet::new();
    for root in &selection.roots {
        walk_stmt(body, *root, &mut |stmt_id| {
            collect_declared(body, stmt_id, &mut declared);
        });
    }

    let mut captures = BTreeSet::new();
    for root in &selection.roots {
        walk_stmt_exprs(body, *root, &mut |expr_id| {
            if let Some(binding) = binding_of(
                analysis
                    .resolutions
                    .get_in_body(selection.body_id, expr_id)
                    .as_ref(),
            ) && !declared.contains(&binding)
            {
                captures.insert(binding);
            }
        });
    }

    let escaping: BTreeSet<LocalId> = declared
        .iter()
        .filter_map(|binding| match binding {
            Binding::Local(local) => Some(*local),
            Binding::Param(_) => None,
        })
        .filter(|local| {
            body.exprs.iter().enumerate().any(|(index, _)| {
                let expr_id = ExprId::from_usize(index);
                usize::from(body.expr_span(expr_id).start()) >= usize::from(selection.range.end())
                    && matches!(
                        analysis
                            .resolutions
                            .get_in_body(selection.body_id, expr_id),
                        Some(Res::Local(found)) if found == *local
                    )
            })
        })
        .collect();

    let helper_name = fresh_name(&analysis.text, "extracted_proc");
    let flow_name = fresh_name(&analysis.text, "__extract_flow");
    let indent = line_indent(&analysis.text, usize::from(selection.range.start()));
    let unit = if request.indent_unit.is_empty() {
        "  "
    } else {
        request.indent_unit.as_str()
    };

    let mut params = Vec::new();
    let mut args = Vec::new();
    let mut binding_parameters = Vec::new();
    for (index, binding) in captures.iter().copied().enumerate() {
        let param = fresh_name(&analysis.text, &format!("__extract_capture_{index}"));
        params.push(format!("{param}: *$ExtractCapture{index}"));
        args.push(format!(
            "*{}",
            analysis.binding_name(body, selection.proc_id, binding)
        ));
        binding_parameters.push((binding, param));
    }
    for (index, local) in escaping.iter().copied().enumerate() {
        let param = fresh_name(&analysis.text, &format!("__extract_output_{index}"));
        params.push(format!("{param}: *$ExtractOutput{index}"));
        args.push(format!(
            "*{}",
            analysis.binding_name(body, selection.proc_id, Binding::Local(local))
        ));
        binding_parameters.push((Binding::Local(local), param));
    }

    let mut capture_rewrites = Vec::new();
    for root in &selection.roots {
        walk_stmt_exprs(body, *root, &mut |expr_id| {
            let Some((binding, fields)) = binding_path(
                analysis
                    .resolutions
                    .get_in_body(selection.body_id, expr_id)
                    .as_ref(),
            ) else {
                return;
            };
            let Some((_, parameter)) = binding_parameters
                .iter()
                .find(|(candidate, _)| *candidate == binding)
            else {
                return;
            };
            let Expr::Name { span, .. } = body.expr(expr_id) else {
                return;
            };
            let suffix = fields
                .iter()
                .map(|field| format!(".{}", analysis.db.interner().resolve(*field)))
                .collect::<String>();
            if let Some(parent) = expr_parents[expr_id.index()]
                && let Expr::Unary {
                    op: UnOp::AddrOf,
                    span: unary_span,
                    ..
                } = body.expr(parent)
            {
                capture_rewrites.push(Rewrite {
                    range: unary_span.range,
                    replacement: format!("{parameter}{suffix}"),
                });
            } else {
                capture_rewrites.push(Rewrite {
                    range: span.range,
                    replacement: format!("{parameter}.*{suffix}"),
                });
            }
        });
    }

    let mut covering_rewrites = Vec::new();
    let mut caller_declarations = Vec::new();
    for local in &escaping {
        let declaration = body.local(*local);
        if declaration.using {
            return Err(Refusal::UnsupportedConstruct);
        }
        let ty = analysis
            .types
            .local_type(selection.body_id, *local)
            .and_then(|ty| analysis.render_type(ty))
            .ok_or(Refusal::UnspellableType)?;
        let name = analysis.db.interner().resolve(declaration.name);
        caller_declarations.push(format!("{indent}{name}: {ty};"));

        let Some(stmt_id) = selected
            .iter()
            .copied()
            .find(|id| matches!(body.stmt(*id), Stmt::Local(found, _) if found == local))
        else {
            continue;
        };
        let span = stmt_span(body.stmt(stmt_id)).range;
        let replacement = declaration.init.map_or_else(String::new, |init| {
            let value = rewrite_range(analysis, body.expr_span(init).range, &capture_rewrites);
            let parameter = binding_parameters
                .iter()
                .find(|(binding, _)| *binding == Binding::Local(*local))
                .map(|(_, parameter)| parameter.as_str())
                .expect("escaping local has an output parameter");
            format!("{parameter}.* = {value};")
        });
        covering_rewrites.push(Rewrite {
            range: span,
            replacement,
        });
    }

    let has_outward_return = selected
        .iter()
        .any(|id| matches!(body.stmt(*id), Stmt::Return(..) | Stmt::ReturnTuple(..)));
    let return_types = if has_outward_return {
        let proc_sig = analysis
            .signatures
            .proc_sig(selection.proc_id)
            .ok_or(Refusal::UnsupportedConstruct)?;
        result_types(analysis, proc_sig.ret)?
    } else {
        Vec::new()
    };
    let mut return_slots = Vec::new();
    for (index, ty) in return_types.iter().copied().enumerate() {
        let slot = fresh_name(&analysis.text, &format!("__extract_return_{index}"));
        let spelling = analysis.render_type(ty).ok_or(Refusal::UnspellableType)?;
        caller_declarations.push(format!("{indent}{slot}: {spelling};"));
        let param = fresh_name(&analysis.text, &format!("__extract_return_slot_{index}"));
        params.push(format!("{param}: *$ExtractReturn{index}"));
        args.push(format!("*{slot}"));
        return_slots.push((slot, param));
    }

    let mut flow_cases = Vec::new();
    let mut next_code = 1usize;
    for root in &selection.roots {
        walk_stmt(body, *root, &mut |stmt_id| {
            let external = match body.stmt(stmt_id) {
                Stmt::Return(value, span) => Some((
                    Flow::Return,
                    return_replacement(
                        analysis,
                        body,
                        *span,
                        value.iter().copied().collect(),
                        &return_slots,
                        next_code,
                        &capture_rewrites,
                    ),
                )),
                Stmt::ReturnTuple(values, span) => Some((
                    Flow::Return,
                    return_replacement(
                        analysis,
                        body,
                        *span,
                        values.clone(),
                        &return_slots,
                        next_code,
                        &capture_rewrites,
                    ),
                )),
                Stmt::Break(label, span) => loop_target(body, &stmt_parents, stmt_id, *label)
                    .filter(|target| !selected.contains(target))
                    .map(|_| {
                        let label =
                            label.map(|name| analysis.db.interner().resolve(name).to_owned());
                        (
                            Flow::Break(label),
                            flow_return_replacement(span.range, next_code),
                        )
                    }),
                Stmt::Continue(label, span) => loop_target(body, &stmt_parents, stmt_id, *label)
                    .filter(|target| !selected.contains(target))
                    .map(|_| {
                        let label =
                            label.map(|name| analysis.db.interner().resolve(name).to_owned());
                        (
                            Flow::Continue(label),
                            flow_return_replacement(span.range, next_code),
                        )
                    }),
                _ => None,
            };
            if let Some((flow, rewrite)) = external {
                flow_cases.push(FlowCase {
                    code: next_code,
                    flow,
                });
                covering_rewrites.push(rewrite);
                next_code += 1;
            }
        });
    }

    let mut rewrites = capture_rewrites;
    rewrites.retain(|rewrite| {
        !covering_rewrites
            .iter()
            .any(|cover| contains_range(cover.range, rewrite.range))
    });
    rewrites.extend(covering_rewrites);
    let transformed = apply_rewrites(
        analysis.slice(selection.range),
        selection.range.start(),
        &rewrites,
    )?;
    let body_text = reindent(&transformed, &indent, &format!("{indent}{unit}"));

    let uses_flow = !flow_cases.is_empty();
    let return_text = if uses_flow { " -> s64" } else { "" };
    let mut replacement = format!(
        "{indent}{helper_name} :: ({}){return_text} {{\n{}",
        params.join(", "),
        body_text
    );
    if !body_text.ends_with('\n') {
        replacement.push('\n');
    }
    if uses_flow {
        replacement.push_str(&format!("{indent}{unit}return 0;\n"));
    }
    replacement.push_str(&format!("{indent}}}\n"));
    if !caller_declarations.is_empty() {
        replacement.push_str(&caller_declarations.join("\n"));
        replacement.push('\n');
    }
    if uses_flow {
        replacement.push_str(&format!(
            "{indent}{flow_name} := {helper_name}({});\n",
            args.join(", ")
        ));
        for case in &flow_cases {
            let action = match &case.flow {
                Flow::Return if return_slots.is_empty() => String::from("return;"),
                Flow::Return => {
                    let values = return_slots
                        .iter()
                        .map(|(slot, _)| slot.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("return {values};")
                }
                Flow::Break(None) => String::from("break;"),
                Flow::Break(Some(label)) => format!("break {label};"),
                Flow::Continue(None) => String::from("continue;"),
                Flow::Continue(Some(label)) => format!("continue {label};"),
            };
            replacement.push_str(&format!(
                "{indent}if {flow_name} == {} {{\n{indent}{unit}{action}\n{indent}}}\n",
                case.code
            ));
        }
        replacement.pop();
    } else {
        replacement.push_str(&format!("{indent}{helper_name}({});", args.join(", ")));
    }

    Ok(Extraction {
        title: format!("extract statements to procedure `{helper_name}`"),
        kind: ExtractionKind::Procedure,
        change: SourceChange {
            edits: vec![SourceEdit {
                range: selection.range,
                replacement,
            }],
        },
    })
}

fn normalized_selection(text: &str, selection: TextRange) -> Option<TextRange> {
    let mut start = usize::from(selection.start()).min(text.len());
    let mut end = usize::from(selection.end()).min(text.len());
    while start < end {
        let ch = text[start..end].chars().next()?;
        if !ch.is_whitespace() {
            break;
        }
        start += ch.len_utf8();
    }
    while end > start {
        let ch = text[start..end].chars().next_back()?;
        if !ch.is_whitespace() {
            break;
        }
        end -= ch.len_utf8();
    }
    if start == end {
        return None;
    }
    Some(TextRange::new(
        TextSize::from(u32::try_from(start).ok()?),
        TextSize::from(u32::try_from(end).ok()?),
    ))
}

fn find_expression_selection(hir: &FileHir, range: TextRange) -> Option<ExpressionSelection> {
    for proc in &hir.procs {
        let Some(body_id) = proc.body else {
            continue;
        };
        let body = hir.body(body_id);
        for index in 0..body.exprs.len() {
            let expr = ExprId::from_usize(index);
            if body.expr_span(expr).range != range {
                continue;
            }
            let statement = containing_statement(body, body.root, expr)?;
            return Some(ExpressionSelection {
                body_id,
                expr,
                statement,
            });
        }
    }
    None
}

fn find_statement_selection(
    hir: &FileHir,
    text: &str,
    requested: TextRange,
) -> Option<StatementSelection> {
    let range = normalized_selection(text, requested)?;
    for (proc_index, proc) in hir.procs.iter().enumerate() {
        let Some(body_id) = proc.body else {
            continue;
        };
        let body = hir.body(body_id);
        if let Some((roots, reaches_block_tail)) = statement_sequence(body, body.root, range) {
            return Some(StatementSelection {
                body_id,
                proc_id: ProcId::from_usize(proc_index),
                roots,
                range,
                reaches_block_tail,
            });
        }
    }
    None
}

fn statement_sequence(
    body: &Body,
    stmt_id: StmtId,
    range: TextRange,
) -> Option<(Vec<StmtId>, bool)> {
    if let Stmt::Block(children, _) = body.stmt(stmt_id) {
        for start in 0..children.len() {
            if stmt_span(body.stmt(children[start])).start() != range.start() {
                continue;
            }
            for end in start..children.len() {
                if stmt_span(body.stmt(children[end])).end() == range.end() {
                    return Some((children[start..=end].to_vec(), end + 1 == children.len()));
                }
            }
        }
    }
    for child in stmt_children(body.stmt(stmt_id)) {
        if let Some(found) = statement_sequence(body, child, range) {
            return Some(found);
        }
    }
    None
}

fn expression_moves_immediately_before_statement(
    body: &Body,
    statement: StmtId,
    expr: ExprId,
) -> bool {
    match body.stmt(statement) {
        Stmt::Local(local, _) => body.local(*local).init == Some(expr),
        Stmt::Assign { lhs, rhs, .. } => *rhs == expr && pure_place(body, *lhs),
        Stmt::Return(Some(value), _) => *value == expr,
        Stmt::ReturnTuple(values, _) => values.len() == 1 && values[0] == expr,
        Stmt::If { cond, .. } | Stmt::Switch { value: cond, .. } => *cond == expr,
        Stmt::Discard { value, .. } => *value == expr,
        Stmt::Block(..)
        | Stmt::Item(..)
        | Stmt::Expr(..)
        | Stmt::LocalTuple { .. }
        | Stmt::AssignTuple { .. }
        | Stmt::While { .. }
        | Stmt::Return(None, _)
        | Stmt::Break(..)
        | Stmt::Continue(..)
        | Stmt::Todo { .. }
        | Stmt::For { .. }
        | Stmt::Defer(..)
        | Stmt::PushContext(..)
        | Stmt::Insert { .. }
        | Stmt::Error(..) => false,
    }
}

fn pure_place(body: &Body, expr: ExprId) -> bool {
    match body.expr(expr) {
        Expr::Name { .. } => true,
        Expr::Field { receiver, .. } | Expr::Deref(receiver, _) => pure_place(body, *receiver),
        Expr::Index { .. }
        | Expr::Literal(..)
        | Expr::Context(..)
        | Expr::Binary { .. }
        | Expr::Unary { .. }
        | Expr::Call { .. }
        | Expr::ArrayLit { .. }
        | Expr::StructLit { .. }
        | Expr::Slice { .. }
        | Expr::Uninit(..)
        | Expr::Cast { .. }
        | Expr::Autocast { .. }
        | Expr::Member { .. }
        | Expr::Run(..)
        | Expr::Directive { .. }
        | Expr::Error(..) => false,
    }
}

fn containing_statement(body: &Body, stmt_id: StmtId, needle: ExprId) -> Option<StmtId> {
    let mut direct = false;
    walk_direct_stmt_exprs(body, stmt_id, &mut |expr| {
        if expr == needle || expr_contains(body, expr, needle) {
            direct = true;
        }
    });
    if direct {
        return Some(stmt_id);
    }
    for child in stmt_children(body.stmt(stmt_id)) {
        if let Some(found) = containing_statement(body, child, needle) {
            return Some(found);
        }
    }
    None
}

fn expr_contains(body: &Body, root: ExprId, needle: ExprId) -> bool {
    let mut found = false;
    walk_expr(body, root, &mut |expr| found |= expr == needle);
    found
}

fn collect_declared(body: &Body, stmt_id: StmtId, out: &mut BTreeSet<Binding>) {
    match body.stmt(stmt_id) {
        Stmt::Local(local, _) => {
            out.insert(Binding::Local(*local));
        }
        Stmt::LocalTuple { targets, .. } => {
            out.extend(targets.iter().flatten().copied().map(Binding::Local));
        }
        Stmt::For { value, index, .. } => {
            out.insert(Binding::Local(*value));
            out.extend(index.iter().copied().map(Binding::Local));
        }
        _ => {}
    }
}

fn selected_statement_set(body: &Body, roots: &[StmtId]) -> BTreeSet<StmtId> {
    let mut selected = BTreeSet::new();
    for root in roots {
        walk_stmt(body, *root, &mut |stmt| {
            selected.insert(stmt);
        });
    }
    selected
}

fn binding_of(res: Option<&Res>) -> Option<Binding> {
    binding_path(res).map(|(binding, _)| binding)
}

fn binding_path(res: Option<&Res>) -> Option<(Binding, Vec<jr_base::Symbol>)> {
    match res? {
        Res::Local(local) => Some((Binding::Local(*local), Vec::new())),
        Res::Param(param) => Some((Binding::Param(*param), Vec::new())),
        Res::Promoted { base, field } => {
            let (binding, mut fields) = binding_path(Some(base))?;
            fields.push(*field);
            Some((binding, fields))
        }
        Res::Item(..) | Res::Imported(..) | Res::Error => None,
    }
}

fn result_types(analysis: &Analysis<'_>, ret: PoolId) -> Result<Vec<PoolId>, Refusal> {
    let pool = analysis.db.read_pool();
    match pool.item(ret) {
        PoolItem::VoidType => Ok(Vec::new()),
        PoolItem::ResultsType { elems } => Ok(elems.clone()),
        PoolItem::ErrorType => Err(Refusal::UnspellableType),
        _ => Ok(vec![ret]),
    }
}

fn return_replacement(
    analysis: &Analysis<'_>,
    body: &Body,
    span: jr_base::Span,
    values: Vec<ExprId>,
    slots: &[(String, String)],
    code: usize,
    rewrites: &[Rewrite],
) -> Rewrite {
    let indent = line_indent(&analysis.text, usize::from(span.start()));
    let mut replacement = String::new();
    for ((_, parameter), value) in slots.iter().zip(values) {
        let value = rewrite_range(analysis, body.expr_span(value).range, rewrites);
        replacement.push_str(&format!("{parameter}.* = {value};\n{indent}"));
    }
    replacement.push_str(&format!("return {code};"));
    Rewrite {
        range: span.range,
        replacement,
    }
}

fn flow_return_replacement(range: TextRange, code: usize) -> Rewrite {
    Rewrite {
        range,
        replacement: format!("return {code};"),
    }
}

fn loop_target(
    body: &Body,
    parents: &[Option<StmtId>],
    statement: StmtId,
    label: Option<jr_base::Symbol>,
) -> Option<StmtId> {
    let mut current = parents[statement.index()];
    while let Some(candidate) = current {
        match body.stmt(candidate) {
            Stmt::While {
                label: candidate_label,
                ..
            }
            | Stmt::For {
                label: candidate_label,
                ..
            } if label.is_none() || label == *candidate_label => return Some(candidate),
            _ => current = parents[candidate.index()],
        }
    }
    None
}

fn statement_parents(body: &Body) -> Vec<Option<StmtId>> {
    let mut parents = vec![None; body.stmts.len()];
    fn visit(body: &Body, id: StmtId, parent: Option<StmtId>, parents: &mut [Option<StmtId>]) {
        parents[id.index()] = parent;
        for child in stmt_children(body.stmt(id)) {
            visit(body, child, Some(id), parents);
        }
    }
    visit(body, body.root, None, &mut parents);
    parents
}

fn expression_parents(body: &Body) -> Vec<Option<ExprId>> {
    let mut parents = vec![None; body.exprs.len()];
    fn visit(body: &Body, id: ExprId, parent: Option<ExprId>, parents: &mut [Option<ExprId>]) {
        if parents[id.index()].is_some() {
            return;
        }
        parents[id.index()] = parent;
        for child in expr_children(body.expr(id)) {
            visit(body, child, Some(id), parents);
        }
    }
    for index in 0..body.exprs.len() {
        visit(body, ExprId::from_usize(index), None, &mut parents);
    }
    parents
}

fn walk_stmt(body: &Body, id: StmtId, visit: &mut impl FnMut(StmtId)) {
    visit(id);
    for child in stmt_children(body.stmt(id)) {
        walk_stmt(body, child, visit);
    }
}

fn walk_stmt_exprs(body: &Body, id: StmtId, visit: &mut impl FnMut(ExprId)) {
    walk_direct_stmt_exprs(body, id, &mut |expr| walk_expr(body, expr, visit));
    for child in stmt_children(body.stmt(id)) {
        walk_stmt_exprs(body, child, visit);
    }
}

fn walk_direct_stmt_exprs(body: &Body, id: StmtId, visit: &mut impl FnMut(ExprId)) {
    match body.stmt(id) {
        Stmt::Block(..)
        | Stmt::Item(..)
        | Stmt::Break(..)
        | Stmt::Continue(..)
        | Stmt::Todo { .. }
        | Stmt::Defer(..)
        | Stmt::PushContext(..)
        | Stmt::Error(..) => {}
        Stmt::Local(local, _) => {
            if let Some(init) = body.local(*local).init {
                visit(init);
            }
        }
        Stmt::Expr(expr, _) => visit(*expr),
        Stmt::Assign { lhs, rhs, .. } => {
            visit(*lhs);
            visit(*rhs);
        }
        Stmt::Discard { value, .. } => visit(*value),
        Stmt::LocalTuple { call, .. } => visit(*call),
        Stmt::AssignTuple { targets, call, .. } => {
            for target in targets.iter().flatten() {
                visit(*target);
            }
            visit(*call);
        }
        Stmt::If { cond, .. } | Stmt::While { cond, .. } => visit(*cond),
        Stmt::Return(value, _) => {
            if let Some(value) = value {
                visit(*value);
            }
        }
        Stmt::ReturnTuple(values, _) => {
            for value in values {
                visit(*value);
            }
        }
        Stmt::For { iterable, .. } => match iterable {
            ForIterable::Sequence(expr) => visit(*expr),
            ForIterable::Range { start, end } => {
                visit(*start);
                visit(*end);
            }
        },
        Stmt::Switch { value, arms, .. } => {
            visit(*value);
            for value in arms.iter().filter_map(|arm| arm.value) {
                visit(value);
            }
        }
        Stmt::Insert { operand, .. } => {
            if let Some(operand) = operand {
                visit(*operand);
            }
        }
    }
}

fn walk_expr(body: &Body, id: ExprId, visit: &mut impl FnMut(ExprId)) {
    visit(id);
    for child in expr_children(body.expr(id)) {
        walk_expr(body, child, visit);
    }
}

fn expr_children(expr: &Expr) -> Vec<ExprId> {
    match expr {
        Expr::Literal(..)
        | Expr::Context(..)
        | Expr::Name { .. }
        | Expr::Uninit(..)
        | Expr::Member { .. }
        | Expr::Directive { .. }
        | Expr::Error(..) => Vec::new(),
        Expr::Binary { lhs, rhs, .. } => vec![*lhs, *rhs],
        Expr::Unary { operand, .. }
        | Expr::Slice { base: operand, .. }
        | Expr::Deref(operand, _)
        | Expr::Autocast { operand, .. }
        | Expr::Run(operand, _) => vec![*operand],
        Expr::Call { callee, args, .. } => {
            let mut children = Vec::with_capacity(args.len() + 1);
            children.push(*callee);
            children.extend(args.iter().copied());
            children
        }
        Expr::Field { receiver, .. } => vec![*receiver],
        Expr::ArrayLit { elem_ty, elems, .. } => {
            let mut children = Vec::with_capacity(elems.len() + 1);
            children.push(*elem_ty);
            children.extend(elems.iter().copied());
            children
        }
        Expr::StructLit {
            explicit_ty,
            entries,
            ..
        } => explicit_ty
            .iter()
            .copied()
            .chain(entries.iter().map(StructLitEntry::value))
            .collect(),
        Expr::Index { base, index, .. } => vec![*base, *index],
        Expr::Cast { operand, .. } => vec![*operand],
    }
}

fn stmt_children(stmt: &Stmt) -> Vec<StmtId> {
    match stmt {
        Stmt::Block(stmts, _) => stmts.clone(),
        Stmt::If { then, else_, .. } => {
            let mut children = vec![*then];
            children.extend(else_.iter().copied());
            children
        }
        Stmt::While { body, .. }
        | Stmt::For { body, .. }
        | Stmt::Defer(body, _)
        | Stmt::PushContext(body, _) => vec![*body],
        Stmt::Switch { arms, .. } => arms.iter().map(|arm| arm.body).collect(),
        Stmt::Insert { stmts, .. } => stmts.clone(),
        Stmt::Local(..)
        | Stmt::Item(..)
        | Stmt::Expr(..)
        | Stmt::Assign { .. }
        | Stmt::Discard { .. }
        | Stmt::LocalTuple { .. }
        | Stmt::AssignTuple { .. }
        | Stmt::Return(..)
        | Stmt::ReturnTuple(..)
        | Stmt::Break(..)
        | Stmt::Continue(..)
        | Stmt::Todo { .. }
        | Stmt::Error(..) => Vec::new(),
    }
}

fn stmt_span(stmt: &Stmt) -> jr_base::Span {
    match stmt {
        Stmt::Block(_, span)
        | Stmt::ReturnTuple(_, span)
        | Stmt::LocalTuple { span, .. }
        | Stmt::AssignTuple { span, .. }
        | Stmt::Local(_, span)
        | Stmt::Item(_, span)
        | Stmt::Expr(_, span)
        | Stmt::Discard { span, .. }
        | Stmt::Return(_, span)
        | Stmt::Error(span)
        | Stmt::Break(_, span)
        | Stmt::Continue(_, span)
        | Stmt::Defer(_, span)
        | Stmt::PushContext(_, span) => *span,
        Stmt::Todo { span, .. }
        | Stmt::Switch { span, .. }
        | Stmt::Insert { span, .. }
        | Stmt::Assign { span, .. }
        | Stmt::If { span, .. }
        | Stmt::While { span, .. }
        | Stmt::For { span, .. } => *span,
    }
}

fn rewrite_range(analysis: &Analysis<'_>, range: TextRange, rewrites: &[Rewrite]) -> String {
    let applicable: Vec<_> = rewrites
        .iter()
        .filter(|rewrite| contains_range(range, rewrite.range))
        .cloned()
        .collect();
    apply_rewrites(analysis.slice(range), range.start(), &applicable)
        .unwrap_or_else(|_| analysis.slice(range).to_owned())
}

fn apply_rewrites(original: &str, base: TextSize, rewrites: &[Rewrite]) -> Result<String, Refusal> {
    let mut rewrites = rewrites.to_vec();
    rewrites.sort_by_key(|rewrite| (rewrite.range.start(), rewrite.range.end()));
    for pair in rewrites.windows(2) {
        if pair[0].range.end() > pair[1].range.start() {
            return Err(Refusal::UnsupportedConstruct);
        }
    }
    let mut out = String::with_capacity(original.len());
    let mut cursor = 0usize;
    for rewrite in rewrites {
        let start = usize::from(rewrite.range.start() - base);
        let end = usize::from(rewrite.range.end() - base);
        out.push_str(&original[cursor..start]);
        out.push_str(&rewrite.replacement);
        cursor = end;
    }
    out.push_str(&original[cursor..]);
    Ok(out)
}

fn contains_range(outer: TextRange, inner: TextRange) -> bool {
    outer.start() <= inner.start() && inner.end() <= outer.end()
}

fn line_indent(text: &str, offset: usize) -> String {
    let line_start = text[..offset.min(text.len())]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    text[line_start..offset.min(text.len())]
        .chars()
        .take_while(|ch| *ch == ' ' || *ch == '\t')
        .collect()
}

fn fresh_name(text: &str, base: &str) -> String {
    if !contains_identifier(text, base) {
        return base.to_owned();
    }
    for suffix in 2usize.. {
        let candidate = format!("{base}_{suffix}");
        if !contains_identifier(text, &candidate) {
            return candidate;
        }
    }
    unreachable!()
}

fn contains_identifier(text: &str, needle: &str) -> bool {
    text.split(|ch: char| !(ch == '_' || ch.is_ascii_alphanumeric()))
        .any(|word| word == needle)
}

fn reindent(text: &str, old_indent: &str, new_indent: &str) -> String {
    text.lines()
        .map(|line| {
            let line = line.strip_prefix(old_indent).unwrap_or(line);
            if line.is_empty() {
                String::new()
            } else {
                format!("{new_indent}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_type(
    pool: &jr_pool::Pool,
    own: &FileSignatures,
    imports: &[ImportTypes],
    ty: PoolId,
) -> Option<String> {
    if ty.index() >= pool.len() {
        return None;
    }
    let structural = |inner| render_type(pool, own, imports, inner);
    match pool.item(ty) {
        PoolItem::VoidType => Some(String::from("void")),
        PoolItem::BoolType => Some(String::from("bool")),
        PoolItem::IntType { signed, bits } => {
            Some(format!("{}{bits}", if *signed { 's' } else { 'u' }))
        }
        PoolItem::FloatType { bits } => Some(format!("float{bits}")),
        PoolItem::StringType => Some(String::from("string")),
        PoolItem::TypeType => Some(String::from("type")),
        PoolItem::ContextType => Some(String::from("Context")),
        PoolItem::PointerType(inner) => Some(format!("*{}", structural(*inner)?)),
        PoolItem::ArrayType { elem, len } => Some(format!("[{len}]{}", structural(*elem)?)),
        PoolItem::VectorType { elem, lanes } => {
            Some(format!("#simd [{lanes}]{}", structural(*elem)?))
        }
        PoolItem::ViewType { elem } => Some(format!("[]{}", structural(*elem)?)),
        PoolItem::DynamicArrayType { elem } => Some(format!("[..]{}", structural(*elem)?)),
        PoolItem::ResultsType { elems } => Some(format!(
            "({})",
            elems
                .iter()
                .map(|ty| structural(*ty))
                .collect::<Option<Vec<_>>>()?
                .join(", ")
        )),
        PoolItem::StructType { args, .. }
        | PoolItem::UnionType { args, .. }
        | PoolItem::VariantType { args, .. } => {
            let name = writable_nominal_name(pool, own, imports, ty)?;
            if args.is_empty() {
                Some(name)
            } else {
                Some(format!(
                    "{name}({})",
                    args.iter()
                        .map(|ty| structural(*ty))
                        .collect::<Option<Vec<_>>>()?
                        .join(", ")
                ))
            }
        }
        PoolItem::EnumType { .. } => writable_nominal_name(pool, own, imports, ty),
        PoolItem::ProcType {
            params,
            ret,
            context,
            effects,
        } => {
            // `#must` participates in procedure type identity but is not spellable on a procedure
            // type expression today; emitting the declaration attribute here would produce a
            // different grammar construct.
            if effects.must {
                return None;
            }
            let mut rendered = format!(
                "({})",
                params
                    .iter()
                    .map(|ty| structural(*ty))
                    .collect::<Option<Vec<_>>>()?
                    .join(", ")
            );
            if *ret != PoolId::VOID {
                rendered.push_str(&format!(" -> {}", structural(*ret)?));
            }
            if *context == ContextKind::CCall {
                rendered.push_str(" #c_call");
            }
            Some(rendered)
        }
        PoolItem::ErrorType
        | PoolItem::ForeignLibraryType
        | PoolItem::VoidValue
        | PoolItem::BoolValue(_)
        | PoolItem::IntValue { .. }
        | PoolItem::FloatValue { .. }
        | PoolItem::StaticArray { .. }
        | PoolItem::StrValue(_)
        | PoolItem::TypeValue(_)
        | PoolItem::ProcValue { .. }
        | PoolItem::ForeignLibraryValue(_, _)
        | PoolItem::AggregateValue { .. } => None,
    }
}

fn writable_nominal_name(
    pool: &jr_pool::Pool,
    own: &FileSignatures,
    imports: &[ImportTypes],
    ty: PoolId,
) -> Option<String> {
    if let Some(name) = own.type_name(ty) {
        return Some(name.to_owned());
    }
    let decl = pool.nominal_decl(ty)?;
    let declared_name = pool.decl_name(decl)?.to_owned();
    if own.file() == decl.file {
        return Some(declared_name);
    }
    for import in imports {
        if import.signatures.file() != decl.file {
            continue;
        }
        return Some(
            import
                .alias
                .as_ref()
                .map_or(declared_name.clone(), |alias| {
                    format!("{alias}.{declared_name}")
                }),
        );
    }
    None
}
