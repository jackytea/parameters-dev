use parameters_core::workflow_schema;
use rmcp::ErrorData;
use rmcp::model::{ReadResourceResult, Resource, ResourceContents, ResourceTemplate};
use serde_json::json;

use crate::McpServer;

pub const LANGUAGE_REFERENCE: &str = "parameters://reference/language";
pub const WORKFLOW_SCHEMA: &str = "parameters://schema/workflow";
pub const NODE_TEMPLATE: &str = "parameters://nodes/{type}";
const NODE_PREFIX: &str = "parameters://nodes/";
const MARKDOWN: &str = "text/markdown";
const JSON: &str = "application/json";

pub(crate) fn listed() -> Vec<Resource> {
    vec![
        Resource::new(LANGUAGE_REFERENCE, "language-reference")
            .with_title("Parameters language reference")
            .with_description(
                "The syntax of the Parameters language, with a complete example, the types, expressions, reserved keywords and diagnostic codes",
            )
            .with_mime_type(MARKDOWN),
        Resource::new(WORKFLOW_SCHEMA, "workflow-schema")
            .with_title("Workflow JSON Schema")
            .with_description("The JSON Schema of the workflow document format")
            .with_mime_type(JSON),
    ]
}

pub(crate) fn templates() -> Vec<ResourceTemplate> {
    vec![
        ResourceTemplate::new(NODE_TEMPLATE, "node")
            .with_title("Node")
            .with_description(
                "A node's description, inputs, outputs and an example, for built-in and custom nodes",
            )
            .with_mime_type(JSON),
    ]
}

impl McpServer {
    pub async fn read(&self, uri: &str) -> Result<ReadResourceResult, ErrorData> {
        let (text, mime_type) = match uri {
            LANGUAGE_REFERENCE => (parameters_lang::reference(), MARKDOWN),
            WORKFLOW_SCHEMA => (pretty(&workflow_schema())?, JSON),
            _ => match uri.strip_prefix(NODE_PREFIX) {
                Some(type_name) => {
                    let node = self
                        .service
                        .describe_node(&self.actor, type_name)
                        .await
                        .map_err(|error| {
                            ErrorData::resource_not_found(
                                error.to_string(),
                                Some(json!({"uri": uri})),
                            )
                        })?;
                    (pretty(&node)?, JSON)
                }
                None => {
                    return Err(ErrorData::resource_not_found(
                        format!("there is no resource at {uri}"),
                        Some(json!({"uri": uri})),
                    ));
                }
            },
        };
        Ok(ReadResourceResult::new(vec![
            ResourceContents::TextResourceContents {
                uri: uri.to_owned(),
                mime_type: Some(mime_type.to_owned()),
                text,
                meta: None,
            },
        ]))
    }
}

fn pretty(value: &impl serde::Serialize) -> Result<String, ErrorData> {
    serde_json::to_string_pretty(value)
        .map_err(|error| ErrorData::internal_error(error.to_string(), None))
}
