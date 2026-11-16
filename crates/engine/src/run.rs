use std::collections::{HashMap, VecDeque};
use std::hash::{BuildHasher, RandomState};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use parameters_core::{ValueType, Workflow};
use serde_json::Value;
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::eval::{Outcome, Scope};
use crate::event::{Event, RunReport, RunStatus, StepReport, StepStatus};
use crate::node::{BoxFuture, Inputs, NodeContext, Values};
use crate::plan::{Action, CompileError, GraphPlan, InputPlan, Plan, StepPlan, compile_plan};
use crate::redact::Redactor;
use crate::registry::Registry;
use crate::secrets::SecretStore;

pub const MAX_COMPOSITE_DEPTH: usize = 32;
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);
const DEFAULT_RETRY_DELAY: Duration = Duration::from_millis(500);

#[derive(Debug, thiserror::Error)]
#[error("cannot create the HTTP client: {0}")]
pub struct EngineError(String);

pub struct Engine {
    registry: Registry,
    http: reqwest::Client,
    secrets: Arc<dyn SecretStore>,
    directory: PathBuf,
    retry_delay: Duration,
}

#[derive(Debug, Clone, Default)]
pub struct RunControl {
    pub cancel: CancellationToken,
    pub events: Option<mpsc::Sender<Event>>,
}

pub(crate) struct Shared {
    pub(crate) http: reqwest::Client,
    pub(crate) directory: PathBuf,
    pub(crate) cancel: CancellationToken,
    pub(crate) redactor: Redactor,
    events: Option<mpsc::Sender<Event>>,
    secrets: Arc<dyn SecretStore>,
    permits: Arc<Semaphore>,
    definitions: Arc<HashMap<String, Arc<GraphPlan>>>,
    reports: Mutex<Vec<StepReport>>,
    retry_delay: Duration,
}

impl Shared {
    pub(crate) async fn emit(&self, event: Event) {
        if let Some(events) = &self.events {
            let _ = events.send(event).await;
        }
    }
}

struct GraphOutcome {
    failed: Vec<String>,
    outputs: Values,
    output_errors: Vec<String>,
}

enum Prepared {
    Run(Values),
    Done(StepStatus, Option<String>),
}

enum Attempt {
    Failed(String),
    Cancelled,
}

impl Engine {
    pub fn new(
        registry: Registry,
        secrets: Arc<dyn SecretStore>,
        directory: impl Into<PathBuf>,
    ) -> Result<Self, EngineError> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("parameters/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(30))
            .build()
            .map_err(|error| EngineError(error.to_string()))?;
        Ok(Self {
            registry,
            http,
            secrets,
            directory: directory.into(),
            retry_delay: DEFAULT_RETRY_DELAY,
        })
    }

    #[must_use]
    pub const fn with_retry_delay(mut self, delay: Duration) -> Self {
        self.retry_delay = delay;
        self
    }

    #[must_use]
    pub const fn registry(&self) -> &Registry {
        &self.registry
    }

    pub fn compile(&self, workflow: &Workflow) -> Result<Plan, CompileError> {
        compile_plan(workflow, &self.registry)
    }

