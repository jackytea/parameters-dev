use std::time::Duration;

use parameters_api::{
    ApiError, CreatedToken, DeviceStart, Limits, Me, Mode, Scope, Scopes, TokenSummary, UserInfo,
    Uuid, Via,
};
use parameters_store::NewToken;
use ring::digest::{SHA256, digest};
use time::OffsetDateTime;

use crate::mail::Mail;
use crate::oauth::ProviderKind;
use crate::{Actor, LOCAL_USER, Result, Service, internal};

pub const TOKEN_PREFIX: &str = "prm_";
const LOGIN_TTL: Duration = Duration::from_mins(15);
const SESSION_TTL: Duration = Duration::from_hours(30 * 24);
const EMAIL_LIMIT: u32 = 5;
const ADDRESS_LIMIT: u32 = 20;
const LIMIT_PERIOD: Duration = Duration::from_hours(1);
const MAX_EMAIL: usize = 254;
const MAX_TOKEN_NAME: usize = 100;
const MAX_TOKEN_DAYS: u32 = 365;

const DEVICE_TTL: Duration = Duration::from_mins(10);
const DEVICE_INTERVAL: u64 = 2;
const DEVICE_LIMIT: u32 = 10;
const DEVICE_ALPHABET: &[u8; 20] = b"BCDFGHJKLMNPQRSTVWXZ";
const MAX_NEXT: usize = 512;

pub struct SignedIn {
    pub session: String,
    pub next: String,
}

#[must_use]
pub fn safe_next(next: Option<&str>) -> Option<String> {
    next.filter(|path| {
        path.len() <= MAX_NEXT
            && path.starts_with('/')
            && !path.starts_with("//")
            && !path.contains('\\')
            && !path.chars().any(char::is_control)
    })
    .map(str::to_owned)
}

fn normalize_user_code(code: &str) -> String {
    code.chars()
        .filter(char::is_ascii_alphabetic)
        .map(|character| character.to_ascii_uppercase())
        .collect()
}

pub struct OAuthStart {
    pub url: String,
    pub state: String,
    pub verifier: String,
}

pub(crate) fn hash(token: &str) -> [u8; 32] {
    let mut out = [0; 32];
    out.copy_from_slice(digest(&SHA256, token.as_bytes()).as_ref());
    out
}

pub(crate) fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let triple = chunk.iter().enumerate().fold(0_u32, |bits, (index, byte)| {
            bits | u32::from(*byte) << (16 - 8 * index)
        });
        for index in 0..=chunk.len() {
            out.push(char::from(
                ALPHABET[((triple >> (18 - 6 * index)) & 63) as usize],
            ));
        }
    }
    out
}

fn normalize_email(email: &str) -> Result<String> {
    let email = email.trim().to_lowercase();
    let valid = email.len() <= MAX_EMAIL
        && !email
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
        && email.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
                && !domain.contains('@')
        });
    if valid {
        Ok(email)
    } else {
        Err(ApiError::bad_request("enter a valid email address"))
    }
}

impl Service {
    fn managed(&self) -> Result<&crate::Managed> {
        self.inner
            .options
            .managed
            .as_ref()
            .ok_or_else(|| ApiError::bad_request("this server has no accounts"))
    }

    fn random_token(&self) -> Result<String> {
        Ok(base64url(
            &self.inner.cipher.random_bytes().map_err(internal)?,
        ))
    }

    async fn new_session(&self, user: Uuid) -> Result<String> {
        let token = self.random_token()?;
        self.store()
            .add_session(&hash(&token), user, SESSION_TTL)
            .await
            .map_err(internal)?;
        Ok(token)
    }

