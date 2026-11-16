use std::collections::{BTreeMap, HashMap, HashSet};

use parameters_api::{
    ApiError, ImportItem, ImportStatus, NodeInfo, NodeKind, NodeRecord, NodeSummary, Problem,
    Resolution, Scope, WorkflowSummary,
};
use parameters_core::name::is_valid_node_type;
use parameters_core::{
    Catalog, Definition, MergeStatus, OrderedMap, Signature, Workflow, merge_definitions,
    plan_merge, rename_node_type, validate,
};
use parameters_engine::Registry;
use parameters_store::{LibraryChange, StoredNode, hex};
use uuid::Uuid;

use crate::library::node_info;
use crate::{Actor, Result, Service, internal, source};

pub(crate) struct Library {
    pub(crate) workflow: Workflow,
    stored: HashMap<String, StoredNode>,
}

impl Service {
    pub async fn list_nodes(&self, actor: &Actor) -> Result<Vec<NodeSummary>> {
        actor.require(Scope::Read)?;
        self.store()
            .list_nodes(actor.user)
            .await
            .map_err(internal)?
            .into_iter()
            .map(|stored| record(stored).map(|record| record.summary))
            .collect()
    }

    pub async fn node_catalog(&self, actor: &Actor) -> Result<Vec<NodeInfo>> {
        actor.require(Scope::Read)?;
        let registry = self.registry();
        let mut catalog: Vec<NodeInfo> = registry
            .node_types()
            .into_iter()
            .filter(|name| {
                self.inner.options.capabilities.sandbox.privileged_nodes
                    || !registry.is_privileged(name)
            })
            .filter_map(|name| Some((name, registry.signature(name)?)))
            .map(|(name, signature)| builtin_info(registry, name, signature))
            .collect();
        let library = self.library(actor.user).await?;
        catalog.extend(
            library
                .workflow
                .definitions
                .iter()
                .map(|(name, definition)| {
                    custom_info(name, definition, &library.workflow, registry)
                }),
        );
        Ok(catalog)
    }

    pub async fn describe_node(&self, actor: &Actor, type_name: &str) -> Result<NodeInfo> {
        actor.require(Scope::Read)?;
        let registry = self.registry();
        if let Some(signature) = registry.signature(type_name) {
            return Ok(builtin_info(registry, type_name, signature));
        }
        let library = self.library(actor.user).await?;
        library
            .workflow
            .definitions
            .get(type_name)
            .map(|definition| custom_info(type_name, definition, &library.workflow, registry))
            .ok_or_else(|| ApiError::not_found(format!("there is no node named `{type_name}`")))
    }

    pub async fn get_node(&self, actor: &Actor, type_name: &str) -> Result<NodeRecord> {
        actor.require(Scope::Read)?;
        record(
            self.store()
                .get_node(actor.user, type_name)
                .await
                .map_err(internal)?
                .ok_or_else(|| node_not_found(type_name))?,
        )
    }

    pub async fn save_node(
        &self,
        actor: &Actor,
        type_name: &str,
        definition: Definition,
        expected_version: Option<i32>,
    ) -> Result<NodeRecord> {
        actor.require(Scope::Write)?;
        if !is_valid_node_type(type_name) {
            return Err(ApiError::bad_request(format!(
                "`{type_name}` is not a valid node type: use a qualified name such as `acme.deploy`"
            )));
        }
        if self.registry().contains(type_name) {
            return Err(ApiError::bad_request(format!(
                "`{type_name}` is a built-in node; choose another name"
            )));
        }
        let mut library = self.library(actor.user).await?;
        let current = library.stored.get(type_name).map(|stored| stored.version);
        if current != expected_version {
            return Err(ApiError::Conflict {
                message: match current {
                    Some(version) => format!(
                        "the node `{type_name}` is at version {version}; load it again and reapply the edit"
                    ),
                    None => format!("there is no node named `{type_name}` to update"),
                },
                current_version: current,
            });
        }
        library.workflow.definitions.insert(type_name, definition);
        self.commit(actor.user, &library).await?;
        self.get_node(actor, type_name).await
    }

    pub async fn rename_node(&self, actor: &Actor, from: &str, to: &str) -> Result<NodeRecord> {
        actor.require(Scope::Write)?;
        let mut library = self.library(actor.user).await?;
        rename_node_type(&mut library.workflow, from, to, self.registry())
            .map_err(|error| ApiError::bad_request(error.to_string()))?;
        self.commit(actor.user, &library).await?;
        self.get_node(actor, to).await
    }

    pub async fn delete_node(
        &self,
        actor: &Actor,
        type_name: &str,
        expected_version: i32,
    ) -> Result<()> {
        actor.require(Scope::Write)?;
        let mut library = self.library(actor.user).await?;
        if !library.stored.contains_key(type_name) {
            return Err(node_not_found(type_name));
        }
        let users: Vec<&str> = library
            .workflow
            .definitions
            .iter()
            .filter(|(_, definition)| definition.steps.iter().any(|step| step.uses == type_name))
            .map(|(name, _)| name)
            .collect();
        if !users.is_empty() {
            return Err(ApiError::bad_request(format!(
                "`{type_name}` is used by {}; change those nodes first",
                users.join(", ")
            )));
        }
        library.workflow.definitions.remove(type_name);
        if let Some(stored) = library.stored.get_mut(type_name) {
            stored.version = expected_version;
        }
        self.commit(actor.user, &library).await
    }

    pub async fn plan_node_import(
        &self,
        actor: &Actor,
        definitions: &OrderedMap<Definition>,
    ) -> Result<Vec<ImportItem>> {
        actor.require(Scope::Read)?;
        let library = self.library(actor.user).await?;
        let items = plan_merge(&library.workflow.definitions, definitions, self.registry())
            .map_err(|error| ApiError::bad_request(error.to_string()))?;
        Ok(items.into_iter().map(import_item).collect())
    }

