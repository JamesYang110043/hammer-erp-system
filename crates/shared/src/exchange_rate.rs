//! 匯率的 request / response（docs/architecture.md §6.1、§8.6 `exchange_rates`）。
//!
//! 匯率不是資料類別，使用專屬端點（§10）：
//!
//! | 端點 | 權限 | request | response |
//! |---|---|---|---|
//! | `GET /api/v1/exchange-rates` | `record.read` | [`ExchangeRateQuery`]（query string） | [`Page`](crate::pagination::Page)`<`[`ExchangeRate`]`>` |
//! | `GET /api/v1/exchange-rates/effective` | `record.read` | [`EffectiveRateQuery`]（query string） | [`EffectiveRate`] |
//! | `POST /api/v1/exchange-rates` | `record.write` | [`CreateExchangeRateRequest`] | `201` [`ExchangeRate`] |
//! | `PATCH /api/v1/exchange-rates/{id}` | `record.write` | [`UpdateExchangeRateRequest`] | [`ExchangeRate`] |
//! | `DELETE /api/v1/exchange-rates/{id}?version=N` | `record.write` | [`DeleteExchangeRateQuery`] | `204 No Content` |
//!
//! 匯率為「1 外幣 = ? 台幣」，`NUMERIC(18,6)`，JSON 中是字串（D9）。日期為 `YYYY-MM-DD`（Asia/Taipei）。
//!
//! 錯誤：
//!
//! - 輸入不符合規則（幣別不存在、已停用或為本位幣、匯率不大於 0 或超過 6 位小數、
//!   同一幣別同一天已有匯率等）→ `422 VALIDATION`，`details.field` 為欄位或參數名稱
//! - query string 格式錯誤（日期不是 `YYYY-MM-DD`、頁碼不是整數）→ `400 BAD_REQUEST`
//! - `version` 不符 → `409 CONFLICT`（§9.4）
//! - 匯率不存在或已刪除 → `404 NOT_FOUND`

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use specta::Type;
use time::{Date, OffsetDateTime};

use crate::Id;

/// 匯率的小數位數上限（`NUMERIC(18,6)`）。
pub const RATE_SCALE: u32 = 6;

/// 驗證錯誤 `details.field` 的欄位或參數名稱，與 request 的名稱相同。
pub mod field {
    pub const CURRENCY_CODE: &str = "currency_code";
    pub const RATE_DATE: &str = "rate_date";
    pub const RATE: &str = "rate";
    pub const DATE_FROM: &str = "date_from";
    pub const DATE: &str = "date";
}

/// 匯率。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ExchangeRate {
    pub id: Id,
    /// 幣別代碼。
    pub currency_code: String,
    /// 匯率適用的日期（`YYYY-MM-DD`）。
    #[serde(with = "crate::time::date_str")]
    #[specta(type = String)]
    pub rate_date: Date,
    /// 1 外幣 = ? 台幣，固定 6 位小數，例如 `"32.150000"`。
    #[serde(with = "crate::decimal::serde_str")]
    #[specta(type = String)]
    pub rate: Decimal,
    /// 建立時間（RFC 3339）。
    #[serde(with = "time::serde::rfc3339")]
    #[specta(type = String)]
    pub created_at: OffsetDateTime,
    /// 最後修改時間（RFC 3339）。
    #[serde(with = "time::serde::rfc3339")]
    #[specta(type = String)]
    pub updated_at: OffsetDateTime,
    /// 樂觀鎖定版本號，修改、刪除時帶回（§9.4）。
    pub version: i32,
}

/// `GET /api/v1/exchange-rates` 的 query string。所有條件都可省略。
///
/// 只列出未刪除的匯率，依 `rate_date` 由新到舊、再依幣別、`id` 排序。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ExchangeRateQuery {
    /// 幣別代碼（不分大小寫）；空字串視為未指定。
    #[serde(default)]
    pub currency_code: Option<String>,
    /// `rate_date >= date_from`（含）。
    #[serde(with = "crate::time::date_str::option", default)]
    #[specta(type = Option<String>)]
    pub date_from: Option<Date>,
    /// `rate_date <= date_to`（含）。
    #[serde(with = "crate::time::date_str::option", default)]
    #[specta(type = Option<String>)]
    pub date_to: Option<Date>,
    /// 頁碼，從 1 開始（預設 1）。
    #[serde(default)]
    pub page: Option<u32>,
    /// 每頁筆數（預設 50，上限 200）。
    #[serde(default)]
    pub page_size: Option<u32>,
}

