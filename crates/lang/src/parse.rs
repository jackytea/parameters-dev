use std::collections::HashSet;

use parameters_core::expr::{self, Expr, ExprKind, TextPart};
use parameters_core::name::is_keyword;
use parameters_core::suggest::closest;
use parameters_core::{
    Code, Definition, Diagnostic, Duration, FieldValue, Input, JsonPath, Location, OrderedMap,
    Output, Span, Step, ValueType, Workflow,
};
use serde_json::Value;

use crate::source::{LineIndex, SourceMap, TextDiagnostic};

pub const MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;
const MAX_DEPTH: usize = 3;
const ROOT_STATEMENTS: [&str; 5] = ["workflow", "input", "step", "output", "define"];
const STEP_SETTINGS: [&str; 5] = ["note", "after", "when", "retry", "timeout"];
const NOTE_HINT: &str =
    "write a note as text in double quotes, such as `note \"Deploys the site\"`";

#[derive(Debug, Clone)]
pub struct Parsed {
    pub workflow: Workflow,
    pub diagnostics: Vec<TextDiagnostic>,
    pub source_map: SourceMap,
    lines: LineIndex,
}

impl Parsed {
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(TextDiagnostic::is_error)
    }

    #[must_use]
    pub const fn line_index(&self) -> &LineIndex {
        &self.lines
    }
}

#[must_use]
pub fn parse(source: &str) -> Parsed {
    let mut parser = Parser {
        source,
        lines: LineIndex::new(source),
        diagnostics: Vec::new(),
        map: SourceMap::default(),
    };
    let workflow = parser.workflow();
    parser
        .diagnostics
        .sort_by_key(|diagnostic| (diagnostic.span.start, diagnostic.span.end));
    Parsed {
        workflow,
        diagnostics: parser.diagnostics,
        source_map: parser.map,
        lines: parser.lines,
    }
}

#[must_use]
pub fn literal_value(expr: &Expr) -> Option<Value> {
    Some(match &expr.kind {
        ExprKind::Boolean(value) => Value::Bool(*value),
        ExprKind::Number(number) => Value::Number(number.clone()),
        ExprKind::Text(parts) => Value::String(match parts.as_slice() {
            [] => String::new(),
            [TextPart::Literal(text)] => text.clone(),
            _ => return None,
        }),
        ExprKind::List(items) => {
            Value::Array(items.iter().map(literal_value).collect::<Option<_>>()?)
        }
        ExprKind::Object(entries) => Value::Object(
            entries
                .iter()
                .map(|(key, value)| Some((key.name.clone(), literal_value(value)?)))
                .collect::<Option<_>>()?,
        ),
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy)]
struct Line {
    indent: usize,
    start: usize,
    end: usize,
}

impl Line {
    const fn span(self) -> Span {
        Span::new(self.start, self.end)
    }
}

struct Block {
    line: Line,
    children: Vec<Block>,
}

struct Parser<'a> {
    source: &'a str,
    lines: LineIndex,
    diagnostics: Vec<TextDiagnostic>,
    map: SourceMap,
}

impl<'a> Parser<'a> {
    fn workflow(&mut self) -> Workflow {
        let mut workflow = Workflow::new("");
        let root = JsonPath::root();
        if self.source.len() > MAX_SOURCE_BYTES {
            self.error(
                Code::LimitExceeded,
                Span::new(0, 0),
                &root,
                format!(
                    "the file is {} bytes; the limit is {MAX_SOURCE_BYTES} bytes",
                    self.source.len()
                ),
            );
            return workflow;
        }
        let lines = self.logical_lines();
        let blocks = self.blocks(&lines, &mut 0, None, 0);
        let starts_with_workflow = blocks
            .first()
            .is_some_and(|first| self.first_word(first.line) == Some("workflow"));
        if !starts_with_workflow {
            let span = blocks
                .first()
                .map_or(Span::new(0, 0), |first| first.line.span());
            self.error(
                Code::InvalidSyntax,
                span,
                &root,
                "a workflow file starts with `workflow \"Name\"`",
            );
        }
        let mut seen_workflow = false;
        for block in &blocks {
            let mut cursor = Cursor::new(self.source, block.line);
            let Some(keyword) = cursor.word() else {
                continue;
            };
            match cursor.text(keyword) {
                "workflow" => {
                    self.header(block, &mut cursor, &mut workflow, &mut seen_workflow);
                }
                "input" => self.input(block, &mut cursor, &mut workflow.inputs, &root),
                "step" => self.step(block, &mut cursor, &mut workflow.steps, &root),
                "output" => self.output(block, &mut cursor, &mut workflow.outputs, &root),
                "define" => self.definition(block, &mut cursor, &mut workflow.definitions),
                word => self.unexpected(
                    keyword,
                    word,
                    &ROOT_STATEMENTS,
                    "expected `workflow`, `input`, `step`, `output` or `define node`",
                    &root,
                ),
            }
        }
        workflow
    }

