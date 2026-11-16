use parameters_api::{TriggerKind, TriggerSummary};
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{Result, Store, StoreError, object};

pub struct TriggerRecord<'a> {
    pub id: Uuid,
    pub workflow_id: Uuid,
    pub name: &'a str,
    pub inputs: Value,
    pub webhook: Option<WebhookSecret<'a>>,
    pub schedule: Option<(&'a str, OffsetDateTime)>,
}

pub struct WebhookSecret<'a> {
    pub token_hash: &'a [u8; 32],
    pub nonce: Option<Vec<u8>>,
    pub ciphertext: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct FiringTrigger {
    pub id: Uuid,
    pub owner: Uuid,
    pub workflow_id: Uuid,
    pub inputs: Value,
    pub nonce: Option<Vec<u8>>,
    pub ciphertext: Option<Vec<u8>>,
}

struct Row {
    id: Uuid,
    workflow_id: Uuid,
    name: String,
    kind: String,
    cron: Option<String>,
    next_run_at: Option<OffsetDateTime>,
    has_secret: bool,
    inputs: Value,
    enabled: bool,
    created_at: OffsetDateTime,
    last_fired_at: Option<OffsetDateTime>,
    last_error: Option<String>,
}

impl Row {
    fn summary(self) -> Result<TriggerSummary> {
        let kind = match (self.kind.as_str(), self.cron) {
            ("webhook", _) => TriggerKind::Webhook {
                signed: self.has_secret,
            },
            ("schedule", Some(cron)) => TriggerKind::Schedule {
                cron,
                next_run_at: self.next_run_at,
            },
            (other, _) => return Err(StoreError::Corrupt(format!("trigger kind `{other}`"))),
        };
        Ok(TriggerSummary {
            id: self.id,
            workflow_id: self.workflow_id,
            name: self.name,
            kind,
            enabled: self.enabled,
            inputs: object(self.inputs),
            created_at: self.created_at,
            last_fired_at: self.last_fired_at,
            last_error: self.last_error,
        })
    }
}

impl Store {
    pub async fn add_trigger(
        &self,
        owner: Uuid,
        trigger: TriggerRecord<'_>,
    ) -> Result<TriggerSummary> {
        let (token_hash, nonce, ciphertext) = match trigger.webhook {
            Some(webhook) => (
                Some(webhook.token_hash.to_vec()),
                webhook.nonce,
                webhook.ciphertext,
            ),
            None => (None, None, None),
        };
        let (cron, next_run_at) = trigger
            .schedule
            .map_or((None, None), |(cron, next)| (Some(cron), Some(next)));
        sqlx::query_as!(
            Row,
            r#"INSERT INTO triggers (id, owner_id, workflow_id, name, kind, token_hash,
                                     secret_nonce, secret_ciphertext, cron, next_run_at, inputs)
               VALUES ($1, $2, $3, $4, CASE WHEN $5::bytea IS NULL THEN 'schedule' ELSE 'webhook' END,
                       $5, $6, $7, $8, $9, $10)
               RETURNING id, workflow_id, name, kind, cron, next_run_at,
                         secret_ciphertext IS NOT NULL AS "has_secret!", inputs, enabled,
                         created_at, last_fired_at, last_error"#,
            trigger.id,
            owner,
            trigger.workflow_id,
            trigger.name,
            token_hash,
            nonce,
            ciphertext,
            cron,
            next_run_at,
            trigger.inputs
        )
        .fetch_one(&self.pool)
        .await?
        .summary()
    }

    pub async fn triggers(
        &self,
        owner: Uuid,
        workflow: Option<Uuid>,
    ) -> Result<Vec<TriggerSummary>> {
        sqlx::query_as!(
            Row,
            r#"SELECT id, workflow_id, name, kind, cron, next_run_at,
                      secret_ciphertext IS NOT NULL AS "has_secret!", inputs, enabled,
                      created_at, last_fired_at, last_error
               FROM triggers
               WHERE owner_id = $1 AND ($2::uuid IS NULL OR workflow_id = $2)
               ORDER BY created_at"#,
            owner,
            workflow
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(Row::summary)
        .collect()
    }

    pub async fn set_trigger_enabled(
        &self,
        owner: Uuid,
        id: Uuid,
        enabled: bool,
        next_run_at: Option<OffsetDateTime>,
    ) -> Result<bool> {
        Ok(sqlx::query!(
            r#"UPDATE triggers
               SET enabled = $3,
                   next_run_at = CASE WHEN kind = 'schedule' THEN $4 ELSE next_run_at END
               WHERE owner_id = $1 AND id = $2"#,
            owner,
            id,
            enabled,
            next_run_at
        )
        .execute(&self.pool)
        .await?
        .rows_affected()
            > 0)
    }

    pub async fn trigger_cron(&self, owner: Uuid, id: Uuid) -> Result<Option<Option<String>>> {
        Ok(sqlx::query_scalar!(
            "SELECT cron FROM triggers WHERE owner_id = $1 AND id = $2",
            owner,
            id
        )
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn delete_trigger(&self, owner: Uuid, id: Uuid) -> Result<bool> {
        Ok(sqlx::query!(
            "DELETE FROM triggers WHERE owner_id = $1 AND id = $2",
            owner,
            id
        )
        .execute(&self.pool)
        .await?
        .rows_affected()
            > 0)
    }

    pub async fn webhook(&self, token_hash: &[u8; 32]) -> Result<Option<FiringTrigger>> {
        Ok(sqlx::query_as!(
            FiringTrigger,
            r#"SELECT id, owner_id AS owner, workflow_id, inputs,
                      secret_nonce AS nonce, secret_ciphertext AS ciphertext
               FROM triggers
               WHERE token_hash = $1 AND kind = 'webhook' AND enabled"#,
            token_hash.as_slice()
        )
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn claim_due_schedules(
        &self,
        limit: i64,
        next: impl Fn(&str) -> Option<OffsetDateTime>,
    ) -> Result<Vec<FiringTrigger>> {
        let mut transaction = self.pool.begin().await?;
        let due = sqlx::query!(
            r#"SELECT id, owner_id, workflow_id, inputs, cron AS "cron!"
               FROM triggers
               WHERE kind = 'schedule' AND enabled AND next_run_at <= now()
               ORDER BY next_run_at
               FOR UPDATE SKIP LOCKED
               LIMIT $1"#,
            limit
        )
        .fetch_all(&mut *transaction)
        .await?;
        let mut claimed = Vec::with_capacity(due.len());
        for row in due {
            sqlx::query!(
                r#"UPDATE triggers SET next_run_at = $2, last_fired_at = now(),
                       enabled = enabled AND $2::timestamptz IS NOT NULL
                   WHERE id = $1"#,
                row.id,
                next(&row.cron)
            )
            .execute(&mut *transaction)
            .await?;
            claimed.push(FiringTrigger {
                id: row.id,
                owner: row.owner_id,
                workflow_id: row.workflow_id,
                inputs: row.inputs,
                nonce: None,
                ciphertext: None,
            });
        }
        transaction.commit().await?;
        Ok(claimed)
    }

    pub async fn record_firing(&self, id: Uuid, error: Option<&str>) -> Result<()> {
        sqlx::query!(
            "UPDATE triggers SET last_fired_at = now(), last_error = $2 WHERE id = $1",
            id,
            error
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn next_schedule_at(&self) -> Result<Option<OffsetDateTime>> {
        Ok(sqlx::query_scalar!(
            "SELECT min(next_run_at) FROM triggers WHERE kind = 'schedule' AND enabled"
        )
        .fetch_one(&self.pool)
        .await?)
    }
}
