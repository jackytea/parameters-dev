use std::collections::HashMap;
use std::sync::Arc;

use parameters_core::expr::{BinaryOperator, Expr, ExprKind, Reference, StepState, TextPart};
use serde_json::Value;

use crate::event::StepStatus;
use crate::node::Values;
use crate::redact::Redactor;
use crate::secrets::SecretStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Root {
    Input(usize),
    Step(usize),
}

#[derive(Debug, Clone)]
pub(crate) struct Path {
    root: Root,
    fields: Vec<String>,
    written: String,
}

#[derive(Debug, Clone)]
pub(crate) enum Piece {
    Literal(String),
    Reference(Path),
}

#[derive(Debug, Clone)]
pub(crate) enum Compiled {
    Value(Value),
    Text(Vec<Piece>),
    Secret(String),
    Reference(Path),
    State(usize, StepState),
    List(Vec<Compiled>),
    Object(Vec<(String, Compiled)>),
    Not(Box<Compiled>),
    Binary(BinaryOperator, Box<Compiled>, Box<Compiled>),
}

#[derive(Debug, Clone)]
pub(crate) struct Outcome {
    pub(crate) status: StepStatus,
    pub(crate) outputs: Arc<Values>,
}

pub(crate) struct Scope<'a> {
    pub(crate) inputs: &'a [Value],
    pub(crate) steps: &'a [Option<Outcome>],
    pub(crate) step_ids: &'a [String],
    pub(crate) secrets: &'a dyn SecretStore,
    pub(crate) redactor: &'a Redactor,
}

pub(crate) fn compile(expr: &Expr, names: &HashMap<&str, Root>) -> Result<Compiled, String> {
    Ok(match &expr.kind {
        ExprKind::Boolean(value) => Compiled::Value(Value::Bool(*value)),
        ExprKind::Number(number) => Compiled::Value(Value::Number(number.clone())),
        ExprKind::Secret(name) => Compiled::Secret(name.clone()),
        ExprKind::Text(parts) => Compiled::Text(
            parts
                .iter()
                .map(|part| match part {
                    TextPart::Literal(text) => Ok(Piece::Literal(text.clone())),
                    TextPart::Reference(reference) => path(reference, names).map(Piece::Reference),
                })
                .collect::<Result<_, _>>()?,
        ),
        ExprKind::Reference(reference) => Compiled::Reference(path(reference, names)?),
        ExprKind::State { step, state } => match names.get(step.name.as_str()) {
            Some(Root::Step(index)) => Compiled::State(*index, *state),
            _ => return Err(format!("`{}` is not a step", step.name)),
        },
        ExprKind::List(items) => Compiled::List(
            items
                .iter()
                .map(|item| compile(item, names))
                .collect::<Result<_, _>>()?,
        ),
        ExprKind::Object(entries) => Compiled::Object(
            entries
                .iter()
                .map(|(key, value)| Ok((key.name.clone(), compile(value, names)?)))
                .collect::<Result<_, String>>()?,
        ),
        ExprKind::Not(inner) => Compiled::Not(Box::new(compile(inner, names)?)),
        ExprKind::Binary {
            operator,
            left,
            right,
        } => Compiled::Binary(
            *operator,
            Box::new(compile(left, names)?),
            Box::new(compile(right, names)?),
        ),
    })
}

fn path(reference: &Reference, names: &HashMap<&str, Root>) -> Result<Path, String> {
    let root = reference.root();
    let resolved = names
        .get(root.name.as_str())
        .copied()
        .ok_or_else(|| format!("unknown reference `{}`", root.name))?;
    let segments: Vec<&str> = reference
        .segments
        .iter()
        .map(|segment| segment.name.as_str())
        .collect();
    Ok(Path {
        root: resolved,
        fields: segments[1..]
            .iter()
            .map(|segment| (*segment).to_owned())
            .collect(),
        written: segments.join("."),
    })
}

