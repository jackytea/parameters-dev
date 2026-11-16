use dioxus::prelude::*;
use parameters_api::PortInfo;
use parameters_core::edit::{remove_dependency, remove_step, rename_step};
use parameters_core::expr::{self, print_value};
use parameters_core::{Duration, FieldValue, Step, Workflow};
use parameters_lang::literal_value;

use crate::ui::editor::EditorState;

pub(crate) fn field_value(text: &str) -> Result<Option<FieldValue>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let parsed = expr::parse(text).map_err(|error| error.message)?;
    Ok(Some(match literal_value(&parsed) {
        Some(value) => FieldValue::Literal(value),
        None => FieldValue::expression(text),
    }))
}

fn shown(value: &FieldValue) -> String {
    match value {
        FieldValue::Literal(value) => print_value(value),
        FieldValue::Expression(source) => source.clone(),
    }
}

fn step_mut<'a>(workflow: &'a mut Workflow, id: &str) -> Result<&'a mut Step, String> {
    workflow
        .steps
        .iter_mut()
        .find(|step| step.id == id)
        .ok_or_else(|| format!("there is no step named `{id}`"))
}

#[component]
pub(crate) fn Inspector() -> Element {
    let state = use_context::<EditorState>();
    let Some(id) = state.selected.read().clone() else {
        return rsx! {
            p { class: "muted", "Select a step in the graph to edit it here." }
        };
    };
    let Some(step) = state
        .workflow
        .read()
        .steps
        .iter()
        .find(|step| step.id == id)
        .cloned()
    else {
        return rsx! {
            p { class: "muted", "Select a step in the graph to edit it here." }
        };
    };
    let editable = *state.text_valid.read();
    rsx! {
        StepForm { key: "{step.id}", step, editable }
    }
}

