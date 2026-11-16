use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub at: OffsetDateTime,
    pub id: Uuid,
}

impl Cursor {
    #[must_use]
    pub fn encode(&self) -> String {
        format!(
            "{}_{}",
            self.at.unix_timestamp_nanos() / 1_000,
            self.id.simple()
        )
    }

    #[must_use]
    pub fn decode(text: &str) -> Option<Self> {
        let (micros, id) = text.split_once('_')?;
        let micros: i128 = micros.parse().ok()?;
        Some(Self {
            at: OffsetDateTime::from_unix_timestamp_nanos(micros.checked_mul(1_000)?).ok()?,
            id: Uuid::try_parse(id).ok()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_rejects_garbage() {
        let cursor = Cursor {
            at: OffsetDateTime::from_unix_timestamp_nanos(1_763_251_200_123_456_000).unwrap(),
            id: Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef),
        };
        assert_eq!(Cursor::decode(&cursor.encode()), Some(cursor));
        for garbage in ["", "_", "12", "x_0123", "99999999999999999999999999_00"] {
            assert_eq!(Cursor::decode(garbage), None, "{garbage}");
        }
    }
}