    pub async fn request_email_sign_in(
        &self,
        email: &str,
        client: &str,
        next: Option<&str>,
    ) -> Result<()> {
        let managed = self.managed()?;
        let email = normalize_email(email)?;
        let limiter = &self.inner.limiter;
        if !limiter.allow(&format!("sign-in:{email}"), EMAIL_LIMIT, LIMIT_PERIOD)
            || !limiter.allow(
                &format!("sign-in-from:{client}"),
                ADDRESS_LIMIT,
                LIMIT_PERIOD,
            )
        {
            return Err(ApiError::TooManyRequests {
                message: "too many sign-in emails; wait a while and try again".into(),
            });
        }
        let mailer = managed
            .mailer
            .clone()
            .ok_or_else(|| ApiError::bad_request("email sign-in is not set up on this server"))?;
        let token = self.random_token()?;
        self.store()
            .add_login_token(&hash(&token), &email, LOGIN_TTL, safe_next(next).as_deref())
            .await
            .map_err(internal)?;
        let link = format!("{}/auth/email?token={token}", managed.public_url);
        mailer
            .send(Mail {
                to: email,
                subject: "Sign in to Parameters".into(),
                text: format!(
                    "Open this link to sign in to Parameters:\n\n{link}\n\nThe link works once and expires in 15 minutes. If you did not ask to sign in, you can ignore this email.\n"
                ),
            })
            .await
            .map_err(internal)
    }

    pub async fn finish_email_sign_in(&self, token: &str) -> Result<SignedIn> {
        self.managed()?;
        let (email, next) = self
            .store()
            .consume_login_token(&hash(token))
            .await
            .map_err(internal)?
            .ok_or_else(|| ApiError::Unauthorized {
                message: "this sign-in link has expired or was already used; ask for a new one"
                    .into(),
            })?;
        let user = self
            .store()
            .user_for_email(&email)
            .await
            .map_err(internal)?;
        Ok(SignedIn {
            session: self.new_session(user).await?,
            next: safe_next(next.as_deref()).unwrap_or_else(|| "/workflows".into()),
        })
    }

    pub async fn start_device(&self, name: &str, client: &str) -> Result<DeviceStart> {
        let managed = self.managed()?;
        if !self
            .inner
            .limiter
            .allow(&format!("device-from:{client}"), DEVICE_LIMIT, LIMIT_PERIOD)
        {
            return Err(ApiError::TooManyRequests {
                message: "too many sign-in attempts from this address; wait a while".into(),
            });
        }
        let name: String = name.trim().chars().take(MAX_TOKEN_NAME).collect();
        let name = if name.is_empty() {
            "Parameters app".to_owned()
        } else {
            name
        };
        let device_code = self.random_token()?;
        loop {
            let bytes = self.inner.cipher.random_bytes().map_err(internal)?;
            let user_code: String = bytes[..8]
                .iter()
                .map(|byte| char::from(DEVICE_ALPHABET[usize::from(*byte) % DEVICE_ALPHABET.len()]))
                .collect();
            if self
                .store()
                .add_device_code(&hash(&device_code), &user_code, &name, DEVICE_TTL)
                .await
                .map_err(internal)?
            {
                let shown = format!("{}-{}", &user_code[..4], &user_code[4..]);
                return Ok(DeviceStart {
                    verification_uri: format!("{}/connect?code={shown}", managed.public_url),
                    device_code,
                    user_code: shown,
                    expires_in: DEVICE_TTL.as_secs(),
                    interval: DEVICE_INTERVAL,
                });
            }
        }
    }

    pub async fn approve_device(&self, actor: &Actor, code: &str) -> Result<String> {
        if actor.via != Via::Ui {
            return Err(ApiError::forbidden(
                "devices can only be connected in the Parameters app",
            ));
        }
        self.store()
            .approve_device_code(&normalize_user_code(code), actor.user)
            .await
            .map_err(internal)?
            .ok_or_else(|| {
                ApiError::not_found("that code is unknown or expired; start again in the app")
            })
    }

