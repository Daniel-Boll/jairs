//! The stdio loop: the main thread writes, a worker reads a snapshot, salsa cancels it.
//!
//! [ADR-0024](../../../docs/adr/0024-language-server.md) §2. The shape is the smallest
//! one that actually exercises what salsa is for:
//!
//! ```text
//!   stdin ─► main thread ─┬─ write (didOpen/didChange): db.set_file_text, then
//!                         │     salsa cancels whatever the worker was doing
//!                         └─ read  (hover/definition/diagnostics): send a snapshot
//!                                   ─► worker ─► response ─► stdout
//! ```
//!
//! # Why the main thread must stay fast
//!
//! salsa cancels readers when a writer wants the next revision, and a writer **blocks
//! until the snapshot count drops back to one**. So the worker taking one snapshot per
//! job and dropping it — including when it unwinds — is not hygiene, it is what keeps
//! the next keystroke from stalling. A worker that cached a snapshot between jobs would
//! turn every edit into a wait for the slowest outstanding request.
//!
//! # Why a cancelled request answers `ContentModified`
//!
//! Because that is what it means: the text the request was computed against no longer
//! exists. The protocol has a code for exactly this, and clients know to re-ask rather
//! than show an error. Answering with a success and stale data would be worse, and
//! answering nothing at all leaves a client waiting.
//!
//! # What it does not do
//!
//! No request queue and no coalescing: jobs run in arrival order, one at a time. Two
//! hovers in flight is not a thing an editor does, and building a scheduler before one
//! is observed would be the same mistake as building `AstIdMap` before measuring.

use std::panic::AssertUnwindSafe;
use std::path::PathBuf;

use crossbeam_channel::Sender;
use jr_db::{JairsDatabase, ModuleSearchPaths, SourceFile};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::{
    Diagnostic, GotoDefinitionResponse, HoverProviderCapability, InitializeParams, OneOf,
    PublishDiagnosticsParams, ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind,
    Uri,
};

use crate::handlers;
use crate::position::Encoding;
use crate::uri;

/// How the server was configured.
#[derive(Debug, Clone, Default)]
pub struct ServerOptions {
    /// Where to look for `#import`ed modules.
    ///
    /// Supplied by the caller rather than discovered, for the reason `jr check
    /// --module-path` exists: guessing a search path silently changes which module a
    /// program means.
    pub module_search_paths: Vec<PathBuf>,
}

/// The capabilities this server advertises, under a negotiated encoding.
///
/// Exactly the three §1.4 asks for. A server that quietly advertised a fourth would be
/// promising something wave W9 owns.
#[must_use]
pub fn capabilities(encoding: Encoding) -> ServerCapabilities {
    ServerCapabilities {
        position_encoding: Some(encoding.kind()),
        // Full rather than incremental: salsa's invalidation grain is the file, so
        // patching ranges buys nothing analytically (ADR-0024 §2).
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        completion_provider: Some(lsp_types::CompletionOptions {
            // `.` for fields and `#` for directives (ADR-0028 §5). Identifier characters
            // are deliberately absent: a client asks on its own as a word is typed, and
            // listing them would mean a request per keystroke.
            trigger_characters: Some(vec![String::from("."), String::from("#")]),
            resolve_provider: Some(true),
            ..lsp_types::CompletionOptions::default()
        }),
        ..ServerCapabilities::default()
    }
}

