//! The LSP loop: document store, workspace index and request handlers.

use std::collections::{BTreeMap, HashSet};
use std::error::Error;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use lsp_server::{Connection, Message, Notification, Request, Response};
use lsp_types::notification::{self as notif, Notification as _};
use lsp_types::request::{self as req, Request as _};
use lsp_types::*;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::analysis::{self, Analysis, Kind, Shape, Symbol};
use crate::line_index::LineIndex;

type Result<T> = std::result::Result<T, Box<dyn Error + Sync + Send>>;

/// `jt/expand`: the Rust code generated for one file.
pub enum Expand {}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpandParams {
    pub text_document: TextDocumentIdentifier,
}

#[derive(Deserialize, Serialize)]
pub struct ExpandResult {
    /// The formatted Rust code, or the error that stopped the expansion.
    pub code: Option<String>,
    pub error: Option<String>,
}

impl req::Request for Expand {
    type Params = ExpandParams;
    type Result = ExpandResult;
    const METHOD: &'static str = "jt/expand";
}

struct Doc {
    uri: Uri,
    text: String,
    index: LineIndex,
    analysis: Analysis,
    /// Open in the editor. The editor's text wins over the disk.
    open: bool,
}

impl Doc {
    fn new(uri: Uri, text: String, open: bool) -> Self {
        let index = LineIndex::new(&text);
        let analysis = analysis::analyze(&text);
        Doc {
            uri,
            text,
            index,
            analysis,
            open,
        }
    }

    fn range(&self, range: &Range<usize>) -> lsp_types::Range {
        lsp_types::Range::new(
            self.index.position(&self.text, range.start),
            self.index.position(&self.text, range.end),
        )
    }

    /// The identifier under the cursor, or touching it on the left.
    fn word_at(&self, position: Position) -> Option<&str> {
        let at = self.index.offset(&self.text, position);
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let start = self.text[..at]
            .char_indices()
            .rev()
            .take_while(|&(_, c)| is_word(c))
            .last()
            .map_or(at, |(i, _)| i);
        let end = self.text[at..]
            .char_indices()
            .find(|&(_, c)| !is_word(c))
            .map_or(self.text.len(), |(i, _)| at + i);
        let word = &self.text[start..end];
        (!word.is_empty() && !word.starts_with(|c: char| c.is_ascii_digit())).then_some(word)
    }
}

struct Server<'a> {
    connection: &'a Connection,
    /// Keyed by URI text, so the order is stable for tests and listings.
    docs: BTreeMap<String, Doc>,
}

pub fn run(connection: &Connection) -> Result<()> {
    let (id, params) = connection.initialize_start()?;
    let params: InitializeParams = serde_json::from_value(params)?;
    let result = InitializeResult {
        capabilities: capabilities(),
        server_info: Some(ServerInfo {
            name: "jt-lsp".into(),
            version: Some(env!("CARGO_PKG_VERSION").into()),
        }),
    };
    connection.initialize_finish(id, serde_json::to_value(result)?)?;

    let mut server = Server {
        connection,
        docs: BTreeMap::new(),
    };
    for root in roots(&params) {
        server.scan(&root);
    }
    let uris: Vec<String> = server.docs.keys().cloned().collect();
    for uri in uris {
        server.publish(&uri)?;
    }

    for message in &connection.receiver {
        match message {
            Message::Request(request) => {
                if connection.handle_shutdown(&request)? {
                    return Ok(());
                }
                let id = request.id.clone();
                let response = match server.request(request) {
                    Ok(value) => Response::new_ok(id, value),
                    Err(e) => Response::new_err(
                        id,
                        lsp_server::ErrorCode::InternalError as i32,
                        e.to_string(),
                    ),
                };
                connection.sender.send(response.into())?;
            }
            Message::Notification(n) => server.notification(n)?,
            Message::Response(_) => {}
        }
    }
    Ok(())
}

