//! 幣別的 request / response（docs/architecture.md §6.1、§8.6 `currencies`）。
//!
//! 幣別不是資料類別，使用專屬端點（§10）：
//!
//! | 端點 | 權限 | request | response |
//! |---|---|---|---|
//! | `GET /api/v1/currencies` | 登入 | [`CurrencyListQuery`]（query string） | `Currency[]` |
//! | `POST /api/v1/currencies` | `settings.manage` | [`CreateCurrencyRequest`] | `201` [`Currency`] |
//! | `PATCH /api/v1/currencies/{code}` | `settings.manage` | [`UpdateCurrencyRequest`] | [`Currency`] |
//!
//! 列表不分頁，本位幣優先、再依代碼排序。幣別不可刪除，只能停用。
//!
//! 錯誤：
//!
//! - 輸入不符合規則（代碼格式、代碼重複、停用本位幣等）→ `422 VALIDATION`，`details.field` 為欄位名稱
//! - `version` 不符 → `409 CONFLICT`（§9.4）
//! - 幣別不存在 → `404 NOT_FOUND`

use serde::{Deserialize, Serialize};
use specta::Type;
use time::OffsetDateTime;

use crate::Id;

/// 本位幣代碼。
pub const BASE_CURRENCY: &str = "TWD";

/// 金額小數位數的上限（外幣金額為 `NUMERIC(18,4)`，§8.1）。
pub const MAX_DECIMAL_PLACES: i16 = 4;

/// 驗證錯誤 `details.field` 的欄位名稱，與 request 的欄位名稱相同。
pub mod field {
    pub const CODE: &str = "code";
    pub const NAME: &str = "name";
    pub const SYMBOL: &str = "symbol";
    pub const DECIMAL_PLACES: &str = "decimal_places";
    pub const IS_ACTIVE: &str = "is_active";
}

/// 幣別。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct Currency {
    pub id: Id,
    /// ISO 4217 代碼（3 個大寫英文字母），建立後不可修改。其他資料以代碼參照幣別。
    pub code: String,
    /// 名稱，例如「新台幣」。
    pub name: String,
    /// 符號，例如 `NT$`；沒有時為 `null`。
    pub symbol: Option<String>,
    /// 金額的小數位數，0–4。
    pub decimal_places: i16,
    /// 本位幣（只有 TWD）。
    pub is_base: bool,
    /// `false` 代表已停用：不出現在幣別選單、不可用於新的資料，也不可新增或修改它的匯率。
    pub is_active: bool,
    /// 建立時間（RFC 3339）。
    #[serde(with = "time::serde::rfc3339")]
    #[specta(type = String)]
    pub created_at: OffsetDateTime,
    /// 最後修改時間（RFC 3339）。
    #[serde(with = "time::serde::rfc3339")]
    #[specta(type = String)]
    pub updated_at: OffsetDateTime,
    /// 樂觀鎖定版本號，修改時帶回（§9.4）。
    pub version: i32,
}

/// `GET /api/v1/currencies` 的 query string。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CurrencyListQuery {
    /// `true` 時包含已停用的幣別；預設只列出啟用中的。
    #[serde(default)]
    pub include_inactive: Option<bool>,
}

/// `POST /api/v1/currencies` 的輸入。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CreateCurrencyRequest {
    /// 3 個英文字母，小寫會轉成大寫；不可與既有幣別（含已停用）重複。
    pub code: String,
    /// 名稱（前後空白會去除），不可為空。
    pub name: String,
    /// 符號（前後空白會去除）；省略、`null` 或空字串代表沒有符號。
    #[serde(default)]
    pub symbol: Option<String>,
    /// 金額的小數位數，0–4。
    pub decimal_places: i16,
}

/// `PATCH /api/v1/currencies/{code}` 的輸入。省略或 `null` 的欄位不修改。
///
/// `code`、`is_base` 不可修改。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct UpdateCurrencyRequest {
    /// 讀取時取得的版本號；不符時回傳 `409 CONFLICT`。
    pub version: i32,
    #[serde(default)]
    pub name: Option<String>,
    /// 空字串代表清除符號。
    #[serde(default)]
    pub symbol: Option<String>,
    /// 只影響之後輸入的金額，已存的金額不變。
    #[serde(default)]
    pub decimal_places: Option<i16>,
    /// 本位幣不可停用。
    #[serde(default)]
    pub is_active: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use time::macros::datetime;

    #[test]
    fn currency_json_shape() {
        let id = crate::new_id();
        let currency = Currency {
            id,
            code: "USD".into(),
            name: "美元".into(),
            symbol: Some("US$".into()),
            decimal_places: 2,
            is_base: false,
            is_active: true,
            created_at: datetime!(2026-11-17 01:00 UTC),
            updated_at: datetime!(2026-11-17 01:00 UTC),
            version: 1,
        };
        let value = serde_json::to_value(&currency).unwrap();
        assert_eq!(
            value,
            json!({
                "id": id.to_string(),
                "code": "USD",
                "name": "美元",
                "symbol": "US$",
                "decimal_places": 2,
                "is_base": false,
                "is_active": true,
                "created_at": "2026-11-17T01:00:00Z",
                "updated_at": "2026-11-17T01:00:00Z",
                "version": 1,
            })
        );
        assert_eq!(serde_json::from_value::<Currency>(value).unwrap(), currency);
    }

    #[test]
    fn request_optional_fields() {
        let req: CreateCurrencyRequest =
            serde_json::from_value(json!({ "code": "gbp", "name": "英鎊", "decimal_places": 2 }))
                .unwrap();
        assert_eq!(req.symbol, None);
        let req: UpdateCurrencyRequest = serde_json::from_value(json!({ "version": 3 })).unwrap();
        assert_eq!(
            req,
            UpdateCurrencyRequest {
                version: 3,
                name: None,
                symbol: None,
                decimal_places: None,
                is_active: None,
            }
        );
        assert!(serde_json::from_value::<UpdateCurrencyRequest>(json!({})).is_err());
    }
}
