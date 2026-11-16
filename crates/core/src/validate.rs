use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::catalog::{Catalog, Signature};
use crate::diagnostic::{Code, Diagnostic, JsonPath, Location};
use crate::document::{EXPRESSION_KEY, FieldValue, Graph, Input, Step, ValueType, Workflow};
use crate::expr::{self, BinaryOperator, Expr, ExprKind, Reference, Segment, TextPart};
use crate::graph::DependencyGraph;
use crate::load::CURRENT_VERSION;
use crate::name::{NameKind, NameProblem, check_name, is_valid_node_type, suggest_name};
use crate::suggest::closest;

pub const MAX_STEPS: usize = 10_000;
pub const MAX_DEFINITIONS: usize = 1_000;
pub const MAX_RETRIES: u32 = 100;
pub const MAX_CONCURRENCY: u32 = 1_024;

#[must_use]
pub fn validate(workflow: &Workflow, builtins: &dyn Catalog) -> Vec<Diagnostic> {
    let context = Context {
        builtins,
        definitions: workflow
            .definitions
            .iter()
            .map(|(name, definition)| (name, Signature::of_definition(definition)))
            .collect(),
    };
    let mut out = Vec::new();
    check_header(workflow, &mut out);
    context.check_definitions(workflow, &mut out);
    context.check_graph(workflow.graph(), &JsonPath::root(), &mut out);
    out
}

struct Context<'a> {
    builtins: &'a dyn Catalog,
    definitions: HashMap<&'a str, Signature>,
}

#[derive(Clone, Copy)]
enum Binding<'a> {
    Input(&'a Input),
    Step(&'a Step),
}

struct Scope<'a> {
    bindings: HashMap<&'a str, Binding<'a>>,
}

impl Scope<'_> {
    fn names(&self) -> impl Iterator<Item = &str> {
        self.bindings.keys().copied()
    }
}

impl<'a> Context<'a> {
    fn check_definitions(&self, workflow: &'a Workflow, out: &mut Vec<Diagnostic>) {
        let base = JsonPath::root().key("definitions");
        if workflow.definitions.len() > MAX_DEFINITIONS {
            out.push(Diagnostic::new(
                Code::LimitExceeded,
                Location::at(base),
                format!("a workflow can have at most {MAX_DEFINITIONS} definitions"),
            ));
            return;
        }
        let names: Vec<&str> = workflow.definitions.keys().collect();
        let indices: HashMap<&str, usize> = names
            .iter()
            .enumerate()
            .map(|(index, name)| (*name, index))
            .collect();
        let mut uses = Vec::with_capacity(names.len());
        for (name, definition) in workflow.definitions.iter() {
            let path = base.key(name);
            if !is_valid_node_type(name) {
                out.push(Diagnostic::new(
                    Code::InvalidNodeType,
                    Location::at(path.clone()),
                    format!("`{name}` is not a valid node type: use a qualified name such as `acme.deploy`"),
                ));
            }
            if self.builtins.contains(name) {
                out.push(Diagnostic::new(
                    Code::BuiltinShadowed,
                    Location::at(path.clone()),
                    format!("`{name}` is a built-in node and cannot be redefined"),
                ));
            }
            uses.push(
                definition
                    .steps
                    .iter()
                    .filter_map(|step| indices.get(step.uses.as_str()).copied())
                    .collect::<Vec<_>>(),
            );
            self.check_graph(definition.graph(), &path, out);
        }
        if let Err(found) = DependencyGraph::from_dependencies(&uses).topological_order() {
            for cycle in found.cycles {
                let members: Vec<&str> = cycle.iter().map(|&index| names[index]).collect();
                out.push(Diagnostic::new(
                    Code::DefinitionCycle,
                    Location::at(base.key(members[0])),
                    cycle_message(
                        &members,
                        "definition",
                        "uses itself, so it could never finish",
                        "use each other, so none of them could finish",
                    ),
                ));
            }
        }
    }

