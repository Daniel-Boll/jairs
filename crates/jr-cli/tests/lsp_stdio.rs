//! One test that speaks the real LSP protocol to the real binary.
//!
//! # Why this exists when `jr-lsp`'s own tests already pass
//!
//! Because those tests would pass with a completely broken transport, and
//! [ADR-0024](../../../docs/adr/0024-language-server.md) §4 says so for a specific
//! reason. The first native run of `024-hello.jr` printed both its lines perfectly and
//! exited **1**, because a `void`-returning Jairs `main` left the C runtime whatever was
//! in the return register. Output alone said the back end worked. A language server's
//! transport is the same kind of surface: plausible-looking JSON, wrong framing, nothing
//! to see.
//!
//! # Why it lives in `jr-cli` and not in `jr-lsp`
//!
//! `CARGO_BIN_EXE_jr` is only defined for an integration test of the crate that declares
//! the binary, and cargo guarantees the binary is built before it runs. From `jr-lsp` the
//! test would have to guess a target path and would silently exercise a stale build —
//! and `jr-lsp` cannot depend on `jr-cli` anyway, because `jr-cli` depends on it.
//! `differential.rs` is here for the same reason.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A running server, with framed reads and writes.
struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Server {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_jr"))
            .arg("lsp")
            .arg("--quiet")
            .arg("--module-path")
            .arg(workspace_root().join("modules"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the `jr` binary is built before its own integration tests");
        let stdin = child.stdin.take().expect("piped");
        let stdout = BufReader::new(child.stdout.take().expect("piped"));
        Self {
            child,
            stdin,
            stdout,
        }
    }

    /// Writes one message with LSP's `Content-Length` framing.
    fn send(&mut self, value: &serde_json::Value) {
        let body = serde_json::to_string(value).expect("serialisable");
        write!(self.stdin, "Content-Length: {}\r\n\r\n{body}", body.len())
            .expect("the server is still listening");
        self.stdin.flush().expect("flushable");
    }

    /// Reads one framed message.
    ///
    /// The framing is parsed by hand rather than with a helper, because the framing is
    /// what this test is checking: a server that wrote the wrong length, or `\n` instead
    /// of `\r\n`, fails here rather than somewhere confusing later.
    fn receive(&mut self) -> serde_json::Value {
        let mut length: Option<usize> = None;
        loop {
            let mut line = String::new();
            let read = self
                .stdout
                .read_line(&mut line)
                .expect("the server must not close mid-header");
            assert_ne!(read, 0, "the server closed stdout before replying");
            if line == "\r\n" {
                break;
            }
            if let Some(rest) = line.strip_prefix("Content-Length: ") {
                length = Some(rest.trim().parse().expect("a numeric content length"));
            }
        }
        let length = length.expect("every message must carry a Content-Length");
        let mut body = vec![0u8; length];
        self.stdout
            .read_exact(&mut body)
            .expect("the body must be as long as the header said");
        serde_json::from_slice(&body).expect("a message body must be JSON")
    }

    /// Reads until a message with the given id arrives, skipping notifications.
    ///
    /// Skipping is necessary rather than tidy: the server publishes diagnostics after
    /// `didOpen`, so a response is not the next thing on the wire.
    fn response(&mut self, id: i64) -> serde_json::Value {
        for _ in 0..32 {
            let message = self.receive();
            if message.get("id").and_then(serde_json::Value::as_i64) == Some(id) {
                return message;
            }
        }
        panic!("no response with id {id} arrived");
    }

    fn initialize(&mut self, encodings: serde_json::Value) -> serde_json::Value {
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "capabilities": { "general": { "positionEncodings": encodings } } }
        }));
        let reply = self.response(1);
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "initialized",
            "params": {}
        }));
        reply
    }

    fn did_open(&mut self, uri: &str, text: &str) {
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": uri,
                    "languageId": "jairs",
                    "version": 1,
                    "text": text
                }
            }
        }));
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn the_server_completes_a_handshake_and_answers_a_hover() {
    let dir = tempfile::TempDir::new().expect("a temporary directory");
    let path = dir.path().join("main.jr");
    let source = "main :: () {\n    n := 7;\n    m := n;\n}\n";
    std::fs::write(&path, source).expect("a writable temporary directory");
    let uri = format!("file://{}", path.display());

    let mut server = Server::start();
    let initialized = server.initialize(serde_json::json!(["utf-8", "utf-16"]));
    assert_eq!(
        initialized["result"]["capabilities"]["positionEncoding"], "utf-8",
        "the server must echo the encoding it chose, or every column is a guess"
    );
    assert_eq!(
        initialized["result"]["capabilities"]["hoverProvider"], true,
        "a client that is not told about hover will never send one"
    );
    assert_eq!(
        initialized["result"]["capabilities"]["definitionProvider"],
        true
    );

    server.did_open(&uri, source);

    // Line 2, character 9 is the `n` in `m := n;`.
    server.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "textDocument/hover",
        "params": {
            "textDocument": { "uri": uri },
            "position": { "line": 2, "character": 9 }
        }
    }));
    let hovered = server.response(2);
    assert!(
        hovered["error"].is_null(),
        "the hover failed: {}",
        hovered["error"]
    );
    let value = hovered["result"]["contents"]["value"]
        .as_str()
        .unwrap_or_default();
    assert!(
        value.contains("s64"),
        "the hover should name the type, got {value:?}"
    );

    server.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "shutdown",
        "params": serde_json::Value::Null
    }));
    let shut = server.response(3);
    assert!(shut["error"].is_null(), "shutdown failed: {shut:?}");
    server.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "method": "exit",
        "params": {}
    }));

    let status = server
        .child
        .wait()
        .expect("the server must exit after `exit`");
    assert!(
        status.success(),
        "a clean shutdown must exit 0, or an editor reports the server as crashed"
    );
}

