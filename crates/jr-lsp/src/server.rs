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
//! # The ordering rule on the write side
//!
//! ADR-0024 §2 stated salsa's obligation on the *reader* — snapshot per job, drop it on
//! unwind — and not the matching one on this thread. [ADR-0032](../../../docs/adr/0032-write-before-queue.md):
//! **every write for a notification happens before the snapshot that answers it.** A
//! snapshot is bound to its revision and the next write cancels it, so a job dispatched
//! before a write is a job racing that write. `Job::Diagnostics` answers a cancellation by
//! publishing nothing, which is only correct when the canceller re-queues — `set_file_text`
//! does, `set_workspace_roots` does not. Getting this backwards cost a client with no file
//! watcher its diagnostics on open, 11 times in 16 on a loaded machine, for several waves
//! behind the word "flaky".
//!
//! # What it does not do
//!
//! No request queue and no coalescing: jobs run in arrival order, one at a time. Two
//! hovers in flight is not a thing an editor does, and building a scheduler before one
//! is observed would be the same mistake as building `AstIdMap` before measuring.

use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;

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
/// Twelve now. Each is advertised only where it is implemented for every case a client may
/// send: advertising one that answers "nothing" for half its inputs is worse than not
/// advertising it, because the client stops offering the user an alternative.
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
        references_provider: Some(OneOf::Left(true)),
        document_highlight_provider: Some(OneOf::Left(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        // `prepare_provider` is what lets the server refuse a keyword or a builtin type
        // *before* the user types a replacement (ADR-0030 §3). Without it a client goes
        // straight to `rename`, and the only way to refuse is after the fact.
        rename_provider: Some(OneOf::Right(lsp_types::RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: lsp_types::WorkDoneProgressOptions::default(),
        })),
        // The kinds are listed rather than left to `Some(true)` so a client can put
        // "organise imports" on a menu of its own — several do, and an unlisted kind is
        // reachable only through the generic lightbulb.
        code_action_provider: Some(lsp_types::CodeActionProviderCapability::Options(
            lsp_types::CodeActionOptions {
                code_action_kinds: Some(vec![
                    lsp_types::CodeActionKind::QUICKFIX,
                    lsp_types::CodeActionKind::REFACTOR_REWRITE,
                    lsp_types::CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
                ]),
                // No `codeAction/resolve`: every action here carries its edit already. An
                // edit computed lazily would be computed against a later revision than the
                // one the user was looking at.
                resolve_provider: Some(false),
                work_done_progress_options: lsp_types::WorkDoneProgressOptions::default(),
            },
        )),
        signature_help_provider: Some(lsp_types::SignatureHelpOptions {
            // `(` opens the help and `,` moves to the next parameter. `)` is deliberately
            // absent as a *trigger*: the call is finished there.
            trigger_characters: Some(vec![String::from("("), String::from(",")]),
            retrigger_characters: None,
            work_done_progress_options: lsp_types::WorkDoneProgressOptions::default(),
        }),
        inlay_hint_provider: Some(OneOf::Left(true)),
        ..ServerCapabilities::default()
    }
}

/// Whether the client can watch files for us.
///
/// ADR-0029 §2: with a watcher the file list is refreshed when the filesystem changes;
/// without one it is refreshed on `didOpen` and `didSave`, and the staleness window is much
/// larger. Read from the client's own capabilities rather than assumed, because assuming is
/// what the last two waves each got wrong.
fn client_can_watch(params: &InitializeParams) -> bool {
    params
        .capabilities
        .workspace
        .as_ref()
        .and_then(|workspace| workspace.did_change_watched_files.as_ref())
        .and_then(|watched| watched.dynamic_registration)
        .unwrap_or(false)
}

/// The directories discovery walks: the search paths, plus the client's root.
///
/// ADR-0029 §1. The root arrives from the client at `initialize`; a client that sends none
/// leaves only the search paths, which is correct rather than degraded — that is exactly
/// the situation `jr check --module-path` is in.
fn workspace_roots(options: &ServerOptions, params: &InitializeParams) -> Vec<PathBuf> {
    let mut roots = options.module_search_paths.clone();
    if let Some(folders) = params.workspace_folders.as_ref() {
        for folder in folders {
            if let Some(path) = uri::to_path(&folder.uri) {
                roots.push(path);
            }
        }
    }
    roots
}