/// Runs the server over stdin and stdout until the client shuts it down.
///
/// # Errors
/// Any transport or protocol failure. A handler that panics is caught as a cancellation
/// and answered; anything else propagates, because a compiler fault should not be
/// disguised as an empty hover.
pub fn run_stdio(options: &ServerOptions) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (connection, io) = Connection::stdio();

    let (id, params) = connection.initialize_start()?;
    let params: InitializeParams = serde_json::from_value(params)?;
    let encoding = Encoding::negotiate(
        params
            .capabilities
            .general
            .as_ref()
            .and_then(|general| general.position_encodings.as_deref()),
    );
    let init = serde_json::json!({
        "capabilities": capabilities(encoding),
        "serverInfo": { "name": "jairs", "version": env!("CARGO_PKG_VERSION") },
    });
    connection.initialize_finish(id, init)?;

    let mut db = JairsDatabase::default();
    let search_paths = db.set_module_search_paths(options.module_search_paths.clone());

    let (jobs, worker) = spawn_worker(connection.sender.clone(), search_paths, encoding);

    for message in &connection.receiver {
        match message {
            Message::Request(request) => {
                if connection.handle_shutdown(&request)? {
                    break;
                }
                dispatch(&db, &jobs, request);
            }
            Message::Notification(notification) => {
                // A write. It happens on this thread, and salsa cancels whatever the
                // worker had in flight against the previous revision.
                if let Some(file) = apply(&mut db, &notification) {
                    let _ = jobs.send(Job::Diagnostics {
                        db: Box::new(db.snapshot()),
                        file,
                    });
                }
            }
            Message::Response(_) => {}
        }
    }

    // Shutting down is three drops in a fixed order, and getting it wrong hangs the
    // process rather than failing it.
    //
    // Dropping `jobs` is what ends the worker's `for job in receive` loop. Dropping
    // `connection` is what lets `io.join()` return: the writer thread runs until every
    // `Sender` into it is gone, and `connection.sender` is one — so joining while
    // `connection` is still in scope deadlocks. The worker's clone of that sender is the
    // other, which is why the worker is joined first.
    //
    // The stdio test is what found this. Nothing in `jr-lsp`'s handler tests could:
    // every request was answered correctly and the process simply never exited.
    drop(jobs);
    worker.join().map_err(|_| "the worker thread panicked")?;
    drop(connection);
    io.join()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Writes
// ---------------------------------------------------------------------------

/// Applies a text notification, returning the file it touched.
///
/// `None` for a notification this server does not act on, and for one naming a URI that
/// is not a saved file — `uri::to_path` refuses `untitled:` rather than inventing a path
/// and attaching diagnostics to a file that does not exist.
fn apply(db: &mut JairsDatabase, notification: &Notification) -> Option<SourceFile> {
    let (uri_value, text) = match notification.method.as_str() {
        "textDocument/didOpen" => {
            let params: lsp_types::DidOpenTextDocumentParams =
                serde_json::from_value(notification.params.clone()).ok()?;
            (params.text_document.uri, params.text_document.text)
        }
        "textDocument/didChange" => {
            let params: lsp_types::DidChangeTextDocumentParams =
                serde_json::from_value(notification.params.clone()).ok()?;
            // Full sync, so the last change carries the whole document.
            let text = params.content_changes.into_iter().last()?.text;
            (params.text_document.uri, text)
        }
        _ => return None,
    };
    let path = uri::to_path(&uri_value)?;
    let path = path.to_string_lossy().into_owned();
    db.set_file_text(path.clone(), text);
    let file = db.source_file(&path)?;
    db.load_modules_transitively(file);
    Some(file)
}

// ---------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------

/// One unit of read-only work for the worker.
///
/// **Each job owns its snapshot.** That is not a style choice: salsa blocks a writer
/// until the snapshot count drops back to one, so a worker that kept a snapshot in a
/// local between jobs would make every edit wait for it. Bundling the snapshot into the
/// job means the borrow ends exactly when the job does, including when it unwinds — and
/// the first version of this file got it wrong and deadlocked on the second keystroke,
/// which is why the rule is written here rather than remembered.
enum Job {
    Diagnostics {
        db: Box<JairsDatabase>,
        file: SourceFile,
    },
    Hover {
        db: Box<JairsDatabase>,
        id: RequestId,
        file: SourceFile,
        position: lsp_types::Position,
    },
    Definition {
        db: Box<JairsDatabase>,
        id: RequestId,
        file: SourceFile,
        position: lsp_types::Position,
    },
    Completion {
        db: Box<JairsDatabase>,
        id: RequestId,
        file: SourceFile,
        position: lsp_types::Position,
    },
    /// `completionItem/resolve`: fill in one item's documentation.
    ///
    /// Carries the item rather than a position, because by the time a client asks, the
    /// cursor has usually moved on.
    ResolveCompletion {
        db: Box<JairsDatabase>,
        id: RequestId,
        /// `None` for an item with nothing to resolve — a keyword, a builtin type, a
        /// field. The item is then echoed back unchanged, which is what the protocol
        /// wants; answering `null` would make a client drop an item it is displaying.
        file: Option<SourceFile>,
        item: Box<lsp_types::CompletionItem>,
    },
    /// A request naming a file this server has never been told about.
    Unknown { id: RequestId },
}