    fn check_graph(&self, graph: Graph<'a>, base: &JsonPath, out: &mut Vec<Diagnostic>) {
        if graph.steps.len() > MAX_STEPS {
            out.push(Diagnostic::new(
                Code::LimitExceeded,
                Location::at(base.key("steps")),
                format!("a graph can have at most {MAX_STEPS} steps"),
            ));
            return;
        }
        let scope = check_names(graph, base, out);
        for (index, input) in graph.inputs.iter().enumerate() {
            check_input_default(input, &base.key("inputs").index(index), out);
        }
        for (index, step) in graph.steps.iter().enumerate() {
            self.check_step(step, &base.key("steps").index(index), &scope, out);
        }
        let mut output_names = HashSet::new();
        for (index, output) in graph.outputs.iter().enumerate() {
            let path = base.key("outputs").index(index);
            check_name_at(&output.name, NameKind::Output, &path.key("name"), out);
            if !output_names.insert(output.name.as_str()) {
                out.push(duplicate(&output.name, "output", path.key("name")));
            }
            let expected = output.value_type.unwrap_or(ValueType::Any);
            self.check_value(&output.value, expected, &path.key("value"), &scope, out);
        }
        if let Err(found) = DependencyGraph::of(graph).topological_order() {
            for cycle in found.cycles {
                let members: Vec<&str> = cycle
                    .iter()
                    .map(|&index| graph.steps[index].id.as_str())
                    .collect();
                out.push(Diagnostic::new(
                    Code::DependencyCycle,
                    Location::at(base.key("steps").index(cycle[0])),
                    cycle_message(
                        &members,
                        "step",
                        "depends on itself, so it could never start",
                        "depend on each other, so none of them could start",
                    ),
                ));
            }
        }
        let step_ids: HashSet<&str> = graph.steps.iter().map(|step| step.id.as_str()).collect();
        for key in graph.layout.keys().filter(|key| !step_ids.contains(key)) {
            out.push(Diagnostic::new(
                Code::UnknownLayoutEntry,
                Location::at(base.key("layout").key(key)),
                format!("the layout has a position for `{key}`, which is not a step"),
            ));
        }
    }

    fn check_step(
        &self,
        step: &Step,
        path: &JsonPath,
        scope: &Scope<'_>,
        out: &mut Vec<Diagnostic>,
    ) {
        let signature = self.resolve_node_type(&step.uses, &path.key("uses"), out);
        for (index, name) in step.after.iter().enumerate() {
            let location = Location::at(path.key("after").index(index));
            match scope.bindings.get(name.as_str()) {
                Some(Binding::Step(_)) => {}
                Some(Binding::Input(_)) => out.push(Diagnostic::new(
                    Code::UnknownStep,
                    location,
                    format!("`{name}` is an input, not a step, so nothing can wait for it"),
                )),
                None => out.push(
                    Diagnostic::new(
                        Code::UnknownStep,
                        location,
                        format!("unknown step `{name}`"),
                    )
                    .with_suggestion(closest_step(name, scope)),
                ),
            }
        }
        if step.retry.is_some_and(|retry| retry > MAX_RETRIES) {
            out.push(Diagnostic::new(
                Code::InvalidSetting,
                Location::at(path.key("retry")),
                format!("a step can be retried at most {MAX_RETRIES} times"),
            ));
        }
        if step.timeout.is_some_and(|timeout| timeout.as_millis() == 0) {
            out.push(Diagnostic::new(
                Code::InvalidSetting,
                Location::at(path.key("timeout")),
                "the timeout must be longer than zero",
            ));
        }
        if let Some(condition) = &step.when {
            self.check_expression(condition, ValueType::Boolean, &path.key("when"), scope, out);
        }
        for (field, value) in step.with.iter() {
            let field_path = path.key("with").key(field);
            check_name_at(field, NameKind::Field, &field_path, out);
            let port = signature.and_then(|signature| signature.input(field));
            if let (Some(signature), None) = (signature, port) {
                out.push(
                    Diagnostic::new(
                        Code::UnknownField,
                        Location::at(field_path.clone()),
                        format!("`{}` has no field `{field}`", step.uses),
                    )
                    .with_suggestion(
                        closest(
                            field,
                            signature.inputs.iter().map(|port| port.name.as_str()),
                        )
                        .map(str::to_owned),
                    ),
                );
            }
            let expected = port.map_or(ValueType::Any, |port| port.value_type);
            self.check_value(value, expected, &field_path, scope, out);
        }
        for port in signature.iter().flat_map(|signature| &signature.inputs) {
            if port.required && !step.with.contains_key(&port.name) {
                out.push(Diagnostic::new(
                    Code::MissingField,
                    Location::at(path.key("with")),
                    format!(
                        "`{}` needs the field `{}` ({})",
                        step.uses, port.name, port.value_type
                    ),
                ));
            }
        }
    }

