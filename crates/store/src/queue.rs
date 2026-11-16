use std::collections::HashMap;
use std::time::Duration;

use parameters_api::{RunState, StepState};
use serde_json::Value;
use uuid::Uuid;

use crate::notify::{Notification, send};
use crate::{Result, Store, named};

const RESTARTED: &str = "the server stopped during the run, so the run starts again";
const ABANDONED: &str = "the server stopped during the run too many times";

#[derive(Debug, Clone)]
pub struct ClaimedRun {
    pub id: Uuid,
    pub owner: Uuid,
    pub snapshot: String,
    pub hash: Vec<u8>,
    pub inputs: Value,
    pub attempts: i32,
    pub cancel_requested: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StepWrite {
    pub path: String,
    pub status: StepState,
    pub attempt: i32,
    pub message: Option<String>,
    pub outputs: Value,
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogWrite {
    pub path: String,
    pub message: String,
}

impl Store {
    pub async fn claim_run(&self, worker: Uuid) -> Result<Option<ClaimedRun>> {
        let mut transaction = self.pool.begin().await?;
        let claimed = sqlx::query_as!(
            ClaimedRun,
            r#"UPDATE runs
               SET status = 'running', claimed_by = $1, heartbeat_at = now(),
                   attempts = attempts + 1, started_at = coalesce(started_at, now())
               WHERE id = (
                   SELECT id FROM runs
                   WHERE status = 'queued'
                   ORDER BY created_at, id
                   FOR UPDATE SKIP LOCKED
                   LIMIT 1
               )
               RETURNING id, owner_id AS owner, snapshot::text AS "snapshot!",
                         document_hash AS hash, inputs, attempts,
                         cancel_requested"#,
            worker
        )
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some(run) = &claimed {
            send(&mut *transaction, Notification::RunChanged(run.id)).await?;
        }
        transaction.commit().await?;
        Ok(claimed)
    }

    pub async fn heartbeat(&self, run: Uuid, worker: Uuid) -> Result<Option<bool>> {
        Ok(sqlx::query_scalar!(
            r#"UPDATE runs SET heartbeat_at = now()
               WHERE id = $1 AND claimed_by = $2 AND status = 'running'
               RETURNING cancel_requested"#,
            run,
            worker
        )
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn write_events(
        &self,
        run: Uuid,
        worker: Uuid,
        steps: Vec<StepWrite>,
        logs: Vec<LogWrite>,
    ) -> Result<bool> {
        let mut latest: HashMap<String, StepWrite> = HashMap::with_capacity(steps.len());
        for step in steps {
            latest.insert(step.path.clone(), step);
        }
        let mut transaction = self.pool.begin().await?;
        let owned = sqlx::query_scalar!(
            r#"SELECT EXISTS (
                   SELECT 1 FROM runs WHERE id = $1 AND claimed_by = $2 AND status = 'running'
               ) AS "owned!""#,
            run,
            worker
        )
        .fetch_one(&mut *transaction)
        .await?;
        if !owned {
            return Ok(false);
        }
        if !latest.is_empty() {
            let mut paths = Vec::with_capacity(latest.len());
            let mut statuses = Vec::with_capacity(latest.len());
            let mut attempts = Vec::with_capacity(latest.len());
            let mut messages = Vec::with_capacity(latest.len());
            let mut outputs = Vec::with_capacity(latest.len());
            let mut durations = Vec::with_capacity(latest.len());
            for step in latest.into_values() {
                paths.push(step.path);
                statuses.push(step.status.name().to_owned());
                attempts.push(step.attempt);
                messages.push(step.message);
                outputs.push(step.outputs);
                durations.push(step.duration_ms);
            }
            sqlx::query!(
                r#"INSERT INTO run_steps (run_id, path, status, attempt, message, outputs, duration_ms)
                   SELECT $1, * FROM UNNEST($2::text[], $3::text[], $4::int4[], $5::text[],
                                            $6::jsonb[], $7::int8[])
                   ON CONFLICT (run_id, path) DO UPDATE
                   SET status = excluded.status, attempt = excluded.attempt,
                       message = excluded.message, outputs = excluded.outputs,
                       duration_ms = excluded.duration_ms, seq = nextval('run_event_seq')"#,
                run,
                &paths,
                &statuses,
                &attempts,
                &messages as &[Option<String>],
                &outputs,
                &durations as &[Option<i64>]
            )
            .execute(&mut *transaction)
            .await?;
        }
        if !logs.is_empty() {
            let (paths, messages): (Vec<String>, Vec<String>) =
                logs.into_iter().map(|log| (log.path, log.message)).unzip();
            sqlx::query!(
                r#"INSERT INTO run_logs (run_id, path, message)
                   SELECT $1, * FROM UNNEST($2::text[], $3::text[])"#,
                run,
                &paths,
                &messages
            )
            .execute(&mut *transaction)
            .await?;
        }
        send(&mut *transaction, Notification::RunChanged(run)).await?;
        transaction.commit().await?;
        Ok(true)
    }

    pub async fn finish_run(
        &self,
        run: Uuid,
        worker: Uuid,
        status: RunState,
        outputs: Value,
        errors: Vec<String>,
    ) -> Result<bool> {
        let mut transaction = self.pool.begin().await?;
        let finished = sqlx::query!(
            r#"UPDATE runs
               SET status = $3, outputs = $4, errors = $5, finished_at = now(),
                   claimed_by = NULL, heartbeat_at = NULL
               WHERE id = $1 AND claimed_by = $2 AND status = 'running'"#,
            run,
            worker,
            status.name(),
            outputs,
            Value::from(errors)
        )
        .execute(&mut *transaction)
        .await?
        .rows_affected()
            > 0;
        if finished {
            send(&mut *transaction, Notification::RunChanged(run)).await?;
        }
        transaction.commit().await?;
        Ok(finished)
    }

    pub async fn release_runs(&self, worker: Uuid) -> Result<Vec<Uuid>> {
        let mut transaction = self.pool.begin().await?;
        let rows = sqlx::query!(
            r#"UPDATE runs
               SET status = CASE WHEN cancel_requested THEN 'cancelled' ELSE 'queued' END,
                   finished_at = CASE WHEN cancel_requested THEN now() END,
                   attempts = greatest(attempts - 1, 0),
                   claimed_by = NULL, heartbeat_at = NULL
               WHERE claimed_by = $1 AND status = 'running'
               RETURNING id, status"#,
            worker
        )
        .fetch_all(&mut *transaction)
        .await?;
        let released = settle(
            &mut transaction,
            rows.into_iter().map(|row| (row.id, row.status)),
        )
        .await?;
        transaction.commit().await?;
        Ok(released.into_iter().map(|(id, _)| id).collect())
    }

    pub async fn recover_stale_runs(
        &self,
        stale_after: Duration,
        max_attempts: i32,
    ) -> Result<Vec<(Uuid, RunState)>> {
        let mut transaction = self.pool.begin().await?;
        let rows = sqlx::query!(
            r#"WITH stale AS (
                   SELECT id FROM runs
                   WHERE status = 'running' AND heartbeat_at < now() - make_interval(secs => $1)
                   FOR UPDATE SKIP LOCKED
               )
               UPDATE runs
               SET status = CASE
                       WHEN runs.cancel_requested THEN 'cancelled'
                       WHEN runs.attempts >= $2 THEN 'failed'
                       ELSE 'queued'
                   END,
                   finished_at = CASE
                       WHEN runs.cancel_requested OR runs.attempts >= $2 THEN now()
                   END,
                   errors = CASE
                       WHEN runs.attempts >= $2 AND NOT runs.cancel_requested
                       THEN jsonb_build_array($3::text)
                       ELSE runs.errors
                   END,
                   claimed_by = NULL, heartbeat_at = NULL
               FROM stale
               WHERE runs.id = stale.id
               RETURNING runs.id, runs.status"#,
            stale_after.as_secs_f64(),
            max_attempts,
            ABANDONED
        )
        .fetch_all(&mut *transaction)
        .await?;
        let recovered = settle(
            &mut transaction,
            rows.into_iter().map(|row| (row.id, row.status)),
        )
        .await?;
        transaction.commit().await?;
        Ok(recovered)
    }
}

async fn settle(
    transaction: &mut sqlx::PgTransaction<'_>,
    rows: impl Iterator<Item = (Uuid, String)>,
) -> Result<Vec<(Uuid, RunState)>> {
    let mut settled = Vec::new();
    let mut restarted = Vec::new();
    for (id, status) in rows {
        let status = named(&status, RunState::from_name)?;
        if status == RunState::Queued {
            restarted.push(id);
        }
        send(&mut **transaction, Notification::RunChanged(id)).await?;
        settled.push((id, status));
    }
    restart(transaction, &restarted).await?;
    Ok(settled)
}

async fn restart(transaction: &mut sqlx::PgTransaction<'_>, runs: &[Uuid]) -> Result<()> {
    if runs.is_empty() {
        return Ok(());
    }
    sqlx::query!("DELETE FROM run_steps WHERE run_id = ANY($1)", runs)
        .execute(&mut **transaction)
        .await?;
    sqlx::query!(
        r#"INSERT INTO run_logs (run_id, path, message)
           SELECT run_id, '', $2 FROM UNNEST($1::uuid[]) AS run_id"#,
        runs,
        RESTARTED
    )
    .execute(&mut **transaction)
    .await?;
    send(&mut **transaction, Notification::Queued).await?;
    Ok(())
}