fn dispatch(db: &JairsDatabase, jobs: &Sender<Job>, request: Request) {
    let id = request.id.clone();
    let job = match request.method.as_str() {
        "textDocument/hover" => serde_json::from_value::<lsp_types::HoverParams>(request.params)
            .ok()
            .and_then(|params| {
                let file = file_of(db, &params.text_document_position_params.text_document.uri)?;
                Some(Job::Hover {
                    db: Box::new(db.snapshot()),
                    id: id.clone(),
                    file,
                    position: params.text_document_position_params.position,
                })
            }),
        "textDocument/definition" => {
            serde_json::from_value::<lsp_types::GotoDefinitionParams>(request.params)
                .ok()
                .and_then(|params| {
                    let file =
                        file_of(db, &params.text_document_position_params.text_document.uri)?;
                    Some(Job::Definition {
                        db: Box::new(db.snapshot()),
                        id: id.clone(),
                        file,
                        position: params.text_document_position_params.position,
                    })
                })
        }
        "textDocument/completion" => {
            serde_json::from_value::<lsp_types::CompletionParams>(request.params)
                .ok()
                .and_then(|params| {
                    let file = file_of(db, &params.text_document_position.text_document.uri)?;
                    Some(Job::Completion {
                        db: Box::new(db.snapshot()),
                        id: id.clone(),
                        file,
                        position: params.text_document_position.position,
                    })
                })
        }
        // A resolve request carries no document, so the item's own `data` says which file
        // declared it. Falling back to *some* open file would resolve the wrong item's
        // docs, which is worse than resolving none.
        "completionItem/resolve" => {
            serde_json::from_value::<lsp_types::CompletionItem>(request.params)
                .ok()
                .map(|item| Job::ResolveCompletion {
                    db: Box::new(db.snapshot()),
                    id: id.clone(),
                    file: resolve_target(db, &item),
                    item: Box::new(item),
                })
        }
        _ => None,
    };
    // Every request gets an answer, including one this server does not implement and one
    // naming a file it has never been told about. A client left waiting is worse than a
    // client told "nothing here".
    let _ = jobs.send(job.unwrap_or(Job::Unknown { id }));
}

fn file_of(db: &JairsDatabase, uri_value: &Uri) -> Option<SourceFile> {
    let path = uri::to_path(uri_value)?;
    db.source_file(path.to_string_lossy().as_ref())
}

/// Starts the reader thread.
fn spawn_worker(
    out: Sender<Message>,
    search_paths: ModuleSearchPaths,
    encoding: Encoding,
) -> (Sender<Job>, std::thread::JoinHandle<()>) {
    let (send, receive) = crossbeam_channel::unbounded::<Job>();
    let handle = std::thread::Builder::new()
        .name(String::from("jairs-lsp-reader"))
        .spawn(move || {
            for job in receive {
                run(&out, search_paths, encoding, job);
            }
        })
        .expect("spawning a thread");
    (send, handle)
}

