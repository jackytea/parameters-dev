use std::fmt::Write;

use dioxus::prelude::*;
use parameters_api::{Limits, Me, Mode, NodeInfo, NodeKind, PortInfo, ServerDocs, ToolDoc};
use parameters_core::name::KEYWORDS;
use parameters_core::{Code, ValueType};
use parameters_lang::{
    EXAMPLE, EXPRESSIONS, EXPRESSIONS_INTRODUCTION, EXPRESSIONS_NOTE, INTRODUCTION,
    KEYWORDS_INTRODUCTION, STATEMENTS, type_description,
};

use crate::api::server_docs;
use crate::ui::common::{on_client, script};
use crate::ui::editor::code::pieces;

#[derive(Debug, Clone, PartialEq, Default)]
struct Entry {
    id: String,
    title: String,
    badges: Vec<String>,
    paragraphs: Vec<String>,
    ports: Vec<(&'static str, Vec<PortInfo>)>,
    sample: Option<String>,
    command: Option<String>,
    search: String,
}

impl Entry {
    fn new(section: &str, title: impl Into<String>) -> Self {
        let title = title.into();
        Self {
            id: format!("{section}-{}", slug(&title)),
            title,
            ..Self::default()
        }
    }

    fn text(mut self, paragraph: impl Into<String>) -> Self {
        self.paragraphs.push(paragraph.into());
        self
    }

    fn badge(mut self, badge: impl Into<String>) -> Self {
        self.badges.push(badge.into());
        self
    }

    fn sample(mut self, sample: impl Into<String>) -> Self {
        self.sample = Some(sample.into());
        self
    }

    fn command(mut self, command: impl Into<String>) -> Self {
        self.command = Some(command.into());
        self
    }

    fn ports(mut self, label: &'static str, ports: &[PortInfo]) -> Self {
        if !ports.is_empty() {
            self.ports.push((label, ports.to_vec()));
        }
        self
    }

    fn indexed(mut self, section: &str) -> Self {
        let mut search = format!("{section} {} {}", self.title, self.badges.join(" "));
        for paragraph in &self.paragraphs {
            search.push(' ');
            search.push_str(paragraph);
        }
        for (label, ports) in &self.ports {
            search.push(' ');
            search.push_str(label);
            for port in ports {
                search.push(' ');
                search.push_str(&port.name);
            }
        }
        for code in self.sample.iter().chain(&self.command) {
            search.push(' ');
            search.push_str(code);
        }
        self.search = search.replace('`', "").to_lowercase();
        self
    }

