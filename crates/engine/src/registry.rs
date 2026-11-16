use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use parameters_core::{Catalog, Signature};

use crate::node::Node;
use crate::nodes;

#[derive(Default)]
pub struct Registry {
    nodes: HashMap<String, Arc<dyn Node>>,
    signatures: BTreeMap<String, Signature>,
}

impl Registry {
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn builtins() -> Self {
        let mut registry = Self::empty();
        registry.register("http.request", nodes::HttpRequest);
        registry.register("shell.run", nodes::ShellRun);
        registry.register("process.run", nodes::ProcessRun);
        registry.register("file.read", nodes::FileRead);
        registry.register("file.write", nodes::FileWrite);
        registry.register("file.list", nodes::FileList);
        registry.register("time.wait", nodes::TimeWait);
        registry.register("log.write", nodes::LogWrite);
        registry
    }

    pub fn register(&mut self, node_type: impl Into<String>, node: impl Node) {
        let node_type = node_type.into();
        self.signatures.insert(node_type.clone(), node.signature());
        self.nodes.insert(node_type, Arc::new(node));
    }

    #[must_use]
    pub fn node(&self, node_type: &str) -> Option<&Arc<dyn Node>> {
        self.nodes.get(node_type)
    }

    #[must_use]
    pub fn description(&self, node_type: &str) -> Option<&'static str> {
        self.nodes.get(node_type).map(|node| node.description())
    }

    #[must_use]
    pub fn is_privileged(&self, node_type: &str) -> bool {
        self.nodes
            .get(node_type)
            .is_some_and(|node| node.privileged())
    }
}

impl Catalog for Registry {
    fn signature(&self, node_type: &str) -> Option<&Signature> {
        self.signatures.get(node_type)
    }

    fn node_types(&self) -> Vec<&str> {
        self.signatures.keys().map(String::as_str).collect()
    }
}

#[cfg(test)]
mod tests {
    use parameters_core::name::{is_valid_name, is_valid_node_type};

    use super::*;

    #[test]
    fn built_in_node_types_are_valid_and_ports_are_unique() {
        let registry = Registry::builtins();
        assert_eq!(registry.node_types().len(), 8);
        let privileged: Vec<&str> = registry
            .node_types()
            .into_iter()
            .filter(|name| registry.is_privileged(name))
            .collect();
        assert_eq!(
            privileged,
            [
                "file.list",
                "file.read",
                "file.write",
                "process.run",
                "shell.run"
            ]
        );
        for name in registry.node_types() {
            assert!(is_valid_node_type(name), "{name}");
            assert!(!registry.description(name).unwrap().is_empty(), "{name}");
            let signature = registry.signature(name).unwrap();
            for ports in [&signature.inputs, &signature.outputs] {
                let mut names: Vec<&str> = ports.iter().map(|port| port.name.as_str()).collect();
                for port in &names {
                    assert!(
                        is_valid_name(port),
                        "{name} has a port named `{port}`, which is not a valid name"
                    );
                }
                names.sort_unstable();
                names.dedup();
                assert_eq!(names.len(), ports.len(), "{name} has duplicate ports");
            }
        }
    }
}
