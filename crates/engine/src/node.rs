use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use parameters_core::Signature;
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::event::Event;
use crate::run::Shared;

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
pub type Values = Map<String, Value>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct NodeError {
    pub message: String,
}

impl NodeError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

pub trait Node: Send + Sync + 'static {
    fn signature(&self) -> Signature;

    fn description(&self) -> &'static str;

    fn privileged(&self) -> bool {
        false
    }

    fn run(&self, context: NodeContext, inputs: Inputs) -> BoxFuture<Result<Values, NodeError>>;
}

#[derive(Clone)]
pub struct NodeContext {
    pub(crate) shared: Arc<Shared>,
    pub(crate) path: Arc<str>,
}

impl NodeContext {
    #[must_use]
    pub fn sandbox(&self) -> crate::sandbox::Sandbox {
        self.shared.sandbox
    }

    #[must_use]
    pub fn http(&self) -> &reqwest::Client {
        &self.shared.http
    }

    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.shared.directory
    }

    #[must_use]
    pub fn resolve(&self, path: &str) -> PathBuf {
        self.shared.directory.join(path)
    }

    #[must_use]
    pub fn cancellation(&self) -> &CancellationToken {
        &self.shared.cancel
    }

    pub async fn log(&self, message: &str) {
        let message = self.shared.redactor.text(message);
        self.shared
            .emit(Event::Log {
                path: self.path.to_string(),
                message,
            })
            .await;
    }
}

#[derive(Debug, Clone, Default)]
pub struct Inputs(pub(crate) Values);

impl Inputs {
    #[must_use]
    pub fn optional(&self, name: &str) -> Option<&Value> {
        self.0.get(name)
    }

    pub fn text(&self, name: &str) -> Result<String, NodeError> {
        self.optional_text(name)?
            .ok_or_else(|| NodeError::new(format!("the field `{name}` is required")))
    }

    pub fn optional_text(&self, name: &str) -> Result<Option<String>, NodeError> {
        self.optional(name)
            .map(|value| scalar_text(value).ok_or_else(|| expected(name, "text", value)))
            .transpose()
    }

    pub fn optional_object(&self, name: &str) -> Result<Option<&Map<String, Value>>, NodeError> {
        self.optional(name)
            .map(|value| {
                value
                    .as_object()
                    .ok_or_else(|| expected(name, "an object", value))
            })
            .transpose()
    }

    pub fn optional_list(&self, name: &str) -> Result<Option<&Vec<Value>>, NodeError> {
        self.optional(name)
            .map(|value| {
                value
                    .as_array()
                    .ok_or_else(|| expected(name, "a list", value))
            })
            .transpose()
    }
}

#[must_use]
pub fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(boolean) => Some(boolean.to_string()),
        _ => None,
    }
}

fn expected(name: &str, kind: &str, value: &Value) -> NodeError {
    let found = match value {
        Value::Null => "nothing",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "text",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    };
    NodeError::new(format!(
        "the field `{name}` must be {kind}, but it is {found}"
    ))
}
