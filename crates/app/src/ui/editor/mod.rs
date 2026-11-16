mod code;
mod graph;
mod inspector;

use dioxus::prelude::*;
use parameters_api::{ApiError, Problem, Review, Source, Uuid, Via, WorkflowRecord};
use parameters_core::layout::complete_layouts;
use parameters_core::{Position, ValueType, Workflow, to_json};
use serde_json::{Map, Value};

use crate::Route;
use crate::api::{
    approve_workflow, get_workflow, node_catalog, review_workflow, start_run, update_workflow,
    watch_workflow,
};
use crate::catalog::NodeCatalog;
use crate::failure::Failure;
use crate::ui::common::{Notice, NoticeBar, RunBadge, download, on_client, pause};
use crate::ui::runs::{LiveRun, RunDetails, follow_run};

#[derive(Clone, Copy)]
pub(crate) struct EditorState {
    pub(crate) id: Uuid,
    pub(crate) record: Signal<Option<WorkflowRecord>>,
    pub(crate) workflow: Signal<Workflow>,
    pub(crate) text: Signal<String>,
    pub(crate) problems: Signal<Vec<Problem>>,
    pub(crate) text_valid: Signal<bool>,
    pub(crate) dirty: Signal<bool>,
    pub(crate) selected: Signal<Option<String>>,
    pub(crate) catalog: Signal<NodeCatalog>,
    pub(crate) remote: Signal<Option<WorkflowRecord>>,
    pub(crate) notice: Signal<Option<Notice>>,
    pub(crate) live: Signal<LiveRun>,
    pub(crate) run: Signal<Option<Uuid>>,
}

impl EditorState {
    fn new(id: Uuid) -> Self {
        Self {
            id,
            record: Signal::new(None),
            workflow: Signal::new(Workflow::new("")),
            text: Signal::new(String::new()),
            problems: Signal::new(Vec::new()),
            text_valid: Signal::new(true),
            dirty: Signal::new(false),
            selected: Signal::new(None),
            catalog: Signal::new(NodeCatalog::default()),
            remote: Signal::new(None),
            notice: Signal::new(None),
            live: Signal::new(LiveRun::default()),
            run: Signal::new(None),
        }
    }

    fn problems_in(&self, text: &str, workflow: &Workflow) -> Vec<Problem> {
        let catalog = self
            .catalog
            .peek()
            .without_custom(workflow.definitions.keys());
        if catalog.nodes().is_empty() {
            return Vec::new();
        }
        parameters_lang::check(text, &catalog)
            .iter()
            .map(Problem::from)
            .collect()
    }

    pub(crate) fn load(mut self, record: WorkflowRecord) {
        let text = parameters_lang::print(&record.document);
        let problems = self.problems_in(&text, &record.document);
        let selected = self.selected.peek().clone();
        if selected.is_some_and(|id| !record.document.steps.iter().any(|step| step.id == id)) {
            self.selected.set(None);
        }
        self.problems.set(problems);
        self.text.set(text);
        self.workflow.set(record.document.clone());
        self.text_valid.set(true);
        self.dirty.set(false);
        self.remote.set(None);
        self.record.set(Some(record));
    }

    pub(crate) fn recheck(mut self) {
        let text = self.text.peek().clone();
        let problems = self.problems_in(&text, &self.workflow.peek());
        self.problems.set(problems);
    }

    pub(crate) fn type_text(mut self, text: String) {
        self.dirty.set(true);
        let parsed = parameters_lang::parse(&text);
        if parsed.has_errors() {
            self.problems
                .set(parsed.diagnostics.iter().map(Problem::from).collect());
            self.text_valid.set(false);
            self.text.set(text);
            return;
        }
        let mut workflow = parsed.workflow;
        {
            let previous = self.workflow.peek();
            for step in &workflow.steps {
                if let Some(position) = previous.layout.get(&step.id) {
                    workflow.layout.insert(step.id.clone(), *position);
                }
            }
            for (name, definition) in workflow.definitions.iter_mut() {
                if let Some(old) = previous.definitions.get(name) {
                    definition.layout = old.layout.clone();
                }
            }
        }
        complete_layouts(&mut workflow);
        let problems = self.problems_in(&text, &workflow);
        self.problems.set(problems);
        self.text_valid.set(true);
        self.workflow.set(workflow);
        self.text.set(text);
    }