    fn resolve_node_type(
        &self,
        node_type: &str,
        path: &JsonPath,
        out: &mut Vec<Diagnostic>,
    ) -> Option<&Signature> {
        if !is_valid_node_type(node_type) {
            out.push(Diagnostic::new(
                Code::InvalidNodeType,
                Location::at(path.clone()),
                format!("`{node_type}` is not a valid node type: use a qualified name such as `http.request`"),
            ));
            return None;
        }
        let signature = self.signature(node_type);
        if signature.is_none() {
            let known = self
                .builtins
                .node_types()
                .into_iter()
                .chain(self.definitions.keys().copied());
            out.push(
                Diagnostic::new(
                    Code::UnknownNodeType,
                    Location::at(path.clone()),
                    format!("unknown node type `{node_type}`"),
                )
                .with_suggestion(closest(node_type, known).map(str::to_owned)),
            );
        }
        signature
    }

    fn signature(&self, node_type: &str) -> Option<&Signature> {
        self.builtins
            .signature(node_type)
            .or_else(|| self.definitions.get(node_type))
    }

    fn check_value(
        &self,
        value: &FieldValue,
        expected: ValueType,
        path: &JsonPath,
        scope: &Scope<'_>,
        out: &mut Vec<Diagnostic>,
    ) {
        match value {
            FieldValue::Expression(source) => {
                self.check_expression(source, expected, path, scope, out);
            }
            FieldValue::Literal(literal) => check_literal(literal, expected, path, out),
        }
    }

    fn check_expression(
        &self,
        source: &str,
        expected: ValueType,
        path: &JsonPath,
        scope: &Scope<'_>,
        out: &mut Vec<Diagnostic>,
    ) {
        let parsed = match expr::parse(source) {
            Ok(parsed) => parsed,
            Err(error) => {
                out.push(Diagnostic::new(
                    Code::InvalidExpression,
                    Location::within(path.clone(), error.span),
                    error.message,
                ));
                return;
            }
        };
        let Some(found) = self.infer(&parsed, path, scope, out) else {
            return;
        };
        if !expected.accepts(found) {
            out.push(mismatch(
                expected,
                found,
                Location::within(path.clone(), parsed.span),
            ));
        }
    }

