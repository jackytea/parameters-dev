use std::sync::Arc;
use std::time::Duration;

use parameters_api::{RunState, StepState};
use parameters_core::from_json;
use parameters_engine::{Event, RunControl, RunStatus, StepStatus, Values};
use parameters_store::{ClaimedRun, LogWrite, Notification, StepWrite, StoreError};
use serde_json::Value;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::review::inspect;
use crate::{Service, StartError};

const EVENT_BUFFER: usize = 256;
const MAX_BATCH: usize = 500;
const MAX_LOG_BYTES: usize = 64 * 1024;
const LISTENER_RETRY: Duration = Duration::from_secs(1);

pub struct Background {
    service: Service,
    shutdown: CancellationToken,
    tasks: JoinSet<()>,
}

impl Background {
    pub async fn shutdown(mut self) {
        self.shutdown.cancel();
        while self.tasks.join_next().await.is_some() {}
        let worker = self.service.inner.worker;
        match self.service.store().release_runs(worker).await {
            Ok(released) if !released.is_empty() => {
                tracing::info!(
                    count = released.len(),
                    "queued unfinished runs to start again"
                );
            }
            Ok(_) => {}
            Err(error) => tracing::error!(%error, "cannot release unfinished runs"),
        }
    }
}

impl Service {
    pub async fn run_in_background(&self) -> Result<Background, StartError> {
        let listener = self.store().listen().await?;
        let shutdown = CancellationToken::new();
        let mut tasks = JoinSet::new();
        tasks.spawn(listen(self.clone(), listener, shutdown.clone()));
        tasks.spawn(recover(self.clone(), shutdown.clone()));
        for _ in 0..self.inner.options.max_runs.max(1) {
            tasks.spawn(work(self.clone(), shutdown.clone()));
        }
        Ok(Background {
            service: self.clone(),
            shutdown,
            tasks,
        })
    }
}

async fn listen(
    service: Service,
    mut listener: parameters_store::Listener,
    shutdown: CancellationToken,
) {
    loop {
        let received = tokio::select! {
            () = shutdown.cancelled() => return,
            received = listener.recv() => received,
        };
        match received {
            Ok(notification) => service.inner.hub.publish(notification),
            Err(error) => {
                tracing::warn!(%error, "lost the database notification connection");
                service.inner.hub.publish(Notification::Resync);
                if shutdown
                    .run_until_cancelled(tokio::time::sleep(LISTENER_RETRY))
                    .await
                    .is_none()
                {
                    return;
                }
            }
        }
    }
}

async fn recover(service: Service, shutdown: CancellationToken) {
    let options = &service.inner.options;
    let mut ticker = tokio::time::interval(options.heartbeat);
    while shutdown.run_until_cancelled(ticker.tick()).await.is_some() {
        match service
            .store()
            .recover_stale_runs(options.stale_after, options.max_attempts)
            .await
        {
            Ok(recovered) => {
                for (run, status) in recovered {
                    tracing::warn!(%run, status = status.name(), "recovered a run whose server stopped");
                }
            }
            Err(error) => tracing::error!(%error, "cannot recover stale runs"),
        }
    }
}

async fn work(service: Service, shutdown: CancellationToken) {
    let worker = service.inner.worker;
    while !shutdown.is_cancelled() {
        let queued = service.inner.hub.queue.notified();
        tokio::pin!(queued);
        queued.as_mut().enable();
        match service.store().claim_run(worker).await {
            Ok(Some(run)) => {
                if shutdown
                    .run_until_cancelled(execute(&service, run))
                    .await
                    .is_none()
                {
                    return;
                }
            }
            Ok(None) => {
                let idle = tokio::time::sleep(service.inner.options.idle_poll);
                tokio::select! {
                    () = shutdown.cancelled() => return,
                    () = queued => {}
                    () = idle => {}
                }
            }
            Err(error) => {
                tracing::error!(%error, "cannot claim a run");
                let pause = tokio::time::sleep(service.inner.options.idle_poll);
                if shutdown.run_until_cancelled(pause).await.is_none() {
                    return;
                }
            }
        }
    }
}

async fn execute(service: &Service, run: ClaimedRun) {
    let id = run.id;
    let outcome = if run.cancel_requested {
        Finished::new(RunState::Cancelled, Vec::new())
    } else {
        match prepare(service, &run).await {
            Ok(prepared) => perform(service, &run, prepared).await,
            Err(error) => Finished::new(RunState::Failed, vec![error]),
        }
    };
    let finished = service
        .store()
        .finish_run(
            id,
            service.inner.worker,
            outcome.status,
            outcome.outputs,
            outcome.errors,
        )
        .await;
    match finished {
        Ok(true) => {}
        Ok(false) => tracing::warn!(%id, "another server took over the run before it finished"),
        Err(error) => tracing::error!(%id, %error, "cannot record the end of the run"),
    }
}

struct Prepared {
    plan: Arc<parameters_engine::Plan>,
    secrets: std::collections::HashMap<String, String>,
    inputs: Values,
}

