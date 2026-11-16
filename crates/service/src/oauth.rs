use reqwest::header::{ACCEPT, USER_AGENT};
use serde_json::Value;

const AGENT: &str = concat!("parameters/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    Google,
    Github,
    Discord,
}

impl ProviderKind {
    pub const ALL: [Self; 3] = [Self::Google, Self::Github, Self::Discord];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Google => "google",
            Self::Github => "github",
            Self::Discord => "discord",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Provider {
    pub kind: ProviderKind,
    pub client_id: String,
    pub client_secret: String,
    pub authorize_url: String,
    pub token_url: String,
    pub profile_url: String,
    pub emails_url: Option<String>,
    pub scope: &'static str,
}

impl std::fmt::Debug for Provider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Provider")
            .field("kind", &self.kind)
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

impl Provider {
    #[must_use]
    pub fn new(kind: ProviderKind, client_id: String, client_secret: String) -> Self {
        let urls = |authorize: &str, token: &str, profile: &str| {
            (authorize.to_owned(), token.to_owned(), profile.to_owned())
        };
        let ((authorize_url, token_url, profile_url), emails_url, scope) = match kind {
            ProviderKind::Google => (
                urls(
                    "https://accounts.google.com/o/oauth2/v2/auth",
                    "https://oauth2.googleapis.com/token",
                    "https://openidconnect.googleapis.com/v1/userinfo",
                ),
                None,
                "openid email profile",
            ),
            ProviderKind::Github => (
                urls(
                    "https://github.com/login/oauth/authorize",
                    "https://github.com/login/oauth/access_token",
                    "https://api.github.com/user",
                ),
                Some("https://api.github.com/user/emails".to_owned()),
                "read:user user:email",
            ),
            ProviderKind::Discord => (
                urls(
                    "https://discord.com/oauth2/authorize",
                    "https://discord.com/api/oauth2/token",
                    "https://discord.com/api/users/@me",
                ),
                None,
                "identify email",
            ),
        };
        Self {
            kind,
            client_id,
            client_secret,
            authorize_url,
            token_url,
            profile_url,
            emails_url,
            scope,
        }
    }

    pub(crate) fn authorize(
        &self,
        redirect_uri: &str,
        state: &str,
        challenge: &str,
    ) -> Result<String, String> {
        reqwest::Url::parse_with_params(
            &self.authorize_url,
            &[
                ("response_type", "code"),
                ("client_id", self.client_id.as_str()),
                ("redirect_uri", redirect_uri),
                ("scope", self.scope),
                ("state", state),
                ("code_challenge", challenge),
                ("code_challenge_method", "S256"),
            ],
        )
        .map(String::from)
        .map_err(|error| error.to_string())
    }

    pub(crate) async fn identify(
        &self,
        http: &reqwest::Client,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<Identity, String> {
        let token: Value = http
            .post(&self.token_url)
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, AGENT)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", redirect_uri),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("code_verifier", verifier),
            ])
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|error| format!("the sign-in provider refused the code: {error}"))?
            .json()
            .await
            .map_err(|error| error.to_string())?;
        let access = token["access_token"]
            .as_str()
            .ok_or("the sign-in provider did not return an access token")?;
        let get = |url: &str| {
            http.get(url)
                .bearer_auth(access)
                .header(ACCEPT, "application/json")
                .header(USER_AGENT, AGENT)
                .send()
        };
        let profile: Value = get(&self.profile_url)
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|error| format!("cannot read the account from the sign-in provider: {error}"))?
            .json()
            .await
            .map_err(|error| error.to_string())?;
        let emails = match &self.emails_url {
            Some(url) => Some(
                get(url)
                    .await
                    .and_then(reqwest::Response::error_for_status)
                    .map_err(|error| format!("cannot read the account's email: {error}"))?
                    .json()
                    .await
                    .map_err(|error| error.to_string())?,
            ),
            None => None,
        };
        parse_identity(self.kind, &profile, emails.as_ref())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub kind: ProviderKind,
    pub id: String,
    pub verified_email: Option<String>,
    pub name: String,
}

fn text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

pub(crate) fn parse_identity(
    kind: ProviderKind,
    profile: &Value,
    emails: Option<&Value>,
) -> Result<Identity, String> {
    let missing = || format!("the {} account has no id", kind.name());
    let (id, verified_email, name) = match kind {
        ProviderKind::Google => (
            text(&profile["sub"]).ok_or_else(missing)?,
            (profile["email_verified"] == Value::Bool(true))
                .then(|| text(&profile["email"]))
                .flatten(),
            text(&profile["name"]),
        ),
        ProviderKind::Github => (
            text(&profile["id"]).ok_or_else(missing)?,
            emails
                .and_then(Value::as_array)
                .and_then(|list| {
                    list.iter().find(|entry| {
                        entry["primary"] == Value::Bool(true)
                            && entry["verified"] == Value::Bool(true)
                    })
                })
                .and_then(|entry| text(&entry["email"])),
            text(&profile["name"]).or_else(|| text(&profile["login"])),
        ),
        ProviderKind::Discord => (
            text(&profile["id"]).ok_or_else(missing)?,
            (profile["verified"] == Value::Bool(true))
                .then(|| text(&profile["email"]))
                .flatten(),
            text(&profile["global_name"]).or_else(|| text(&profile["username"])),
        ),
    };
    let name = name
        .or_else(|| verified_email.clone())
        .unwrap_or_else(|| format!("{} user", kind.name()));
    Ok(Identity {
        kind,
        id,
        verified_email,
        name,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn identities_use_only_verified_emails() {
        let google = parse_identity(
            ProviderKind::Google,
            &json!({"sub": "1", "email": "a@example.com", "email_verified": true, "name": "Ada"}),
            None,
        )
        .unwrap();
        assert_eq!(google.verified_email.as_deref(), Some("a@example.com"));
        let unverified = parse_identity(
            ProviderKind::Discord,
            &json!({"id": "9", "email": "b@example.com", "verified": false, "username": "bee"}),
            None,
        )
        .unwrap();
        assert_eq!(
            (unverified.verified_email, unverified.name.as_str()),
            (None, "bee")
        );
        let github = parse_identity(
            ProviderKind::Github,
            &json!({"id": 42, "login": "octo", "name": null}),
            Some(&json!([
                {"email": "old@example.com", "primary": false, "verified": true},
                {"email": "main@example.com", "primary": true, "verified": true}
            ])),
        )
        .unwrap();
        assert_eq!(
            (
                github.id.as_str(),
                github.verified_email.as_deref(),
                github.name.as_str()
            ),
            ("42", Some("main@example.com"), "octo")
        );
        assert!(parse_identity(ProviderKind::Google, &json!({}), None).is_err());
    }

    #[test]
    fn authorize_urls_carry_pkce_and_state() {
        let provider = Provider::new(ProviderKind::Github, "client".into(), "secret".into());
        let url = provider
            .authorize(
                "https://parameters.dev/auth/github/callback",
                "state",
                "challenge",
            )
            .unwrap();
        assert!(url.starts_with("https://github.com/login/oauth/authorize?response_type=code"));
        assert!(url.contains("code_challenge=challenge&code_challenge_method=S256"));
        assert!(
            url.contains("redirect_uri=https%3A%2F%2Fparameters.dev%2Fauth%2Fgithub%2Fcallback")
        );
        assert!(!url.contains("secret"));
    }
}