/// Runs one job, answering a cancellation rather than dying of it.
fn run(out: &Sender<Message>, search_paths: ModuleSearchPaths, encoding: Encoding, job: Job) {
    match job {
        Job::Unknown { id } => {
            let _ = out.send(Message::Response(Response::new_ok(
                id,
                serde_json::Value::Null,
            )));
        }
        Job::Diagnostics { db, file } => {
            let db = db.as_ref();
            let path = file.path(db);
            let computed = catch(|| handlers::diagnostics(db, file, search_paths, encoding));
            // A cancelled diagnostics pass is simply not published: the write that
            // cancelled it will queue another one, so there is nothing to report and
            // nothing to apologise for.
            let Ok(items) = computed else { return };
            publish(out, path.as_ref(), items);
        }
        Job::Hover {
            db,
            id,
            file,
            position,
        } => {
            let db = db.as_ref();
            let computed = catch(|| handlers::hover(db, file, search_paths, encoding, position));
            answer(out, id, computed.map(serde_json::to_value));
        }
        Job::Definition {
            db,
            id,
            file,
            position,
        } => {
            let db = db.as_ref();
            let computed = catch(|| {
                handlers::goto_definition(db, file, search_paths, encoding, position)
                    .map(GotoDefinitionResponse::Scalar)
            });
            answer(out, id, computed.map(serde_json::to_value));
        }
        Job::Completion {
            db,
            id,
            file,
            position,
        } => {
            let db = db.as_ref();
            let computed = catch(|| {
                // `is_incomplete: false`: the list is everything in scope, so a client is
                // free to filter it locally as the user keeps typing rather than asking
                // again on every keystroke.
                lsp_types::CompletionResponse::List(lsp_types::CompletionList {
                    is_incomplete: false,
                    items: crate::completion::completion(
                        db,
                        file,
                        search_paths,
                        encoding,
                        position,
                    ),
                })
            });
            answer(out, id, computed.map(serde_json::to_value));
        }
        Job::ResolveCompletion { db, id, file, item } => {
            let db = db.as_ref();
            let computed = catch(|| match file {
                Some(file) => crate::completion::resolve_completion(db, file, search_paths, *item),
                None => *item,
            });
            answer(out, id, computed.map(serde_json::to_value));
        }
    }
}

/// Which file a `completionItem/resolve` request is about.
///
/// Every resolvable item this server produced carries the requesting file's path in
/// `data`, stamped by `completion::completion`. `None` here means the item is not one of
/// ours or has nothing to resolve — a keyword or a field — and it is echoed back
/// unchanged. Guessing a file would apply an `ItemId` to the wrong file's items and
/// resolve a plausible wrong card.
fn resolve_target(db: &JairsDatabase, item: &lsp_types::CompletionItem) -> Option<SourceFile> {
    let data = item.data.as_ref()?;
    let path = data.get("file").and_then(serde_json::Value::as_str)?;
    db.source_file(path)
}

/// Runs a handler, catching salsa's cancellation unwind.
///
/// `AssertUnwindSafe` is needed because `&JairsDatabase` is not `UnwindSafe` — the
/// `Interner`'s `ThreadedRodeo` is not `RefUnwindSafe` — and asserting it is correct for
/// the same reason rust-analyzer asserts it: the unwind is salsa's own, salsa is designed
/// to be resumed after one, and nothing here mutates the database. The snapshot is
/// dropped either way, which is the invariant that actually matters (ADR-0024 §2).
fn catch<T>(f: impl FnOnce() -> T) -> Result<T, salsa::Cancelled> {
    salsa::Cancelled::catch(AssertUnwindSafe(f))
}

/// Sends a response, turning a cancellation into `ContentModified`.
fn answer(
    out: &Sender<Message>,
    id: RequestId,
    computed: Result<Result<serde_json::Value, serde_json::Error>, salsa::Cancelled>,
) {
    let response = match computed {
        Ok(Ok(value)) => Response::new_ok(id, value),
        // The text this was computed against no longer exists. The protocol has a code
        // for that and clients re-ask; a stale success would be worse.
        Err(cancelled) => {
            Response::new_err(id, ErrorCode::ContentModified as i32, cancelled.to_string())
        }
        Ok(Err(error)) => Response::new_err(id, ErrorCode::InternalError as i32, error.to_string()),
    };
    let _ = out.send(Message::Response(response));
}

fn publish(out: &Sender<Message>, path: &str, items: Vec<Diagnostic>) {
    let Some(uri_value) = uri::from_path(std::path::Path::new(path)) else {
        return;
    };
    let params = PublishDiagnosticsParams {
        uri: uri_value,
        diagnostics: items,
        version: None,
    };
    let Ok(params) = serde_json::to_value(params) else {
        return;
    };
    let _ = out.send(Message::Notification(Notification {
        method: String::from("textDocument/publishDiagnostics"),
        params,
    }));
}