    fn matches(&self, terms: &[String]) -> bool {
        terms.iter().all(|term| self.search.contains(term.as_str()))
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Section {
    id: &'static str,
    title: &'static str,
    summary: String,
    entries: Vec<Entry>,
}

impl Section {
    fn new(
        id: &'static str,
        title: &'static str,
        summary: impl Into<String>,
        entries: Vec<Entry>,
    ) -> Self {
        let entries = entries
            .into_iter()
            .map(|entry| entry.indexed(title))
            .collect();
        Self {
            id,
            title,
            summary: summary.into(),
            entries,
        }
    }
}

fn slug(text: &str) -> String {
    let mut slug = String::new();
    for character in text.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_end_matches('-').to_owned()
}

fn app_section() -> Section {
    let id = "app";
    Section::new(
        id,
        "Using the app",
        "The pages in the header and what you can do on each.",
        vec![
            Entry::new(id, "Workflows")
                .text("The Workflows page lists your saved workflows, most recently changed first. Create a workflow, open one in the editor, duplicate it or delete it.")
                .text("Import accepts a workflow in the Parameters language or as JSON, pasted or from a file. Problems are listed with their line and column before anything is saved."),
            Entry::new(id, "Runs")
                .text("Every run, newest first, with its status and how it started: from the app, an AI agent, the API, a webhook or a schedule.")
                .text("Open a run to follow each step's status, outputs and log as they happen. Cancel a run while it is queued or running."),
            Entry::new(id, "Node library")
                .text("The Nodes page holds your custom nodes, written with `define node`. Any workflow can use them like built-in nodes, and a workflow gets its own copy of the nodes it uses when it is saved.")
                .text("Import nodes from a workflow file. When a name is already taken, choose to rename the new node, replace the existing one, or keep the existing one."),
            Entry::new(id, "Secrets")
                .text("Store passwords, tokens and keys by name, then use them in a workflow with `secret \"NAME\"`. Values are encrypted, never shown again after you save them, and masked in outputs, logs and errors."),
            Entry::new(id, "Account and access tokens")
                .text("On a server with accounts, sign in with an emailed link or with Google, GitHub or Discord. Create access tokens on the Account page for AI agents and scripts: each one can read, write and/or run, can expire, can be revoked, and is shown only once.")
                .text("A self-hosted server runs on your machine and has no accounts or tokens."),
            Entry::new(id, "Desktop and mobile apps")
                .text("The desktop and mobile apps are this same interface, connected to a Parameters server. On a server with accounts, the app shows a code and opens your browser; check that the code matches and press Allow."),
            Entry::new(id, "Theme")
                .text("The sun and moon button in the header switches between the light and dark themes and remembers your choice on this device. Until you choose, the theme follows your system."),
        ],
    )
}

fn editor_section() -> Section {
    let id = "editor";
    Section::new(
        id,
        "Editor",
        "Open a workflow to edit it as text, as a graph, or both.",
        vec![
            Entry::new(id, "Code and graph")
                .text("The text and the graph are two views of the same workflow, kept in sync. Choose Code, Both or Graph at the top of the editor. On a narrow screen, switch between them."),
            Entry::new(id, "Write as text")
                .text("Type in the code pane. Problems are underlined as you type and listed below the editor with their code; see Diagnostics. While the text has a syntax error, the graph is read-only.")
                .text("Format rewrites the text in the standard layout."),
            Entry::new(id, "Build in the graph")
                .text("Pick a node in Add a step and press Add. Drag a step's right-hand port onto another step to make that step wait for it, and click a connection to remove it.")
                .text("Drag the background to move around, and use the zoom buttons or Fit to show every step."),
            Entry::new(id, "Edit a step")
                .text("Select a step in the graph to edit its name and fields in the inspector on the right. Steps that run commands or use files are marked as needing approval."),
            Entry::new(id, "Save and export")
                .text("Save stores a new version. Export JSON downloads the workflow document, which is the source of truth for both views.")
                .text("Changes saved elsewhere, including by an AI agent, appear in open editors straight away. If you have unsaved changes, you choose which version to keep."),
            Entry::new(id, "Approve privileged steps")
                .text("Before a workflow with steps that run commands or use files can run, the review panel shows the commands, files, hosts and secrets it uses. Approve that exact version to let it run. Every change needs a new approval, and only a person in the app can approve.")
                .badge("needs approval"),
            Entry::new(id, "Run")
                .text("Press Run, fill in the workflow's inputs and watch each step's status on the graph, with outputs and the log in the run panel. Steps run in parallel unless something orders them."),
        ],
    )
}

fn trigger_section() -> Section {
    let id = "triggers";
    Section::new(
        id,
        "Triggers",
        "Start runs automatically. Add triggers in the editor's Triggers panel.",
        vec![
            Entry::new(id, "Webhooks")
                .text("A webhook gets a secret address, `/hooks/whk_…`, shown once. A `POST` to it queues a run, and the request body (up to 256 KiB, JSON or text) becomes the workflow's input named `payload`, if it declares one. Each webhook accepts up to 60 calls a minute."),
            Entry::new(id, "Signed webhooks")
                .text("A signed webhook also needs the header `X-Parameters-Signature: sha256=<hex>`, the HMAC-SHA256 of the body with the trigger's signing secret. GitHub's `X-Hub-Signature-256` header is accepted too.")
                .command("curl -X POST \"$WEBHOOK_URL\" \\\n  -H \"X-Parameters-Signature: sha256=$(printf '%s' \"$BODY\" | openssl dgst -sha256 -hmac \"$SIGNING_SECRET\" | cut -d ' ' -f 2)\" \\\n  -d \"$BODY\""),
            Entry::new(id, "Schedules")
                .text("Schedules use five cron fields in UTC: minute, hour, day of month, month and day of week. Lists, ranges and steps work, such as `0 9 * * 1-5` or `*/15 * * * *`, and so do `@hourly`, `@daily`, `@weekly`, `@monthly` and `@yearly`.")
                .text("A schedule missed while the server was down runs once when it comes back."),
            Entry::new(id, "Runs from triggers")
                .text("Runs started by a trigger count as yours: they need the same approvals, count against the same limits and are labelled webhook or schedule on the Runs page. If a trigger cannot start a run, its last error is shown next to it."),
        ],
    )
}

fn language_section() -> Section {
    let id = "language";
    let mut entries = vec![
        Entry::new(id, "Overview")
            .text(INTRODUCTION[0])
            .text(INTRODUCTION[1]),
        Entry::new(id, "Example").sample(EXAMPLE),
    ];
    entries.extend(STATEMENTS.iter().map(|(form, place, meaning)| {
        Entry::new(id, format!("`{form}`"))
            .text(*meaning)
            .text(format!("Where: {place}."))
    }));
    entries.push(
        Entry::new(id, "Reserved keywords")
            .text(KEYWORDS_INTRODUCTION)
            .text(
                KEYWORDS
                    .iter()
                    .map(|keyword| format!("`{keyword}`"))
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
    );
    Section::new(id, "Language", "Statements, one per line.", entries)
}

fn types_section() -> Section {
    let id = "types";
    Section::new(
        id,
        "Types",
        "The types of inputs, outputs and node fields.",
        ValueType::ALL
            .iter()
            .map(|value_type| {
                Entry::new(id, format!("`{value_type}`")).text(type_description(*value_type))
            })
            .collect(),
    )
}

fn expressions_section() -> Section {
    let id = "expressions";
    let mut entries: Vec<Entry> = EXPRESSIONS
        .iter()
        .map(|(title, rule)| Entry::new(id, *title).text(*rule))
        .collect();
    entries.push(Entry::new(id, "How expressions are stored").text(EXPRESSIONS_NOTE));
    Section::new(id, "Expressions", EXPRESSIONS_INTRODUCTION, entries)
}

fn diagnostics_section() -> Section {
    let id = "diagnostics";
    Section::new(
        id,
        "Diagnostics",
        "Every problem the checker reports has a code. Errors stop a workflow from running; warnings do not.",
        Code::ALL
            .iter()
            .map(|code| {
                Entry::new(id, format!("`{}` {}", code.id(), sentence(code.title())))
                    .badge(code.severity().to_string())
            })
            .collect(),
    )
}

fn sentence(text: &str) -> String {
    let mut characters = text.chars();
    characters
        .next()
        .map(|first| first.to_uppercase().chain(characters).collect())
        .unwrap_or_default()
}

fn node_section(nodes: &[NodeInfo], limits: &Limits) -> Section {
    let id = "nodes";
    let summary = if limits.privileged_nodes {
        "The nodes a step can use on this server: the built-in nodes and your custom nodes."
    } else {
        "The nodes a step can use on this server: the built-in nodes and your custom nodes. Nodes that run commands or use files are not available here."
    };
    Section::new(
        id,
        "Nodes",
        summary,
        nodes
            .iter()
            .map(|node| {
                let mut entry =
                    Entry::new(id, format!("`{}`", node.type_name)).badge(match node.kind {
                        NodeKind::Builtin => "built-in",
                        NodeKind::Custom => "custom",
                    });
                if node.privileged {
                    entry = entry.badge("needs approval");
                }
                if let Some(description) = &node.description {
                    entry = entry.text(description.clone());
                }
                entry
                    .ports("Inputs", &node.inputs)
                    .ports("Outputs", &node.outputs)
                    .sample(node.example.clone())
            })
            .collect(),
    )
}

fn agent_section(tools: &[ToolDoc], mode: Mode, origin: &str) -> Section {
    let id = "agents";
    let connect = match mode {
        Mode::Local => Entry::new(id, "Connect an agent")
            .text("This server speaks the Model Context Protocol at `/mcp`. On a self-hosted server, agents connect without a token. For example, with Claude Code:")
            .command(format!("claude mcp add --transport http parameters {origin}/mcp")),
        Mode::Managed => Entry::new(id, "Connect an agent")
            .text("This server speaks the Model Context Protocol at `/mcp`. Create an access token on the Account page, then connect. For example, with Claude Code:")
            .command(format!(
                "claude mcp add --transport http parameters {origin}/mcp --header \"Authorization: Bearer prm_…\""
            )),
    };
    let mut entries = vec![
        connect,
        Entry::new(id, "What agents can and cannot do")
            .text("Agents read the language reference, write and validate workflows, run them, manage triggers and read the results. They cannot delete or approve workflows: a workflow with privileged steps only runs after a person approves it in the app.")
            .text("Run results are returned as data, with long values shortened, and labelled so the agent does not treat them as instructions."),
    ];
    entries.extend(tools.iter().map(|tool| {
        Entry::new(id, tool.title.clone())
            .badge(tool.name.clone())
            .badge(if tool.read_only { "reads" } else { "changes" })
            .text(tool.description.clone())
    }));
    Section::new(
        id,
        "AI agents",
        "Tools this server offers to AI agents over MCP.",
        entries,
    )
}

fn server_section(docs: &ServerDocs) -> Section {
    let id = "server";
    let limits = docs.limits;
    let or_none = |value: Option<String>| value.unwrap_or_else(|| "no limit".to_owned());
    let mode = match docs.mode {
        Mode::Local => Entry::new(id, "Self-hosted")
            .text("This server runs on your machine, has no accounts and only accepts requests from localhost."),
        Mode::Managed => Entry::new(id, "Managed")
            .text("This server has accounts. Your workflows, runs, secrets and nodes are visible only to you."),
    };
    let sandbox = if limits.privileged_nodes {
        "Steps can run commands and use files in the server's workspace, after you approve the workflow."
    } else {
        "Steps cannot run commands or use files."
    };
    let network = if limits.private_network {
        "HTTP requests can reach any address."
    } else {
        "HTTP requests cannot reach private, loopback, link-local or cloud metadata addresses, including through redirects."
    };
    Section::new(
        id,
        "This server",
        "How this server is set up and the limits it applies.",
        vec![
            mode,
            Entry::new(id, "Sandbox").text(sandbox).text(network),
            Entry::new(id, "Limits")
                .text(format!(
                    "Steps per workflow: {}.",
                    or_none(limits.max_steps.map(|steps| steps.to_string()))
                ))
                .text(format!(
                    "Time per run: {}.",
                    or_none(
                        limits
                            .max_run_minutes
                            .map(|minutes| format!("{minutes} minutes"))
                    )
                ))
                .text(format!(
                    "Queued or running runs at once: {}.",
                    or_none(limits.max_active_runs.map(|runs| runs.to_string()))
                ))
                .text(format!(
                    "Requests per minute: {}.",
                    or_none(
                        limits
                            .requests_per_minute
                            .map(|requests| requests.to_string())
                    )
                ))
                .text(format!(
                    "Shortest time between scheduled runs: {}.",
                    limits.min_schedule_minutes.map_or_else(
                        || "1 minute".to_owned(),
                        |minutes| format!("{minutes} minutes")
                    )
                )),
        ],
    )
}

fn sections(server: Option<&ServerDocs>, origin: &str) -> Vec<Section> {
    let mut sections = vec![
        app_section(),
        editor_section(),
        trigger_section(),
        language_section(),
        types_section(),
        expressions_section(),
        diagnostics_section(),
    ];
    if let Some(server) = server {
        sections.push(node_section(&server.nodes, &server.limits));
        sections.push(agent_section(&server.tools, server.mode, origin));
        sections.push(server_section(server));
    }
    sections
}

fn terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(|term| term.trim_matches('`').to_lowercase())
        .filter(|term| !term.is_empty())
        .collect()
}

fn inline(text: &str) -> Element {
    rsx! {
        for (index , part) in text.split('`').enumerate() {
            if index % 2 == 1 {
                code { "{part}" }
            } else {
                "{part}"
            }
        }
    }
}

#[component]
pub(crate) fn Docs(q: String) -> Element {
    let mut query = use_signal(|| q.clone());
    let mut origin = use_signal(String::new);
    let me = use_context::<Signal<Option<Me>>>();
    let server = use_resource(move || {
        let signed_in = me.read().as_ref().map(|me| me.user.is_some());
        async move {
            if on_client() {
                Some((signed_in, server_docs().await))
            } else {
                None
            }
        }
    });
    use_effect(move || {
        if !on_client() {
            return;
        }
        script(
            "if (!window.parametersDocsKeys) {
               window.parametersDocsKeys = true;
               document.addEventListener('keydown', (event) => {
                 const target = event.target;
                 const typing = target instanceof HTMLElement && (target.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName));
                 const search = document.getElementById('docs-search');
                 if (event.key === '/' && !typing && search) {
                   event.preventDefault();
                   search.focus();
                 }
               });
             }",
        );
        spawn(async move {
            if let Ok(location) = document::eval("return location.origin;")
                .join::<String>()
                .await
            {
                origin.set(location);
            }
        });
    });

