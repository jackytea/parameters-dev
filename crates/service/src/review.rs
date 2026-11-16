use std::collections::BTreeSet;

use parameters_api::PrivilegedStep;
use parameters_core::expr::{self, Expr, ExprKind, TextPart};
use parameters_core::{FieldValue, Graph, ValueType, Workflow};
use parameters_engine::Registry;

const HTTP_REQUEST: &str = "http.request";
const URL_FIELD: &str = "url";

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Findings {
    pub(crate) privileged: Vec<PrivilegedStep>,
    pub(crate) hosts: Vec<String>,
    pub(crate) dynamic_urls: Vec<String>,
    pub(crate) secrets: Vec<String>,
}

pub(crate) fn inspect(workflow: &Workflow, registry: &Registry) -> Findings {
    let mut findings = Findings::default();
    let mut hosts = BTreeSet::new();
    let mut secrets: BTreeSet<String> = workflow
        .inputs
        .iter()
        .filter(|input| input.value_type == ValueType::Secret)
        .map(|input| input.name.clone())
        .collect();
    let graphs = std::iter::once((String::new(), workflow.graph())).chain(
        workflow
            .definitions
            .iter()
            .map(|(name, definition)| (format!("{name}/"), definition.graph())),
    );
    for (prefix, graph) in graphs {
        inspect_graph(
            &prefix,
            graph,
            registry,
            &mut findings,
            &mut hosts,
            &mut secrets,
        );
    }
    findings.hosts = hosts.into_iter().collect();
    findings.secrets = secrets.into_iter().collect();
    findings
}

fn inspect_graph(
    prefix: &str,
    graph: Graph<'_>,
    registry: &Registry,
    findings: &mut Findings,
    hosts: &mut BTreeSet<String>,
    secrets: &mut BTreeSet<String>,
) {
    for step in graph.steps {
        let path = format!("{prefix}{}", step.id);
        if registry.is_privileged(&step.uses) {
            findings.privileged.push(PrivilegedStep {
                path: path.clone(),
                uses: step.uses.clone(),
            });
        }
        if step.uses == HTTP_REQUEST
            && let Some(url) = step.with.get(URL_FIELD)
        {
            match url_host(url) {
                Some(host) => {
                    hosts.insert(host);
                }
                None => findings.dynamic_urls.push(path),
            }
        }
        let sources = step
            .with
            .values()
            .filter_map(|value| match value {
                FieldValue::Expression(source) => Some(source.as_str()),
                FieldValue::Literal(_) => None,
            })
            .chain(step.when.as_deref());
        for source in sources {
            collect_secrets(source, secrets);
        }
    }
    for output in graph.outputs {
        if let FieldValue::Expression(source) = &output.value {
            collect_secrets(source, secrets);
        }
    }
}

fn collect_secrets(source: &str, secrets: &mut BTreeSet<String>) {
    if let Ok(parsed) = expr::parse(source) {
        visit_secrets(&parsed, secrets);
    }
}

fn visit_secrets(expression: &Expr, secrets: &mut BTreeSet<String>) {
    match &expression.kind {
        ExprKind::Secret(name) => {
            secrets.insert(name.clone());
        }
        ExprKind::List(items) => {
            for item in items {
                visit_secrets(item, secrets);
            }
        }
        ExprKind::Object(entries) => {
            for (_, value) in entries {
                visit_secrets(value, secrets);
            }
        }
        ExprKind::Not(inner) => visit_secrets(inner, secrets),
        ExprKind::Binary { left, right, .. } => {
            visit_secrets(left, secrets);
            visit_secrets(right, secrets);
        }
        ExprKind::Boolean(_)
        | ExprKind::Number(_)
        | ExprKind::Text(_)
        | ExprKind::Reference(_)
        | ExprKind::State { .. } => {}
    }
}

fn url_host(url: &FieldValue) -> Option<String> {
    match url {
        FieldValue::Literal(serde_json::Value::String(text)) => literal_host(text, true),
        FieldValue::Literal(_) => None,
        FieldValue::Expression(source) => match expr::parse(source).ok()?.kind {
            ExprKind::Text(parts) => match parts.as_slice() {
                [TextPart::Literal(prefix), rest @ ..] => literal_host(prefix, rest.is_empty()),
                _ => None,
            },
            _ => None,
        },
    }
}

fn literal_host(text: &str, complete: bool) -> Option<String> {
    let (_, rest) = text.split_once("://")?;
    let end = rest.find(['/', '?', '#']);
    if end.is_none() && !complete {
        return None;
    }
    let authority = &rest[..end.unwrap_or(rest.len())];
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn findings(source: &str) -> Findings {
        let parsed = parameters_lang::parse(source);
        assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
        inspect(&parsed.workflow, &Registry::builtins())
    }

    #[test]
    fn lists_privileged_steps_hosts_and_secrets() {
        let found = findings(
            "workflow \"Review\"\n\ninput token: secret\n\nstep build uses shell.run\n  command is \"make\"\n\nstep ping uses http.request\n  url is \"https://User@API.example.com:8443/v1?x=1\"\n  headers is { authorization: secret \"API_KEY\" }\n\nstep fetch uses http.request\n  url is \"https://cdn.example.org/{build.stdout}\"\n\nstep dynamic uses http.request\n  url is \"{build.stdout}\"\n\nstep hidden uses http.request\n  url is secret \"HOOK\"\n\ndefine node acme.copy\n  step copy uses file.write\n    path is \"out.txt\"\n    content is \"x\"\n",
        );
        assert_eq!(
            found.privileged,
            [
                PrivilegedStep {
                    path: "build".into(),
                    uses: "shell.run".into()
                },
                PrivilegedStep {
                    path: "acme.copy/copy".into(),
                    uses: "file.write".into()
                },
            ]
        );
        assert_eq!(found.hosts, ["api.example.com:8443", "cdn.example.org"]);
        assert_eq!(found.dynamic_urls, ["dynamic", "hidden"]);
        assert_eq!(found.secrets, ["API_KEY", "HOOK", "token"]);
    }

    #[test]
    fn plain_workflows_need_nothing() {
        let found = findings("workflow \"Calm\"\n\nstep say uses log.write\n  message is \"hi\"\n");
        assert_eq!(found, Findings::default());
    }
}
