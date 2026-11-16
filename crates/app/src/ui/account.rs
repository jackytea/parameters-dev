use std::fmt::Write;

use dioxus::prelude::*;
use parameters_api::{CreatedToken, Me, Mode, Scope, Scopes};

use crate::Route;
use crate::api::{create_token, list_tokens, request_sign_in, revoke_token};
use crate::ui::common::{Loading, Notice, NoticeBar, on_client, timestamp};

fn provider_label(name: &str) -> &'static str {
    match name {
        "google" => "Continue with Google",
        "github" => "Continue with GitHub",
        "discord" => "Continue with Discord",
        _ => "Continue",
    }
}

fn encode(text: &str) -> String {
    text.bytes().fold(String::new(), |mut out, byte| {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
        out
    })
}

#[component]
pub(crate) fn SignIn() -> Element {
    let me = use_context::<Signal<Option<Me>>>();
    let route = use_route::<Route>();
    let next = if matches!(route, Route::SignIn {}) {
        "/".to_owned()
    } else {
        route.to_string()
    };
    let next_query = encode(&next);
    let mut email = use_signal(String::new);
    let mut sent = use_signal(|| false);
    let mut notice = use_signal(|| None::<Notice>);
    let current = me.read().clone();
    if current.as_ref().is_some_and(|me| me.mode == Mode::Local) {
        return rsx! {
            section { class: "empty",
                h1 { "No sign-in needed" }
                p { "This Parameters server runs on your machine and has no accounts." }
                Link { to: Route::Workflows {}, "Go to your workflows" }
            }
        };
    }
    let providers = current.map(|me| me.providers).unwrap_or_default();
    let send = move |event: FormEvent| {
        let next = next.clone();
        async move {
            event.prevent_default();
            let address = email.read().clone();
            match request_sign_in(address, Some(next)).await {
                Ok(()) => sent.set(true),
                Err(failure) => notice.set(Some(Notice::from(failure))),
            }
        }
    };
    rsx! {
        section { class: "signin",
            h1 { "Sign in to Parameters" }
            NoticeBar { notice }
            if sent() {
                p { class: "notice", role: "status",
                    "Check {email} for a sign-in link. It works once and expires in 15 minutes."
                }
            } else {
                form { class: "panel", onsubmit: send,
                    label {
                        "Email"
                        input {
                            r#type: "email",
                            required: true,
                            autocomplete: "email",
                            value: "{email}",
                            oninput: move |event| email.set(event.value()),
                        }
                    }
                    div { class: "actions",
                        button { class: "primary", r#type: "submit", "Email me a sign-in link" }
                    }
                }
            }
            if !providers.is_empty() {
                div { class: "providers",
                    for provider in providers {
                        a {
                            key: "{provider}",
                            class: "button",
                            href: "/auth/{provider}?next={next_query}",
                            "{provider_label(&provider)}"
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub(crate) fn Account() -> Element {
    let me = use_context::<Signal<Option<Me>>>();
    let mut tokens = use_resource(|| async {
        if on_client() {
            Some(list_tokens().await)
        } else {
            None
        }
    });
    let mut notice = use_signal(|| None::<Notice>);
    let mut name = use_signal(String::new);
    let mut scopes = use_signal(|| vec![Scope::Read]);
    let mut days = use_signal(|| "90".to_owned());
    let mut created = use_signal(|| None::<CreatedToken>);
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
    let current = me.read().clone();
    if current.as_ref().is_some_and(|me| me.mode == Mode::Local) {
        return rsx! {
            div { class: "heading", h1 { "Account" } }
            p { "This server runs on your machine and has no accounts. AI agents connect to " code { "{origin}/mcp" } " without a token." }
        };
    }
    let create = move |event: FormEvent| async move {
        event.prevent_default();
        let chosen: Scopes = scopes.read().iter().copied().collect();
        let expires = days.read().parse::<u32>().ok();
        let label = name.read().clone();
        match create_token(label, chosen, expires).await {
            Ok(token) => {
                created.set(Some(token));
                name.set(String::new());
                tokens.restart();
            }
            Err(failure) => notice.set(Some(Notice::from(failure))),
        }
    };
    let listing = match &*tokens.read() {
        Some(Some(Ok(list))) if list.is_empty() => rsx! {
            p { class: "empty", "No access tokens." }
        },
        Some(Some(Ok(list))) => rsx! {
            table { class: "list",
                thead {
                    tr {
                        th { "Name" }
                        th { "Can" }
                        th { "Expires" }
                        th { "Last used" }
                        th { span { class: "sr-only", "Actions" } }
                    }
                }
                tbody {
                    for token in list.clone() {
                        tr { key: "{token.id}",
                            td { "{token.name}" }
                            td { "{token.scopes.list().iter().map(|scope| scope.name()).collect::<Vec<_>>().join(\", \")}" }
                            td { class: "muted", "{token.expires_at.map(timestamp).unwrap_or_else(|| \"never\".into())}" }
                            td { class: "muted", "{token.last_used_at.map(timestamp).unwrap_or_else(|| \"never\".into())}" }
                            td { class: "row-actions",
                                button {
                                    class: "quiet",
                                    onclick: move |_| async move {
                                        match revoke_token(token.id).await {
                                            Ok(()) => tokens.restart(),
                                            Err(failure) => notice.set(Some(Notice::from(failure))),
                                        }
                                    },
                                    "Revoke"
                                }
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
    let who = current
        .and_then(|me| me.user)
        .map(|user| user.email.unwrap_or(user.display_name));
    rsx! {
        div { class: "heading",
            h1 { "Account" }
            if let Some(who) = who {
                span { class: "muted", "{who}" }
            }
        }
        NoticeBar { notice }
        h2 { "Access tokens" }
        p { class: "muted", "Give AI agents and scripts a token instead of your password. Each token can only do what you allow, and you can revoke it at any time." }
        if let Some(token) = created() {
            section { class: "panel review attention",
                p { "Copy this token now; it is not shown again." }
                pre { class: "output", "{token.token}" }
                p { "To connect Claude Code:" }
                pre { class: "output",
                    "claude mcp add --transport http parameters {origin}/mcp --header \"Authorization: Bearer {token.token}\""
                }
                button { class: "quiet", onclick: move |_| created.set(None), "Done" }
            }
        }
        form { class: "panel inline-form", onsubmit: create,
            label {
                "Name"
                input {
                    required: true,
                    maxlength: "100",
                    placeholder: "Claude on my laptop",
                    value: "{name}",
                    oninput: move |event| name.set(event.value()),
                }
            }
            fieldset { class: "chips",
                legend { "Can" }
                for scope in Scope::ALL {
                    label { key: "{scope.name()}", class: "chip",
                        input {
                            r#type: "checkbox",
                            checked: scopes.read().contains(&scope),
                            onchange: move |event| {
                                let mut chosen = scopes.write();
                                chosen.retain(|existing| *existing != scope);
                                if event.checked() {
                                    chosen.push(scope);
                                }
                            },
                        }
                        "{scope.name()}"
                    }
                }
            }
            label {
                "Expires"
                select { value: "{days}", onchange: move |event| days.set(event.value()),
                    option { value: "30", "in 30 days" }
                    option { value: "90", "in 90 days" }
                    option { value: "365", "in a year" }
                    option { value: "never", "never" }
                }
            }
            button { class: "primary", r#type: "submit", "Create token" }
        }
        {listing}
    }
}
