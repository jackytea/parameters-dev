use std::time::Duration;

use parameters_api::{
    ApiError, CreatedTrigger, NewTrigger, RunSummary, Schedule, Scope, TriggerSummary, Uuid, Via,
};
use parameters_core::ValueType;
use parameters_store::{EncryptedSecret, FiringTrigger, TriggerRecord, WebhookSecret};
use ring::hmac;
use serde_json::{Map, Value};
use time::OffsetDateTime;

use crate::accounts::{base64url, hash};
use crate::workflows::parse_stored;
use crate::{Actor, Result, Service, internal};

pub const HOOK_PREFIX: &str = "whk_";
pub const PAYLOAD_INPUT: &str = "payload";
pub const MAX_PAYLOAD_BYTES: usize = 256 * 1024;
const MAX_NAME: usize = 100;
const WEBHOOK_LIMIT: u32 = 60;
const DUE_BATCH: i64 = 50;

fn secret_name(id: Uuid) -> String {
    format!("trigger:{id}")
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(text.get(index..index + 2)?, 16).ok())
        .collect()
}

fn next_run(cron: &str) -> Option<OffsetDateTime> {
    Schedule::parse(cron)
        .ok()?
        .next_after(OffsetDateTime::now_utc())
}

impl Service {
    pub async fn list_triggers(
        &self,
        actor: &Actor,
        workflow: Option<Uuid>,
    ) -> Result<Vec<TriggerSummary>> {
        actor.require(Scope::Read)?;
        self.store()
            .triggers(actor.user, workflow)
            .await
            .map_err(internal)
    }

    pub async fn create_trigger(
        &self,
        actor: &Actor,
        workflow_id: Uuid,
        name: &str,
        kind: NewTrigger,
        inputs: Map<String, Value>,
    ) -> Result<CreatedTrigger> {
        actor.require(Scope::Write)?;
        let stored = self.stored_workflow(actor, workflow_id).await?;
        let workflow = parse_stored(&stored)?;
        let name = name.trim();
        if name.is_empty() || name.len() > MAX_NAME {
            return Err(ApiError::bad_request(format!(
                "give the trigger a name of up to {MAX_NAME} characters"
            )));
        }
        check_inputs(&workflow, &inputs)?;
        let id = self.inner.cipher.random_uuid().map_err(internal)?;
        let random = || {
            self.inner
                .cipher
                .random_bytes()
                .map(|bytes| base64url(&bytes))
                .map_err(internal)
        };
        let record = |webhook, schedule| TriggerRecord {
            id,
            workflow_id,
            name,
            inputs: Value::Object(inputs.clone()),
            webhook,
            schedule,
        };
        match kind {
            NewTrigger::Webhook { signed } => {
                let token = format!("{HOOK_PREFIX}{}", random()?);
                let secret = if signed { Some(random()?) } else { None };
                let sealed = secret
                    .as_deref()
                    .map(|secret| self.inner.cipher.seal(actor.user, &secret_name(id), secret))
                    .transpose()
                    .map_err(internal)?;
                let token_hash = hash(&token);
                let (nonce, ciphertext) = sealed.map_or((None, None), |sealed| {
                    (Some(sealed.nonce), Some(sealed.ciphertext))
                });
                let summary = self
                    .store()
                    .add_trigger(
                        actor.user,
                        record(
                            Some(WebhookSecret {
                                token_hash: &token_hash,
                                nonce,
                                ciphertext,
                            }),
                            None,
                        ),
                    )
                    .await
                    .map_err(internal)?;
                Ok(CreatedTrigger {
                    summary,
                    hook_path: Some(format!("/hooks/{token}")),
                    signing_secret: secret,
                })
            }
            NewTrigger::Schedule { cron } => {
                let next = self.first_run(&cron)?;
                let summary = self
                    .store()
                    .add_trigger(actor.user, record(None, Some((cron.trim(), next))))
                    .await
                    .map_err(internal)?;
                Ok(CreatedTrigger {
                    summary,
                    hook_path: None,
                    signing_secret: None,
                })
            }
        }
    }

    fn first_run(&self, cron: &str) -> Result<OffsetDateTime> {
        let schedule = Schedule::parse(cron).map_err(ApiError::bad_request)?;
        let now = OffsetDateTime::now_utc();
        let next = schedule
            .next_after(now)
            .ok_or_else(|| ApiError::bad_request("this schedule never runs"))?;
        if let (Some(minimum), Some(gap)) = (
            self.inner.options.capabilities.min_schedule_gap,
            schedule.shortest_gap(now),
        ) && gap < minimum
        {
            return Err(ApiError::bad_request(format!(
                "schedules here can run at most every {} minutes",
                minimum.as_secs() / 60
            )));
        }
        Ok(next)
    }

    pub async fn set_trigger_enabled(&self, actor: &Actor, id: Uuid, enabled: bool) -> Result<()> {
        actor.require(Scope::Write)?;
        let cron = self
            .store()
            .trigger_cron(actor.user, id)
            .await
            .map_err(internal)?
            .ok_or_else(|| trigger_not_found(id))?;
        let next = cron.as_deref().and_then(next_run);
        self.store()
            .set_trigger_enabled(actor.user, id, enabled, next)
            .await
            .map_err(internal)?;
        Ok(())
    }

