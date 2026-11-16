pub(crate) mod account;
pub(crate) mod common;
pub(crate) mod device;
pub(crate) mod editor;
pub(crate) mod home;
pub(crate) mod nodes;
pub(crate) mod runs;
pub(crate) mod secrets;
pub(crate) mod workflows;

use dioxus::prelude::*;

use parameters_api::{Me, Mode};

use crate::Route;
use crate::api::me;
use crate::ui::common::{on_client, script};

const THEME_KEY: &str = "parameters-theme";

#[component]
pub(crate) fn Shell() -> Element {
    let mut session = use_context_provider(|| Signal::new(None::<Me>));
    let mut loaded = use_resource(|| async { if on_client() { Some(me().await) } else { None } });
    use_effect(move || {
        let current = loaded.read().clone().flatten().and_then(Result::ok);
        session.set(current);
    });
    let unreachable = loaded
        .read()
        .clone()
        .flatten()
        .and_then(Result::err)
        .map(|failure| failure.to_string());
    let current = session.read().clone();
    let managed = current.as_ref().is_some_and(|me| me.mode == Mode::Managed);
    let user = current.and_then(|me| me.user);
    let signed_out = managed && user.is_none() && loaded.read().is_some();
    let route = use_route::<Route>();
    let home = matches!(route, Route::Home {});
    let frame = if home { "frame landing" } else { "frame" };
    rsx! {
        div { class: "app",
            header { class: "topbar",
                div { class: "frame",
                    Link { class: "brand", to: Route::Home {},
                        Mark {}
                        span { "Parameters" }
                    }
                    nav { class: "tabs",
                        if !signed_out {
                            Link { to: Route::Workflows {}, active_class: "active", "Workflows" }
                            Link { to: Route::Runs {}, active_class: "active", "Runs" }
                            Link { to: Route::Nodes {}, active_class: "active", "Nodes" }
                            Link { to: Route::Secrets {}, active_class: "active", "Secrets" }
                            Link { to: Route::Account {}, active_class: "active", "Account" }
                        }
                    }
                    ThemeToggle {}
                    if signed_out && !matches!(route, Route::SignIn {}) {
                        Link { class: "button", to: Route::SignIn {}, "Sign in" }
                    }
                    if managed {
                        if let Some(user) = user {
                            div { class: "signout",
                                span { class: "muted", "{user.display_name}" }
                                button {
                                    class: "quiet",
                                    r#type: "button",
                                    onclick: move |_| async move {
                                        if cfg!(any(feature = "desktop", feature = "mobile")) {
                                            #[cfg(any(feature = "desktop", feature = "mobile"))]
                                            crate::native::sign_out().await;
                                            loaded.restart();
                                        } else {
                                            script(
                                                "fetch('/auth/signout', { method: 'POST', credentials: 'same-origin' })
                                                 .finally(() => location.assign('/signin'));",
                                            );
                                        }
                                    },
                                    "Sign out"
                                }
                            }
                        }
                    }
                }
            }
            main { class: "page",
                div { class: frame,
                    {page(unreachable, signed_out, &route, move || loaded.restart())}
                }
            }
            footer { class: "site-footer",
                div { class: "frame",
                    p { "© Parameters" }
                    nav { class: "social",
                        a { href: "https://github.com/parameters-dev", target: "_blank", rel: "noopener noreferrer", "GitHub" }
                        a { href: "https://www.linkedin.com/company/parameters-dot-dev", target: "_blank", rel: "noopener noreferrer", "LinkedIn" }
                        a { href: "https://x.com/parameters_dev", target: "_blank", rel: "noopener noreferrer", "X" }
                    }
                }
            }
        }
    }
}

#[component]
fn Mark() -> Element {
    rsx! {
        svg {
            class: "mark",
            view_box: "0 0 24 24",
            width: "20",
            height: "20",
            "aria-hidden": "true",
            rect { x: "2", y: "3", width: "7", height: "7" }
            rect { x: "15", y: "14", width: "7", height: "7" }
            path { d: "M9 6.5h4.5v11H15" }
        }
    }
}

#[cfg(any(feature = "desktop", feature = "mobile"))]
fn page(
    unreachable: Option<String>,
    signed_out: bool,
    _route: &Route,
    retry: impl FnMut() + Clone + 'static,
) -> Element {
    use crate::ui::device::native;
    if let Some(problem) = unreachable {
        let mut retry = retry;
        return rsx! {
            native::Unreachable { problem, on_retry: move |()| retry() }
        };
    }
    if signed_out {
        let mut retry = retry;
        return rsx! {
            native::SignIn { on_done: move |()| retry() }
        };
    }
    rsx! {
        Outlet::<Route> {}
    }
}

#[cfg(not(any(feature = "desktop", feature = "mobile")))]
fn page(
    _unreachable: Option<String>,
    signed_out: bool,
    route: &Route,
    _retry: impl FnMut() + Clone + 'static,
) -> Element {
    use crate::ui::account::SignIn;
    if signed_out && !matches!(route, Route::SignIn {} | Route::Home {}) {
        rsx! {
            SignIn {}
        }
    } else {
        rsx! {
            Outlet::<Route> {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Theme {
    System,
    Light,
    Dark,
}

impl Theme {
    const fn name(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::System => "Theme: system",
            Self::Light => "Theme: light",
            Self::Dark => "Theme: dark",
        }
    }

    const fn next(self) -> Self {
        match self {
            Self::System => Self::Light,
            Self::Light => Self::Dark,
            Self::Dark => Self::System,
        }
    }

    fn from_name(name: &str) -> Self {
        [Self::Light, Self::Dark]
            .into_iter()
            .find(|theme| theme.name() == name)
            .unwrap_or(Self::System)
    }
}

fn apply_theme(theme: Theme) {
    script(&format!(
        "try {{ localStorage.setItem({key:?}, {name:?}); }} catch (_) {{}}
         if ({name:?} === 'system') delete document.documentElement.dataset.theme;
         else document.documentElement.dataset.theme = {name:?};",
        key = THEME_KEY,
        name = theme.name(),
    ));
}

#[component]
fn ThemeToggle() -> Element {
    let mut theme = use_signal(|| Theme::System);
    use_effect(move || {
        if !on_client() {
            return;
        }
        spawn(async move {
            let stored = document::eval(&format!(
                "try {{ return localStorage.getItem({THEME_KEY:?}) ?? 'system'; }} catch (_) {{ return 'system'; }}"
            ))
            .join::<String>()
            .await
            .unwrap_or_default();
            let stored = Theme::from_name(&stored);
            theme.set(stored);
            apply_theme(stored);
        });
    });
    rsx! {
        button {
            class: "quiet",
            r#type: "button",
            title: "Switch between the system, light and dark themes",
            onclick: move |_| {
                let next = theme().next();
                theme.set(next);
                apply_theme(next);
            },
            "{theme().label()}"
        }
    }
}

#[component]
pub(crate) fn NotFound(segments: Vec<String>) -> Element {
    let path = segments.join("/");
    rsx! {
        section { class: "empty",
            h1 { "Nothing here" }
            p { "There is no page at /{path}." }
            Link { to: Route::Workflows {}, "Go to your workflows" }
        }
    }
}
