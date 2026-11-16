use std::collections::BTreeMap;

use crate::catalog::Catalog;
use crate::document::{Step, Workflow};
use crate::name::is_valid_node_type;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RenameError {
    #[error("there is no custom node named `{0}`")]
    UnknownDefinition(String),
    #[error("`{0}` is not a valid node type: use a qualified name such as `acme.deploy`")]
    InvalidNodeType(String),
    #[error("the node type `{0}` is already taken")]
    NameTaken(String),
}

pub fn rename_node_type(
    workflow: &mut Workflow,
    old: &str,
    new: &str,
    builtins: &dyn Catalog,
) -> Result<(), RenameError> {
    if !workflow.definitions.contains_key(old) {
        return Err(RenameError::UnknownDefinition(old.to_owned()));
    }
    if old == new {
        return Ok(());
    }
    if !is_valid_node_type(new) {
        return Err(RenameError::InvalidNodeType(new.to_owned()));
    }
    if workflow.definitions.contains_key(new) || builtins.contains(new) {
        return Err(RenameError::NameTaken(new.to_owned()));
    }
    workflow.definitions.rename_key(old, new);
    let renames = BTreeMap::from([(old.to_owned(), new.to_owned())]);
    for graph in workflow.graphs_mut() {
        rename_uses(graph.steps.iter_mut(), &renames);
    }
    Ok(())
}

pub fn rename_uses<'a>(
    steps: impl IntoIterator<Item = &'a mut Step>,
    renames: &BTreeMap<String, String>,
) {
    for step in steps {
        if let Some(new) = renames.get(&step.uses) {
            step.uses.clone_from(new);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Signature;
    use crate::document::Definition;

    fn workflow() -> Workflow {
        let mut workflow = Workflow::new("Deploy");
        workflow.steps.push(Step::new("deploy", "netlify.deploy"));
        workflow.steps.push(Step::new("notify", "http.request"));
        let mut wrapper = Definition::default();
        wrapper.steps.push(Step::new("inner", "netlify.deploy"));
        workflow
            .definitions
            .insert("netlify.deploy", Definition::default());
        workflow.definitions.insert("acme.release", wrapper);
        workflow
    }

    fn builtins() -> BTreeMap<String, Signature> {
        BTreeMap::from([("http.request".to_owned(), Signature::default())])
    }

    #[test]
    fn rewrites_every_use_and_keeps_definition_order() {
        let mut workflow = workflow();
        rename_node_type(
            &mut workflow,
            "netlify.deploy",
            "netlify.publish",
            &builtins(),
        )
        .unwrap();
        assert_eq!(workflow.steps[0].uses, "netlify.publish");
        assert_eq!(workflow.steps[1].uses, "http.request");
        assert_eq!(
            workflow.definitions.keys().collect::<Vec<_>>(),
            ["netlify.publish", "acme.release"]
        );
        assert_eq!(
            workflow.definitions.get("acme.release").unwrap().steps[0].uses,
            "netlify.publish"
        );
    }

    #[test]
    fn refuses_invalid_or_taken_names() {
        let mut workflow = workflow();
        let builtins = builtins();
        assert_eq!(
            rename_node_type(&mut workflow, "missing.node", "a.b", &builtins),
            Err(RenameError::UnknownDefinition("missing.node".into()))
        );
        assert_eq!(
            rename_node_type(&mut workflow, "netlify.deploy", "deploy", &builtins),
            Err(RenameError::InvalidNodeType("deploy".into()))
        );
        assert_eq!(
            rename_node_type(&mut workflow, "netlify.deploy", "http.request", &builtins),
            Err(RenameError::NameTaken("http.request".into()))
        );
        assert_eq!(
            rename_node_type(&mut workflow, "netlify.deploy", "acme.release", &builtins),
            Err(RenameError::NameTaken("acme.release".into()))
        );
        assert_eq!(
            rename_node_type(&mut workflow, "netlify.deploy", "netlify.deploy", &builtins),
            Ok(())
        );
    }
}