    pub async fn run(&self, plan: &Plan, inputs: Values, control: RunControl) -> RunReport {
        let started = Instant::now();
        let shared = Arc::new(Shared {
            http: self.http.clone(),
            directory: self.directory.clone(),
            cancel: control.cancel,
            redactor: Redactor::default(),
            events: control.events,
            secrets: Arc::clone(&self.secrets),
            permits: Arc::new(Semaphore::new(
                plan.concurrency.clamp(1, Semaphore::MAX_PERMITS),
            )),
            definitions: Arc::clone(&plan.definitions),
            reports: Mutex::new(Vec::new()),
            retry_delay: self.retry_delay,
        });
        let values = match root_inputs(&plan.root.inputs, inputs, &shared) {
            Ok(values) => values,
            Err(errors) => {
                return RunReport {
                    status: RunStatus::Failed,
                    errors,
                    steps: Vec::new(),
                    outputs: Values::new(),
                    duration: started.elapsed(),
                };
            }
        };
        let outcome = run_graph(
            Arc::clone(&shared),
            Arc::clone(&plan.root),
            values,
            String::new(),
            0,
        )
        .await;
        let errors: Vec<String> = if outcome.failed.is_empty() {
            outcome
                .output_errors
                .iter()
                .map(|error| shared.redactor.text(error))
                .collect()
        } else {
            Vec::new()
        };
        let status = if shared.cancel.is_cancelled() {
            RunStatus::Cancelled
        } else if outcome.failed.is_empty() && errors.is_empty() {
            RunStatus::Succeeded
        } else {
            RunStatus::Failed
        };
        let steps = std::mem::take(
            &mut *shared
                .reports
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        RunReport {
            status,
            errors,
            steps,
            outputs: redact_values(&shared.redactor, &outcome.outputs),
            duration: started.elapsed(),
        }
    }
}

fn root_inputs(
    plans: &[InputPlan],
    mut provided: Values,
    shared: &Shared,
) -> Result<Vec<Value>, Vec<String>> {
    let mut values = Vec::with_capacity(plans.len());
    let mut errors = Vec::new();
    for input in plans {
        let name = &input.name;
        if input.value_type == ValueType::Secret {
            if provided.remove(name).is_some() {
                errors.push(format!(
                    "the secret input `{name}` cannot be given directly; it is read from the secret store"
                ));
                continue;
            }
            match shared.secrets.get(name) {
                Some(secret) => {
                    shared.redactor.add(&secret);
                    values.push(Value::String(secret));
                }
                None => errors.push(format!("the secret for the input `{name}` is not set")),
            }
            continue;
        }
        match provided.remove(name).or_else(|| input.default.clone()) {
            Some(value) if matches_type(&value, input.value_type) => values.push(value),
            Some(_) => errors.push(format!("the input `{name}` must be {}", input.value_type)),
            None => errors.push(format!("the input `{name}` is required")),
        }
    }
    errors.extend(
        provided
            .keys()
            .map(|name| format!("the workflow has no input `{name}`")),
    );
    if errors.is_empty() {
        Ok(values)
    } else {
        Err(errors)
    }
}

const fn matches_type(value: &Value, value_type: ValueType) -> bool {
    matches!(
        (value_type, value),
        (ValueType::Any, _)
            | (
                ValueType::Text | ValueType::Path | ValueType::Secret,
                Value::String(_)
            )
            | (ValueType::Number, Value::Number(_))
            | (ValueType::Boolean, Value::Bool(_))
            | (ValueType::List, Value::Array(_))
            | (ValueType::Object, Value::Object(_))
    )
}

fn run_graph(
    shared: Arc<Shared>,
    graph: Arc<GraphPlan>,
    inputs: Vec<Value>,
    prefix: String,
    depth: usize,
) -> BoxFuture<GraphOutcome> {
    Box::pin(async move {
        let count = graph.steps.len();
        let mut remaining: Vec<usize> = (0..count)
            .map(|step| graph.graph.dependencies(step).len())
            .collect();
        let mut ready: VecDeque<usize> = (0..count).filter(|&step| remaining[step] == 0).collect();
        let mut results: Vec<Option<Outcome>> = vec![None; count];
        let mut running = JoinSet::new();
        let mut tasks = HashMap::new();
        let mut failed = Vec::new();
        loop {
            while let Some(index) = ready.pop_front() {
                let path = step_path(&prefix, &graph.step_ids[index]);
                let outcome = match prepare(&shared, &graph, &inputs, &results, index) {
                    Prepared::Run(values) => {
                        let task = running.spawn(execute(
                            Arc::clone(&shared),
                            Arc::clone(&graph),
                            index,
                            values,
                            path,
                            depth,
                        ));
                        tasks.insert(task.id(), index);
                        continue;
                    }
                    Prepared::Done(status, message) => {
                        finish(
                            &shared,
                            path,
                            status,
                            0,
                            message,
                            Values::new(),
                            Duration::ZERO,
                        )
                        .await
                    }
                };
                complete(
                    &graph,
                    index,
                    outcome,
                    &mut results,
                    &mut remaining,
                    &mut ready,
                    &mut failed,
                    &prefix,
                );
            }
            let Some(joined) = running.join_next_with_id().await else {
                break;
            };
            let (index, outcome) = match joined {
                Ok((_, (index, outcome))) => (index, outcome),
                Err(error) => {
                    let index = tasks.get(&error.id()).copied().unwrap_or_default();
                    let path = step_path(&prefix, &graph.step_ids[index]);
                    let message = Some("the step stopped unexpectedly".to_owned());
                    let outcome = finish(
                        &shared,
                        path,
                        StepStatus::Failed,
                        1,
                        message,
                        Values::new(),
                        Duration::ZERO,
                    )
                    .await;
                    (index, outcome)
                }
            };
            complete(
                &graph,
                index,
                outcome,
                &mut results,
                &mut remaining,
                &mut ready,
                &mut failed,
                &prefix,
            );
        }
        let (outputs, output_errors) = graph_outputs(&shared, &graph, &inputs, &results);
        GraphOutcome {
            failed,
            outputs,
            output_errors,
        }
    })
}

fn graph_outputs(
    shared: &Shared,
    graph: &GraphPlan,
    inputs: &[Value],
    results: &[Option<Outcome>],
) -> (Values, Vec<String>) {
    let scope = Scope {
        inputs,
        steps: results,
        step_ids: &graph.step_ids,
        secrets: shared.secrets.as_ref(),
        redactor: &shared.redactor,
    };
    let mut outputs = Values::new();
    let mut errors = Vec::new();
    for (name, value) in &graph.outputs {
        match value.evaluate(&scope) {
            Ok(value) => {
                outputs.insert(name.clone(), value);
            }
            Err(error) => errors.push(format!(
                "the output `{name}` could not be produced: {error}"
            )),
        }
    }
    (outputs, errors)
}

#[expect(
    clippy::too_many_arguments,
    reason = "the scheduler's bookkeeping is updated in one place"
)]
fn complete(
    graph: &GraphPlan,
    index: usize,
    outcome: Outcome,
    results: &mut [Option<Outcome>],
    remaining: &mut [usize],
    ready: &mut VecDeque<usize>,
    failed: &mut Vec<String>,
    prefix: &str,
) {
    if outcome.status == StepStatus::Failed {
        failed.push(step_path(prefix, &graph.step_ids[index]));
    }
    results[index] = Some(outcome);
    for &dependent in graph.graph.dependents(index) {
        remaining[dependent] -= 1;
        if remaining[dependent] == 0 {
            ready.push_back(dependent);
        }
    }
}

