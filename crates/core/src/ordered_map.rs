use std::borrow::Cow;
use std::fmt;
use std::marker::PhantomData;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, PartialEq)]
pub struct OrderedMap<V> {
    entries: Vec<(String, V)>,
}

impl<V> Default for OrderedMap<V> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl<V> OrderedMap<V> {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&V> {
        self.position(key).map(|index| &self.entries[index].1)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut V> {
        self.position(key).map(|index| &mut self.entries[index].1)
    }

    #[must_use]
    pub fn contains_key(&self, key: &str) -> bool {
        self.position(key).is_some()
    }

    pub fn insert(&mut self, key: impl Into<String>, value: V) -> Option<V> {
        let key = key.into();
        if let Some(index) = self.position(&key) {
            Some(std::mem::replace(&mut self.entries[index].1, value))
        } else {
            self.entries.push((key, value));
            None
        }
    }

    pub fn remove(&mut self, key: &str) -> Option<V> {
        self.position(key).map(|index| self.entries.remove(index).1)
    }

    pub fn rename_key(&mut self, old: &str, new: impl Into<String>) -> bool {
        let new = new.into();
        if self.contains_key(&new) {
            return false;
        }
        match self.position(old) {
            Some(index) => {
                self.entries[index].0 = new;
                true
            }
            None => false,
        }
    }

    pub(crate) fn push_absent(&mut self, key: String, value: V) {
        self.entries.push((key, value));
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &V)> {
        self.entries
            .iter()
            .map(|(key, value)| (key.as_str(), value))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&str, &mut V)> {
        self.entries
            .iter_mut()
            .map(|(key, value)| (key.as_str(), value))
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(key, _)| key.as_str())
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.iter().map(|(_, value)| value)
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.entries.iter_mut().map(|(_, value)| value)
    }

    fn position(&self, key: &str) -> Option<usize> {
        self.entries
            .iter()
            .position(|(existing, _)| existing == key)
    }
}

impl<V> FromIterator<(String, V)> for OrderedMap<V> {
    fn from_iter<I: IntoIterator<Item = (String, V)>>(iter: I) -> Self {
        let mut map = Self::new();
        for (key, value) in iter {
            map.insert(key, value);
        }
        map
    }
}

impl<V> IntoIterator for OrderedMap<V> {
    type Item = (String, V);
    type IntoIter = std::vec::IntoIter<(String, V)>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

impl<V: Serialize> Serialize for OrderedMap<V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.entries.len()))?;
        for (key, value) in &self.entries {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

impl<'de, V: Deserialize<'de>> Deserialize<'de> for OrderedMap<V> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct OrderedMapVisitor<V>(PhantomData<V>);

        impl<'de, V: Deserialize<'de>> Visitor<'de> for OrderedMapVisitor<V> {
            type Value = OrderedMap<V>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut map = OrderedMap::new();
                while let Some(entry) = access.next_entry::<String, V>()? {
                    map.entries.push(entry);
                }
                let mut keys: Vec<&str> = map.keys().collect();
                keys.sort_unstable();
                let duplicate = keys.windows(2).find_map(|pair| match pair {
                    [left, right] if left == right => Some(*left),
                    _ => None,
                });
                if let Some(key) = duplicate {
                    return Err(serde::de::Error::custom(format!("duplicate key `{key}`")));
                }
                Ok(map)
            }
        }

        deserializer.deserialize_map(OrderedMapVisitor(PhantomData))
    }
}

impl<V: JsonSchema> JsonSchema for OrderedMap<V> {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        format!("Map_of_{}", V::schema_name()).into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "object",
            "additionalProperties": generator.subschema_for::<V>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_insertion_order_through_json() {
        let map: OrderedMap<u32> = serde_json::from_str(r#"{"z": 1, "a": 2, "m": 3}"#).unwrap();
        assert_eq!(map.keys().collect::<Vec<_>>(), ["z", "a", "m"]);
        assert_eq!(
            serde_json::to_string(&map).unwrap(),
            r#"{"z":1,"a":2,"m":3}"#
        );
    }

    #[test]
    fn rejects_duplicate_keys() {
        let error = serde_json::from_str::<OrderedMap<u32>>(r#"{"a": 1, "a": 2}"#).unwrap_err();
        assert!(error.to_string().contains("duplicate key `a`"));
    }

    #[test]
    fn insert_replaces_in_place_and_rename_keeps_position() {
        let mut map: OrderedMap<u32> = [("a".into(), 1), ("b".into(), 2)].into_iter().collect();
        assert_eq!(map.insert("a", 10), Some(1));
        assert!(map.rename_key("a", "c"));
        assert!(!map.rename_key("c", "b"));
        assert_eq!(map.iter().collect::<Vec<_>>(), [("c", &10), ("b", &2)]);
    }
}