#[test]
fn the_negotiated_encoding_is_utf16_when_utf8_is_not_offered() {
    // ADR-0024 §3's fallback, over the wire. A server that advertised `utf-8` to a
    // client that never offered it would misplace every column on a non-ASCII line, and
    // the client would have no way to know.
    let mut server = Server::start();
    let initialized = server.initialize(serde_json::json!(["utf-16"]));
    assert_eq!(
        initialized["result"]["capabilities"]["positionEncoding"],
        "utf-16"
    );
}

#[test]
fn a_relative_module_path_still_resolves_across_an_import() {
    // A regression test for a *silent* failure. A `Location` needs a `file:` URI and
    // `jr_lsp::uri::from_path` correctly refuses a relative path, so a server started
    // with `--module-path modules` — which is what a person types first — answered
    // goto-definition into a module with "nothing here" rather than an error. The fix
    // absolutises the search path once, in `jr lsp`, because a server's working
    // directory is whatever the editor happened to have.
    //
    // Run from the workspace root so that `modules` is a meaningful relative path, which
    // is the whole point of the case.
    let mut child = Command::new(env!("CARGO_BIN_EXE_jr"))
        .arg("lsp")
        .arg("--quiet")
        .arg("--module-path")
        .arg("modules")
        .current_dir(workspace_root())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the `jr` binary is built before its own integration tests");
    let stdin = child.stdin.take().expect("piped");
    let stdout = BufReader::new(child.stdout.take().expect("piped"));
    let mut server = Server {
        child,
        stdin,
        stdout,
    };

    let path = workspace_root()
        .join("tests/corpus/valid/024-hello.jr")
        .canonicalize()
        .expect("the exit criterion's file must exist");
    let source = std::fs::read_to_string(&path).expect("readable");
    let uri = format!("file://{}", path.display());

    let _ = server.initialize(serde_json::json!(["utf-8"]));
    server.did_open(&uri, &source);

    // Line 30 is `        print(MESSAGE);`; character 8 is the `p` of `print`, which
    // resolves through the `#import` into `modules/Basic`.
    server.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "textDocument/definition",
        "params": {
            "textDocument": { "uri": uri },
            "position": { "line": 30, "character": 8 }
        }
    }));
    let found = server.response(2);
    let target = found["result"]["uri"].as_str().unwrap_or_default();
    assert!(
        target.ends_with("modules/Basic/module.jr"),
        "expected a location in the Basic module, got {target:?}"
    );
}

#[test]
fn opening_a_broken_file_publishes_diagnostics() {
    // The other half of the transport: a *notification* the server sends unprompted. A
    // handler test cannot see this at all, because nothing calls it — the server decides
    // to publish.
    let dir = tempfile::TempDir::new().expect("a temporary directory");
    let path = dir.path().join("broken.jr");
    let source = "main :: () {\n    x: bool = 1;\n}\n";
    std::fs::write(&path, source).expect("a writable temporary directory");
    let uri = format!("file://{}", path.display());

    let mut server = Server::start();
    let _ = server.initialize(serde_json::json!(["utf-8"]));
    server.did_open(&uri, source);

    for _ in 0..32 {
        let message = server.receive();
        if message["method"] == "textDocument/publishDiagnostics" {
            let items = message["params"]["diagnostics"]
                .as_array()
                .expect("an array of diagnostics");
            assert!(!items.is_empty(), "a type error must be published");
            assert_eq!(items[0]["source"], "jairs");
            return;
        }
    }
    panic!("the server never published diagnostics");
}

