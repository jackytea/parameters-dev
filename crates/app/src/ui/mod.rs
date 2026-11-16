pub(crate) mod account;
pub(crate) mod common;
pub(crate) mod device;
pub(crate) mod docs;
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
                        Link {
                            class: if matches!(route, Route::Docs { .. }) { "active" } else { "" },
                            to: Route::Docs { q: String::new() },
                            "Docs"
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
                    nav { class: "social", "aria-label": "Parameters elsewhere",
                        a {
                            href: "https://www.linkedin.com/company/parameters-dot-dev",
                            target: "_blank",
                            rel: "noopener noreferrer",
                            title: "LinkedIn",
                            svg { class: "icon", view_box: "0 0 24 24", "aria-hidden": "true",
                                path { d: "M16 8a6 6 0 0 1 6 6v7h-4v-7a2 2 0 0 0-2-2 2 2 0 0 0-2 2v7h-4v-7a6 6 0 0 1 6-6z" }
                                rect { width: "4", height: "12", x: "2", y: "9" }
                                circle { cx: "4", cy: "4", r: "2" }
                            }
                            span { class: "sr-only", "LinkedIn" }
                        }
                        a {
                            href: "https://github.com/parameters-dev",
                            target: "_blank",
                            rel: "noopener noreferrer",
                            title: "GitHub",
                            svg { class: "icon", view_box: "0 0 24 24", "aria-hidden": "true",
                                path { d: "M15 22v-4a4.8 4.8 0 0 0-1-3.5c3 0 6-2 6-5.5.08-1.25-.27-2.48-1-3.5.28-1.15.28-2.35 0-3.5 0 0-1 0-3 1.5-2.64-.5-5.36-.5-8 0C6 2 5 2 5 2c-.3 1.15-.3 2.35 0 3.5A5.403 5.403 0 0 0 4 9c0 3.5 3 5.5 6 5.5-.39.49-.68 1.05-.85 1.65-.17.6-.22 1.23-.15 1.85v4" }
                                path { d: "M9 18c-4.51 2-5-2-7-2" }
                            }
                            span { class: "sr-only", "GitHub" }
                        }
                        a {
                            href: "https://x.com/parameters_dev",
                            target: "_blank",
                            rel: "noopener noreferrer",
                            title: "X (Twitter)",
                            svg { class: "icon", view_box: "0 0 24 24", "aria-hidden": "true",
                                path { d: "M22 4s-.7 2.1-2 3.4c1.6 10-9.4 17.3-18 11.6 2.2.1 4.4-.6 6-2C3 15.5.5 9.6 3 5c2.2 2.6 5.6 4.1 9 4-.9-4.2 4-6.6 7-3.8 1.1 0 3-1.2 3-1.2z" }
                            }
                            span { class: "sr-only", "X (Twitter)" }
                        }
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
            view_box: "0 0 403 453",
            "aria-hidden": "true",
            path { d: "M204.319 120.257L301.314 173.007V278.507L204.319 331.257L107.324 278.507V173.007L204.319 120.257Z" }
            path { d: "M213.369 6.75293L390.039 105.753L402.639 112.813V339.7L390.039 346.761L213.369 445.761L201.319 452.514L189.27 445.761L107.319 399.838V343.324L201.319 396L353.339 310.812V141.7L201.319 56.5127L49.2998 141.7V310.812L49.3193 310.823V367.337L12.5996 346.761L0 339.7V112.813L12.5996 105.753L189.27 6.75293L201.319 0L213.369 6.75293Z" }
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
    if signed_out
        && !matches!(
            route,
            Route::SignIn {} | Route::Home {} | Route::Docs { .. }
        )
    {
        rsx! {
            SignIn {}
        }
    } else {
        rsx! {
            Outlet::<Route> {}
        }
    }
}

const STORED_THEME: &str = concat!(
    "try { const theme = localStorage.getItem('parameters-theme');",
    " if (theme === 'light' || theme === 'dark') document.documentElement.dataset.theme = theme; }",
    " catch (_) {}"
);

const SWITCH_THEME: &str = concat!(
    "const root = document.documentElement;",
    " const dark = root.dataset.theme ? root.dataset.theme === 'dark'",
    " : matchMedia('(prefers-color-scheme: dark)').matches;",
    " const next = dark ? 'light' : 'dark';",
    " root.dataset.theme = next;",
    " try { localStorage.setItem('parameters-theme', next); } catch (_) {}"
);

#[component]
fn ThemeToggle() -> Element {
    use_effect(|| script(STORED_THEME));
    rsx! {
        button {
            class: "quiet theme-toggle",
            r#type: "button",
            title: "Switch between the light and dark themes",
            onclick: move |_| script(SWITCH_THEME),
            svg {
                class: "icon icon-sun",
                view_box: "0 0 24 24",
                "aria-hidden": "true",
                circle { cx: "12", cy: "12", r: "4" }
                path { d: "M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41" }
            }
            svg {
                class: "icon icon-moon",
                view_box: "0 0 24 24",
                "aria-hidden": "true",
                path { d: "M20.985 12.486a9 9 0 1 1-9.473-9.472c.405-.022.617.46.402.803a6 6 0 0 0 8.268 8.268c.344-.215.825-.004.803.401" }
            }
            span { class: "sr-only", "Toggle theme" }
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