    fn header(
        &mut self,
        block: &Block,
        cursor: &mut Cursor<'a>,
        workflow: &mut Workflow,
        seen: &mut bool,
    ) {
        let root = JsonPath::root();
        if std::mem::replace(seen, true) {
            self.error(
                Code::DuplicateStatement,
                block.line.span(),
                &root,
                "a file can only contain one `workflow` statement",
            );
            return;
        }
        self.map.record(root.clone(), block.line.span());
        if let Some(name) = self.text_after(
            cursor,
            block.line,
            &root.key("name"),
            "the workflow needs a name in double quotes, such as `workflow \"Deploy\"`",
        ) {
            workflow.name = name;
        }
        let mut seen = HashSet::new();
        for child in &block.children {
            self.no_children(child);
            let mut cursor = Cursor::new(self.source, child.line);
            let Some(keyword) = cursor.word() else {
                continue;
            };
            let word = cursor.text(keyword);
            if !["note", "concurrency"].contains(&word) {
                self.unexpected(
                    keyword,
                    word,
                    &["note", "concurrency"],
                    "a workflow can only contain `note` and `concurrency`",
                    &root,
                );
            } else if self.once(&mut seen, word, child.line, &root) {
                if word == "note" {
                    workflow.note =
                        self.text_after(&mut cursor, child.line, &root.key("note"), NOTE_HINT);
                } else {
                    workflow.concurrency = self.number_after(
                        &mut cursor,
                        child.line,
                        &root.key("concurrency"),
                        "write `concurrency` followed by a whole number, such as `concurrency 4`",
                    );
                }
            }
        }
    }

    fn input(
        &mut self,
        block: &Block,
        cursor: &mut Cursor<'a>,
        inputs: &mut Vec<Input>,
        base: &JsonPath,
    ) {
        let path = base.key("inputs").index(inputs.len());
        let hint = "write inputs as `input name: type`, such as `input branch: text`";
        let Some(name) = cursor.word() else {
            self.error(Code::InvalidSyntax, block.line.span(), &path, hint);
            return;
        };
        if !cursor.eat(':') {
            self.error(
                Code::InvalidSyntax,
                block.line.span(),
                &path,
                "expected `:` and a type after the input name, such as `input branch: text`",
            );
            return;
        }
        let Some(value_type) = self.value_type(cursor, block.line, &path) else {
            return;
        };
        let mut input = Input {
            name: cursor.text(name).to_owned(),
            value_type,
            default: None,
            note: None,
        };
        self.map.record(path.clone(), block.line.span());
        self.map.record(path.key("name"), name);
        if cursor.eat('=') {
            match cursor.rest() {
                Some(span) => {
                    input.default = self.literal(span, &path.key("default"));
                    self.map.record(path.key("default"), span);
                }
                None => self.error(
                    Code::InvalidSyntax,
                    block.line.span(),
                    &path,
                    "expected a default value after `=`",
                ),
            }
        } else {
            self.expect_end(cursor, &path);
        }
        let mut seen = HashSet::new();
        for child in &block.children {
            self.no_children(child);
            let mut cursor = Cursor::new(self.source, child.line);
            let Some(keyword) = cursor.word() else {
                continue;
            };
            let word = cursor.text(keyword);
            if word != "note" {
                self.unexpected(
                    keyword,
                    word,
                    &["note"],
                    "an input can only contain `note`",
                    &path,
                );
            } else if self.once(&mut seen, word, child.line, &path) {
                input.note = self.text_after(&mut cursor, child.line, &path.key("note"), NOTE_HINT);
            }
        }
        inputs.push(input);
    }

