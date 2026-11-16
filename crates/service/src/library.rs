use std::collections::{BTreeMap, HashSet, VecDeque};

use parameters_api::{ApiError, NodeInfo, NodeKind, PortInfo};
use parameters_core::{
    Catalog, Definition, FieldValue, MergeOutcome, OrderedMap, Port, Signature, Step, ValueType,
    Workflow, merge_definitions,
};
use parameters_engine::Registry;
use serde_json::json;

pub(crate) struct WithLibrary<'a> {
    builtins: &'a Registry,
    library: BTreeMap<&'a str, Signature>,
}

impl<'a> WithLibrary<'a> {
    pub(crate) fn new(
        builtins: &'a Registry,
        library: &'a OrderedMap<Definition>,
        declared: &HashSet<String>,
    ) -> Self {
        Self {
            builtins,
            library: library
                .iter()
                .filter(|(name, _)| !declared.contains(*name))
                .map(|(name, definition)| (name, Signature::of_definition(definition)))
                .collect(),
        }
    }
}

impl Catalog for WithLibrary<'_> {
    fn signature(&self, node_type: &str) -> Option<&Signature> {
        self.builtins
            .signature(node_type)
            .or_else(|| self.library.get(node_type))
    }

    fn node_types(&self) -> Vec<&str> {
        let mut types = self.builtins.node_types();
        types.extend(self.library.keys().copied());
        types
    }
}

pub(crate) fn include_library(
    workflow: &mut Workflow,
    library: &OrderedMap<Definition>,
    builtins: &Registry,
) -> Result<MergeOutcome, ApiError> {
    let from_library =
        |node_type: &str| !builtins.contains(node_type) && library.contains_key(node_type);
    let mut queue: VecDeque<String> = workflow
        .steps
        .iter()
        .chain(
            workflow
                .definitions
                .values()
                .flat_map(|definition| &definition.steps),
        )
        .filter(|step| from_library(&step.uses) && !workflow.definitions.contains_key(&step.uses))
        .map(|step| step.uses.clone())
        .collect();
    let mut needed = OrderedMap::new();
    while let Some(name) = queue.pop_front() {
        if needed.contains_key(&name) {
            continue;
        }
        let Some(definition) = library.get(&name) else {
            continue;
        };
        queue.extend(
            definition
                .steps
                .iter()
                .filter(|step| from_library(&step.uses))
                .map(|step| step.uses.clone()),
        );
        needed.insert(name, definition.clone());
    }
    if needed.is_empty() {
        return Ok(MergeOutcome::default());
    }
    merge_definitions(
        &mut workflow.definitions,
        &needed,
        builtins,
        &BTreeMap::new(),
    )
    .map_err(|error| ApiError::bad_request(error.to_string()))
}

pub(crate) fn node_info(
    type_name: &str,
    kind: NodeKind,
    description: Option<String>,
    privileged: bool,
    signature: &Signature,
) -> NodeInfo {
    NodeInfo {
        type_name: type_name.to_owned(),
        kind,
        description,
        privileged,
        inputs: ports(&signature.inputs),
        outputs: ports(&signature.outputs),
        example: example(type_name, signature),
    }
}

fn ports(ports: &[Port]) -> Vec<PortInfo> {
    ports
        .iter()
        .map(|port| PortInfo {
            name: port.name.clone(),
            value_type: port.value_type,
            required: port.required,
        })
        .collect()
}

fn example(type_name: &str, signature: &Signature) -> String {
    let mut step = Step::new("example", type_name);
    for port in signature.inputs.iter().filter(|port| port.required) {
        let value = match port.value_type {
            ValueType::Number => FieldValue::Literal(json!(1)),
            ValueType::Boolean => FieldValue::Literal(json!(true)),
            ValueType::List => FieldValue::Literal(json!([])),
            ValueType::Object => FieldValue::Literal(json!({})),
            ValueType::Path => FieldValue::Literal(json!("notes.txt")),
            ValueType::Secret => FieldValue::expression("secret \"API_TOKEN\""),
            ValueType::Text | ValueType::Any => FieldValue::Literal(json!("…")),
        };
        step.with.insert(port.name.clone(), value);
    }
    let mut workflow = Workflow::new("Example");
    workflow.steps.push(step);
    parameters_lang::print(&workflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library() -> OrderedMap<Definition> {
        parameters_lang::parse(
            "workflow \"Library\"\n\ndefine node acme.base\n  step a uses log.write\n    message is \"library base\"\n\ndefine node acme.top\n  step b uses acme.base\n",
        )
        .workflow
        .definitions
    }

    #[test]
    fn copies_used_library_nodes_and_renames_conflicting_dependencies() {
        let registry = Registry::builtins();
        let library = library();
        let mut workflow = parameters_lang::parse(
            "workflow \"Uses\"\n\nstep top uses acme.top\n\ndefine node acme.base\n  step own uses time.wait\n    duration is \"1s\"\n",
        )
        .workflow;
        let outcome = include_library(&mut workflow, &library, &registry).unwrap();
        assert_eq!(
            outcome.renames,
            BTreeMap::from([("acme.base".to_owned(), "acme.base_2".to_owned())])
        );
        let names: Vec<&str> = workflow.definitions.keys().collect();
        assert_eq!(names, ["acme.base", "acme.base_2", "acme.top"]);
        assert_eq!(
            workflow.definitions.get("acme.top").unwrap().steps[0].uses,
            "acme.base_2"
        );
        assert_eq!(
            workflow.definitions.get("acme.base").unwrap().steps[0].uses,
            "time.wait"
        );
        assert!(
            parameters_core::validate(&workflow, &registry)
                .iter()
                .all(|diagnostic| !diagnostic.is_error())
        );
    }

    #[test]
    fn the_catalog_hides_library_nodes_the_document_defines_itself() {
        let registry = Registry::builtins();
        let library = library();
        let declared = HashSet::from(["acme.base".to_owned()]);
        let catalog = WithLibrary::new(&registry, &library, &declared);
        assert!(catalog.contains("acme.top"));
        assert!(catalog.contains("log.write"));
        assert!(!catalog.contains("acme.base"));
    }

    #[test]
    fn examples_are_valid_workflows() {
        let registry = Registry::builtins();
        for name in registry.node_types() {
            let info = node_info(
                name,
                NodeKind::Builtin,
                None,
                false,
                registry.signature(name).unwrap(),
            );
            let problems = parameters_lang::check(&info.example, &registry);
            assert!(
                problems.is_empty(),
                "{name}: {problems:?}\n{}",
                info.example
            );
        }
    }
}
