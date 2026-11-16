use parameters_api::{Via, WorkflowSummary};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::notify::{Notification, send};
use crate::{Cursor, Result, Store, Versioned, named};

pub struct NewWorkflow<'a> {
    pub name: &'a str,
    pub document: &'a str,
    pub hash: &'a [u8; 32],
    pub via: Via,
}

pub struct WorkflowUpdate<'a> {
    pub expected_version: i32,
    pub workflow: NewWorkflow<'a>,
}

#[derive(Debug, Clone)]
pub struct StoredWorkflow {
    pub summary: WorkflowSummary,
    pub document: String,
    pub hash: Vec<u8>,
}

struct Row {
    id: Uuid,
    name: String,
    version: i32,
    updated_via: String,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

impl Row {
    fn summary(self) -> Result<WorkflowSummary> {
        Ok(WorkflowSummary {
            id: self.id,
            name: self.name,
            version: self.version,
            updated_via: named(&self.updated_via, Via::from_name)?,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

impl Store {
    pub async fn list_workflows(
        &self,
        owner: Uuid,
        limit: i64,
        after: Option<Cursor>,
    ) -> Result<Vec<WorkflowSummary>> {
        sqlx::query_as!(
            Row,
            r#"SELECT id, name, version, updated_via, created_at, updated_at
               FROM workflows
               WHERE owner_id = $1
                 AND ($2::timestamptz IS NULL OR (updated_at, id) < ($2, $3))
               ORDER BY updated_at DESC, id DESC
               LIMIT $4"#,
            owner,
            after.map(|cursor| cursor.at),
            after.map(|cursor| cursor.id),
            limit
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(Row::summary)
        .collect()
    }

    pub async fn get_workflow(&self, owner: Uuid, id: Uuid) -> Result<Option<StoredWorkflow>> {
        let Some(row) = sqlx::query!(
            r#"SELECT id, name, version, updated_via, created_at, updated_at,
                      document::text AS "document!", document_hash
               FROM workflows
               WHERE owner_id = $1 AND id = $2"#,
            owner,
            id
        )
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };
        Ok(Some(StoredWorkflow {
            summary: Row {
                id: row.id,
                name: row.name,
                version: row.version,
                updated_via: row.updated_via,
                created_at: row.created_at,
                updated_at: row.updated_at,
            }
            .summary()?,
            document: row.document,
            hash: row.document_hash,
        }))
    }

    pub async fn insert_workflow(
        &self,
        owner: Uuid,
        workflow: NewWorkflow<'_>,
    ) -> Result<WorkflowSummary> {
        sqlx::query_as!(
            Row,
            r#"INSERT INTO workflows (owner_id, name, document, document_hash, updated_via)
               VALUES ($1, $2, $3::text::json, $4, $5)
               RETURNING id, name, version, updated_via, created_at, updated_at"#,
            owner,
            workflow.name,
            workflow.document,
            workflow.hash.as_slice(),
            workflow.via.name()
        )
        .fetch_one(&self.pool)
        .await?
        .summary()
    }

    pub async fn update_workflow(
        &self,
        owner: Uuid,
        id: Uuid,
        update: WorkflowUpdate<'_>,
    ) -> Result<Versioned<WorkflowSummary>> {
        let workflow = update.workflow;
        let mut transaction = self.pool.begin().await?;
        let updated = sqlx::query_as!(
            Row,
            r#"UPDATE workflows
               SET name = $4, document = $5::text::json, document_hash = $6, updated_via = $7,
                   version = version + 1, updated_at = now()
               WHERE owner_id = $1 AND id = $2 AND version = $3
               RETURNING id, name, version, updated_via, created_at, updated_at"#,
            owner,
            id,
            update.expected_version,
            workflow.name,
            workflow.document,
            workflow.hash.as_slice(),
            workflow.via.name()
        )
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some(row) = updated {
            send(&mut *transaction, Notification::WorkflowChanged(id)).await?;
            transaction.commit().await?;
            return Ok(Versioned::Done(row.summary()?));
        }
        transaction.rollback().await?;
        let current = sqlx::query_scalar!(
            "SELECT version FROM workflows WHERE owner_id = $1 AND id = $2",
            owner,
            id
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(current.map_or(Versioned::Missing, Versioned::Conflict))
    }

    pub async fn delete_workflow(&self, owner: Uuid, id: Uuid) -> Result<bool> {
        let mut transaction = self.pool.begin().await?;
        let deleted = sqlx::query!(
            "DELETE FROM workflows WHERE owner_id = $1 AND id = $2",
            owner,
            id
        )
        .execute(&mut *transaction)
        .await?
        .rows_affected()
            > 0;
        if deleted {
            send(&mut *transaction, Notification::WorkflowChanged(id)).await?;
        }
        transaction.commit().await?;
        Ok(deleted)
    }

    pub async fn workflows_using_node(
        &self,
        owner: Uuid,
        type_name: &str,
    ) -> Result<Vec<WorkflowSummary>> {
        sqlx::query_as!(
            Row,
            r#"SELECT id, name, version, updated_via, created_at, updated_at
               FROM workflows
               WHERE owner_id = $1 AND jsonb_exists(document::jsonb -> 'definitions', $2)
               ORDER BY updated_at DESC, id DESC"#,
            owner,
            type_name
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(Row::summary)
        .collect()
    }

    pub async fn is_approved(&self, workflow: Uuid, hash: &[u8]) -> Result<bool> {
        Ok(sqlx::query_scalar!(
            r#"SELECT EXISTS (
                   SELECT 1 FROM approvals WHERE workflow_id = $1 AND document_hash = $2
               ) AS "approved!""#,
            workflow,
            hash
        )
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn approve(&self, workflow: Uuid, hash: &[u8], by: Uuid) -> Result<()> {
        sqlx::query!(
            r#"INSERT INTO approvals (workflow_id, document_hash, approved_by)
               VALUES ($1, $2, $3)
               ON CONFLICT (workflow_id, document_hash) DO NOTHING"#,
            workflow,
            hash,
            by
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
