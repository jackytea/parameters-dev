use std::collections::HashSet;

use parameters_api::{ApiError, Format, Problem, Severity, Source, TextPosition, TextRange};
use parameters_core::layout::complete_layouts;
use parameters_core::{Catalog, Code, LoadError, Workflow, from_json, to_json, validate};

pub(crate) fn check(source: &Source, catalog: &dyn Catalog) -> (Option<Workflow>, Vec<Problem>) {
    match source {
        Source::Text(text) => {
            let problems: Vec<Problem> = parameters_lang::check(text, catalog)
                .iter()
                .map(Problem::from)
                .collect();
            if problems.iter().any(Problem::is_error) {
                return (None, problems);
            }
            (Some(parameters_lang::parse(text).workflow), problems)
        }
        Source::Json(text) => match from_json(text) {
            Ok(workflow) => {
                let problems: Vec<Problem> = validate(&workflow, catalog)
                    .iter()
                    .map(Problem::from)
                    .collect();
                let valid = !problems.iter().any(Problem::is_error);
                (valid.then_some(workflow), problems)
            }
            Err(error) => (None, vec![load_problem(&error)]),
        },
    }
}

pub(crate) fn load(source: &Source, catalog: &dyn Catalog) -> Result<Workflow, ApiError> {
    match check(source, catalog) {
        (Some(workflow), _) => Ok(workflow),
        (None, problems) => Err(invalid(problems)),
    }
}

pub(crate) fn declared(source: &Source) -> HashSet<String> {
    let workflow = match source {
        Source::Text(text) => Some(parameters_lang::parse(text).workflow),
        Source::Json(text) => from_json(text).ok(),
    };
    workflow.map_or_else(HashSet::new, |workflow| {
        workflow.definitions.keys().map(str::to_owned).collect()
    })
}

pub(crate) fn convert(source: &Source, to: Format) -> Result<String, ApiError> {
    let mut workflow = match source {
        Source::Text(text) => {
            let parsed = parameters_lang::parse(text);
            if parsed.has_errors() {
                return Err(invalid(
                    parsed.diagnostics.iter().map(Problem::from).collect(),
                ));
            }
            parsed.workflow
        }
        Source::Json(text) => {
            from_json(text).map_err(|error| invalid(vec![load_problem(&error)]))?
        }
    };
    Ok(match to {
        Format::Text => parameters_lang::print(&workflow),
        Format::Json => {
            complete_layouts(&mut workflow);
            to_json(&workflow)
        }
    })
}

pub(crate) fn invalid(problems: Vec<Problem>) -> ApiError {
    let errors = problems.iter().filter(|problem| problem.is_error()).count();
    ApiError::Invalid {
        message: format!(
            "the workflow has {errors} {}",
            if errors == 1 { "problem" } else { "problems" }
        ),
        problems,
    }
}

fn load_problem(error: &LoadError) -> Problem {
    let range = match error {
        LoadError::Malformed { line, column, .. } => {
            let position = TextPosition {
                line: *line,
                column: *column,
            };
            Some(TextRange {
                start: position,
                end: position,
            })
        }
        _ => None,
    };
    let message = match error {
        LoadError::Malformed { message, .. } => message.clone(),
        other => other.to_string(),
    };
    Problem {
        code: Code::InvalidSyntax.id().to_owned(),
        severity: Severity::Error,
        message,
        path: String::new(),
        suggestion: None,
        range,
    }
}

#[cfg(test)]
mod tests {
    use parameters_engine::Registry;

    use super::*;

    #[test]
    fn text_and_json_report_problems_with_positions() {
        let registry = Registry::builtins();
        let (workflow, problems) = check(
            &Source::Text("workflow \"A\"\n\nstep a uses log.write\n  message is b.c\n".into()),
            &registry,
        );
        assert!(workflow.is_none());
        assert_eq!(problems[0].code, "P0302");
        assert_eq!(problems[0].range.unwrap().start.line, 4);
        let (workflow, problems) = check(&Source::Json("{\"parameters\": 1,".into()), &registry);
        assert!(workflow.is_none());
        assert_eq!(problems[0].code, "P0001");
        assert!(problems[0].range.is_some());
        let error = load(&Source::Json("[]".into()), &registry).unwrap_err();
        assert!(matches!(error, ApiError::Invalid { .. }), "{error:?}");
    }

    #[test]
    fn converts_both_ways_without_validating_node_types() {
        let text = "workflow \"Custom\"\n\nstep a uses acme.unknown\n";
        let json = convert(&Source::Text(text.into()), Format::Json).unwrap();
        assert!(json.contains("\"layout\""), "{json}");
        assert_eq!(convert(&Source::Json(json), Format::Text).unwrap(), text);
        assert!(matches!(
            convert(&Source::Text("step".into()), Format::Json),
            Err(ApiError::Invalid { .. })
        ));
    }

    #[test]
    fn valid_sources_load() {
        let registry = Registry::builtins();
        let workflow = load(
            &Source::Text(parameters_lang::EXAMPLE.to_owned()),
            &registry,
        )
        .unwrap();
        let json = parameters_core::to_json(&workflow);
        assert_eq!(load(&Source::Json(json), &registry).unwrap(), workflow);
    }
}
