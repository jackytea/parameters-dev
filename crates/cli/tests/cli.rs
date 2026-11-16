#![allow(
    clippy::unwrap_used,
    reason = "test harness: a failing file operation should fail the test loudly"
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use parameters_core::from_json;
use parameters_lang::EXAMPLE;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let directory =
            std::env::temp_dir().join(format!("parameters-cli-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        Self(directory)
    }

    fn file(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, contents).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn parameters(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_parameters"))
        .args(args)
        .output()
        .unwrap()
}

fn path(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn converts_text_to_laid_out_json_and_back_unchanged() {
    let scratch = Scratch::new("convert");
    let text = scratch.file("deploy.parameters", EXAMPLE);
    let json = scratch.0.join("deploy.json");
    let back = scratch.0.join("back.parameters");
    assert!(
        parameters(&["convert", path(&text), "--output", path(&json)])
            .status
            .success()
    );
    let workflow = from_json(&fs::read_to_string(&json).unwrap()).unwrap();
    assert_eq!(workflow.layout.len(), workflow.steps.len());
    assert!(
        parameters(&["convert", path(&json), "-o", path(&back)])
            .status
            .success()
    );
    assert_eq!(fs::read_to_string(&back).unwrap(), EXAMPLE);
    let printed = parameters(&["convert", path(&json)]);
    assert_eq!(String::from_utf8(printed.stdout).unwrap(), EXAMPLE);
}

#[test]
fn the_reference_example_passes_check_against_the_built_in_nodes() {
    let scratch = Scratch::new("example");
    let text = scratch.file("deploy.parameters", EXAMPLE);
    let output = parameters(&["check", path(&text)]);
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(output.status.success(), "{stderr}");
    assert!(stderr.contains("no problems"), "{stderr}");
}

#[test]
fn check_reports_problems_with_positions_and_fails() {
    let scratch = Scratch::new("check");
    let text = scratch.file(
        "broken.parameters",
        "workflow \"Broken\"\n\nstep build uses shell.run\n  command is \"make {chekout.path}\"\n",
    );
    let output = parameters(&["check", path(&text)]);
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr.contains("broken.parameters:4:21: error[P0302]: unknown reference `chekout`"),
        "{stderr}"
    );
    assert!(stderr.contains("1 error"), "{stderr}");
}

#[test]
fn fmt_reports_and_rewrites_files_that_are_not_canonical() {
    let scratch = Scratch::new("fmt");
    let messy = "workflow   \"Tidy\"\r\n\r\n\r\nstep   a   uses  log.write\r\n    message   is   (\"hi\")\r\n";
    let text = scratch.file("tidy.parameters", messy);
    assert_eq!(
        parameters(&["fmt", "--check", path(&text)]).status.code(),
        Some(1)
    );
    assert_eq!(fs::read_to_string(&text).unwrap(), messy);
    assert!(parameters(&["fmt", path(&text)]).status.success());
    assert_eq!(
        fs::read_to_string(&text).unwrap(),
        "workflow \"Tidy\"\n\nstep a uses log.write\n  message is \"hi\"\n"
    );
    assert!(
        parameters(&["fmt", "--check", path(&text)])
            .status
            .success()
    );
}

#[test]
fn usage_errors_exit_with_two() {
    assert_eq!(parameters(&[]).status.code(), Some(2));
    assert_eq!(parameters(&["launch"]).status.code(), Some(2));
    assert_eq!(parameters(&["check"]).status.code(), Some(2));
    assert_eq!(
        parameters(&["fmt", "--fast", "x.parameters"]).status.code(),
        Some(2)
    );
    assert_eq!(
        parameters(&["convert", "a.json", "b.json"]).status.code(),
        Some(2)
    );
    assert!(parameters(&["help"]).status.success());
}