    pub async fn claim_device(&self, device_code: &str) -> Result<Option<String>> {
        match self
            .store()
            .claim_device_code(&hash(device_code))
            .await
            .map_err(internal)?
        {
            Some(Some(user)) => Ok(Some(self.new_session(user).await?)),
            Some(None) => Ok(None),
            None => Err(ApiError::Unauthorized {
                message: "this sign-in expired; start again".into(),
            }),
        }
    }

    pub fn start_oauth(&self, provider: &str) -> Result<OAuthStart> {
        let managed = self.managed()?;
        let provider = managed
            .providers
            .iter()
            .find(|candidate| candidate.kind.name() == provider)
            .ok_or_else(|| ApiError::not_found(format!("`{provider}` sign-in is not set up")))?;
        let state = self.random_token()?;
        let verifier = self.random_token()?;
        let challenge = base64url(digest(&SHA256, verifier.as_bytes()).as_ref());
        let redirect = format!(
            "{}/auth/{}/callback",
            managed.public_url,
            provider.kind.name()
        );
        let url = provider
            .authorize(&redirect, &state, &challenge)
            .map_err(internal)?;
        Ok(OAuthStart {
            url,
            state,
            verifier,
        })
    }

    pub async fn finish_oauth(&self, provider: &str, code: &str, verifier: &str) -> Result<String> {
        let managed = self.managed()?;
        let provider = managed
            .providers
            .iter()
            .find(|candidate| candidate.kind.name() == provider)
            .ok_or_else(|| ApiError::not_found(format!("`{provider}` sign-in is not set up")))?;
        let redirect = format!(
            "{}/auth/{}/callback",
            managed.public_url,
            provider.kind.name()
        );
        let identity = provider
            .identify(&self.inner.http, code, verifier, &redirect)
            .await
            .map_err(|message| ApiError::Unauthorized { message })?;
        self.sign_in_identity(&identity).await
    }

    pub async fn sign_in_identity(&self, identity: &crate::oauth::Identity) -> Result<String> {
        self.managed()?;
        let user = self
            .store()
            .user_for_identity(
                identity.kind.name(),
                &identity.id,
                identity.verified_email.as_deref(),
                &identity.name,
            )
            .await
            .map_err(internal)?;
        self.new_session(user).await
    }

    pub async fn session_actor(&self, token: &str) -> Option<Actor> {
        let user = self
            .store()
            .session_user(&hash(token), SESSION_TTL)
            .await
            .ok()
            .flatten()?;
        Some(Actor {
            user,
            via: Via::Ui,
            scopes: None,
        })
    }

    pub async fn sign_out(&self, token: &str) -> Result<()> {
        self.store()
            .delete_session(&hash(token))
            .await
            .map_err(internal)
    }

    pub async fn token_actor(&self, token: &str, via: Via) -> Option<Actor> {
        if !token.starts_with(TOKEN_PREFIX) {
            return None;
        }
        let (user, scopes) = self.store().token_user(&hash(token)).await.ok().flatten()?;
        Some(Actor {
            user,
            via,
            scopes: Some(scopes),
        })
    }

    pub async fn create_token(
        &self,
        actor: &Actor,
        name: &str,
        scopes: Scopes,
        expires_in_days: Option<u32>,
    ) -> Result<CreatedToken> {
        if actor.via != Via::Ui {
            return Err(ApiError::forbidden(
                "access tokens can only be created in the Parameters app",
            ));
        }
        let name = name.trim();
        if name.is_empty() || name.len() > MAX_TOKEN_NAME {
            return Err(ApiError::bad_request(format!(
                "give the token a name of up to {MAX_TOKEN_NAME} characters"
            )));
        }
        if scopes.is_empty() {
            return Err(ApiError::bad_request(
                "choose at least one permission: read, write or run",
            ));
        }
        let expires_at = match expires_in_days {
            Some(days) if (1..=MAX_TOKEN_DAYS).contains(&days) => {
                Some(OffsetDateTime::now_utc() + Duration::from_secs(u64::from(days) * 86_400))
            }
            Some(_) => {
                return Err(ApiError::bad_request(format!(
                    "a token can last from 1 to {MAX_TOKEN_DAYS} days"
                )));
            }
            None => None,
        };
        let token = format!("{TOKEN_PREFIX}{}", self.random_token()?);
        let summary = self
            .store()
            .add_token(
                actor.user,
                NewToken {
                    name,
                    hash: &hash(&token),
                    scopes,
                    expires_at,
                },
            )
            .await
            .map_err(internal)?;
        Ok(CreatedToken { summary, token })
    }

