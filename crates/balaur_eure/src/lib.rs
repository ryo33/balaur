//! The Eure language as a Balaur plugin: `eure.*` for scripts.
//!
//! `eure` answers everything an editor asks — tokens, problems, hover,
//! completion, definitions — as queries over a runtime that suspends when it
//! needs a file it has not seen. This crate runs that runtime on the frame
//! thread and serves each suspension from the disk on the spot, so a
//! script's `eure.hover(...)` returns in the same call. That is what lets
//! the editor, itself a script, be an Eure editor without a second process
//! or a protocol.

mod api;
mod text;

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use balaur_core::{App, DetHashMap, DetHashSet, Plugin};
use eure::query::{
    build_runtime, get_completions, get_definition, get_hover, DiagnosticSeverity,
    GetFileDiagnostics, GetSemanticTokens, Glob, GlobResult, OpenDocuments, OpenDocumentsList,
    SemanticTokenModifier, SemanticTokenType, TextFile, TextFileContent, Workspace, WorkspaceId,
};
use eure::query_flow::{DurabilityLevel, QueryError, QueryRuntime};

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

/// One problem the language reports, in the editor's `lint` shape.
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

/// The names scripts see for the language's token kinds.
const fn token_kind(kind: SemanticTokenType) -> &'static str {
    match kind {
        SemanticTokenType::Keyword => "keyword",
        SemanticTokenType::Number => "number",
        SemanticTokenType::String => "string",
        SemanticTokenType::Comment => "comment",
        SemanticTokenType::Operator => "operator",
        SemanticTokenType::Property => "property",
        SemanticTokenType::Punctuation => "punctuation",
        SemanticTokenType::Macro => "macro",
        SemanticTokenType::Decorator => "decorator",
        SemanticTokenType::SectionMarker => "section_marker",
        SemanticTokenType::ExtensionMarker => "extension_marker",
        SemanticTokenType::ExtensionIdent => "extension_ident",
    }
}

const fn token_modifier(modifier: SemanticTokenModifier) -> &'static str {
    match modifier {
        SemanticTokenModifier::Declaration => "declaration",
        SemanticTokenModifier::Definition => "definition",
        SemanticTokenModifier::SectionHeader => "header",
    }
}