/// Asks the client to watch `**/*.jr`.
///
/// Sent as a `client/registerCapability` request. The response is not awaited: the reply
/// carries no information beyond success, and blocking the message loop on it would delay
/// the first `didOpen`.
fn register_watcher(connection: &Connection) {
    let registration = lsp_types::Registration {
        id: String::from("jairs-watch-jr-files"),
        method: String::from("workspace/didChangeWatchedFiles"),
        register_options: serde_json::to_value(
            lsp_types::DidChangeWatchedFilesRegistrationOptions {
                watchers: vec![lsp_types::FileSystemWatcher {
                    glob_pattern: lsp_types::GlobPattern::String(String::from("**/*.jr")),
                    kind: None,
                }],
            },
        )
        .ok(),
    };
    let request = lsp_server::Request::new(
        lsp_server::RequestId::from(String::from("jairs-watch-registration")),
        String::from("client/registerCapability"),
        lsp_types::RegistrationParams {
            registrations: vec![registration],
        },
    );
    let _ = connection.sender.send(Message::Request(request));
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

    // Discovery runs once here, on the main thread, outside any query — which is ADR-0029
    // §2's requirement, not a convenience.
    let roots = workspace_roots(options, &params);
    db.set_workspace_roots(&roots);

    let mut roots = roots;
    let watching = client_can_watch(&params);
    if watching {
        register_watcher(&connection);
    }

    let (jobs, worker) = spawn_worker(connection.sender.clone(), search_paths, encoding);

    for message in &connection.receiver {
        match message {
            Message::Request(request) => {
                if connection.handle_shutdown(&request)? {
                    break;
                }
                dispatch(&mut db, &jobs, request);
            }
            Message::Notification(notification) => {
                if notification.method == "workspace/didChangeWatchedFiles" {
                    // A whole re-walk rather than applying the delta. The notification's
                    // `changes` are enough to patch the list, but a re-walk is one code
                    // path instead of two and cannot drift from the walk's own rules about
                    // symlinks and ignored directories. It is also what the fallback below
                    // does, so both routes converge on the same state.
                    db.set_workspace_roots(&roots);
                    continue;
                }
                // A write. It happens on this thread, and salsa cancels whatever the
                // worker had in flight against the previous revision.
                let touched = apply(&mut db, &notification);

                // **Every write first, then one snapshot, then the job.** The order is the
                // whole point, and getting it wrong is how `didOpen` silently published no
                // diagnostics at all.
                //
                // A snapshot is bound to the revision it was taken in, and the *next* write
                // cancels every reader still holding an older one. `Job::Diagnostics`
                // answers a cancellation by publishing nothing — correctly, because the
                // write that cancelled it normally queues a replacement. `set_workspace_roots`
                // is the writer that does **not**: it is the workspace list changing, not the
                // file, so nothing re-queues, and the diagnostics for the file the user just
                // opened are never published.
                //
                // It reproduced 5 times in 12 under CPU load and 0 times in 12 idle — a race
                // whose window is the walk between the snapshot and the set — which is why it
                // survived several waves as "a flaky test" rather than being read as the
                // user-visible bug it is: a real editor with no file watcher gets no
                // diagnostics on open, on a loaded machine, some of the time.
                if let Some(file) = touched {
                    // A file the workspace does not cover contributes its own directory as
                    // a root. A client that sends no `workspaceFolders` — or a user opening
                    // a scratch file outside the tree — would otherwise get a rename and a
                    // reference search over an empty file list, which look like working
                    // features returning "only the declaration".
                    if adopt_root(&mut db, &mut roots, file) {
                        db.set_workspace_roots(&roots);
                    }
                }
                // The fallback for a client that cannot watch (ADR-0029 §2). Re-walking on
                // every keystroke would be indefensible, so it is tied to open and save —
                // which is why a client *with* a watcher is much fresher, and why the
                // difference is stated rather than hidden.
                if !watching
                    && matches!(
                        notification.method.as_str(),
                        "textDocument/didOpen" | "textDocument/didSave"
                    )
                {
                    db.set_workspace_roots(&roots);
                }
                // Only now, with no write left to cancel it.
                if let Some(file) = touched {
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
    References {
        db: Box<JairsDatabase>,
        id: RequestId,
        file: SourceFile,
        position: lsp_types::Position,
        include_declaration: bool,
        /// The workspace list, captured at dispatch so the job sees one consistent set.
        workspace: Arc<jr_db::WorkspaceFileList>,
    },
    Highlight {
        db: Box<JairsDatabase>,
        id: RequestId,
        file: SourceFile,
        position: lsp_types::Position,
    },
    PrepareRename {
        db: Box<JairsDatabase>,
        id: RequestId,
        file: SourceFile,
        position: lsp_types::Position,
    },
    Rename {
        db: Box<JairsDatabase>,
        id: RequestId,
        file: SourceFile,
        position: lsp_types::Position,
        new_name: String,
        workspace: Arc<jr_db::WorkspaceFileList>,
    },
    DocumentSymbols {
        db: Box<JairsDatabase>,
        id: RequestId,
        file: SourceFile,
    },
    WorkspaceSymbols {
        db: Box<JairsDatabase>,
        id: RequestId,
        query: String,
        workspace: Arc<jr_db::WorkspaceFileList>,
    },
    CodeActions {
        db: Box<JairsDatabase>,
        id: RequestId,
        file: SourceFile,
        range: lsp_types::Range,
        /// The diagnostics the *client* holds for that range.
        ///
        /// Taken from the request rather than recomputed, because an action must appear
        /// exactly where the user already sees a problem — and because a client may hold a
        /// diagnostic from a revision this snapshot has moved past (ADR-0031 §4).
        diagnostics: Vec<lsp_types::Diagnostic>,
        /// Captured at dispatch: auto-import consults the discovered modules.
        workspace: Arc<jr_db::WorkspaceFileList>,
    },
    SignatureHelp {
        db: Box<JairsDatabase>,
        id: RequestId,
        file: SourceFile,
        position: lsp_types::Position,
    },
    InlayHints {
        db: Box<JairsDatabase>,
        id: RequestId,
        file: SourceFile,
        range: lsp_types::Range,
    },
    /// A request naming a file this server has never been told about.
    Unknown { id: RequestId },
}

/// Adds the opened file's directory to `roots` when discovery does not already cover it.
///
/// Returns whether `roots` changed, so the caller only re-walks when it must.
fn adopt_root(db: &mut JairsDatabase, roots: &mut Vec<PathBuf>, file: SourceFile) -> bool {
    let path = PathBuf::from(file.path(db).as_ref());
    let covered = db
        .workspace_files()
        .map(|files| files.list(db))
        .is_some_and(|list| list.contains(&path));
    if covered {
        return false;
    }
    let Some(parent) = path.parent().map(std::path::Path::to_path_buf) else {
        return false;
    };
    if roots.contains(&parent) {
        return false;
    }
    roots.push(parent);
    true
}

/// Whether answering this request requires every workspace file to be in the database.
///
/// Checked before the snapshot is taken, because loading is a write and a job holds a
/// read-only snapshot. Getting this list wrong is not a crash — it is a reference search
/// that quietly sees only the files the editor happened to open, which is exactly the
/// confident wrong answer ADR-0029 §3 warns about.
fn needs_whole_workspace(method: &str) -> bool {
    matches!(
        method,
        "textDocument/references"
            | "textDocument/rename"
            | "workspace/symbol"
            // Auto-import must know which discovered module exports the missing name, and
            // discovery yields paths rather than loaded files (ADR-0031 §5). Without this
            // the quick fix silently offers only modules the editor happens to have open,
            // and an absent offer reads as "there is nothing to import".
            | "textDocument/codeAction"
    )
}

fn dispatch(db: &mut JairsDatabase, jobs: &Sender<Job>, request: Request) {
    if needs_whole_workspace(&request.method) {
        db.load_workspace_files();
    }
    let workspace = db
        .workspace_files()
        .map(|files| files.list(db))
        .unwrap_or_default();

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
        "textDocument/references" => {
            serde_json::from_value::<lsp_types::ReferenceParams>(request.params)
                .ok()
                .and_then(|params| {
                    let file = file_of(db, &params.text_document_position.text_document.uri)?;
                    Some(Job::References {
                        db: Box::new(db.snapshot()),
                        id: id.clone(),
                        file,
                        position: params.text_document_position.position,
                        include_declaration: params.context.include_declaration,
                        workspace: Arc::clone(&workspace),
                    })
                })
        }
        "textDocument/documentHighlight" => serde_json::from_value::<
            lsp_types::DocumentHighlightParams,
        >(request.params)
        .ok()
        .and_then(|params| {
            let file = file_of(db, &params.text_document_position_params.text_document.uri)?;
            Some(Job::Highlight {
                db: Box::new(db.snapshot()),
                id: id.clone(),
                file,
                position: params.text_document_position_params.position,
            })
        }),
        "textDocument/prepareRename" => {
            serde_json::from_value::<lsp_types::TextDocumentPositionParams>(request.params)
                .ok()
                .and_then(|params| {
                    let file = file_of(db, &params.text_document.uri)?;
                    Some(Job::PrepareRename {
                        db: Box::new(db.snapshot()),
                        id: id.clone(),
                        file,
                        position: params.position,
                    })
                })
        }
        "textDocument/rename" => serde_json::from_value::<lsp_types::RenameParams>(request.params)
            .ok()
            .and_then(|params| {
                let file = file_of(db, &params.text_document_position.text_document.uri)?;
                Some(Job::Rename {
                    db: Box::new(db.snapshot()),
                    id: id.clone(),
                    file,
                    position: params.text_document_position.position,
                    new_name: params.new_name,
                    workspace: Arc::clone(&workspace),
                })
            }),
        "textDocument/documentSymbol" => {
            serde_json::from_value::<lsp_types::DocumentSymbolParams>(request.params)
                .ok()
                .and_then(|params| {
                    let file = file_of(db, &params.text_document.uri)?;
                    Some(Job::DocumentSymbols {
                        db: Box::new(db.snapshot()),
                        id: id.clone(),
                        file,
                    })
                })
        }
        "workspace/symbol" => {
            serde_json::from_value::<lsp_types::WorkspaceSymbolParams>(request.params)
                .ok()
                .map(|params| Job::WorkspaceSymbols {
                    db: Box::new(db.snapshot()),
                    id: id.clone(),
                    query: params.query,
                    workspace: Arc::clone(&workspace),
                })
        }
        "textDocument/codeAction" => {
            serde_json::from_value::<lsp_types::CodeActionParams>(request.params)
                .ok()
                .and_then(|params| {
                    let file = file_of(db, &params.text_document.uri)?;
                    Some(Job::CodeActions {
                        db: Box::new(db.snapshot()),
                        id: id.clone(),
                        file,
                        range: params.range,
                        diagnostics: params.context.diagnostics,
                        workspace: Arc::clone(&workspace),
                    })
                })
        }
        "textDocument/signatureHelp" => {
            serde_json::from_value::<lsp_types::SignatureHelpParams>(request.params)
                .ok()
                .and_then(|params| {
                    let file =
                        file_of(db, &params.text_document_position_params.text_document.uri)?;
                    Some(Job::SignatureHelp {
                        db: Box::new(db.snapshot()),
                        id: id.clone(),
                        file,
                        position: params.text_document_position_params.position,
                    })
                })
        }
        "textDocument/inlayHint" => {
            serde_json::from_value::<lsp_types::InlayHintParams>(request.params)
                .ok()
                .and_then(|params| {
                    let file = file_of(db, &params.text_document.uri)?;
                    Some(Job::InlayHints {
                        db: Box::new(db.snapshot()),
                        id: id.clone(),
                        file,
                        range: params.range,
                    })
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
            // A cancelled diagnostics pass is not published — but that is only correct
            // when a **re-queueing** writer cancelled it (ADR-0032 §2). `set_file_text`
            // re-queues; `set_workspace_roots` does not, and this comment used to claim
            // otherwise, which is how `didOpen` came to silently publish nothing at all
            // for a client with no file watcher. What makes silence safe here is §1's
            // ordering: the job is dispatched after every write, so nothing is left to
            // cancel it except the next real edit.
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
        Job::References {
            db,
            id,
            file,
            position,
            include_declaration,
            workspace,
        } => {
            let db = db.as_ref();
            let computed = catch(|| {
                crate::navigate::find_references(
                    db,
                    file,
                    search_paths,
                    encoding,
                    position,
                    include_declaration,
                    &workspace.files,
                )
            });
            answer(out, id, computed.map(serde_json::to_value));
        }
        Job::Highlight {
            db,
            id,
            file,
            position,
        } => {
            let db = db.as_ref();
            let computed = catch(|| {
                crate::navigate::document_highlight(db, file, search_paths, encoding, position)
            });
            answer(out, id, computed.map(serde_json::to_value));
        }
        Job::PrepareRename {
            db,
            id,
            file,
            position,
        } => {
            let db = db.as_ref();
            let computed = catch(|| {
                crate::navigate::prepare_rename(db, file, search_paths, encoding, position)
            });
            answer(out, id, computed.map(serde_json::to_value));
        }
        Job::Rename {
            db,
            id,
            file,
            position,
            new_name,
            workspace,
        } => {
            let db = db.as_ref();
            let computed = catch(|| {
                crate::navigate::rename(
                    db,
                    file,
                    search_paths,
                    encoding,
                    position,
                    &new_name,
                    workspace.as_ref(),
                )
            });
            match computed {
                // A refusal is an error *response*, not an empty edit: a client that gets
                // `null` shows nothing and the user concludes rename is broken, where an
                // error message says which of ADR-0030 §3's five reasons applied.
                Ok(Err(refusal)) => {
                    let _ = out.send(Message::Response(Response::new_err(
                        id,
                        lsp_server::ErrorCode::RequestFailed as i32,
                        refusal.to_string(),
                    )));
                }
                Ok(Ok(edit)) => answer(out, id, Ok(serde_json::to_value(edit))),
                Err(cancelled) => answer(out, id, Err(cancelled)),
            }
        }
        Job::DocumentSymbols { db, id, file } => {
            let db = db.as_ref();
            let computed = catch(|| {
                lsp_types::DocumentSymbolResponse::Nested(crate::navigate::document_symbol(
                    db,
                    file,
                    search_paths,
                    encoding,
                ))
            });
            answer(out, id, computed.map(serde_json::to_value));
        }
        Job::WorkspaceSymbols {
            db,
            id,
            query,
            workspace,
        } => {
            let db = db.as_ref();
            let computed = catch(|| {
                crate::navigate::workspace_symbol(
                    db,
                    search_paths,
                    encoding,
                    &query,
                    &workspace.files,
                )
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
        Job::CodeActions {
            db,
            id,
            file,
            range,
            diagnostics,
            workspace,
        } => {
            let db = db.as_ref();
            let computed = catch(|| {
                crate::actions::code_actions(
                    db,
                    file,
                    search_paths,
                    encoding,
                    range,
                    &diagnostics,
                    workspace.as_ref(),
                )
            });
            answer(out, id, computed.map(serde_json::to_value));
        }
        Job::SignatureHelp {
            db,
            id,
            file,
            position,
        } => {
            let db = db.as_ref();
            let computed =
                catch(|| crate::hints::signature_help(db, file, search_paths, encoding, position));
            answer(out, id, computed.map(serde_json::to_value));
        }
        Job::InlayHints {
            db,
            id,
            file,
            range,
        } => {
            let db = db.as_ref();
            let computed =
                catch(|| crate::hints::inlay_hints(db, file, search_paths, encoding, range));
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