    fn infer(
        &self,
        expr: &Expr,
        path: &JsonPath,
        scope: &Scope<'_>,
        out: &mut Vec<Diagnostic>,
    ) -> Option<ValueType> {
        match &expr.kind {
            ExprKind::Boolean(_) => Some(ValueType::Boolean),
            ExprKind::Number(_) => Some(ValueType::Number),
            ExprKind::Secret(_) => Some(ValueType::Secret),
            ExprKind::Text(parts) => {
                let mut contains_secret = false;
                for part in parts {
                    let TextPart::Reference(reference) = part else {
                        continue;
                    };
                    match self.resolve(reference, path, scope, out) {
                        Some(ValueType::Secret) => contains_secret = true,
                        Some(found @ (ValueType::List | ValueType::Object)) => {
                            out.push(Diagnostic::new(
                                Code::TypeMismatch,
                                Location::within(path.clone(), reference.span()),
                                format!("a {found} cannot be placed inside text"),
                            ));
                        }
                        _ => {}
                    }
                }
                Some(if contains_secret {
                    ValueType::Secret
                } else {
                    ValueType::Text
                })
            }
            ExprKind::Reference(reference) => self.resolve(reference, path, scope, out),
            ExprKind::State { step, .. } => {
                resolve_state(step, path, scope, out);
                Some(ValueType::Boolean)
            }
            ExprKind::List(items) => {
                for item in items {
                    self.infer(item, path, scope, out);
                }
                Some(ValueType::List)
            }
            ExprKind::Object(entries) => {
                for (_, value) in entries {
                    self.infer(value, path, scope, out);
                }
                Some(ValueType::Object)
            }
            ExprKind::Not(inner) => {
                self.expect_operand(inner, ValueType::Boolean, path, scope, out);
                Some(ValueType::Boolean)
            }
            ExprKind::Binary {
                operator,
                left,
                right,
            } => {
                let operand = match operator {
                    BinaryOperator::And | BinaryOperator::Or => ValueType::Boolean,
                    operator if operator.is_ordering() => ValueType::Number,
                    _ => ValueType::Any,
                };
                self.expect_operand(left, operand, path, scope, out);
                self.expect_operand(right, operand, path, scope, out);
                Some(ValueType::Boolean)
            }
        }
    }

    fn expect_operand(
        &self,
        operand: &Expr,
        expected: ValueType,
        path: &JsonPath,
        scope: &Scope<'_>,
        out: &mut Vec<Diagnostic>,
    ) {
        if let Some(found) = self
            .infer(operand, path, scope, out)
            .filter(|&found| !expected.accepts(found))
        {
            out.push(mismatch(
                expected,
                found,
                Location::within(path.clone(), operand.span),
            ));
        }
    }

    fn resolve(
        &self,
        reference: &Reference,
        path: &JsonPath,
        scope: &Scope<'_>,
        out: &mut Vec<Diagnostic>,
    ) -> Option<ValueType> {
        let root = reference.root();
        let rest = &reference.segments[1..];
        match scope.bindings.get(root.name.as_str()) {
            None => {
                out.push(
                    Diagnostic::new(
                        Code::UnknownReference,
                        Location::within(path.clone(), root.span),
                        format!("unknown reference `{}`", root.name),
                    )
                    .with_suggestion(closest(&root.name, scope.names()).map(str::to_owned)),
                );
                None
            }
            Some(Binding::Input(input)) => {
                walk_fields(input.value_type, &root.name, rest, path, out)
            }
            Some(Binding::Step(step)) => {
                let Some(output) = rest.first() else {
                    return Some(ValueType::Object);
                };
                let signature = self.signature(&step.uses)?;
                let Some(port) = signature.output(&output.name) else {
                    out.push(
                        Diagnostic::new(
                            Code::UnknownOutput,
                            Location::within(path.clone(), output.span),
                            format!("`{}` has no output `{}`", step.id, output.name),
                        )
                        .with_suggestion(
                            closest(
                                &output.name,
                                signature.outputs.iter().map(|port| port.name.as_str()),
                            )
                            .map(str::to_owned),
                        ),
                    );
                    return None;
                };
                walk_fields(
                    port.value_type,
                    &format!("{}.{}", step.id, output.name),
                    &rest[1..],
                    path,
                    out,
                )
            }
        }
    }
}

