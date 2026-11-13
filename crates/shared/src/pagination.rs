//! 列表 API 的分頁慣例（docs/architecture.md §10「分頁」）。
//!
//! - query string：`page`（從 1 開始，預設 1）、`page_size`（預設 [`DEFAULT_PAGE_SIZE`]，
//!   超過 [`MAX_PAGE_SIZE`] 時套用上限）
//! - 回應：[`Page`]，包含該頁資料與符合條件的總筆數
//! - `page` 或 `page_size` 小於 1 → `422 VALIDATION`，`details.field` 為參數名稱
//!
//! 各列表 API 的 query 型別直接包含 `page`、`page_size` 兩個欄位（不使用 `#[serde(flatten)]`，
//! 因為 query string 搭配 flatten 時數字欄位無法解析），再用 [`PageRequest::new`] 檢查與套用預設值。

use serde::{Deserialize, Serialize};
use specta::Type;

/// 預設每頁筆數。
pub const DEFAULT_PAGE_SIZE: u32 = 50;
/// 每頁筆數上限；超過時套用上限。
pub const MAX_PAGE_SIZE: u32 = 200;

/// 分頁參數名稱，用於驗證錯誤的 `details.field`。
pub mod field {
    pub const PAGE: &str = "page";
    pub const PAGE_SIZE: &str = "page_size";
}

/// 分頁參數不合法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidPageParam {
    /// 參數名稱（[`field`]）。
    pub field: &'static str,
}

impl InvalidPageParam {
    /// 顯示給使用者的訊息。
    pub fn message(&self) -> &'static str {
        match self.field {
            field::PAGE => "頁碼必須大於或等於 1",
            _ => "每頁筆數必須大於或等於 1",
        }
    }
}

/// 檢查並套用預設值後的分頁參數。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRequest {
    /// 頁碼，從 1 開始。
    pub page: u32,
    /// 每頁筆數，介於 1 到 [`MAX_PAGE_SIZE`]。
    pub page_size: u32,
}

impl PageRequest {
    /// 由 query string 的 `page`、`page_size` 建立。省略時套用預設值，
    /// `page_size` 超過上限時套用上限；小於 1 時回傳錯誤。
    pub fn new(page: Option<u32>, page_size: Option<u32>) -> Result<Self, InvalidPageParam> {
        let page = page.unwrap_or(1);
        if page < 1 {
            return Err(InvalidPageParam { field: field::PAGE });
        }
        let page_size = page_size.unwrap_or(DEFAULT_PAGE_SIZE);
        if page_size < 1 {
            return Err(InvalidPageParam {
                field: field::PAGE_SIZE,
            });
        }
        Ok(Self {
            page,
            page_size: page_size.min(MAX_PAGE_SIZE),
        })
    }

    /// SQL `LIMIT`。
    pub fn limit(&self) -> i64 {
        i64::from(self.page_size)
    }

    /// SQL `OFFSET`。
    pub fn offset(&self) -> i64 {
        i64::from(self.page - 1) * i64::from(self.page_size)
    }
}

impl Default for PageRequest {
    fn default() -> Self {
        Self {
            page: 1,
            page_size: DEFAULT_PAGE_SIZE,
        }
    }
}

/// 分頁的列表回應。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct Page<T> {
    /// 該頁的資料。頁碼超過最後一頁時為空陣列。
    pub items: Vec<T>,
    /// 符合條件的總筆數（不受分頁影響）。
    pub total: u32,
    /// 實際使用的頁碼。
    pub page: u32,
    /// 實際使用的每頁筆數（已套用預設值與上限）。
    pub page_size: u32,
}

impl<T> Page<T> {
    /// 由查詢結果建立。
    pub fn new(items: Vec<T>, total: u32, request: PageRequest) -> Self {
        Self {
            items,
            total,
            page: request.page,
            page_size: request.page_size,
        }
    }

    /// 總頁數；沒有資料時為 0。
    pub fn total_pages(&self) -> u32 {
        self.total.div_ceil(self.page_size.max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_when_omitted() {
        assert_eq!(PageRequest::new(None, None), Ok(PageRequest::default()));
        assert_eq!(
            PageRequest::default(),
            PageRequest {
                page: 1,
                page_size: 50
            }
        );
    }

    #[test]
    fn page_size_is_capped() {
        let req = PageRequest::new(Some(2), Some(1000)).unwrap();
        assert_eq!(req.page_size, MAX_PAGE_SIZE);
        assert_eq!(PageRequest::new(None, Some(200)).unwrap().page_size, 200);
        assert_eq!(PageRequest::new(None, Some(1)).unwrap().page_size, 1);
    }

    #[test]
    fn zero_is_rejected() {
        let err = PageRequest::new(Some(0), None).unwrap_err();
        assert_eq!(err.field, field::PAGE);
        assert!(err.message().contains("頁碼"));
        let err = PageRequest::new(None, Some(0)).unwrap_err();
        assert_eq!(err.field, field::PAGE_SIZE);
        assert!(err.message().contains("每頁筆數"));
    }

    #[test]
    fn limit_and_offset() {
        let req = PageRequest::new(Some(3), Some(20)).unwrap();
        assert_eq!((req.limit(), req.offset()), (20, 40));
        let req = PageRequest::new(Some(u32::MAX), Some(MAX_PAGE_SIZE)).unwrap();
        assert_eq!(req.offset(), i64::from(u32::MAX - 1) * 200);
    }

    #[test]
    fn total_pages() {
        let req = PageRequest::new(None, Some(10)).unwrap();
        assert_eq!(Page::<u8>::new(vec![], 0, req).total_pages(), 0);
        assert_eq!(Page::<u8>::new(vec![], 10, req).total_pages(), 1);
        assert_eq!(Page::<u8>::new(vec![], 11, req).total_pages(), 2);
    }

    #[test]
    fn page_json_shape() {
        let page = Page::new(
            vec!["a", "b"],
            7,
            PageRequest::new(Some(2), Some(2)).unwrap(),
        );
        assert_eq!(
            serde_json::to_value(&page).unwrap(),
            json!({ "items": ["a", "b"], "total": 7, "page": 2, "page_size": 2 })
        );
    }
}
