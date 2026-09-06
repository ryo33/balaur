//! The language server driven the way the editor drives it: a project on
//! disk, a schema bound through a workspace config the project does not
//! have, and every answer arriving in the call that asked.

use balaur_eure::{EureState, Span};

const SCHEMA: &str = "title = `text`\ncount = `integer`\ncount.$optional = true\n";
const CONFIG: &str = "@ targets.docs\nglobs = [\"*.eure\"]\nschema = \"schemas/doc.schema.eure\"\n";

struct Project {
    dir: tempfile::TempDir,
    state: EureState,
}

impl Project {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("schemas")).unwrap();
        std::fs::write(dir.path().join("schemas/doc.schema.eure"), SCHEMA).unwrap();
        let mut state = EureState::new();
        state.workspace(dir.path().to_str().unwrap(), Some(CONFIG));
        Self { dir, state }
    }

    fn open(&mut self, name: &str, text: &str) -> String {
        let path = self.dir.path().join(name).to_string_lossy().into_owned();
        std::fs::write(&path, text).unwrap();
        self.state.open(&path, text);
        path
    }
}

#[test]
fn an_open_document_has_tokens_for_its_keys_and_values() {
    let mut p = Project::new();
    let path = p.open("a.eure", "title: hello\ncount = 3\n");
    let tokens = p.state.tokens(&path).unwrap();
    let kinds: Vec<&str> = tokens.iter().map(|t| t.kind).collect();
    assert!(kinds.contains(&"property"), "{kinds:?}");
    assert!(kinds.contains(&"number"), "{kinds:?}");
    let first = &tokens[0];
    assert_eq!((first.at.line, first.at.column), (1, 1));
    assert_eq!(first.length, "title".len());
}

#[test]
fn a_value_of_the_wrong_type_is_a_diagnostic_and_fixing_it_clears_it() {
    let mut p = Project::new();
    let path = p.open("b.eure", "title: hello\ncount = \"three\"\n");
    let found = p.state.diagnostics(&path);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].severity, "error");
    assert_eq!(found[0].at.line, 2);
    p.state.open(&path, "title: hello\ncount = 3\n");
    assert!(p.state.diagnostics(&path).is_empty());
}

#[test]
fn a_file_the_project_never_opened_has_no_diagnostics() {
    let p = Project::new();
    assert!(p.state.diagnostics("/nowhere/none.eure").is_empty());
    assert!(p.state.document("/nowhere/none.eure").is_none());
}

#[test]
fn hovering_a_key_describes_it_from_the_schema() {
    let mut p = Project::new();
    let path = p.open("c.eure", "title: hello\n");
    let hover = p
        .state
        .hover(&path, Span { line: 1, column: 2 })
        .unwrap()
        .expect("hover on a key");
    assert!(hover.text.contains("text"), "{}", hover.text);
    assert_eq!(hover.from, Span { line: 1, column: 1 });
}

#[test]
fn completion_on_an_empty_line_offers_the_schema_fields() {
    let mut p = Project::new();
    let path = p.open("d.eure", "title: hello\n\n");
    let items = p
        .state
        .completion(&path, Span { line: 2, column: 1 })
        .unwrap();
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(labels.contains(&"count"), "{labels:?}");
    let count = items.iter().find(|i| i.label == "count").unwrap();
    assert_eq!(count.kind, "field");
    assert_eq!(count.from, Span { line: 2, column: 1 });
}

#[test]
fn a_key_is_defined_in_the_schema_that_declares_it() {
    let mut p = Project::new();
    let path = p.open("e.eure", "count = 3\n");
    let targets = p
        .state
        .definition(&path, Span { line: 1, column: 1 })
        .unwrap();
    assert_eq!(targets.len(), 1, "{targets:?}");
    assert!(targets[0].file.ends_with("schemas/doc.schema.eure"));
    assert_eq!(targets[0].from.line, 2);
}

#[test]
fn a_query_on_a_closed_document_is_an_error_rather_than_an_empty_answer() {
    let mut p = Project::new();
    let path = p.open("f.eure", "title: hello\n");
    p.state.close(&path);
    assert!(p.state.tokens(&path).is_err());
    assert!(p.state.diagnostics(&path).is_empty());
}
