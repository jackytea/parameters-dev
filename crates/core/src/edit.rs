use crate::document::{FieldValue, Position, Step, Workflow};
use crate::expr::{self, Expr, ExprKind, TextPart};
use crate::graph::DependencyGraph;
use crate::layout::complete_layout;
use crate::name::check_name;

const FALLBACK_ID: &str = "task";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EditError {
    #[error("there is no step named `{0}`")]
    UnknownStep(String),
    #[error(
        "`{0}` is not a valid name: use lowercase letters, digits and underscores, and avoid keywords"
    )]
    InvalidName(String),
    #[error("the name `{0}` is already used by a step or an input")]
    NameTaken(String),
    #[error("`{step}` cannot wait for `{before}`, because `{before}` already waits for it")]
    Cycle { before: String, step: String },
    #[error("a step cannot wait for itself")]
    SelfDependency,
}

pub fn add_step(workflow: &mut Workflow, uses: &str, position: Option<Position>) -> String {
    let base = uses.rsplit('.').next().unwrap_or(uses).to_ascii_lowercase();
    let base = if check_name(&base).is_ok() {
        base
    } else {
        FALLBACK_ID.to_owned()
    };
    let id = free_id(workflow, &base);
    workflow.steps.push(Step::new(id.clone(), uses));
    match position {
        Some(position) => {
            workflow.layout.insert(id.clone(), position);
        }
        None => complete_layout(&workflow.steps, &mut workflow.layout),
    }
    id
}

pub fn remove_step(workflow: &mut Workflow, id: &str) -> Result<(), EditError> {
    let index = step_index(workflow, id)?;
    workflow.steps.remove(index);
    for step in &mut workflow.steps {
        step.after.retain(|before| before != id);
    }
    workflow.layout.remove(id);
    Ok(())
}

pub fn rename_step(workflow: &mut Workflow, old: &str, new: &str) -> Result<(), EditError> {
    let index = step_index(workflow, old)?;
    if old == new {
        return Ok(());
    }
    check_name(new).map_err(|_| EditError::InvalidName(new.to_owned()))?;
    if is_taken(workflow, new) {
        return Err(EditError::NameTaken(new.to_owned()));
    }
    new.clone_into(&mut workflow.steps[index].id);
    for step in &mut workflow.steps {
        for before in &mut step.after {
            if before == old {
                new.clone_into(before);
            }
        }
        for value in step.with.values_mut() {
            if let FieldValue::Expression(source) = value {
                rename_in_source(source, old, new);
            }
        }
        if let Some(condition) = &mut step.when {
            rename_in_source(condition, old, new);
        }
    }
    for output in &mut workflow.outputs {
        if let FieldValue::Expression(source) = &mut output.value {
            rename_in_source(source, old, new);
        }
    }
    workflow.layout.rename_key(old, new);
    Ok(())
}

pub fn add_dependency(workflow: &mut Workflow, before: &str, step: &str) -> Result<(), EditError> {
    if before == step {
        return Err(EditError::SelfDependency);
    }
    step_index(workflow, before)?;
    let index = step_index(workflow, step)?;
    if workflow.steps[index]
        .after
        .iter()
        .any(|name| name == before)
    {
        return Ok(());
    }
    workflow.steps[index].after.push(before.to_owned());
    if DependencyGraph::of(workflow.graph())
        .topological_order()
        .is_err()
    {
        workflow.steps[index].after.pop();
        return Err(EditError::Cycle {
            before: before.to_owned(),
            step: step.to_owned(),
        });
    }
    Ok(())
}

pub fn remove_dependency(workflow: &mut Workflow, before: &str, step: &str) -> bool {
    let Some(target) = workflow
        .steps
        .iter_mut()
        .find(|candidate| candidate.id == step)
    else {
        return false;
    };
    let count = target.after.len();
    target.after.retain(|name| name != before);
    target.after.len() != count
}

fn free_id(workflow: &Workflow, base: &str) -> String {
    if !is_taken(workflow, base) {
        return base.to_owned();
    }
    let mut number = 2_u32;
    loop {
        let candidate = format!("{base}_{number}");
        if !is_taken(workflow, &candidate) {
            return candidate;
        }
        number += 1;
    }
}

fn step_index(workflow: &Workflow, id: &str) -> Result<usize, EditError> {
    workflow
        .steps
        .iter()
        .position(|step| step.id == id)
        .ok_or_else(|| EditError::UnknownStep(id.to_owned()))
}

fn is_taken(workflow: &Workflow, name: &str) -> bool {
    workflow.steps.iter().any(|step| step.id == name)
        || workflow.inputs.iter().any(|input| input.name == name)
}

fn rename_in_source(source: &mut String, old: &str, new: &str) {
    let Ok(mut parsed) = expr::parse(source) else {
        return;
    };
    if rename_in_expr(&mut parsed, old, new) {
        *source = expr::print(&parsed);
    }
}

