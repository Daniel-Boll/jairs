//! Scratch probe for parser hangs and stack limits.
//!
//! ```text
//! cargo run -p jr-syntax --example dbg_parse -- --text '<source>'
//! cargo run -p jr-syntax --example dbg_parse -- --prefixes <path.jr>
//! cargo run -p jr-syntax --example dbg_parse -- --stack <bytes> <shape> <depth>
//! ```
//!
//! `--stack` parses a synthetic deeply-nested input on a thread with the given
//! stack size, which is how we calibrate `MAX_DEPTH` against the ~2 MiB stacks
//! that test harnesses and language-server worker threads actually provide.

use std::io::Write;

fn parse_one(label: &str, text: &str) {
    let parsed = jr_syntax::parse(text, jr_base::FileId::from_usize(0));
    let out = parsed.syntax().text().to_string();
    if out != text {
        eprintln!("{label}: ROUND-TRIP MISMATCH");
    }
}

fn synth(shape: &str, depth: usize) -> String {
    match shape {
        "parens" => format!(
            "main :: () {{ x := {}1{}; }}",
            "(".repeat(depth),
            ")".repeat(depth)
        ),
        "blocks" => format!(
            "main :: () {{ {}{} }}",
            "{".repeat(depth),
            "}".repeat(depth)
        ),
        "unary" => format!("main :: () {{ x := {}1; }}", "!".repeat(depth)),
        // `-` repeated does NOT test prefix nesting: `---` lexes as one UNINIT
        // token. Kept as a separate shape so that distinction stays visible.
        "minus" => format!("main :: () {{ x := {}1; }}", "-".repeat(depth)),
        "spaced_minus" => format!("main :: () {{ x := {}1; }}", "- ".repeat(depth)),
        "ptr" => format!("main :: () {{ p: {}s64; }}", "*".repeat(depth)),
        "calls" => format!(
            "main :: () {{ x := f{}{}; }}",
            "(".repeat(depth),
            ")".repeat(depth)
        ),
        "fields" => format!("main :: () {{ x := a{}; }}", ".b".repeat(depth)),
        "deref" => format!("main :: () {{ x := a{}; }}", ".*".repeat(depth)),
        "struct_nesting" => format!(
            "T :: struct {{ {}x: s64;{} }}",
            "f: struct { ".repeat(depth),
            " }".repeat(depth)
        ),
        "binary_chain" => format!("main :: () {{ x := 1{}; }}", " + 1".repeat(depth)),
        other => panic!("unknown shape {other}"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--prefixes") => {
            let path = args.get(2).expect("expected a path");
            let text = std::fs::read_to_string(path).expect("cannot read file");
            for (offset, _) in text.char_indices() {
                eprint!("\r{offset}    ");
                let _ = std::io::stderr().flush();
                parse_one(&format!("offset {offset}"), &text[..offset]);
            }
            eprintln!("\rall {} prefixes ok", text.len());
        }
        Some("--dump") => {
            let text = args[2].clone();
            let parsed = jr_syntax::parse(&text, jr_base::FileId::from_usize(0));
            print!("{}", jr_syntax::dump_tree(&parsed.syntax()));
            eprintln!("diagnostics: {}", parsed.diagnostics().len());
        }
        Some("--stack") => {
            let bytes: usize = args[2].parse().expect("stack bytes");
            let shape = args[3].clone();
            let depth: usize = args[4].parse().expect("depth");
            let handle = std::thread::Builder::new()
                .stack_size(bytes)
                .spawn(move || {
                    let text = synth(&shape, depth);
                    let stage = |s: &str| {
                        eprintln!("stage: {s}");
                        let _ = std::io::stderr().flush();
                    };
                    stage("parsing");
                    let parsed = jr_syntax::parse(&text, jr_base::FileId::from_usize(0));
                    stage("parsed");
                    let node = parsed.syntax();
                    stage("syntax node built");
                    let out = node.text().to_string();
                    stage("text extracted");
                    assert_eq!(out, text, "round-trip mismatch");
                    stage("round-trip verified");
                    drop(node);
                    stage("node dropped");
                    drop(parsed);
                    stage("parse dropped");
                })
                .expect("spawn");
            handle.join().expect("thread panicked");
            eprintln!("ok");
        }
        _ => {
            let text = args.last().cloned().unwrap_or_default();
            eprintln!("parsing {text:?} ({} bytes)", text.len());
            parse_one("text", &text);
            eprintln!("done");
        }
    }
}
