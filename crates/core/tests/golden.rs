#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "golden-file harness: a missing or unreadable fixture should fail the test loudly"
)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use parameters_core::layout::complete_layout;
use parameters_core::validate::MAX_STEPS;
use parameters_core::{
    FieldValue, Port, Position, Signature, Step, ValueType, Workflow, from_json, to_json, validate,
    workflow_schema,
};

fn builtins() -> BTreeMap<String, Signature> {
    use ValueType::{Any, Number, Object, Path, Text};
    let node = |inputs: Vec<Port>, outputs: Vec<Port>| Signature { inputs, outputs };
    BTreeMap::from([
        (
            "git.clone".to_owned(),
            node(
                vec![
                    Port::required("repository", Text),
                    Port::optional("ref", Text),
                ],
                vec![Port::optional("path", Path), Port::optional("commit", Text)],
            ),
        ),
        (
            "shell.run".to_owned(),
            node(
                vec![
                    Port::required("command", Text),
                    Port::optional("directory", Path),
                    Port::optional("env", Object),
                ],
                vec![
                    Port::optional("stdout", Text),
                    Port::optional("stderr", Text),
                    Port::optional("exit_code", Number),
                ],
            ),
        ),
        (
            "http.request".to_owned(),
            node(
                vec![
                    Port::optional("method", Text),
                    Port::required("url", Text),
                    Port::optional("headers", Object),
                    Port::optional("body", Any),
                    Port::optional("file", Path),
                ],
                vec![
                    Port::optional("status", Number),
                    Port::optional("headers", Object),
                    Port::optional("body", Any),
                ],
            ),
        ),
        (
            "log.write".to_owned(),
            node(vec![Port::required("message", Text)], vec![]),
        ),
    ])
}

fn fixtures(kind: &str) -> Vec<PathBuf> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(kind);
    let mut paths: Vec<PathBuf> = fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no fixtures in {}", directory.display());
    paths
}

fn updating() -> bool {
    std::env::var_os("UPDATE_GOLDEN").is_some()
}

fn compare(path: &Path, actual: &str) {
    if updating() {
        fs::write(path, actual).unwrap();
        return;
    }
    let expected = fs::read_to_string(path)
        .unwrap_or_else(|_| panic!("missing {}; run with UPDATE_GOLDEN=1", path.display()));
    assert_eq!(
        expected,
        actual,
        "{} is out of date; run with UPDATE_GOLDEN=1 and review the diff",
        path.display()
    );
}

#[test]
fn valid_documents_round_trip_and_have_no_diagnostics() {
    let builtins = builtins();
    for path in fixtures("valid") {
        let source = fs::read_to_string(&path).unwrap();
        let workflow =
            from_json(&source).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let diagnostics: Vec<String> = validate(&workflow, &builtins)
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(
            diagnostics.is_empty(),
            "{}:\n{}",
            path.display(),
            diagnostics.join("\n")
        );
        compare(&path, &to_json(&workflow));
        assert_eq!(from_json(&to_json(&workflow)).unwrap(), workflow);
    }
}

#[test]
fn invalid_documents_report_the_expected_diagnostics() {
    let builtins = builtins();
    for path in fixtures("invalid") {
        let source = fs::read_to_string(&path).unwrap();
        let workflow =
            from_json(&source).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let diagnostics: Vec<String> = validate(&workflow, &builtins)
            .iter()
            .map(ToString::to_string)
            .collect();
        let rendered = if diagnostics.is_empty() {
            "no diagnostics
"
            .to_owned()
        } else {
            diagnostics.join(
                "
",
            ) + "
"
        };
        compare(&path.with_extension("expected"), &rendered);
    }
}

#[test]
fn published_schema_matches_the_document_types() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schema/workflow.v1.json");
    compare(&path, &workflow_schema());
}

#[test]
fn the_largest_allowed_graph_loads_validates_and_lays_out() {
    let mut workflow = Workflow::new("Chain");
    for index in 0..MAX_STEPS {
        let mut step = Step::new(format!("step_{index}"), "shell.run");
        let command = if index == 0 {
            FieldValue::Literal("echo start".into())
        } else {
            FieldValue::expression(format!("\"echo {{step_{}.stdout}}\"", index - 1))
        };
        step.with.insert("command", command);
        workflow.steps.push(step);
    }
    let workflow = from_json(&to_json(&workflow)).unwrap();
    assert_eq!(validate(&workflow, &builtins()), []);
    let mut layout = workflow.layout.clone();
    complete_layout(&workflow.steps, &mut layout);
    assert_eq!(layout.len(), MAX_STEPS);
    let last_column = i32::try_from(MAX_STEPS - 1).unwrap() * 280;
    assert_eq!(
        layout.get(&format!("step_{}", MAX_STEPS - 1)),
        Some(&Position::new(last_column, 0))
    );
}
