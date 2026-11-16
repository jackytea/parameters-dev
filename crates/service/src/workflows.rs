use parameters_api::{
    ApiError, Format, Page, Problem, Review, SavedWorkflow, Scope, Source, Via, WorkflowRecord,
    WorkflowSummary,
};
use parameters_core::layout::complete_layouts;
use parameters_core::{MergeOutcome, Workflow, from_json, to_json, validate};
use parameters_store::{
    Cursor, NewWorkflow, Notification, StoredWorkflow, Versioned, WorkflowUpdate, hex,
};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::hub::changed;
use crate::library::{WithLibrary, include_library};
use crate::page::PageRequest;
use crate::review::inspect;
use crate::{Actor, Result, Service, internal, source};

impl Service {
    pub async fn validate(&self, actor: &Actor, source: &Source) -> Result<Vec<Problem>> {
        actor.require(Scope::Read)?;
        let library = self.library(actor.user).await?.workflow.definitions;
        let declared = source::declared(source);
        Ok(source::check(
            source,
            &WithLibrary::new(self.registry(), &library, &declared),
        )
        .1)
    }

    pub fn convert(&self, source: &Source, to: Format) -> Result<String> {
        source::convert(source, to)
    }

    pub async fn list_workflows(
        &self,
        actor: &Actor,
        size: Option<u32>,
        after: Option<&str>,
    ) -> Result<Page<WorkflowSummary>> {
        actor.require(Scope::Read)?;
        let request = PageRequest::new(size, after)?;
        let items = self
            .store()
            .list_workflows(actor.user, request.fetch_limit(), request.after)
            .await
            .map_err(internal)?;
        Ok(request.page(items, |item| Cursor {
            at: item.updated_at,
            id: item.id,
        }))
    }

    pub async fn get_workflow(&self, actor: &Actor, id: Uuid) -> Result<WorkflowRecord> {
        actor.require(Scope::Read)?;
        record(self.stored_workflow(actor, id).await?)
    }

    pub async fn create_workflow(&self, actor: &Actor, source: &Source) -> Result<SavedWorkflow> {
        actor.require(Scope::Write)?;
        let Prepared {
            workflow,
            document,
            hash,
            merged,
        } = self.prepare(actor, source).await?;
        let summary = self
            .store()
            .insert_workflow(
                actor.user,
                NewWorkflow {
                    name: &workflow.name,
                    document: &document,
                    hash: &hash,
                    via: actor.via,
                },
            )
            .await
            .map_err(internal)?;
        Ok(saved(summary, &hash, workflow, merged))
    }

    pub async fn update_workflow(
        &self,
        actor: &Actor,
        id: Uuid,
        expected_version: i32,
        source: &Source,
    ) -> Result<SavedWorkflow> {
        actor.require(Scope::Write)?;
        let Prepared {
            workflow,
            document,
            hash,
            merged,
        } = self.prepare(actor, source).await?;
        let update = WorkflowUpdate {
            expected_version,
            workflow: NewWorkflow {
                name: &workflow.name,
                document: &document,
                hash: &hash,
                via: actor.via,
            },
        };
        match self
            .store()
            .update_workflow(actor.user, id, update)
            .await
            .map_err(internal)?
        {
            Versioned::Done(summary) => Ok(saved(summary, &hash, workflow, merged)),
            Versioned::Conflict(current) => Err(ApiError::Conflict {
                message: format!(
                    "the workflow was changed after version {expected_version}; it is now version {current}, so load it again and reapply the edit"
                ),
                current_version: Some(current),
            }),
            Versioned::Missing => Err(not_found(id)),
        }
    }

    pub async fn watch_workflow(&self, actor: &Actor, id: Uuid) -> Result<WorkflowWatch> {
        actor.require(Scope::Read)?;
        let notifications = self.inner.hub.subscribe();
        self.stored_workflow(actor, id).await?;
        Ok(WorkflowWatch {
            service: self.clone(),
            actor: *actor,
            id,
            version: None,
            notifications,
            done: false,
        })
    }

    pub async fn delete_workflow(&self, actor: &Actor, id: Uuid) -> Result<()> {
        actor.require(Scope::Write)?;
        if self
            .store()
            .delete_workflow(actor.user, id)
            .await
            .map_err(internal)?
        {
            Ok(())
        } else {
            Err(not_found(id))
        }
    }

    pub async fn review_workflow(&self, actor: &Actor, id: Uuid) -> Result<Review> {
        actor.require(Scope::Read)?;
        let stored = self.stored_workflow(actor, id).await?;
        self.review(&stored, &parse_stored(&stored)?).await
    }

