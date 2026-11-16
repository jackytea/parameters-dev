use std::process::Stdio;

use parameters_core::{Port, Signature, ValueType};
use serde_json::{Number, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use super::MAX_CAPTURED_BYTES;
use crate::node::{BoxFuture, Inputs, Node, NodeContext, NodeError, Values, scalar_text};

const ERROR_OUTPUT_PREVIEW: usize = 2_000;

pub(crate) struct ShellRun;

impl Node for ShellRun {
    fn description(&self) -> &'static str {
        "Runs a command through the system shell (`sh -c`, or `cmd /C` on Windows) in the workspace or `directory`. Fails on a non-zero exit code. Prefer `process.run` when arguments come from inputs or other steps."
    }

    fn privileged(&self) -> bool {
        true
    }

    fn signature(&self) -> Signature {
        Signature {
            inputs: vec![
                Port::required("command", ValueType::Text),
                Port::optional("directory", ValueType::Path),
                Port::optional("env", ValueType::Object),
            ],
            outputs: command_outputs(),
        }
    }

    fn run(&self, context: NodeContext, inputs: Inputs) -> BoxFuture<Result<Values, NodeError>> {
        Box::pin(async move {
            let script = inputs.text("command")?;
            let mut command = if cfg!(windows) {
                let mut command = Command::new("cmd");
                command.arg("/C").arg(&script);
                command
            } else {
                let mut command = Command::new("sh");
                command.arg("-c").arg(&script);
                command
            };
            configure(&mut command, &context, &inputs)?;
            execute(command, None, "the command").await
        })
    }
}

pub(crate) struct ProcessRun;

impl Node for ProcessRun {
    fn description(&self) -> &'static str {
        "Runs a program directly with a list of `arguments` and no shell, so values from inputs and other steps cannot inject commands. Fails on a non-zero exit code."
    }

    fn privileged(&self) -> bool {
        true
    }

    fn signature(&self) -> Signature {
        Signature {
            inputs: vec![
                Port::required("program", ValueType::Text),
                Port::optional("arguments", ValueType::List),
                Port::optional("directory", ValueType::Path),
                Port::optional("env", ValueType::Object),
                Port::optional("stdin", ValueType::Text),
            ],
            outputs: command_outputs(),
        }
    }

    fn run(&self, context: NodeContext, inputs: Inputs) -> BoxFuture<Result<Values, NodeError>> {
        Box::pin(async move {
            let program = inputs.text("program")?;
            let mut command = Command::new(&program);
            for argument in inputs.optional_list("arguments")?.into_iter().flatten() {
                command.arg(scalar_text(argument).ok_or_else(|| {
                    NodeError::new("every argument must be text, a number or a boolean")
                })?);
            }
            configure(&mut command, &context, &inputs)?;
            let input = inputs.optional_text("stdin")?;
            execute(command, input, &format!("`{program}`")).await
        })
    }
}

fn command_outputs() -> Vec<Port> {
    vec![
        Port::optional("stdout", ValueType::Text),
        Port::optional("stderr", ValueType::Text),
        Port::optional("exit_code", ValueType::Number),
    ]
}

fn configure(
    command: &mut Command,
    context: &NodeContext,
    inputs: &Inputs,
) -> Result<(), NodeError> {
    let directory = inputs.optional_text("directory")?.map_or_else(
        || context.directory().to_path_buf(),
        |directory| context.resolve(&directory),
    );
    command.current_dir(directory);
    for (name, value) in inputs.optional_object("env")?.into_iter().flatten() {
        let value = scalar_text(value).ok_or_else(|| {
            NodeError::new(format!("the environment variable `{name}` must be text"))
        })?;
        command.env(name, value);
    }
    Ok(())
}

async fn execute(
    mut command: Command,
    input: Option<String>,
    label: &str,
) -> Result<Values, NodeError> {
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|error| NodeError::new(format!("cannot start {label}: {error}")))?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        tokio::spawn(async move {
            let _ = stdin.write_all(input.as_bytes()).await;
        });
    }
    let stdout = child
        .stdout
        .take()
        .map(|stream| tokio::spawn(capture(stream)));
    let stderr = child
        .stderr
        .take()
        .map(|stream| tokio::spawn(capture(stream)));
    let status = child
        .wait()
        .await
        .map_err(|error| NodeError::new(format!("{label} could not be waited on: {error}")))?;
    let stdout = collect(stdout).await;
    let stderr = collect(stderr).await;
    let exit_code = status.code().unwrap_or(-1);
    if !status.success() {
        let tail: String = {
            let skip = stderr.chars().count().saturating_sub(ERROR_OUTPUT_PREVIEW);
            stderr.chars().skip(skip).collect()
        };
        return Err(NodeError::new(format!(
            "{label} exited with code {exit_code}: {}",
            tail.trim_end()
        )));
    }
    Ok(Values::from_iter([
        ("stdout".to_owned(), Value::String(stdout)),
        ("stderr".to_owned(), Value::String(stderr)),
        (
            "exit_code".to_owned(),
            Value::Number(Number::from(exit_code)),
        ),
    ]))
}

async fn collect(handle: Option<tokio::task::JoinHandle<String>>) -> String {
    match handle {
        Some(handle) => handle.await.unwrap_or_default(),
        None => String::new(),
    }
}

async fn capture(mut stream: impl AsyncRead + Unpin) -> String {
    let mut kept = Vec::new();
    let limit = u64::try_from(MAX_CAPTURED_BYTES).unwrap_or(u64::MAX);
    let _ = (&mut stream).take(limit).read_to_end(&mut kept).await;
    let discarded = tokio::io::copy(&mut stream, &mut tokio::io::sink())
        .await
        .unwrap_or(0);
    let mut text = String::from_utf8_lossy(&kept).into_owned();
    if discarded > 0 {
        text.push_str("\n[output truncated]");
    }
    text
}
