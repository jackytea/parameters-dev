use std::collections::{BTreeMap, HashSet};

use parameters_api::{NodeInfo, NodeKind, PortInfo};
use parameters_core::{Catalog, Port, Signature};

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct NodeCatalog {
    nodes: Vec<NodeInfo>,
    signatures: BTreeMap<String, Signature>,
}

impl NodeCatalog {
    pub(crate) fn new(nodes: Vec<NodeInfo>) -> Self {
        let signatures = nodes
            .iter()
            .map(|node| {
                (
                    node.type_name.clone(),
                    Signature {
                        inputs: ports(&node.inputs),
                        outputs: ports(&node.outputs),
                    },
                )
            })
            .collect();
        Self { nodes, signatures }
    }

    pub(crate) fn nodes(&self) -> &[NodeInfo] {
        &self.nodes
    }

    pub(crate) fn info(&self, node_type: &str) -> Option<&NodeInfo> {
        self.nodes.iter().find(|node| node.type_name == node_type)
    }

    pub(crate) fn without_custom<'a>(&self, defined: impl IntoIterator<Item = &'a str>) -> Self {
        let defined: HashSet<&str> = defined.into_iter().collect();
        let keep = |node: &NodeInfo| {
            node.kind == NodeKind::Builtin || !defined.contains(node.type_name.as_str())
        };
        Self::new(
            self.nodes
                .iter()
                .filter(|node| keep(node))
                .cloned()
                .collect(),
        )
    }
}

fn ports(ports: &[PortInfo]) -> Vec<Port> {
    ports
        .iter()
        .map(|port| Port {
            name: port.name.clone(),
            value_type: port.value_type,
            required: port.required,
        })
        .collect()
}

impl Catalog for NodeCatalog {
    fn signature(&self, node_type: &str) -> Option<&Signature> {
        self.signatures.get(node_type)
    }

    fn node_types(&self) -> Vec<&str> {
        self.signatures.keys().map(String::as_str).collect()
    }
}

#[cfg(test)]
mod tests {
    use parameters_core::ValueType;

    use super::*;

    fn node(type_name: &str, kind: NodeKind) -> NodeInfo {
        NodeInfo {
            type_name: type_name.into(),
            kind,
            description: None,
            privileged: false,
            inputs: vec![PortInfo {
                name: "message".into(),
                value_type: ValueType::Text,
                required: true,
            }],
            outputs: Vec::new(),
            example: String::new(),
        }
    }

    #[test]
    fn checks_text_against_the_listed_nodes() {
        let catalog = NodeCatalog::new(vec![
            node("log.write", NodeKind::Builtin),
            node("acme.say", NodeKind::Custom),
        ]);
        let source = "workflow \"A\"\n\nstep a uses acme.say\n  message is \"hi\"\n";
        assert_eq!(parameters_lang::check(source, &catalog), []);
        let hidden = catalog.without_custom(["acme.say", "log.write"]);
        assert!(hidden.contains("log.write"));
        assert!(!hidden.contains("acme.say"));
    }
}