fn check_header(workflow: &Workflow, out: &mut Vec<Diagnostic>) {
    let root = JsonPath::root();
    if workflow.version != CURRENT_VERSION {
        out.push(Diagnostic::new(
            Code::InvalidSetting,
            Location::at(root.key("parameters")),
            format!(
                "unsupported document version {}; expected {CURRENT_VERSION}",
                workflow.version
            ),
        ));
    }
    if workflow.name.trim().is_empty() {
        out.push(Diagnostic::new(
            Code::InvalidSetting,
            Location::at(root.key("name")),
            "the workflow needs a name",
        ));
    }
    if let Some(concurrency) = workflow
        .concurrency
        .filter(|&limit| limit == 0 || limit > MAX_CONCURRENCY)
    {
        out.push(Diagnostic::new(
            Code::InvalidSetting,
            Location::at(root.key("concurrency")),
            format!("concurrency must be between 1 and {MAX_CONCURRENCY}, not {concurrency}"),
        ));
    }
}

fn check_names<'a>(graph: Graph<'a>, base: &JsonPath, out: &mut Vec<Diagnostic>) -> Scope<'a> {
    let mut bindings = HashMap::new();
    for (index, input) in graph.inputs.iter().enumerate() {
        let path = base.key("inputs").index(index).key("name");
        check_name_at(&input.name, NameKind::Input, &path, out);
        if bindings
            .insert(input.name.as_str(), Binding::Input(input))
            .is_some()
        {
            out.push(duplicate(&input.name, "input", path));
        }
    }
    for (index, step) in graph.steps.iter().enumerate() {
        let path = base.key("steps").index(index).key("id");
        check_name_at(&step.id, NameKind::Step, &path, out);
        if let Some(previous) = bindings.insert(step.id.as_str(), Binding::Step(step)) {
            let kind = match previous {
                Binding::Input(_) => "input",
                Binding::Step(_) => "step",
            };
            out.push(duplicate(&step.id, kind, path));
        }
    }
    Scope { bindings }
}

fn resolve_state(step: &Segment, path: &JsonPath, scope: &Scope<'_>, out: &mut Vec<Diagnostic>) {
    let location = Location::within(path.clone(), step.span);
    match scope.bindings.get(step.name.as_str()) {
        Some(Binding::Step(_)) => {}
        Some(Binding::Input(_)) => out.push(Diagnostic::new(
            Code::UnknownStep,
            location,
            format!(
                "`{}` is an input, not a step, so it has no state",
                step.name
            ),
        )),
        None => out.push(
            Diagnostic::new(
                Code::UnknownStep,
                location,
                format!("unknown step `{}`", step.name),
            )
            .with_suggestion(closest_step(&step.name, scope)),
        ),
    }
}

fn walk_fields(
    value_type: ValueType,
    owner: &str,
    rest: &[Segment],
    path: &JsonPath,
    out: &mut Vec<Diagnostic>,
) -> Option<ValueType> {
    match rest.first() {
        None => Some(value_type),
        Some(_) if value_type.has_fields() => Some(ValueType::Any),
        Some(field) => {
            out.push(Diagnostic::new(
                Code::InvalidFieldAccess,
                Location::within(path.clone(), field.span),
                format!(
                    "`{owner}` is {value_type} and has no field `{}`",
                    field.name
                ),
            ));
            None
        }
    }
}

fn check_input_default(input: &Input, path: &JsonPath, out: &mut Vec<Diagnostic>) {
    let Some(default) = &input.default else {
        return;
    };
    let path = path.key("default");
    if input.value_type == ValueType::Secret {
        out.push(Diagnostic::new(
            Code::LiteralSecret,
            Location::at(path),
            "secret inputs cannot have a default, because the secret would be stored in the workflow",
        ));
    } else {
        check_literal(default, input.value_type, &path, out);
    }
}

fn check_literal(literal: &Value, expected: ValueType, path: &JsonPath, out: &mut Vec<Diagnostic>) {
    check_reserved_keys(literal, path, out);
    if expected == ValueType::Secret {
        out.push(Diagnostic::new(
            Code::LiteralSecret,
            Location::at(path.clone()),
            "secret fields need `secret \"NAME\"` or a secret input, so the secret is never stored in the workflow",
        ));
        return;
    }
    match literal_type(literal) {
        None => out.push(Diagnostic::new(
            Code::TypeMismatch,
            Location::at(path.clone()),
            "null is not a supported value; remove the field instead",
        )),
        Some(found) if !expected.accepts(found) => {
            out.push(mismatch(expected, found, Location::at(path.clone())));
        }
        Some(_) => {}
    }
}

