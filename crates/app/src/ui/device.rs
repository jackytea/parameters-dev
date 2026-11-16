use dioxus::prelude::*;

use crate::api::approve_device;
use crate::ui::common::{Notice, NoticeBar};

#[component]
pub(crate) fn Connect(code: String) -> Element {
    let mut typed = use_signal(|| code.clone());
    let mut connected = use_signal(|| None::<String>);
    let mut notice = use_signal(|| None::<Notice>);
    let approve = move |event: FormEvent| async move {
        event.prevent_default();
        let code = typed.read().clone();
        match approve_device(code).await {
            Ok(name) => connected.set(Some(name)),
            Err(failure) => notice.set(Some(Notice::from(failure))),
        }
    };
    rsx! {
        section { class: "signin",
            h1 { "Connect the Parameters app" }
            NoticeBar { notice }
            if let Some(name) = connected() {
                p { class: "notice", role: "status", "{name} is connected. You can go back to the app." }
            } else {
                p { "Check that this code matches the one the app shows, then allow it to use your account." }
                form { class: "panel", onsubmit: approve,
                    label {
                        "Code"
                        input {
                            class: "mono",
                            required: true,
                            autocomplete: "off",
                            value: "{typed}",
                            oninput: move |event| typed.set(event.value()),
                        }
                    }
                    div { class: "actions",
                        button { class: "primary", r#type: "submit", "Allow" }
                    }
                }
            }
        }
    }
}

#[cfg(any(feature = "desktop", feature = "mobile"))]
pub(crate) mod native {
    use dioxus::prelude::*;
    use parameters_api::DeviceStart;

    use crate::native;

    #[component]
    pub(crate) fn Unreachable(problem: String, on_retry: EventHandler<()>) -> Element {
        let mut address = use_signal(|| native::server().to_owned());
        let mut error = use_signal(|| None::<String>);
        let server = native::server();
        rsx! {
            section { class: "signin",
                h1 { "Cannot reach {server}" }
                p { class: "muted", "{problem}" }
                button { class: "primary", onclick: move |_| on_retry.call(()), "Try again" }
                if cfg!(feature = "desktop") {
                    form {
                        class: "panel",
                        onsubmit: move |event: FormEvent| async move {
                            event.prevent_default();
                            match native::save_server(&address.read()).await {
                                Ok(()) => {
                                    #[cfg(feature = "desktop")]
                                    native::restart();
                                }
                                Err(message) => error.set(Some(message)),
                            }
                        },
                        label {
                            "Server"
                            input {
                                r#type: "url",
                                value: "{address}",
                                oninput: move |event| address.set(event.value()),
                            }
                        }
                        p { class: "muted",
                            "Use https://parameters.dev, or http://localhost:8080 for Parameters running on this computer. The app restarts to connect."
                        }
                        if let Some(message) = error() {
                            p { class: "notice error", "{message}" }
                        }
                        div { class: "actions",
                            button { r#type: "submit", "Connect" }
                        }
                    }
                }
            }
        }
    }

    #[component]
    pub(crate) fn SignIn(on_done: EventHandler<()>) -> Element {
        let mut pending = use_signal(|| None::<DeviceStart>);
        let mut error = use_signal(|| None::<String>);
        let mut busy = use_signal(|| false);
        let start = move |_| async move {
            busy.set(true);
            error.set(None);
            match native::sign_in(move |start| pending.set(Some(start))).await {
                Ok(()) => on_done.call(()),
                Err(message) => error.set(Some(message)),
            }
            pending.set(None);
            busy.set(false);
        };
        rsx! {
            section { class: "signin",
                h1 { "Sign in to Parameters" }
                p { "Signing in opens your browser. Approve this device there, and the app continues by itself." }
                if let Some(start) = pending() {
                    p { "Your browser should open. If it does not, go to " code { "{start.verification_uri}" } "." }
                    p { "Check that it shows this code:" }
                    pre { class: "output device-code", "{start.user_code}" }
                } else {
                    button { class: "primary", disabled: busy(), onclick: start, "Sign in with your browser" }
                }
                if let Some(message) = error() {
                    p { class: "notice error", "{message}" }
                }
            }
        }
    }
}