    fn step(
        &mut self,
        block: &Block,
        cursor: &mut Cursor<'a>,
        steps: &mut Vec<Step>,
        base: &JsonPath,
    ) {
        let path = base.key("steps").index(steps.len());
        let Some(id) = cursor.word() else {
            self.error(
                Code::InvalidSyntax,
                block.line.span(),
                &path,
                "write steps as `step id uses node.type`, such as `step build uses shell.run`",
            );
            return;
        };
        if !cursor.eat_word("uses") {
            self.error(
                Code::InvalidSyntax,
                block.line.span(),
                &path,
                "expected `uses` and a node type after the step id, such as `step build uses shell.run`",
            );
            return;
        }
        let Some(uses) = cursor.word() else {
            self.error(
                Code::InvalidSyntax,
                block.line.span(),
                &path,
                "expected a node type after `uses`, such as `shell.run`",
            );
            return;
        };
        self.expect_end(cursor, &path);
        let mut step = Step::new(cursor.text(id), cursor.text(uses));
        self.map.record(path.clone(), block.line.span());
        self.map.record(path.key("id"), id);
        self.map.record(path.key("uses"), uses);
        let mut seen = HashSet::new();
        for child in &block.children {
            self.no_children(child);
            let mut cursor = Cursor::new(self.source, child.line);
            let Some(first) = cursor.word() else {
                continue;
            };
            let word = cursor.text(first);
            if !STEP_SETTINGS.contains(&word) {
                self.field(first, &mut cursor, child.line, &mut step.with, &path);
                continue;
            }
            if !self.once(&mut seen, word, child.line, &path) {
                continue;
            }
            match word {
                "note" => {
                    step.note =
                        self.text_after(&mut cursor, child.line, &path.key("note"), NOTE_HINT);
                }
                "after" => step.after = self.names(&mut cursor, child.line, &path.key("after")),
                "when" => match cursor.rest() {
                    Some(condition) => {
                        step.when = Some(cursor.text(condition).to_owned());
                        self.map
                            .record_value(path.key("when"), child.line.span(), condition.start);
                    }
                    None => self.error(
                        Code::InvalidSyntax,
                        child.line.span(),
                        &path,
                        "write a condition after `when`, such as `when build succeeded`",
                    ),
                },
                "retry" => step.retry = self.retry(&mut cursor, child.line, &path.key("retry")),
                _ => step.timeout = self.timeout(&mut cursor, child.line, &path.key("timeout")),
            }
        }
        steps.push(step);
    }

    fn field(
        &mut self,
        name: Span,
        cursor: &mut Cursor<'a>,
        line: Line,
        with: &mut OrderedMap<FieldValue>,
        path: &JsonPath,
    ) {
        let word = cursor.text(name);
        if !cursor.eat_word("is") {
            let message = if is_keyword(word) {
                format!("`{word}` cannot be used inside a step")
            } else {
                format!("expected `is` after `{word}`; fields are written as `name is value`")
            };
            self.unexpected(name, word, &STEP_SETTINGS, &message, path);
            return;
        }
        let Some(value) = cursor.rest() else {
            self.error(
                Code::InvalidSyntax,
                line.span(),
                path,
                format!("expected a value after `{word} is`"),
            );
            return;
        };
        if with.contains_key(word) {
            self.error(
                Code::DuplicateStatement,
                line.span(),
                path,
                format!("the field `{word}` is already set in this step"),
            );
            return;
        }
        with.insert(word, self.value(value));
        self.map
            .record_value(path.key("with").key(word), line.span(), value.start);
    }

