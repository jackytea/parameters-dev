use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::ordered_map::OrderedMap;

pub const EXPRESSION_KEY: &str = "$expr";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ValueType {
    Text,
    Number,
    Boolean,
    List,
    Object,
    Path,
    Secret,
    Any,
}

impl ValueType {
    pub const ALL: [Self; 8] = [
        Self::Text,
        Self::Number,
        Self::Boolean,
        Self::List,
        Self::Object,
        Self::Path,
        Self::Secret,
        Self::Any,
    ];

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|value_type| value_type.name() == name)
    }

    #[must_use]
    pub const fn accepts(self, value: Self) -> bool {
        matches!(
            (self, value),
            (Self::Any, _)
                | (_, Self::Any)
                | (Self::Text, Self::Text | Self::Path | Self::Secret)
                | (Self::Path, Self::Path | Self::Text)
                | (Self::Number, Self::Number)
                | (Self::Boolean, Self::Boolean)
                | (Self::List, Self::List)
                | (Self::Object, Self::Object)
                | (Self::Secret, Self::Secret)
        )
    }

    #[must_use]
    pub const fn has_fields(self) -> bool {
        matches!(self, Self::Object | Self::Any)
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::List => "list",
            Self::Object => "object",
            Self::Path => "path",
            Self::Secret => "secret",
            Self::Any => "any",
        }
    }
}

impl fmt::Display for ValueType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Duration {
    millis: u64,
}

impl Duration {
    const UNITS: [(&'static str, u64); 4] =
        [("h", 3_600_000), ("m", 60_000), ("s", 1_000), ("ms", 1)];

    #[must_use]
    pub const fn from_millis(millis: u64) -> Self {
        Self { millis }
    }

    #[must_use]
    pub const fn as_millis(self) -> u64 {
        self.millis
    }

    #[must_use]
    pub const fn to_std(self) -> std::time::Duration {
        std::time::Duration::from_millis(self.millis)
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let digits = text.bytes().take_while(u8::is_ascii_digit).count();
        let (amount, unit) = text.split_at(digits);
        let factor = Self::UNITS
            .iter()
            .find(|(name, _)| *name == unit)
            .map(|(_, factor)| *factor);
        match (amount.parse::<u64>(), factor) {
            (Ok(amount), Some(factor)) => amount
                .checked_mul(factor)
                .map(Self::from_millis)
                .ok_or_else(|| format!("duration `{text}` is too long")),
            _ => Err(format!(
                "invalid duration `{text}`: expected a whole number followed by ms, s, m or h"
            )),
        }
    }
}

impl fmt::Display for Duration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (unit, factor) = Self::UNITS
            .iter()
            .find(|(_, factor)| self.millis != 0 && self.millis.is_multiple_of(*factor))
            .copied()
            .unwrap_or(("ms", 1));
        write!(formatter, "{}{unit}", self.millis / factor)
    }
}

impl Serialize for Duration {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Duration {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = Cow::<str>::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for Duration {
    fn schema_name() -> Cow<'static, str> {
        "Duration".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": "^[0-9]+(ms|s|m|h)$",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(from = "[i32; 2]", into = "[i32; 2]")]
pub struct Position {
    pub x: i32,
    pub y: i32,
}

impl Position {
    #[must_use]
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

impl From<[i32; 2]> for Position {
    fn from([x, y]: [i32; 2]) -> Self {
        Self { x, y }
    }
}

impl From<Position> for [i32; 2] {
    fn from(position: Position) -> Self {
        [position.x, position.y]
    }
}

impl JsonSchema for Position {
    fn schema_name() -> Cow<'static, str> {
        "Position".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "array",
            "items": { "type": "integer" },
            "minItems": 2,
            "maxItems": 2,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    Literal(Value),
    Expression(String),
}

impl FieldValue {
    #[must_use]
    pub fn expression(source: impl Into<String>) -> Self {
        Self::Expression(source.into())
    }
}

impl Serialize for FieldValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Literal(value) => value.serialize(serializer),
            Self::Expression(source) => {
                use serde::ser::SerializeMap;
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry(EXPRESSION_KEY, source)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for FieldValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        if let Value::Object(object) = &value
            && let (1, Some(Value::String(source))) = (object.len(), object.get(EXPRESSION_KEY))
        {
            return Ok(Self::Expression(source.clone()));
        }
        Ok(Self::Literal(value))
    }
}

impl JsonSchema for FieldValue {
    fn schema_name() -> Cow<'static, str> {
        "FieldValue".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "anyOf": [
                {
                    "type": "object",
                    "properties": { EXPRESSION_KEY: { "type": "string" } },
                    "required": [EXPRESSION_KEY],
                    "additionalProperties": false,
                },
                {},
            ],
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub name: String,
    #[serde(rename = "type")]
    pub value_type: ValueType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Output {
    pub name: String,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub value_type: Option<ValueType>,
    pub value: FieldValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub id: String,
    pub uses: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<Duration>,
    #[serde(default, skip_serializing_if = "OrderedMap::is_empty")]
    pub with: OrderedMap<FieldValue>,
}

impl Step {
    #[must_use]
    pub fn new(id: impl Into<String>, uses: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            uses: uses.into(),
            note: None,
            after: Vec::new(),
            when: None,
            retry: None,
            timeout: None,
            with: OrderedMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<Input>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<Step>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<Output>,
    #[serde(default, skip_serializing_if = "OrderedMap::is_empty")]
    pub layout: OrderedMap<Position>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Workflow {
    #[serde(rename = "parameters")]
    pub version: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<Input>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<Step>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<Output>,
    #[serde(default, skip_serializing_if = "OrderedMap::is_empty")]
    pub definitions: OrderedMap<Definition>,
    #[serde(default, skip_serializing_if = "OrderedMap::is_empty")]
    pub layout: OrderedMap<Position>,
}

impl Workflow {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            version: crate::load::CURRENT_VERSION,
            name: name.into(),
            note: None,
            concurrency: None,
            inputs: Vec::new(),
            steps: Vec::new(),
            outputs: Vec::new(),
            definitions: OrderedMap::new(),
            layout: OrderedMap::new(),
        }
    }