    pub async fn approve_workflow(&self, actor: &Actor, id: Uuid, hash: &str) -> Result<Review> {
        actor.require(Scope::Write)?;
        if actor.via != Via::Ui {
            return Err(ApiError::bad_request(
                "only a person can approve a workflow, from the Parameters app",
            ));
        }
        let stored = self.stored_workflow(actor, id).await?;
        if hex(&stored.hash) != hash {
            return Err(ApiError::Conflict {
                message: "the workflow changed after it was reviewed; review it again".into(),
                current_version: Some(stored.summary.version),
            });
        }
        self.store()
            .approve(id, &stored.hash, actor.user)
            .await
            .map_err(internal)?;
        self.review(&stored, &parse_stored(&stored)?).await
    }

    pub(crate) async fn review(
        &self,
        stored: &StoredWorkflow,
        workflow: &Workflow,
    ) -> Result<Review> {
        let findings = inspect(workflow, self.registry());
        let approved = findings.privileged.is_empty()
            || self
                .store()
                .is_approved(stored.summary.id, &stored.hash)
                .await
                .map_err(internal)?;
        Ok(Review {
            workflow_id: stored.summary.id,
            hash: hex(&stored.hash),
            approved,
            privileged: findings.privileged,
            hosts: findings.hosts,
            dynamic_urls: findings.dynamic_urls,
            secrets: findings.secrets,
        })
    }

    pub(crate) async fn stored_workflow(&self, actor: &Actor, id: Uuid) -> Result<StoredWorkflow> {
        self.store()
            .get_workflow(actor.user, id)
            .await
            .map_err(internal)?
            .ok_or_else(|| not_found(id))
    }

    async fn prepare(&self, actor: &Actor, source: &Source) -> Result<Prepared> {
        let library = self.library(actor.user).await?.workflow.definitions;
        let declared = source::declared(source);
        let mut workflow = source::load(
            source,
            &WithLibrary::new(self.registry(), &library, &declared),
        )?;
        let merged = include_library(&mut workflow, &library, self.registry())?;
        if !merged.items.is_empty() {
            let problems: Vec<Problem> = validate(&workflow, self.registry())
                .iter()
                .filter(|diagnostic| diagnostic.is_error())
                .map(Problem::from)
                .collect();
            if !problems.is_empty() {
                return Err(source::invalid(problems));
            }
        }
        complete_layouts(&mut workflow);
        let document = to_json(&workflow);
        let hash = *workflow.content_hash().as_bytes();
        Ok(Prepared {
            workflow,
            document,
            hash,
            merged,
        })
    }
}

pub struct WorkflowWatch {
    service: Service,
    actor: Actor,
    id: Uuid,
    version: Option<i32>,
    notifications: broadcast::Receiver<Notification>,
    done: bool,
}

impl WorkflowWatch {
    pub async fn next(&mut self) -> Option<Result<WorkflowRecord>> {
        while !self.done {
            if self.version.is_some() {
                changed(
                    &mut self.notifications,
                    Notification::WorkflowChanged(self.id),
                )
                .await;
            }
            match self.service.get_workflow(&self.actor, self.id).await {
                Ok(record) if Some(record.summary.version) != self.version => {
                    self.version = Some(record.summary.version);
                    return Some(Ok(record));
                }
                Ok(_) => {}
                Err(ApiError::NotFound { .. }) if self.version.is_some() => self.done = true,
                Err(error) => {
                    self.done = true;
                    return Some(Err(error));
                }
            }
        }
        None
    }
}

struct Prepared {
    workflow: Workflow,
    document: String,
    hash: [u8; 32],
    merged: MergeOutcome,
}

fn saved(
    summary: WorkflowSummary,
    hash: &[u8; 32],
    document: Workflow,
    merged: MergeOutcome,
) -> SavedWorkflow {
    SavedWorkflow {
        record: WorkflowRecord {
            summary,
            hash: hex(hash),
            document,
        },
        added_nodes: merged.items.into_iter().map(|item| item.name).collect(),
        renamed_nodes: merged.renames,
    }
}

pub(crate) fn parse_stored(stored: &StoredWorkflow) -> Result<Workflow> {
    from_json(&stored.document).map_err(internal)
}

fn record(stored: StoredWorkflow) -> Result<WorkflowRecord> {
    let document = parse_stored(&stored)?;
    Ok(WorkflowRecord {
        hash: hex(&stored.hash),
        summary: stored.summary,
        document,
    })
}

fn not_found(id: Uuid) -> ApiError {
    ApiError::not_found(format!("there is no workflow with the id {id}"))
}
