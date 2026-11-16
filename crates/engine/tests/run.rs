#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "integration tests: a broken fixture should fail the test loudly"
)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use parameters_engine::{
    Engine, Event, MASK, Registry, RunControl, RunReport, RunStatus, StepStatus, Values,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
struct Request {
    path: String,
    headers: HashMap<String, String>,
    body: String,
}

#[derive(Clone)]
struct Stub {
    address: SocketAddr,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl Stub {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let stub = Self {
            address: listener.local_addr().unwrap(),
            requests: Arc::default(),
        };
        let requests = Arc::clone(&stub.requests);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve(stream, Arc::clone(&requests)));
            }
        });
        stub
    }

    fn base(&self) -> String {
        format!("http://{}", self.address)
    }

    fn received(&self, path: &str) -> Vec<Request> {
        let requests = self.requests.lock().unwrap();
        requests
            .iter()
            .filter(|request| request.path == path)
            .cloned()
            .collect()
    }
}

async fn serve(mut stream: TcpStream, requests: Arc<Mutex<Vec<Request>>>) {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        let read = stream.read(&mut chunk).await.unwrap_or(0);
        if read == 0 {
            return;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let head = String::from_utf8_lossy(&buffer[..header_end]).into_owned();
    let mut lines = head.lines();
    let path = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or_default()
        .to_owned();
    let headers: HashMap<String, String> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let length: usize = headers
        .get("content-length")
        .and_then(|length| length.parse().ok())
        .unwrap_or(0);
    while buffer.len() < header_end + length {
        let read = stream.read(&mut chunk).await.unwrap_or(0);
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let body = String::from_utf8_lossy(&buffer[header_end..]).into_owned();
    let request = Request {
        path: path.clone(),
        headers,
        body,
    };
    let earlier = {
        let mut requests = requests.lock().unwrap();
        requests.push(request.clone());
        requests.iter().filter(|seen| seen.path == path).count() - 1
    };
    let (status, content_type, body) = match path.as_str() {
        "/deploy" => (
            200,
            "application/json",
            json!({ "deploy_url": "https://example.test/site" }).to_string(),
        ),
        "/flaky" if earlier < 2 => (503, "text/plain", "try again".to_owned()),
        "/flaky" => (200, "application/json", json!({ "ok": true }).to_string()),
        "/slow" => {
            tokio::time::sleep(Duration::from_secs(5)).await;
            (200, "text/plain", "late".to_owned())
        }
        "/echo" => (
            200,
            "application/json",
            json!({
                "authorization": request.headers.get("authorization"),
                "body": request.body,
            })
            .to_string(),
        ),
        _ => (404, "text/plain", "not here".to_owned()),
    };
    let reason = if status == 200 { "OK" } else { "Error" };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

struct Outcome {
    report: RunReport,
    events: Vec<Event>,
    elapsed: Duration,
}

impl Outcome {
    fn status(&self, path: &str) -> StepStatus {
        self.report
            .step(path)
            .unwrap_or_else(|| panic!("no step `{path}` in {:#?}", self.report))
            .status
    }

    fn message(&self, path: &str) -> String {
        self.report
            .step(path)
            .and_then(|step| step.message.clone())
            .unwrap_or_default()
    }

    fn output(&self, path: &str, name: &str) -> Value {
        self.report
            .step(path)
            .and_then(|step| step.outputs.get(name))
            .cloned()
            .unwrap_or_else(|| panic!("no output `{path}.{name}` in {:#?}", self.report))
    }

    fn finished_before_started(&self, first: &str, then: &str) -> bool {
        let finished = self
            .events
            .iter()
            .position(|event| matches!(event, Event::StepFinished(report) if report.path == first));
        let started = self
            .events
            .iter()
            .position(|event| matches!(event, Event::StepStarted { path, .. } if path == then));
        matches!((finished, started), (Some(finished), Some(started)) if finished < started)
    }
}

async fn run_in(
    directory: &Path,
    source: &str,
    inputs: Value,
    secrets: &[(&str, &str)],
    control: RunControl,
) -> Outcome {
    let parsed = parameters_lang::parse(source);
    assert!(!parsed.has_errors(), "{:#?}", parsed.diagnostics);
    let secrets: HashMap<String, String> = secrets
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect();
    let engine = Engine::new(Registry::builtins(), Arc::new(secrets), directory)
        .unwrap()
        .with_retry_delay(Duration::from_millis(1));
    let plan = engine
        .compile(&parsed.workflow)
        .unwrap_or_else(|error| panic!("{error}"));
    let (sender, mut receiver) = mpsc::channel(1024);
    let collector = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Some(event) = receiver.recv().await {
            events.push(event);
        }
        events
    });
    let inputs: Values = inputs.as_object().cloned().unwrap_or_default();
    let started = Instant::now();
    let report = engine
        .run(
            &plan,
            inputs,
            RunControl {
                events: Some(sender),
                ..control
            },
        )
        .await;
    let elapsed = started.elapsed();
    drop(engine);
    let events = collector.await.unwrap();
    Outcome {
        report,
        events,
        elapsed,
    }
}

async fn run(source: &str, inputs: Value, secrets: &[(&str, &str)]) -> Outcome {
    run_in(
        &std::env::temp_dir(),
        source,
        inputs,
        secrets,
        RunControl::default(),
    )
    .await
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let directory =
            std::env::temp_dir().join(format!("parameters-engine-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        Self(directory)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const PARALLEL: &str = r#"workflow "Parallel"

step a uses time.wait
  duration is "300ms"

step b uses time.wait
  duration is "300ms"

step c uses log.write
  after a and b
  message is "both finished"
"#;

#[tokio::test(flavor = "multi_thread")]
async fn independent_steps_run_in_parallel_and_dependents_wait_for_all_of_them() {
    let outcome = run(PARALLEL, json!({}), &[]).await;
    assert_eq!(outcome.report.status, RunStatus::Succeeded);
    assert!(
        outcome.elapsed >= Duration::from_millis(300),
        "{:?}",
        outcome.elapsed
    );
    assert!(
        outcome.elapsed < Duration::from_millis(550),
        "{:?}",
        outcome.elapsed
    );
    assert!(outcome.finished_before_started("a", "c"));
    assert!(outcome.finished_before_started("b", "c"));
    assert!(outcome.events.iter().any(|event| matches!(
        event,
        Event::Log { path, message } if path == "c" && message == "both finished"
    )));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_concurrency_limit_holds_steps_back() {
    let limited = PARALLEL.replacen(
        "workflow \"Parallel\"\n",
        "workflow \"Parallel\"\n  concurrency 1\n",
        1,
    );
    let outcome = run(&limited, json!({}), &[]).await;
    assert_eq!(outcome.report.status, RunStatus::Succeeded);
    assert!(
        outcome.elapsed >= Duration::from_millis(600),
        "{:?}",
        outcome.elapsed
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn requests_use_secrets_and_every_reported_secret_is_masked() {
    let stub = Stub::start().await;
    let source = r#"workflow "Echo"

input base: text
input token: secret

step call uses http.request
  method is "post"
  url is "{base}/echo"
  headers is { Authorization: "Bearer {token}" }
  body is { site: "docs" }

output authorization is call.body.authorization
"#;
    let outcome = run(
        source,
        json!({ "base": stub.base() }),
        &[("token", "s3cr3t-value")],
    )
    .await;
    assert_eq!(
        outcome.report.status,
        RunStatus::Succeeded,
        "{:#?}",
        outcome.report
    );
    let received = stub.received("/echo");
    assert_eq!(received[0].headers["authorization"], "Bearer s3cr3t-value");
    assert_eq!(received[0].body, r#"{"site":"docs"}"#);
    assert_eq!(
        outcome.report.outputs["authorization"],
        json!(format!("Bearer {MASK}"))
    );
    assert_eq!(
        outcome.output("call", "body")["authorization"],
        json!(format!("Bearer {MASK}"))
    );
    let everything = format!("{:?}{:?}", outcome.report, outcome.events);
    assert!(!everything.contains("s3cr3t-value"));
}

#[tokio::test(flavor = "multi_thread")]
async fn failing_steps_are_retried_until_they_succeed() {
    let stub = Stub::start().await;
    let source = r#"workflow "Flaky"

input base: text

step call uses http.request
  retry 3 times
  url is "{base}/flaky"
"#;
    let outcome = run(source, json!({ "base": stub.base() }), &[]).await;
    assert_eq!(outcome.status("call"), StepStatus::Succeeded);
    assert_eq!(outcome.report.step("call").unwrap().attempts, 3);
    let retries = outcome
        .events
        .iter()
        .filter(|event| matches!(event, Event::StepRetrying { .. }))
        .count();
    assert_eq!(retries, 2);
    let fresh = Stub::start().await;
    let once = run(
        &source.replace(
            "  retry 3 times
",
            "",
        ),
        json!({ "base": fresh.base() }),
        &[],
    )
    .await;
    assert_eq!(once.status("call"), StepStatus::Failed);
    assert!(
        once.message("call").contains("returned 503"),
        "{}",
        once.message("call")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn timeouts_fail_the_step_and_only_failure_handlers_run_after_it() {
    let stub = Stub::start().await;
    let source = r#"workflow "Slow"

input base: text

step slow uses http.request
  timeout 100ms
  url is "{base}/slow"

step next uses log.write
  after slow
  message is "never"

step recover uses log.write
  when slow failed
  message is "recovered"

step later uses log.write
  after next
  message is "never either"
"#;
    let outcome = run(source, json!({ "base": stub.base() }), &[]).await;
    assert_eq!(outcome.report.status, RunStatus::Failed);
    assert_eq!(outcome.status("slow"), StepStatus::Failed);
    assert_eq!(outcome.message("slow"), "timed out after 100ms");
    assert_eq!(outcome.status("next"), StepStatus::Skipped);
    assert_eq!(outcome.message("next"), "`slow` failed");
    assert_eq!(outcome.status("later"), StepStatus::Skipped);
    assert_eq!(outcome.message("later"), "`next` was skipped");
    assert_eq!(outcome.status("recover"), StepStatus::Succeeded);
    assert!(
        outcome.elapsed < Duration::from_secs(2),
        "{:?}",
        outcome.elapsed
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn custom_nodes_run_their_own_steps_and_return_outputs() {
    let stub = Stub::start().await;
    let source = r#"workflow "Publish"

input base: text

step publish uses acme.deploy
  url is "{base}/deploy"

output site is publish.url

define node acme.deploy
  input url: text
  input method: text = "POST"

  step upload uses http.request
    method is method
    url is url

  output url: text is upload.body.deploy_url
"#;
    let outcome = run(source, json!({ "base": stub.base() }), &[]).await;
    assert_eq!(
        outcome.report.status,
        RunStatus::Succeeded,
        "{:#?}",
        outcome.report
    );
    assert_eq!(
        outcome.report.outputs["site"],
        json!("https://example.test/site")
    );
    assert_eq!(outcome.status("publish/upload"), StepStatus::Succeeded);
    assert_eq!(
        outcome.output("publish", "url"),
        json!("https://example.test/site")
    );
    assert_eq!(stub.received("/deploy").len(), 1);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn commands_and_files_work_relative_to_the_run_directory() {
    let scratch = Scratch::new("files");
    let source = r#"workflow "Files"

step write uses file.write
  path is "out/hello.txt"
  content is "hello"

step read uses file.read
  path is write.path

step list uses file.list
  after write
  directory is "out"

step shell uses shell.run
  after write
  command is "cat out/hello.txt; echo warning >&2"

step piped uses process.run
  program is "cat"
  stdin is read.content

step broken uses shell.run
  command is "echo broken >&2; exit 3"
"#;
    let outcome = run_in(&scratch.0, source, json!({}), &[], RunControl::default()).await;
    assert_eq!(outcome.output("read", "content"), json!("hello"));
    assert_eq!(outcome.output("list", "paths"), json!(["out/hello.txt"]));
    assert_eq!(outcome.output("shell", "stdout"), json!("hello"));
    assert_eq!(outcome.output("shell", "stderr"), json!("warning\n"));
    assert_eq!(outcome.output("shell", "exit_code"), json!(0));
    assert_eq!(outcome.output("piped", "stdout"), json!("hello"));
    assert_eq!(outcome.status("broken"), StepStatus::Failed);
    assert_eq!(
        outcome.message("broken"),
        "the command exited with code 3: broken"
    );
    assert!(scratch.0.join("out/hello.txt").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn inputs_are_checked_before_anything_runs() {
    let source = r#"workflow "Inputs"

input name: text
input count: number = 2
input token: secret

step greet uses log.write
  message is "{name} {count}"
"#;
    let outcome = run(
        source,
        json!({ "count": "two", "extra": 1, "token": "inline" }),
        &[],
    )
    .await;
    assert_eq!(outcome.report.status, RunStatus::Failed);
    assert_eq!(outcome.report.steps, []);
    assert_eq!(
        outcome.report.errors,
        [
            "the input `name` is required",
            "the input `count` must be number",
            "the secret input `token` cannot be given directly; it is read from the secret store",
            "the workflow has no input `extra`",
        ]
    );
    let defaults = run(source, json!({ "name": "world" }), &[("token", "x")]).await;
    assert_eq!(defaults.report.status, RunStatus::Succeeded);
    assert!(defaults.events.iter().any(|event| matches!(
        event,
        Event::Log { message, .. } if message == "world 2"
    )));
}

#[tokio::test(flavor = "multi_thread")]
async fn false_conditions_skip_steps_and_the_skip_spreads_to_dependents() {
    let source = r#"workflow "Conditions"

input deploy: boolean = false

step build uses log.write
  message is "building"

step release uses log.write
  when deploy and build succeeded
  message is "releasing"

step announce uses log.write
  after release
  message is "announcing"

output built is build succeeded
"#;
    let outcome = run(source, json!({}), &[]).await;
    assert_eq!(outcome.report.status, RunStatus::Succeeded);
    assert_eq!(outcome.status("release"), StepStatus::Skipped);
    assert_eq!(outcome.message("release"), "its `when` condition is false");
    assert_eq!(outcome.status("announce"), StepStatus::Skipped);
    assert_eq!(outcome.report.outputs["built"], json!(true));
    let enabled = run(source, json!({ "deploy": true }), &[]).await;
    assert_eq!(enabled.status("announce"), StepStatus::Succeeded);
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_a_run_stops_waiting_steps_promptly() {
    let source = r#"workflow "Long"

step wait uses time.wait
  duration is "10s"

step after_wait uses log.write
  after wait
  message is "never"
"#;
    let control = RunControl::default();
    let cancel = control.cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel.cancel();
    });
    let outcome = run_in(&std::env::temp_dir(), source, json!({}), &[], control).await;
    assert_eq!(outcome.report.status, RunStatus::Cancelled);
    assert_eq!(outcome.status("wait"), StepStatus::Cancelled);
    assert_eq!(outcome.status("after_wait"), StepStatus::Cancelled);
    assert!(
        outcome.elapsed < Duration::from_secs(1),
        "{:?}",
        outcome.elapsed
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn http_errors_fail_the_step_with_the_status_and_body() {
    let stub = Stub::start().await;
    let source = r#"workflow "Missing"

input base: text

step call uses http.request
  url is "{base}/missing"
"#;
    let outcome = run(source, json!({ "base": stub.base() }), &[]).await;
    assert_eq!(outcome.status("call"), StepStatus::Failed);
    assert!(
        outcome
            .message("call")
            .ends_with("returned 404 Not Found: not here"),
        "{}",
        outcome.message("call")
    );
}