    pub(crate) fn edit(mut self, change: impl FnOnce(&mut Workflow) -> Result<(), String>) {
        if !*self.text_valid.peek() {
            self.notice.set(Some(Notice::Error(
                "Fix the problems in the text first; the graph is read-only until the text is valid."
                    .into(),
            )));
            return;
        }
        let mut workflow = self.workflow.peek().clone();
        if let Err(message) = change(&mut workflow) {
            self.notice.set(Some(Notice::Error(message)));
            return;
        }
        let text = parameters_lang::print(&workflow);
        let problems = self.problems_in(&text, &workflow);
        self.problems.set(problems);
        self.text.set(text);
        self.workflow.set(workflow);
        self.dirty.set(true);
    }

    pub(crate) fn place(mut self, id: &str, position: Position) {
        self.workflow.write().layout.insert(id.to_owned(), position);
        self.dirty.set(true);
    }

    fn receive(mut self, record: WorkflowRecord) {
        let known = self
            .record
            .peek()
            .as_ref()
            .map(|current| current.summary.version);
        if known.is_some_and(|version| record.summary.version <= version) {
            return;
        }
        let ours = self.workflow.peek().content_hash().to_string();
        if known.is_none() || record.hash == ours || !*self.dirty.peek() {
            let by_agent =
                known.is_some() && record.hash != ours && record.summary.updated_via == Via::Mcp;
            self.load(record);
            if by_agent {
                self.notice.set(Some(Notice::Info(
                    "An AI agent changed this workflow. You are looking at its version.".into(),
                )));
            }
        } else {
            self.remote.set(Some(record));
        }
    }
}

#[expect(
    clippy::large_types_passed_by_value,
    reason = "the state is a set of copyable signal handles that outlive the task"
)]
async fn follow_workflow(mut state: EditorState) {
    if !on_client() {
        return;
    }
    loop {
        match watch_workflow(state.id).await {
            Ok(mut events) => {
                while let Some(Ok(record)) = events.recv().await {
                    state.receive(record);
                }
            }
            Err(Failure(ApiError::NotFound { message })) => {
                state.notice.set(Some(Notice::Error(message)));
                return;
            }
            Err(_) => {}
        }
        pause(2_000).await;
    }
}

#[expect(
    clippy::large_types_passed_by_value,
    reason = "the state is a set of copyable signal handles that outlive the task"
)]
async fn save(mut state: EditorState) {
    let Some(record) = state.record.peek().clone() else {
        return;
    };
    let document = state.workflow.peek().clone();
    match update_workflow(
        state.id,
        record.summary.version,
        Source::Json(to_json(&document)),
    )
    .await
    {
        Ok(saved) => {
            let added = saved.added_nodes.clone();
            state.load(saved.record);
            state.notice.set(Some(Notice::Info(if added.is_empty() {
                "Saved".into()
            } else {
                format!(
                    "Saved, and copied these library nodes into the workflow: {}",
                    added.join(", ")
                )
            })));
        }
        Err(Failure(ApiError::Conflict { .. })) => {
            if let Ok(latest) = get_workflow(state.id).await {
                state.remote.set(Some(latest));
            }
            state.notice.set(Some(Notice::Error(
                "Someone saved a newer version while you were editing. Load it, or keep yours and save again to replace it."
                    .into(),
            )));
        }
        Err(failure) => {
            if let ApiError::Invalid { problems, .. } = &failure.0 {
                state.problems.set(problems.clone());
            }
            state.notice.set(Some(Notice::from(failure)));
        }
    }
}

