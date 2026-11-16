use super::lexer::{Token, TokenKind, tokenize};
use super::{
    BinaryOperator, Expr, ExprError, ExprKind, MAX_NESTING, Reference, Segment, StepState, TextPart,
};
use crate::diagnostic::Span;
use crate::name::{NameProblem, check_name, is_keyword};

pub fn parse(source: &str) -> Result<Expr, ExprError> {
    let tokens = tokenize(source)?;
    let mut parser = Parser {
        source,
        tokens,
        position: 0,
        depth: 0,
    };
    let expr = parser.expression()?;
    match parser.peek() {
        None => Ok(expr),
        Some(token) => Err(ExprError::new(
            format!("unexpected `{}` after the expression", parser.text(token)),
            token.span,
        )),
    }
}

struct Parser<'a> {
    source: &'a str,
    tokens: Vec<Token>,
    position: usize,
    depth: usize,
}

impl Parser<'_> {
    fn expression(&mut self) -> Result<Expr, ExprError> {
        self.depth += 1;
        if self.depth > MAX_NESTING {
            let span = self.peek().map_or_else(|| self.end(), |token| token.span);
            return Err(ExprError::new(
                format!("expression is nested more than {MAX_NESTING} levels deep"),
                span,
            ));
        }
        let expr = self.or();
        self.depth -= 1;
        expr
    }

    fn or(&mut self) -> Result<Expr, ExprError> {
        let mut left = self.and()?;
        while self.eat_word("or") {
            let right = self.and()?;
            left = binary(BinaryOperator::Or, left, right);
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Expr, ExprError> {
        let mut left = self.not()?;
        while self.eat_word("and") {
            let right = self.not()?;
            left = binary(BinaryOperator::And, left, right);
        }
        Ok(left)
    }

    fn not(&mut self) -> Result<Expr, ExprError> {
        if let Some(token) = self.peek().filter(|token| self.is_word(*token, "not")) {
            self.position += 1;
            self.depth += 1;
            if self.depth > MAX_NESTING {
                return Err(ExprError::new(
                    format!("expression is nested more than {MAX_NESTING} levels deep"),
                    token.span,
                ));
            }
            let inner = self.not();
            self.depth -= 1;
            let inner = inner?;
            return Ok(Expr {
                span: token.span.to(inner.span),
                kind: ExprKind::Not(Box::new(inner)),
            });
        }
        self.comparison()
    }

    fn comparison(&mut self) -> Result<Expr, ExprError> {
        let left = self.primary()?;
        let Some(operator) = self
            .peek()
            .and_then(|token| comparison_operator(token.kind))
        else {
            return Ok(left);
        };
        self.position += 1;
        let right = self.primary()?;
        if let Some(token) = self
            .peek()
            .filter(|token| comparison_operator(token.kind).is_some())
        {
            return Err(ExprError::new(
                "comparisons cannot be chained; combine them with `and`",
                token.span,
            ));
        }
        Ok(binary(operator, left, right))
    }

    fn primary(&mut self) -> Result<Expr, ExprError> {
        let token = self.next().ok_or_else(|| {
            ExprError::new("expected a value but the expression ended", self.end())
        })?;
        match token.kind {
            TokenKind::Number => self.number(token),
            TokenKind::Text => Ok(Expr {
                kind: ExprKind::Text(self.text_parts(token)?),
                span: token.span,
            }),
            TokenKind::OpenParen => {
                let inner = self.expression()?;
                let close = self.expect(TokenKind::CloseParen, "`)`")?;
                Ok(Expr {
                    kind: inner.kind,
                    span: token.span.to(close.span),
                })
            }
            TokenKind::OpenBracket => self.list(token),
            TokenKind::OpenBrace => self.object(token),
            TokenKind::Word => self.word(token),
            _ => Err(ExprError::new(
                format!("expected a value but found `{}`", self.text(token)),
                token.span,
            )),
        }
    }

    fn word(&mut self, token: Token) -> Result<Expr, ExprError> {
        let kind = match self.text(token) {
            "true" => ExprKind::Boolean(true),
            "false" => ExprKind::Boolean(false),
            "secret" => return self.secret(token),
            word if is_keyword(word) => {
                return Err(ExprError::new(
                    format!("`{word}` is a reserved keyword and cannot be used here"),
                    token.span,
                ));
            }
            _ => return self.reference_or_state(token),
        };
        Ok(Expr {
            kind,
            span: token.span,
        })
    }

    fn secret(&mut self, keyword: Token) -> Result<Expr, ExprError> {
        let token = self
            .next()
            .filter(|token| token.kind == TokenKind::Text)
            .ok_or_else(|| {
                ExprError::new(
                    "`secret` must be followed by a name in quotes",
                    keyword.span,
                )
            })?;
        let parts = self.text_parts(token)?;
        let name = match parts.as_slice() {
            [TextPart::Literal(name)] => name.clone(),
            [] => String::new(),
            _ => {
                return Err(ExprError::new(
                    "secret names cannot contain `{...}` interpolation",
                    token.span,
                ));
            }
        };
        if !is_secret_name(&name) {
            return Err(ExprError::new(
                "secret names may only contain letters, digits and underscores, and cannot start with a digit",
                token.span,
            ));
        }
        Ok(Expr {
            kind: ExprKind::Secret(name),
            span: keyword.span.to(token.span),
        })
    }

    fn reference_or_state(&mut self, first: Token) -> Result<Expr, ExprError> {
        let root = self.root_segment(first)?;
        let state = self
            .peek()
            .filter(|token| token.kind == TokenKind::Word)
            .and_then(|token| step_state(self.text(token)).map(|state| (state, token.span)));
        if let Some((state, span)) = state {
            self.position += 1;
            return Ok(Expr {
                span: root.span.to(span),
                kind: ExprKind::State { step: root, state },
            });
        }
        let mut segments = vec![root];
        while self
            .peek()
            .is_some_and(|token| token.kind == TokenKind::Dot)
        {
            self.position += 1;
            let token = self.expect(TokenKind::Word, "a field name after `.`")?;
            segments.push(Segment {
                name: self.text(token).to_owned(),
                span: token.span,
            });
        }
        let reference = Reference { segments };
        Ok(Expr {
            span: reference.span(),
            kind: ExprKind::Reference(reference),
        })
    }

    fn root_segment(&self, token: Token) -> Result<Segment, ExprError> {
        let name = self.text(token);
        match check_name(name) {
            Ok(()) => Ok(Segment {
                name: name.to_owned(),
                span: token.span,
            }),
            Err(NameProblem::Keyword) => Err(ExprError::new(
                format!("`{name}` is a reserved keyword and cannot be used here"),
                token.span,
            )),
            Err(_) => Err(ExprError::new(
                format!(
                    "`{name}` is not a valid name: use lowercase letters, digits and underscores"
                ),
                token.span,
            )),
        }
    }

    fn list(&mut self, open: Token) -> Result<Expr, ExprError> {
        let mut items = Vec::new();
        let close = loop {
            if let Some(close) = self.eat(TokenKind::CloseBracket) {
                break close;
            }
            items.push(self.expression()?);
            if self.eat(TokenKind::Comma).is_none() {
                break self.expect(TokenKind::CloseBracket, "`,` or `]`")?;
            }
        };
        Ok(Expr {
            kind: ExprKind::List(items),
            span: open.span.to(close.span),
        })
    }

    fn object(&mut self, open: Token) -> Result<Expr, ExprError> {
        let mut entries: Vec<(Segment, Expr)> = Vec::new();
        let close = loop {
            if let Some(close) = self.eat(TokenKind::CloseBrace) {
                break close;
            }
            let key = self.object_key()?;
            if entries
                .iter()
                .any(|(existing, _)| existing.name == key.name)
            {
                return Err(ExprError::new(
                    format!("duplicate key `{}`", key.name),
                    key.span,
                ));
            }
            self.expect(TokenKind::Colon, "`:` after the key")?;
            entries.push((key, self.expression()?));
            if self.eat(TokenKind::Comma).is_none() {
                break self.expect(TokenKind::CloseBrace, "`,` or `}`")?;
            }
        };
        Ok(Expr {
            kind: ExprKind::Object(entries),
            span: open.span.to(close.span),
        })
    }

    fn object_key(&mut self) -> Result<Segment, ExprError> {
        let token = self
            .next()
            .ok_or_else(|| ExprError::new("expected a key but the expression ended", self.end()))?;
        let name = match token.kind {
            TokenKind::Word => self.text(token).to_owned(),
            TokenKind::Text => match self.text_parts(token)?.as_slice() {
                [TextPart::Literal(key)] => key.clone(),
                [] => String::new(),
                _ => {
                    return Err(ExprError::new(
                        "keys cannot contain `{...}` interpolation",
                        token.span,
                    ));
                }
            },
            _ => {
                return Err(ExprError::new(
                    format!("expected a key but found `{}`", self.text(token)),
                    token.span,
                ));
            }
        };
        if name.starts_with('$') {
            return Err(ExprError::new(
                "keys starting with `$` are reserved",
                token.span,
            ));
        }
        Ok(Segment {
            name,
            span: token.span,
        })
    }

    fn number(&self, token: Token) -> Result<Expr, ExprError> {
        let number = self
            .text(token)
            .parse::<serde_json::Number>()
            .map_err(|_| ExprError::new("invalid number", token.span))?;
        Ok(Expr {
            kind: ExprKind::Number(number),
            span: token.span,
        })
    }

    fn text_parts(&self, token: Token) -> Result<Vec<TextPart>, ExprError> {
        let start = token.span.start + 1;
        let body = &self.source[start..token.span.end - 1];
        let mut parts = Vec::new();
        let mut literal = String::new();
        let mut characters = body.char_indices();
        while let Some((offset, character)) = characters.next() {
            match character {
                '\\' => {
                    let escaped = characters.next().map(|(_, escaped)| escaped);
                    let escape_end = start + offset + 1 + escaped.map_or(0, char::len_utf8);
                    literal.push(match escaped {
                        Some('n') => '\n',
                        Some('t') => '\t',
                        Some('r') => '\r',
                        Some(escaped @ ('"' | '\\' | '{' | '}')) => escaped,
                        _ => {
                            return Err(ExprError::new(
                                "unknown escape: use \\\", \\\\, \\n, \\t, \\r, \\{ or \\}",
                                Span::new(start + offset, escape_end),
                            ));
                        }
                    });
                }
                '{' => {
                    let Some(length) = body[offset + 1..].find('}') else {
                        return Err(ExprError::new(
                            "unclosed `{` in text: close it with `}` or write `\\{`",
                            Span::new(start + offset, token.span.end),
                        ));
                    };
                    if !literal.is_empty() {
                        parts.push(TextPart::Literal(std::mem::take(&mut literal)));
                    }
                    let inner_start = start + offset + 1;
                    parts.push(TextPart::Reference(interpolated_reference(
                        &body[offset + 1..offset + 1 + length],
                        inner_start,
                    )?));
                    for _ in 0..=length {
                        characters.next();
                    }
                }
                '}' => {
                    return Err(ExprError::new(
                        "unmatched `}` in text: write `\\}` for a literal brace",
                        Span::new(start + offset, start + offset + 1),
                    ));
                }
                _ => literal.push(character),
            }
        }
        if !literal.is_empty() {
            parts.push(TextPart::Literal(literal));
        }
        Ok(parts)
    }

    fn peek(&self) -> Option<Token> {
        self.tokens.get(self.position).copied()
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.peek()?;
        self.position += 1;
        Some(token)
    }

    fn eat(&mut self, kind: TokenKind) -> Option<Token> {
        let token = self.peek().filter(|token| token.kind == kind)?;
        self.position += 1;
        Some(token)
    }

    fn eat_word(&mut self, word: &str) -> bool {
        let matched = self.peek().is_some_and(|token| self.is_word(token, word));
        if matched {
            self.position += 1;
        }
        matched
    }

    fn expect(&mut self, kind: TokenKind, expected: &str) -> Result<Token, ExprError> {
        match self.peek() {
            Some(token) if token.kind == kind => {
                self.position += 1;
                Ok(token)
            }
            Some(token) => Err(ExprError::new(
                format!("expected {expected} but found `{}`", self.text(token)),
                token.span,
            )),
            None => Err(ExprError::new(
                format!("expected {expected} but the expression ended"),
                self.end(),
            )),
        }
    }

    fn is_word(&self, token: Token, word: &str) -> bool {
        token.kind == TokenKind::Word && self.text(token) == word
    }

    fn text(&self, token: Token) -> &str {
        &self.source[token.span.start..token.span.end]
    }

    fn end(&self) -> Span {
        Span::new(self.source.len(), self.source.len())
    }
}

fn interpolated_reference(inner: &str, offset: usize) -> Result<Reference, ExprError> {
    let span = Span::new(offset, offset + inner.len());
    let invalid = || {
        ExprError::new(
            "only references such as `{step.output}` can be interpolated in text",
            span,
        )
    };
    let tokens = tokenize(inner).map_err(|_| invalid())?;
    let mut parser = Parser {
        source: inner,
        tokens,
        position: 0,
        depth: 0,
    };
    let first = parser.next().filter(|token| token.kind == TokenKind::Word);
    let Some(first) = first else {
        return Err(invalid());
    };
    let expr = parser
        .reference_or_state(first)
        .map_err(|error| ExprError {
            span: error.span.offset(offset),
            ..error
        })?;
    match (expr.kind, parser.peek()) {
        (ExprKind::Reference(reference), None) => Ok(Reference {
            segments: reference
                .segments
                .into_iter()
                .map(|segment| Segment {
                    span: segment.span.offset(offset),
                    ..segment
                })
                .collect(),
        }),
        _ => Err(invalid()),
    }
}

fn binary(operator: BinaryOperator, left: Expr, right: Expr) -> Expr {
    Expr {
        span: left.span.to(right.span),
        kind: ExprKind::Binary {
            operator,
            left: Box::new(left),
            right: Box::new(right),
        },
    }
}

fn comparison_operator(kind: TokenKind) -> Option<BinaryOperator> {
    Some(match kind {
        TokenKind::Equal => BinaryOperator::Equal,
        TokenKind::NotEqual => BinaryOperator::NotEqual,
        TokenKind::Less => BinaryOperator::Less,
        TokenKind::LessOrEqual => BinaryOperator::LessOrEqual,
        TokenKind::Greater => BinaryOperator::Greater,
        TokenKind::GreaterOrEqual => BinaryOperator::GreaterOrEqual,
        _ => return None,
    })
}

fn step_state(word: &str) -> Option<StepState> {
    match word {
        "succeeded" => Some(StepState::Succeeded),
        "failed" => Some(StepState::Failed),
        "skipped" => Some(StepState::Skipped),
        _ => None,
    }
}

fn is_secret_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= 128
        && bytes
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(path: &str) -> Vec<String> {
        match parse(path).unwrap().kind {
            ExprKind::Reference(reference) => reference
                .segments
                .into_iter()
                .map(|segment| segment.name)
                .collect(),
            other => panic!("expected a reference, got {other:?}"),
        }
    }

    #[test]
    fn parses_references_with_field_paths() {
        assert_eq!(reference("checkout.path"), ["checkout", "path"]);
        assert_eq!(
            reference("upload.body.deploy_url"),
            ["upload", "body", "deploy_url"]
        );
        assert_eq!(reference("response.body.for"), ["response", "body", "for"]);
    }

    #[test]
    fn parses_interpolated_text() {
        let ExprKind::Text(parts) = parse(r#""{checkout.path}/dist \{x\}""#).unwrap().kind else {
            panic!("expected text");
        };
        let [TextPart::Reference(reference), TextPart::Literal(rest)] = parts.as_slice() else {
            panic!("unexpected parts {parts:?}");
        };
        assert_eq!(reference.segments[1].name, "path");
        assert_eq!(reference.span(), Span::new(2, 15));
        assert_eq!(rest, "/dist {x}");
    }

    #[test]
    fn parses_secrets_states_and_logic_with_precedence() {
        let expr = parse(r#"not deploy failed and region == "eu" or retry_all"#).unwrap();
        let ExprKind::Binary {
            operator: BinaryOperator::Or,
            left,
            ..
        } = expr.kind
        else {
            panic!("expected `or` at the top");
        };
        let ExprKind::Binary {
            operator: BinaryOperator::And,
            left: negated,
            ..
        } = left.kind
        else {
            panic!("expected `and` under `or`");
        };
        assert!(matches!(negated.kind, ExprKind::Not(_)));
        assert!(matches!(
            parse(r#"secret "NETLIFY_TOKEN""#).unwrap().kind,
            ExprKind::Secret(name) if name == "NETLIFY_TOKEN"
        ));
        assert_eq!(parse("deploy succeeded").unwrap().root_names(), ["deploy"]);
    }

    #[test]
    fn parses_lists_and_objects() {
        let expr =
            parse(r#"{ text: "Deployed {deploy.url}", "Content-Type": "json", n: [1, -2.5,] }"#)
                .unwrap();
        let ExprKind::Object(entries) = &expr.kind else {
            panic!("expected an object");
        };
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[1].0.name, "Content-Type");
        assert_eq!(expr.root_names(), ["deploy"]);
    }

    #[test]
    fn reports_errors_with_spans() {
        for (source, span, needle) in [
            ("checkout.", Span::new(9, 9), "field name"),
            ("a == b == c", Span::new(7, 9), "chained"),
            ("step.id", Span::new(0, 4), "reserved"),
            ("Build", Span::new(0, 5), "not a valid name"),
            (r#""{a b}""#, Span::new(2, 5), "only references"),
            (r#""}""#, Span::new(1, 2), "unmatched"),
            (r#""\q""#, Span::new(1, 3), "unknown escape"),
            (r#""\é""#, Span::new(1, 4), "unknown escape"),
            (r#"secret "a-b""#, Span::new(7, 12), "secret names"),
            (r#"secret "{a}""#, Span::new(7, 12), "interpolation"),
            ("{a: 1, a: 2}", Span::new(7, 8), "duplicate key"),
            (r#"{"$expr": 1}"#, Span::new(1, 8), "reserved"),
            ("[1, 2", Span::new(5, 5), "`,` or `]`"),
            ("a b", Span::new(2, 3), "after the expression"),
            ("", Span::new(0, 0), "expected a value"),
        ] {
            let error = parse(source).unwrap_err();
            assert_eq!(error.span, span, "{source}");
            assert!(
                error.message.contains(needle),
                "{source}: {}",
                error.message
            );
        }
    }

    #[test]
    fn limits_nesting_depth() {
        let deep = format!(
            "{}a{}",
            "(".repeat(MAX_NESTING + 1),
            ")".repeat(MAX_NESTING + 1)
        );
        assert!(parse(&deep).unwrap_err().message.contains("nested"));
        let negations = format!("{}a", "not ".repeat(MAX_NESTING + 1));
        assert!(parse(&negations).unwrap_err().message.contains("nested"));
        let shallow = format!(
            "{}a{}",
            "(".repeat(MAX_NESTING - 1),
            ")".repeat(MAX_NESTING - 1)
        );
        assert!(parse(&shallow).is_ok());
    }
}
