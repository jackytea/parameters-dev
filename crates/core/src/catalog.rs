use std::collections::BTreeMap;

use crate::document::{Definition, ValueType};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Port {
    pub name: String,
    pub value_type: ValueType,
    pub required: bool,
}

impl Port {
    #[must_use]
    pub fn required(name: impl Into<String>, value_type: ValueType) -> Self {
        Self {
            name: name.into(),
            value_type,
            required: true,
        }
    }

    #[must_use]
    pub fn optional(name: impl Into<String>, value_type: ValueType) -> Self {
        Self {
            name: name.into(),
            value_type,
            required: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Signature {
    pub inputs: Vec<Port>,
    pub outputs: Vec<Port>,
}

impl Signature {
    #[must_use]
    pub fn of_definition(definition: &Definition) -> Self {
        Self {
            inputs: definition
                .inputs
                .iter()
                .map(|input| Port {
                    name: input.name.clone(),
                    value_type: input.value_type,
                    required: input.default.is_none(),
                })
                .collect(),
            outputs: definition
                .outputs
                .iter()
                .map(|output| {
                    Port::optional(&output.name, output.value_type.unwrap_or(ValueType::Any))
                })
                .collect(),
        }
    }

    #[must_use]
    pub fn input(&self, name: &str) -> Option<&Port> {
        self.inputs.iter().find(|port| port.name == name)
    }

    #[must_use]
    pub fn output(&self, name: &str) -> Option<&Port> {
        self.outputs.iter().find(|port| port.name == name)
    }
}

pub trait Catalog {
    fn signature(&self, node_type: &str) -> Option<&Signature>;

    fn node_types(&self) -> Vec<&str>;

    fn contains(&self, node_type: &str) -> bool {
        self.signature(node_type).is_some()
    }
}

impl Catalog for BTreeMap<String, Signature> {
    fn signature(&self, node_type: &str) -> Option<&Signature> {
        self.get(node_type)
    }

    fn node_types(&self) -> Vec<&str> {
        self.keys().map(String::as_str).collect()
    }
}
