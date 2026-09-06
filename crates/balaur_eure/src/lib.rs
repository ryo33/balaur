//! The Eure language server as a Balaur plugin: `eure.*` for scripts.
//!
//! `eure-ls` is a headless state machine: requests go in, responses and
//! effects (read this file, expand this glob) come out. This crate runs it
//! on the frame thread and answers every effect from the disk on the spot,
//! so a script's `eure.hover(...)` returns in the same call. That is what
//! lets the editor, itself a script, be an Eure editor without a second
//! process or a protocol.

mod api;
mod text;

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use balaur_core::{App, DetHashMap, DetHashSet, Plugin};
use eure::query::{TextFile, TextFileContent, Workspace, WorkspaceId};
use eure::query_flow::DurabilityLevel;
use eure_ls::{CoreRequestId, Effect, LspCore, LspOutput};
use lsp_types::{
    CompletionItemKind, CompletionResponse, CompletionTextEdit, DiagnosticSeverity,
    GotoDefinitionResponse, HoverContents, MarkedString, PublishDiagnosticsParams,
};
use serde_json::{json, Value as Json};

pub use text::Span;

/// One highlighted run of an open document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub at: Span,
    /// In characters, on `at.line`.
    pub length: usize,
    pub kind: &'static str,
    pub modifiers: Vec<&'static str>,
}

/// One problem the server reports, in the editor's `lint` shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub at: Span,
    pub severity: &'static str,
    pub message: String,
}

/// What hovering a position says, and the range it says it about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hover {
    pub text: String,
    pub from: Span,
    pub to: Span,
}

/// One completion: `text` replaces `from..to` when it is accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    pub label: String,
    pub kind: &'static str,
    pub detail: Option<String>,
    pub documentation: Option<String>,
    pub text: String,
    pub from: Span,
    pub to: Span,
}

/// Where a definition is: an absolute path and the range to select there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub file: PathBuf,
    pub from: Span,
    pub to: Span,
}

/// The legend `eure-ls` advertises, by index; the names scripts see.
const TOKEN_KINDS: [&str; 12] = [
    "keyword",
    "number",
    "string",
    "comment",
    "operator",
    "property",
    "punctuation",
    "macro",
    "decorator",
    "section_marker",
    "extension_marker",
    "extension_ident",
];

const TOKEN_MODIFIERS: [&str; 3] = ["declaration", "definition", "header"];

/// The running server and what it has told us: the diagnostics it last
/// published per file, and the files we serve in place of the disk.
pub struct EureState {
    core: LspCore,
    next_request: i32,
    responses: DetHashMap<i32, Result<Json, String>>,
    diagnostics: DetHashMap<PathBuf, Vec<lsp_types::Diagnostic>>,
    virtual_files: DetHashMap<PathBuf, String>,
    workspaces: DetHashSet<PathBuf>,
}

impl Default for EureState {
    fn default() -> Self {
        Self::new()
    }
}

impl EureState {
    pub fn new() -> Self {
        let mut core = LspCore::new();
        core.set_initialized();
        Self {
            core,
            next_request: 1,
            responses: DetHashMap::default(),
            diagnostics: DetHashMap::default(),
            virtual_files: DetHashMap::default(),
            workspaces: DetHashSet::default(),
        }
    }

    /// Make `root` a workspace: its `Eure.eure` maps files to schemas.
    ///
    /// `config` stands in for that file when the project has none, so an
    /// editor can bind its own schemas to a project it does not own.
    pub fn workspace(&mut self, root: &str, config: Option<&str>) {
        let root = text::absolute(root);
        if !self.workspaces.insert(root.clone()) {
            return;
        }
        let config_path = root.join("Eure.eure");
        if let Some(text) = config {
            if !config_path.exists() {
                self.core.runtime_mut().resolve_asset(
                    TextFile::from_path(config_path.clone()),
                    TextFileContent(text.to_string()),
                    DurabilityLevel::Static,
                );
                self.virtual_files
                    .insert(config_path.clone(), text.to_string());
            }
        }
        self.core.runtime_mut().resolve_asset(
            WorkspaceId(root.to_string_lossy().into_owned()),
            Workspace {
                path: root,
                config_path,
            },
            DurabilityLevel::Static,
        );
    }