/// Completion and `completionItem/resolve`, over the real transport.
///
/// ADR-0024 §4's rule, applied to the capabilities ADR-0028 added: a handler test would
/// pass with a broken transport, and the first stdio test in this file caught a deadlock
/// no in-process assertion could see. Resolve is the interesting half — it is a second
/// round trip whose request carries no document, so the item's own `data` has to survive
/// serialisation to the client and back.
#[test]
fn the_server_advertises_completion_and_resolves_an_item() {
    let dir = tempfile::TempDir::new().expect("a temporary directory");
    let path = dir.path().join("main.jr");
    let source = "/// Adds two numbers.\nadd :: (a: s64, b: s64) -> s64 {\n    return a + b;\n}\n\nmain :: () {\n    n := a\n}\n";
    std::fs::write(&path, source).expect("a writable temporary directory");
    let uri = format!("file://{}", path.display());

    let mut server = Server::start();
    let initialized = server.initialize(serde_json::json!(["utf-8"]));

    let completion = &initialized["result"]["capabilities"]["completionProvider"];
    assert!(
        !completion.is_null(),
        "a client that is not told about completion will never send one"
    );
    assert_eq!(
        completion["resolveProvider"], true,
        "documentation is lazy, so resolve must be advertised or it never arrives"
    );
    let triggers = completion["triggerCharacters"]
        .as_array()
        .expect("trigger characters");
    assert!(
        triggers.iter().any(|c| c == ".") && triggers.iter().any(|c| c == "#"),
        "`.` and `#` are what open a field and a directive list: {triggers:?}"
    );

    server.did_open(&uri, source);

    // Line 6, character 10: just after the `a` in `n := a`.
    server.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "textDocument/completion",
        "params": {
            "textDocument": { "uri": uri },
            "position": { "line": 6, "character": 10 }
        }
    }));
    let completed = server.response(2);
    assert!(
        completed["error"].is_null(),
        "the completion failed: {}",
        completed["error"]
    );
    let items = completed["result"]["items"]
        .as_array()
        .expect("a completion list")
        .clone();
    let add = items
        .iter()
        .find(|item| item["label"] == "add")
        .unwrap_or_else(|| panic!("`add` must be offered: {items:?}"))
        .clone();
    assert_eq!(add["detail"], "add :: (a: s64, b: s64) -> s64");
    assert_eq!(add["insertText"], "add(${1:a}, ${2:b})$0");
    assert_eq!(add["insertTextFormat"], 2, "2 is Snippet");
    assert!(
        add["documentation"].is_null(),
        "documentation must be lazy in the list"
    );

    // Round-trip the item exactly as a client would, `data` and all.
    server.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "completionItem/resolve",
        "params": add
    }));
    let resolved = server.response(3);
    assert!(
        resolved["error"].is_null(),
        "the resolve failed: {}",
        resolved["error"]
    );
    let docs = resolved["result"]["documentation"]["value"]
        .as_str()
        .unwrap_or_default();
    assert_eq!(
        docs, "```jr\nmain\nadd :: (a: s64, b: s64) -> s64\n```\n\n---\n\nAdds two numbers.",
        "resolve must render the same card the hover does"
    );

    server.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "shutdown",
        "params": serde_json::Value::Null
    }));
    assert!(server.response(4)["error"].is_null());
    server.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "method": "exit",
        "params": {}
    }));
    let status = server.child.wait().expect("the server must exit");
    assert!(status.success(), "a clean shutdown must exit 0");
}

