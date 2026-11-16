use std::collections::{BTreeMap, HashMap, HashSet};

use crate::catalog::Catalog;
use crate::document::Definition;
use crate::graph::DependencyGraph;
use crate::name::is_valid_node_type;
use crate::ordered_map::OrderedMap;
use crate::rename::rename_uses;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeStatus {
    New,
    Identical,
    Conflict { suggested: String },
    BuiltinClash { suggested: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeItem {
    pub name: String,
    pub status: MergeStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Rename(String),
    UseExisting,
    Replace,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MergeError {
    #[error("the incoming nodes {} use each other in a cycle", quoted(.0))]
    DefinitionCycle(Vec<String>),
    #[error("`{0}` is not a valid node type: use a qualified name such as `acme.deploy`")]
    InvalidNodeType(String),
    #[error("the node type `{0}` is already taken")]
    NameTaken(String),
    #[error("`{name}` cannot be resolved that way: {reason}")]
    InvalidResolution { name: String, reason: &'static str },
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MergeOutcome {
    pub items: Vec<MergeItem>,
    pub renames: BTreeMap<String, String>,
}

pub fn plan_merge(
    existing: &OrderedMap<Definition>,
    incoming: &OrderedMap<Definition>,
    builtins: &dyn Catalog,
) -> Result<Vec<MergeItem>, MergeError> {
    Ok(resolve(existing, incoming, builtins, &BTreeMap::new())?
        .outcome
        .items)
}

pub fn merge_definitions(
    existing: &mut OrderedMap<Definition>,
    incoming: &OrderedMap<Definition>,
    builtins: &dyn Catalog,
    resolutions: &BTreeMap<String, Resolution>,
) -> Result<MergeOutcome, MergeError> {
    let resolved = resolve(existing, incoming, builtins, resolutions)?;
    for (name, definition) in resolved.writes {
        existing.insert(name, definition);
    }
    Ok(resolved.outcome)
}

struct Resolved {
    outcome: MergeOutcome,
    writes: Vec<(String, Definition)>,
}

fn resolve(
    existing: &OrderedMap<Definition>,
    incoming: &OrderedMap<Definition>,
    builtins: &dyn Catalog,
    resolutions: &BTreeMap<String, Resolution>,
) -> Result<Resolved, MergeError> {
    let names: Vec<&str> = incoming.keys().collect();
    let mut taken: HashSet<String> = existing
        .keys()
        .chain(incoming.keys())
        .map(str::to_owned)
        .collect();
    let mut outcome = MergeOutcome::default();
    let mut writes = Vec::new();
    for index in dependency_order(incoming)? {
        let name = names[index];
        let mut definition = incoming.get(name).cloned().unwrap_or_default();
        rename_uses(definition.steps.iter_mut(), &outcome.renames);
        let status = if builtins.contains(name) {
            MergeStatus::BuiltinClash {
                suggested: free_name(name, &taken, builtins),
            }
        } else {
            match existing.get(name) {
                None => MergeStatus::New,
                Some(current) if current.content_hash() == definition.content_hash() => {
                    MergeStatus::Identical
                }
                Some(_) => MergeStatus::Conflict {
                    suggested: free_name(name, &taken, builtins),
                },
            }
        };
        let target = match (&status, resolutions.get(name)) {
            (_, Some(Resolution::Rename(new))) => Some((new.clone(), true)),
            (MergeStatus::New, None | Some(Resolution::Replace))
            | (MergeStatus::Conflict { .. }, Some(Resolution::Replace)) => {
                Some((name.to_owned(), false))
            }
            (
                MergeStatus::Conflict { suggested } | MergeStatus::BuiltinClash { suggested },
                None,
            ) => Some((suggested.clone(), true)),
            (MergeStatus::Identical, _)
            | (MergeStatus::Conflict { .. }, Some(Resolution::UseExisting)) => None,
            (MergeStatus::New, Some(Resolution::UseExisting)) => {
                return Err(invalid(name, "there is no existing node with that name"));
            }
            (MergeStatus::BuiltinClash { .. }, Some(_)) => {
                return Err(invalid(
                    name,
                    "built-in nodes cannot be replaced, so it must be renamed",
                ));
            }
        };
        if let Some((target, renamed)) = target {
            if renamed {
                check_free(&target, &taken, builtins)?;
                taken.insert(target.clone());
                outcome.renames.insert(name.to_owned(), target.clone());
            }
            writes.push((target, definition));
        }
        outcome.items.push(MergeItem {
            name: name.to_owned(),
            status,
        });
    }
    Ok(Resolved { outcome, writes })
}

fn dependency_order(definitions: &OrderedMap<Definition>) -> Result<Vec<usize>, MergeError> {
    let indices: HashMap<&str, usize> = definitions
        .keys()
        .enumerate()
        .map(|(index, name)| (name, index))
        .collect();
    let uses: Vec<Vec<usize>> = definitions
        .values()
        .map(|definition| {
            definition
                .steps
                .iter()
                .filter_map(|step| indices.get(step.uses.as_str()).copied())
                .collect()
        })
        .collect();
    DependencyGraph::from_dependencies(&uses)
        .topological_order()
        .map_err(|found| {
            let names: Vec<&str> = definitions.keys().collect();
            MergeError::DefinitionCycle(
                found
                    .cycles
                    .into_iter()
                    .flatten()
                    .map(|index| names[index].to_owned())
                    .collect(),
            )
        })
}

fn free_name(name: &str, taken: &HashSet<String>, builtins: &dyn Catalog) -> String {
    let base = match name.rsplit_once('_') {
        Some((head, suffix))
            if !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            head
        }
        _ => name,
    };
    let candidates = taken.len() + builtins.node_types().len() + 2;
    (2..=candidates)
        .map(|number| format!("{base}_{number}"))
        .find(|candidate| !taken.contains(candidate) && !builtins.contains(candidate))
        .unwrap_or_else(|| format!("{base}_copy"))
}

fn check_free(
    name: &str,
    taken: &HashSet<String>,
    builtins: &dyn Catalog,
) -> Result<(), MergeError> {
    if !is_valid_node_type(name) {
        Err(MergeError::InvalidNodeType(name.to_owned()))
    } else if taken.contains(name) || builtins.contains(name) {
        Err(MergeError::NameTaken(name.to_owned()))
    } else {
        Ok(())
    }
}

fn invalid(name: &str, reason: &'static str) -> MergeError {
    MergeError::InvalidResolution {
        name: name.to_owned(),
        reason,
    }
}

fn quoted(names: &[String]) -> String {
    names
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Signature;
    use crate::document::Step;

    fn builtins() -> BTreeMap<String, Signature> {
        BTreeMap::from([("http.request".to_owned(), Signature::default())])
    }

    fn definition(note: &str, uses: &[&str]) -> Definition {
        Definition {
            note: Some(note.to_owned()),
            steps: uses
                .iter()
                .enumerate()
                .map(|(index, node_type)| Step::new(format!("step_{index}"), *node_type))
                .collect(),
            ..Definition::default()
        }
    }

    fn map(entries: &[(&str, Definition)]) -> OrderedMap<Definition> {
        entries
            .iter()
            .map(|(name, definition)| ((*name).to_owned(), definition.clone()))
            .collect()
    }

    #[test]
    fn classifies_new_identical_conflicting_and_builtin_names() {
        let existing = map(&[
            ("acme.same", definition("same", &[])),
            ("acme.deploy", definition("ours", &[])),
        ]);
        let incoming = map(&[
            ("acme.same", definition("same", &[])),
            ("acme.deploy", definition("theirs", &[])),
            ("acme.fresh", definition("fresh", &[])),
            ("http.request", definition("fake", &[])),
        ]);
        let items = plan_merge(&existing, &incoming, &builtins()).unwrap();
        let status = |name: &str| {
            items
                .iter()
                .find(|item| item.name == name)
                .map(|item| item.status.clone())
                .unwrap()
        };
        assert_eq!(status("acme.same"), MergeStatus::Identical);
        assert_eq!(
            status("acme.deploy"),
            MergeStatus::Conflict {
                suggested: "acme.deploy_2".into()
            }
        );
        assert_eq!(status("acme.fresh"), MergeStatus::New);
        assert_eq!(
            status("http.request"),
            MergeStatus::BuiltinClash {
                suggested: "http.request_2".into()
            }
        );
    }

    #[test]
    fn default_merge_renames_conflicts_and_rewrites_incoming_uses() {
        let mut existing = map(&[("acme.upload", definition("ours", &[]))]);
        let incoming = map(&[
            ("acme.release", definition("wrapper", &["acme.upload"])),
            ("acme.upload", definition("theirs", &["http.request"])),
        ]);
        let outcome =
            merge_definitions(&mut existing, &incoming, &builtins(), &BTreeMap::new()).unwrap();
        assert_eq!(
            outcome.renames,
            BTreeMap::from([("acme.upload".to_owned(), "acme.upload_2".to_owned())])
        );
        assert_eq!(
            existing.keys().collect::<Vec<_>>(),
            ["acme.upload", "acme.upload_2", "acme.release"]
        );
        assert_eq!(
            existing.get("acme.release").unwrap().steps[0].uses,
            "acme.upload_2"
        );
        assert_eq!(
            existing.get("acme.upload").unwrap().note.as_deref(),
            Some("ours")
        );
    }

    #[test]
    fn suggestions_skip_taken_suffixes_and_restart_numbering() {
        let existing = map(&[
            ("acme.deploy", definition("a", &[])),
            ("acme.deploy_2", definition("b", &[])),
        ]);
        let incoming = map(&[("acme.deploy_2", definition("c", &[]))]);
        let items = plan_merge(&existing, &incoming, &builtins()).unwrap();
        assert_eq!(
            items[0].status,
            MergeStatus::Conflict {
                suggested: "acme.deploy_3".into()
            }
        );
    }

    #[test]
    fn applies_explicit_resolutions() {
        let mut existing = map(&[
            ("acme.keep", definition("ours", &[])),
            ("acme.swap", definition("ours", &[])),
        ]);
        let incoming = map(&[
            ("acme.keep", definition("theirs", &[])),
            ("acme.swap", definition("theirs", &[])),
            ("acme.named", definition("theirs", &[])),
        ]);
        let resolutions = BTreeMap::from([
            ("acme.keep".to_owned(), Resolution::UseExisting),
            ("acme.swap".to_owned(), Resolution::Replace),
            (
                "acme.named".to_owned(),
                Resolution::Rename("acme.custom".into()),
            ),
        ]);
        merge_definitions(&mut existing, &incoming, &builtins(), &resolutions).unwrap();
        assert_eq!(
            existing.get("acme.keep").unwrap().note.as_deref(),
            Some("ours")
        );
        assert_eq!(
            existing.get("acme.swap").unwrap().note.as_deref(),
            Some("theirs")
        );
        assert!(existing.contains_key("acme.custom"));
        assert!(!existing.contains_key("acme.named"));
    }

    #[test]
    fn rejects_impossible_resolutions_and_cycles() {
        let mut existing = OrderedMap::new();
        let builtins = builtins();
        let incoming = map(&[("http.request", definition("fake", &[]))]);
        let replace = BTreeMap::from([("http.request".to_owned(), Resolution::Replace)]);
        assert!(matches!(
            merge_definitions(&mut existing, &incoming, &builtins, &replace),
            Err(MergeError::InvalidResolution { .. })
        ));
        let incoming = map(&[("acme.fresh", definition("fresh", &[]))]);
        let taken = BTreeMap::from([(
            "acme.fresh".to_owned(),
            Resolution::Rename("http.request".into()),
        )]);
        assert_eq!(
            merge_definitions(&mut existing, &incoming, &builtins, &taken),
            Err(MergeError::NameTaken("http.request".into()))
        );
        let cyclic = map(&[
            ("acme.a", definition("a", &["acme.b"])),
            ("acme.b", definition("b", &["acme.a"])),
        ]);
        assert_eq!(
            plan_merge(&existing, &cyclic, &builtins),
            Err(MergeError::DefinitionCycle(vec![
                "acme.a".into(),
                "acme.b".into()
            ]))
        );
        assert!(existing.is_empty());
    }
}