#[component]
pub(crate) fn Editor(id: Uuid) -> Element {
    rsx! {
        EditorPage { key: "{id}", id }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Both,
    Code,
    Graph,
}

#[component]
fn EditorPage(id: Uuid) -> Element {
    let mut state = use_context_provider(|| EditorState::new(id));
    let mut view = use_signal(|| View::Both);
    let mut reviewing = use_resource(move || async move {
        let hash = state
            .record
            .read()
            .as_ref()
            .map(|record| record.hash.clone());
        match hash {
            Some(_) if on_client() => review_workflow(id).await.ok(),
            _ => None,
        }
    });
    use_future(move || async move {
        if on_client() {
            match node_catalog().await {
                Ok(nodes) => {
                    state.catalog.set(NodeCatalog::new(nodes));
                    state.recheck();
                }
                Err(failure) => state.notice.set(Some(Notice::from(failure))),
            }
        }
    });
    use_future(move || follow_workflow(state));

    let record = state.record.read().clone();
    let Some(record) = record else {
        return rsx! {
            NoticeBar { notice: state.notice }
            p { class: "muted", "Loading…" }
        };
    };
    let dirty = *state.dirty.read();
    let text_valid = *state.text_valid.read();
    let has_errors = state.problems.read().iter().any(Problem::is_error);
    let view_class = match view() {
        View::Both => "view-both",
        View::Code => "view-code",
        View::Graph => "view-graph",
    };
    let review = reviewing.read().clone().flatten();

    rsx! {
        div { class: "editor",
            div { class: "heading",
                WorkflowTitle {}
                span { class: "muted", "version {record.summary.version}" }
                if dirty {
                    span { class: "badge", "unsaved" }
                }
                if record.summary.updated_via == Via::Mcp {
                    span { class: "badge agent", title: "Last saved by an AI agent", "agent" }
                }
                div { class: "actions",
                    button {
                        class: "primary",
                        disabled: !dirty || !text_valid,
                        onclick: move |_| save(state),
                        "Save"
                    }
                    button {
                        disabled: !text_valid,
                        onclick: move |_| {
                            let text = parameters_lang::print(&state.workflow.peek());
                            state.text.set(text);
                        },
                        "Format"
                    }
                    button {
                        onclick: move |_| {
                            let workflow = state.workflow.peek().clone();
                            download(&format!("{}.json", file_name(&workflow.name)), &to_json(&workflow), "application/json");
                        },
                        "Export JSON"
                    }
                    div { class: "segmented", role: "group", "aria-label": "Layout",
                        for (choice , label) in [(View::Code, "Code"), (View::Both, "Both"), (View::Graph, "Graph")] {
                            button {
                                class: if view() == choice { "selected" },
                                "aria-pressed": view() == choice,
                                onclick: move |_| view.set(choice),
                                "{label}"
                            }
                        }
                    }
                }
            }
            NoticeBar { notice: state.notice }
            RemoteBanner {}
            if let Some(review) = review {
                ReviewPanel {
                    review,
                    on_approved: move |()| reviewing.restart(),
                }
            }
            div { class: "editor-body {view_class}",
                section { class: "pane code-pane", "aria-label": "Code", code::CodeEditor {} }
                section { class: "pane graph-pane", "aria-label": "Graph", graph::GraphCanvas {} }
                aside { class: "pane inspector", inspector::Inspector {} }
            }
            RunPanel { blocked: dirty || has_errors }
        }
        Link { class: "muted", to: Route::Workflows {}, "← All workflows" }
    }
}

#[component]
fn WorkflowTitle() -> Element {
    let state = use_context::<EditorState>();
    let name = state.workflow.read().name.clone();
    rsx! {
        h1 { "{name}" }
    }
}

fn file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|character| {
            if character.is_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('-');
    if trimmed.is_empty() {
        "workflow".into()
    } else {
        trimmed.to_owned()
    }
}

#[component]
fn RemoteBanner() -> Element {
    let mut state = use_context::<EditorState>();
    let Some(remote) = state.remote.read().clone() else {
        return rsx! {};
    };
    let who = if remote.summary.updated_via == Via::Mcp {
        "an AI agent"
    } else {
        "someone else"
    };
    let version = remote.summary.version;
    rsx! {
        div { class: "notice warning", role: "alert",
            span { "Version {version} was saved by {who} while you were editing." }
            button { onclick: move |_| state.load(remote.clone()), "Load version {version}" }
            button {
                class: "quiet",
                onclick: move |_| {
                    if let Some(record) = state.record.write().as_mut() {
                        record.summary.version = version;
                    }
                    state.remote.set(None);
                },
                "Keep mine"
            }
        }
    }
}

#[component]
fn ReviewPanel(review: Review, on_approved: EventHandler<()>) -> Element {
    let mut state = use_context::<EditorState>();
    if review.privileged.is_empty() && review.hosts.is_empty() && review.secrets.is_empty() {
        return rsx! {};
    }
    let hash = review.hash.clone();
    let id = state.id;
    rsx! {
        details { class: if review.needs_approval() { "panel review attention" } else { "panel review" }, open: review.needs_approval(),
            summary {
                if review.needs_approval() {
                    "This version needs your approval before it can run"
                } else if review.privileged.is_empty() {
                    "What this workflow uses"
                } else {
                    "Approved: what this workflow can do"
                }
            }
            if !review.privileged.is_empty() {
                p { "Runs commands or uses files on this machine:" }
                ul {
                    for step in review.privileged.clone() {
                        li { key: "{step.path}", code { "{step.path}" } " uses " code { "{step.uses}" } }
                    }
                }
            }
            if !review.hosts.is_empty() {
                p { "Contacts: " "{review.hosts.join(\", \")}" }
            }
            if !review.dynamic_urls.is_empty() {
                p { "Builds addresses while it runs in: " "{review.dynamic_urls.join(\", \")}" }
            }
            if !review.secrets.is_empty() {
                p { "Reads secrets: " "{review.secrets.join(\", \")}" }
            }
            if review.needs_approval() {
                button {
                    class: "primary",
                    onclick: move |_| {
                        let hash = hash.clone();
                        async move {
                            match approve_workflow(id, hash).await {
                                Ok(_) => {
                                    state.notice.set(Some(Notice::Info("Approved this version".into())));
                                    on_approved.call(());
                                }
                                Err(failure) => state.notice.set(Some(Notice::from(failure))),
                            }
                        }
                    },
                    "Approve this version"
                }
            }
        }
    }
}

fn input_value(value_type: ValueType, text: &str) -> Result<Value, String> {
    match value_type {
        ValueType::Text | ValueType::Path | ValueType::Secret => Ok(Value::String(text.to_owned())),
        ValueType::Any => {
            Ok(serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_owned())))
        }
        other => {
            serde_json::from_str(text).map_err(|_| format!("must be {other}, written as JSON"))
        }
    }
}

