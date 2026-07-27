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
