use std::collections::BTreeMap;

use parameters_core::{Port, Signature, ValueType};

#[must_use]
pub fn builtins() -> BTreeMap<String, Signature> {
    use ValueType::{Any, List, Number, Object, Path, Text};
    let command_outputs = || {
        vec![
            Port::optional("stdout", Text),
            Port::optional("stderr", Text),
            Port::optional("exit_code", Number),
        ]
    };
    [
        (
            "http.request",
            vec![
                Port::required("url", Text),
                Port::optional("method", Text),
                Port::optional("headers", Object),
                Port::optional("query", Object),
                Port::optional("body", Any),
                Port::optional("file", Path),
            ],
            vec![
                Port::optional("status", Number),
                Port::optional("headers", Object),
                Port::optional("body", Any),
            ],
        ),
        (
            "shell.run",
            vec![
                Port::required("command", Text),
                Port::optional("directory", Path),
                Port::optional("env", Object),
            ],
            command_outputs(),
        ),
        (
            "process.run",
            vec![
                Port::required("program", Text),
                Port::optional("arguments", List),
                Port::optional("directory", Path),
                Port::optional("env", Object),
                Port::optional("input", Text),
            ],
            command_outputs(),
        ),
        (
            "file.read",
            vec![Port::required("path", Path)],
            vec![Port::optional("content", Text)],
        ),
        (
            "file.write",
            vec![
                Port::required("path", Path),
                Port::required("content", Text),
            ],
            vec![Port::optional("path", Path)],
        ),
        (
            "file.list",
            vec![Port::required("directory", Path)],
            vec![Port::optional("paths", List)],
        ),
        ("time.wait", vec![Port::required("duration", Text)], vec![]),
        ("log.write", vec![Port::required("message", Text)], vec![]),
    ]
    .into_iter()
    .map(|(name, inputs, outputs)| (name.to_owned(), Signature { inputs, outputs }))
    .collect()
}

#[cfg(test)]
mod tests {
    use parameters_core::name::is_valid_node_type;

    use super::*;

    #[test]
    fn built_in_node_types_are_valid_and_unique_per_port() {
        for (name, signature) in builtins() {
            assert!(is_valid_node_type(&name), "{name}");
            for ports in [&signature.inputs, &signature.outputs] {
                let mut names: Vec<&str> = ports.iter().map(|port| port.name.as_str()).collect();
                names.sort_unstable();
                names.dedup();
                assert_eq!(names.len(), ports.len(), "{name} has duplicate ports");
            }
        }
    }
}