    pub async fn import_nodes(
        &self,
        actor: &Actor,
        definitions: &OrderedMap<Definition>,
        resolutions: &BTreeMap<String, Resolution>,
    ) -> Result<Vec<ImportItem>> {
        actor.require(Scope::Write)?;
        let mut library = self.library(actor.user).await?;
        let resolutions = resolutions
            .iter()
            .map(|(name, resolution)| {
                let resolution = match resolution {
                    Resolution::Rename(to) => parameters_core::Resolution::Rename(to.clone()),
                    Resolution::UseExisting => parameters_core::Resolution::UseExisting,
                    Resolution::Replace => parameters_core::Resolution::Replace,
                };
                (name.clone(), resolution)
            })
            .collect();
        let outcome = merge_definitions(
            &mut library.workflow.definitions,
            definitions,
            self.registry(),
            &resolutions,
        )
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
        self.commit(actor.user, &library).await?;
        Ok(outcome.items.into_iter().map(import_item).collect())
    }

    pub async fn node_users(&self, actor: &Actor, type_name: &str) -> Result<Vec<WorkflowSummary>> {
        actor.require(Scope::Read)?;
        self.store()
            .workflows_using_node(actor.user, type_name)
            .await
            .map_err(internal)
    }

    pub(crate) async fn library(&self, owner: Uuid) -> Result<Library> {
        let nodes = self.store().list_nodes(owner).await.map_err(internal)?;
        let mut workflow = Workflow::new("Node library");
        let mut stored = HashMap::with_capacity(nodes.len());
        for node in nodes {
            workflow
                .definitions
                .insert(node.type_name.clone(), parse(&node)?);
            stored.insert(node.type_name.clone(), node);
        }
        Ok(Library { workflow, stored })
    }

    async fn commit(&self, owner: Uuid, library: &Library) -> Result<()> {
        let problems: Vec<Problem> = validate(&library.workflow, self.registry())
            .iter()
            .filter(|diagnostic| diagnostic.is_error())
            .map(Problem::from)
            .collect();
        if !problems.is_empty() {
            return Err(source::invalid(problems));
        }
        let mut changes = Vec::new();
        for (name, definition) in library.workflow.definitions.iter() {
            let text = serde_json::to_string(definition).map_err(internal)?;
            let previous = library.stored.get(name);
            if previous.is_some_and(|stored| stored.definition == text) {
                continue;
            }
            changes.push(LibraryChange::Put {
                type_name: name.to_owned(),
                hash: *definition.content_hash().as_bytes(),
                definition: text,
                expected_version: previous.map(|stored| stored.version),
            });
        }
        for (name, stored) in &library.stored {
            if !library.workflow.definitions.contains_key(name) {
                changes.push(LibraryChange::Delete {
                    type_name: name.clone(),
                    expected_version: stored.version,
                });
            }
        }
        if changes.is_empty()
            || self
                .store()
                .change_library(owner, &changes)
                .await
                .map_err(internal)?
        {
            Ok(())
        } else {
            Err(ApiError::Conflict {
                message: "the node library changed while saving; load it again and retry".into(),
                current_version: None,
            })
        }
    }
}

fn builtin_info(registry: &Registry, type_name: &str, signature: &Signature) -> NodeInfo {
    node_info(
        type_name,
        NodeKind::Builtin,
        registry.description(type_name).map(str::to_owned),
        registry.is_privileged(type_name),
        signature,
    )
}

fn custom_info(
    type_name: &str,
    definition: &Definition,
    library: &Workflow,
    registry: &Registry,
) -> NodeInfo {
    let privileged = reachable(type_name, library).iter().any(|definition| {
        definition
            .steps
            .iter()
            .any(|step| registry.is_privileged(&step.uses))
    });
    node_info(
        type_name,
        NodeKind::Custom,
        definition.note.clone(),
        privileged,
        &Signature::of_definition(definition),
    )
}

fn reachable<'a>(type_name: &'a str, library: &'a Workflow) -> Vec<&'a Definition> {
    let mut seen = HashSet::new();
    let mut pending = vec![type_name];
    let mut found = Vec::new();
    while let Some(name) = pending.pop() {
        if !seen.insert(name) {
            continue;
        }
        if let Some(definition) = library.definitions.get(name) {
            pending.extend(definition.steps.iter().map(|step| step.uses.as_str()));
            found.push(definition);
        }
    }
    found
}

fn parse(stored: &StoredNode) -> Result<Definition> {
    serde_json::from_str(&stored.definition).map_err(internal)
}

fn record(stored: StoredNode) -> Result<NodeRecord> {
    let definition = parse(&stored)?;
    Ok(NodeRecord {
        summary: NodeSummary {
            note: definition.note.clone(),
            hash: hex(&stored.hash),
            type_name: stored.type_name,
            version: stored.version,
            updated_at: stored.updated_at,
        },
        definition,
    })
}

fn import_item(item: parameters_core::MergeItem) -> ImportItem {
    ImportItem {
        name: item.name,
        status: match item.status {
            MergeStatus::New => ImportStatus::New,
            MergeStatus::Identical => ImportStatus::Identical,
            MergeStatus::Conflict { suggested } => ImportStatus::Conflict { suggested },
            MergeStatus::BuiltinClash { suggested } => ImportStatus::BuiltinClash { suggested },
        },
    }
}

fn node_not_found(type_name: &str) -> ApiError {
    ApiError::not_found(format!(
        "there is no node named `{type_name}` in the library"
    ))
}