#[component]
fn RunPanel(blocked: bool) -> Element {
    let mut state = use_context::<EditorState>();
    let mut values = use_signal(Map::<String, Value>::new);
    let inputs: Vec<_> = state
        .workflow
        .read()
        .inputs
        .iter()
        .filter(|input| input.value_type != ValueType::Secret)
        .cloned()
        .collect();
    let live = state.live.read().clone();
    let run_id = *state.run.read();

    let start = move |_| async move {
        match start_run(state.id, values.peek().clone()).await {
            Ok(summary) => {
                state.live.set(LiveRun::default());
                state.run.set(Some(summary.id));
                spawn(follow_run(summary.id, state.live));
            }
            Err(Failure(ApiError::ApprovalRequired { message, .. })) => {
                state.notice.set(Some(Notice::Error(message)));
            }
            Err(failure) => state.notice.set(Some(Notice::from(failure))),
        }
    };

    rsx! {
        section { class: "panel run-panel",
            h2 { "Run" }
            if !inputs.is_empty() {
                div { class: "inline-form",
                    for input in inputs {
                        label { key: "{input.name}",
                            "{input.name} ({input.value_type})"
                            input {
                                placeholder: input.default.as_ref().map(ToString::to_string).unwrap_or_default(),
                                onchange: move |event| {
                                    let text = event.value();
                                    let mut current = values.write();
                                    if text.is_empty() {
                                        current.remove(&input.name);
                                        return;
                                    }
                                    match input_value(input.value_type, &text) {
                                        Ok(value) => {
                                            current.insert(input.name.clone(), value);
                                        }
                                        Err(message) => state.notice.set(Some(Notice::Error(format!("{}: {message}", input.name)))),
                                    }
                                },
                            }
                        }
                    }
                }
            }
            div { class: "actions",
                button {
                    class: "primary",
                    disabled: blocked,
                    title: if blocked { "Save the workflow and fix its problems before running it" },
                    onclick: start,
                    "Run"
                }
                if let Some(run) = &live.run {
                    RunBadge { status: run.summary.status }
                }
                if let Some(id) = run_id {
                    Link { to: Route::RunPage { id }, "Open run" }
                }
            }
            if let Some(run) = &live.run {
                for error in run.errors.clone() {
                    p { class: "notice error", "{error}" }
                }
                if !run.outputs.is_empty() {
                    pre { class: "output", "{serde_json::to_string_pretty(&run.outputs).unwrap_or_default()}" }
                }
            }
            if run_id.is_some() {
                details {
                    summary { "Steps and log" }
                    RunDetails { live }
                }
            }
        }
    }
}
