#![allow(
    clippy::unwrap_used,
    reason = "property-test strategies build values that are valid by construction"
)]

use std::fmt::Write;

use parameters_core::expr::{
    self, BinaryOperator, Expr, ExprKind, Reference, Segment, StepState, TextPart,
};
use parameters_core::{
    Definition, Duration, FieldValue, Input, OrderedMap, Output, Span, Step, ValueType, Workflow,
};
use parameters_lang::{format, highlight, literal_value, parse, print};
use proptest::collection::vec;
use proptest::option;
use proptest::prelude::*;
use serde_json::{Map, Number, Value};

const NAMES: [&str; 6] = ["build", "deploy", "test_suite", "a", "x_1", "_hidden"];
const FIELDS: [&str; 6] = ["path", "body", "for", "url", "x2", "Upper"];
const NODE_TYPES: [&str; 4] = ["shell.run", "http.request", "acme.release", "a.b.c"];
const SECRETS: [&str; 3] = ["TOKEN", "_x", "Api_Key_2"];
const OPERATORS: [BinaryOperator; 8] = [
    BinaryOperator::And,
    BinaryOperator::Or,
    BinaryOperator::Equal,
    BinaryOperator::NotEqual,
    BinaryOperator::Less,
    BinaryOperator::LessOrEqual,
    BinaryOperator::Greater,
    BinaryOperator::GreaterOrEqual,
];

fn pick(values: &'static [&'static str]) -> impl Strategy<Value = String> {
    prop::sample::select(values).prop_map(str::to_owned)
}

fn text() -> impl Strategy<Value = String> {
    vec(
        prop_oneof![
            4 => any::<char>(),
            1 => prop::sample::select(vec!['"', '\\', '{', '}', '\n', '\r', '\t', '\u{0}', '\u{2028}', ' ']),
        ],
        0..8,
    )
    .prop_map(String::from_iter)
}

fn key() -> impl Strategy<Value = String> {
    text().prop_filter("`$` keys are reserved", |key| !key.starts_with('$'))
}

fn number() -> impl Strategy<Value = Number> {
    prop_oneof![
        any::<i64>().prop_map(Number::from),
        any::<u64>().prop_map(Number::from),
        prop::num::f64::NORMAL.prop_filter_map("finite", Number::from_f64),
        prop::sample::select(vec![0.5, -0.0, 1e100, 1e-300])
            .prop_filter_map("finite", Number::from_f64),
    ]
}

fn json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        any::<bool>().prop_map(Value::Bool),
        number().prop_map(Value::Number),
        text().prop_map(Value::String),
    ];
    leaf.prop_recursive(3, 16, 4, |inner| {
        prop_oneof![
            vec(inner.clone(), 0..4).prop_map(Value::Array),
            vec((key(), inner), 0..4)
                .prop_map(|entries| Value::Object(entries.into_iter().collect::<Map<_, _>>())),
        ]
    })
}

fn segment(name: String) -> Segment {
    Segment {
        name,
        span: Span::default(),
    }
}

fn reference() -> impl Strategy<Value = Reference> {
    (pick(&NAMES), vec(pick(&FIELDS), 0..3)).prop_map(|(root, rest)| Reference {
        segments: std::iter::once(root).chain(rest).map(segment).collect(),
    })
}

fn text_parts() -> impl Strategy<Value = Vec<TextPart>> {
    vec(
        prop_oneof![
            text().prop_map(TextPart::Literal),
            reference().prop_map(TextPart::Reference),
        ],
        0..4,
    )
    .prop_map(|parts| {
        let mut merged: Vec<TextPart> = Vec::new();
        for part in parts {
            match (merged.last_mut(), part) {
                (_, TextPart::Literal(text)) if text.is_empty() => {}
                (Some(TextPart::Literal(previous)), TextPart::Literal(text)) => {
                    previous.push_str(&text);
                }
                (_, part) => merged.push(part),
            }
        }
        merged
    })
}