fn prepare(
    shared: &Shared,
    graph: &GraphPlan,
    inputs: &[Value],
    results: &[Option<Outcome>],
    index: usize,
) -> Prepared {
    if shared.cancel.is_cancelled() {
        return Prepared::Done(StepStatus::Cancelled, None);
    }
    let step = &graph.steps[index];
    let scope = Scope {
        inputs,
        steps: results,
        step_ids: &graph.step_ids,
        secrets: shared.secrets.as_ref(),
        redactor: &shared.redactor,
    };
    match &step.when {
        None => {
            let blocker = graph
                .graph
                .dependencies(index)
                .iter()
                .find_map(|&dependency| {
                    let status = results[dependency].as_ref()?.status;
                    (status != StepStatus::Succeeded).then_some((dependency, status))
                });
            if let Some((dependency, status)) = blocker {
                let reason = format!("`{}` {}", graph.step_ids[dependency], describe(status));
                return Prepared::Done(StepStatus::Skipped, Some(reason));
            }
        }
        Some(condition) => match condition.condition(&scope) {
            Ok(true) => {}
            Ok(false) => {
                return Prepared::Done(
                    StepStatus::Skipped,
                    Some("its `when` condition is false".to_owned()),
                );
            }
            Err(error) => {
                return Prepared::Done(StepStatus::Failed, Some(format!("`when`: {error}")));
            }
        },
    }
    let mut values = Values::new();
    for (name, value) in &step.fields {
        match value.evaluate(&scope) {
            Ok(value) => {
                values.insert(name.clone(), value);
            }
            Err(error) => {
                return Prepared::Done(StepStatus::Failed, Some(format!("`{name}`: {error}")));
            }
        }
    }
    Prepared::Run(values)
}

async fn execute(
    shared: Arc<Shared>,
    graph: Arc<GraphPlan>,
    index: usize,
    values: Values,
    path: String,
    depth: usize,
) -> (usize, Outcome) {
    let step = &graph.steps[index];
    let started = Instant::now();
    let mut attempt = 0;
    let (status, message, outputs) = loop {
        attempt += 1;
        shared
            .emit(Event::StepStarted {
                path: path.clone(),
                attempt,
            })
            .await;
        match attempt_once(&shared, step, &values, &path, depth).await {
            Ok(outputs) => break (StepStatus::Succeeded, None, outputs),
            Err(Attempt::Cancelled) => break (StepStatus::Cancelled, None, Values::new()),
            Err(Attempt::Failed(error)) if attempt < step.attempts => {
                let delay = backoff(shared.retry_delay, attempt);
                shared
                    .emit(Event::StepRetrying {
                        path: path.clone(),
                        attempt,
                        error: shared.redactor.text(&error),
                        delay,
                    })
                    .await;
                if shared
                    .cancel
                    .run_until_cancelled(tokio::time::sleep(delay))
                    .await
                    .is_none()
                {
                    break (StepStatus::Cancelled, None, Values::new());
                }
            }
            Err(Attempt::Failed(error)) => break (StepStatus::Failed, Some(error), Values::new()),
        }
    };
    let outcome = finish(
        &shared,
        path,
        status,
        attempt,
        message,
        outputs,
        started.elapsed(),
    )
    .await;
    (index, outcome)
}

