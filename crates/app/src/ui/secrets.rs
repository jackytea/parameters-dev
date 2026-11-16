use dioxus::prelude::*;

use crate::api::{delete_secret, list_secrets, set_secret};
use crate::ui::common::{Loading, Notice, NoticeBar, on_client, timestamp};

#[component]
pub(crate) fn Secrets() -> Element {
    let mut secrets = use_resource(|| async {
        if on_client() {
            Some(list_secrets().await)
        } else {
            None
        }
    });
    let mut notice = use_signal(|| None::<Notice>);
    let mut name = use_signal(String::new);
    let mut value = use_signal(String::new);

    let save = move |event: FormEvent| async move {
        event.prevent_default();
        let saved_name = name.read().trim().to_owned();
        let secret = value.read().clone();
        match set_secret(saved_name.clone(), secret).await {
            Ok(()) => {
                value.set(String::new());
                name.set(String::new());
                notice.set(Some(Notice::Info(format!("Saved the secret {saved_name}"))));
                secrets.restart();
            }
            Err(failure) => notice.set(Some(Notice::from(failure))),
        }
    };

    let listing = match &*secrets.read() {
        Some(Some(Ok(list))) if list.is_empty() => rsx! {
            p { class: "empty", "No secrets yet." }
        },
        Some(Some(Ok(list))) => rsx! {
            table { class: "list",
                thead {
                    tr {
                        th { "Name" }
                        th { "Last change" }
                        th { span { class: "sr-only", "Actions" } }
                    }
                }
                tbody {
                    for secret in list.clone() {
                        tr { key: "{secret.name}",
                            td { code { "{secret.name}" } }
                            td { class: "muted", "{timestamp(secret.updated_at)}" }
                            td { class: "row-actions",
                                button {
                                    class: "quiet",
                                    onclick: move |_| {
                                        let name = secret.name.clone();
                                        async move {
                                            match delete_secret(name.clone()).await {
                                                Ok(()) => {
                                                    notice.set(Some(Notice::Info(format!("Deleted the secret {name}"))));
                                                    secrets.restart();
                                                }
                                                Err(failure) => notice.set(Some(Notice::from(failure))),
                                            }
                                        }
                                    },
                                    "Delete"
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

    rsx! {
        div { class: "heading", h1 { "Secrets" } }
        p { class: "muted",
            "Workflows read secrets by name, for example "
            code { "secret \"NETLIFY_TOKEN\"" }
            ". Values are encrypted, never shown again, and masked in run output."
        }
        NoticeBar { notice }
        form { class: "panel inline-form", onsubmit: save,
            label {
                "Name"
                input {
                    required: true,
                    pattern: "[A-Za-z_][A-Za-z0-9_]*",
                    autocomplete: "off",
                    value: "{name}",
                    oninput: move |event| name.set(event.value()),
                }
            }
            label {
                "Value"
                input {
                    r#type: "password",
                    required: true,
                    autocomplete: "new-password",
                    value: "{value}",
                    oninput: move |event| value.set(event.value()),
                }
            }
            button { class: "primary", r#type: "submit", "Save secret" }
        }
        {listing}
    }
}