fn expression() -> impl Strategy<Value = Expr> {
    let leaf = prop_oneof![
        any::<bool>().prop_map(ExprKind::Boolean),
        number().prop_map(ExprKind::Number),
        text_parts().prop_map(ExprKind::Text),
        pick(&SECRETS).prop_map(ExprKind::Secret),
        reference().prop_map(ExprKind::Reference),
        (
            pick(&NAMES),
            prop::sample::select(vec![
                StepState::Succeeded,
                StepState::Failed,
                StepState::Skipped
            ])
        )
            .prop_map(|(step, state)| ExprKind::State {
                step: segment(step),
                state,
            }),
    ];
    leaf.prop_map(wrap).prop_recursive(4, 24, 4, |inner| {
        prop_oneof![
            vec(inner.clone(), 0..3).prop_map(ExprKind::List),
            vec((key(), inner.clone()), 0..3).prop_map(|entries| {
                let mut unique: Vec<(Segment, Expr)> = Vec::new();
                for (key, value) in entries {
                    if unique.iter().all(|(existing, _)| existing.name != key) {
                        unique.push((segment(key), value));
                    }
                }
                ExprKind::Object(unique)
            }),
            inner
                .clone()
                .prop_map(|operand| ExprKind::Not(Box::new(operand))),
            (
                prop::sample::select(OPERATORS.to_vec()),
                inner.clone(),
                inner
            )
                .prop_map(|(operator, left, right)| ExprKind::Binary {
                    operator,
                    left: Box::new(left),
                    right: Box::new(right),
                }),
        ]
        .prop_map(wrap)
    })
}

const fn wrap(kind: ExprKind) -> Expr {
    Expr {
        kind,
        span: Span::new(0, 0),
    }
}

fn shape(expr: &Expr) -> String {
    let mut out = String::new();
    write_shape(expr, &mut out);
    out
}

fn write_shape(expr: &Expr, out: &mut String) {
    match &expr.kind {
        ExprKind::Boolean(value) => {
            let _ = write!(out, "{value}");
        }
        ExprKind::Number(number) => {
            let _ = write!(out, "#{number}");
        }
        ExprKind::Text(parts) => {
            out.push_str("(text");
            for part in parts {
                match part {
                    TextPart::Literal(text) => {
                        let _ = write!(out, " {text:?}");
                    }
                    TextPart::Reference(reference) => {
                        let names: Vec<&str> = reference
                            .segments
                            .iter()
                            .map(|segment| segment.name.as_str())
                            .collect();
                        let _ = write!(out, " {{{}}}", names.join("."));
                    }
                }
            }
            out.push(')');
        }
        ExprKind::Secret(name) => {
            let _ = write!(out, "(secret {name:?})");
        }
        ExprKind::Reference(reference) => {
            let names: Vec<&str> = reference
                .segments
                .iter()
                .map(|segment| segment.name.as_str())
                .collect();
            let _ = write!(out, "(ref {})", names.join("."));
        }
        ExprKind::State { step, state } => {
            let _ = write!(out, "(state {} {state:?})", step.name);
        }
        ExprKind::List(items) => {
            out.push_str("(list");
            for item in items {
                out.push(' ');
                write_shape(item, out);
            }
            out.push(')');
        }
        ExprKind::Object(entries) => {
            out.push_str("(object");
            for (key, value) in entries {
                let _ = write!(out, " {:?}=", key.name);
                write_shape(value, out);
            }
            out.push(')');
        }
        ExprKind::Not(inner) => {
            out.push_str("(not ");
            write_shape(inner, out);
            out.push(')');
        }
        ExprKind::Binary {
            operator,
            left,
            right,
        } => {
            let _ = write!(out, "({operator:?} ");
            write_shape(left, out);
            out.push(' ');
            write_shape(right, out);
            out.push(')');
        }
    }
}

fn value_type() -> impl Strategy<Value = ValueType> {
    prop::sample::select(ValueType::ALL.to_vec())
}

fn field_value() -> impl Strategy<Value = FieldValue> {
    prop_oneof![
        json_value().prop_map(FieldValue::Literal),
        expression()
            .prop_filter("plain values are stored as JSON", |expr| literal_value(
                expr
            )
            .is_none())
            .prop_map(|expr| FieldValue::Expression(expr::print(&expr))),
    ]
}

fn input() -> impl Strategy<Value = Input> {
    (
        pick(&NAMES),
        value_type(),
        option::of(json_value()),
        option::of(text()),
    )
        .prop_map(|(name, value_type, default, note)| Input {
            name,
            value_type,
            default,
            note,
        })
}

fn output() -> impl Strategy<Value = Output> {
    (pick(&NAMES), option::of(value_type()), field_value()).prop_map(|(name, value_type, value)| {
        Output {
            name,
            value_type,
            value,
        }
    })
}

