use parameters_api::{ApiError, MAX_PAGE_SIZE, Page};
use parameters_store::Cursor;

const DEFAULT_PAGE_SIZE: u32 = 50;

pub(crate) struct PageRequest {
    pub(crate) size: usize,
    pub(crate) after: Option<Cursor>,
}

impl PageRequest {
    pub(crate) fn new(size: Option<u32>, after: Option<&str>) -> Result<Self, ApiError> {
        let after = after
            .map(|text| {
                Cursor::decode(text)
                    .ok_or_else(|| ApiError::bad_request("the page cursor is not valid"))
            })
            .transpose()?;
        let size = size.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE);
        Ok(Self {
            size: size as usize,
            after,
        })
    }

    pub(crate) fn fetch_limit(&self) -> i64 {
        i64::try_from(self.size)
            .unwrap_or(i64::MAX)
            .saturating_add(1)
    }

    pub(crate) fn page<T>(&self, mut items: Vec<T>, cursor: impl Fn(&T) -> Cursor) -> Page<T> {
        let next = if items.len() > self.size {
            items.truncate(self.size);
            items.last().map(|last| cursor(last).encode())
        } else {
            None
        };
        Page { items, next }
    }
}