    pub async fn tokens(&self, actor: &Actor) -> Result<Vec<TokenSummary>> {
        actor.require(Scope::Read)?;
        self.store().tokens(actor.user).await.map_err(internal)
    }

    pub async fn revoke_token(&self, actor: &Actor, id: Uuid) -> Result<()> {
        if actor.via != Via::Ui {
            return Err(ApiError::forbidden(
                "access tokens can only be revoked in the Parameters app",
            ));
        }
        if self
            .store()
            .revoke_token(actor.user, id)
            .await
            .map_err(internal)?
        {
            Ok(())
        } else {
            Err(ApiError::not_found("there is no such token"))
        }
    }

    pub async fn me(&self, actor: Option<&Actor>) -> Result<Me> {
        let (mode, providers) = match &self.inner.options.managed {
            Some(managed) => (
                Mode::Managed,
                managed
                    .providers
                    .iter()
                    .map(|provider| provider.kind.name().to_owned())
                    .collect(),
            ),
            None => (Mode::Local, Vec::new()),
        };
        let user = match actor {
            Some(actor) => self.store().user(actor.user).await.map_err(internal)?,
            None => None,
        };
        Ok(Me {
            mode,
            user: user.map(|user| UserInfo {
                display_name: if user.id == LOCAL_USER {
                    "You".into()
                } else {
                    user.display_name
                },
                ..user
            }),
            providers,
        })
    }

    #[must_use]
    pub fn allow_request(&self, key: &str) -> bool {
        self.inner
            .options
            .capabilities
            .requests_per_minute
            .is_none_or(|limit| {
                self.inner
                    .limiter
                    .allow(&format!("requests:{key}"), limit, Duration::from_secs(60))
            })
    }

    #[must_use]
    pub fn trusts_proxy(&self) -> bool {
        self.inner
            .options
            .managed
            .as_ref()
            .is_some_and(|managed| managed.trust_proxy)
    }

    #[must_use]
    pub fn limits(&self) -> Limits {
        self.inner.options.capabilities.limits()
    }

    #[must_use]
    pub fn is_managed(&self) -> bool {
        self.inner.options.managed.is_some()
    }

    #[must_use]
    pub fn public_url(&self) -> Option<&str> {
        self.inner
            .options
            .managed
            .as_ref()
            .map(|managed| managed.public_url.as_str())
    }

    #[must_use]
    pub fn providers(&self) -> Vec<ProviderKind> {
        self.inner
            .options
            .managed
            .as_ref()
            .map(|managed| {
                managed
                    .providers
                    .iter()
                    .map(|provider| provider.kind)
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_matches_the_standard_vectors() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg"),
            ("fo", "Zm8"),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg"),
            ("fooba", "Zm9vYmE"),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64url(input.as_bytes()), expected);
        }
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
        assert_eq!(base64url(&[0; 32]).len(), 43);
    }

    #[test]
    fn emails_are_normalized_and_checked() {
        assert_eq!(
            normalize_email("  Ada@Example.COM ").unwrap(),
            "ada@example.com"
        );
        for bad in [
            "",
            "ada",
            "ada@",
            "@example.com",
            "ada@example",
            "a b@example.com",
            "a@@b.com",
            "ada@.com",
        ] {
            assert!(normalize_email(bad).is_err(), "{bad}");
        }
    }
}
