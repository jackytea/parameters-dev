use parameters_core::expr::{self, quote};
use parameters_core::{FieldValue, Input, Output, Step, Workflow};

const INDENT: &str = "  ";

#[must_use]
pub fn print(workflow: &Workflow) -> String {
    let mut paragraphs = Vec::new();
    let mut header = vec![format!("workflow {}", quote(&workflow.name))];
    if let Some(note) = &workflow.note {
        header.push(indented(1, &format!("note {}", quote(note))));
    }
    if let Some(concurrency) = workflow.concurrency {
        header.push(indented(1, &format!("concurrency {concurrency}")));
    }
    paragraphs.push(header);
    if !workflow.inputs.is_empty() {
        paragraphs.push(inputs(&workflow.inputs, 0));
    }
    graph(&workflow.steps, &workflow.outputs, 0, &mut paragraphs);
    for (name, definition) in workflow.definitions.iter() {
        let mut head = vec![format!("define node {name}")];
        if let Some(note) = &definition.note {
            head.push(indented(1, &format!("note {}", quote(note))));
        }
        head.extend(inputs(&definition.inputs, 1));
        let mut inner = vec![head];
        graph(&definition.steps, &definition.outputs, 1, &mut inner);
        paragraphs.push(inner.join(&String::new()));
    }
    let mut out = String::new();
    for (index, paragraph) in paragraphs.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        for line in paragraph {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn graph(steps: &[Step], outputs: &[Output], depth: usize, paragraphs: &mut Vec<Vec<String>>) {
    for step in steps {
        paragraphs.push(step_lines(step, depth));
    }
    if !outputs.is_empty() {
        paragraphs.push(
            outputs
                .iter()
                .map(|output| output_line(output, depth))
                .collect(),
        );
    }
}

fn inputs(inputs: &[Input], depth: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for input in inputs {
        let mut line = format!("input {}: {}", input.name, input.value_type);
        if let Some(default) = &input.default {
            line.push_str(" = ");
            line.push_str(&expr::print_value(default));
        }
        lines.push(indented(depth, &line));
        if let Some(note) = &input.note {
            lines.push(indented(depth + 1, &format!("note {}", quote(note))));
        }
    }
    lines
}

fn step_lines(step: &Step, depth: usize) -> Vec<String> {
    let mut lines = vec![indented(
        depth,
        &format!("step {} uses {}", step.id, step.uses),
    )];
    let mut child = |line: String| lines.push(indented(depth + 1, &line));
    if let Some(note) = &step.note {
        child(format!("note {}", quote(note)));
    }
    if !step.after.is_empty() {
        child(format!("after {}", list(&step.after)));
    }
    if let Some(condition) = &step.when {
        child(format!("when {}", expression(condition)));
    }
    if let Some(retry) = step.retry {
        child(format!(
            "retry {retry} {}",
            if retry == 1 { "time" } else { "times" }
        ));
    }
    if let Some(timeout) = step.timeout {
        child(format!("timeout {timeout}"));
    }
    for (field, value) in step.with.iter() {
        child(format!("{field} is {}", field_value(value)));
    }
    lines
}

fn output_line(output: &Output, depth: usize) -> String {
    let typed = output
        .value_type
        .map_or_else(String::new, |value_type| format!(": {value_type}"));
    indented(
        depth,
        &format!(
            "output {}{typed} is {}",
            output.name,
            field_value(&output.value)
        ),
    )
}

fn field_value(value: &FieldValue) -> String {
    match value {
        FieldValue::Literal(literal) => expr::print_value(literal),
        FieldValue::Expression(source) => expression(source),
    }
}

fn expression(source: &str) -> String {
    expr::parse(source).map_or_else(
        |_| source.replace(['\r', '\n'], " ").trim().to_owned(),
        |parsed| expr::print(&parsed),
    )
}

fn list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [only] => only.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

fn indented(depth: usize, line: &str) -> String {
    format!("{}{line}", INDENT.repeat(depth))
}