fn rename_in_expr(expression: &mut Expr, old: &str, new: &str) -> bool {
    let rename = |name: &mut String| {
        if name == old {
            new.clone_into(name);
            true
        } else {
            false
        }
    };
    match &mut expression.kind {
        ExprKind::Reference(reference) => rename(&mut reference.segments[0].name),
        ExprKind::State { step, .. } => rename(&mut step.name),
        ExprKind::Text(parts) => parts.iter_mut().fold(false, |changed, part| match part {
            TextPart::Reference(reference) => rename(&mut reference.segments[0].name) | changed,
            TextPart::Literal(_) => changed,
        }),
        ExprKind::List(items) => items.iter_mut().fold(false, |changed, item| {
            rename_in_expr(item, old, new) | changed
        }),
        ExprKind::Object(entries) => entries.iter_mut().fold(false, |changed, (_, value)| {
            rename_in_expr(value, old, new) | changed
        }),
        ExprKind::Not(inner) => rename_in_expr(inner, old, new),
        ExprKind::Binary { left, right, .. } => {
            rename_in_expr(left, old, new) | rename_in_expr(right, old, new)
        }
        ExprKind::Boolean(_) | ExprKind::Number(_) | ExprKind::Secret(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Output;

    fn workflow() -> Workflow {
        let mut workflow = Workflow::new("Edit");
        let mut build = Step::new("build", "shell.run");
        build
            .with
            .insert("command", FieldValue::Literal("make".into()));
        let mut deploy = Step::new("deploy", "http.request");
        deploy.after.push("build".into());
        deploy.when = Some("build succeeded and not build.skipped".into());
        deploy.with.insert(
            "url",
            FieldValue::expression("\"https://x.example/{build.stdout}\""),
        );
        deploy.with.insert(
            "body",
            FieldValue::expression("{ log: build.stdout, keep: [rebuild.stdout] }"),
        );
        workflow.steps = vec![build, deploy];
        workflow.outputs.push(Output {
            name: "result".into(),
            value_type: None,
            value: FieldValue::expression("build.exit_code"),
        });
        workflow.layout.insert("build", Position::new(0, 0));
        workflow
    }

    #[test]
    fn renaming_rewrites_every_reference() {
        let mut edited = workflow();
        rename_step(&mut edited, "build", "compile").unwrap();
        let deploy = &edited.steps[1];
        assert_eq!(edited.steps[0].id, "compile");
        assert_eq!(deploy.after, ["compile"]);
        assert_eq!(
            deploy.when.as_deref(),
            Some("compile succeeded and not compile.skipped")
        );
        assert_eq!(
            deploy.with.get("url"),
            Some(&FieldValue::expression(
                "\"https://x.example/{compile.stdout}\""
            ))
        );
        assert_eq!(
            deploy.with.get("body"),
            Some(&FieldValue::expression(
                "{ log: compile.stdout, keep: [rebuild.stdout] }"
            ))
        );
        assert_eq!(
            edited.outputs[0].value,
            FieldValue::expression("compile.exit_code")
        );
        assert!(edited.layout.contains_key("compile") && !edited.layout.contains_key("build"));
    }

    #[test]
    fn renaming_rejects_bad_and_taken_names() {
        let mut edited = workflow();
        assert_eq!(
            rename_step(&mut edited, "build", "deploy"),
            Err(EditError::NameTaken("deploy".into()))
        );
        assert_eq!(
            rename_step(&mut edited, "build", "after"),
            Err(EditError::InvalidName("after".into()))
        );
        assert_eq!(
            rename_step(&mut edited, "nope", "x"),
            Err(EditError::UnknownStep("nope".into()))
        );
        assert_eq!(edited, workflow());
    }

    #[test]
    fn dependencies_cannot_form_cycles() {
        let mut edited = workflow();
        assert_eq!(
            add_dependency(&mut edited, "deploy", "build"),
            Err(EditError::Cycle {
                before: "deploy".into(),
                step: "build".into()
            })
        );
        assert_eq!(edited, workflow());
        assert_eq!(
            add_dependency(&mut edited, "build", "build"),
            Err(EditError::SelfDependency)
        );
        add_dependency(&mut edited, "build", "deploy").unwrap();
        assert_eq!(edited.steps[1].after, ["build"]);
        assert!(remove_dependency(&mut edited, "build", "deploy"));
        assert!(!remove_dependency(&mut edited, "build", "deploy"));
        assert_eq!(edited.steps[1].after, Vec::<String>::new());
    }

    #[test]
    fn adding_and_removing_steps_keeps_names_unique_and_layout_complete() {
        let mut edited = workflow();
        let first = add_step(&mut edited, "log.write", None);
        let second = add_step(&mut edited, "log.write", Some(Position::new(5, 6)));
        let odd = add_step(&mut edited, "acme.After", None);
        assert_eq!((first.as_str(), second.as_str()), ("write", "write_2"));
        assert_eq!(odd, "task");
        assert_eq!(edited.layout.get("write_2"), Some(&Position::new(5, 6)));
        assert!(
            edited
                .steps
                .iter()
                .all(|step| edited.layout.contains_key(&step.id))
        );
        remove_step(&mut edited, "build").unwrap();
        assert_eq!(edited.steps[0].after, Vec::<String>::new());
        assert!(!edited.layout.contains_key("build"));
    }
}
