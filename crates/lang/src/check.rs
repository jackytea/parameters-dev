use parameters_core::{Catalog, Code, validate};

use crate::parse::parse;
use crate::print::print;
use crate::source::TextDiagnostic;

#[must_use]
pub fn check(source: &str, builtins: &dyn Catalog) -> Vec<TextDiagnostic> {
    let mut parsed = parse(source);
    if parsed.has_errors() {
        return parsed.diagnostics;
    }
    let mut diagnostics = std::mem::take(&mut parsed.diagnostics);
    for diagnostic in validate(&parsed.workflow, builtins) {
        if diagnostic.code == Code::UnknownLayoutEntry {
            continue;
        }
        let span = parsed
            .source_map
            .locate(&diagnostic.location)
            .unwrap_or_default();
        diagnostics.push(TextDiagnostic::new(
            diagnostic,
            span,
            source,
            parsed.line_index(),
        ));
    }
    diagnostics.sort_by_key(|diagnostic| (diagnostic.span.start, diagnostic.span.end));
    diagnostics
}

pub fn format(source: &str) -> Result<String, Vec<TextDiagnostic>> {
    let parsed = parse(source);
    if parsed.has_errors() {
        Err(parsed.diagnostics)
    } else {
        Ok(print(&parsed.workflow))
    }
}
