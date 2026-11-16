use std::collections::BTreeMap;

use dioxus::fullstack::ServerEvents;
use dioxus::prelude::*;
use parameters_api::{
    CreatedToken, CreatedTrigger, Format, ImportItem, Me, NewTrigger, NodeInfo, NodeRecord,
    NodeSummary, Page, Problem, Resolution, Review, RunState, RunSummary, RunUpdate, SavedWorkflow,
    Scopes, SecretSummary, ServerDocs, Source, TokenSummary, TriggerSummary, Uuid, WorkflowRecord,
    WorkflowSummary,
};
use parameters_core::{Definition, OrderedMap};
use serde_json::{Map, Value};

use crate::failure::Failure;
#[cfg(feature = "server")]
use crate::server::{actor, client_address, service, signed_in};

#[get("/api/workflows?size&after")]
pub(crate) async fn list_workflows(
    size: Option<u32>,
    after: Option<String>,
) -> Result<Page<WorkflowSummary>, Failure> {
    Ok(service()?
        .list_workflows(&actor()?, size, after.as_deref())
        .await?)
}

#[post("/api/workflows")]
pub(crate) async fn create_workflow(source: Source) -> Result<SavedWorkflow, Failure> {
    Ok(service()?.create_workflow(&actor()?, &source).await?)
}

#[get("/api/workflows/{id}")]
pub(crate) async fn get_workflow(id: Uuid) -> Result<WorkflowRecord, Failure> {
    Ok(service()?.get_workflow(&actor()?, id).await?)
}

#[put("/api/workflows/{id}")]
pub(crate) async fn update_workflow(
    id: Uuid,
    version: i32,
    source: Source,
) -> Result<SavedWorkflow, Failure> {
    Ok(service()?
        .update_workflow(&actor()?, id, version, &source)
        .await?)
}

#[get("/api/workflows/{id}/events")]
pub(crate) async fn watch_workflow(id: Uuid) -> Result<ServerEvents<WorkflowRecord>, Failure> {
    let mut watch = service()?.watch_workflow(&actor()?, id).await?;
    Ok(ServerEvents::new(move |mut events| async move {
        while let Some(Ok(record)) = watch.next().await {
            if events.send(record).await.is_err() {
                return;
            }
        }
    }))
}

#[delete("/api/workflows/{id}")]
pub(crate) async fn delete_workflow(id: Uuid) -> Result<(), Failure> {
    Ok(service()?.delete_workflow(&actor()?, id).await?)
}

#[post("/api/check")]
pub(crate) async fn check_workflow(source: Source) -> Result<Vec<Problem>, Failure> {
    Ok(service()?.validate(&actor()?, &source).await?)
}

#[post("/api/convert")]
#[allow(clippy::unused_async, reason = "server functions are async")]
pub(crate) async fn convert_workflow(source: Source, to: Format) -> Result<String, Failure> {
    Ok(service()?.convert(&source, to)?)
}

#[get("/api/workflows/{id}/review")]
pub(crate) async fn review_workflow(id: Uuid) -> Result<Review, Failure> {
    Ok(service()?.review_workflow(&actor()?, id).await?)
}

#[post("/api/workflows/{id}/approve")]
pub(crate) async fn approve_workflow(id: Uuid, hash: String) -> Result<Review, Failure> {
    Ok(service()?.approve_workflow(&actor()?, id, &hash).await?)
}

#[post("/api/workflows/{id}/runs")]
pub(crate) async fn start_run(id: Uuid, inputs: Map<String, Value>) -> Result<RunSummary, Failure> {
    Ok(service()?.start_run(&actor()?, id, inputs).await?)
}

#[get("/api/runs?workflow&size&after")]
pub(crate) async fn list_runs(
    workflow: Option<Uuid>,
    size: Option<u32>,
    after: Option<String>,
) -> Result<Page<RunSummary>, Failure> {
    Ok(service()?
        .list_runs(&actor()?, workflow, size, after.as_deref())
        .await?)
}

#[get("/api/runs/{id}")]
pub(crate) async fn get_run(id: Uuid) -> Result<RunUpdate, Failure> {
    Ok(service()?.get_run(&actor()?, id).await?)
}

#[post("/api/runs/{id}/cancel")]
pub(crate) async fn cancel_run(id: Uuid) -> Result<RunState, Failure> {
    Ok(service()?.cancel_run(&actor()?, id).await?)
}

#[get("/api/runs/{id}/events")]
pub(crate) async fn watch_run(id: Uuid) -> Result<ServerEvents<RunUpdate>, Failure> {
    let mut watch = service()?.watch_run(&actor()?, id).await?;
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
    Ok(service()?.list_secrets(&actor()?).await?)
}

#[put("/api/secrets/{name}")]
pub(crate) async fn set_secret(name: String, value: String) -> Result<(), Failure> {
    Ok(service()?.set_secret(&actor()?, &name, &value).await?)
}

#[delete("/api/secrets/{name}")]
pub(crate) async fn delete_secret(name: String) -> Result<(), Failure> {
    Ok(service()?.delete_secret(&actor()?, &name).await?)
}

#[get("/api/node-catalog")]
pub(crate) async fn node_catalog() -> Result<Vec<NodeInfo>, Failure> {
    Ok(service()?.node_catalog(&actor()?).await?)
}

