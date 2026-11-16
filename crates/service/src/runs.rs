use parameters_api::{ApiError, Page, RunState, RunSummary, RunUpdate};
use parameters_store::{Cursor, NewRun, Notification};
use serde_json::{Map, Value};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::hub::changed;
use crate::page::PageRequest;
use crate::workflows::parse_stored;
use crate::{Actor, Result, Service, internal};

const LOG_PAGE: i64 = 1_000;

impl Service {
    pub async fn start_run(
        &self,
        actor: &Actor,
        workflow_id: Uuid,
        inputs: Map<String, Value>,
    ) -> Result<RunSummary> {
        let stored = self.stored_workflow(actor, workflow_id).await?;
        let workflow = parse_stored(&stored)?;
        let review = self.review(&stored, &workflow).await?;
        if review.needs_approval() {
            return Err(ApiError::ApprovalRequired {
                message: "this workflow runs commands or uses files, so a person must review and approve this version in the Parameters app before it can run".into(),
                review: Box::new(review),
            });
        }
        let plan = self
            .inner
            .plans
            .get_or_compile(&stored.hash, &workflow, &self.inner.engine)
            .map_err(|error| ApiError::bad_request(error.to_string()))?;
        let errors = plan.check_inputs(&inputs);
        if !errors.is_empty() {
            return Err(ApiError::bad_request(errors.join("; ")));
        }
        let hash: [u8; 32] = stored.hash.as_slice().try_into().map_err(internal)?;
        self.store()
            .insert_run(
                actor.user,
                NewRun {
                    workflow_id,
                    workflow_name: &workflow.name,
                    snapshot: &stored.document,
                    hash: &hash,
                    inputs: Value::Object(inputs),
                    trigger: actor.via,
                },
            )
            .await
            .map_err(internal)
    }

    pub async fn list_runs(
        &self,
        actor: &Actor,
        workflow: Option<Uuid>,
        size: Option<u32>,
        after: Option<&str>,
    ) -> Result<Page<RunSummary>> {
        let request = PageRequest::new(size, after)?;
        let items = self
            .store()
            .list_runs(actor.user, workflow, request.fetch_limit(), request.after)
            .await
            .map_err(internal)?;
        Ok(request.page(items, |item| Cursor {
            at: item.created_at,
            id: item.id,
        }))
    }

    pub async fn get_run(&self, actor: &Actor, id: Uuid) -> Result<RunUpdate> {
        Ok(self.run_update(actor.user, id, 0).await?.0)
    }

    pub async fn cancel_run(&self, actor: &Actor, id: Uuid) -> Result<RunState> {
        self.store()
            .request_cancel(actor.user, id)
            .await
            .map_err(internal)?
            .ok_or_else(|| run_not_found(id))
    }

    pub async fn watch_run(&self, actor: &Actor, id: Uuid) -> Result<RunWatch> {
        let notifications = self.inner.hub.subscribe();
        self.store()
            .get_run(actor.user, id)
            .await
            .map_err(internal)?
            .ok_or_else(|| run_not_found(id))?;
        Ok(RunWatch {
            service: self.clone(),
            owner: actor.user,
            id,
            cursor: 0,
            notifications,
            state: WatchState::Fetch,
        })
    }

    async fn run_update(&self, owner: Uuid, id: Uuid, after: i64) -> Result<(RunUpdate, bool)> {
        let run = self
            .store()
            .get_run(owner, id)
            .await
            .map_err(internal)?
            .ok_or_else(|| run_not_found(id))?;
        let changes = self
            .store()
            .run_changes(id, after, LOG_PAGE)
            .await
            .map_err(internal)?;
        Ok((
            RunUpdate {
                run,
                steps: changes.steps,
                logs: changes.logs,
                cursor: changes.cursor,
            },
            changes.more,
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatchState {
    Fetch,
    Wait,
    Done,
}

pub struct RunWatch {
    service: Service,
    owner: Uuid,
    id: Uuid,
    cursor: i64,
    notifications: broadcast::Receiver<Notification>,
    state: WatchState,
}

impl RunWatch {
    pub async fn next(&mut self) -> Option<Result<RunUpdate>> {
        match self.state {
            WatchState::Done => return None,
            WatchState::Wait => self.wait().await,
            WatchState::Fetch => {}
        }
        match self
            .service
            .run_update(self.owner, self.id, self.cursor)
            .await
        {
            Ok((update, more)) => {
                self.cursor = update.cursor;
                self.state = if more {
                    WatchState::Fetch
                } else if update.run.summary.status.is_finished() {
                    WatchState::Done
                } else {
                    WatchState::Wait
                };
                Some(Ok(update))
            }
            Err(error) => {
                self.state = WatchState::Done;
                Some(Err(error))
            }
        }
    }

    async fn wait(&mut self) {
        changed(&mut self.notifications, Notification::RunChanged(self.id)).await;
    }
}

fn run_not_found(id: Uuid) -> ApiError {
    ApiError::not_found(format!("there is no run with the id {id}"))
}