    fn output(
        &mut self,
        block: &Block,
        cursor: &mut Cursor<'a>,
        outputs: &mut Vec<Output>,
        base: &JsonPath,
    ) {
        let path = base.key("outputs").index(outputs.len());
        self.no_children(block);
        let Some(name) = cursor.word() else {
            self.error(
                Code::InvalidSyntax,
                block.line.span(),
                &path,
                "write outputs as `output name is value`, such as `output url is deploy.url`",
            );
            return;
        };
        let value_type = if cursor.eat(':') {
            let Some(value_type) = self.value_type(cursor, block.line, &path) else {
                return;
            };
            Some(value_type)
        } else {
            None
        };
        if !cursor.eat_word("is") {
            self.error(
                Code::InvalidSyntax,
                block.line.span(),
                &path,
                "expected `is` and a value after the output name, such as `output url is deploy.url`",
            );
            return;
        }
        let Some(value) = cursor.rest() else {
            self.error(
                Code::InvalidSyntax,
                block.line.span(),
                &path,
                "expected a value after `is`",
            );
            return;
        };
        self.map.record(path.clone(), block.line.span());
        self.map.record(path.key("name"), name);
        self.map
            .record_value(path.key("value"), block.line.span(), value.start);
        outputs.push(Output {
            name: cursor.text(name).to_owned(),
            value_type,
            value: self.value(value),
        });
    }

    fn definition(
        &mut self,
        block: &Block,
        cursor: &mut Cursor<'a>,
        definitions: &mut OrderedMap<Definition>,
    ) {
        let hint =
            "write custom nodes as `define node namespace.name`, such as `define node acme.deploy`";
        let name = if cursor.eat_word("node") {
            cursor.word()
        } else {
            None
        };
        let Some(name) = name else {
            self.error(
                Code::InvalidSyntax,
                block.line.span(),
                &JsonPath::root(),
                hint,
            );
            return;
        };
        let name = cursor.text(name);
        let path = JsonPath::root().key("definitions").key(name);
        self.expect_end(cursor, &path);
        if definitions.contains_key(name) {
            self.error(
                Code::DuplicateStatement,
                block.line.span(),
                &path,
                format!("the custom node `{name}` is already defined"),
            );
            return;
        }
        self.map.record(path.clone(), block.line.span());
        let mut definition = Definition::default();
        let mut seen = HashSet::new();
        for child in &block.children {
            let mut cursor = Cursor::new(self.source, child.line);
            let Some(keyword) = cursor.word() else {
                continue;
            };
            match cursor.text(keyword) {
                "note" => {
                    self.no_children(child);
                    if self.once(&mut seen, "note", child.line, &path) {
                        definition.note =
                            self.text_after(&mut cursor, child.line, &path.key("note"), NOTE_HINT);
                    }
                }
                "input" => self.input(child, &mut cursor, &mut definition.inputs, &path),
                "step" => self.step(child, &mut cursor, &mut definition.steps, &path),
                "output" => self.output(child, &mut cursor, &mut definition.outputs, &path),
                word => self.unexpected(
                    keyword,
                    word,
                    &["note", "input", "step", "output"],
                    "a custom node can only contain `note`, `input`, `step` and `output`",
                    &path,
                ),
            }
        }
        definitions.insert(name, definition);
    }

    fn value_type(
        &mut self,
        cursor: &mut Cursor<'a>,
        line: Line,
        path: &JsonPath,
    ) -> Option<ValueType> {
        let names = ValueType::ALL.map(ValueType::name);
        let Some(span) = cursor.word() else {
            self.error(
                Code::InvalidSyntax,
                line.span(),
                path,
                format!("expected a type: {}", names.join(", ")),
            );
            return None;
        };
        let word = cursor.text(span);
        let value_type = ValueType::from_name(word);
        if value_type.is_none() {
            self.error_with(
                Code::InvalidSyntax,
                span,
                path,
                format!("unknown type `{word}`; use one of {}", names.join(", ")),
                closest(word, names).map(str::to_owned),
            );
        }
        value_type
    }