fn check_reserved_keys(literal: &Value, path: &JsonPath, out: &mut Vec<Diagnostic>) {
    match literal {
        Value::Object(object) => {
            for (key, value) in object {
                let child = path.key(key);
                if key.starts_with('$') {
                    let message = if key == EXPRESSION_KEY {
                        format!(
                            "`{EXPRESSION_KEY}` must be the only key and hold the expression as text"
                        )
                    } else {
                        format!("keys starting with `$` are reserved, so `{key}` cannot be used")
                    };
                    out.push(Diagnostic::new(
                        Code::ReservedKey,
                        Location::at(child.clone()),
                        message,
                    ));
                }
                check_reserved_keys(value, &child, out);
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                check_reserved_keys(item, &path.index(index), out);
            }
        }
        _ => {}
    }
}

const fn literal_type(literal: &Value) -> Option<ValueType> {
    match literal {
        Value::Null => None,
        Value::Bool(_) => Some(ValueType::Boolean),
        Value::Number(_) => Some(ValueType::Number),
        Value::String(_) => Some(ValueType::Text),
        Value::Array(_) => Some(ValueType::List),
        Value::Object(_) => Some(ValueType::Object),
    }
}

fn mismatch(expected: ValueType, found: ValueType, location: Location) -> Diagnostic {
    if expected == ValueType::Secret {
        Diagnostic::new(
            Code::LiteralSecret,
            location,
            format!("expected a secret but found {found}; use `secret \"NAME\"` or a secret input"),
        )
    } else {
        Diagnostic::new(
            Code::TypeMismatch,
            location,
            format!("expected {expected} but found {found}"),
        )
    }
}

fn check_name_at(name: &str, kind: NameKind, path: &JsonPath, out: &mut Vec<Diagnostic>) {
    let Err(problem) = check_name(name) else {
        return;
    };
    let (code, message) = match problem {
        NameProblem::Empty => (Code::InvalidName, "names cannot be empty".to_owned()),
        NameProblem::TooLong => (
            Code::InvalidName,
            format!(
                "`{name}` is longer than {} characters",
                crate::name::MAX_NAME_LENGTH
            ),
        ),
        NameProblem::InvalidCharacters => (
            Code::InvalidName,
            format!(
                "`{name}` is not a valid name: use lowercase letters, digits and underscores, starting with a letter or underscore"
            ),
        ),
        NameProblem::Keyword => (
            Code::ReservedName,
            format!("`{name}` is a reserved keyword"),
        ),
    };
    out.push(
        Diagnostic::new(code, Location::at(path.clone()), message)
            .with_suggestion(suggest_name(name, kind)),
    );
}

fn duplicate(name: &str, previous_kind: &str, path: JsonPath) -> Diagnostic {
    Diagnostic::new(
        Code::DuplicateName,
        Location::at(path),
        format!("`{name}` is already used by another {previous_kind}"),
    )
}

fn closest_step(name: &str, scope: &Scope<'_>) -> Option<String> {
    let steps = scope
        .bindings
        .iter()
        .filter(|(_, binding)| matches!(binding, Binding::Step(_)))
        .map(|(name, _)| *name);
    closest(name, steps).map(str::to_owned)
}

fn cycle_message(names: &[&str], noun: &str, alone: &str, together: &str) -> String {
    let quoted: Vec<String> = names.iter().map(|name| format!("`{name}`")).collect();
    match quoted.as_slice() {
        [only] => format!("{noun} {only} {alone}"),
        [init @ .., last] => format!("{noun}s {} and {last} {together}", init.join(", ")),
        [] => String::new(),
    }
}
