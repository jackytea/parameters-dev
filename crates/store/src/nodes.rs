use time::OffsetDateTime;
use uuid::Uuid;

use crate::{Result, Store};

#[derive(Debug, Clone)]
pub struct StoredNode {
    pub type_name: String,
    pub definition: String,
    pub hash: Vec<u8>,
    pub version: i32,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone)]
pub enum LibraryChange {
    Put {
        type_name: String,
        definition: String,
        hash: [u8; 32],
        expected_version: Option<i32>,
    },
    Delete {
        type_name: String,
        expected_version: i32,
    },
}

impl Store {
    pub async fn list_nodes(&self, owner: Uuid) -> Result<Vec<StoredNode>> {
        Ok(sqlx::query_as!(
            StoredNode,
            r#"SELECT type_name, definition::text AS "definition!", content_hash AS hash,
                      version, updated_at
               FROM node_definitions
               WHERE owner_id = $1
               ORDER BY type_name"#,
            owner
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn get_node(&self, owner: Uuid, type_name: &str) -> Result<Option<StoredNode>> {
        Ok(sqlx::query_as!(
            StoredNode,
            r#"SELECT type_name, definition::text AS "definition!", content_hash AS hash,
                      version, updated_at
               FROM node_definitions
               WHERE owner_id = $1 AND type_name = $2"#,
            owner,
            type_name
        )
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn change_library(&self, owner: Uuid, changes: &[LibraryChange]) -> Result<bool> {
        let mut transaction = self.pool.begin().await?;
        for change in changes {
            let affected = match change {
                LibraryChange::Put {
                    type_name,
                    definition,
                    hash,
                    expected_version: None,
                } => sqlx::query!(
                    r#"INSERT INTO node_definitions (owner_id, type_name, definition, content_hash)
                       VALUES ($1, $2, $3::text::json, $4)
                       ON CONFLICT (owner_id, type_name) DO NOTHING"#,
                    owner,
                    type_name,
                    definition,
                    hash.as_slice()
                )
                .execute(&mut *transaction)
                .await?
                .rows_affected(),
                LibraryChange::Put {
                    type_name,
                    definition,
                    hash,
                    expected_version: Some(version),
                } => sqlx::query!(
                    r#"UPDATE node_definitions
                       SET definition = $4::text::json, content_hash = $5,
                           version = version + 1, updated_at = now()
                       WHERE owner_id = $1 AND type_name = $2 AND version = $3"#,
                    owner,
                    type_name,
                    version,
                    definition,
                    hash.as_slice()
                )
                .execute(&mut *transaction)
                .await?
                .rows_affected(),
                LibraryChange::Delete {
                    type_name,
                    expected_version,
                } => sqlx::query!(
                    r#"DELETE FROM node_definitions
                       WHERE owner_id = $1 AND type_name = $2 AND version = $3"#,
                    owner,
                    type_name,
                    expected_version
                )
                .execute(&mut *transaction)
                .await?
                .rows_affected(),
            };
            if affected == 0 {
                transaction.rollback().await?;
                return Ok(false);
            }
        }
        transaction.commit().await?;
        Ok(true)
    }
}
