use parameters_api::{LogLine, RunRecord, RunState, RunSummary, StepRecord, StepState, Via};
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::notify::{Notification, send};
use crate::{Cursor, Result, Store, StoreError, hex, named, object};

pub struct NewRun<'a> {
    pub workflow_id: Uuid,
    pub workflow_name: &'a str,
    pub snapshot: &'a str,
    pub hash: &'a [u8; 32],
    pub inputs: Value,
    pub trigger: Via,
}

#[derive(Debug, Clone, Default)]
pub struct RunChanges {
    pub steps: Vec<StepRecord>,
    pub logs: Vec<LogLine>,
    pub cursor: i64,
    pub more: bool,
}

struct SummaryRow {
    id: Uuid,
    workflow_id: Option<Uuid>,
    workflow_name: String,
    status: String,
    trigger: String,
    attempts: i32,
    created_at: OffsetDateTime,
    started_at: Option<OffsetDateTime>,
    finished_at: Option<OffsetDateTime>,
}

impl SummaryRow {
    fn summary(self) -> Result<RunSummary> {
        Ok(RunSummary {
            id: self.id,
            workflow_id: self.workflow_id,
            workflow_name: self.workflow_name,
            status: named(&self.status, RunState::from_name)?,
            trigger: named(&self.trigger, Via::from_name)?,
            attempts: self.attempts,
            created_at: self.created_at,
            started_at: self.started_at,
            finished_at: self.finished_at,
        })
    }
}

impl Store {
    pub async fn insert_run(&self, owner: Uuid, run: NewRun<'_>) -> Result<RunSummary> {
        let mut transaction = self.pool.begin().await?;
        let summary = sqlx::query_as!(
            SummaryRow,
            r#"INSERT INTO runs (owner_id, workflow_id, workflow_name, snapshot, document_hash,
                                 inputs, trigger)
               VALUES ($1, $2, $3, $4::text::json, $5, $6, $7)
               RETURNING id, workflow_id, workflow_name, status, trigger, attempts, created_at,
                         started_at, finished_at"#,
            owner,
            run.workflow_id,
            run.workflow_name,
            run.snapshot,
            run.hash.as_slice(),
            run.inputs,
            run.trigger.name()
        )
        .fetch_one(&mut *transaction)
        .await?
        .summary()?;
        send(&mut *transaction, Notification::Queued).await?;
        transaction.commit().await?;
        Ok(summary)
    }

    pub async fn list_runs(
        &self,
        owner: Uuid,
        workflow: Option<Uuid>,
        limit: i64,
        after: Option<Cursor>,
    ) -> Result<Vec<RunSummary>> {
        sqlx::query_as!(
            SummaryRow,
            r#"SELECT id, workflow_id, workflow_name, status, trigger, attempts, created_at,
                      started_at, finished_at
               FROM runs
               WHERE owner_id = $1
                 AND ($2::uuid IS NULL OR workflow_id = $2)
                 AND ($3::timestamptz IS NULL OR (created_at, id) < ($3, $4))
               ORDER BY created_at DESC, id DESC
               LIMIT $5"#,
            owner,
            workflow,
            after.map(|cursor| cursor.at),
            after.map(|cursor| cursor.id),
            limit
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(SummaryRow::summary)
        .collect()
    }

    pub async fn get_run(&self, owner: Uuid, id: Uuid) -> Result<Option<RunRecord>> {
        let Some(row) = sqlx::query!(
            r#"SELECT id, workflow_id, workflow_name, status, trigger, attempts, created_at,
                      started_at, finished_at, document_hash, inputs, outputs, errors,
                      cancel_requested
               FROM runs
               WHERE owner_id = $1 AND id = $2"#,
            owner,
            id
        )
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };
        let errors = serde_json::from_value(row.errors)
            .map_err(|error| StoreError::Corrupt(error.to_string()))?;
        Ok(Some(RunRecord {
            summary: SummaryRow {
                id: row.id,
                workflow_id: row.workflow_id,
                workflow_name: row.workflow_name,
                status: row.status,
                trigger: row.trigger,
                attempts: row.attempts,
                created_at: row.created_at,
                started_at: row.started_at,
                finished_at: row.finished_at,
            }
            .summary()?,
            hash: hex(&row.document_hash),
            inputs: object(row.inputs),
            outputs: object(row.outputs),
            errors,
            cancel_requested: row.cancel_requested,
        }))
    }

    pub async fn run_changes(&self, run: Uuid, after: i64, log_limit: i64) -> Result<RunChanges> {
        let logs = sqlx::query_as!(
            LogLine,
            r#"SELECT seq, path, at, message
               FROM run_logs
               WHERE run_id = $1 AND seq > $2
               ORDER BY seq
               LIMIT $3"#,
            run,
            after,
            log_limit
        )
        .fetch_all(&self.pool)
        .await?;
        let truncated = i64::try_from(logs.len()).unwrap_or(i64::MAX) >= log_limit;
        let upper = if truncated {
            logs.last().map_or(i64::MAX, |log| log.seq)
        } else {
            i64::MAX
        };
        let rows = sqlx::query!(
            r#"SELECT path, status, attempt, message, outputs, duration_ms, seq
               FROM run_steps
               WHERE run_id = $1 AND seq > $2 AND seq <= $3
               ORDER BY seq"#,
            run,
            after,
            upper
        )
        .fetch_all(&self.pool)
        .await?;
        let mut cursor = logs.last().map_or(after, |log| log.seq);
        let mut steps = Vec::with_capacity(rows.len());
        for row in rows {
            cursor = cursor.max(row.seq);
            steps.push(StepRecord {
                path: row.path,
                status: named(&row.status, StepState::from_name)?,
                attempt: row.attempt,
                message: row.message,
                outputs: object(row.outputs),
                duration_ms: row.duration_ms,
            });
        }
        if truncated {
            cursor = upper;
        }
        Ok(RunChanges {
            steps,
            logs,
            cursor,
            more: truncated,
        })
    }

    pub async fn request_cancel(&self, owner: Uuid, id: Uuid) -> Result<Option<RunState>> {
        let mut transaction = self.pool.begin().await?;
        let Some(status) = sqlx::query_scalar!(
            r#"UPDATE runs
               SET cancel_requested = cancel_requested OR status IN ('queued', 'running'),
                   status = CASE WHEN status = 'queued' THEN 'cancelled' ELSE status END,
                   finished_at = CASE WHEN status = 'queued' THEN now() ELSE finished_at END
               WHERE owner_id = $1 AND id = $2
               RETURNING status"#,
            owner,
            id
        )
        .fetch_optional(&mut *transaction)
        .await?
        else {
            return Ok(None);
        };
        send(&mut *transaction, Notification::CancelRequested(id)).await?;
        send(&mut *transaction, Notification::RunChanged(id)).await?;
        transaction.commit().await?;
        Ok(Some(named(&status, RunState::from_name)?))
    }
}
