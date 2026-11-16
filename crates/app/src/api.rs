use std::collections::BTreeMap;

use dioxus::fullstack::ServerEvents;
use dioxus::prelude::*;
use parameters_api::{
    ImportItem, NodeRecord, NodeSummary, Page, Problem, Resolution, Review, RunState, RunSummary,
    RunUpdate, SecretSummary, Source, Uuid, WorkflowRecord, WorkflowSummary,
};
use parameters_core::{Definition, OrderedMap};
use serde_json::{Map, Value};

use crate::failure::Failure;
#[cfg(feature = "server")]
use crate::server::{service, ui};

#[get("/api/workflows?size&after")]
pub(crate) async fn list_workflows(
    size: Option<u32>,
    after: Option<String>,
) -> Result<Page<WorkflowSummary>, Failure> {
    Ok(service()?
        .list_workflows(&ui(), size, after.as_deref())
        .await?)
}

#[post("/api/workflows")]
pub(crate) async fn create_workflow(source: Source) -> Result<WorkflowRecord, Failure> {
    Ok(service()?.create_workflow(&ui(), &source).await?)
}

#[get("/api/workflows/{id}")]
pub(crate) async fn get_workflow(id: Uuid) -> Result<WorkflowRecord, Failure> {
    Ok(service()?.get_workflow(&ui(), id).await?)
}

#[put("/api/workflows/{id}")]
pub(crate) async fn update_workflow(
    id: Uuid,
    version: i32,
    source: Source,
) -> Result<WorkflowRecord, Failure> {
    Ok(service()?
        .update_workflow(&ui(), id, version, &source)
        .await?)
}

#[delete("/api/workflows/{id}")]
pub(crate) async fn delete_workflow(id: Uuid) -> Result<(), Failure> {
    Ok(service()?.delete_workflow(&ui(), id).await?)
}

#[post("/api/check")]
#[allow(clippy::unused_async, reason = "server functions are async")]
pub(crate) async fn check_workflow(source: Source) -> Result<Vec<Problem>, Failure> {
    Ok(service()?.check(&source))
}

#[get("/api/workflows/{id}/review")]
pub(crate) async fn review_workflow(id: Uuid) -> Result<Review, Failure> {
    Ok(service()?.review_workflow(&ui(), id).await?)
}

#[post("/api/workflows/{id}/approve")]
pub(crate) async fn approve_workflow(id: Uuid, hash: String) -> Result<Review, Failure> {
    Ok(service()?.approve_workflow(&ui(), id, &hash).await?)
}

#[post("/api/workflows/{id}/runs")]
pub(crate) async fn start_run(id: Uuid, inputs: Map<String, Value>) -> Result<RunSummary, Failure> {
    Ok(service()?.start_run(&ui(), id, inputs).await?)
}

#[get("/api/runs?workflow&size&after")]
pub(crate) async fn list_runs(
    workflow: Option<Uuid>,
    size: Option<u32>,
    after: Option<String>,
) -> Result<Page<RunSummary>, Failure> {
    Ok(service()?
        .list_runs(&ui(), workflow, size, after.as_deref())
        .await?)
}

#[get("/api/runs/{id}")]
pub(crate) async fn get_run(id: Uuid) -> Result<RunUpdate, Failure> {
    Ok(service()?.get_run(&ui(), id).await?)
}

#[post("/api/runs/{id}/cancel")]
pub(crate) async fn cancel_run(id: Uuid) -> Result<RunState, Failure> {
    Ok(service()?.cancel_run(&ui(), id).await?)
}

#[get("/api/runs/{id}/events")]
pub(crate) async fn watch_run(id: Uuid) -> Result<ServerEvents<RunUpdate>, Failure> {
    let mut watch = service()?.watch_run(&ui(), id).await?;
    Ok(ServerEvents::new(move |mut events| async move {
        while let Some(Ok(update)) = watch.next().await {
            if events.send(update).await.is_err() {
                return;
            }
        }
    }))
}

#[get("/api/secrets")]
pub(crate) async fn list_secrets() -> Result<Vec<SecretSummary>, Failure> {
    Ok(service()?.list_secrets(&ui()).await?)
}

#[put("/api/secrets/{name}")]
pub(crate) async fn set_secret(name: String, value: String) -> Result<(), Failure> {
    Ok(service()?.set_secret(&ui(), &name, &value).await?)
}

#[delete("/api/secrets/{name}")]
pub(crate) async fn delete_secret(name: String) -> Result<(), Failure> {
    Ok(service()?.delete_secret(&ui(), &name).await?)
}

#[get("/api/nodes")]
pub(crate) async fn list_nodes() -> Result<Vec<NodeSummary>, Failure> {
    Ok(service()?.list_nodes(&ui()).await?)
}

#[get("/api/nodes/{type_name}")]
pub(crate) async fn get_node(type_name: String) -> Result<NodeRecord, Failure> {
    Ok(service()?.get_node(&ui(), &type_name).await?)
}

#[put("/api/nodes/{type_name}")]
pub(crate) async fn save_node(
    type_name: String,
    definition: Definition,
    version: Option<i32>,
) -> Result<NodeRecord, Failure> {
    Ok(service()?
        .save_node(&ui(), &type_name, definition, version)
        .await?)
}

#[post("/api/nodes/{type_name}/rename")]
pub(crate) async fn rename_node(type_name: String, to: String) -> Result<NodeRecord, Failure> {
    Ok(service()?.rename_node(&ui(), &type_name, &to).await?)
}

#[delete("/api/nodes/{type_name}?version")]
pub(crate) async fn delete_node(type_name: String, version: i32) -> Result<(), Failure> {
    Ok(service()?.delete_node(&ui(), &type_name, version).await?)
}

#[get("/api/nodes/{type_name}/users")]
pub(crate) async fn node_users(type_name: String) -> Result<Vec<WorkflowSummary>, Failure> {
    Ok(service()?.node_users(&ui(), &type_name).await?)
}

#[post("/api/node-imports/plan")]
pub(crate) async fn plan_node_import(
    definitions: OrderedMap<Definition>,
) -> Result<Vec<ImportItem>, Failure> {
    Ok(service()?.plan_node_import(&ui(), &definitions).await?)
}

#[post("/api/node-imports")]
pub(crate) async fn import_nodes(
    definitions: OrderedMap<Definition>,
    resolutions: BTreeMap<String, Resolution>,
) -> Result<Vec<ImportItem>, Failure> {
    Ok(service()?
        .import_nodes(&ui(), &definitions, &resolutions)
        .await?)
}