    fn names(&mut self, cursor: &mut Cursor<'a>, line: Line, path: &JsonPath) -> Vec<String> {
        let mut names = Vec::new();
        let mut expecting_name = true;
        loop {
            if cursor.eat(',') {
                if expecting_name {
                    break;
                }
                expecting_name = true;
                continue;
            }
            let Some(span) = cursor.word() else {
                break;
            };
            let word = cursor.text(span);
            if word == "and" {
                if expecting_name {
                    self.error(
                        Code::InvalidSyntax,
                        span,
                        path,
                        "expected a step name before `and`",
                    );
                    return names;
                }
                expecting_name = true;
                continue;
            }
            if !expecting_name {
                self.error(
                    Code::InvalidSyntax,
                    span,
                    path,
                    "separate step names with `,` or `and`",
                );
                return names;
            }
            self.map.record(path.index(names.len()), span);
            names.push(word.to_owned());
            expecting_name = false;
        }
        if expecting_name || cursor.rest().is_some() {
            self.error(
                Code::InvalidSyntax,
                line.span(),
                path,
                "list the steps to wait for, such as `after build and test`",
            );
        }
        names
    }

    fn retry(&mut self, cursor: &mut Cursor<'a>, line: Line, path: &JsonPath) -> Option<u32> {
        let hint = "write retries as `retry 3 times`";
        let count = cursor
            .word()
            .and_then(|span| Some((span, cursor.text(span).parse::<u32>().ok()?)));
        let Some((span, count)) = count else {
            self.error(Code::InvalidSyntax, line.span(), path, hint);
            return None;
        };
        if !(cursor.eat_word("times") || cursor.eat_word("time")) {
            self.error(Code::InvalidSyntax, line.span(), path, hint);
            return None;
        }
        self.expect_end(cursor, path);
        self.map.record(path.clone(), span);
        Some(count)
    }

    fn timeout(
        &mut self,
        cursor: &mut Cursor<'a>,
        line: Line,
        path: &JsonPath,
    ) -> Option<Duration> {
        let span = cursor.word().filter(|_| cursor.rest().is_none());
        let Some(span) = span else {
            self.error(
                Code::InvalidSyntax,
                line.span(),
                path,
                "write a timeout as a number and a unit, such as `timeout 30s` or `timeout 10m`",
            );
            return None;
        };
        self.map.record(path.clone(), span);
        match Duration::parse(cursor.text(span)) {
            Ok(duration) => Some(duration),
            Err(message) => {
                self.error(Code::InvalidSyntax, span, path, message);
                None
            }
        }
    }

    fn text_after(
        &mut self,
        cursor: &mut Cursor<'a>,
        line: Line,
        path: &JsonPath,
        hint: &str,
    ) -> Option<String> {
        let Some(span) = cursor.rest() else {
            self.error(Code::InvalidSyntax, line.span(), path, hint);
            return None;
        };
        self.map.record(path.clone(), span);
        let text = cursor.text(span);
        match expr::parse(text) {
            Ok(Expr {
                kind: ExprKind::Text(parts),
                ..
            }) => match parts.as_slice() {
                [] => Some(String::new()),
                [TextPart::Literal(value)] => Some(value.clone()),
                _ => {
                    self.error(
                        Code::InvalidSyntax,
                        span,
                        path,
                        "this text cannot contain `{...}` interpolation; write `\\{` for a literal brace",
                    );
                    None
                }
            },
            Ok(_) => {
                self.error(Code::InvalidSyntax, span, path, hint);
                None
            }
            Err(error) => {
                self.error(
                    Code::InvalidSyntax,
                    error.span.offset(span.start),
                    path,
                    error.message,
                );
                None
            }
        }
    }

    fn number_after(
        &mut self,
        cursor: &mut Cursor<'a>,
        line: Line,
        path: &JsonPath,
        hint: &str,
    ) -> Option<u32> {
        let number = cursor
            .word()
            .and_then(|span| Some((span, cursor.text(span).parse::<u32>().ok()?)));
        let Some((span, number)) = number else {
            self.error(Code::InvalidSyntax, line.span(), path, hint);
            return None;
        };
        self.expect_end(cursor, path);
        self.map.record(path.clone(), span);
        Some(number)
    }