    /// Open `path` with `text`, or replace its text when it is already open.
    pub fn open(&mut self, path: &str, text: &str) {
        let uri = text::path_to_uri(&text::absolute(path));
        let (method, params) = if self.core.get_document(&uri).is_some() {
            (
                "textDocument/didChange",
                json!({
                    "textDocument": { "uri": uri, "version": self.next_request },
                    "contentChanges": [{ "text": text }],
                }),
            )
        } else {
            (
                "textDocument/didOpen",
                json!({
                    "textDocument": {
                        "uri": uri, "languageId": "eure", "version": 1, "text": text,
                    },
                }),
            )
        };
        self.next_request += 1;
        let (outputs, effects) = self.core.handle_notification(method, params);
        self.absorb(outputs, effects);
    }

    pub fn close(&mut self, path: &str) {
        let abs = text::absolute(path);
        let uri = text::path_to_uri(&abs);
        let (outputs, effects) = self.core.handle_notification(
            "textDocument/didClose",
            json!({ "textDocument": { "uri": uri } }),
        );
        self.absorb(outputs, effects);
        self.diagnostics.shift_remove(&abs);
    }

    /// The text last given to `open` for `path`, if it is open.
    pub fn document(&self, path: &str) -> Option<&str> {
        let uri = text::path_to_uri(&text::absolute(path));
        self.core.get_document(&uri).map(String::as_str)
    }

    pub fn tokens(&mut self, path: &str) -> Result<Vec<Token>> {
        let (uri, source) = self.open_document(path)?;
        let answer = self.request(
            "textDocument/semanticTokens/full",
            json!({ "textDocument": { "uri": uri } }),
        )?;
        let tokens: Option<lsp_types::SemanticTokens> = serde_json::from_value(answer)?;
        let mut out = Vec::new();
        let (mut line, mut start) = (0u32, 0u32);
        for t in tokens.map(|t| t.data).unwrap_or_default() {
            line += t.delta_line;
            start = if t.delta_line == 0 {
                start + t.delta_start
            } else {
                t.delta_start
            };
            let at = text::from_lsp(&source, lsp_types::Position::new(line, start));
            let modifiers = TOKEN_MODIFIERS
                .iter()
                .enumerate()
                .filter(|(bit, _)| t.token_modifiers_bitset & (1 << bit) != 0)
                .map(|(_, name)| *name)
                .collect();
            out.push(Token {
                at,
                length: text::char_length(&source, line, start, t.length),
                kind: TOKEN_KINDS
                    .get(t.token_type as usize)
                    .copied()
                    .unwrap_or("other"),
                modifiers,
            });
        }
        Ok(out)
    }