    #[must_use]
    pub fn graph(&self) -> Graph<'_> {
        Graph {
            inputs: &self.inputs,
            steps: &self.steps,
            outputs: &self.outputs,
            layout: &self.layout,
        }
    }

    pub fn graphs_mut(&mut self) -> impl Iterator<Item = GraphMut<'_>> {
        std::iter::once(GraphMut {
            steps: &mut self.steps,
            layout: &mut self.layout,
        })
        .chain(self.definitions.values_mut().map(Definition::graph_mut))
    }
}

impl Definition {
    #[must_use]
    pub fn graph(&self) -> Graph<'_> {
        Graph {
            inputs: &self.inputs,
            steps: &self.steps,
            outputs: &self.outputs,
            layout: &self.layout,
        }
    }

    pub fn graph_mut(&mut self) -> GraphMut<'_> {
        GraphMut {
            steps: &mut self.steps,
            layout: &mut self.layout,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Graph<'a> {
    pub inputs: &'a [Input],
    pub steps: &'a [Step],
    pub outputs: &'a [Output],
    pub layout: &'a OrderedMap<Position>,
}

#[derive(Debug)]
pub struct GraphMut<'a> {
    pub steps: &'a mut Vec<Step>,
    pub layout: &'a mut OrderedMap<Position>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_round_trip_in_their_largest_exact_unit() {
        for (text, millis, canonical) in [
            ("30s", 30_000, "30s"),
            ("90s", 90_000, "90s"),
            ("120s", 120_000, "2m"),
            ("1500ms", 1_500, "1500ms"),
            ("2h", 7_200_000, "2h"),
            ("0s", 0, "0ms"),
        ] {
            let duration = Duration::parse(text).unwrap();
            assert_eq!(duration.as_millis(), millis);
            assert_eq!(duration.to_string(), canonical);
        }
    }

    #[test]
    fn rejects_malformed_durations() {
        for text in ["", "s", "10", "10x", "1.5s", "-1s", "99999999999999999999h"] {
            assert!(Duration::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn field_values_distinguish_expressions_from_literals() {
        let expression: FieldValue = serde_json::from_str(r#"{"$expr": "checkout.path"}"#).unwrap();
        assert_eq!(expression, FieldValue::expression("checkout.path"));
        let literal: FieldValue = serde_json::from_str(r#"{"$expr": 1}"#).unwrap();
        assert!(matches!(literal, FieldValue::Literal(_)));
        let mixed: FieldValue = serde_json::from_str(r#"{"$expr": "a", "b": 1}"#).unwrap();
        assert!(matches!(mixed, FieldValue::Literal(_)));
        assert_eq!(
            serde_json::to_string(&expression).unwrap(),
            r#"{"$expr":"checkout.path"}"#
        );
    }

    #[test]
    fn text_accepts_paths_and_secrets_but_secret_only_accepts_secrets() {
        assert!(ValueType::Text.accepts(ValueType::Secret));
        assert!(ValueType::Path.accepts(ValueType::Text));
        assert!(!ValueType::Secret.accepts(ValueType::Text));
        assert!(!ValueType::Number.accepts(ValueType::Text));
        assert!(ValueType::Number.accepts(ValueType::Any));
    }
}