    fn literal(&mut self, span: Span, path: &JsonPath) -> Option<Value> {
        match expr::parse(&self.source[span.start..span.end]) {
            Ok(parsed) => {
                let value = literal_value(&parsed);
                if value.is_none() {
                    self.error(
                        Code::InvalidSyntax,
                        span,
                        path,
                        "defaults must be plain values such as \"main\", 3, true, lists or objects",
                    );
                }
                value
            }
            Err(error) => {
                self.error(
                    Code::InvalidExpression,
                    error.span.offset(span.start),
                    path,
                    error.message,
                );
                None
            }
        }
    }

    fn value(&self, span: Span) -> FieldValue {
        let text = &self.source[span.start..span.end];
        expr::parse(text)
            .ok()
            .and_then(|parsed| literal_value(&parsed))
            .map_or_else(
                || FieldValue::Expression(text.to_owned()),
                FieldValue::Literal,
            )
    }

    fn once(
        &mut self,
        seen: &mut HashSet<&'static str>,
        word: &str,
        line: Line,
        path: &JsonPath,
    ) -> bool {
        let known = STEP_SETTINGS
            .iter()
            .chain(&["concurrency"])
            .find(|setting| **setting == word)
            .copied();
        let Some(setting) = known else {
            return true;
        };
        if seen.insert(setting) {
            return true;
        }
        self.error(
            Code::DuplicateStatement,
            line.span(),
            path,
            format!("`{word}` can only appear once here"),
        );
        false
    }

    fn no_children(&mut self, block: &Block) {
        if let Some(first) = block.children.first() {
            self.error(
                Code::InvalidIndentation,
                first.line.span(),
                &JsonPath::root(),
                "this line is indented under a statement that cannot contain other statements",
            );
        }
    }

    fn expect_end(&mut self, cursor: &mut Cursor<'a>, path: &JsonPath) {
        if let Some(extra) = cursor.rest() {
            self.error(
                Code::InvalidSyntax,
                extra,
                path,
                format!("unexpected `{}` at the end of the line", cursor.text(extra)),
            );
        }
    }

    fn unexpected(
        &mut self,
        span: Span,
        word: &str,
        candidates: &[&str],
        message: &str,
        path: &JsonPath,
    ) {
        self.error_with(
            Code::InvalidSyntax,
            span,
            path,
            message,
            closest(word, candidates.iter().copied()).map(str::to_owned),
        );
    }

