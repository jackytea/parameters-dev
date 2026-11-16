use dioxus::prelude::*;
use parameters_api::{Me, Mode};

use crate::Route;
use crate::ui::editor::code::pieces;

const SAMPLE: &str = r#"workflow "Morning weather"
  note "Fetches the forecast and posts it to the team chat"

input city: text = "Lisbon"

step forecast uses http.request
  retry 3 times
  timeout 30s
  url is "https://wttr.in/{city}?format=3"

step post uses http.request
  when forecast succeeded
  method is "POST"
  url is secret "CHAT_WEBHOOK_URL"
  body is { text: forecast.body }

step record uses log.write
  after post
  message is "Posted the weather for {city}"

output report is forecast.body
"#;

const NODES: [(&str, &str); 8] = [
    ("http.request", "Call any HTTP API"),
    ("shell.run", "Run a shell command"),
    ("process.run", "Start a program"),
    ("file.read", "Read a file"),
    ("file.write", "Write a file"),
    ("file.list", "List a folder"),
    ("time.wait", "Pause for a while"),
    ("log.write", "Write to the run log"),
];

const FEATURES: [(&str, &str, &str); 6] = [
    (
        "Write",
        "Steps read like sentences, and every change is checked as you type.",
        "step greet uses log.write",
    ),
    (
        "Wire",
        "The graph and the text are two views of one JSON document. Edit either one.",
        "after prepare and warm_up",
    ),
    (
        "Trigger",
        "Start runs from signed webhooks or cron schedules, or by hand.",
        "0 9 * * MON-FRI",
    ),
    (
        "Watch",
        "Steps and logs stream live while a run works through the graph.",
        "forecast succeeded",
    ),
    (
        "Protect",
        "Secrets are encrypted and masked. Shell and file steps wait for a person's approval.",
        "secret \"CHAT_WEBHOOK_URL\"",
    ),
    (
        "Delegate",
        "AI agents read, write and run workflows through the built-in MCP server.",
        "/mcp",
    ),
];

struct Edition {
    name: &'static str,
    summary: &'static str,
    figure: &'static str,
    unit: &'static str,
    points: [&'static str; 3],
}

const EDITIONS: [Edition; 3] = [
    Edition {
        name: "Self-hosted",
        summary: "Runs on your own machine, with no accounts.",
        figure: "$0",
        unit: "/ AGPL-3.0",
        points: [
            "Docker Compose and PostgreSQL",
            "Every node, including shell and files",
            "Listens on localhost only",
        ],
    },
    Edition {
        name: "Managed",
        summary: "Hosted for you at parameters.dev.",
        figure: "500",
        unit: "/ steps per run",
        points: [
            "Sign in by email, Google, GitHub or Discord",
            "Sandboxed network for every run",
            "Tokens for apps, scripts and agents",
        ],
    },
    Edition {
        name: "Command line",
        summary: "Check, format, convert and run workflow files.",
        figure: "4",
        unit: "/ commands",
        points: [
            "parameters check",
            "parameters fmt and convert",
            "parameters run",
        ],
    },
];

#[component]
pub(crate) fn Home() -> Element {
    let navigator = use_navigator();
    use_effect(move || {
        if cfg!(any(feature = "desktop", feature = "mobile")) {
            navigator.replace(Route::Workflows {});
        }
    });
    let me = use_context::<Signal<Option<Me>>>();
    let current = me.read().clone();
    let signed_out = current
        .as_ref()
        .is_some_and(|me| me.mode == Mode::Managed && me.user.is_none());
    rsx! {
        section { class: "hero",
            span { class: "pill", "Open source workflow automation" }
            h1 { "PARAMETERS" }
            p { class: "lead", "Automation you can read." }
            p { class: "muted hero-detail",
                "Describe a workflow in plain English, see it as a graph, and run it on your machine or ours."
            }
            Calls { signed_out }
        }
        div { class: "band", "LANGUAGE" }
        section { class: "showcase",
            pre { class: "sample",
                code {
                    for (class , text) in pieces(SAMPLE) {
                        span { class, "{text}" }
                    }
                }
            }
            div { class: "showcase-notes",
                h3 { "One file, two views" }
                p { class: "muted",
                    "Every workflow is a JSON document. The text and the graph editor both edit it, and converting between them loses nothing."
                }
                h3 { "Parallel by default" }
                p { class: "muted",
                    "Steps run as soon as what they depend on is ready. Referring to another step's output is enough to wait for it."
                }
                h3 { "Checked before it runs" }
                p { class: "muted",
                    "Types, references and cycles are checked while you type, with the same rules the server uses."
                }
            }
        }
        div { class: "band", "NODES" }
        section { class: "cells",
            for (name , summary) in NODES {
                div { class: "cell",
                    code { "{name}" }
                    span { class: "muted", "{summary}" }
                }
            }
        }
        div { class: "band", "FEATURES" }
        section { class: "features",
            for (title , detail , sample) in FEATURES {
                div { class: "feature",
                    code { class: "feature-sample", "{sample}" }
                    h3 { "{title}" }
                    p { class: "muted", "{detail}" }
                }
            }
        }
        div { class: "band", "EDITIONS" }
        section { class: "editions",
            for edition in EDITIONS {
                div { class: "edition",
                    h3 { "{edition.name}" }
                    p { class: "muted", "{edition.summary}" }
                    p { class: "figure",
                        strong { "{edition.figure}" }
                        span { class: "muted", "{edition.unit}" }
                    }
                    ul {
                        for point in edition.points {
                            li { "{point}" }
                        }
                    }
                }
            }
        }
        section { class: "closing",
            h2 { "Write. Wire. Run." }
            p { class: "lead", "Plain steps, checked before they run." }
            Calls { signed_out }
        }
    }
}

#[component]
fn Calls(signed_out: bool) -> Element {
    rsx! {
        div { class: "calls",
            if signed_out {
                Link { class: "button primary", to: Route::SignIn {}, "Sign in" }
            } else {
                Link { class: "button primary", to: Route::Workflows {}, "Open your workflows" }
            }
            Link { class: "button", to: Route::Docs { q: String::new() }, "Read the docs" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sample_parses_cleanly() {
        assert_eq!(parameters_lang::parse(SAMPLE).diagnostics, []);
    }
}