    let state = server.read().clone().flatten().map(|(_, result)| result);
    let loaded = state.clone().and_then(Result::ok);
    let failed = state
        .and_then(Result::err)
        .map(|failure| failure.to_string());
    let pending = loaded.is_none() && failed.is_none();
    let all = sections(loaded.as_ref(), &origin.read());
    let current = query.read().clone();
    let terms = terms(&current);
    let shown: Vec<Section> = all
        .into_iter()
        .map(|mut section| {
            section.entries.retain(|entry| entry.matches(&terms));
            section
        })
        .filter(|section| !section.entries.is_empty())
        .collect();
    let found: usize = shown.iter().map(|section| section.entries.len()).sum();
    let searching = !terms.is_empty();

    rsx! {
        div { class: "docs",
            aside { class: "docs-nav",
                label { class: "sr-only", r#for: "docs-search", "Search the docs" }
                input {
                    id: "docs-search",
                    r#type: "search",
                    placeholder: "Search the docs  /",
                    autocomplete: "off",
                    value: "{current}",
                    oninput: move |event| {
                        let value = event.value();
                        let address = serde_json::to_string(&if value.trim().is_empty() {
                                "/docs".to_owned()
                            } else {
                                format!("/docs?q={}", js_encode(&value))
                            })
                            .unwrap_or_default();
                        script(&format!("history.replaceState(history.state, '', {address});"));
                        query.set(value);
                    },
                }
                nav { "aria-label": "Sections",
                    for group in shown.iter() {
                        a { href: "#{group.id}",
                            span { "{group.title}" }
                            if searching {
                                span { class: "muted", "{group.entries.len()}" }
                            }
                        }
                    }
                }
            }
            article { class: "docs-body",
                header { class: "docs-intro",
                    h1 { "Documentation" }
                    p { class: "muted",
                        "Built from this server and the language itself, so it always matches the version you are using. The node list includes your custom nodes."
                    }
                    if searching {
                        p { class: "docs-count", role: "status",
                            if found == 1 {
                                "1 result for “{current.trim()}”"
                            } else {
                                "{found} results for “{current.trim()}”"
                            }
                        }
                    }
                }
                if shown.is_empty() {
                    p { class: "empty", "Nothing matches “{current.trim()}”. Try fewer or different words." }
                }
                for group in shown {
                    section { class: "docs-section", id: "{group.id}", key: "{group.id}",
                        h2 { "{group.title}" }
                        p { class: "muted", {inline(&group.summary)} }
                        for entry in group.entries {
                            DocEntry { key: "{entry.id}", entry }
                        }
                    }
                }
                if pending {
                    p { class: "muted docs-pending", "Loading the nodes, agent tools and limits from this server…" }
                }
                if let Some(problem) = failed {
                    p { class: "notice error", "Could not load the nodes, agent tools and limits: {problem}" }
                }
            }
        }
    }
}

fn js_encode(text: &str) -> String {
    text.bytes().fold(String::new(), |mut out, byte| {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
        out
    })
}

#[component]
fn DocEntry(entry: Entry) -> Element {
    rsx! {
        div { class: "docs-entry", id: "{entry.id}",
            h3 {
                a { class: "anchor", href: "#{entry.id}", {inline(&entry.title)} }
                for badge in entry.badges.iter() {
                    span { class: "badge", "{badge}" }
                }
            }
            for paragraph in entry.paragraphs.iter() {
                p { {inline(paragraph)} }
            }
            for (label , ports) in entry.ports.iter() {
                table { class: "list ports",
                    thead {
                        tr {
                            th { "{label}" }
                            th { "Type" }
                            th { "Required" }
                        }
                    }
                    tbody {
                        for port in ports.iter() {
                            tr {
                                td { code { "{port.name}" } }
                                td { code { "{port.value_type}" } }
                                td { if port.required { "yes" } else { "no" } }
                            }
                        }
                    }
                }
            }
            if let Some(sample) = &entry.sample {
                pre { class: "sample",
                    code {
                        for (class , text) in pieces(sample) {
                            span { class, "{text}" }
                        }
                    }
                }
            }
            if let Some(command) = &entry.command {
                pre { class: "sample", code { "{command}" } }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_readable_and_unique_within_a_section() {
        assert_eq!(slug("`step id uses node.type`"), "step-id-uses-node-type");
        let language = language_section();
        let mut ids: Vec<&str> = language
            .entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count);
    }

    #[test]
    fn search_needs_every_term() {
        let sections = sections(None, "http://localhost:8080");
        let found = |query: &str| -> Vec<String> {
            sections
                .iter()
                .flat_map(|section| &section.entries)
                .filter(|entry| entry.matches(&terms(query)))
                .map(|entry| entry.title.clone())
                .collect()
        };
        assert_eq!(found("P0205"), ["`P0205` Type mismatch"]);
        assert!(found("retry times").contains(&"`retry 3 times`".to_owned()));
        assert!(found("webhook signature").contains(&"Signed webhooks".to_owned()));
        assert_eq!(found("nothing-like-this"), Vec::<String>::new());
    }

    #[test]
    fn every_language_rule_is_documented() {
        let sections = sections(None, "");
        let count = |id: &str| {
            sections
                .iter()
                .find(|section| section.id == id)
                .map_or(0, |section| section.entries.len())
        };
        assert_eq!(count("types"), ValueType::ALL.len());
        assert_eq!(count("diagnostics"), Code::ALL.len());
        assert_eq!(count("expressions"), EXPRESSIONS.len() + 1);
    }
}
