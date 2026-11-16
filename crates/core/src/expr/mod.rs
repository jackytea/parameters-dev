mod lexer;
mod parser;

pub use lexer::{Token, TokenKind, tokenize};
pub use parser::parse;

use crate::diagnostic::Span;

pub const MAX_EXPRESSION_LENGTH: usize = 16 * 1024;
pub const MAX_NESTING: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ExprError {
    pub message: String,
    pub span: Span,
}

impl ExprError {
    pub(crate) fn new(message: impl Into<String>, span: Span) -> Self {
        Self {
            message: message.into(),
            span,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Boolean(bool),
    Number(serde_json::Number),
    Text(Vec<TextPart>),
    Secret(String),
    Reference(Reference),
    State {
        step: Segment,
        state: StepState,
    },
    List(Vec<Expr>),
    Object(Vec<(Segment, Expr)>),
    Not(Box<Expr>),
    Binary {
        operator: BinaryOperator,
        left: Box<Expr>,
        right: Box<Expr>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextPart {
    Literal(String),
    Reference(Reference),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub segments: Vec<Segment>,
}

impl Reference {
    #[must_use]
    pub fn root(&self) -> &Segment {
        &self.segments[0]
    }

    #[must_use]
    pub fn span(&self) -> Span {
        self.segments[0]
            .span
            .to(self.segments[self.segments.len() - 1].span)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StepState {
    Succeeded,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinaryOperator {
    And,
    Or,
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}

impl BinaryOperator {
    #[must_use]
    pub const fn is_logical(self) -> bool {
        matches!(self, Self::And | Self::Or)
    }

    #[must_use]
    pub const fn is_ordering(self) -> bool {
        matches!(
            self,
            Self::Less | Self::LessOrEqual | Self::Greater | Self::GreaterOrEqual
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Use<'a> {
    Value(&'a Reference),
    State(&'a Segment),
}

impl Expr {
    pub fn visit_uses<'a>(&'a self, visit: &mut impl FnMut(Use<'a>)) {
        match &self.kind {
            ExprKind::Boolean(_) | ExprKind::Number(_) | ExprKind::Secret(_) => {}
            ExprKind::Text(parts) => {
                for part in parts {
                    if let TextPart::Reference(reference) = part {
                        visit(Use::Value(reference));
                    }
                }
            }
            ExprKind::Reference(reference) => visit(Use::Value(reference)),
            ExprKind::State { step, .. } => visit(Use::State(step)),
            ExprKind::List(items) => {
                for item in items {
                    item.visit_uses(visit);
                }
            }
            ExprKind::Object(entries) => {
                for (_, value) in entries {
                    value.visit_uses(visit);
                }
            }
            ExprKind::Not(inner) => inner.visit_uses(visit),
            ExprKind::Binary { left, right, .. } => {
                left.visit_uses(visit);
                right.visit_uses(visit);
            }
        }
    }

    #[must_use]
    pub fn root_names(&self) -> Vec<&str> {
        let mut names = Vec::new();
        self.visit_uses(&mut |used| {
            names.push(match used {
                Use::Value(reference) => reference.root().name.as_str(),
                Use::State(step) => step.name.as_str(),
            });
        });
        names
    }
}
