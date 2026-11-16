pub(crate) mod common;
pub(crate) mod editor;
pub(crate) mod nodes;
pub(crate) mod runs;
pub(crate) mod secrets;
pub(crate) mod workflows;

use dioxus::prelude::*;

use crate::Route;
use crate::ui::common::{on_client, script};

const THEME_KEY: &str = "parameters-theme";

#[component]
pub(crate) fn Shell() -> Element {
    rsx! {
        header { class: "topbar",
            Link { class: "brand", to: Route::Workflows {}, "Parameters" }
            nav { class: "tabs",
                Link { to: Route::Workflows {}, active_class: "active", "Workflows" }
                Link { to: Route::Runs {}, active_class: "active", "Runs" }
                Link { to: Route::Nodes {}, active_class: "active", "Nodes" }
                Link { to: Route::Secrets {}, active_class: "active", "Secrets" }
            }
            ThemeToggle {}
        }
        main { class: "page", Outlet::<Route> {} }
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
