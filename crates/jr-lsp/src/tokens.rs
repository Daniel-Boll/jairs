//! Semantic tokens: the last LSP capability, and the one the grammar cannot give an editor.
//!
//! # Why this exists when `tree-sitter` highlighting already does
//!
//! [ADR-0025](../../../docs/adr/0025-tree-sitter-grammar.md) ships a grammar and highlight queries, and they
//! are genuinely good: fast, incremental, and correct for everything decidable from **shape**. What they
//! cannot do is tell one identifier from another. `Point` and `count` are both `IDENT` to a grammar, and so
//! are a parameter, a local, a field, a procedure and an imported module.
//!
//! A semantic-token response answers exactly that, and it is the only LSP capability whose whole value is
//! information the parser does not have. That is why it is the last one: everything else the server offers
//! (hover, definition, references, rename, completion, code actions, signature help, inlay hints) is a
//! *lookup*, and this is a **classification of every token in the file**.
//!
//! # Why this crate gained a dependency on `jr-syntax`
//!
//! Every other provider here works from the **HIR** and its spans, which is why `jr-lsp` had no syntax
//! dependency for thirteen capabilities. This one cannot: a token classifier's whole job is to say what each
//! token *is*, including the ones the HIR never sees — punctuation, keywords, comments, and the name of a
//! declaration that failed to lower. The CST is the only artefact that has all of them, in order, with their
//! offsets. Recorded because a new dependency on a crate this one deliberately avoided deserves a reason.
//!
//! # Why the CST leads and resolution follows
//!
//! Each token is classified by its **syntactic context** first — the parent node kind and the token's place
//! in it — and only then by resolution. Three reasons, in the order they mattered.
//!
//! A declaration's own name is not a *reference*, so no resolution answers it: `Point :: struct { … }` has
//! nothing to look up, and the parent node kind says "this is a type being declared" directly. **Context is
//! available in a file that does not parse cleanly**, which is the state an editor spends most of its time in
//! — resolution needs a HIR, and a HIR needs a tree without holes in the interesting places. And context is
//! *cheap*: one walk, no queries.
//!
//! Resolution is then asked only about a bare `NAME_EXPR`, where context genuinely cannot decide — `count`
//! could be a local, a parameter or a file constant, and only the resolver knows.
//!
//! # Why the token type list is short
//!
//! Eleven types and two modifiers, against the protocol's twenty-two and ten. A type earns its place by
//! being **distinguishable by this compiler** and **useful to a reader**; anything else is a legend entry
//! that never appears, which costs a client a lookup table for nothing.
//!
//! Notably absent: `class` and `interface` (this language has neither), `event` and `regexp` (likewise),
//! `typeParameter` — which *is* distinguishable, and is reported as `type` because a reader wants `$T` to look
//! like the type it stands for rather than like a fourth colour — and `modifier`/`static`/`abstract`, which
//! describe declarations this language does not have.
//!
//! # Why the encoding is written out rather than delegated
//!
//! The protocol asks for five integers per token, **delta-encoded against the previous token**: line delta,
//! then start delta *within a line* but absolute start on a new line, then length, type, modifiers. It is the
//! one genuinely fiddly part, and the fiddly bit is that the second number changes meaning depending on the
//! first. Encoded here in one place, with the tokens sorted by position first, because an out-of-order token
//! makes every later delta wrong — a whole-file corruption from one misplaced entry, which is the failure
//! mode this module's tests are aimed at.

use jr_db::{Db, SourceFile};
use jr_hir::{Expr, ExprScope, FileHir, ItemKind, Res};
use jr_syntax::kind::{SyntaxKind, SyntaxNode, SyntaxToken};
use lsp_types::{SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens};

use crate::position::{Encoding, Positions};

/// The token types this server reports, in the order a client indexes them.
///
/// The order is the wire protocol: a response carries an *index* into this list, so appending is safe and
/// reordering silently recolours every file. That is why the list is a `const` beside the encoder rather than
/// built where `capabilities()` is assembled.
pub const TOKEN_TYPES: &[SemanticTokenType] = &[
    SemanticTokenType::NAMESPACE,
    SemanticTokenType::TYPE,
    SemanticTokenType::ENUM,
    SemanticTokenType::STRUCT,
    SemanticTokenType::ENUM_MEMBER,
    SemanticTokenType::FUNCTION,
    SemanticTokenType::PARAMETER,
    SemanticTokenType::VARIABLE,
    SemanticTokenType::PROPERTY,
    SemanticTokenType::KEYWORD,
    SemanticTokenType::COMMENT,
    SemanticTokenType::STRING,
    SemanticTokenType::NUMBER,
    SemanticTokenType::OPERATOR,
    SemanticTokenType::MACRO,
    SemanticTokenType::DECORATOR,
];