fn step() -> impl Strategy<Value = Step> {
    (
        pick(&NAMES),
        pick(&NODE_TYPES),
        option::of(text()),
        vec(pick(&NAMES), 0..4),
        option::of(expression().prop_map(|expr| expr::print(&expr))),
        option::of(any::<u32>()),
        option::of(any::<u32>().prop_map(|millis| Duration::from_millis(u64::from(millis)))),
        vec((pick(&NAMES), field_value()), 0..4),
    )
        .prop_map(
            |(id, uses, note, after, when, retry, timeout, fields)| Step {
                id,
                uses,
                note,
                after,
                when,
                retry,
                timeout,
                with: fields.into_iter().collect::<OrderedMap<_>>(),
            },
        )
}

fn definition() -> impl Strategy<Value = Definition> {
    (
        option::of(text()),
        vec(input(), 0..3),
        vec(step(), 0..3),
        vec(output(), 0..3),
    )
        .prop_map(|(note, inputs, steps, outputs)| Definition {
            note,
            inputs,
            steps,
            outputs,
            layout: OrderedMap::new(),
        })
}

fn workflow() -> impl Strategy<Value = Workflow> {
    (
        text(),
        option::of(text()),
        option::of(any::<u32>()),
        vec(input(), 0..3),
        vec(step(), 0..4),
        vec(output(), 0..3),
        vec((pick(&NODE_TYPES), definition()), 0..3),
    )
        .prop_map(
            |(name, note, concurrency, inputs, steps, outputs, definitions)| {
                let mut workflow = Workflow::new(name);
                workflow.note = note;
                workflow.concurrency = concurrency;
                workflow.inputs = inputs;
                workflow.steps = steps;
                workflow.outputs = outputs;
                workflow.definitions = definitions.into_iter().collect();
                workflow
            },
        )
}

fn noisy_source() -> impl Strategy<Value = String> {
    let pieces = [
        "workflow",
        "step",
        "input",
        "output",
        "define",
        "node",
        "uses",
        "is",
        "after",
        "and",
        "when",
        "retry",
        "times",
        "timeout",
        "note",
        "concurrency",
        "secret",
        "not",
        "succeeded",
        ",",
        ":",
        "=",
        "\"",
        "\"x\"",
        "\"{a}\"",
        "{",
        "}",
        "[",
        "]",
        "(",
        ")",
        "\\",
        "\n",
        "\r\n",
        "  ",
        "\t",
        " ",
        "a.b",
        "shell.run",
        "1",
        "-",
        "10m",
        "é",
        "\u{1F600}",
        "==",
        "<",
    ];
    prop_oneof![
        vec(prop::sample::select(pieces.to_vec()), 0..60).prop_map(|parts| parts.concat()),
        any::<String>(),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn printed_workflows_parse_back_to_the_same_document(workflow in workflow()) {
        let text = print(&workflow);
        let parsed = parse(&text);
        prop_assert!(!parsed.has_errors(), "{}\n{:#?}", text, parsed.diagnostics);
        prop_assert_eq!(&parsed.workflow, &workflow, "{}", text);
        prop_assert_eq!(format(&text).unwrap(), text);
    }

    #[test]
    fn printed_expressions_parse_back_to_the_same_tree(expr in expression()) {
        let printed = expr::print(&expr);
        let reparsed = expr::parse(&printed);
        prop_assert!(reparsed.is_ok(), "{} -> {:?}", printed, reparsed);
        prop_assert_eq!(shape(&reparsed.unwrap()), shape(&expr), "{}", printed);
    }

    #[test]
    fn parsing_and_highlighting_never_fail_on_arbitrary_input(source in noisy_source()) {
        let parsed = parse(&source);
        for diagnostic in &parsed.diagnostics {
            prop_assert!(diagnostic.span.start <= diagnostic.span.end && diagnostic.span.end <= source.len());
            prop_assert!(source.is_char_boundary(diagnostic.span.start) && source.is_char_boundary(diagnostic.span.end));
        }
        let mut previous_end = 0;
        for token in highlight(&source) {
            prop_assert!(token.span.start >= previous_end && token.span.start < token.span.end);
            prop_assert!(token.span.end <= source.len());
            prop_assert!(source.is_char_boundary(token.span.start) && source.is_char_boundary(token.span.end));
            previous_end = token.span.end;
        }
    }
}