async fn attempt_once(
    shared: &Arc<Shared>,
    step: &StepPlan,
    values: &Values,
    path: &str,
    depth: usize,
) -> Result<Values, Attempt> {
    let work: BoxFuture<Result<Values, String>> = match &step.action {
        Action::Node(node) => {
            let permit = shared
                .cancel
                .run_until_cancelled(Arc::clone(&shared.permits).acquire_owned())
                .await
                .ok_or(Attempt::Cancelled)?
                .map_err(|_| Attempt::Cancelled)?;
            let context = NodeContext {
                shared: Arc::clone(shared),
                path: Arc::from(path),
            };
            let running = node.run(context, Inputs(values.clone()));
            Box::pin(async move {
                let result = running.await.map_err(|error| error.message);
                drop(permit);
                result
            })
        }
        Action::Composite(name) => composite(
            Arc::clone(shared),
            name.clone(),
            values.clone(),
            path.to_owned(),
            depth + 1,
        ),
    };
    let limited = async {
        match step.timeout {
            Some(limit) => tokio::time::timeout(limit.to_std(), work)
                .await
                .unwrap_or_else(|_| Err(format!("timed out after {limit}"))),
            None => work.await,
        }
    };
    match shared.cancel.run_until_cancelled(limited).await {
        None => Err(Attempt::Cancelled),
        Some(Ok(outputs)) => Ok(outputs),
        Some(Err(error)) => Err(Attempt::Failed(error)),
    }
}

fn composite(
    shared: Arc<Shared>,
    name: String,
    values: Values,
    path: String,
    depth: usize,
) -> BoxFuture<Result<Values, String>> {
    Box::pin(async move {
        if depth > MAX_COMPOSITE_DEPTH {
            return Err(format!(
                "custom nodes are nested more than {MAX_COMPOSITE_DEPTH} levels deep"
            ));
        }
        let graph = shared
            .definitions
            .get(&name)
            .cloned()
            .ok_or_else(|| format!("the custom node `{name}` does not exist"))?;
        let inputs = graph
            .inputs
            .iter()
            .map(|input| {
                let value = values
                    .get(&input.name)
                    .cloned()
                    .or_else(|| input.default.clone())
                    .ok_or_else(|| format!("the field `{}` is required", input.name))?;
                if matches_type(&value, input.value_type) {
                    Ok(value)
                } else {
                    Err(format!(
                        "the field `{}` must be {}",
                        input.name, input.value_type
                    ))
                }
            })
            .collect::<Result<Vec<_>, String>>()?;
        let outcome = run_graph(shared, graph, inputs, path, depth).await;
        if !outcome.failed.is_empty() {
            let steps: Vec<String> = outcome
                .failed
                .iter()
                .map(|step| format!("`{step}`"))
                .collect();
            return Err(format!("{} failed", steps.join(", ")));
        }
        if !outcome.output_errors.is_empty() {
            return Err(outcome.output_errors.join("; "));
        }
        Ok(outcome.outputs)
    })
}

async fn finish(
    shared: &Shared,
    path: String,
    status: StepStatus,
    attempts: u32,
    message: Option<String>,
    outputs: Values,
    duration: Duration,
) -> Outcome {
    let report = StepReport {
        path,
        status,
        attempts,
        message: message.map(|message| shared.redactor.text(&message)),
        outputs: redact_values(&shared.redactor, &outputs),
        duration,
    };
    shared
        .reports
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(report.clone());
    shared.emit(Event::StepFinished(report)).await;
    Outcome {
        status,
        outputs: Arc::new(outputs),
    }
}

fn redact_values(redactor: &Redactor, values: &Values) -> Values {
    values
        .iter()
        .map(|(name, value)| (name.clone(), redactor.value(value)))
        .collect()
}

fn backoff(base: Duration, attempt: u32) -> Duration {
    let growth = 2_u32.saturating_pow(attempt.saturating_sub(1));
    let capped = base.saturating_mul(growth).min(MAX_RETRY_DELAY);
    let random = RandomState::new().hash_one(attempt);
    let fraction = 500 + u128::from(random % 500);
    let nanos = capped.as_nanos() * fraction / 1000;
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

fn step_path(prefix: &str, id: &str) -> String {
    if prefix.is_empty() {
        id.to_owned()
    } else {
        format!("{prefix}/{id}")
    }
}

const fn describe(status: StepStatus) -> &'static str {
    match status {
        StepStatus::Succeeded => "succeeded",
        StepStatus::Failed => "failed",
        StepStatus::Skipped => "was skipped",
        StepStatus::Cancelled => "was cancelled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_with_jitter_and_is_capped() {
        let base = Duration::from_millis(100);
        for attempt in 1..=3 {
            let full = base * 2_u32.pow(attempt - 1);
            let delay = backoff(base, attempt);
            assert!(delay >= full / 2 && delay <= full, "{attempt}: {delay:?}");
        }
        assert!(backoff(base, 40) <= MAX_RETRY_DELAY);
    }
}
