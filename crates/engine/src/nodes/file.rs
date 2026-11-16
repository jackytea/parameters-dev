use parameters_core::{Port, Signature, ValueType};
use serde_json::Value;

use super::MAX_CAPTURED_BYTES;
use crate::node::{BoxFuture, Inputs, Node, NodeContext, NodeError, Values};

pub(crate) struct FileRead;

impl Node for FileRead {
    fn signature(&self) -> Signature {
        Signature {
            inputs: vec![Port::required("path", ValueType::Path)],
            outputs: vec![Port::optional("content", ValueType::Text)],
        }
    }

    fn run(&self, context: NodeContext, inputs: Inputs) -> BoxFuture<Result<Values, NodeError>> {
        Box::pin(async move {
            let written = inputs.text("path")?;
            let path = context.resolve(&written);
            let size = tokio::fs::metadata(&path)
                .await
                .map_err(|error| NodeError::new(format!("cannot read {written}: {error}")))?
                .len();
            if usize::try_from(size).map_or(true, |size| size > MAX_CAPTURED_BYTES) {
                return Err(NodeError::new(format!(
                    "{written} is larger than {MAX_CAPTURED_BYTES} bytes"
                )));
            }
            let bytes = tokio::fs::read(&path)
                .await
                .map_err(|error| NodeError::new(format!("cannot read {written}: {error}")))?;
            let text = String::from_utf8(bytes)
                .map_err(|_| NodeError::new(format!("{written} is not UTF-8 text")))?;
            Ok(Values::from_iter([(
                "content".to_owned(),
                Value::String(text),
            )]))
        })
    }
}

pub(crate) struct FileWrite;

impl Node for FileWrite {
    fn signature(&self) -> Signature {
        Signature {
            inputs: vec![
                Port::required("path", ValueType::Path),
                Port::required("content", ValueType::Text),
            ],
            outputs: vec![Port::optional("path", ValueType::Path)],
        }
    }

    fn run(&self, context: NodeContext, inputs: Inputs) -> BoxFuture<Result<Values, NodeError>> {
        Box::pin(async move {
            let written = inputs.text("path")?;
            let text = inputs.text("content")?;
            let path = context.resolve(&written);
            let failed =
                |error: std::io::Error| NodeError::new(format!("cannot write {written}: {error}"));
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(failed)?;
            }
            tokio::fs::write(&path, text).await.map_err(failed)?;
            Ok(Values::from_iter([(
                "path".to_owned(),
                Value::String(written.clone()),
            )]))
        })
    }
}

pub(crate) struct FileList;

impl Node for FileList {
    fn signature(&self) -> Signature {
        Signature {
            inputs: vec![Port::required("directory", ValueType::Path)],
            outputs: vec![Port::optional("paths", ValueType::List)],
        }
    }

    fn run(&self, context: NodeContext, inputs: Inputs) -> BoxFuture<Result<Values, NodeError>> {
        Box::pin(async move {
            let written = inputs.text("directory")?;
            let failed =
                |error: std::io::Error| NodeError::new(format!("cannot list {written}: {error}"));
            let mut entries = tokio::fs::read_dir(context.resolve(&written))
                .await
                .map_err(failed)?;
            let mut names = Vec::new();
            while let Some(entry) = entries.next_entry().await.map_err(failed)? {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
            names.sort_unstable();
            let paths = names
                .into_iter()
                .map(|name| Value::String(child(&written, &name)))
                .collect();
            Ok(Values::from_iter([(
                "paths".to_owned(),
                Value::Array(paths),
            )]))
        })
    }
}

fn child(directory: &str, name: &str) -> String {
    if directory.is_empty() {
        return name.to_owned();
    }
    if directory.ends_with(['/', '\\']) {
        format!("{directory}{name}")
    } else {
        format!("{directory}/{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::child;

    #[test]
    fn joins_listed_names_onto_the_directory_as_written() {
        assert_eq!(child("", "a.txt"), "a.txt");
        assert_eq!(child(".", "a.txt"), "./a.txt");
        assert_eq!(child("out/", "a.txt"), "out/a.txt");
        assert_eq!(child("/", "a.txt"), "/a.txt");
    }
}
