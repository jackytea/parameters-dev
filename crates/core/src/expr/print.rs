use std::fmt::Write;

use serde_json::Value;

use super::{BinaryOperator, Expr, ExprKind, Reference, StepState, TextPart};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Level {
    Or,
    And,
    Not,
    Comparison,
    Primary,
}

#[must_use]
pub fn print(expr: &Expr) -> String {
    let mut out = String::new();
    write_expr(expr, Level::Or, &mut out);
    out
}

#[must_use]
pub fn print_value(value: &Value) -> String {
    let mut out = String::new();
    write_value(value, &mut out);
    out
}

#[must_use]
pub fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    escape_into(text, &mut out);
    out.push('"');
    out
}

#[must_use]
pub fn is_bare_key(key: &str) -> bool {
    let mut bytes = key.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn level(expr: &Expr) -> Level {
    match &expr.kind {
        ExprKind::Binary {
            operator: BinaryOperator::Or,
            ..
        } => Level::Or,
        ExprKind::Binary {
            operator: BinaryOperator::And,
            ..
        } => Level::And,
        ExprKind::Not(_) => Level::Not,
        ExprKind::Binary { .. } => Level::Comparison,
        _ => Level::Primary,
    }
}

fn write_expr(expr: &Expr, required: Level, out: &mut String) {
    let parenthesized = level(expr) < required;
    if parenthesized {
        out.push('(');
    }
    match &expr.kind {
        ExprKind::Boolean(value) => out.push_str(if *value { "true" } else { "false" }),
        ExprKind::Number(number) => {
            let _ = write!(out, "{number}");
        }
        ExprKind::Text(parts) => write_text(parts, out),
        ExprKind::Secret(name) => {
            out.push_str("secret ");
            out.push_str(&quote(name));
        }
        ExprKind::Reference(reference) => write_reference(reference, out),
        ExprKind::State { step, state } => {
            out.push_str(&step.name);
            out.push_str(match state {
                StepState::Succeeded => " succeeded",
                StepState::Failed => " failed",
                StepState::Skipped => " skipped",
            });
        }
        ExprKind::List(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                write_expr(item, Level::Or, out);
            }
            out.push(']');
        }
        ExprKind::Object(entries) => {
            if entries.is_empty() {
                out.push_str("{}");
            } else {
                out.push_str("{ ");
                for (index, (key, value)) in entries.iter().enumerate() {
                    if index > 0 {
                        out.push_str(", ");
                    }
                    write_key(&key.name, out);
                    write_expr(value, Level::Or, out);
                }
                out.push_str(" }");
            }
        }
        ExprKind::Not(inner) => {
            out.push_str("not ");
            write_expr(inner, Level::Not, out);
        }
        ExprKind::Binary {
            operator,
            left,
            right,
        } => {
            let (left_level, right_level) = match operator {
                BinaryOperator::Or => (Level::Or, Level::And),
                BinaryOperator::And => (Level::And, Level::Not),
                _ => (Level::Primary, Level::Primary),
            };
            write_expr(left, left_level, out);
            out.push(' ');
            out.push_str(operator_symbol(*operator));
            out.push(' ');
            write_expr(right, right_level, out);
        }
    }
    if parenthesized {
        out.push(')');
    }
}

fn write_text(parts: &[TextPart], out: &mut String) {
    out.push('"');
    for part in parts {
        match part {
            TextPart::Literal(text) => escape_into(text, out),
            TextPart::Reference(reference) => {
                out.push('{');
                write_reference(reference, out);
                out.push('}');
            }
        }
    }
    out.push('"');
}

fn write_reference(reference: &Reference, out: &mut String) {
    for (index, segment) in reference.segments.iter().enumerate() {
        if index > 0 {
            out.push('.');
        }
        out.push_str(&segment.name);
    }
}

fn write_key(key: &str, out: &mut String) {
    if is_bare_key(key) {
        out.push_str(key);
    } else {
        out.push_str(&quote(key));
    }
    out.push_str(": ");
}

fn write_value(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(boolean) => out.push_str(if *boolean { "true" } else { "false" }),
        Value::Number(number) => {
            let _ = write!(out, "{number}");
        }
        Value::String(text) => out.push_str(&quote(text)),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                write_value(item, out);
            }
            out.push(']');
        }
        Value::Object(object) if object.is_empty() => out.push_str("{}"),
        Value::Object(object) => {
            let mut entries: Vec<(&String, &Value)> = object.iter().collect();
            entries.sort_unstable_by(|left, right| left.0.cmp(right.0));
            out.push_str("{ ");
            for (index, (key, item)) in entries.into_iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                write_key(key, out);
                write_value(item, out);
            }
            out.push_str(" }");
        }
    }
}

fn escape_into(text: &str, out: &mut String) {
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            control if control.is_control() => {
                let _ = write!(out, "\\u{{{:x}}}", u32::from(control));
            }
            other => out.push(other),
        }
    }
}

const fn operator_symbol(operator: BinaryOperator) -> &'static str {
    match operator {
        BinaryOperator::And => "and",
        BinaryOperator::Or => "or",
        BinaryOperator::Equal => "==",
        BinaryOperator::NotEqual => "!=",
        BinaryOperator::Less => "<",
        BinaryOperator::LessOrEqual => "<=",
        BinaryOperator::Greater => ">",
        BinaryOperator::GreaterOrEqual => ">=",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::expr::parse;

    fn canonical(source: &str) -> String {
        print(&parse(source).unwrap())
    }

    #[test]
    fn normalizes_spacing_and_redundant_parentheses() {
        assert_eq!(canonical("(a)and(b  ==  1)"), "a and b == 1");
        assert_eq!(
            canonical("{a:1,\"b c\":[ x ,y.z ]}"),
            r#"{ a: 1, "b c": [x, y.z] }"#
        );
        assert_eq!(canonical("secret   \"TOKEN\""), r#"secret "TOKEN""#);
        assert_eq!(canonical("deploy   failed"), "deploy failed");
    }

    #[test]
    fn keeps_parentheses_that_change_meaning() {
        for source in [
            "a and (b or c)",
            "(a or b) and c",
            "a or (b or c)",
            "a and (b and c)",
            "not (a and b)",
            "(not a) == b",
            "(a == b) == c",
            "not not a",
            "not a == b",
        ] {
            assert_eq!(canonical(source), source);
            assert_eq!(canonical(&canonical(source)), canonical(source));
        }
        assert_eq!(canonical("(a or b) or c"), "a or b or c");
    }

    #[test]
    fn escapes_text_so_it_parses_back_to_the_same_value() {
        let text = "say \"hi\" {not a ref} \\ \n\t\r \u{1} \u{7f} é";
        let quoted = quote(text);
        assert_eq!(
            quoted,
            r#""say \"hi\" \{not a ref\} \\ \n\t\r \u{1} \u{7f} é""#
        );
        assert_eq!(canonical(&quoted), quoted);
        let ExprKind::Text(parts) = parse(&quoted).unwrap().kind else {
            panic!("expected text");
        };
        assert_eq!(parts, [TextPart::Literal(text.to_owned())]);
    }

    #[test]
    fn prints_json_values_with_sorted_keys() {
        assert_eq!(
            print_value(&json!({ "b": [1, 2.5, true], "a": "x", "Content-Type": {} })),
            r#"{ "Content-Type": {}, a: "x", b: [1, 2.5, true] }"#
        );
        assert_eq!(print_value(&json!(1e100)), "1e+100");
        assert_eq!(canonical("1e100"), "1e+100");
        assert_eq!(canonical("1e+100"), "1e+100");
    }
}