    fn first_word(&self, line: Line) -> Option<&'a str> {
        let mut cursor = Cursor::new(self.source, line);
        cursor.word().map(|span| cursor.text(span))
    }

    fn logical_lines(&mut self) -> Vec<Line> {
        let mut physical = Vec::new();
        let mut offset = 0;
        for piece in self.source.split_inclusive('\n') {
            let text = piece.strip_suffix('\n').unwrap_or(piece);
            physical.push((offset, text.strip_suffix('\r').unwrap_or(text)));
            offset += piece.len();
        }
        let mut lines = Vec::new();
        let mut index = 0;
        while let Some(&(start, text)) = physical.get(index) {
            index += 1;
            let content = text.trim_end();
            let indent = content.len() - content.trim_start_matches([' ', '\t']).len();
            if indent == content.len() {
                continue;
            }
            if content[..indent].contains('\t') {
                self.error(
                    Code::InvalidIndentation,
                    Span::new(start, start + indent),
                    &JsonPath::root(),
                    "indent with spaces, not tabs",
                );
            }
            let mut end = start + content.len();
            let mut depth = bracket_delta(&content[indent..]);
            while depth > 0 {
                let Some(&(next_start, next_text)) = physical.get(index) else {
                    break;
                };
                index += 1;
                let next_content = next_text.trim_end();
                if !next_content.is_empty() {
                    end = next_start + next_content.len();
                }
                depth += bracket_delta(next_content);
            }
            lines.push(Line {
                indent,
                start: start + indent,
                end,
            });
        }
        lines
    }

    fn blocks(
        &mut self,
        lines: &[Line],
        position: &mut usize,
        parent: Option<usize>,
        depth: usize,
    ) -> Vec<Block> {
        let mut blocks = Vec::new();
        let mut expected = None;
        while let Some(&line) = lines.get(*position) {
            if parent.is_some_and(|parent| line.indent <= parent) {
                break;
            }
            *position += 1;
            match expected {
                None => {
                    if parent.is_none() && line.indent > 0 {
                        self.error(
                            Code::InvalidIndentation,
                            line.span(),
                            &JsonPath::root(),
                            "top-level statements must not be indented",
                        );
                    }
                    expected = Some(line.indent);
                }
                Some(indent) if indent != line.indent => self.error(
                    Code::InvalidIndentation,
                    line.span(),
                    &JsonPath::root(),
                    "this line is indented differently from the lines above it",
                ),
                Some(_) => {}
            }
            let children = if depth + 1 < MAX_DEPTH {
                self.blocks(lines, position, Some(line.indent), depth + 1)
            } else {
                self.skip_nested(lines, position, line)
            };
            blocks.push(Block { line, children });
        }
        blocks
    }

    fn skip_nested(&mut self, lines: &[Line], position: &mut usize, parent: Line) -> Vec<Block> {
        let mut reported = false;
        while let Some(&line) = lines
            .get(*position)
            .filter(|line| line.indent > parent.indent)
        {
            *position += 1;
            if !std::mem::replace(&mut reported, true) {
                self.error(
                    Code::InvalidIndentation,
                    line.span(),
                    &JsonPath::root(),
                    "statements cannot be nested this deeply",
                );
            }
        }
        Vec::new()
    }

    fn error(&mut self, code: Code, span: Span, path: &JsonPath, message: impl Into<String>) {
        self.error_with(code, span, path, message, None);
    }

    fn error_with(
        &mut self,
        code: Code,
        span: Span,
        path: &JsonPath,
        message: impl Into<String>,
        suggestion: Option<String>,
    ) {
        let diagnostic =
            Diagnostic::new(code, Location::at(path.clone()), message).with_suggestion(suggestion);
        self.diagnostics.push(TextDiagnostic::new(
            diagnostic,
            span,
            self.source,
            &self.lines,
        ));
    }
}

#[derive(Debug, Clone, Copy)]
struct Cursor<'a> {
    source: &'a str,
    position: usize,
    end: usize,
}

impl<'a> Cursor<'a> {
    const fn new(source: &'a str, line: Line) -> Self {
        Self {
            source,
            position: line.start,
            end: line.end,
        }
    }

    fn skip_spaces(&mut self) {
        while self.position < self.end
            && self.source.as_bytes()[self.position].is_ascii_whitespace()
        {
            self.position += 1;
        }
    }

    fn word(&mut self) -> Option<Span> {
        self.skip_spaces();
        let start = self.position;
        for character in self.source[self.position..self.end].chars() {
            if character.is_whitespace() || matches!(character, ':' | ',' | '=') {
                break;
            }
            self.position += character.len_utf8();
        }
        (self.position > start).then(|| Span::new(start, self.position))
    }

    fn eat_word(&mut self, word: &str) -> bool {
        let mut probe = *self;
        let matched = probe.word().is_some_and(|span| probe.text(span) == word);
        if matched {
            *self = probe;
        }
        matched
    }

    fn eat(&mut self, symbol: char) -> bool {
        self.skip_spaces();
        let matched = self.source[self.position..self.end].starts_with(symbol);
        if matched {
            self.position += symbol.len_utf8();
        }
        matched
    }

    fn rest(&mut self) -> Option<Span> {
        self.skip_spaces();
        (self.position < self.end).then(|| {
            let span = Span::new(self.position, self.end);
            self.position = self.end;
            span
        })
    }

    fn text(&self, span: Span) -> &'a str {
        &self.source[span.start..span.end]
    }
}

fn bracket_delta(text: &str) -> isize {
    let mut delta = 0;
    let mut in_text = false;
    let mut escaped = false;
    for character in text.chars() {
        if in_text {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_text = false;
            }
            continue;
        }
        match character {
            '"' => in_text = true,
            '(' | '[' | '{' => delta += 1,
            ')' | ']' | '}' => delta -= 1,
            _ => {}
        }
    }
    delta
}
