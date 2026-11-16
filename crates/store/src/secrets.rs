use parameters_api::SecretSummary;
use uuid::Uuid;

use crate::{Result, Store};

#[derive(Debug, Clone)]
pub struct EncryptedSecret {
    pub name: String,
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
}

impl Store {
    pub async fn list_secrets(&self, owner: Uuid) -> Result<Vec<SecretSummary>> {
        Ok(sqlx::query_as!(
            SecretSummary,
            "SELECT name, updated_at FROM secrets WHERE owner_id = $1 ORDER BY name",
            owner
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn put_secret(&self, owner: Uuid, secret: &EncryptedSecret) -> Result<()> {
        sqlx::query!(
            r#"INSERT INTO secrets (owner_id, name, nonce, ciphertext)
               VALUES ($1, $2, $3, $4)
               ON CONFLICT (owner_id, name)
               DO UPDATE SET nonce = excluded.nonce, ciphertext = excluded.ciphertext,
                             updated_at = now()"#,
            owner,
            secret.name,
            secret.nonce,
            secret.ciphertext
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_secret(&self, owner: Uuid, name: &str) -> Result<bool> {
        let result = sqlx::query!(
            "DELETE FROM secrets WHERE owner_id = $1 AND name = $2",
            owner,
            name
        )
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn get_secrets(&self, owner: Uuid, names: &[String]) -> Result<Vec<EncryptedSecret>> {
        Ok(sqlx::query_as!(
            EncryptedSecret,
            "SELECT name, nonce, ciphertext FROM secrets WHERE owner_id = $1 AND name = ANY($2)",
            owner,
            names
        )
        .fetch_all(&self.pool)
        .await?)
    }
}