#[get("/api/node-catalog/{type_name}")]
pub(crate) async fn describe_node(type_name: String) -> Result<NodeInfo, Failure> {
    Ok(service()?.describe_node(&actor()?, &type_name).await?)
}

#[get("/api/nodes")]
pub(crate) async fn list_nodes() -> Result<Vec<NodeSummary>, Failure> {
    Ok(service()?.list_nodes(&actor()?).await?)
}

#[get("/api/nodes/{type_name}")]
pub(crate) async fn get_node(type_name: String) -> Result<NodeRecord, Failure> {
    Ok(service()?.get_node(&actor()?, &type_name).await?)
}

#[put("/api/nodes/{type_name}")]
pub(crate) async fn save_node(
    type_name: String,
    definition: Definition,
    version: Option<i32>,
) -> Result<NodeRecord, Failure> {
    Ok(service()?
        .save_node(&actor()?, &type_name, definition, version)
        .await?)
}

#[post("/api/nodes/{type_name}/rename")]
pub(crate) async fn rename_node(type_name: String, to: String) -> Result<NodeRecord, Failure> {
    Ok(service()?.rename_node(&actor()?, &type_name, &to).await?)
}

#[delete("/api/nodes/{type_name}?version")]
pub(crate) async fn delete_node(type_name: String, version: i32) -> Result<(), Failure> {
    Ok(service()?
        .delete_node(&actor()?, &type_name, version)
        .await?)
}

#[get("/api/nodes/{type_name}/users")]
pub(crate) async fn node_users(type_name: String) -> Result<Vec<WorkflowSummary>, Failure> {
    Ok(service()?.node_users(&actor()?, &type_name).await?)
}

#[post("/api/node-imports/plan")]
pub(crate) async fn plan_node_import(
    definitions: OrderedMap<Definition>,
) -> Result<Vec<ImportItem>, Failure> {
    Ok(service()?.plan_node_import(&actor()?, &definitions).await?)
}

#[post("/api/node-imports")]
pub(crate) async fn import_nodes(
    definitions: OrderedMap<Definition>,
    resolutions: BTreeMap<String, Resolution>,
) -> Result<Vec<ImportItem>, Failure> {
    Ok(service()?
        .import_nodes(&actor()?, &definitions, &resolutions)
        .await?)
}

#[get("/api/me")]
pub(crate) async fn me() -> Result<Me, Failure> {
    Ok(service()?.me(signed_in().as_ref()).await?)
}

#[post("/api/auth/email")]
pub(crate) async fn request_sign_in(email: String, next: Option<String>) -> Result<(), Failure> {
    Ok(service()?
        .request_email_sign_in(&email, &client_address(), next.as_deref())
        .await?)
}

#[post("/api/devices/approve")]
pub(crate) async fn approve_device(code: String) -> Result<String, Failure> {
    Ok(service()?.approve_device(&actor()?, &code).await?)
}

#[get("/api/tokens")]
pub(crate) async fn list_tokens() -> Result<Vec<TokenSummary>, Failure> {
    Ok(service()?.tokens(&actor()?).await?)
}

#[post("/api/tokens")]
pub(crate) async fn create_token(
    name: String,
    scopes: Scopes,
    expires_in_days: Option<u32>,
) -> Result<CreatedToken, Failure> {
    Ok(service()?
        .create_token(&actor()?, &name, scopes, expires_in_days)
        .await?)
}

#[delete("/api/tokens/{id}")]
pub(crate) async fn revoke_token(id: Uuid) -> Result<(), Failure> {
    Ok(service()?.revoke_token(&actor()?, id).await?)
}

#[get("/api/triggers?workflow")]
pub(crate) async fn list_triggers(workflow: Option<Uuid>) -> Result<Vec<TriggerSummary>, Failure> {
    Ok(service()?.list_triggers(&actor()?, workflow).await?)
}

#[post("/api/triggers")]
pub(crate) async fn create_trigger(
    workflow_id: Uuid,
    name: String,
    kind: NewTrigger,
    inputs: Map<String, Value>,
) -> Result<CreatedTrigger, Failure> {
    Ok(service()?
        .create_trigger(&actor()?, workflow_id, &name, kind, inputs)
        .await?)
}

#[post("/api/triggers/{id}/enabled")]
pub(crate) async fn set_trigger_enabled(id: Uuid, enabled: bool) -> Result<(), Failure> {
    Ok(service()?
        .set_trigger_enabled(&actor()?, id, enabled)
        .await?)
}

#[delete("/api/triggers/{id}")]
pub(crate) async fn delete_trigger(id: Uuid) -> Result<(), Failure> {
    Ok(service()?.delete_trigger(&actor()?, id).await?)
}

#[get("/api/docs")]
pub(crate) async fn server_docs() -> Result<ServerDocs, Failure> {
    let service = service()?;
    let actor = signed_in();
    Ok(ServerDocs {
        mode: if service.is_managed() {
            parameters_api::Mode::Managed
        } else {
            parameters_api::Mode::Local
        },
        nodes: service.documented_nodes(actor.as_ref()).await?,
        tools: parameters_mcp::tool_docs(),
        limits: service.limits(),
    })
}