/// The modifiers this server reports.
///
/// Two, because two are decidable and useful: whether this token *is* the declaration, and whether the thing
/// it names cannot change. A `readonly` local is what `::` means, and it is the distinction this language cares
/// about most — `a :: 1` and `a := 1` differ in exactly that.
pub const TOKEN_MODIFIERS: &[SemanticTokenModifier] = &[
    SemanticTokenModifier::DECLARATION,
    SemanticTokenModifier::READONLY,
];

/// A token type, as an index into [`TOKEN_TYPES`].
///
/// A named enum rather than bare indices, so that adding a type is a compile error at every site that has to
/// decide — the same reason this project bans a `_` match arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// An imported module name.
    Namespace,
    /// A type: a builtin, a `$T`, or a name in type position.
    Type,
    /// An `enum` or `enum_flags` declaration's name.
    Enum,
    /// A `struct`, `union` or `variant` declaration's name.
    Struct,
    /// An enum member, including a bare `.RED`.
    EnumMember,
    /// A procedure.
    Function,
    /// A procedure parameter.
    Parameter,
    /// A local or a file-scope constant.
    Variable,
    /// A struct field, in a declaration or a `.field` access.
    Property,
    /// A keyword.
    Keyword,
    /// A comment, including a doc comment.
    Comment,
    /// A string literal.
    String,
    /// An integer or float literal.
    Number,
    /// An operator, in an `operator +` declaration.
    Operator,
    /// A `#directive`.
    Macro,
    /// An `@note`.
    Decorator,
}

impl Kind {
    /// This kind's index in [`TOKEN_TYPES`].
    const fn index(self) -> u32 {
        match self {
            Self::Namespace => 0,
            Self::Type => 1,
            Self::Enum => 2,
            Self::Struct => 3,
            Self::EnumMember => 4,
            Self::Function => 5,
            Self::Parameter => 6,
            Self::Variable => 7,
            Self::Property => 8,
            Self::Keyword => 9,
            Self::Comment => 10,
            Self::String => 11,
            Self::Number => 12,
            Self::Operator => 13,
            Self::Macro => 14,
            Self::Decorator => 15,
        }
    }
}

/// `declaration`'s bit in [`TOKEN_MODIFIERS`].
const DECLARATION: u32 = 1;

/// `readonly`'s bit.
const READONLY: u32 = 2;

/// One classified token, before delta encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Classified {
    /// Byte offset of the token's start.
    start: jr_base::TextSize,
    /// Byte length.
    length: u32,
    /// What it is.
    kind: Kind,
    /// The modifier bitset.
    modifiers: u32,
}

/// Every semantic token in `file`, delta-encoded for the client.
///
/// Returns `None` only when the file is not in the database, which the server treats as "no tokens" — an
/// absent response is better than an empty one, because an empty one tells a client to *clear* what it has.
#[must_use]
pub fn semantic_tokens(
    db: &dyn Db,
    file: SourceFile,
    encoding: Encoding,
) -> Option<SemanticTokens> {
    let text = file.text(db);
    let index = jr_db::line_index(db, file);
    let parse = jr_db::parse_file(db, file);
    let hir = jr_db::file_hir(db, file);

    let mut out = Vec::new();
    collect(&parse.syntax(), hir.as_ref(), &mut out);
    // **Sorted before encoding**, because the deltas are relative: one out-of-order token does not misplace
    // itself, it misplaces every token after it. The walk is already in source order for a well-formed tree,
    // so this is a cheap guarantee rather than a fix for a known problem — which is exactly when to add it.
    out.sort_by_key(|token| token.start);

    let positions = Positions::new(text.as_ref(), &index, encoding);
    Some(SemanticTokens {
        result_id: None,
        data: encode(&out, &positions),
    })
}