/// Rename, references and symbols over the real transport, including a refusal.
///
/// The refusal is the half a handler test cannot check: ADR-0030 §3 says a refused rename is
/// an error *response* rather than an empty edit, and whether that survives serialisation —
/// and whether the client sees a message rather than `null` — is a transport property.
#[test]
fn the_server_answers_navigation_requests_and_refuses_a_bad_rename() {
    let dir = tempfile::TempDir::new().expect("a temporary directory");
    let path = dir.path().join("main.jr");
    let source = "first :: 1;\nsecond :: 2;\n\nmain :: () {\n    n := first;\n}\n";
    std::fs::write(&path, source).expect("a writable temporary directory");
    let uri = format!("file://{}", path.display());

    let mut server = Server::start();
    let initialized = server.initialize(serde_json::json!(["utf-8"]));
    let caps = &initialized["result"]["capabilities"];
    assert_eq!(caps["referencesProvider"], true);
    assert_eq!(caps["documentHighlightProvider"], true);
    assert_eq!(caps["documentSymbolProvider"], true);
    assert_eq!(caps["workspaceSymbolProvider"], true);
    assert_eq!(
        caps["renameProvider"]["prepareProvider"], true,
        "without prepareProvider a client cannot be told `while` is not renameable"
    );

    server.did_open(&uri, source);

    // documentSymbol.
    server.send(&serde_json::json!({
        "jsonrpc": "2.0", "id": 2,
        "method": "textDocument/documentSymbol",
        "params": { "textDocument": { "uri": uri } }
    }));
    let symbols = server.response(2);
    assert!(symbols["error"].is_null(), "{}", symbols["error"]);
    let names: Vec<String> = symbols["result"]
        .as_array()
        .expect("an outline")
        .iter()
        .map(|s| s["name"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(names, vec!["first", "second", "main"]);

    // references on `first`, line 4 character 9.
    server.send(&serde_json::json!({
        "jsonrpc": "2.0", "id": 3,
        "method": "textDocument/references",
        "params": {
            "textDocument": { "uri": uri },
            "position": { "line": 4, "character": 9 },
            "context": { "includeDeclaration": true }
        }
    }));
    let found = server.response(3);
    assert!(found["error"].is_null(), "{}", found["error"]);
    assert_eq!(
        found["result"].as_array().expect("locations").len(),
        2,
        "the declaration and one use: {}",
        found["result"]
    );

    // prepareRename on the same position.
    server.send(&serde_json::json!({
        "jsonrpc": "2.0", "id": 4,
        "method": "textDocument/prepareRename",
        "params": {
            "textDocument": { "uri": uri },
            "position": { "line": 4, "character": 9 }
        }
    }));
    let prepared = server.response(4);
    assert!(prepared["error"].is_null(), "{}", prepared["error"]);
    assert_eq!(prepared["result"]["placeholder"], "first");

    // A rename that collides must come back as an error with a message, not as null.
    server.send(&serde_json::json!({
        "jsonrpc": "2.0", "id": 5,
        "method": "textDocument/rename",
        "params": {
            "textDocument": { "uri": uri },
            "position": { "line": 4, "character": 9 },
            "newName": "second"
        }
    }));
    let refused = server.response(5);
    assert!(
        !refused["error"].is_null(),
        "a colliding rename must be refused, got {refused}"
    );
    let message = refused["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("already declared"),
        "the refusal must say why: {message:?}"
    );

    // And a legal one produces edits.
    server.send(&serde_json::json!({
        "jsonrpc": "2.0", "id": 6,
        "method": "textDocument/rename",
        "params": {
            "textDocument": { "uri": uri },
            "position": { "line": 4, "character": 9 },
            "newName": "renamed"
        }
    }));
    let renamed = server.response(6);
    assert!(renamed["error"].is_null(), "{}", renamed["error"]);
    let changes = renamed["result"]["changes"]
        .as_object()
        .expect("a WorkspaceEdit with changes");
    let edits = changes.values().next().expect("one file's edits");
    assert_eq!(edits.as_array().expect("edits").len(), 2);

    server.send(&serde_json::json!({
        "jsonrpc": "2.0", "id": 7, "method": "shutdown", "params": serde_json::Value::Null
    }));
    assert!(server.response(7)["error"].is_null());
    server.send(&serde_json::json!({ "jsonrpc": "2.0", "method": "exit", "params": {} }));
    assert!(
        server.child.wait().expect("the server must exit").success(),
        "a clean shutdown must exit 0"
    );
}

/// A client that advertises watched-file support is asked to watch `**/*.jr`.
///
/// ADR-0029 §2 makes the watcher the primary freshness mechanism, so the registration going
/// out at all is worth asserting: without it the server silently falls back to re-walking on
/// save, and nothing else would notice.
#[test]
fn the_server_registers_a_file_watcher_when_the_client_can_watch() {
    let mut server = Server::start();
    server.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "capabilities": {
                "general": { "positionEncodings": ["utf-8"] },
                "workspace": {
                    "didChangeWatchedFiles": { "dynamicRegistration": true }
                }
            }
        }
    }));
    let _ = server.response(1);
    server.send(&serde_json::json!({
        "jsonrpc": "2.0", "method": "initialized", "params": {}
    }));

    // The registration is a *server-to-client* request, so it arrives as a message with a
    // method rather than as a response to anything.
    let mut registered = None;
    for _ in 0..10 {
        let message = server.receive();
        if message["method"] == "client/registerCapability" {
            registered = Some(message);
            break;
        }
    }
    let registration = registered.expect("the server must ask the client to watch files");
    let first = &registration["params"]["registrations"][0];
    assert_eq!(first["method"], "workspace/didChangeWatchedFiles");
    assert_eq!(
        first["registerOptions"]["watchers"][0]["globPattern"], "**/*.jr",
        "the watcher must cover Jairs sources: {registration}"
    );

    server.send(&serde_json::json!({
        "jsonrpc": "2.0", "id": 99, "method": "shutdown", "params": serde_json::Value::Null
    }));
    server.send(&serde_json::json!({ "jsonrpc": "2.0", "method": "exit", "params": {} }));
    let _ = server.child.wait();
}