fn capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::FULL),
                ..Default::default()
            },
        )),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        completion_provider: Some(CompletionOptions::default()),
        ..Default::default()
    }
}

#[allow(deprecated)] // `root_uri` is the only root an old client sends.
fn roots(params: &InitializeParams) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = params
        .workspace_folders
        .iter()
        .flatten()
        .filter_map(|f| path_of(&f.uri))
        .collect();
    if roots.is_empty() {
        roots.extend(params.root_uri.as_ref().and_then(path_of));
    }
    roots
}

fn path_of(uri: &Uri) -> Option<PathBuf> {
    url::Url::parse(uri.as_str()).ok()?.to_file_path().ok()
}

fn uri_of(path: &Path) -> Option<Uri> {
    let url = url::Url::from_file_path(path).ok()?;
    Uri::from_str(url.as_str()).ok()
}

/// The URI as one canonical string, so that `file:///a%20b` and a URI
/// built from the path `/a b` find the same document.
fn key_of(uri: &Uri) -> String {
    match url::Url::parse(uri.as_str()) {
        Ok(url) => url.to_string(),
        Err(_) => uri.as_str().to_owned(),
    }
}

fn params<P: serde::de::DeserializeOwned>(value: Json) -> Result<P> {
    Ok(serde_json::from_value(value)?)
}