/// `POST /api/v1/exchange-rates` 的輸入。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CreateExchangeRateRequest {
    /// 幣別代碼（不分大小寫）：必須存在、啟用中、不是本位幣。
    pub currency_code: String,
    /// 適用日期；同一幣別同一天只能有一筆未刪除的匯率。可以是未來的日期。
    #[serde(with = "crate::time::date_str")]
    #[specta(type = String)]
    pub rate_date: Date,
    /// 大於 0，最多 6 位小數（不自動四捨五入）。
    #[serde(with = "crate::decimal::serde_str")]
    #[specta(type = String)]
    pub rate: Decimal,
}

/// `PATCH /api/v1/exchange-rates/{id}` 的輸入。只能修改匯率；要換日期或幣別時刪除後重新新增。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct UpdateExchangeRateRequest {
    /// 讀取時取得的版本號；不符時回傳 `409 CONFLICT`。
    pub version: i32,
    /// 大於 0，最多 6 位小數。
    #[serde(with = "crate::decimal::serde_str")]
    #[specta(type = String)]
    pub rate: Decimal,
}

/// `DELETE /api/v1/exchange-rates/{id}` 的 query string。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct DeleteExchangeRateQuery {
    /// 讀取時取得的版本號；不符時回傳 `409 CONFLICT`。
    pub version: i32,
}

/// `GET /api/v1/exchange-rates/effective` 的 query string。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct EffectiveRateQuery {
    /// 幣別代碼（不分大小寫）。已停用的幣別仍可查詢。
    pub currency_code: String,
    /// 單據日期（`YYYY-MM-DD`）。
    #[serde(with = "crate::time::date_str")]
    #[specta(type = String)]
    pub date: Date,
}

/// 適用的匯率：`rate_date` 小於或等於 `date`、未刪除的匯率中日期最近的一筆。
///
/// - 找不到時 `rate`、`rate_date` 為 `null`（不是錯誤，單據的匯率欄位留空讓使用者輸入）
/// - 本位幣固定 `rate = "1.000000"`、`rate_date = null`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct EffectiveRate {
    /// 幣別代碼（大寫）。
    pub currency_code: String,
    /// 查詢的單據日期。
    #[serde(with = "crate::time::date_str")]
    #[specta(type = String)]
    pub date: Date,
    #[serde(with = "crate::decimal::serde_str_option", default)]
    #[specta(type = Option<String>)]
    pub rate: Option<Decimal>,
    /// 採用的匯率的日期。
    #[serde(with = "crate::time::date_str::option", default)]
    #[specta(type = Option<String>)]
    pub rate_date: Option<Date>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::str::FromStr;
    use time::macros::date;

    #[test]
    fn effective_rate_json_shape() {
        let found = EffectiveRate {
            currency_code: "USD".into(),
            date: date!(2026 - 11 - 17),
            rate: Some(Decimal::from_str("32.150000").unwrap()),
            rate_date: Some(date!(2026 - 11 - 15)),
        };
        assert_eq!(
            serde_json::to_value(&found).unwrap(),
            json!({
                "currency_code": "USD",
                "date": "2026-11-17",
                "rate": "32.150000",
                "rate_date": "2026-11-15",
            })
        );
        let missing = EffectiveRate {
            rate: None,
            rate_date: None,
            ..found
        };
        let value = serde_json::to_value(&missing).unwrap();
        assert_eq!(value["rate"], json!(null));
        assert_eq!(value["rate_date"], json!(null));
        assert_eq!(
            serde_json::from_value::<EffectiveRate>(value).unwrap(),
            missing
        );
    }

    #[test]
    fn create_request_requires_string_rate() {
        let req: CreateExchangeRateRequest = serde_json::from_value(
            json!({ "currency_code": "USD", "rate_date": "2026-11-17", "rate": "32.15" }),
        )
        .unwrap();
        assert_eq!(req.rate_date, date!(2026 - 11 - 17));
        assert_eq!(req.rate.to_string(), "32.15");
        assert!(
            serde_json::from_value::<CreateExchangeRateRequest>(
                json!({ "currency_code": "USD", "rate_date": "2026-11-17", "rate": 32.15 })
            )
            .is_err()
        );
    }
}
