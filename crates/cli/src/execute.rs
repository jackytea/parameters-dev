use std::collections::HashMap;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use parameters_core::{ValueType, Workflow, from_json, validate};
use parameters_engine::{
    Engine, Event, Plan, Registry, RunControl, RunStatus, SecretStore, StepStatus, Values,
};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::{Failure, Format, files, format_of, read};

pub(crate) const SECRET_PREFIX: &str = "PARAMETERS_SECRET_";
const CANCELLED: u8 = 130;

struct EnvironmentSecrets;

impl SecretStore for EnvironmentSecrets {
    fn get(&self, name: &str) -> Option<String> {
        std::env::var(format!("{SECRET_PREFIX}{name}")).ok()
    }
}

pub(crate) fn run(args: &[String]) -> Result<ExitCode, Failure> {
    let (assignments, rest) = assignments(args)?;
    let [file] = files(&rest)?
        .try_into()
        .map_err(|_| Failure::Usage("run takes exactly one workflow file".into()))?;
    let source = read(&file)?;
    let registry = Registry::builtins();
    let Some(workflow) = load(&file, &source, &registry) else {
        return Ok(ExitCode::FAILURE);
    };
    let directory = std::env::current_dir()
        .map_err(|error| Failure::Io(format!("cannot read the current directory: {error}")))?;
    let engine =
        Engine::new(registry, directory).map_err(|error| Failure::Io(error.to_string()))?;
    let plan = engine
        .compile(&workflow)
        .map_err(|error| Failure::Io(error.to_string()))?;
    let inputs = input_values(&plan, assignments)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| Failure::Io(format!("cannot start the runtime: {error}")))?;
    let report = runtime.block_on(async {
        let (sender, receiver) = mpsc::channel(256);
        let printer = tokio::spawn(print_events(receiver));
        let control = RunControl {
            events: Some(sender),
            secrets: Arc::new(EnvironmentSecrets),
            ..RunControl::default()
        };
        let cancel = control.cancel.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                eprintln!("cancelling the run");
                cancel.cancel();
            }
        });
        let report = engine.run(&plan, inputs, control).await;
        let _ = printer.await;
        report
    });
    for error in &report.errors {
        eprintln!("error: {error}");
    }
    let status = match report.status {
        RunStatus::Succeeded => "succeeded",
        RunStatus::Failed => "failed",
        RunStatus::Cancelled => "was cancelled",
    };
    eprintln!("run {status} in {}", seconds(report.duration));
    match serde_json::to_string_pretty(&Value::Object(report.outputs)) {
        Ok(outputs) => println!("{outputs}"),
        Err(error) => eprintln!("error: cannot print the outputs: {error}"),
    }
    Ok(match report.status {
        RunStatus::Succeeded => ExitCode::SUCCESS,
        RunStatus::Failed => ExitCode::FAILURE,
        RunStatus::Cancelled => ExitCode::from(CANCELLED),
    })
}

fn load(file: &Path, source: &str, registry: &Registry) -> Option<Workflow> {
    match format_of(file) {
        Format::Text => {
            let diagnostics = parameters_lang::check(source, registry);
            for diagnostic in &diagnostics {
                eprintln!("{}:{diagnostic}", file.display());
            }
            if diagnostics
                .iter()
                .any(parameters_lang::TextDiagnostic::is_error)
            {
                return None;
            }
            Some(parameters_lang::parse(source).workflow)
        }
        Format::Json => {
            let workflow = match from_json(source) {
                Ok(workflow) => workflow,
                Err(error) => {
                    eprintln!("{}: error: {error}", file.display());
                    return None;
                }
            };
            let diagnostics = validate(&workflow, registry);
            for diagnostic in &diagnostics {
                eprintln!("{}: {diagnostic}", file.display());
            }
            (!diagnostics
                .iter()
                .any(parameters_core::Diagnostic::is_error))
            .then_some(workflow)
        }
    }
}

type Assignments = Vec<(String, String)>;

fn assignments(args: &[String]) -> Result<(Assignments, Vec<String>), Failure> {
    let mut assignments = Vec::new();
    let mut rest = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg != "--input" && arg != "-i" {
            rest.push(arg.clone());
            continue;
        }
        let assignment = iter
            .next()
            .ok_or_else(|| Failure::Usage(format!("`{arg}` needs name=value")))?;
        let (name, value) = assignment.split_once('=').ok_or_else(|| {
            Failure::Usage(format!("write inputs as name=value, not `{assignment}`"))
        })?;
        assignments.push((name.to_owned(), value.to_owned()));
    }
    Ok((assignments, rest))
}

fn input_values(plan: &Plan, assignments: Assignments) -> Result<Values, Failure> {
    let types: HashMap<&str, ValueType> = plan
        .inputs()
        .map(|(name, value_type, _)| (name, value_type))
        .collect();
    let mut values = Values::new();
    for (name, raw) in assignments {
        let value = match types.get(name.as_str()) {
            None => {
                return Err(Failure::Usage(format!(
                    "the workflow has no input `{name}`"
                )));
            }
            Some(ValueType::Secret) => {
                return Err(Failure::Usage(format!(
                    "`{name}` is a secret input; set the {SECRET_PREFIX}{name} environment variable instead"
                )));
            }
            Some(ValueType::Text | ValueType::Path) => Value::String(raw),
            Some(ValueType::Any) => serde_json::from_str(&raw).unwrap_or(Value::String(raw)),
            Some(value_type) => serde_json::from_str(&raw).map_err(|_| {
                Failure::Usage(format!(
                    "`{name}` must be {value_type}, written as JSON, not `{raw}`"
                ))
            })?,
        };
        if values.insert(name.clone(), value).is_some() {
            return Err(Failure::Usage(format!(
                "the input `{name}` is given more than once"
            )));
        }
    }
    Ok(values)
}

async fn print_events(mut receiver: mpsc::Receiver<Event>) {
    while let Some(event) = receiver.recv().await {
        match event {
            Event::StepStarted { path, attempt: 1 } => eprintln!("{:>10} {path}", "started"),
            Event::StepStarted { .. } => {}
            Event::StepRetrying {
                path,
                attempt,
                error,
                delay,
            } => eprintln!(
                "{:>10} {path} after attempt {attempt}, in {}: {error}",
                "retrying",
                seconds(delay)
            ),
            Event::StepFinished(step) => {
                let label = match step.status {
                    StepStatus::Succeeded => "succeeded",
                    StepStatus::Failed => "failed",
                    StepStatus::Skipped => "skipped",
                    StepStatus::Cancelled => "cancelled",
                };
                let timing = if step.attempts == 0 {
                    String::new()
                } else {
                    format!(" ({})", seconds(step.duration))
                };
                match step.message {
                    Some(message) => eprintln!("{label:>10} {}{timing}: {message}", step.path),
                    None => eprintln!("{label:>10} {}{timing}", step.path),
                }
            }
            Event::Log { path, message } => {
                for line in message.lines() {
                    eprintln!("{:>10} {path}: {line}", "log");
                }
            }
        }
    }
}

fn seconds(duration: Duration) -> String {
    format!("{:.2}s", duration.as_secs_f64())
}