impl Server<'_> {
    /// Indexes every `.jt` file under `root`, skipping build output and
    /// hidden directories.
    fn scan(&mut self, root: &Path) {
        let walker = walkdir::WalkDir::new(root).into_iter().filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            e.depth() == 0 || !(name.starts_with('.') || name == "target" || name == "node_modules")
        });
        for entry in walker.flatten() {
            let path = entry.path();
            if entry.file_type().is_file() && path.extension().is_some_and(|e| e == "jt") {
                self.load(path);
            }
        }
    }

    /// Reads a file from disk unless the editor has it open.
    fn load(&mut self, path: &Path) -> Option<String> {
        let uri = uri_of(path)?;
        let key = key_of(&uri);
        if self.docs.get(&key).is_some_and(|d| d.open) {
            return Some(key);
        }
        let text = std::fs::read_to_string(path).ok()?;
        self.docs.insert(key.clone(), Doc::new(uri, text, false));
        Some(key)
    }

    fn publish(&self, key: &str) -> Result<()> {
        let Some(doc) = self.docs.get(key) else {
            return Ok(());
        };
        let diagnostics = doc
            .analysis
            .diagnostics
            .iter()
            .map(|d| lsp_types::Diagnostic {
                range: doc.range(&d.range),
                severity: Some(DiagnosticSeverity::ERROR),
                source: Some("jt".into()),
                message: d.message.clone(),
                ..Default::default()
            })
            .collect();
        self.clear_or_send(doc.uri.clone(), diagnostics)
    }

    fn clear_or_send(&self, uri: Uri, diagnostics: Vec<lsp_types::Diagnostic>) -> Result<()> {
        let params = PublishDiagnosticsParams {
            uri,
            diagnostics,
            version: None,
        };
        let n = Notification::new(notif::PublishDiagnostics::METHOD.into(), params);
        self.connection.sender.send(n.into())?;
        Ok(())
    }

    fn notification(&mut self, n: Notification) -> Result<()> {
        match n.method.as_str() {
            notif::DidOpenTextDocument::METHOD => {
                let p: DidOpenTextDocumentParams = params(n.params)?;
                let key = key_of(&p.text_document.uri);
                let doc = Doc::new(p.text_document.uri, p.text_document.text, true);
                self.docs.insert(key.clone(), doc);
                self.publish(&key)?;
            }
            notif::DidChangeTextDocument::METHOD => {
                let p: DidChangeTextDocumentParams = params(n.params)?;
                // Full sync: the last change holds the whole text.
                if let Some(change) = p.content_changes.into_iter().last() {
                    let key = key_of(&p.text_document.uri);
                    let doc = Doc::new(p.text_document.uri, change.text, true);
                    self.docs.insert(key.clone(), doc);
                    self.publish(&key)?;
                }
            }
            notif::DidCloseTextDocument::METHOD => {
                let p: DidCloseTextDocumentParams = params(n.params)?;
                let key = key_of(&p.text_document.uri);
                if let Some(doc) = self.docs.get_mut(&key) {
                    doc.open = false;
                }
                // Back to the disk version, or gone if the file is gone.
                let reloaded = path_of(&p.text_document.uri).and_then(|path| self.load(&path));
                if reloaded.is_some() {
                    self.publish(&key)?;
                } else {
                    self.docs.remove(&key);
                    self.clear_or_send(p.text_document.uri, Vec::new())?;
                }
            }
            notif::DidChangeWatchedFiles::METHOD => {
                let p: DidChangeWatchedFilesParams = params(n.params)?;
                for change in p.changes {
                    let key = key_of(&change.uri);
                    if change.typ == FileChangeType::DELETED {
                        if self.docs.get(&key).is_some_and(|d| !d.open) {
                            self.docs.remove(&key);
                            self.clear_or_send(change.uri, Vec::new())?;
                        }
                    } else if let Some(path) = path_of(&change.uri) {
                        if let Some(key) = self.load(&path) {
                            self.publish(&key)?;
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn request(&mut self, r: Request) -> Result<Json> {
        let value = match r.method.as_str() {
            req::HoverRequest::METHOD => serde_json::to_value(self.hover(params(r.params)?))?,
            req::GotoDefinition::METHOD => {
                serde_json::to_value(self.definition(params(r.params)?))?
            }
            req::References::METHOD => serde_json::to_value(self.references(params(r.params)?))?,
            req::DocumentSymbolRequest::METHOD => {
                serde_json::to_value(self.document_symbols(params(r.params)?))?
            }
            req::WorkspaceSymbolRequest::METHOD => {
                serde_json::to_value(self.workspace_symbols(params(r.params)?))?
            }
            req::Completion::METHOD => serde_json::to_value(self.completion(params(r.params)?))?,
            Expand::METHOD => serde_json::to_value(self.expand(params(r.params)?))?,
            _ => {
                return Err(format!("unsupported request: {}", r.method).into());
            }
        };
        Ok(value)
    }

    fn doc(&self, uri: &Uri) -> Option<&Doc> {
        self.docs.get(&key_of(uri))
    }

    /// Symbols named `name`: the ones in `here` if it has any, otherwise
    /// every match in the workspace.
    fn lookup<'s>(&'s self, here: &'s Doc, name: &str) -> Vec<(&'s Doc, &'s Symbol)> {
        let matches = |doc: &'s Doc| {
            doc.analysis
                .symbols
                .iter()
                .filter(move |s| s.name == name && s.kind != Kind::Impl)
                .map(move |s| (doc, s))
        };
        let local: Vec<_> = matches(here).collect();
        if !local.is_empty() {
            return local;
        }
        self.docs.values().flat_map(matches).collect()
    }

    fn hover(&self, p: HoverParams) -> Option<Hover> {
        let at = p.text_document_position_params;
        let doc = self.doc(&at.text_document.uri)?;
        let word = doc.word_at(at.position)?;
        let found = self.lookup(doc, word);
        let (_, symbol) = found.first()?;
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: hover_text(symbol),
            }),
            range: None,
        })
    }

    fn definition(&self, p: GotoDefinitionParams) -> Option<GotoDefinitionResponse> {
        let at = p.text_document_position_params;
        let doc = self.doc(&at.text_document.uri)?;
        let word = doc.word_at(at.position)?;
        let locations: Vec<Location> = self
            .lookup(doc, word)
            .into_iter()
            .map(|(d, s)| Location::new(d.uri.clone(), d.range(&s.selection)))
            .collect();
        (!locations.is_empty()).then_some(GotoDefinitionResponse::Array(locations))
    }

    fn references(&self, p: ReferenceParams) -> Option<Vec<Location>> {
        let at = p.text_document_position;
        let doc = self.doc(&at.text_document.uri)?;
        let word = doc.word_at(at.position)?;
        let mut out = Vec::new();
        // No module resolution: every use of the name in the workspace counts.
        for other in self.docs.values() {
            for range in analysis::references(&other.text, word) {
                let is_decl = other
                    .analysis
                    .symbols
                    .iter()
                    .any(|s| s.name == word && s.selection == range);
                if is_decl && !p.context.include_declaration {
                    continue;
                }
                out.push(Location::new(other.uri.clone(), other.range(&range)));
            }
        }
        Some(out)
    }

    fn document_symbols(&self, p: DocumentSymbolParams) -> Option<DocumentSymbolResponse> {
        let doc = self.doc(&p.text_document.uri)?;
        #[allow(deprecated)] // `deprecated` is a required field.
        let symbols = doc
            .analysis
            .symbols
            .iter()
            .map(|s| DocumentSymbol {
                name: s.name.clone(),
                detail: Some(s.detail.clone()),
                kind: symbol_kind(s.kind),
                tags: None,
                deprecated: None,
                range: doc.range(&s.range),
                selection_range: doc.range(&s.selection),
                children: None,
            })
            .collect();
        Some(DocumentSymbolResponse::Nested(symbols))
    }

    fn workspace_symbols(&self, p: WorkspaceSymbolParams) -> Option<WorkspaceSymbolResponse> {
        let query = p.query.to_lowercase();
        let mut out = Vec::new();
        for doc in self.docs.values() {
            for s in &doc.analysis.symbols {
                if s.kind == Kind::Impl || !s.name.to_lowercase().contains(&query) {
                    continue;
                }
                #[allow(deprecated)]
                out.push(SymbolInformation {
                    name: s.name.clone(),
                    kind: symbol_kind(s.kind),
                    tags: None,
                    deprecated: None,
                    location: Location::new(doc.uri.clone(), doc.range(&s.selection)),
                    container_name: path_of(&doc.uri)
                        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())),
                });
            }
        }
        Some(WorkspaceSymbolResponse::Flat(out))
    }

    fn completion(&self, p: CompletionParams) -> Option<CompletionResponse> {
        let here = self.doc(&p.text_document_position.text_document.uri)?;
        let mut items = Vec::new();
        let mut seen = HashSet::new();
        // This file first, then templates from the rest of the workspace.
        let docs =
            std::iter::once(here).chain(self.docs.values().filter(|d| !std::ptr::eq(*d, here)));
        for doc in docs {
            let local = std::ptr::eq(doc, here);
            for s in &doc.analysis.symbols {
                if s.kind == Kind::Impl || (!local && s.kind != Kind::Template) {
                    continue;
                }
                if !seen.insert(s.name.clone()) {
                    continue;
                }
                items.push(symbol_completion(s, local));
            }
        }
        items.extend(SNIPPETS.iter().map(|(label, detail, body)| CompletionItem {
            label: (*label).into(),
            kind: Some(CompletionItemKind::SNIPPET),
            detail: Some((*detail).into()),
            insert_text: Some((*body).into()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        }));
        Some(CompletionResponse::Array(items))
    }

    fn expand(&self, p: ExpandParams) -> ExpandResult {
        let Some(doc) = self.doc(&p.text_document.uri) else {
            return ExpandResult {
                code: None,
                error: Some("the document is not open".into()),
            };
        };
        match analysis::expand(&doc.text) {
            Ok(code) => ExpandResult {
                code: Some(code),
                error: None,
            },
            Err(e) => ExpandResult {
                code: None,
                error: Some(e),
            },
        }
    }
}

