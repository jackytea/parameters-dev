use dioxus::prelude::*;
use parameters_lang::{TokenClass, highlight};

use crate::ui::common::Problems;
use crate::ui::editor::EditorState;

const fn class_of(class: TokenClass) -> &'static str {
    match class {
        TokenClass::Keyword => "token-keyword",
        TokenClass::Name => "token-name",
        TokenClass::NodeType => "token-type",
        TokenClass::Text => "token-text",
        TokenClass::Interpolation => "token-interpolation",
        TokenClass::Number => "token-number",
        TokenClass::Operator => "token-operator",
        TokenClass::Punctuation => "token-punctuation",
        TokenClass::Invalid => "token-invalid",
    }
}

pub(crate) fn pieces(source: &str) -> Vec<(&'static str, &str)> {
    let mut pieces = Vec::new();
    let mut at = 0;
    for token in highlight(source) {
        let (start, end) = (token.span.start, token.span.end);
        if start < at {
            continue;
        }
        if let Some(plain) = source.get(at..start).filter(|plain| !plain.is_empty()) {
            pieces.push(("", plain));
        }
        if let Some(text) = source.get(start..end) {
            pieces.push((class_of(token.class), text));
            at = end;
        } else {
            at = start;
        }
    }
    if let Some(rest) = source.get(at..).filter(|rest| !rest.is_empty()) {
        pieces.push(("", rest));
    }
    pieces
}

#[component]
pub(crate) fn CodeEditor() -> Element {
    let state = use_context::<EditorState>();
    let text = state.text.read().clone();
    let rows = text.lines().count() + 2;
    let columns = text
        .lines()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0)
        + 2;
    let valid = *state.text_valid.read();
    rsx! {
        div { class: "code",
            pre { class: "code-highlight", "aria-hidden": "true",
                for (index , (class , piece)) in pieces(&text).into_iter().enumerate() {
                    span { key: "{index}", class, "{piece}" }
                }
                "\n"
            }
            textarea {
                class: "code-input",
                "aria-label": "Workflow source",
                spellcheck: "false",
                autocomplete: "off",
                autocapitalize: "off",
                rows: "{rows}",
                cols: "{columns}",
                wrap: "off",
                value: "{text}",
                oninput: move |event| state.type_text(event.value()),
            }
        }
        if !valid {
            p { class: "notice warning", "The graph is read-only until these problems are fixed." }
        }
        Problems { problems: state.problems.read().clone() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces_cover_the_source_exactly() {
        let source =
            "workflow \"Hi\"\n\nstep say uses log.write\n  message is \"Hello, {name}! é\"\n";
        let joined: String = pieces(source).iter().map(|(_, text)| *text).collect();
        assert_eq!(joined, source);
        assert!(
            pieces(source)
                .iter()
                .any(|(class, text)| *class == "token-keyword" && *text == "step")
        );
    }
}
