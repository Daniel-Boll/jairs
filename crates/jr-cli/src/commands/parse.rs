//! Implementation of `jr parse`.

use anyhow::Result;
use jr_base::SourceMap;
use jr_syntax::{dump_tree, lex, parse};

use crate::cli::{GlobalArgs, ParseArgs};
use crate::files::read_file;
use crate::report::{emit_diagnostics, make_renderer};

/// Run `jr parse`.
///
/// Returns exit code 0 if no errors, 1 if any errors were found, 3 on I/O
/// failure (propagated as `Err`).
pub fn run(args: ParseArgs, global: &GlobalArgs) -> Result<i32> {
    let colour = global.color.resolve();
    let renderer = make_renderer(colour);

    let text = read_file(&args.path)?;
    let mut map = SourceMap::new();
    let file_id = map.add(args.path.as_path(), &text);

    if args.tokens {
        // Print one token per line: `KIND range "text"`.
        let lex_out = lex(&text, file_id);
        for tok in &lex_out.tokens {
            let tok_text = &text[tok.range];
            let escaped = tok_text
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
                .replace('\r', "\\r");
            println!(
                "{:?} {}..{} \"{}\"",
                tok.kind,
                u32::from(tok.range.start()),
                u32::from(tok.range.end()),
                escaped
            );
        }
        // Also report lex diagnostics.
        emit_diagnostics(&renderer, &map, &lex_out.diagnostics);
        return Ok(if lex_out.diagnostics.has_errors() {
            1
        } else {
            0
        });
    }

    let parsed = parse(&text, file_id);
    let diags = parsed.diagnostics();

    if args.dump {
        let tree = dump_tree(&parsed.syntax());
        print!("{tree}");
    } else {
        // Summary mode.
        let lex_out = lex(&text, file_id);
        let token_count = lex_out.tokens.len();
        let node_count = count_nodes(&parsed.syntax());
        let diag_count = diags.len();
        println!(
            "{}: {} tokens, {} nodes, {} diagnostics",
            args.path.display(),
            token_count,
            node_count,
            diag_count
        );
    }

    emit_diagnostics(&renderer, &map, diags);

    Ok(if diags.has_errors() { 1 } else { 0 })
}

/// Count all syntax nodes (non-token elements) in the tree.
fn count_nodes(node: &jr_syntax::SyntaxNode) -> usize {
    1 + node
        .children()
        .map(|child| count_nodes(&child))
        .sum::<usize>()
}
