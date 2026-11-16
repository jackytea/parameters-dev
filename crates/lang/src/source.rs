use std::collections::HashMap;
use std::fmt;

use parameters_core::{Diagnostic, JsonPath, Location, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LineColumn {
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    #[must_use]
    pub fn new(source: &str) -> Self {
        let starts = std::iter::once(0)
            .chain(source.match_indices('\n').map(|(offset, _)| offset + 1))
            .collect();
        Self { starts }
    }

    #[must_use]
    pub fn locate(&self, source: &str, offset: usize) -> LineColumn {
        let offset = offset.min(source.len());
        let line = self.starts.partition_point(|&start| start <= offset) - 1;
        let start = self.starts[line];
        let column = source
            .get(start..offset)
            .map_or(0, |prefix| prefix.chars().count());
        LineColumn {
            line: line + 1,
            column: column + 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextDiagnostic {
    pub diagnostic: Diagnostic,
    pub span: Span,
    pub start: LineColumn,
    pub end: LineColumn,
}

impl TextDiagnostic {
    #[must_use]
    pub fn new(diagnostic: Diagnostic, span: Span, source: &str, lines: &LineIndex) -> Self {
        Self {
            diagnostic,
            span,
            start: lines.locate(source, span.start),
            end: lines.locate(source, span.end),
        }
    }

    #[must_use]
    pub const fn is_error(&self) -> bool {
        self.diagnostic.is_error()
    }
}

impl fmt::Display for TextDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let diagnostic = &self.diagnostic;
        write!(
            formatter,
            "{}:{}: {}[{}]: {}",
            self.start.line,
            self.start.column,
            diagnostic.severity(),
            diagnostic.code.id(),
            diagnostic.message
        )?;
        if let Some(suggestion) = &diagnostic.suggestion {
            write!(formatter, "; did you mean `{suggestion}`?")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    span: Span,
    value_start: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceMap {
    entries: HashMap<JsonPath, Entry>,
}

impl SourceMap {
    pub(crate) fn record(&mut self, path: JsonPath, span: Span) {
        self.entries.insert(
            path,
            Entry {
                span,
                value_start: None,
            },
        );
    }

    pub(crate) fn record_value(&mut self, path: JsonPath, span: Span, value_start: usize) {
        self.entries.insert(
            path,
            Entry {
                span,
                value_start: Some(value_start),
            },
        );
    }

    #[must_use]
    pub fn locate(&self, location: &Location) -> Option<Span> {
        if let Some(entry) = self.entries.get(&location.path) {
            return Some(match (location.span, entry.value_start) {
                (Some(inner), Some(start)) => inner.offset(start),
                _ => entry.span,
            });
        }
        let mut path = location.path.parent();
        while let Some(current) = path {
            if let Some(entry) = self.entries.get(&current) {
                return Some(entry.span);
            }
            path = current.parent();
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locates_lines_and_character_columns() {
        let source = "ab\nçd\n\nx";
        let lines = LineIndex::new(source);
        assert_eq!(lines.locate(source, 0), LineColumn { line: 1, column: 1 });
        assert_eq!(lines.locate(source, 3), LineColumn { line: 2, column: 1 });
        assert_eq!(lines.locate(source, 5), LineColumn { line: 2, column: 2 });
        assert_eq!(lines.locate(source, 7), LineColumn { line: 3, column: 1 });
        assert_eq!(lines.locate(source, 8), LineColumn { line: 4, column: 1 });
        assert_eq!(lines.locate(source, 99), LineColumn { line: 4, column: 2 });
    }

    #[test]
    fn falls_back_to_the_closest_recorded_ancestor() {
        let mut map = SourceMap::default();
        let step = JsonPath::root().key("steps").index(0);
        map.record(JsonPath::root(), Span::new(0, 10));
        map.record(step.clone(), Span::new(20, 40));
        map.record_value(step.key("with").key("url"), Span::new(25, 40), 32);
        let inner = Location::within(step.key("with").key("url"), Span::new(1, 4));
        assert_eq!(map.locate(&inner), Some(Span::new(33, 36)));
        let whole = Location::at(step.key("with").key("url"));
        assert_eq!(map.locate(&whole), Some(Span::new(25, 40)));
        let nested = Location::at(step.key("with").key("url").key("deep"));
        assert_eq!(map.locate(&nested), Some(Span::new(25, 40)));
        let sibling = Location::at(step.key("with").key("body"));
        assert_eq!(map.locate(&sibling), Some(Span::new(20, 40)));
        let missing = Location::at(JsonPath::root().key("layout").key("x"));
        assert_eq!(map.locate(&missing), Some(Span::new(0, 10)));
    }
}