const SNIPPETS: &[(&str, &str, &str)] = &[
    (
        "template",
        "object template",
        "pub fn ${1:Name}(${2}) -> Json {\n\t$0\n}",
    ),
    (
        "template=",
        "value template",
        "pub fn ${1:Name}(${2}) -> Json = ${0:[]};",
    ),
    (
        "for",
        "repeat entries",
        "for ${1:item} in ${2:items} {\n\t$0\n}",
    ),
    ("if", "conditional entries", "if ${1:condition} {\n\t$0\n}"),
    (
        "iflet",
        "entries for a match",
        "if let ${1:Some(x)} = ${2:option} {\n\t$0\n}",
    ),
    (
        "match",
        "entries per arm",
        "match ${1:value} {\n\t${2:pattern} => { $0 },\n}",
    ),
    ("let", "binding", "let ${1:name} = ${2:expr};"),
    ("opt", "field when Some", "\"${1:key}\"?: ${0:option},"),
];

fn symbol_completion(s: &Symbol, local: bool) -> CompletionItem {
    let (kind, insert) = match s.kind {
        Kind::Template | Kind::Function => (
            if s.kind == Kind::Template {
                CompletionItemKind::FUNCTION
            } else {
                CompletionItemKind::METHOD
            },
            Some(format!("{}($0)", s.name)),
        ),
        Kind::Struct => (CompletionItemKind::STRUCT, None),
        Kind::Enum => (CompletionItemKind::ENUM, None),
        Kind::Const | Kind::Static => (CompletionItemKind::CONSTANT, None),
        Kind::Type => (CompletionItemKind::TYPE_PARAMETER, None),
        Kind::Trait => (CompletionItemKind::INTERFACE, None),
        Kind::Module => (CompletionItemKind::MODULE, None),
        Kind::Macro | Kind::Impl => (CompletionItemKind::KEYWORD, None),
    };
    CompletionItem {
        label: s.name.clone(),
        kind: Some(kind),
        detail: Some(s.detail.clone()),
        documentation: (!s.docs.is_empty()).then(|| {
            Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: s.docs.clone(),
            })
        }),
        insert_text_format: insert.as_ref().map(|_| InsertTextFormat::SNIPPET),
        insert_text: insert,
        // Names from this file sort before names from other files.
        sort_text: Some(format!("{}{}", if local { 0 } else { 1 }, s.name)),
        ..Default::default()
    }
}