impl Compiled {
    pub(crate) fn evaluate(&self, scope: &Scope<'_>) -> Result<Value, String> {
        match self {
            Self::Value(value) => Ok(value.clone()),
            Self::Text(pieces) => {
                let mut text = String::new();
                for piece in pieces {
                    match piece {
                        Piece::Literal(literal) => text.push_str(literal),
                        Piece::Reference(path) => text.push_str(&stringify(&path.evaluate(scope)?)),
                    }
                }
                Ok(Value::String(text))
            }
            Self::Secret(name) => {
                let value = scope
                    .secrets
                    .get(name)
                    .ok_or_else(|| format!("the secret `{name}` is not set"))?;
                scope.redactor.add(&value);
                Ok(Value::String(value))
            }
            Self::Reference(path) => path.evaluate(scope),
            Self::State(index, state) => {
                let status = scope.steps[*index].as_ref().map(|outcome| outcome.status);
                Ok(Value::Bool(matches!(
                    (state, status),
                    (StepState::Succeeded, Some(StepStatus::Succeeded))
                        | (StepState::Failed, Some(StepStatus::Failed))
                        | (StepState::Skipped, Some(StepStatus::Skipped))
                )))
            }
            Self::List(items) => Ok(Value::Array(
                items
                    .iter()
                    .map(|item| item.evaluate(scope))
                    .collect::<Result<_, _>>()?,
            )),
            Self::Object(entries) => Ok(Value::Object(
                entries
                    .iter()
                    .map(|(key, value)| Ok((key.clone(), value.evaluate(scope)?)))
                    .collect::<Result<_, String>>()?,
            )),
            Self::Not(inner) => Ok(Value::Bool(!boolean(&inner.evaluate(scope)?, "not")?)),
            Self::Binary(operator, left, right) => binary(*operator, left, right, scope),
        }
    }

    pub(crate) fn condition(&self, scope: &Scope<'_>) -> Result<bool, String> {
        boolean(&self.evaluate(scope)?, "when")
    }
}

impl Path {
    fn evaluate(&self, scope: &Scope<'_>) -> Result<Value, String> {
        let (mut value, mut fields, mut owner) = match self.root {
            Root::Input(index) => (
                scope.inputs[index].clone(),
                self.fields.as_slice(),
                self.written_root(),
            ),
            Root::Step(index) => {
                let id = &scope.step_ids[index];
                let outcome = scope.steps[index]
                    .as_ref()
                    .filter(|outcome| outcome.status == StepStatus::Succeeded)
                    .ok_or_else(|| {
                        format!("`{id}` did not succeed, so its outputs are not available")
                    })?;
                let Some((output, rest)) = self.fields.split_first() else {
                    return Ok(Value::Object(outcome.outputs.as_ref().clone()));
                };
                let value = outcome
                    .outputs
                    .get(output)
                    .cloned()
                    .ok_or_else(|| format!("`{id}` did not produce the output `{output}`"))?;
                (value, rest, format!("{id}.{output}"))
            }
        };
        while let Some((field, rest)) = fields.split_first() {
            value = value
                .get(field.as_str())
                .cloned()
                .ok_or_else(|| format!("`{owner}` has no field `{field}`"))?;
            owner = format!("{owner}.{field}");
            fields = rest;
        }
        Ok(value)
    }

    fn written_root(&self) -> String {
        self.written
            .split('.')
            .next()
            .unwrap_or_default()
            .to_owned()
    }
}

fn binary(
    operator: BinaryOperator,
    left: &Compiled,
    right: &Compiled,
    scope: &Scope<'_>,
) -> Result<Value, String> {
    let left_value = left.evaluate(scope)?;
    match operator {
        BinaryOperator::And if !boolean(&left_value, "and")? => return Ok(Value::Bool(false)),
        BinaryOperator::Or if boolean(&left_value, "or")? => return Ok(Value::Bool(true)),
        _ => {}
    }
    let right_value = right.evaluate(scope)?;
    let result = match operator {
        BinaryOperator::And => boolean(&right_value, "and")?,
        BinaryOperator::Or => boolean(&right_value, "or")?,
        BinaryOperator::Equal => equal(&left_value, &right_value),
        BinaryOperator::NotEqual => !equal(&left_value, &right_value),
        ordering => {
            let (Some(left_number), Some(right_number)) =
                (left_value.as_f64(), right_value.as_f64())
            else {
                return Err("only numbers can be compared with <, <=, > and >=".to_owned());
            };
            match ordering {
                BinaryOperator::Less => left_number < right_number,
                BinaryOperator::LessOrEqual => left_number <= right_number,
                BinaryOperator::Greater => left_number > right_number,
                _ => left_number >= right_number,
            }
        }
    };
    Ok(Value::Bool(result))
}