/// Walks the tree, classifying every non-whitespace token.
fn collect(node: &SyntaxNode, hir: &FileHir, out: &mut Vec<Classified>) {
    for element in node.children_with_tokens() {
        match element {
            jr_syntax::kind::SyntaxElement::Token(token) => {
                if let Some(classified) = classify(&token, hir) {
                    out.push(classified);
                }
            }
            jr_syntax::kind::SyntaxElement::Node(child) => collect(&child, hir, out),
        }
    }
}

/// What one token is, or `None` for whitespace and punctuation.
///
/// Punctuation is deliberately unclassified: a client colours it from the grammar already, and reporting a
/// `,` as an operator would fight the editor's own theme for no information gained.
fn classify(token: &SyntaxToken, hir: &FileHir) -> Option<Classified> {
    let kind = token.kind();
    let simple = match kind {
        SyntaxKind::LINE_COMMENT
        | SyntaxKind::BLOCK_COMMENT
        | SyntaxKind::DOC_COMMENT
        | SyntaxKind::MODULE_DOC_COMMENT => Some(Kind::Comment),
        SyntaxKind::STRING_LITERAL => Some(Kind::String),
        SyntaxKind::INT_LITERAL | SyntaxKind::FLOAT_LITERAL => Some(Kind::Number),
        SyntaxKind::DIRECTIVE => Some(Kind::Macro),
        // `true`, `false` and `null` are keywords in the grammar and *values* to a reader. Reported as
        // keywords, because that is what an editor's theme expects of them and what every other language
        // server does — the alternative would make `true` a different colour from `if`, which surprises.
        _ if kind.is_keyword() => Some(Kind::Keyword),
        _ => None,
    };
    if let Some(kind) = simple {
        return Some(Classified {
            start: token.text_range().start(),
            length: length_of(token),
            kind,
            modifiers: 0,
        });
    }
    if kind != SyntaxKind::IDENT {
        return None;
    }
    let (kind, modifiers) = classify_ident(token, hir)?;
    Some(Classified {
        start: token.text_range().start(),
        length: length_of(token),
        kind,
        modifiers,
    })
}