    pub async fn delete_trigger(&self, actor: &Actor, id: Uuid) -> Result<()> {
        actor.require(Scope::Write)?;
        if self
            .store()
            .delete_trigger(actor.user, id)
            .await
            .map_err(internal)?
        {
            Ok(())
        } else {
            Err(trigger_not_found(id))
        }
    }

    pub async fn fire_webhook(
        &self,
        token: &str,
        body: &[u8],
        signature: Option<&str>,
    ) -> Result<RunSummary> {
        let missing = || ApiError::not_found("there is no webhook at this address");
        if !token.starts_with(HOOK_PREFIX) {
            return Err(missing());
        }
        if body.len() > MAX_PAYLOAD_BYTES {
            return Err(ApiError::bad_request(format!(
                "the payload can be at most {MAX_PAYLOAD_BYTES} bytes"
            )));
        }
        let trigger = self
            .store()
            .webhook(&hash(token))
            .await
            .map_err(internal)?
            .ok_or_else(missing)?;
        if !self.inner.limiter.allow(
            &format!("webhook:{}", trigger.id),
            WEBHOOK_LIMIT,
            Duration::from_mins(1),
        ) {
            return Err(ApiError::TooManyRequests {
                message: "this webhook was called too often; slow down".into(),
            });
        }
        if let (Some(nonce), Some(ciphertext)) = (&trigger.nonce, &trigger.ciphertext) {
            let secret = self
                .inner
                .cipher
                .open(
                    trigger.owner,
                    &EncryptedSecret {
                        name: secret_name(trigger.id),
                        nonce: nonce.clone(),
                        ciphertext: ciphertext.clone(),
                    },
                )
                .map_err(internal)?;
            let expected = signature
                .and_then(|value| value.trim().strip_prefix("sha256="))
                .and_then(decode_hex);
            let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
            if expected.is_none_or(|tag| hmac::verify(&key, body, &tag).is_err()) {
                let error = ApiError::Unauthorized {
                    message: "the signature is missing or does not match".into(),
                };
                let _ = self
                    .store()
                    .record_firing(trigger.id, Some(&error.to_string()))
                    .await;
                return Err(error);
            }
        }
        let payload = serde_json::from_slice(body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(body).into_owned()));
        self.fire(&trigger, Via::Webhook, Some(payload)).await
    }

    pub async fn fire_due_schedules(&self) -> usize {
        let due = match self.store().claim_due_schedules(DUE_BATCH, next_run).await {
            Ok(due) => due,
            Err(error) => {
                tracing::error!(%error, "cannot claim due schedules");
                return 0;
            }
        };
        for trigger in &due {
            let _ = self.fire(trigger, Via::Schedule, None).await;
        }
        due.len()
    }

    async fn fire(
        &self,
        trigger: &FiringTrigger,
        via: Via,
        payload: Option<Value>,
    ) -> Result<RunSummary> {
        let actor = Actor {
            user: trigger.owner,
            via,
            scopes: Some([Scope::Read, Scope::Run].into_iter().collect()),
        };
        let mut inputs = match &trigger.inputs {
            Value::Object(inputs) => inputs.clone(),
            _ => Map::new(),
        };
        let started = async {
            if let Some(payload) = payload {
                let workflow =
                    parse_stored(&self.stored_workflow(&actor, trigger.workflow_id).await?)?;
                if workflow
                    .inputs
                    .iter()
                    .any(|input| input.name == PAYLOAD_INPUT)
                {
                    inputs.insert(PAYLOAD_INPUT.to_owned(), payload);
                }
            }
            self.start_run(&actor, trigger.workflow_id, inputs).await
        }
        .await;
        let error = started.as_ref().err().map(ToString::to_string);
        if let Err(failure) = self
            .store()
            .record_firing(trigger.id, error.as_deref())
            .await
        {
            tracing::error!(%failure, "cannot record a trigger firing");
        }
        started
    }
}

fn check_inputs(workflow: &parameters_core::Workflow, inputs: &Map<String, Value>) -> Result<()> {
    for given in inputs.keys() {
        let declared = workflow.inputs.iter().find(|input| &input.name == given);
        match declared {
            None => {
                return Err(ApiError::bad_request(format!(
                    "the workflow has no input `{given}`"
                )));
            }
            Some(input) if input.value_type == ValueType::Secret => {
                return Err(ApiError::bad_request(format!(
                    "`{given}` is a secret input; it is read from your secrets"
                )));
            }
            Some(_) => {}
        }
    }
    Ok(())
}

fn trigger_not_found(id: Uuid) -> ApiError {
    ApiError::not_found(format!("there is no trigger with the id {id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_hex_signatures() {
        assert_eq!(decode_hex("00ff10"), Some(vec![0, 255, 16]));
        assert_eq!(decode_hex("abc"), None);
        assert_eq!(decode_hex("zz"), None);
    }
}
