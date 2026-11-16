use std::collections::BTreeMap;

use dioxus::prelude::*;
use parameters_api::{LogLine, RunRecord, StepRecord, Uuid};

use crate::Route;
use crate::api::{cancel_run, list_runs, watch_run};
use crate::ui::common::{
    Loading, Notice, NoticeBar, RunBadge, StepBadge, on_client, pause, timestamp,
};

#[component]
pub(crate) fn Runs() -> Element {
    let runs = use_resource(|| async {
        if on_client() {
            Some(list_runs(None, Some(200), None).await)
        } else {
            None
        }
    });
    let listing = match &*runs.read() {
        Some(Some(Ok(page))) if page.items.is_empty() => rsx! {
            p { class: "empty", "No runs yet. Open a workflow and run it." }
        },
        Some(Some(Ok(page))) => rsx! {
            table { class: "list",
                thead {
                    tr {
                        th { "Workflow" }
                        th { "Status" }
                        th { "Started by" }
                        th { "Queued" }
                    }
                }
                tbody {
                    for run in page.items.clone() {
                        tr { key: "{run.id}",
                            td { Link { to: Route::RunPage { id: run.id }, "{run.workflow_name}" } }
                            td { RunBadge { status: run.status } }
                            td { class: "muted", "{run.trigger.name()}" }
                            td { class: "muted", "{timestamp(run.created_at)}" }
                        }
                    }
                }
            }
        },
        Some(Some(Err(failure))) => rsx! {
            p { class: "notice error", "{failure}" }
        },
        _ => rsx! {
            Loading {}
        },
    };
    rsx! {
        div { class: "heading", h1 { "Runs" } }
        {listing}
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct LiveRun {
    pub(crate) run: Option<RunRecord>,
    pub(crate) steps: BTreeMap<String, StepRecord>,
    pub(crate) logs: Vec<LogLine>,
}

pub(crate) async fn follow_run(id: Uuid, mut live: Signal<LiveRun>) {
    if !on_client() {
        return;
    }
    loop {
        if let Ok(mut events) = watch_run(id).await {
            while let Some(Ok(update)) = events.recv().await {
                let finished = update.run.summary.status.is_finished();
                let mut current = live.write();
                for step in update.steps {
                    current.steps.insert(step.path.clone(), step);
                }
                let last = current.logs.last().map_or(0, |line| line.seq);
                current
                    .logs
                    .extend(update.logs.into_iter().filter(|line| line.seq > last));
                current.run = Some(update.run);
                if finished {
                    return;
                }
            }
        }
        if live
            .peek()
            .run
            .as_ref()
            .is_some_and(|run| run.summary.status.is_finished())
        {
            return;
        }
        pause(2_000).await;
    }
}

#[component]
pub(crate) fn RunPage(id: Uuid) -> Element {
    let live = use_signal(LiveRun::default);
    let mut notice = use_signal(|| None::<Notice>);
    use_future(move || follow_run(id, live));
    let current = live.read();
    let Some(run) = current.run.clone() else {
        return rsx! {
            Loading {}
        };
    };
    let summary = &run.summary;
    let outputs = serde_json::to_string_pretty(&run.outputs).unwrap_or_default();
    rsx! {
        div { class: "heading",
            h1 { "{summary.workflow_name}" }
            RunBadge { status: summary.status }
            div { class: "actions",
                if let Some(workflow) = summary.workflow_id {
                    Link { class: "button", to: Route::Editor { id: workflow }, "Open workflow" }
                }
                if !summary.status.is_finished() {
                    button {
                        class: "danger",
                        onclick: move |_| async move {
                            if let Err(failure) = cancel_run(id).await {
                                notice.set(Some(Notice::from(failure)));
                            }
                        },
                        "Cancel run"
                    }
                }
            }
        }
        NoticeBar { notice }
        p { class: "muted",
            "Started by {summary.trigger.name()} · queued {timestamp(summary.created_at)} · attempt {summary.attempts}"
        }
        for error in run.errors.clone() {
            p { class: "notice error", "{error}" }
        }
        RunDetails { live: current.clone() }
        if !run.outputs.is_empty() {
            h2 { "Outputs" }
            pre { class: "output", "{outputs}" }
        }
    }
}

#[component]
pub(crate) fn RunDetails(live: LiveRun) -> Element {
    rsx! {
        h2 { "Steps" }
        if live.steps.is_empty() {
            p { class: "muted", "No step has started yet." }
        } else {
            table { class: "list",
                thead {
                    tr {
                        th { "Step" }
                        th { "Status" }
                        th { "Attempt" }
                        th { "Time" }
                        th { "Details" }
                    }
                }
                tbody {
                    for step in live.steps.values().cloned() {
                        tr { key: "{step.path}",
                            td { code { "{step.path}" } }
                            td { StepBadge { status: step.status } }
                            td { "{step.attempt}" }
                            td { class: "muted",
                                if let Some(duration) = step.duration_ms { "{duration} ms" }
                            }
                            td {
                                if let Some(message) = &step.message {
                                    p { "{message}" }
                                }
                                if !step.outputs.is_empty() {
                                    details {
                                        summary { "Outputs" }
                                        pre { class: "output",
                                            "{serde_json::to_string_pretty(&step.outputs).unwrap_or_default()}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        h2 { "Log" }
        if live.logs.is_empty() {
            p { class: "muted", "Nothing logged yet." }
        } else {
            pre { class: "log",
                for line in live.logs {
                    span { key: "{line.seq}",
                        span { class: "muted", "{line.path} " }
                        "{line.message}\n"
                    }
                }
            }
        }
    }
}
