use dioxus::prelude::*;
use parameters_api::{Source, Uuid, Via, WorkflowSummary};
use parameters_core::to_json;

use crate::Route;
use crate::api::{create_workflow, delete_workflow, get_workflow, list_workflows};
use crate::ui::common::{
    Loading, Notice, NoticeBar, Problems, failure_problems, on_client, timestamp,
};

const NEW_WORKFLOW: &str = "workflow \"Untitled\"\n";

pub(crate) fn source_of(text: &str) -> Source {
    if text.trim_start().starts_with('{') {
        Source::Json(text.to_owned())
    } else {
        Source::Text(text.to_owned())
    }
}

#[component]
pub(crate) fn Workflows() -> Element {
    let mut workflows = use_resource(|| async {
        if on_client() {
            Some(list_workflows(Some(200), None).await)
        } else {
            None
        }
    });
    let mut notice = use_signal(|| None::<Notice>);
    let mut importing = use_signal(|| false);
    let mut import_text = use_signal(String::new);
    let mut import_problems = use_signal(Vec::new);
    let navigator = use_navigator();

    let open = move |source: Source| async move {
        match create_workflow(source).await {
            Ok(saved) => {
                navigator.push(Route::Editor {
                    id: saved.record.summary.id,
                });
            }
            Err(failure) => {
                import_problems.set(failure_problems(&failure));
                notice.set(Some(Notice::from(failure)));
            }
        }
    };

    let listing = match &*workflows.read() {
        Some(Some(Ok(page))) => rsx! {
            if page.items.is_empty() {
                p { class: "empty", "No workflows yet. Create one, or import a workflow someone shared with you." }
            } else {
                table { class: "list",
                    thead {
                        tr {
                            th { "Name" }
                            th { "Version" }
                            th { "Last change" }
                            th { span { class: "sr-only", "Actions" } }
                        }
                    }
                    tbody {
                        for workflow in page.items.clone() {
                            WorkflowRow {
                                key: "{workflow.id}",
                                workflow,
                                on_change: move |message: Notice| {
                                    notice.set(Some(message));
                                    workflows.restart();
                                },
                            }
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
        div { class: "heading",
            h1 { "Workflows" }
            div { class: "actions",
                button {
                    class: "primary",
                    onclick: move |_| open(Source::Text(NEW_WORKFLOW.into())),
                    "New workflow"
                }
                button { onclick: move |_| importing.toggle(), "Import" }
            }
        }
        NoticeBar { notice }
        if importing() {
            section { class: "panel",
                h2 { "Import a workflow" }
                p { class: "muted",
                    "Paste a workflow in the Parameters language or as JSON, or choose a file. Custom nodes it defines come with it; review what it can do before you run it."
                }
                input {
                    r#type: "file",
                    accept: ".json,.parameters,.txt",
                    onchange: move |event| async move {
                        if let Some(file) = event.files().into_iter().next() {
                            match file.read_string().await {
                                Ok(text) => import_text.set(text),
                                Err(error) => notice.set(Some(Notice::Error(error.to_string()))),
                            }
                        }
                    },
                }
                textarea {
                    class: "source",
                    rows: "12",
                    spellcheck: "false",
                    placeholder: "workflow \"Name\"\n\nstep …",
                    value: "{import_text}",
                    oninput: move |event| import_text.set(event.value()),
                }
                Problems { problems: import_problems() }
                div { class: "actions",
                    button {
                        class: "primary",
                        disabled: import_text.read().trim().is_empty(),
                        onclick: move |_| open(source_of(&import_text.read())),
                        "Import"
                    }
                    button { class: "quiet", onclick: move |_| importing.set(false), "Cancel" }
                }
            }
        }
        {listing}
    }
}

#[component]
fn WorkflowRow(workflow: WorkflowSummary, on_change: EventHandler<Notice>) -> Element {
    let mut confirming = use_signal(|| false);
    let id = workflow.id;
    let name = workflow.name.clone();
    let duplicate = move |_| async move {
        match duplicate_workflow(id).await {
            Ok(()) => on_change.call(Notice::Info("Duplicated the workflow".into())),
            Err(notice) => on_change.call(notice),
        }
    };
    let remove = move |_| {
        let name = name.clone();
        async move {
            if !confirming() {
                confirming.set(true);
                return;
            }
            match delete_workflow(id).await {
                Ok(()) => on_change.call(Notice::Info(format!("Deleted {name}"))),
                Err(failure) => on_change.call(Notice::from(failure)),
            }
        }
    };
    rsx! {
        tr {
            td {
                Link { to: Route::Editor { id }, "{workflow.name}" }
                if workflow.updated_via == Via::Mcp {
                    span { class: "badge agent", title: "Last changed by an AI agent", "agent" }
                }
            }
            td { "{workflow.version}" }
            td { class: "muted", "{timestamp(workflow.updated_at)}" }
            td { class: "row-actions",
                button { class: "quiet", onclick: duplicate, "Duplicate" }
                button {
                    class: if confirming() { "danger" } else { "quiet" },
                    onclick: remove,
                    onblur: move |_| confirming.set(false),
                    if confirming() { "Confirm delete" } else { "Delete" }
                }
            }
        }
    }
}

async fn duplicate_workflow(id: Uuid) -> Result<(), Notice> {
    let mut document = get_workflow(id).await.map_err(Notice::from)?.document;
    document.name = format!("{} copy", document.name);
    create_workflow(Source::Json(to_json(&document)))
        .await
        .map(drop)
        .map_err(Notice::from)
}