#[component]
fn StepForm(step: Step, editable: bool) -> Element {
    let mut state = use_context::<EditorState>();
    let id = step.id.clone();
    let info = state.catalog.read().info(&step.uses).cloned();
    let ports: Vec<PortInfo> = info
        .as_ref()
        .map(|info| info.inputs.clone())
        .unwrap_or_default();
    let extra: Vec<(String, FieldValue)> = step
        .with
        .iter()
        .filter(|(name, _)| !ports.iter().any(|port| port.name == *name))
        .map(|(name, value)| (name.to_owned(), value.clone()))
        .collect();
    let types: Vec<String> = state
        .catalog
        .read()
        .nodes()
        .iter()
        .map(|node| node.type_name.clone())
        .collect();

    let rename = {
        let id = id.clone();
        move |event: FormEvent| {
            let new = event.value().trim().to_owned();
            let mut renamed = false;
            state.edit(|workflow| {
                rename_step(workflow, &id, &new).map_err(|error| error.to_string())?;
                renamed = true;
                Ok(())
            });
            if renamed {
                state.selected.set(Some(new));
            }
        }
    };
    let change_type = {
        let id = id.clone();
        move |event: FormEvent| {
            let uses = event.value();
            state.edit(|workflow| {
                step_mut(workflow, &id)?.uses = uses;
                Ok(())
            });
        }
    };
    let set_when = {
        let id = id.clone();
        move |event: FormEvent| {
            let text = event.value().trim().to_owned();
            state.edit(|workflow| {
                if !text.is_empty() {
                    expr::parse(&text).map_err(|error| error.message)?;
                }
                step_mut(workflow, &id)?.when = (!text.is_empty()).then_some(text);
                Ok(())
            });
        }
    };
    let set_retry = {
        let id = id.clone();
        move |event: FormEvent| {
            let text = event.value();
            state.edit(|workflow| {
                let retry = if text.trim().is_empty() {
                    None
                } else {
                    Some(
                        text.trim()
                            .parse::<u32>()
                            .map_err(|_| "retries must be a whole number".to_owned())?,
                    )
                };
                step_mut(workflow, &id)?.retry = retry.filter(|count| *count > 0);
                Ok(())
            });
        }
    };
    let set_timeout = {
        let id = id.clone();
        move |event: FormEvent| {
            let text = event.value().trim().to_owned();
            state.edit(|workflow| {
                let timeout = if text.is_empty() {
                    None
                } else {
                    Some(Duration::parse(&text)?)
                };
                step_mut(workflow, &id)?.timeout = timeout;
                Ok(())
            });
        }
    };
    let set_note = {
        let id = id.clone();
        move |event: FormEvent| {
            let text = event.value().trim().to_owned();
            state.edit(|workflow| {
                step_mut(workflow, &id)?.note = (!text.is_empty()).then_some(text);
                Ok(())
            });
        }
    };
    let delete = {
        let id = id.clone();
        move |_| {
            state.edit(|workflow| remove_step(workflow, &id).map_err(|error| error.to_string()));
            state.selected.set(None);
        }
    };

    rsx! {
        fieldset { class: "step-form", disabled: !editable,
            legend { "Step" }
            label {
                "Name"
                input { value: "{step.id}", onchange: rename }
            }
            label {
                "Node"
                select { value: "{step.uses}", onchange: change_type,
                    if !types.contains(&step.uses) {
                        option { value: "{step.uses}", "{step.uses} (unknown)" }
                    }
                    for node_type in types {
                        option { key: "{node_type}", value: "{node_type}", "{node_type}" }
                    }
                }
            }
            if let Some(description) = info.as_ref().and_then(|info| info.description.clone()) {
                p { class: "muted", "{description}" }
            }
            if info.as_ref().is_some_and(|info| info.privileged) {
                p { class: "badge warning", "Needs approval: runs commands or uses files" }
            }
            for port in ports {
                FieldInput {
                    key: "{port.name}",
                    step: id.clone(),
                    name: port.name.clone(),
                    hint: format!("{}{}", port.value_type, if port.required { ", required" } else { "" }),
                    value: step.with.get(&port.name).map(shown).unwrap_or_default(),
                }
            }
            for (name , value) in extra {
                FieldInput {
                    key: "{name}",
                    step: id.clone(),
                    name: name.clone(),
                    hint: "not an input of this node".to_owned(),
                    value: shown(&value),
                }
            }
            if !step.after.is_empty() {
                div { class: "chips",
                    span { class: "muted", "Waits for " }
                    for before in step.after.clone() {
                        button {
                            key: "{before}",
                            class: "chip",
                            title: "Stop waiting for {before}",
                            onclick: {
                                let id = id.clone();
                                move |_| {
                                    let before = before.clone();
                                    let id = id.clone();
                                    state.edit(move |workflow| {
                                        remove_dependency(workflow, &before, &id);
                                        Ok(())
                                    });
                                }
                            },
                            "{before} ×"
                        }
                    }
                }
            }
            label {
                "Run when"
                input {
                    placeholder: "build succeeded",
                    value: "{step.when.clone().unwrap_or_default()}",
                    onchange: set_when,
                }
            }
            div { class: "inline-form",
                label {
                    "Retries"
                    input {
                        r#type: "number",
                        min: "0",
                        max: "100",
                        value: "{step.retry.unwrap_or(0)}",
                        onchange: set_retry,
                    }
                }
                label {
                    "Timeout"
                    input {
                        placeholder: "10m",
                        value: "{step.timeout.map(|timeout| timeout.to_string()).unwrap_or_default()}",
                        onchange: set_timeout,
                    }
                }
            }
            label {
                "Note"
                input { value: "{step.note.clone().unwrap_or_default()}", onchange: set_note }
            }
            button { class: "danger", onclick: delete, "Delete step" }
        }
    }
}

#[component]
fn FieldInput(step: String, name: String, hint: String, value: String) -> Element {
    let state = use_context::<EditorState>();
    rsx! {
        label {
            span { "{name} " span { class: "muted", "({hint})" } }
            input {
                class: "mono",
                spellcheck: "false",
                value: "{value}",
                onchange: move |event| {
                    let text = event.value();
                    let (step, name) = (step.clone(), name.clone());
                    state.edit(move |workflow| {
                        let value = field_value(&text)?;
                        let target = step_mut(workflow, &step)?;
                        match value {
                            Some(value) => {
                                target.with.insert(name, value);
                            }
                            None => {
                                target.with.remove(&name);
                            }
                        }
                        Ok(())
                    });
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn fields_become_literals_or_expressions() {
        assert_eq!(field_value("  "), Ok(None));
        assert_eq!(
            field_value("\"hi\""),
            Ok(Some(FieldValue::Literal(json!("hi"))))
        );
        assert_eq!(
            field_value("[1, true]"),
            Ok(Some(FieldValue::Literal(json!([1, true]))))
        );
        assert_eq!(
            field_value("build.stdout"),
            Ok(Some(FieldValue::expression("build.stdout")))
        );
        assert_eq!(
            field_value("\"{build.stdout}/dist\""),
            Ok(Some(FieldValue::expression("\"{build.stdout}/dist\"")))
        );
        assert!(field_value("\"unclosed").is_err());
        assert_eq!(shown(&FieldValue::Literal(json!({"a": 1}))), "{ a: 1 }");
    }
}
