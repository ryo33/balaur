//! `eure.*` bindings: the language's answers as script values.
//!
//! Every position is `{ line, column }`, both from one, the column in
//! characters — the spelling the editor's gutter and `lint` already use.

use balaur_core::Engine;
use balaur_script::{Bindings, BindingsExt, Value};

use crate::{Completion, EureState, Finding, Hover, Location, Span, Token};

fn map(entries: Vec<(&str, Value)>) -> Value {
    Value::Map(
        entries
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    )
}

fn int(n: usize) -> Value {
    Value::Int(i64::try_from(n).unwrap_or(i64::MAX))
}

fn span(at: Span) -> Value {
    map(vec![("line", int(at.line)), ("column", int(at.column))])
}

fn strings(items: Vec<&'static str>) -> Value {
    Value::List(
        items
            .into_iter()
            .map(|s| Value::Str(s.to_string()))
            .collect(),
    )
}

fn token(t: Token) -> Value {
    map(vec![
        ("line", int(t.at.line)),
        ("column", int(t.at.column)),
        ("length", int(t.length)),
        ("kind", Value::Str(t.kind.to_string())),
        ("modifiers", strings(t.modifiers)),
    ])
}

fn finding(f: Finding) -> Value {
    map(vec![
        ("line", int(f.at.line)),
        ("column", int(f.at.column)),
        ("severity", Value::Str(f.severity.to_string())),
        ("message", Value::Str(f.message)),
    ])
}

fn hover(h: Hover) -> Value {
    map(vec![
        ("text", Value::Str(h.text)),
        ("from", span(h.from)),
        ("to", span(h.to)),
    ])
}

fn optional(text: Option<String>) -> Value {
    text.map_or(Value::Nil, Value::Str)
}

fn completion(c: Completion) -> Value {
    map(vec![
        ("label", Value::Str(c.label)),
        ("kind", Value::Str(c.kind.to_string())),
        ("detail", optional(c.detail)),
        ("documentation", optional(c.documentation)),
        ("text", Value::Str(c.text)),
        ("from", span(c.from)),
        ("to", span(c.to)),
    ])
}

fn location(l: &Location) -> Value {
    map(vec![
        ("file", Value::Str(l.file.to_string_lossy().into_owned())),
        ("from", span(l.from)),
        ("to", span(l.to)),
    ])
}

fn at(line: i64, column: i64) -> Span {
    Span {
        line: usize::try_from(line).unwrap_or(1).max(1),
        column: usize::try_from(column).unwrap_or(1).max(1),
    }
}

/// `eure.*`: documents in, tokens, problems and answers about a position out.
pub(crate) fn install_eure_api(m: &mut dyn Bindings<Engine>) {
    m.module_doc(
        "The Eure language, running inside the engine. A script opens a \
         file's text, then asks about it: highlighting, problems, what a \
         position means, what could be typed there, and where it is defined. \
         Paths are project-relative or absolute; positions are `{ line, \
         column }` from one, in characters.",
    );
    m.describe(&[
        ("workspace", &[], "", "Make `root` a workspace whose `Eure.eure` binds files to schemas; `config` is used as that file when the project has none."),
        ("open", &[], "", "Open `path` with `text`, or replace its text when already open. Queries and problems are about the text last given here."),
        ("close", &[], "", "Forget an open document and its problems."),
        ("tokens", &[], "", "Highlighting for an open document: `{ line, column, length, kind, modifiers }` per run, `kind` one of the Eure token names."),
        ("diagnostics", &[], "", "The problems in an open document: `{ line, column, severity, message }`, `severity` `error`, `warning`, `info` or `hint`."),
        ("hover", &[], "", "What the position means, as `{ text, from, to }` markdown over the range it applies to, or nil."),
        ("completion", &[], "", "What could be typed at the position: `{ label, kind, detail, documentation, text, from, to }`, `text` replacing `from..to`."),
        ("definition", &[], "", "Where the thing at the position is defined: `{ file, from, to }` per target, `file` absolute."),
    ]);
    m.function(
        "workspace",
        |eng: &Engine, (root, config): (String, Option<String>)| {
            let state = eng.resource::<EureState>();
            state.borrow_mut().workspace(&root, config.as_deref());
            Ok(())
        },
    );
    m.function("open", |eng: &Engine, (path, text): (String, String)| {
        let state = eng.resource::<EureState>();
        state.borrow_mut().open(&path, &text);
        Ok(())
    });
    m.function("close", |eng: &Engine, path: String| {
        let state = eng.resource::<EureState>();
        state.borrow_mut().close(&path);
        Ok(())
    });
    m.function("tokens", |eng: &Engine, path: String| {
        let state = eng.resource::<EureState>();
        let tokens = state.borrow().tokens(&path)?;
        Ok(Value::List(tokens.into_iter().map(token).collect()))
    });
    m.function("diagnostics", |eng: &Engine, path: String| {
        let state = eng.resource::<EureState>();
        let found = state.borrow().diagnostics(&path);
        Ok(Value::List(found.into_iter().map(finding).collect()))
    });
    m.function(
        "hover",
        |eng: &Engine, (path, line, column): (String, i64, i64)| {
            let state = eng.resource::<EureState>();
            let answer = state.borrow().hover(&path, at(line, column))?;
            Ok(answer.map_or(Value::Nil, hover))
        },
    );
    m.function(
        "completion",
        |eng: &Engine, (path, line, column): (String, i64, i64)| {
            let state = eng.resource::<EureState>();
            let items = state.borrow().completion(&path, at(line, column))?;
            Ok(Value::List(items.into_iter().map(completion).collect()))
        },
    );
    m.function(
        "definition",
        |eng: &Engine, (path, line, column): (String, i64, i64)| {
            let state = eng.resource::<EureState>();
            let targets = state.borrow().definition(&path, at(line, column))?;
            Ok(Value::List(targets.iter().map(location).collect()))
        },
    );
}
