use std::collections::BTreeMap;

use dioxus::prelude::*;
use parameters_api::{ImportItem, ImportStatus, NodeRecord, Resolution, Source};
use parameters_core::{Definition, OrderedMap, Workflow, from_json};

use crate::Route;
use crate::api::{
    delete_node, get_node, import_nodes, list_nodes, node_users, plan_node_import, rename_node,
};
use crate::ui::common::{Loading, Notice, NoticeBar, on_client, timestamp};
use crate::ui::workflows::source_of;

#[component]
pub(crate) fn Nodes() -> Element {
    let mut nodes = use_resource(|| async {
        if on_client() {
            Some(list_nodes().await)
        } else {
            None
        }
    });
    let mut notice = use_signal(|| None::<Notice>);
    let mut selected = use_signal(|| None::<String>);
    let mut importing = use_signal(|| false);
    let mut refresh = move |message: Notice| {
        notice.set(Some(message));
        nodes.restart();
    };

    let listing = match &*nodes.read() {
        Some(Some(Ok(list))) if list.is_empty() => rsx! {
            p { class: "empty",
                "Your library has no custom nodes yet. Import them from a workflow or a shared file; a custom node is a small workflow with inputs and outputs that other workflows can use as a step."
            }
        },
        Some(Some(Ok(list))) => rsx! {
            table { class: "list",
                thead {
                    tr {
                        th { "Node" }
                        th { "Description" }
                        th { "Version" }
                        th { "Last change" }
                    }
                }
                tbody {
                    for node in list.clone() {
                        tr {
                            key: "{node.type_name}",
                            class: if selected.read().as_deref() == Some(node.type_name.as_str()) { "selected" },
                            td {
                                button {
                                    class: "link",
                                    onclick: move |_| selected.set(Some(node.type_name.clone())),
                                    code { "{node.type_name}" }
                                }
                            }
                            td { class: "muted", "{node.note.clone().unwrap_or_default()}" }
                            td { "{node.version}" }
                            td { class: "muted", "{timestamp(node.updated_at)}" }
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
            h1 { "Node library" }
            div { class: "actions",
                button { onclick: move |_| importing.toggle(), "Import nodes" }
            }
        }
        NoticeBar { notice }
        if importing() {
            ImportNodes {
                on_done: move |message: Notice| {
                    importing.set(false);
                    refresh(message);
                },
            }
        }
        {listing}
        if let Some(type_name) = selected() {
            NodeDetail {
                key: "{type_name}",
                type_name,
                on_change: move |(message, next): (Notice, Option<String>)| {
                    selected.set(next);
                    refresh(message);
                },
            }
        }
    }
}

#[component]
fn NodeDetail(type_name: String, on_change: EventHandler<(Notice, Option<String>)>) -> Element {
    let name = type_name.clone();
    let record = use_resource(move || {
        let name = name.clone();
        async move { get_node(name).await }
    });
    let name = type_name.clone();
    let users = use_resource(move || {
        let name = name.clone();
        async move { node_users(name).await }
    });
    let mut new_name = use_signal(|| type_name.clone());
    let Some(Ok(record)) = record.read().clone() else {
        return rsx! {
            Loading {}
        };
    };
    let text = printed(&record);
    let version = record.summary.version;
    let current = type_name.clone();
    let rename = move |_| {
        let current = current.clone();
        async move {
            let to = new_name.read().trim().to_owned();
            match rename_node(current, to.clone()).await {
                Ok(_) => {
                    on_change.call((Notice::Info(format!("Renamed the node to {to}")), Some(to)));
                }
                Err(failure) => on_change.call((Notice::from(failure), None)),
            }
        }
    };
    let current = type_name.clone();
    let remove = move |_| {
        let current = current.clone();
        async move {
            match delete_node(current.clone(), version).await {
                Ok(()) => on_change.call((Notice::Info(format!("Deleted {current}")), None)),
                Err(failure) => on_change.call((Notice::from(failure), None)),
            }
        }
    };
    rsx! {
        section { class: "panel",
            h2 { code { "{type_name}" } }
            pre { class: "source", "{text}" }
            div { class: "inline-form",
                label {
                    "Rename to"
                    input { value: "{new_name}", oninput: move |event| new_name.set(event.value()) }
                }
                button { onclick: rename, "Rename" }
                button { class: "danger", onclick: remove, "Delete" }
            }
            h3 { "Workflows that include it" }
            match &*users.read() {
                Some(Ok(list)) if list.is_empty() => rsx! { p { class: "muted", "None of your workflows include this node." } },
                Some(Ok(list)) => rsx! {
                    ul {
                        for workflow in list.clone() {
                            li { key: "{workflow.id}", Link { to: Route::Editor { id: workflow.id }, "{workflow.name}" } }
                        }
                    }
                },
                Some(Err(failure)) => rsx! { p { class: "notice error", "{failure}" } },
                None => rsx! { Loading {} },
            }
            p { class: "muted",
                "Each workflow keeps its own copy, so renaming or changing the library node does not change workflows that already include it."
            }
        }
    }
}

fn printed(record: &NodeRecord) -> String {
    let mut workflow = Workflow::new(record.summary.type_name.clone());
    workflow
        .definitions
        .insert(record.summary.type_name.clone(), record.definition.clone());
    let text = parameters_lang::print(&workflow);
    text.split_once("\n\n")
        .map_or(text.clone(), |(_, body)| body.to_owned())
}

fn definitions_of(text: &str) -> Result<OrderedMap<Definition>, String> {
    match source_of(text) {
        Source::Text(text) => {
            let parsed = parameters_lang::parse(&text);
            match parsed.diagnostics.iter().find(|problem| problem.is_error()) {
                Some(problem) => Err(problem.to_string()),
                None => Ok(parsed.workflow.definitions),
            }
        }
        Source::Json(text) => from_json(&text)
            .map(|workflow| workflow.definitions)
            .map_err(|error| error.to_string()),
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Choice {
    Rename(String),
    UseExisting,
    Replace,
}

#[component]
fn ImportNodes(on_done: EventHandler<Notice>) -> Element {
    let mut text = use_signal(String::new);
    let mut plan = use_signal(|| None::<(OrderedMap<Definition>, Vec<ImportItem>)>);
    let mut choices = use_signal(BTreeMap::<String, Choice>::new);
    let mut notice = use_signal(|| None::<Notice>);

    let preview = move |_| async move {
        let definitions = match definitions_of(&text.read()) {
            Ok(definitions) if definitions.is_empty() => {
                notice.set(Some(Notice::Error(
                    "There are no `define node` blocks to import.".into(),
                )));
                return;
            }
            Ok(definitions) => definitions,
            Err(message) => {
                notice.set(Some(Notice::Error(message)));
                return;
            }
        };
        match plan_node_import(definitions.clone()).await {
            Ok(items) => {
                choices.set(
                    items
                        .iter()
                        .filter_map(|item| match &item.status {
                            ImportStatus::Conflict { suggested }
                            | ImportStatus::BuiltinClash { suggested } => {
                                Some((item.name.clone(), Choice::Rename(suggested.clone())))
                            }
                            _ => None,
                        })
                        .collect(),
                );
                plan.set(Some((definitions, items)));
            }
            Err(failure) => notice.set(Some(Notice::from(failure))),
        }
    };

    let confirm = move |_| async move {
        let Some((definitions, _)) = plan.read().clone() else {
            return;
        };
        let resolutions = choices
            .read()
            .iter()
            .map(|(name, choice)| {
                let resolution = match choice {
                    Choice::Rename(to) => Resolution::Rename(to.clone()),
                    Choice::UseExisting => Resolution::UseExisting,
                    Choice::Replace => Resolution::Replace,
                };
                (name.clone(), resolution)
            })
            .collect();
        match import_nodes(definitions, resolutions).await {
            Ok(items) => on_done.call(Notice::Info(format!("Imported {} nodes", items.len()))),
            Err(failure) => notice.set(Some(Notice::from(failure))),
        }
    };

    rsx! {
        section { class: "panel",
            h2 { "Import nodes" }
            p { class: "muted", "Paste a workflow or file that contains `define node` blocks, in the Parameters language or as JSON." }
            NoticeBar { notice }
            textarea {
                class: "source",
                rows: "10",
                spellcheck: "false",
                value: "{text}",
                oninput: move |event| {
                    text.set(event.value());
                    plan.set(None);
                },
            }
            if let Some((_, items)) = plan() {
                table { class: "list",
                    thead {
                        tr {
                            th { "Node" }
                            th { "Status" }
                            th { "Import as" }
                        }
                    }
                    tbody {
                        for item in items {
                            ImportRow { key: "{item.name}", item, choices }
                        }
                    }
                }
                div { class: "actions",
                    button { class: "primary", onclick: confirm, "Import" }
                }
            } else {
                div { class: "actions",
                    button {
                        class: "primary",
                        disabled: text.read().trim().is_empty(),
                        onclick: preview,
                        "Review import"
                    }
                }
            }
        }
    }
}

fn choose(choices: Signal<BTreeMap<String, Choice>>, name: String, choice: Choice) {
    let mut choices = choices;
    choices.write().insert(name, choice);
}

#[component]
fn ImportRow(item: ImportItem, choices: Signal<BTreeMap<String, Choice>>) -> Element {
    let name = item.name.clone();
    let choice = choices.read().get(&name).cloned();
    let (status, can_keep) = match &item.status {
        ImportStatus::New => ("new", false),
        ImportStatus::Identical => ("already in your library", false),
        ImportStatus::Conflict { .. } => ("a different node has this name", true),
        ImportStatus::BuiltinClash { .. } => ("clashes with a built-in node", false),
    };
    let renamed = format!("{name}_2");
    let editor = match choice {
        None => rsx! {
            span { class: "muted", "{name}" }
        },
        Some(Choice::Rename(to)) => {
            let typed = name.clone();
            let keep = name.clone();
            let replace = name.clone();
            rsx! {
                div { class: "inline-form",
                    input {
                        "aria-label": "New name for {name}",
                        value: "{to}",
                        oninput: move |event| choose(choices, typed.clone(), Choice::Rename(event.value())),
                    }
                    if can_keep {
                        button {
                            class: "quiet",
                            onclick: move |_| choose(choices, keep.clone(), Choice::UseExisting),
                            "Use mine"
                        }
                        button {
                            class: "quiet",
                            onclick: move |_| choose(choices, replace.clone(), Choice::Replace),
                            "Replace mine"
                        }
                    }
                }
            }
        }
        Some(kept) => {
            let label = if kept == Choice::Replace {
                "Replace the node in your library"
            } else {
                "Keep the node in your library"
            };
            let back = name.clone();
            rsx! {
                span { "{label} " }
                button {
                    class: "quiet",
                    onclick: move |_| choose(choices, back.clone(), Choice::Rename(renamed.clone())),
                    "Rename instead"
                }
            }
        }
    };
    rsx! {
        tr {
            td { code { "{item.name}" } }
            td { "{status}" }
            td { {editor} }
        }
    }
}
