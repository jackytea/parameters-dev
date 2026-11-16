#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "golden-file harness: a missing or unreadable fixture should fail the test loudly"
)]

use std::collections::BTreeMap;
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};

use parameters_core::{Port, Signature, ValueType, from_json, to_json};
use parameters_lang::{EXAMPLE, FILE_EXTENSION, check, format, parse, print, reference};

fn builtins() -> BTreeMap<String, Signature> {
    use ValueType::{Any, List, Number, Object, Path, Text};
    let node = |inputs: Vec<Port>, outputs: Vec<Port>| Signature { inputs, outputs };
    let command_outputs = || {
        vec![
            Port::optional("stdout", Text),
            Port::optional("exit_code", Number),
        ]
    };
    BTreeMap::from([
        (
            "process.run".to_owned(),
            node(
                vec![
                    Port::required("program", Text),
                    Port::optional("arguments", List),
                ],
                command_outputs(),
            ),
        ),
        (
            "shell.run".to_owned(),
            node(
                vec![
                    Port::required("command", Text),
                    Port::optional("directory", Path),
                ],
                command_outputs(),
            ),
        ),
        (
            "http.request".to_owned(),
            node(
                vec![
                    Port::required("url", Text),
                    Port::optional("method", Text),
                    Port::optional("headers", Object),
                    Port::optional("body", Any),
                ],
                vec![
                    Port::optional("status", Number),
                    Port::optional("body", Any),
                ],
            ),
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
                .is_some_and(|extension| extension == FILE_EXTENSION)
        })
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no fixtures in {}", directory.display());
    paths
}

fn compare(path: &Path, actual: &str) {
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
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
fn the_reference_example_is_canonical() {
    assert_eq!(format(EXAMPLE).unwrap(), EXAMPLE);
}

#[test]
fn text_converts_to_json_and_back_unchanged() {
    for path in fixtures("convert")
        .into_iter()
        .chain(std::iter::once(example_fixture()))
    {
        let text = fs::read_to_string(&path).unwrap();
        let parsed = parse(&text);
        assert!(
            !parsed.has_errors(),
            "{}: {:?}",
            path.display(),
            parsed.diagnostics
        );
        let json = to_json(&parsed.workflow);
        compare(&path.with_extension("json"), &json);
        assert_eq!(
            print(&from_json(&json).unwrap()),
            text,
            "{}",
            path.display()
        );
    }
}

#[test]
fn errors_point_at_the_right_text() {
    let builtins = builtins();
    for path in fixtures("errors") {
        let text = fs::read_to_string(&path).unwrap();
        let diagnostics = check(&text, &builtins);
        let mut rendered = String::new();
        for diagnostic in &diagnostics {
            let quoted = &text[diagnostic.span.start..diagnostic.span.end];
            let quoted = quoted.lines().next().unwrap_or_default();
            let _ = writeln!(rendered, "{diagnostic}\n    at `{quoted}`");
        }
        if rendered.is_empty() {
            rendered = "no diagnostics\n".to_owned();
        }
        compare(&path.with_extension("expected"), &rendered);
    }
}

#[test]
fn published_reference_matches_the_language() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/language.md");
    compare(&path, &reference());
}

fn example_fixture() -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("example.{FILE_EXTENSION}"));
    compare(&path, EXAMPLE);
    path
}
