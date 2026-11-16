use dioxus::prelude::*;
use parameters_api::{CreatedTrigger, NewTrigger, Schedule, TriggerKind, TriggerSummary};
use serde_json::Map;

use crate::api::{create_trigger, delete_trigger, list_triggers, set_trigger_enabled};
use crate::ui::common::{Notice, on_client, timestamp};
use crate::ui::editor::EditorState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Webhook,
    Schedule,
}

fn describe(trigger: &TriggerSummary) -> String {
    match &trigger.kind {
        TriggerKind::Webhook { signed: true } => "Webhook, signed".into(),
        TriggerKind::Webhook { signed: false } => "Webhook".into(),
        TriggerKind::Schedule { cron, next_run_at } => match next_run_at {
            Some(next) if trigger.enabled => {
                format!("Schedule `{cron}`, next {}", timestamp(*next))
            }
            _ => format!("Schedule `{cron}`"),
        },
    }
}

#[component]
pub(crate) fn TriggersPanel() -> Element {
    let mut state = use_context::<EditorState>();
    let id = state.id;
    let mut triggers = use_resource(move || async move {
        if on_client() {
            Some(list_triggers(Some(id)).await)
        } else {
            None
        }
    });
    let mut created = use_signal(|| None::<CreatedTrigger>);
    let mut name = use_signal(String::new);
    let mut kind = use_signal(|| Kind::Webhook);
    let mut cron = use_signal(|| "0 9 * * 1-5".to_owned());
    let mut signed = use_signal(|| true);
    let mut origin = use_signal(String::new);
    use_effect(move || {
        if on_client() {
            spawn(async move {
                if let Ok(location) = document::eval("return location.origin;")
                    .join::<String>()
                    .await
                {
                    origin.set(location);
                }
            });
        }
    });
    let cron_problem = (kind() == Kind::Schedule)
        .then(|| Schedule::parse(&cron.read()).err())
        .flatten();
    let cron_invalid = cron_problem.is_some();

    let add = move |event: FormEvent| async move {
        event.prevent_default();
        let new = match kind() {
            Kind::Webhook => NewTrigger::Webhook { signed: signed() },
            Kind::Schedule => NewTrigger::Schedule {
                cron: cron.read().clone(),
            },
        };
        let label = name.read().clone();
        match create_trigger(id, label, new, Map::new()).await {
            Ok(trigger) => {
                created.set(Some(trigger));
                name.set(String::new());
                triggers.restart();
            }
            Err(failure) => state.notice.set(Some(Notice::from(failure))),
        }
    };

    let listing = match &*triggers.read() {
        Some(Some(Ok(list))) if list.is_empty() => rsx! {
            p { class: "muted", "Nothing starts this workflow automatically yet." }
        },
        Some(Some(Ok(list))) => rsx! {
            ul { class: "triggers",
                for trigger in list.clone() {
                    li { key: "{trigger.id}",
                        div {
                            strong { "{trigger.name}" }
                            " "
                            span { class: "muted", "{describe(&trigger)}" }
                            if !trigger.enabled {
                                span { class: "badge", "off" }
                            }
                        }
                        if let Some(at) = trigger.last_fired_at {
                            div { class: "muted", "Last started {timestamp(at)}" }
                        }
                        if let Some(error) = &trigger.last_error {
                            div { class: "notice error", "Last attempt failed: {error}" }
                        }
                        div { class: "row-actions",
                            button {
                                class: "quiet",
                                onclick: move |_| async move {
                                    match set_trigger_enabled(trigger.id, !trigger.enabled).await {
                                        Ok(()) => triggers.restart(),
                                        Err(failure) => state.notice.set(Some(Notice::from(failure))),
                                    }
                                },
                                if trigger.enabled { "Turn off" } else { "Turn on" }
                            }
                            button {
                                class: "quiet",
                                onclick: move |_| async move {
                                    match delete_trigger(trigger.id).await {
                                        Ok(()) => triggers.restart(),
                                        Err(failure) => state.notice.set(Some(Notice::from(failure))),
                                    }
                                },
                                "Delete"
                            }
                        }
                    }
                }
            }
        },
        Some(Some(Err(failure))) => rsx! {
            p { class: "notice error", "{failure}" }
        },
        _ => rsx! {},
    };

    rsx! {
        section { class: "panel triggers-panel",
            h2 { "Triggers" }
            {listing}
            if let Some(trigger) = created() {
                if let Some(path) = &trigger.hook_path {
                    div { class: "panel review attention",
                        p { "Send a POST request to this address to start the workflow. Copy it now; it is not shown again." }
                        pre { class: "output", "{origin}{path}" }
                        if let Some(secret) = &trigger.signing_secret {
                            p { "Sign each request body with HMAC-SHA256 and this secret, and send " code { "X-Parameters-Signature: sha256=<hex>" } ":" }
                            pre { class: "output", "{secret}" }
                        } else {
                            p { "Try it:" }
                            pre { class: "output",
                                "curl -X POST {origin}{path} -H 'content-type: application/json' -d '{{\"event\": \"test\"}}'"
                            }
                        }
                        p { class: "muted", "The request body is passed to the workflow as its `payload` input, if it has one." }
                        button { class: "quiet", onclick: move |_| created.set(None), "Done" }
                    }
                }
            }
            form { class: "inline-form", onsubmit: add,
                label {
                    "Name"
                    input {
                        required: true,
                        maxlength: "100",
                        placeholder: "Deploys from GitHub",
                        value: "{name}",
                        oninput: move |event| name.set(event.value()),
                    }
                }
                label {
                    "Start on"
                    select {
                        onchange: move |event| {
                            kind.set(if event.value() == "schedule" { Kind::Schedule } else { Kind::Webhook });
                        },
                        option { value: "webhook", selected: kind() == Kind::Webhook, "a webhook" }
                        option { value: "schedule", selected: kind() == Kind::Schedule, "a schedule" }
                    }
                }
                if kind() == Kind::Schedule {
                    label {
                        "Cron, in UTC"
                        input {
                            class: "mono",
                            value: "{cron}",
                            oninput: move |event| cron.set(event.value()),
                        }
                    }
                } else {
                    label { class: "chip",
                        input {
                            r#type: "checkbox",
                            checked: signed(),
                            onchange: move |event| signed.set(event.checked()),
                        }
                        "Require a signature"
                    }
                }
                button { class: "primary", r#type: "submit", disabled: cron_invalid, "Add trigger" }
            }
            if let Some(problem) = cron_problem {
                p { class: "muted", "{problem}" }
            }
        }
    }
}