async fn prepare(service: &Service, run: &ClaimedRun) -> Result<Prepared, String> {
    let workflow = from_json(&run.snapshot).map_err(|error| error.to_string())?;
    let plan = service
        .inner
        .plans
        .get_or_compile(&run.hash, &workflow, &service.inner.engine)
        .map_err(|error| error.to_string())?;
    let names = inspect(&workflow, service.registry()).secrets;
    let secrets = service.load_secrets(run.owner, &names).await?;
    let inputs = match &run.inputs {
        Value::Object(inputs) => inputs.clone(),
        _ => Values::new(),
    };
    Ok(Prepared {
        plan,
        secrets,
        inputs,
    })
}

struct Finished {
    status: RunState,
    outputs: Value,
    errors: Vec<String>,
}

impl Finished {
    fn new(status: RunState, errors: Vec<String>) -> Self {
        Self {
            status,
            outputs: Value::Object(Values::new()),
            errors,
        }
    }
}

async fn perform(service: &Service, run: &ClaimedRun, prepared: Prepared) -> Finished {
    let cancel = CancellationToken::new();
    let (events, receiver) = mpsc::channel(EVENT_BUFFER);
    let writer = tokio::spawn(write_events(
        service.clone(),
        run.id,
        receiver,
        cancel.clone(),
    ));
    let watching = CancellationToken::new();
    let watcher = tokio::spawn(watch_claim(
        service.clone(),
        run.id,
        cancel.clone(),
        watching.clone(),
    ));
    let control = RunControl {
        cancel,
        events: Some(events),
        secrets: Arc::new(prepared.secrets),
    };
    let report = service
        .inner
        .engine
        .run(&prepared.plan, prepared.inputs, control)
        .await;
    watching.cancel();
    let _ = watcher.await;
    let _ = writer.await;
    Finished {
        status: match report.status {
            RunStatus::Succeeded => RunState::Succeeded,
            RunStatus::Failed => RunState::Failed,
            RunStatus::Cancelled => RunState::Cancelled,
        },
        outputs: Value::Object(report.outputs),
        errors: report.errors,
    }
}

async fn watch_claim(
    service: Service,
    run: uuid::Uuid,
    cancel: CancellationToken,
    stop: CancellationToken,
) {
    let mut notifications = service.inner.hub.subscribe();
    let mut listening = true;
    let mut ticker = tokio::time::interval(service.inner.options.heartbeat);
    ticker.tick().await;
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            _ = ticker.tick() => {}
            received = notifications.recv(), if listening => match received {
                Ok(Notification::CancelRequested(id)) if id == run => {}
                Ok(Notification::Resync) | Err(RecvError::Lagged(_)) => {}
                Ok(_) => continue,
                Err(RecvError::Closed) => {
                    listening = false;
                    continue;
                }
            },
        }
        match service.store().heartbeat(run, service.inner.worker).await {
            Ok(Some(false)) => {}
            Ok(Some(true) | None) => {
                cancel.cancel();
                return;
            }
            Err(error) => tracing::warn!(%run, %error, "cannot send the run heartbeat"),
        }
    }
}

async fn write_events(
    service: Service,
    run: uuid::Uuid,
    mut events: mpsc::Receiver<Event>,
    cancel: CancellationToken,
) {
    let mut lost = false;
    while let Some(first) = events.recv().await {
        let mut steps = Vec::new();
        let mut logs = Vec::new();
        absorb(first, &mut steps, &mut logs);
        while steps.len() + logs.len() < MAX_BATCH {
            match events.try_recv() {
                Ok(event) => absorb(event, &mut steps, &mut logs),
                Err(_) => break,
            }
        }
        if lost {
            continue;
        }
        match service
            .store()
            .write_events(run, service.inner.worker, steps, logs)
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                lost = true;
                cancel.cancel();
            }
            Err(error) => report_write_error(run, &error),
        }
    }
}

fn report_write_error(run: uuid::Uuid, error: &StoreError) {
    tracing::error!(%run, %error, "cannot record run events");
}

fn absorb(event: Event, steps: &mut Vec<StepWrite>, logs: &mut Vec<LogWrite>) {
    match event {
        Event::StepStarted { path, attempt } => steps.push(StepWrite {
            path,
            status: StepState::Running,
            attempt: count(attempt),
            message: None,
            outputs: Value::Object(Values::new()),
            duration_ms: None,
        }),
        Event::StepRetrying {
            path,
            attempt,
            error,
            ..
        } => steps.push(StepWrite {
            path,
            status: StepState::Retrying,
            attempt: count(attempt),
            message: Some(clip(error)),
            outputs: Value::Object(Values::new()),
            duration_ms: None,
        }),
        Event::StepFinished(report) => steps.push(StepWrite {
            path: report.path,
            status: match report.status {
                StepStatus::Succeeded => StepState::Succeeded,
                StepStatus::Failed => StepState::Failed,
                StepStatus::Skipped => StepState::Skipped,
                StepStatus::Cancelled => StepState::Cancelled,
            },
            attempt: count(report.attempts),
            message: report.message.map(clip),
            outputs: Value::Object(report.outputs),
            duration_ms: Some(i64::try_from(report.duration.as_millis()).unwrap_or(i64::MAX)),
        }),
        Event::Log { path, message } => logs.push(LogWrite {
            path,
            message: clip(message),
        }),
    }
}

fn count(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

fn clip(mut text: String) -> String {
    if text.len() > MAX_LOG_BYTES {
        let mut end = MAX_LOG_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push('…');
    }
    text
}
