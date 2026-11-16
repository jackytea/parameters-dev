use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    #[must_use]
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    #[must_use]
    pub const fn offset(self, by: usize) -> Self {
        Self::new(self.start + by, self.end + by)
    }

    #[must_use]
    pub fn to(self, other: Self) -> Self {
        Self::new(self.start.min(other.start), self.end.max(other.end))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PathSegment {
    Key(String),
    Index(usize),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct JsonPath(Vec<PathSegment>);

impl JsonPath {
    #[must_use]
    pub const fn root() -> Self {
        Self(Vec::new())
    }

    #[must_use]
    pub fn key(&self, key: impl Into<String>) -> Self {
        self.with(PathSegment::Key(key.into()))
    }

    #[must_use]
    pub fn index(&self, index: usize) -> Self {
        self.with(PathSegment::Index(index))
    }

    #[must_use]
    pub fn segments(&self) -> &[PathSegment] {
        &self.0
    }

    fn with(&self, segment: PathSegment) -> Self {
        let mut segments = self.0.clone();
        segments.push(segment);
        Self(segments)
    }
}

impl fmt::Display for JsonPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return formatter.write_str("/");
        }
        for segment in &self.0 {
            match segment {
                PathSegment::Key(key) => {
                    write!(formatter, "/{}", key.replace('~', "~0").replace('/', "~1"))?;
                }
                PathSegment::Index(index) => write!(formatter, "/{index}")?,
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Location {
    pub path: JsonPath,
    pub span: Option<Span>,
}

impl Location {
    #[must_use]
    pub const fn at(path: JsonPath) -> Self {
        Self { path, span: None }
    }

    #[must_use]
    pub const fn within(path: JsonPath, span: Span) -> Self {
        Self {
            path,
            span: Some(span),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    Error,
    Warning,
}

impl fmt::Display for Severity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Error => "error",
            Self::Warning => "warning",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Code {
    InvalidName,
    ReservedName,
    InvalidNodeType,
    DuplicateName,
    UnknownNodeType,
    BuiltinShadowed,
    UnknownField,
    MissingField,
    TypeMismatch,
    LiteralSecret,
    ReservedKey,
    InvalidSetting,
    InvalidExpression,
    UnknownReference,
    UnknownOutput,
    InvalidFieldAccess,
    UnknownStep,
    DependencyCycle,
    DefinitionCycle,
    UnknownLayoutEntry,
    LimitExceeded,
}

impl Code {
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::InvalidName => "P0101",
            Self::ReservedName => "P0102",
            Self::InvalidNodeType => "P0103",
            Self::DuplicateName => "P0104",
            Self::UnknownNodeType => "P0201",
            Self::BuiltinShadowed => "P0202",
            Self::UnknownField => "P0203",
            Self::MissingField => "P0204",
            Self::TypeMismatch => "P0205",
            Self::LiteralSecret => "P0206",
            Self::ReservedKey => "P0207",
            Self::InvalidSetting => "P0208",
            Self::InvalidExpression => "P0301",
            Self::UnknownReference => "P0302",
            Self::UnknownOutput => "P0303",
            Self::InvalidFieldAccess => "P0304",
            Self::UnknownStep => "P0305",
            Self::DependencyCycle => "P0401",
            Self::DefinitionCycle => "P0402",
            Self::UnknownLayoutEntry => "P0501",
            Self::LimitExceeded => "P0502",
        }
    }

    #[must_use]
    pub const fn severity(self) -> Severity {
        match self {
            Self::UnknownLayoutEntry => Severity::Warning,
            _ => Severity::Error,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Diagnostic {
    pub code: Code,
    pub message: String,
    pub location: Location,
    pub suggestion: Option<String>,
}

impl Diagnostic {
    #[must_use]
    pub fn new(code: Code, location: Location, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            location,
            suggestion: None,
        }
    }

    #[must_use]
    pub fn with_suggestion(mut self, suggestion: Option<String>) -> Self {
        self.suggestion = suggestion;
        self
    }

    #[must_use]
    pub const fn severity(&self) -> Severity {
        self.code.severity()
    }

    #[must_use]
    pub const fn is_error(&self) -> bool {
        matches!(self.severity(), Severity::Error)
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}[{}] {}",
            self.severity(),
            self.code.id(),
            self.location.path
        )?;
        if let Some(span) = self.location.span {
            write!(formatter, " ({}..{})", span.start, span.end)?;
        }
        write!(formatter, ": {}", self.message)?;
        if let Some(suggestion) = &self.suggestion {
            write!(formatter, "; did you mean `{suggestion}`?")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_render_as_json_pointers() {
        let path = JsonPath::root()
            .key("steps")
            .index(2)
            .key("with")
            .key("a/b~c");
        assert_eq!(path.to_string(), "/steps/2/with/a~1b~0c");
        assert_eq!(JsonPath::root().to_string(), "/");
    }

    #[test]
    fn diagnostics_render_code_location_and_suggestion() {
        let diagnostic = Diagnostic::new(
            Code::UnknownReference,
            Location::within(JsonPath::root().key("steps").index(1), Span::new(0, 7)),
            "unknown reference `chekout`",
        )
        .with_suggestion(Some("checkout".into()));
        assert_eq!(
            diagnostic.to_string(),
            "error[P0302] /steps/1 (0..7): unknown reference `chekout`; did you mean `checkout`?"
        );
    }
}
