use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Read,
    Write,
    Run,
}

impl Scope {
    pub const ALL: [Self; 3] = [Self::Read, Self::Write, Self::Run];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Run => "run",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|scope| scope.name() == name)
    }

    const fn bit(self) -> u8 {
        match self {
            Self::Read => 1,
            Self::Write => 2,
            Self::Run => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Scopes(u8);

impl Scopes {
    pub const ALL: Self = Self(7);

    #[must_use]
    pub const fn contains(self, scope: Scope) -> bool {
        self.0 & scope.bit() != 0
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    #[must_use]
    pub fn list(self) -> Vec<Scope> {
        Scope::ALL
            .into_iter()
            .filter(|scope| self.contains(*scope))
            .collect()
    }
}

impl FromIterator<Scope> for Scopes {
    fn from_iter<I: IntoIterator<Item = Scope>>(scopes: I) -> Self {
        Self(scopes.into_iter().fold(0, |bits, scope| bits | scope.bit()))
    }
}

impl Serialize for Scopes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.list().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Scopes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Vec::<Scope>::deserialize(deserializer)?
            .into_iter()
            .collect())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Local,
    Managed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserInfo {
    pub id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Me {
    pub mode: Mode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<UserInfo>,
    pub providers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenSummary {
    pub id: Uuid,
    pub name: String,
    pub scopes: Scopes,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedToken {
    #[serde(flatten)]
    pub summary: TokenSummary,
    pub token: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_serialize_as_names() {
        let scopes: Scopes = [Scope::Run, Scope::Read].into_iter().collect();
        assert!(scopes.contains(Scope::Read) && !scopes.contains(Scope::Write));
        let json = serde_json::to_string(&scopes).unwrap();
        assert_eq!(json, r#"["read","run"]"#);
        assert_eq!(serde_json::from_str::<Scopes>(&json).unwrap(), scopes);
        assert!(Scopes::default().is_empty());
    }
}