fn equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => numbers_equal(left, right),
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| equal(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .all(|(key, value)| right.get(key).is_some_and(|other| equal(value, other)))
        }
        _ => left == right,
    }
}

fn numbers_equal(left: &serde_json::Number, right: &serde_json::Number) -> bool {
    if let (Some(left), Some(right)) = (left.as_i64(), right.as_i64()) {
        return left == right;
    }
    if let (Some(left), Some(right)) = (left.as_u64(), right.as_u64()) {
        return left == right;
    }
    left.as_f64() == right.as_f64()
}

fn boolean(value: &Value, operator: &str) -> Result<bool, String> {
    value.as_bool().ok_or_else(|| {
        format!(
            "`{operator}` needs true or false, but got {}",
            stringify(value)
        )
    })
}

pub(crate) fn stringify(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::secrets::NoSecrets;

    fn evaluate(
        source: &str,
        inputs: &[Value],
        steps: &[Option<Outcome>],
    ) -> Result<Value, String> {
        let names: HashMap<&str, Root> =
            [("count", Root::Input(0)), ("build", Root::Step(0))].into();
        let compiled = compile(
            &parameters_core::expr::parse(source).map_err(|error| error.message)?,
            &names,
        )?;
        let ids = ["build".to_owned()];
        let redactor = Redactor::default();
        compiled.evaluate(&Scope {
            inputs,
            steps,
            step_ids: &ids,
            secrets: &NoSecrets,
            redactor: &redactor,
        })
    }

    fn succeeded(outputs: &Value) -> Outcome {
        Outcome {
            status: StepStatus::Succeeded,
            outputs: Arc::new(outputs.as_object().cloned().unwrap_or_default()),
        }
    }

    #[test]
    fn evaluates_references_text_and_logic() {
        let steps = [Some(succeeded(
            &json!({ "body": { "url": "https://x" }, "code": 0 }),
        ))];
        let inputs = [json!(3)];
        assert_eq!(
            evaluate("\"{build.body.url}/{count}\"", &inputs, &steps),
            Ok(json!("https://x/3"))
        );
        assert_eq!(
            evaluate("count > 2 and build succeeded", &inputs, &steps),
            Ok(json!(true))
        );
        assert_eq!(
            evaluate("build.code == 0.0", &inputs, &steps),
            Ok(json!(true))
        );
        assert_eq!(
            evaluate("[count, { n: count }]", &inputs, &steps),
            Ok(json!([3, { "n": 3 }]))
        );
        assert_eq!(
            evaluate("build failed or not true", &inputs, &steps),
            Ok(json!(false))
        );
    }

    #[test]
    fn short_circuits_and_reports_runtime_errors() {
        let failed = [Some(Outcome {
            status: StepStatus::Failed,
            outputs: Arc::default(),
        })];
        let inputs = [json!(3)];
        assert_eq!(
            evaluate("build succeeded and build.url == 1", &inputs, &failed),
            Ok(json!(false))
        );
        assert_eq!(
            evaluate("build.url", &inputs, &failed),
            Err("`build` did not succeed, so its outputs are not available".to_owned())
        );
        let steps = [Some(succeeded(&json!({ "body": "text" })))];
        assert_eq!(
            evaluate("build.body.url", &inputs, &steps),
            Err("`build.body` has no field `url`".to_owned())
        );
        assert_eq!(
            evaluate("secret \"MISSING\"", &inputs, &steps),
            Err("the secret `MISSING` is not set".to_owned())
        );
        assert!(evaluate("count < \"x\"", &inputs, &steps).is_err());
    }
}
