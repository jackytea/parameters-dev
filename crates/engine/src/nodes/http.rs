use parameters_core::{Port, Signature, ValueType};
use reqwest::header::{CONTENT_TYPE, HeaderName, HeaderValue};
use serde_json::{Number, Value};

use super::MAX_CAPTURED_BYTES;
use crate::node::{BoxFuture, Inputs, Node, NodeContext, NodeError, Values, scalar_text};

const ERROR_BODY_PREVIEW: usize = 300;

pub(crate) struct HttpRequest;

impl Node for HttpRequest {
    fn signature(&self) -> Signature {
        use ValueType::{Any, Number, Object, Path, Text};
        Signature {
            inputs: vec![
                Port::required("url", Text),
                Port::optional("method", Text),
                Port::optional("headers", Object),
                Port::optional("query", Object),
                Port::optional("body", Any),
                Port::optional("file", Path),
            ],
            outputs: vec![
                Port::optional("status", Number),
                Port::optional("headers", Object),
                Port::optional("body", Any),
            ],
        }
    }

    fn run(&self, context: NodeContext, inputs: Inputs) -> BoxFuture<Result<Values, NodeError>> {
        Box::pin(request(context, inputs))
    }
}

async fn request(context: NodeContext, inputs: Inputs) -> Result<Values, NodeError> {
    let url = inputs.text("url")?;
    let method_name = inputs
        .optional_text("method")?
        .unwrap_or_else(|| "GET".to_owned())
        .to_ascii_uppercase();
    let method = reqwest::Method::from_bytes(method_name.as_bytes())
        .map_err(|_| NodeError::new(format!("`{method_name}` is not an HTTP method")))?;
    let mut request = context.http().request(method, &url);
    for (name, value) in inputs.optional_object("headers")?.into_iter().flatten() {
        let text = scalar_text(value)
            .ok_or_else(|| NodeError::new(format!("the header `{name}` must be text")))?;
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| NodeError::new(format!("`{name}` is not a valid header name")))?;
        let value = HeaderValue::from_str(&text)
            .map_err(|_| NodeError::new(format!("the header `{name}` has an invalid value")))?;
        request = request.header(name, value);
    }
    if let Some(query) = inputs.optional_object("query")? {
        let pairs = query
            .iter()
            .map(|(name, value)| {
                scalar_text(value)
                    .map(|text| (name.clone(), text))
                    .ok_or_else(|| NodeError::new(format!("the query value `{name}` must be text")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        request = request.query(&pairs);
    }
    request = match (inputs.optional("body"), inputs.optional_text("file")?) {
        (Some(_), Some(_)) => {
            return Err(NodeError::new("set either `body` or `file`, not both"));
        }
        (Some(Value::String(text)), None) => request.body(text.clone()),
        (Some(value), None) => request
            .header(CONTENT_TYPE, "application/json")
            .body(value.to_string()),
        (None, Some(file)) => {
            let bytes = tokio::fs::read(context.resolve(&file))
                .await
                .map_err(|error| NodeError::new(format!("cannot read {file}: {error}")))?;
            request.body(bytes)
        }
        (None, None) => request,
    };
    let mut response = request
        .send()
        .await
        .map_err(|error| NodeError::new(format!("{method_name} {url} failed: {error}")))?;
    let status = response.status();
    let is_json = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("json"));
    let headers: Values = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.to_string(),
                Value::String(String::from_utf8_lossy(value.as_bytes()).into_owned()),
            )
        })
        .collect();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        NodeError::new(format!("reading the response from {url} failed: {error}"))
    })? {
        if bytes.len() + chunk.len() > MAX_CAPTURED_BYTES {
            return Err(NodeError::new(format!(
                "the response from {url} is larger than {MAX_CAPTURED_BYTES} bytes"
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    if status.is_client_error() || status.is_server_error() {
        let preview: String = text.chars().take(ERROR_BODY_PREVIEW).collect();
        return Err(NodeError::new(format!(
            "{method_name} {url} returned {status}: {preview}"
        )));
    }
    let body = if is_json {
        serde_json::from_slice(&bytes).unwrap_or(Value::String(text))
    } else {
        Value::String(text)
    };
    Ok(Values::from_iter([
        (
            "status".to_owned(),
            Value::Number(Number::from(status.as_u16())),
        ),
        ("headers".to_owned(), Value::Object(headers)),
        ("body".to_owned(), body),
    ]))
}