/// An identifier's kind and modifiers, from its syntactic context first.
///
/// The `NAME` indirection is why this reads the *grandparent* in places: a declaration's name is wrapped in a
/// `NAME` node, so `Point :: struct {}` puts the `IDENT` under `NAME` under `CONST_DECL`. Matching only the
/// direct parent would classify every declaration name identically.
fn classify_ident(token: &SyntaxToken, hir: &FileHir) -> Option<(Kind, u32)> {
    let parent = token.parent()?;
    let grandparent = parent.parent();
    let grandkind = grandparent.as_ref().map(SyntaxNode::kind);

    match parent.kind() {
        // A declaration's name. Which *kind* of declaration it is comes from the value beside it, so a
        // `struct` gets `Struct` and a procedure gets `Function` — a reader wants those to differ, and the
        // tree already says which.
        SyntaxKind::NAME => match grandkind {
            Some(SyntaxKind::CONST_DECL) => {
                let value_kind = declared_value_kind(grandparent.as_ref()?);
                Some((value_kind, DECLARATION | READONLY))
            }
            // `a := 1` — a mutable local or file variable.
            Some(SyntaxKind::VAR_DECL | SyntaxKind::DECL_STMT) => {
                Some((Kind::Variable, DECLARATION))
            }
            Some(SyntaxKind::PARAM) => Some((Kind::Parameter, DECLARATION)),
            Some(SyntaxKind::FIELD) => Some((Kind::Property, DECLARATION)),
            Some(SyntaxKind::MEMBER) => Some((Kind::EnumMember, DECLARATION | READONLY)),
            Some(SyntaxKind::IMPORT_DECL) => Some((Kind::Namespace, DECLARATION)),
            Some(SyntaxKind::OPERATOR_DECL) => Some((Kind::Operator, DECLARATION | READONLY)),
            _ => Some((Kind::Variable, DECLARATION)),
        },
        // A name in **type position**. Every one of these is a type whatever it resolves to, which is why
        // context wins here: a `Point` in `p: Point` is a type even in a file where `Point` is undeclared,
        // and colouring it as an unknown variable would be actively misleading.
        SyntaxKind::NAME_TYPE | SyntaxKind::POLY_TYPE => Some((Kind::Type, 0)),
        // `.RED` — a bare enum member (ADR-0046) — and `RED;` inside an `enum` body, which the parser puts
        // directly under `MEMBER` rather than wrapping it in `NAME` the way a `FIELD` does. Both spellings
        // name the same thing, so they get the same kind; the `DECLARATION` modifier is what separates them,
        // and it comes from which node the token sits in rather than from how it is written.
        SyntaxKind::MEMBER => Some((Kind::EnumMember, DECLARATION | READONLY)),
        SyntaxKind::MEMBER_EXPR => Some((Kind::EnumMember, READONLY)),
        // A parameter's own name, which the parser puts directly under `PARAM`. The type beside it is inside
        // a `NAME_TYPE`, so a direct `IDENT` child of `PARAM` can only be the name — which is what makes this
        // arm safe without a positional test.
        SyntaxKind::PARAM => Some((Kind::Parameter, DECLARATION)),
        // A field's own name, for the same reason and with the same guarantee.
        SyntaxKind::FIELD => Some((Kind::Property, DECLARATION)),
        // `p.x` — the field half. The receiver is a `NAME_EXPR` child of the same node, so only the token
        // that follows a `.` is a property; the check is positional because both are `IDENT` here.
        SyntaxKind::FIELD_EXPR => {
            if follows_a_dot(token) {
                Some((Kind::Property, 0))
            } else {
                resolved_kind(token, hir)
            }
        }
        // An `@note`'s name.
        SyntaxKind::NOTE => Some((Kind::Decorator, 0)),
        // A `#foreign libc` library name, and the other attribute operands: they name neither a value nor a
        // type, and `Macro` groups them with the directive they belong to.
        SyntaxKind::FOREIGN_ATTR => Some((Kind::Macro, 0)),
        // A named argument's label, `f(x = 1)`.
        SyntaxKind::NAMED_ARG => {
            if follows_a_dot(token) {
                Some((Kind::Property, 0))
            } else {
                Some((Kind::Parameter, 0))
            }
        }
        // A loop label.
        SyntaxKind::LOOP_LABEL => Some((Kind::Variable, DECLARATION)),
        // A bare reference. This is the only place resolution is needed, and the only place context cannot
        // decide: `count` could be a local, a parameter, a constant or a procedure.
        SyntaxKind::NAME_EXPR => resolved_kind(token, hir),
        _ => None,
    }
}

/// Which token kind a `CONST_DECL`'s value implies.
///
/// `Point :: struct {}` is a struct, `f :: () {}` is a function, `N :: 4` is a constant. Read from the
/// declaration's *value* child rather than from a name convention, because this language has none and a
/// convention would be a guess that looks like knowledge.
fn declared_value_kind(decl: &SyntaxNode) -> Kind {
    for child in decl.children() {
        match child.kind() {
            SyntaxKind::STRUCT_TYPE | SyntaxKind::UNION_TYPE | SyntaxKind::VARIANT_TYPE => {
                return Kind::Struct;
            }
            SyntaxKind::ENUM_TYPE => return Kind::Enum,
            SyntaxKind::PROC => return Kind::Function,
            _ => {}
        }
    }
    Kind::Variable
}

/// Whether the token immediately follows a `.`, ignoring trivia.
///
/// The positional test that separates `p.x`'s receiver from its field. Written as a backward scan over
/// siblings rather than by index, because trivia sits between tokens in this CST and an index would count it.
fn follows_a_dot(token: &SyntaxToken) -> bool {
    let mut cursor = token.prev_sibling_or_token();
    while let Some(element) = cursor {
        match element {
            jr_syntax::kind::SyntaxElement::Token(previous) => {
                if previous.kind().is_trivia() {
                    cursor = previous.prev_sibling_or_token();
                    continue;
                }
                return previous.kind() == SyntaxKind::DOT;
            }
            jr_syntax::kind::SyntaxElement::Node(_) => return false,
        }
    }
    false
}

