use dioxus::prelude::*;
use parameters_api::{ApiError, OffsetDateTime, Problem, RunState, StepState};

use crate::failure::Failure;

pub(crate) const fn on_client() -> bool {
    !cfg!(feature = "server")
}

pub(crate) fn script(code: &str) {
    if on_client() {
        let _ = document::eval(code);
    }
}

pub(crate) async fn pause(milliseconds: u32) {
    if on_client() {
        let _ = document::eval(&format!(
            "await new Promise((resolve) => setTimeout(resolve, {milliseconds})); return 0;"
        ))
        .join::<i32>()
        .await;
    }
}

pub(crate) fn download(file_name: &str, content: &str, mime_type: &str) {
    let content = serde_json::to_string(content).unwrap_or_default();
    script(&format!(
        "const url = URL.createObjectURL(new Blob([{content}], {{ type: {mime_type:?} }}));
         const link = document.createElement('a');
         link.href = url;
         link.download = {file_name:?};
         link.click();
         URL.revokeObjectURL(url);"
    ));
}

pub(crate) fn timestamp(at: OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02} UTC",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute()
    )
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Notice {
    Info(String),
    Error(String),
}

impl Notice {
    pub(crate) fn failed(failure: &Failure) -> Self {
        Self::Error(failure.to_string())
    }
}

impl From<Failure> for Notice {
    fn from(failure: Failure) -> Self {
        Self::failed(&failure)
    }
}

#[component]
pub(crate) fn NoticeBar(notice: Signal<Option<Notice>>) -> Element {
    let current = notice.read().clone();
    match current {
        None => rsx! {},
        Some(Notice::Info(text)) => rsx! {
            div { class: "notice", role: "status",
                span { "{text}" }
                button { class: "quiet", onclick: move |_| notice.set(None), "Dismiss" }
            }
        },
        Some(Notice::Error(text)) => rsx! {
            div { class: "notice error", role: "alert",
                span { "{text}" }
                button { class: "quiet", onclick: move |_| notice.set(None), "Dismiss" }
            }
        },
    }
}

#[component]
pub(crate) fn Problems(problems: Vec<Problem>) -> Element {
    if problems.is_empty() {
        return rsx! {};
    }
    rsx! {
        ul { class: "problems",
            for problem in problems {
                li { class: if problem.is_error() { "problem error" } else { "problem" },
                    if let Some(range) = problem.range {
                        span { class: "position", "{range.start.line}:{range.start.column}" }
                    }
                    span { class: "code-id", "{problem.code}" }
                    span { "{problem.message}" }
                    if let Some(suggestion) = &problem.suggestion {
                        span { class: "muted", " Did you mean `{suggestion}`?" }
                    }
                }
            }
        }
    }
}

pub(crate) fn failure_problems(failure: &Failure) -> Vec<Problem> {
    match &failure.0 {
        ApiError::Invalid { problems, .. } => problems.clone(),
        _ => Vec::new(),
    }
}

#[component]
pub(crate) fn RunBadge(status: RunState) -> Element {
    let name = status.name();
    rsx! { span { class: "badge status-{name}", "{name}" } }
}

#[component]
pub(crate) fn StepBadge(status: StepState) -> Element {
    let name = status.name();
    rsx! { span { class: "badge status-{name}", "{name}" } }
}

#[component]
pub(crate) fn Loading() -> Element {
    rsx! { p { class: "muted", "Loading…" } }
}
