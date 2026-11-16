use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parameters_core::expr;
use parameters_core::{
    Definition, DependencyGraph, Diagnostic, FieldValue, Graph, OrderedMap, ValueType, Workflow,
    validate,
};
use serde_json::Value;

use crate::eval::{Compiled, Root, compile};
use crate::node::{Node, Values};
use crate::registry::Registry;

pub const DEFAULT_CONCURRENCY: u32 = 8;

#[derive(Debug, thiserror::Error)]
pub enum CompileError {
    #[error("the workflow is not valid: {}", .0.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))]
    Invalid(Vec<Diagnostic>),
    #[error("{0}")]
    Unsupported(String),
    #[error("this server does not run steps that run commands or use files: {}", .0.join(", "))]
    NotAllowed(Vec<String>),
}

pub struct Plan {
    pub(crate) root: Arc<GraphPlan>,
    pub(crate) definitions: Arc<HashMap<String, Arc<GraphPlan>>>,
    pub(crate) concurrency: usize,
}

impl Plan {
    pub fn inputs(&self) -> impl Iterator<Item = (&str, ValueType, Option<&Value>)> {
        self.root.inputs.iter().map(|input| {
            (
                input.name.as_str(),
                input.value_type,
                input.default.as_ref(),
            )
        })
    }

    #[must_use]
    pub fn check_inputs(&self, provided: &Values) -> Vec<String> {
        let mut errors = Vec::new();
        for input in &self.root.inputs {
            let name = &input.name;
            match (provided.get(name), input.value_type) {
                (Some(_), ValueType::Secret) => errors.push(format!(
                    "the secret input `{name}` cannot be given directly; it is read from the secret store"
                )),
                (None, ValueType::Secret) => {}
                (Some(value), value_type) if !matches_type(value, value_type) => {
                    errors.push(format!("the input `{name}` must be {value_type}"));
                }
                (None, _) if input.default.is_none() => {
                    errors.push(format!("the input `{name}` is required"));
                }
                _ => {}
            }
        }
        let known: HashSet<&str> = self
            .root
            .inputs
            .iter()
            .map(|input| input.name.as_str())
            .collect();
        errors.extend(
            provided
                .keys()
                .filter(|name| !known.contains(name.as_str()))
                .map(|name| format!("the workflow has no input `{name}`")),
        );
        errors
    }
}

pub(crate) const fn matches_type(value: &Value, value_type: ValueType) -> bool {
    matches!(
        (value_type, value),
        (ValueType::Any, _)
            | (
                ValueType::Text | ValueType::Path | ValueType::Secret,
                Value::String(_)
            )
            | (ValueType::Number, Value::Number(_))
            | (ValueType::Boolean, Value::Bool(_))
            | (ValueType::List, Value::Array(_))
            | (ValueType::Object, Value::Object(_))
    )
}

pub(crate) struct GraphPlan {
    pub(crate) inputs: Vec<InputPlan>,
    pub(crate) steps: Vec<StepPlan>,
    pub(crate) step_ids: Vec<String>,
    pub(crate) graph: DependencyGraph,
    pub(crate) outputs: Vec<(String, Compiled)>,
}

pub(crate) struct InputPlan {
    pub(crate) name: String,
    pub(crate) value_type: ValueType,
    pub(crate) default: Option<Value>,
}

pub(crate) struct StepPlan {
    pub(crate) action: Action,
    pub(crate) fields: Vec<(String, Compiled)>,
    pub(crate) when: Option<Compiled>,
    pub(crate) attempts: u32,
    pub(crate) timeout: Option<parameters_core::Duration>,
}

pub(crate) enum Action {
    Node(Arc<dyn Node>),
    Composite(String),
}

pub fn compile_plan(workflow: &Workflow, registry: &Registry) -> Result<Plan, CompileError> {
    let errors: Vec<Diagnostic> = validate(workflow, registry)
        .into_iter()
        .filter(Diagnostic::is_error)
        .collect();
    if !errors.is_empty() {
        return Err(CompileError::Invalid(errors));
    }
    let definitions = workflow
        .definitions
        .iter()
        .map(|(name, definition)| {
            Ok((
                name.to_owned(),
                Arc::new(graph_plan(
                    definition.graph(),
                    registry,
                    &workflow.definitions,
                )?),
            ))
        })
        .collect::<Result<HashMap<_, _>, CompileError>>()?;
    let concurrency = workflow.concurrency.unwrap_or(DEFAULT_CONCURRENCY);
    Ok(Plan {
        root: Arc::new(graph_plan(
            workflow.graph(),
            registry,
            &workflow.definitions,
        )?),
        definitions: Arc::new(definitions),
        concurrency: usize::try_from(concurrency).unwrap_or(usize::MAX),
    })
}

fn graph_plan(
    graph: Graph<'_>,
    registry: &Registry,
    definitions: &OrderedMap<Definition>,
) -> Result<GraphPlan, CompileError> {
    let names: HashMap<&str, Root> = graph
        .inputs
        .iter()
        .enumerate()
        .map(|(index, input)| (input.name.as_str(), Root::Input(index)))
        .chain(
            graph
                .steps
                .iter()
                .enumerate()
                .map(|(index, step)| (step.id.as_str(), Root::Step(index))),
        )
        .collect();
    let expression = |source: &str| {
        let parsed =
            expr::parse(source).map_err(|error| CompileError::Unsupported(error.message))?;
        compile(&parsed, &names).map_err(CompileError::Unsupported)
    };
    let value = |value: &FieldValue| match value {
        FieldValue::Literal(literal) => Ok(Compiled::Value(literal.clone())),
        FieldValue::Expression(source) => expression(source),
    };
    let steps = graph
        .steps
        .iter()
        .map(|step| {
            let action = match registry.node(&step.uses) {
                Some(node) => Action::Node(Arc::clone(node)),
                None if definitions.contains_key(&step.uses) => {
                    Action::Composite(step.uses.clone())
                }
                None => {
                    return Err(CompileError::Unsupported(format!(
                        "unknown node type `{}`",
                        step.uses
                    )));
                }
            };
            Ok(StepPlan {
                action,
                fields: step
                    .with
                    .iter()
                    .map(|(name, field)| Ok((name.to_owned(), value(field)?)))
                    .collect::<Result<_, CompileError>>()?,
                when: step.when.as_deref().map(expression).transpose()?,
                attempts: step.retry.unwrap_or(0).saturating_add(1),
                timeout: step.timeout,
            })
        })
        .collect::<Result<_, CompileError>>()?;
    Ok(GraphPlan {
        inputs: graph
            .inputs
            .iter()
            .map(|input| InputPlan {
                name: input.name.clone(),
                value_type: input.value_type,
                default: input.default.clone(),
            })
            .collect(),
        steps,
        step_ids: graph.steps.iter().map(|step| step.id.clone()).collect(),
        graph: DependencyGraph::of(graph),
        outputs: graph
            .outputs
            .iter()
            .map(|output| Ok((output.name.clone(), value(&output.value)?)))
            .collect::<Result<_, CompileError>>()?,
    })
}