/// A bare name's kind, from the resolution the lowering recorded.
///
/// Falls back to `Variable` when nothing resolved, rather than to no token at all: an unresolved name is
/// still a name, and dropping it would make a file being typed flicker as identifiers appear and disappear.
fn resolved_kind(token: &SyntaxToken, hir: &FileHir) -> Option<(Kind, u32)> {
    let offset = token.text_range().start();
    let res = resolution_at(hir, offset)?;
    let kind = match res {
        Res::Local(_) => Kind::Variable,
        Res::Param(_) => Kind::Parameter,
        // A promoted name is a field reached through `using` (ADR-0050 §2), which is what it *is* to a
        // reader even though it is spelled like a local.
        Res::Promoted { .. } => Kind::Property,
        Res::Item(item) => item_kind(hir, item),
        // An imported name: the module is a namespace, and the name itself is whatever it is over there —
        // which this file's HIR cannot see, so `Variable` is the honest answer rather than a guess.
        Res::Imported(_, _) => Kind::Variable,
        Res::Error => Kind::Variable,
    };
    Some((kind, 0))
}

/// The resolution recorded for the name expression starting at `offset`.
///
/// Scans every body's expressions for a `Name` whose span starts here. A map keyed by offset would be faster
/// and is not worth building: this runs once per identifier in one file, and the alternative is another index
/// to keep in step with lowering.
fn resolution_at(hir: &FileHir, offset: jr_base::TextSize) -> Option<Res> {
    for (index, body) in hir.bodies.iter().enumerate() {
        let scope = ExprScope::Body(jr_hir::BodyId::from_usize(index));
        for id in 0..body.exprs.len() {
            let expr = body.expr(jr_hir::ExprId::from_usize(id));
            if let Expr::Name { span, res, .. } = expr
                && span.start() == offset
            {
                return Some(res.clone());
            }
            let _ = scope;
        }
    }
    None
}

/// What an item declaration is, for a name that resolved to one.
fn item_kind(hir: &FileHir, item: jr_hir::ItemId) -> Kind {
    match &hir.items[item.index()].kind {
        // Exhaustive over both enums rather than using a `_` arm, so adding a declaration form is a
        // compile error here — which is what this project's house rule is for, and what would have caught a
        // `variant` being coloured as an ordinary constant when ADR-0068 added it.
        ItemKind::Const { value } => match value {
            jr_hir::ConstValue::Proc(_) => Kind::Function,
            jr_hir::ConstValue::Operator(_, _) => Kind::Operator,
            jr_hir::ConstValue::Struct(_)
            | jr_hir::ConstValue::Union(_)
            | jr_hir::ConstValue::Variant(_) => Kind::Struct,
            jr_hir::ConstValue::Enum(_) => Kind::Enum,
            jr_hir::ConstValue::Expr(_) => Kind::Variable,
        },
        ItemKind::Var { .. } => Kind::Variable,
        ItemKind::Import { .. } => Kind::Namespace,
        ItemKind::Run { .. } => Kind::Macro,
    }
}

/// A token's length in the negotiated encoding's units.
///
/// Byte length, which is correct for UTF-8 and wrong for UTF-16 on a non-ASCII token — see [`encode`], which
/// is where the conversion belongs because it has the line index.
fn length_of(token: &SyntaxToken) -> u32 {
    u32::from(token.text_range().len())
}

/// Delta-encodes the sorted tokens.
///
/// Five integers each: line delta from the previous token, start delta *within the line* or absolute start on
/// a new line, length, type index, modifier bits. A token spanning a line break is **dropped**, because the
/// protocol has no encoding for one and reporting a truncated length would colour into the next line.
fn encode(tokens: &[Classified], positions: &Positions<'_>) -> Vec<SemanticToken> {
    let mut data = Vec::with_capacity(tokens.len());
    let mut previous_line = 0_u32;
    let mut previous_start = 0_u32;
    for token in tokens {
        let start = positions.position(token.start);
        let end_offset = token.start + jr_base::TextSize::from(token.length);
        let end = positions.position(end_offset);
        if end.line != start.line {
            continue;
        }
        let delta_line = start.line - previous_line;
        let delta_start = if delta_line == 0 {
            start.character - previous_start
        } else {
            start.character
        };
        data.push(SemanticToken {
            delta_line,
            delta_start,
            // The length in the client's units, from the two positions — which is what makes a non-ASCII
            // token correct under UTF-16 rather than a byte count that would overrun.
            length: end.character - start.character,
            token_type: token.kind.index(),
            token_modifiers_bitset: token.modifiers,
        });
        previous_line = start.line;
        previous_start = start.character;
    }
    data
}
