use std::collections::HashMap;

use parameters_api::{ApiError, MAX_SECRET_BYTES, Scope, SecretSummary, check_secret_name};
use uuid::Uuid;

use crate::{Actor, Result, Service, internal};

impl Service {
    pub async fn list_secrets(&self, actor: &Actor) -> Result<Vec<SecretSummary>> {
        actor.require(Scope::Read)?;
        self.store()
            .list_secrets(actor.user)
            .await
            .map_err(internal)
    }

    pub async fn set_secret(&self, actor: &Actor, name: &str, value: &str) -> Result<()> {
        actor.require(Scope::Write)?;
        check_secret_name(name).map_err(ApiError::bad_request)?;
        if value.len() > MAX_SECRET_BYTES {
            return Err(ApiError::bad_request(format!(
                "a secret can be at most {MAX_SECRET_BYTES} bytes"
            )));
        }
        let sealed = self
            .inner
            .cipher
            .seal(actor.user, name, value)
            .map_err(internal)?;
        self.store()
            .put_secret(actor.user, &sealed)
            .await
            .map_err(internal)
    }

    pub async fn delete_secret(&self, actor: &Actor, name: &str) -> Result<()> {
        actor.require(Scope::Write)?;
        if self
            .store()
            .delete_secret(actor.user, name)
            .await
            .map_err(internal)?
        {
            Ok(())
        } else {
            Err(ApiError::not_found(format!(
                "there is no secret named `{name}`"
            )))
        }
    }

    pub(crate) async fn load_secrets(
        &self,
        owner: Uuid,
        names: &[String],
    ) -> Result<HashMap<String, String>, String> {
        if names.is_empty() {
            return Ok(HashMap::new());
        }
        let sealed = self
            .store()
            .get_secrets(owner, names)
            .await
            .map_err(|error| error.to_string())?;
        sealed
            .iter()
            .map(|secret| {
                self.inner
                    .cipher
                    .open(owner, secret)
                    .map(|value| (secret.name.clone(), value))
                    .map_err(|error| format!("the secret `{}`: {error}", secret.name))
            })
            .collect()
    }
}
