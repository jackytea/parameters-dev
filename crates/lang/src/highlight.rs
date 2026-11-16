use parameters_core::Span;
use parameters_core::name::is_keyword;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenClass {
    Keyword,
    Name,
    NodeType,
    Text,
    Interpolation,
    Number,
    Operator,
    Punctuation,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Highlight {
    pub span: Span,
    pub class: TokenClass,
}

#[must_use]
pub fn highlight(source: &str) -> Vec<Highlight> {
    let mut scanner = Scanner {
        source,
        position: 0,
        out: Vec::new(),
    };
    scanner.run();
    scanner.out
}

struct Scanner<'a> {
    source: &'a str,
    position: usize,
    out: Vec<Highlight>,
}

impl<'a> Scanner<'a> {
    fn run(&mut self) {
        let mut previous_word: Option<&'a str> = None;
        while let Some(character) = self.peek() {
            let start = self.position;
            if character.is_whitespace() {
                self.advance(character);
                if character == '\n' {
                    previous_word = None;
                }
                continue;
            }
            let after_word = previous_word.take();
            if character.is_ascii_alphabetic() || character == '_' {
                let qualified = matches!(after_word, Some("uses" | "node"));
                self.take_while(|next| {
                    next.is_ascii_alphanumeric() || next == '_' || (qualified && next == '.')
                });
                let word = &self.source[start..self.position];
                let class = if qualified {
                    TokenClass::NodeType
                } else if is_keyword(word) {
                    TokenClass::Keyword
                } else {
                    TokenClass::Name
                };
                self.push(start, class);
                previous_word = Some(word);
            } else if character.is_ascii_digit()
                || (character == '-'
                    && self.peek_second().is_some_and(|next| next.is_ascii_digit()))
            {
                self.advance(character);
                self.take_while(|next| {
                    next.is_ascii_digit() || matches!(next, '.' | 'e' | 'E' | '+' | '-')
                });
                if after_word == Some("timeout") {
                    self.take_while(|next| next.is_ascii_alphabetic());
                }
                self.push(start, TokenClass::Number);
            } else if character == '"' {
                self.text();
            } else {
                self.symbol(character);
            }
        }
    }

    fn text(&mut self) {
        let mut segment = self.position;
        self.advance('"');
        while let Some(character) = self.peek() {
            match character {
                '\n' => break,
                '"' => {
                    self.advance(character);
                    break;
                }
                '\\' => {
                    self.advance(character);
                    if let Some(escaped) = self.peek().filter(|&escaped| escaped != '\n') {
                        self.advance(escaped);
                    }
                }
                '{' => {
                    let line_end = self.source[self.position..]
                        .find(['\n', '"'])
                        .map_or(self.source.len(), |offset| self.position + offset);
                    let Some(close) = self.source[self.position..line_end].find('}') else {
                        self.advance(character);
                        continue;
                    };
                    if self.position > segment {
                        self.push(segment, TokenClass::Text);
                    }
                    let start = self.position;
                    self.position += close + 1;
                    self.push(start, TokenClass::Interpolation);
                    segment = self.position;
                }
                _ => self.advance(character),
            }
        }
        if self.position > segment {
            self.push(segment, TokenClass::Text);
        }
    }

    fn symbol(&mut self, character: char) {
        let start = self.position;
        let pair = self.source[start..].get(..2);
        if matches!(pair, Some("==" | "!=" | "<=" | ">=")) {
            self.position += 2;
            self.push(start, TokenClass::Operator);
            return;
        }
        self.advance(character);
        let class = match character {
            '<' | '>' => TokenClass::Operator,
            ':' | ',' | '.' | '=' | '(' | ')' | '[' | ']' | '{' | '}' => TokenClass::Punctuation,
            _ => TokenClass::Invalid,
        };
        self.push(start, class);
    }

    fn peek(&self) -> Option<char> {
        self.source[self.position..].chars().next()
    }

    fn peek_second(&self) -> Option<char> {
        self.source[self.position..].chars().nth(1)
    }

    fn advance(&mut self, character: char) {
        self.position += character.len_utf8();
    }

    fn take_while(&mut self, predicate: impl Fn(char) -> bool) {
        while let Some(character) = self.peek().filter(|&character| predicate(character)) {
            self.advance(character);
        }
    }

    fn push(&mut self, start: usize, class: TokenClass) {
        self.out.push(Highlight {
            span: Span::new(start, self.position),
            class,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classes(source: &str) -> Vec<(&str, TokenClass)> {
        highlight(source)
            .into_iter()
            .map(|token| (&source[token.span.start..token.span.end], token.class))
            .collect()
    }

    #[test]
    fn classifies_statements() {
        use TokenClass::{
            Interpolation, Keyword, Name, NodeType, Number, Operator, Punctuation, Text,
        };
        assert_eq!(
            classes(
                "step build uses shell.run\n  timeout 10m\n  url is \"a{b.c}d\"\n  when x >= 2"
            ),
            [
                ("step", Keyword),
                ("build", Name),
                ("uses", Keyword),
                ("shell.run", NodeType),
                ("timeout", Keyword),
                ("10m", Number),
                ("url", Name),
                ("is", Keyword),
                ("\"a", Text),
                ("{b.c}", Interpolation),
                ("d\"", Text),
                ("when", Keyword),
                ("x", Name),
                (">=", Operator),
                ("2", Number),
            ]
        );
        assert_eq!(
            classes("a.b"),
            [("a", Name), (".", Punctuation), ("b", Name)]
        );
    }

    #[test]
    fn tolerates_unfinished_input() {
        assert_eq!(
            classes("note \"open"),
            [("note", TokenClass::Keyword), ("\"open", TokenClass::Text)]
        );
        assert_eq!(
            classes("\"{x\n~"),
            [("\"{x", TokenClass::Text), ("~", TokenClass::Invalid)]
        );
    }
}
