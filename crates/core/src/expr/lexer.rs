use super::{ExprError, MAX_EXPRESSION_LENGTH};
use crate::diagnostic::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Word,
    Number,
    Text,
    Dot,
    Comma,
    Colon,
    OpenParen,
    CloseParen,
    OpenBracket,
    CloseBracket,
    OpenBrace,
    CloseBrace,
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

pub fn tokenize(source: &str) -> Result<Vec<Token>, ExprError> {
    if source.len() > MAX_EXPRESSION_LENGTH {
        return Err(ExprError::new(
            format!("expression is longer than {MAX_EXPRESSION_LENGTH} bytes"),
            Span::new(0, source.len()),
        ));
    }
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut position = 0;
    while position < bytes.len() {
        let start = position;
        let byte = bytes[position];
        let kind = match byte {
            b' ' | b'\t' | b'\r' | b'\n' => {
                position += 1;
                continue;
            }
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                position += count_while(&bytes[position..], |byte| {
                    byte.is_ascii_alphanumeric() || byte == b'_'
                });
                TokenKind::Word
            }
            b'0'..=b'9' => {
                position = scan_number(bytes, position);
                TokenKind::Number
            }
            b'-' if bytes.get(position + 1).is_some_and(u8::is_ascii_digit) => {
                position = scan_number(bytes, position + 1);
                TokenKind::Number
            }
            b'"' => {
                position = scan_text(source, position)?;
                TokenKind::Text
            }
            _ => {
                let (kind, length) = symbol(bytes, position).ok_or_else(|| {
                    let character = source[position..].chars().next().unwrap_or('?');
                    ExprError::new(
                        format!("unexpected character `{character}`"),
                        Span::new(position, position + character.len_utf8()),
                    )
                })?;
                position += length;
                kind
            }
        };
        tokens.push(Token {
            kind,
            span: Span::new(start, position),
        });
    }
    Ok(tokens)
}

fn count_while(bytes: &[u8], predicate: impl Fn(u8) -> bool) -> usize {
    bytes.iter().take_while(|&&byte| predicate(byte)).count()
}

fn scan_number(bytes: &[u8], start: usize) -> usize {
    let mut position = start + count_while(&bytes[start..], |byte| byte.is_ascii_digit());
    if bytes.get(position) == Some(&b'.') && bytes.get(position + 1).is_some_and(u8::is_ascii_digit)
    {
        position += 1;
        position += count_while(&bytes[position..], |byte| byte.is_ascii_digit());
    }
    position
}

fn scan_text(source: &str, start: usize) -> Result<usize, ExprError> {
    let bytes = source.as_bytes();
    let mut position = start + 1;
    while position < bytes.len() {
        match bytes[position] {
            b'"' => return Ok(position + 1),
            b'\\' => position += 2,
            _ => position += 1,
        }
    }
    Err(ExprError::new(
        "unterminated text: add a closing `\"`",
        Span::new(start, source.len()),
    ))
}

fn symbol(bytes: &[u8], position: usize) -> Option<(TokenKind, usize)> {
    let next = bytes.get(position + 1).copied();
    Some(match (bytes[position], next) {
        (b'=', Some(b'=')) => (TokenKind::Equal, 2),
        (b'!', Some(b'=')) => (TokenKind::NotEqual, 2),
        (b'<', Some(b'=')) => (TokenKind::LessOrEqual, 2),
        (b'>', Some(b'=')) => (TokenKind::GreaterOrEqual, 2),
        (b'<', _) => (TokenKind::Less, 1),
        (b'>', _) => (TokenKind::Greater, 1),
        (b'.', _) => (TokenKind::Dot, 1),
        (b',', _) => (TokenKind::Comma, 1),
        (b':', _) => (TokenKind::Colon, 1),
        (b'(', _) => (TokenKind::OpenParen, 1),
        (b')', _) => (TokenKind::CloseParen, 1),
        (b'[', _) => (TokenKind::OpenBracket, 1),
        (b']', _) => (TokenKind::CloseBracket, 1),
        (b'{', _) => (TokenKind::OpenBrace, 1),
        (b'}', _) => (TokenKind::CloseBrace, 1),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<TokenKind> {
        tokenize(source)
            .unwrap()
            .into_iter()
            .map(|token| token.kind)
            .collect()
    }

    #[test]
    fn tokenizes_operators_words_and_literals() {
        assert_eq!(
            kinds(r#"a.b >= -1.5 and "x\"y" != [1, {k: 2}]"#),
            [
                TokenKind::Word,
                TokenKind::Dot,
                TokenKind::Word,
                TokenKind::GreaterOrEqual,
                TokenKind::Number,
                TokenKind::Word,
                TokenKind::Text,
                TokenKind::NotEqual,
                TokenKind::OpenBracket,
                TokenKind::Number,
                TokenKind::Comma,
                TokenKind::OpenBrace,
                TokenKind::Word,
                TokenKind::Colon,
                TokenKind::Number,
                TokenKind::CloseBrace,
                TokenKind::CloseBracket,
            ]
        );
    }

    #[test]
    fn reports_unterminated_text_and_unknown_characters() {
        let error = tokenize(r#"say "hello"#).unwrap_err();
        assert_eq!(error.span, Span::new(4, 10));
        let error = tokenize("a ~ b").unwrap_err();
        assert_eq!(error.span, Span::new(2, 3));
        assert!(error.message.contains('~'));
    }

    #[test]
    fn rejects_overlong_expressions() {
        assert!(tokenize(&"a".repeat(MAX_EXPRESSION_LENGTH + 1)).is_err());
    }
}