fn symbol_kind(kind: Kind) -> SymbolKind {
    match kind {
        Kind::Template | Kind::Function => SymbolKind::FUNCTION,
        Kind::Struct => SymbolKind::STRUCT,
        Kind::Enum => SymbolKind::ENUM,
        Kind::Const | Kind::Static => SymbolKind::CONSTANT,
        Kind::Type => SymbolKind::TYPE_PARAMETER,
        Kind::Trait => SymbolKind::INTERFACE,
        Kind::Module => SymbolKind::MODULE,
        Kind::Impl => SymbolKind::OBJECT,
        Kind::Macro => SymbolKind::FUNCTION,
    }
}

fn hover_text(s: &Symbol) -> String {
    let mut out = format!("```rust\n{}\n```", s.detail);
    if !s.docs.is_empty() {
        out.push_str("\n\n");
        out.push_str(&s.docs);
    }
    match &s.shape {
        Some(Shape::Object(keys)) if !keys.is_empty() => {
            let keys: Vec<String> = keys.iter().map(|k| format!("`{k}`")).collect();
            out.push_str("\n\n---\n\nRenders an object: ");
            out.push_str(&keys.join(", "));
        }
        Some(Shape::Object(_)) => out.push_str("\n\n---\n\nRenders an empty object."),
        Some(Shape::Array) => out.push_str("\n\n---\n\nRenders an array."),
        Some(Shape::Value) => out.push_str("\n\n---\n\nRenders one value."),
        None => {}
    }
    out
}
