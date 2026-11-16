use std::time::Duration;

use parameters_api::{Scope, Scopes, TokenSummary, UserInfo};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{Result, Store};

pub struct NewToken<'a> {
    pub name: &'a str,
    pub hash: &'a [u8; 32],
    pub scopes: Scopes,
    pub expires_at: Option<OffsetDateTime>,
}

fn names(scopes: Scopes) -> Vec<String> {
    scopes
        .list()
        .into_iter()
        .map(|scope| scope.name().to_owned())
        .collect()
}

fn scopes(names: &[String]) -> Scopes {
    names
        .iter()
        .filter_map(|name| Scope::from_name(name))
        .collect()
}

fn seconds(duration: Duration) -> f64 {
    duration.as_secs_f64()
}

impl Store {
    pub async fn user(&self, id: Uuid) -> Result<Option<UserInfo>> {
        Ok(sqlx::query_as!(
            UserInfo,
            "SELECT id, email, display_name FROM users WHERE id = $1",
            id
        )
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn user_for_email(&self, email: &str) -> Result<Uuid> {
        Ok(sqlx::query_scalar!(
            r#"INSERT INTO users (email, display_name)
               VALUES ($1, split_part($1, '@', 1))
               ON CONFLICT ((lower(email))) DO UPDATE SET email = users.email
               RETURNING id"#,
            email
        )
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn user_for_identity(
        &self,
        provider: &str,
        provider_user_id: &str,
        verified_email: Option<&str>,
        display_name: &str,
    ) -> Result<Uuid> {
        let mut transaction = self.pool.begin().await?;
        let existing = sqlx::query_scalar!(
            "SELECT user_id FROM identities WHERE provider = $1 AND provider_user_id = $2",
            provider,
            provider_user_id
        )
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some(user) = existing {
            transaction.commit().await?;
            return Ok(user);
        }
        let user = match verified_email {
            Some(email) => {
                sqlx::query_scalar!(
                    r#"INSERT INTO users (email, display_name) VALUES ($1, $2)
                   ON CONFLICT ((lower(email))) DO UPDATE SET email = users.email
                   RETURNING id"#,
                    email,
                    display_name
                )
                .fetch_one(&mut *transaction)
                .await?
            }
            None => {
                sqlx::query_scalar!(
                    "INSERT INTO users (display_name) VALUES ($1) RETURNING id",
                    display_name
                )
                .fetch_one(&mut *transaction)
                .await?
            }
        };
        sqlx::query!(
            "INSERT INTO identities (provider, provider_user_id, user_id) VALUES ($1, $2, $3)",
            provider,
            provider_user_id,
            user
        )
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(user)
    }

    pub async fn add_login_token(&self, hash: &[u8; 32], email: &str, ttl: Duration) -> Result<()> {
        sqlx::query!(
            r#"INSERT INTO login_tokens (token_hash, email, expires_at)
               VALUES ($1, $2, now() + make_interval(secs => $3))"#,
            hash.as_slice(),
            email,
            seconds(ttl)
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn consume_login_token(&self, hash: &[u8; 32]) -> Result<Option<String>> {
        Ok(sqlx::query_scalar!(
            r#"UPDATE login_tokens SET consumed_at = now()
               WHERE token_hash = $1 AND consumed_at IS NULL AND expires_at > now()
               RETURNING email"#,
            hash.as_slice()
        )
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn add_session(&self, hash: &[u8; 32], user: Uuid, ttl: Duration) -> Result<()> {
        sqlx::query!(
            r#"INSERT INTO sessions (token_hash, user_id, expires_at)
               VALUES ($1, $2, now() + make_interval(secs => $3))"#,
            hash.as_slice(),
            user,
            seconds(ttl)
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn session_user(&self, hash: &[u8; 32], ttl: Duration) -> Result<Option<Uuid>> {
        Ok(sqlx::query_scalar!(
            r#"UPDATE sessions
               SET last_seen_at = now(), expires_at = now() + make_interval(secs => $2)
               WHERE token_hash = $1 AND expires_at > now()
               RETURNING user_id"#,
            hash.as_slice(),
            seconds(ttl)
        )
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn delete_session(&self, hash: &[u8; 32]) -> Result<()> {
        sqlx::query!(
            "DELETE FROM sessions WHERE token_hash = $1",
            hash.as_slice()
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn add_token(&self, user: Uuid, token: NewToken<'_>) -> Result<TokenSummary> {
        let row = sqlx::query!(
            r#"INSERT INTO api_tokens (user_id, name, token_hash, scopes, expires_at)
               VALUES ($1, $2, $3, $4, $5)
               RETURNING id, name, scopes, created_at, expires_at, last_used_at"#,
            user,
            token.name,
            token.hash.as_slice(),
            &names(token.scopes),
            token.expires_at
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(TokenSummary {
            id: row.id,
            name: row.name,
            scopes: scopes(&row.scopes),
            created_at: row.created_at,
            expires_at: row.expires_at,
            last_used_at: row.last_used_at,
        })
    }

    pub async fn tokens(&self, user: Uuid) -> Result<Vec<TokenSummary>> {
        Ok(sqlx::query!(
            r#"SELECT id, name, scopes, created_at, expires_at, last_used_at
               FROM api_tokens
               WHERE user_id = $1 AND revoked_at IS NULL
               ORDER BY created_at DESC"#,
            user
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| TokenSummary {
            id: row.id,
            name: row.name,
            scopes: scopes(&row.scopes),
            created_at: row.created_at,
            expires_at: row.expires_at,
            last_used_at: row.last_used_at,
        })
        .collect())
    }

    pub async fn revoke_token(&self, user: Uuid, id: Uuid) -> Result<bool> {
        Ok(sqlx::query!(
            r#"UPDATE api_tokens SET revoked_at = now()
               WHERE user_id = $1 AND id = $2 AND revoked_at IS NULL"#,
            user,
            id
        )
        .execute(&self.pool)
        .await?
        .rows_affected()
            > 0)
    }

    pub async fn token_user(&self, hash: &[u8; 32]) -> Result<Option<(Uuid, Scopes)>> {
        Ok(sqlx::query!(
            r#"UPDATE api_tokens SET last_used_at = now()
               WHERE token_hash = $1 AND revoked_at IS NULL
                 AND (expires_at IS NULL OR expires_at > now())
               RETURNING user_id, scopes"#,
            hash.as_slice()
        )
        .fetch_optional(&self.pool)
        .await?
        .map(|row| (row.user_id, scopes(&row.scopes))))
    }

    pub async fn active_runs(&self, owner: Uuid) -> Result<i64> {
        Ok(sqlx::query_scalar!(
            r#"SELECT count(*) AS "count!" FROM runs
               WHERE owner_id = $1 AND status IN ('queued', 'running')"#,
            owner
        )
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn prune_expired_credentials(&self) -> Result<u64> {
        let logins =
            sqlx::query!("DELETE FROM login_tokens WHERE expires_at < now() - interval '1 day'")
                .execute(&self.pool)
                .await?
                .rows_affected();
        let sessions = sqlx::query!("DELETE FROM sessions WHERE expires_at < now()")
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(logins + sessions)
    }
}