    /// What the server last published for `path`; empty when nothing is wrong
    /// or the file was never opened.
    pub fn diagnostics(&self, path: &str) -> Vec<Finding> {
        let abs = text::absolute(path);
        let source = self.document(path).unwrap_or("");
        self.diagnostics
            .get(&abs)
            .map(|found| {
                found
                    .iter()
                    .map(|d| Finding {
                        at: text::from_lsp(source, d.range.start),
                        severity: match d.severity {
                            Some(DiagnosticSeverity::WARNING) => "warning",
                            Some(DiagnosticSeverity::INFORMATION) => "info",
                            Some(DiagnosticSeverity::HINT) => "hint",
                            _ => "error",
                        },
                        message: d.message.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn hover(&mut self, path: &str, at: Span) -> Result<Option<Hover>> {
        let (uri, source) = self.open_document(path)?;
        let answer = self.request(
            "textDocument/hover",
            position_params(&uri, text::to_lsp(&source, at)),
        )?;
        let hover: Option<lsp_types::Hover> = serde_json::from_value(answer)?;
        Ok(hover.map(|h| {
            let range = h.range.unwrap_or_default();
            Hover {
                text: hover_text(h.contents),
                from: text::from_lsp(&source, range.start),
                to: text::from_lsp(&source, range.end),
            }
        }))
    }

    pub fn completion(&mut self, path: &str, at: Span) -> Result<Vec<Completion>> {
        let (uri, source) = self.open_document(path)?;
        let answer = self.request(
            "textDocument/completion",
            position_params(&uri, text::to_lsp(&source, at)),
        )?;
        let response: Option<CompletionResponse> = serde_json::from_value(answer)?;
        let items = match response {
            Some(CompletionResponse::Array(items)) => items,
            Some(CompletionResponse::List(list)) => list.items,
            None => Vec::new(),
        };
        let cursor = text::to_lsp(&source, at);
        Ok(items
            .into_iter()
            .map(|item| {
                let (range, new_text) = match item.text_edit {
                    Some(CompletionTextEdit::Edit(edit)) => (edit.range, edit.new_text),
                    Some(CompletionTextEdit::InsertAndReplace(edit)) => {
                        (edit.replace, edit.new_text)
                    }
                    None => (
                        lsp_types::Range::new(cursor, cursor),
                        item.insert_text
                            .clone()
                            .unwrap_or_else(|| item.label.clone()),
                    ),
                };
                Completion {
                    kind: completion_kind(item.kind),
                    detail: item.detail,
                    documentation: item.documentation.map(|d| match d {
                        lsp_types::Documentation::String(s) => s,
                        lsp_types::Documentation::MarkupContent(m) => m.value,
                    }),
                    label: item.label,
                    text: new_text,
                    from: text::from_lsp(&source, range.start),
                    to: text::from_lsp(&source, range.end),
                }
            })
            .collect())
    }

    pub fn definition(&mut self, path: &str, at: Span) -> Result<Vec<Location>> {
        let (uri, source) = self.open_document(path)?;
        let answer = self.request(
            "textDocument/definition",
            position_params(&uri, text::to_lsp(&source, at)),
        )?;
        let response: Option<GotoDefinitionResponse> = serde_json::from_value(answer)?;
        let locations: Vec<(lsp_types::Uri, lsp_types::Range)> = match response {
            Some(GotoDefinitionResponse::Scalar(l)) => vec![(l.uri, l.range)],
            Some(GotoDefinitionResponse::Array(ls)) => {
                ls.into_iter().map(|l| (l.uri, l.range)).collect()
            }
            Some(GotoDefinitionResponse::Link(ls)) => ls
                .into_iter()
                .map(|l| (l.target_uri, l.target_selection_range))
                .collect(),
            None => Vec::new(),
        };
        Ok(locations
            .into_iter()
            .map(|(target, range)| {
                let file = text::uri_to_path(target.as_str());
                let target_text = self
                    .core
                    .get_document(&text::path_to_uri(&file))
                    .cloned()
                    .or_else(|| std::fs::read_to_string(&file).ok())
                    .unwrap_or_default();
                Location {
                    from: text::from_lsp(&target_text, range.start),
                    to: text::from_lsp(&target_text, range.end),
                    file,
                }
            })
            .collect())
    }

    /// The URI and text of an open document, or why a query cannot run.
    fn open_document(&self, path: &str) -> Result<(String, String)> {
        let uri = text::path_to_uri(&text::absolute(path));
        let source = self
            .core
            .get_document(&uri)
            .cloned()
            .ok_or_else(|| anyhow!("{path} is not open in the Eure language server"))?;
        Ok((uri, source))
    }

    /// One request, answered before this returns: every effect the server
    /// asks for is served from the disk in the same call.
    fn request(&mut self, method: &str, params: Json) -> Result<Json> {
        let id = self.next_request;
        self.next_request += 1;
        let (outputs, effects) = self
            .core
            .handle_request(CoreRequestId::Int(id), method, params);
        self.absorb(outputs, effects);
        match self.responses.shift_remove(&id) {
            Some(Ok(value)) => Ok(value),
            Some(Err(message)) => Err(anyhow!("{method}: {message}")),
            None => Err(anyhow!("{method}: the Eure language server gave no answer")),
        }
    }

    /// Record every output and serve every effect until the server is quiet.
    fn absorb(&mut self, outputs: Vec<LspOutput>, effects: Vec<Effect>) {
        for output in outputs {
            self.record(output);
        }
        let mut queue = effects;
        let mut budget = 10_000;
        while let Some(effect) = queue.pop() {
            budget -= 1;
            if budget == 0 {
                tracing::warn!("eure: the language server keeps asking for files; giving up");
                return;
            }
            let (outputs, more) = match effect {
                Effect::FetchFile(file) => {
                    let content = self.fetch(&file);
                    self.core.resolve_file(file, content)
                }
                Effect::ExpandGlob { id, glob } => {
                    let pattern = glob.full_pattern().to_string_lossy().into_owned();
                    let files = glob::glob(&pattern)
                        .into_iter()
                        .flat_map(|paths| paths.flatten().map(TextFile::from_path))
                        .collect();
                    self.core.resolve_glob(&id, files)
                }
            };
            for output in outputs {
                self.record(output);
            }
            queue.extend(more);
        }
    }

    fn fetch(&self, file: &TextFile) -> Result<String, String> {
        match file.as_local_path() {
            Some(path) => match self.virtual_files.get(path) {
                Some(text) => Ok(text.clone()),
                None => std::fs::read_to_string(path).map_err(|e| e.to_string()),
            },
            None => Err(format!("{file}: remote files are not fetched")),
        }
    }

    fn record(&mut self, output: LspOutput) {
        match output {
            LspOutput::Response { id, result } => {
                if let CoreRequestId::Int(id) = id {
                    self.responses.insert(id, result.map_err(|e| e.message));
                }
            }
            LspOutput::Notification { method, params } => {
                if method != "textDocument/publishDiagnostics" {
                    return;
                }
                if let Ok(published) = serde_json::from_value::<PublishDiagnosticsParams>(params) {
                    let path = text::uri_to_path(published.uri.as_str());
                    self.diagnostics.insert(path, published.diagnostics);
                }
            }
        }
    }
}

fn position_params(uri: &str, at: lsp_types::Position) -> Json {
    json!({
        "textDocument": { "uri": uri },
        "position": { "line": at.line, "character": at.character },
    })
}

fn hover_text(contents: HoverContents) -> String {
    let marked = |m: MarkedString| match m {
        MarkedString::String(s) => s,
        MarkedString::LanguageString(l) => l.value,
    };
    match contents {
        HoverContents::Scalar(m) => marked(m),
        HoverContents::Array(ms) => ms.into_iter().map(marked).collect::<Vec<_>>().join("\n\n"),
        HoverContents::Markup(m) => m.value,
    }
}

fn completion_kind(kind: Option<CompletionItemKind>) -> &'static str {
    match kind {
        Some(CompletionItemKind::FIELD) => "field",
        Some(CompletionItemKind::PROPERTY) => "extension",
        Some(CompletionItemKind::ENUM_MEMBER) => "variant",
        Some(CompletionItemKind::VALUE) => "value",
        _ => "other",
    }
}

/// The `eure` module for scripts, over one [`EureState`].
pub struct EurePlugin;

impl Plugin for EurePlugin {
    fn name(&self) -> &'static str {
        "eure"
    }

    fn build(&mut self, app: &mut App) -> Result<()> {
        app.engine.insert_resource(EureState::new());
        let mut m = app.script_module("eure")?;
        api::install_eure_api(&mut *m);
        Ok(())
    }
}

/// Where a project-relative or absolute `path` lives, for callers that
/// need to compare the server's absolute answers with their own.
pub fn absolute(path: &str) -> PathBuf {
    text::absolute(path)
}

/// Whether `path` is under `root`, both spelled as `absolute` spells them.
pub fn within(path: &Path, root: &str) -> bool {
    path.starts_with(text::absolute(root))
}