/// The language's runtime and what it has been given: the open documents,
/// and the files served in place of the disk.
pub struct EureState {
    runtime: QueryRuntime,
    documents: DetHashMap<PathBuf, String>,
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
        let state = Self {
            runtime: build_runtime(),
            documents: DetHashMap::default(),
            virtual_files: DetHashMap::default(),
            workspaces: DetHashSet::default(),
        };
        state.publish_open_documents();
        state
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
                self.runtime.resolve_asset(
                    TextFile::from_path(config_path.clone()),
                    TextFileContent(text.to_string()),
                    DurabilityLevel::Static,
                );
                self.virtual_files
                    .insert(config_path.clone(), text.to_string());
            }
        }
        self.runtime.resolve_asset(
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
        let abs = text::absolute(path);
        self.runtime.resolve_asset(
            TextFile::from_path(abs.clone()),
            TextFileContent(text.to_string()),
            DurabilityLevel::Volatile,
        );
        self.documents.insert(abs, text.to_string());
        self.publish_open_documents();
    }

    /// Forget an open document; the disk's copy stands for it from here on.
    pub fn close(&mut self, path: &str) {
        let abs = text::absolute(path);
        if self.documents.shift_remove(&abs).is_some() {
            self.runtime.invalidate_asset(&TextFile::from_path(abs));
            self.publish_open_documents();
        }
    }

    /// The text last given to `open` for `path`, if it is open.
    pub fn document(&self, path: &str) -> Option<&str> {
        self.documents
            .get(&text::absolute(path))
            .map(String::as_str)
    }

    pub fn tokens(&self, path: &str) -> Result<Vec<Token>> {
        let (file, source) = self.open_document(path)?;
        let tokens = self.settle(|runtime| runtime.query(GetSemanticTokens::new(file.clone())))?;
        Ok(tokens
            .iter()
            .map(|t| {
                let start = t.start as usize;
                let end = start + t.length as usize;
                Token {
                    at: text::span_at(&source, start),
                    length: text::char_length(&source, start, end),
                    kind: token_kind(t.token_type),
                    modifiers: SemanticTokenModifier::all()
                        .iter()
                        .filter(|m| t.modifiers & m.bitmask() != 0)
                        .map(|m| token_modifier(*m))
                        .collect(),
                }
            })
            .collect())
    }

    /// The problems in `path`; empty when nothing is wrong, the file was
    /// never opened, or the language could not answer (which is logged).
    pub fn diagnostics(&self, path: &str) -> Vec<Finding> {
        let Ok((file, source)) = self.open_document(path) else {
            return Vec::new();
        };
        let found = self.settle(|runtime| runtime.query(GetFileDiagnostics::new(file.clone())));
        let found = match found {
            Ok(found) => found,
            Err(e) => {
                tracing::warn!("eure: {path}: {e:#}");
                return Vec::new();
            }
        };
        found
            .iter()
            .filter(|d| d.file == file)
            .map(|d| Finding {
                at: text::span_at(&source, d.start),
                severity: match d.severity {
                    DiagnosticSeverity::Error => "error",
                    DiagnosticSeverity::Warning => "warning",
                    DiagnosticSeverity::Info => "info",
                    DiagnosticSeverity::Hint => "hint",
                },
                message: d.message.clone(),
            })
            .collect()
    }

    pub fn hover(&self, path: &str, at: Span) -> Result<Option<Hover>> {
        let (file, source) = self.open_document(path)?;
        let offset = text::offset_of(&source, at) as u32;
        let hover = self.settle(|runtime| get_hover(runtime, &file, offset))?;
        Ok(hover.map(|h| Hover {
            text: h.contents,
            from: text::span_at(&source, h.span.start as usize),
            to: text::span_at(&source, h.span.end as usize),
        }))
    }

    pub fn completion(&self, path: &str, at: Span) -> Result<Vec<Completion>> {
        let (file, source) = self.open_document(path)?;
        let offset = text::offset_of(&source, at) as u32;
        let items = self.settle(|runtime| get_completions(runtime, &file, offset))?;
        Ok(items
            .into_iter()
            .map(|item| Completion {
                text: item.label.clone(),
                label: item.label,
                kind: item.kind.as_str(),
                detail: item.detail,
                documentation: item.documentation,
                from: text::span_at(&source, item.replace.start as usize),
                to: text::span_at(&source, item.replace.end as usize),
            })
            .collect())
    }

    pub fn definition(&self, path: &str, at: Span) -> Result<Vec<Location>> {
        let (file, source) = self.open_document(path)?;
        let offset = text::offset_of(&source, at) as u32;
        let found = self.settle(|runtime| get_definition(runtime, &file, offset))?;
        Ok(found
            .into_iter()
            .map(|d| {
                let file = d
                    .file
                    .as_local_path()
                    .map_or_else(|| PathBuf::from(d.file.to_string()), Path::to_path_buf);
                let target = self
                    .documents
                    .get(&file)
                    .cloned()
                    .or_else(|| std::fs::read_to_string(&file).ok())
                    .unwrap_or_default();
                Location {
                    from: text::span_at(&target, d.selection.start as usize),
                    to: text::span_at(&target, d.selection.end as usize),
                    file,
                }
            })
            .collect())
    }

    /// The key and text of an open document, or why a query cannot run.
    fn open_document(&self, path: &str) -> Result<(TextFile, String)> {
        let abs = text::absolute(path);
        let source = self
            .documents
            .get(&abs)
            .cloned()
            .ok_or_else(|| anyhow!("{path} is not open in the Eure language"))?;
        Ok((TextFile::from_path(abs), source))
    }

    /// Run `attempt` until it stops suspending, serving each file or glob it
    /// waits for from the disk in between.
    fn settle<T>(&self, attempt: impl Fn(&QueryRuntime) -> Result<T, QueryError>) -> Result<T> {
        for _ in 0..10_000 {
            match attempt(&self.runtime) {
                Ok(value) => return Ok(value),
                Err(QueryError::Suspend { .. }) => {
                    if !self.serve_pending() {
                        bail!("the Eure language waits for something the disk cannot give");
                    }
                }
                Err(e) => return Err(anyhow!("{e}")),
            }
        }
        bail!("the Eure language keeps asking for files")
    }

    /// Give the runtime every file and glob it is waiting on; false when it
    /// waits on nothing this crate knows how to serve.
    fn serve_pending(&self) -> bool {
        let mut served = false;
        for pending in self.runtime.pending_assets() {
            if let Some(file) = pending.key::<TextFile>() {
                match self.fetch(file) {
                    Ok(content) => self.runtime.resolve_asset(
                        file.clone(),
                        TextFileContent(content),
                        DurabilityLevel::Volatile,
                    ),
                    Err(why) => self.runtime.resolve_asset_error(
                        file.clone(),
                        anyhow!(why),
                        DurabilityLevel::Volatile,
                    ),
                }
                served = true;
            } else if let Some(glob) = pending.key::<Glob>() {
                let pattern = glob.full_pattern().to_string_lossy().into_owned();
                let files = glob::glob(&pattern)
                    .into_iter()
                    .flat_map(|paths| paths.flatten().map(TextFile::from_path))
                    .collect();
                self.runtime.resolve_asset(
                    glob.clone(),
                    GlobResult(files),
                    DurabilityLevel::Volatile,
                );
                served = true;
            }
        }
        served
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

    /// Which documents are open decides which schemas get diagnosed.
    fn publish_open_documents(&self) {
        let files = self
            .documents
            .keys()
            .map(|path| TextFile::from_path(path.clone()))
            .collect();
        self.runtime.resolve_asset(
            OpenDocuments,
            OpenDocumentsList(files),
            DurabilityLevel::Volatile,
        );
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
/// need to compare the language's absolute answers with their own.
pub fn absolute(path: &str) -> PathBuf {
    text::absolute(path)
}

/// Whether `path` is under `root`, both spelled as `absolute` spells them.
pub fn within(path: &Path, root: &str) -> bool {
    path.starts_with(text::absolute(root))
}
