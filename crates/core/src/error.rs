//! `core` 的錯誤。
//!
//! 每個錯誤都對應一個 [`ErrorCode`]，由 `server` 轉成統一的 API 錯誤格式（§10）。

use crate::record_data::FieldError;
use crate::record_lines::LineError;
use crate::schema::RelationReference;
use crate::schema::formula::{FormulaError, FormulaErrorLocation};
use crate::schema::index::DuplicateValue;
use crate::schema::type_change::ConversionFailure;
use hammer_db::DbError;
use hammer_shared::document::{
    DocumentStatus, InvalidStatusDetails, LotInUseDetails, StockInsufficientDetails,
};
use hammer_shared::{ApiError, ErrorCode};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// [`CoreError::LastAdmin`] 在 API 回應 `details.reason` 的值。
pub const LAST_ADMIN_REASON: &str = "last_admin";

/// 商業邏輯層的錯誤。
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// 輸入資料不符合規則，訊息直接顯示給使用者（繁體中文）。
    #[error("{0}")]
    Validation(String),

    /// 某個欄位的輸入不符合規則（例如帳號重複）。API 回應的 `details.field` 為欄位名稱。
    #[error("{message}")]
    InvalidField {
        field: &'static str,
        message: String,
    },

    /// query string 的參數不合法（例如列表的篩選條件）。API 回應為 `VALIDATION`，
    /// `details.field` 為參數名稱（例如 `filter.qty.gte`）。
    #[error("{message}")]
    InvalidParam { param: String, message: String },

    /// 請求格式錯誤（例如 query string 的頁碼不是整數），訊息直接顯示給使用者（繁體中文）。
    #[error("{0}")]
    BadRequest(String),

    /// 上傳的內容超過大小上限（例如附件，§8.4），訊息直接顯示給使用者（繁體中文）。
    #[error("{0}")]
    PayloadTooLarge(String),

    /// 資料的多個欄位不符合欄位定義（[`crate::record_data`]）。
    /// API 回應為 `VALIDATION`，`details.fields` 為 `[{ "field_key", "message" }]`，列出所有出錯的欄位。
    #[error("資料有 {} 個欄位不正確，請修正後再送出", .0.len())]
    InvalidFields(Vec<FieldError>),

    /// 主表與明細一起存檔時，明細不合法（[`crate::record_lines`]）；`fields` 為主表的欄位錯誤
    /// （可能為空）。API 回應為 `VALIDATION`，`details` 為
    /// `{ "fields": [{ "field_key", "message" }], "lines": [{ "detail_key", "index", "field_key", "message" }] }`。
    #[error("資料有 {} 個錯誤，請修正後再送出", .fields.len() + .lines.len())]
    InvalidLines {
        fields: Vec<FieldError>,
        lines: Vec<LineError>,
    },

    /// 變更欄位型態時，有資料無法轉換（§7.4）。`failures` 最多
    /// [`hammer_shared::schema::MAX_CONVERSION_FAILURES`] 筆，`total` 為失敗總筆數。
    /// API 回應為 `VALIDATION`，`details` 為
    /// `{ "field": "data_type", "total", "records": [{ "id", "value", "deleted", "message" }] }`。
    #[error("有 {total} 筆資料無法轉換成新的資料型態")]
    ConversionFailed {
        failures: Vec<ConversionFailure>,
        total: usize,
    },

    /// 開啟「不可重複」（或重新啟用不可重複的欄位）時，未刪除的資料已有重複的值（§7.5）。
    /// `duplicates` 最多 [`hammer_shared::schema::MAX_DUPLICATE_VALUES`] 個值，`total` 為重複的值的總數。
    /// API 回應為 `VALIDATION`，`details` 為
    /// `{ "field": "is_unique", "total", "values": [{ "value", "count", "record_ids" }] }`。
    #[error("此欄位已有 {total} 個值重複，無法設定為不可重複，請先修正重複的資料")]
    DuplicateValues {
        duplicates: Vec<DuplicateValue>,
        total: usize,
    },

    /// 刪除資料類別時，其他有效資料類別的有效 `relation` 欄位還指向它（§8.5）。
    /// API 回應為 `VALIDATION`，`details` 為
    /// `{ "field": "relation_entity_id", "fields": [{ "entity_key", "entity_label", "field_key", "field_label" }] }`。
    #[error("此資料類別還有 {} 個關聯欄位引用，請先停用或刪除這些欄位", .0.len())]
    EntityReferenced(Vec<RelationReference>),

    /// 公式欄位的公式不合法：語法、引用的欄位、型態或循環引用（§7.6，[`crate::schema::formula`]）。
    /// API 回應為 `VALIDATION`，`details` 為 `{ "field": "formula", "span": { "start", "end" } }`
    /// （錯誤在公式中的字元位置，從 0 開始、不含 `end`）或
    /// `{ "field": "formula", "cycle": ["a", "lines.b", "a"] }`（循環路徑）。
    #[error("{}", .0.message)]
    InvalidFormula(FormulaError),

    /// 停用欄位、停用明細子表或刪除資料類別時，還有有效的公式欄位引用它（§7.6）。
    /// API 回應為 `VALIDATION`，`details` 為
    /// `{ "field": "formula", "formulas": [{ "entity_key", "entity_label", "field_key", "field_label" }] }`。
    #[error("還有 {} 個公式欄位引用，請先修改或停用這些公式欄位", .0.len())]
    FormulaReferenced(Vec<RelationReference>),

    /// 刪除基本資料時，還有其他資料使用它（§7.2，[`crate::master_data`]）：單位、分類還有未刪除的商品
    /// 使用，分類還有子分類，或商品已有庫存異動。`products`、`children`、`transactions` 為使用中的商品數、
    /// 子分類數、庫存異動筆數（至少一個大於 0）。API 回應為 `VALIDATION`，`details` 為
    /// `{ "fields": [], "in_use_count": 3, "child_count": 1, "transaction_count": 12 }`（只列出大於 0 的數量）。
    #[error("{message}")]
    RecordInUse {
        message: String,
        products: i64,
        children: i64,
        transactions: i64,
    },

    /// 單據目前的狀態不允許此操作（§7.1「系統單據」、§10「單據動作」）：修改、刪除非草稿，過帳非草稿，
    /// 作廢非已過帳。API 回應為 `INVALID_STATUS`，`details` 為 `{ "status": "posted" }`（目前狀態）。
    #[error("{message}")]
    InvalidStatus {
        status: DocumentStatus,
        message: String,
    },

    /// 過帳或作廢時批號或商品的數量不足（§10「單據動作」，[`crate::inventory`]）。
    /// API 回應為 `STOCK_INSUFFICIENT`，`details` 為 [`StockInsufficientDetails`]（列出所有不足的項目）。
    #[error("{message}")]
    StockInsufficient {
        message: String,
        details: StockInsufficientDetails,
    },

    /// 作廢時要刪除的批號已被其他有效單據使用（§10「單據動作」，[`crate::inventory`]）。
    /// API 回應為 `LOT_IN_USE`，`details` 為 [`LotInUseDetails`]。
    #[error("{message}")]
    LotInUse {
        message: String,
        details: LotInUseDetails,
    },

    /// 操作後系統沒有任何有效（啟用、未刪除）的管理者（§6.2）。
    /// API 回應為 `VALIDATION`，`details.reason` 為 [`LAST_ADMIN_REASON`]。
    #[error("系統至少需要保留一個有效的管理者，無法停用、刪除最後一個管理者或變更其角色")]
    LastAdmin,

    /// 找不到資料（不存在或已刪除），訊息直接顯示給使用者（繁體中文）。
    #[error("{0}")]
    NotFound(String),

    /// 未登入、token 無效、session 已失效，或使用者已停用、已刪除。
    ///
    /// 不區分原因，避免透露帳號狀態；原因只寫進 log。
    #[error("尚未登入或登入已失效，請重新登入")]
    Unauthorized,

    /// 已登入但沒有所需的權限。內容為權限代碼。
    #[error("沒有權限執行此操作")]
    Forbidden(&'static str),

    /// 必須先修改密碼（`users.must_change_password = true`，§6.2）。
    #[error("請先修改密碼")]
    PasswordChangeRequired,

    /// 登入時帳號不存在或密碼錯誤。兩者訊息相同，不透露帳號是否存在。
    #[error("帳號或密碼錯誤")]
    InvalidCredentials,

    /// 連續登入失敗，帳號鎖定到此時間（§6.2）。
    #[error("登入失敗次數過多，帳號已暫時鎖定，請於 {} 後再試", format_taipei_time(.0))]
    AccountLocked(OffsetDateTime),

    /// 帳號已停用。
    #[error("此帳號已停用，請聯絡管理者")]
    AccountDisabled,

    /// 操作與目前的資料狀態衝突，訊息直接顯示給使用者（繁體中文）。
    /// 例如已有帳號時再呼叫初次啟動建立管理者。
    #[error("{0}")]
    Conflict(String),

    /// 資料存取錯誤。
    #[error(transparent)]
    Db(#[from] DbError),

    /// 不應發生的內部錯誤（例如資料庫中的密碼雜湊格式損壞）。訊息只寫進 log。
    #[error("內部錯誤：{0}")]
    Internal(String),
}

impl CoreError {
    /// 對應的錯誤碼。
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Validation(_)
            | Self::InvalidField { .. }
            | Self::InvalidParam { .. }
            | Self::InvalidFields(_)
            | Self::InvalidLines { .. }
            | Self::ConversionFailed { .. }
            | Self::DuplicateValues { .. }
            | Self::EntityReferenced(_)
            | Self::InvalidFormula(_)
            | Self::FormulaReferenced(_)
            | Self::RecordInUse { .. }
            | Self::LastAdmin => ErrorCode::Validation,
            Self::InvalidStatus { .. } => ErrorCode::InvalidStatus,
            Self::StockInsufficient { .. } => ErrorCode::StockInsufficient,
            Self::LotInUse { .. } => ErrorCode::LotInUse,
            Self::BadRequest(_) => ErrorCode::BadRequest,
            Self::PayloadTooLarge(_) => ErrorCode::PayloadTooLarge,
            Self::NotFound(_) => ErrorCode::NotFound,
            Self::Unauthorized => ErrorCode::Unauthorized,
            Self::Forbidden(_) => ErrorCode::Forbidden,
            Self::PasswordChangeRequired => ErrorCode::PasswordChangeRequired,
            Self::InvalidCredentials => ErrorCode::Unauthorized,
            Self::AccountLocked(_) => ErrorCode::AccountLocked,
            Self::AccountDisabled => ErrorCode::AccountDisabled,
            Self::Conflict(_) | Self::Db(DbError::Conflict) => ErrorCode::Conflict,
            Self::Db(_) | Self::Internal(_) => ErrorCode::Internal,
        }
    }

    /// 把 [`CoreError::Validation`] 標上欄位名稱（轉成 [`CoreError::InvalidField`]）。
    /// 其他錯誤不變。
    pub fn with_field(self, field: &'static str) -> Self {
        match self {
            Self::Validation(message) => Self::InvalidField { field, message },
            other => other,
        }
    }

    /// 是否可以把訊息直接顯示給使用者。內部錯誤的細節不應回傳給客戶端。
    pub fn is_user_facing(&self) -> bool {
        !matches!(self.code(), ErrorCode::Internal)
    }
}

impl From<CoreError> for ApiError {
    /// 轉成 API 錯誤。內部錯誤只回傳通用訊息。
    fn from(err: CoreError) -> Self {
        let code = err.code();
        if let CoreError::InvalidField { field, message } = err {
            ApiError::new(code, message).with_details(serde_json::json!({ "field": field }))
        } else if let CoreError::InvalidParam { param, message } = err {
            ApiError::new(code, message).with_details(serde_json::json!({ "field": param }))
        } else if let CoreError::InvalidFields(errors) = &err {
            let fields: Vec<_> = errors
                .iter()
                .map(|e| serde_json::json!({ "field_key": e.field_key, "message": e.message }))
                .collect();
            ApiError::new(code, err.to_string())
                .with_details(serde_json::json!({ "fields": fields }))
        } else if let CoreError::InvalidLines { fields, lines } = &err {
            let fields: Vec<_> = fields
                .iter()
                .map(|e| serde_json::json!({ "field_key": e.field_key, "message": e.message }))
                .collect();
            let lines: Vec<_> = lines
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "detail_key": e.detail_key,
                        "index": e.index,
                        "field_key": e.field_key,
                        "message": e.message,
                    })
                })
                .collect();
            ApiError::new(code, err.to_string())
                .with_details(serde_json::json!({ "fields": fields, "lines": lines }))
        } else if let CoreError::ConversionFailed { failures, total } = &err {
            let records: Vec<_> = failures
                .iter()
                .map(|f| {
                    serde_json::json!({
                        "id": f.record_id,
                        "value": f.value,
                        "deleted": f.deleted,
                        "message": f.message,
                    })
                })
                .collect();
            ApiError::new(code, err.to_string()).with_details(serde_json::json!({
                "field": "data_type",
                "total": total,
                "records": records,
            }))
        } else if let CoreError::DuplicateValues { duplicates, total } = &err {
            let values: Vec<_> = duplicates
                .iter()
                .map(|d| {
                    serde_json::json!({
                        "value": d.value,
                        "count": d.count,
                        "record_ids": d.record_ids,
                    })
                })
                .collect();
            ApiError::new(code, err.to_string()).with_details(serde_json::json!({
                "field": "is_unique",
                "total": total,
                "values": values,
            }))
        } else if let CoreError::EntityReferenced(refs) = &err {
            let fields: Vec<_> = refs
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "entity_key": r.entity_key,
                        "entity_label": r.entity_label,
                        "field_key": r.field_key,
                        "field_label": r.field_label,
                    })
                })
                .collect();
            ApiError::new(code, err.to_string()).with_details(serde_json::json!({
                "field": "relation_entity_id",
                "fields": fields,
            }))
        } else if let CoreError::InvalidFormula(e) = &err {
            let details = match &e.location {
                FormulaErrorLocation::Span { start, end } => serde_json::json!({
                    "field": "formula",
                    "span": { "start": start, "end": end },
                }),
                FormulaErrorLocation::Cycle(path) => serde_json::json!({
                    "field": "formula",
                    "cycle": path,
                }),
            };
            ApiError::new(code, err.to_string()).with_details(details)
        } else if let CoreError::FormulaReferenced(refs) = &err {
            let formulas: Vec<_> = refs
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "entity_key": r.entity_key,
                        "entity_label": r.entity_label,
                        "field_key": r.field_key,
                        "field_label": r.field_label,
                    })
                })
                .collect();
            ApiError::new(code, err.to_string()).with_details(serde_json::json!({
                "field": "formula",
                "formulas": formulas,
            }))
        } else if let CoreError::RecordInUse {
            products,
            children,
            transactions,
            ..
        } = &err
        {
            let mut details = serde_json::json!({ "fields": [] });
            if *products > 0 {
                details["in_use_count"] = serde_json::json!(products);
            }
            if *children > 0 {
                details["child_count"] = serde_json::json!(children);
            }
            if *transactions > 0 {
                details["transaction_count"] = serde_json::json!(transactions);
            }
            ApiError::new(code, err.to_string()).with_details(details)
        } else if let CoreError::InvalidStatus { status, .. } = &err {
            let details = InvalidStatusDetails { status: *status };
            ApiError::new(code, err.to_string())
                .with_details(serde_json::to_value(details).unwrap_or(serde_json::Value::Null))
        } else if let CoreError::StockInsufficient { details, .. } = &err {
            ApiError::new(code, err.to_string())
                .with_details(serde_json::to_value(details).unwrap_or(serde_json::Value::Null))
        } else if let CoreError::LotInUse { details, .. } = &err {
            ApiError::new(code, err.to_string())
                .with_details(serde_json::to_value(details).unwrap_or(serde_json::Value::Null))
        } else if let CoreError::LastAdmin = err {
            ApiError::new(code, err.to_string())
                .with_details(serde_json::json!({ "reason": LAST_ADMIN_REASON }))
        } else if let CoreError::Forbidden(permission) = err {
            ApiError::new(code, err.to_string())
                .with_details(serde_json::json!({ "permission": permission }))
        } else if let CoreError::AccountLocked(until) = err {
            let locked_until = until.format(&Rfc3339).unwrap_or_default();
            ApiError::new(code, err.to_string())
                .with_details(serde_json::json!({ "locked_until": locked_until }))
        } else if err.is_user_facing() {
            ApiError::new(code, err.to_string())
        } else {
            ApiError::new(code, "伺服器發生錯誤，請稍後再試")
        }
    }
}

/// 以台北時間顯示 `HH:MM`（D13）。
fn format_taipei_time(at: &OffsetDateTime) -> String {
    let t = at.to_offset(hammer_shared::time::TAIPEI_OFFSET);
    format!("{:02}:{:02}", t.hour(), t.minute())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_maps_to_validation_code() {
        let err = CoreError::Validation("密碼至少需要 10 個字元".into());
        assert_eq!(err.code(), ErrorCode::Validation);
        let api: ApiError = err.into();
        assert_eq!(api.code, ErrorCode::Validation);
        assert_eq!(api.message, "密碼至少需要 10 個字元");
    }

    #[test]
    fn invalid_status_reports_current_status() {
        let err = CoreError::InvalidStatus {
            status: DocumentStatus::Posted,
            message: "已過帳的單據不可修改".into(),
        };
        assert_eq!(err.code(), ErrorCode::InvalidStatus);
        let api: ApiError = err.into();
        assert_eq!(api.message, "已過帳的單據不可修改");
        assert_eq!(api.details, Some(serde_json::json!({ "status": "posted" })));
    }

    #[test]
    fn stock_errors_serialize_details() {
        let err = CoreError::StockInsufficient {
            message: "商品 A001 庫存不足，尚缺 1".into(),
            details: StockInsufficientDetails { items: vec![] },
        };
        assert_eq!(err.code(), ErrorCode::StockInsufficient);
        let api: ApiError = err.into();
        assert_eq!(api.message, "商品 A001 庫存不足，尚缺 1");
        assert_eq!(api.details, Some(serde_json::json!({ "items": [] })));

        let err = CoreError::LotInUse {
            message: "批號 L-01 已被其他單據使用".into(),
            details: LotInUseDetails { lots: vec![] },
        };
        assert_eq!(err.code(), ErrorCode::LotInUse);
        let api: ApiError = err.into();
        assert_eq!(api.details, Some(serde_json::json!({ "lots": [] })));
    }

    #[test]
    fn record_in_use_lists_counts_in_details() {
        let err = CoreError::RecordInUse {
            message: "此單位還有 3 筆商品使用中，無法刪除".into(),
            products: 3,
            children: 0,
            transactions: 0,
        };
        assert_eq!(err.code(), ErrorCode::Validation);
        let api: ApiError = err.into();
        assert_eq!(api.message, "此單位還有 3 筆商品使用中，無法刪除");
        assert_eq!(
            api.details,
            Some(serde_json::json!({ "fields": [], "in_use_count": 3 }))
        );

        let api: ApiError = CoreError::RecordInUse {
            message: String::new(),
            products: 0,
            children: 2,
            transactions: 0,
        }
        .into();
        assert_eq!(
            api.details,
            Some(serde_json::json!({ "fields": [], "child_count": 2 }))
        );

        let api: ApiError = CoreError::RecordInUse {
            message: String::new(),
            products: 0,
            children: 0,
            transactions: 12,
        }
        .into();
        assert_eq!(
            api.details,
            Some(serde_json::json!({ "fields": [], "transaction_count": 12 }))
        );
    }

    #[test]
    fn invalid_fields_lists_all_fields_in_details() {
        let err = CoreError::InvalidFields(vec![
            FieldError {
                field_key: "name".into(),
                message: "此欄位為必填".into(),
            },
            FieldError {
                field_key: "qty".into(),
                message: "不可小於 1".into(),
            },
        ]);
        assert_eq!(err.code(), ErrorCode::Validation);
        let api: ApiError = err.into();
        assert_eq!(api.message, "資料有 2 個欄位不正確，請修正後再送出");
        assert_eq!(
            api.details,
            Some(serde_json::json!({ "fields": [
                { "field_key": "name", "message": "此欄位為必填" },
                { "field_key": "qty", "message": "不可小於 1" },
            ] }))
        );
    }

    #[test]
    fn entity_referenced_lists_fields_in_details() {
        let err = CoreError::EntityReferenced(vec![RelationReference {
            entity_key: "repair_order".into(),
            entity_label: "維修單".into(),
            field_key: "customer".into(),
            field_label: "客戶".into(),
        }]);
        assert_eq!(err.code(), ErrorCode::Validation);
        let api: ApiError = err.into();
        assert_eq!(
            api.message,
            "此資料類別還有 1 個關聯欄位引用，請先停用或刪除這些欄位"
        );
        assert_eq!(
            api.details,
            Some(serde_json::json!({
                "field": "relation_entity_id",
                "fields": [{
                    "entity_key": "repair_order",
                    "entity_label": "維修單",
                    "field_key": "customer",
                    "field_label": "客戶",
                }],
            }))
        );
    }

    #[test]
    fn invalid_lines_lists_rows_and_fields_in_details() {
        let err = CoreError::InvalidLines {
            fields: vec![FieldError {
                field_key: "name".into(),
                message: "此欄位為必填".into(),
            }],
            lines: vec![
                LineError {
                    detail_key: "parts".into(),
                    index: Some(1),
                    field_key: Some("qty".into()),
                    message: "必須是整數".into(),
                },
                LineError {
                    detail_key: "missing".into(),
                    index: None,
                    field_key: None,
                    message: "沒有此明細子表".into(),
                },
            ],
        };
        assert_eq!(err.code(), ErrorCode::Validation);
        let api: ApiError = err.into();
        assert_eq!(api.message, "資料有 3 個錯誤，請修正後再送出");
        assert_eq!(
            api.details,
            Some(serde_json::json!({
                "fields": [{ "field_key": "name", "message": "此欄位為必填" }],
                "lines": [
                    { "detail_key": "parts", "index": 1, "field_key": "qty", "message": "必須是整數" },
                    { "detail_key": "missing", "index": null, "field_key": null, "message": "沒有此明細子表" },
                ],
            }))
        );
    }

    #[test]
    fn invalid_param_and_bad_request() {
        let api: ApiError = CoreError::InvalidParam {
            param: "filter.qty.gte".into(),
            message: "必須是整數".into(),
        }
        .into();
        assert_eq!(api.code, ErrorCode::Validation);
        assert_eq!(api.message, "必須是整數");
        assert_eq!(
            api.details,
            Some(serde_json::json!({ "field": "filter.qty.gte" }))
        );

        let api: ApiError = CoreError::BadRequest("page 必須是整數".into()).into();
        assert_eq!(api.code, ErrorCode::BadRequest);
        assert_eq!(api.message, "page 必須是整數");
        assert_eq!(api.details, None);
    }

    #[test]
    fn conversion_failed_lists_records_and_total() {
        let id = hammer_shared::new_id();
        let err = CoreError::ConversionFailed {
            failures: vec![ConversionFailure {
                record_id: id,
                value: serde_json::json!("abc"),
                deleted: true,
                message: "「abc」不是整數".into(),
            }],
            total: 150,
        };
        assert_eq!(err.code(), ErrorCode::Validation);
        let api: ApiError = err.into();
        assert_eq!(api.message, "有 150 筆資料無法轉換成新的資料型態");
        assert_eq!(
            api.details,
            Some(serde_json::json!({
                "field": "data_type",
                "total": 150,
                "records": [
                    { "id": id, "value": "abc", "deleted": true, "message": "「abc」不是整數" },
                ],
            }))
        );
    }

    #[test]
    fn duplicate_values_list_values_and_records() {
        let a = hammer_shared::new_id();
        let b = hammer_shared::new_id();
        let err = CoreError::DuplicateValues {
            duplicates: vec![DuplicateValue {
                value: serde_json::json!("A001"),
                count: 2,
                record_ids: vec![a, b],
            }],
            total: 1,
        };
        assert_eq!(err.code(), ErrorCode::Validation);
        let api: ApiError = err.into();
        assert!(api.message.contains("1 個值重複"), "{}", api.message);
        assert_eq!(
            api.details,
            Some(serde_json::json!({
                "field": "is_unique",
                "total": 1,
                "values": [{ "value": "A001", "count": 2, "record_ids": [a, b] }],
            }))
        );
    }

    #[test]
    fn db_conflict_maps_to_conflict() {
        let err = CoreError::from(DbError::Conflict);
        assert_eq!(err.code(), ErrorCode::Conflict);
        assert!(err.is_user_facing());
    }

    #[test]
    fn internal_hides_details() {
        let err = CoreError::Internal("密碼雜湊格式錯誤".into());
        assert_eq!(err.code(), ErrorCode::Internal);
        let api: ApiError = err.into();
        assert_eq!(api.code, ErrorCode::Internal);
        assert!(!api.message.contains("雜湊"));
    }

    #[test]
    fn auth_errors_map_to_codes() {
        let cases = [
            (CoreError::Unauthorized, ErrorCode::Unauthorized),
            (CoreError::Forbidden("record.write"), ErrorCode::Forbidden),
            (
                CoreError::PasswordChangeRequired,
                ErrorCode::PasswordChangeRequired,
            ),
        ];
        for (err, code) in cases {
            assert_eq!(err.code(), code);
            assert!(err.is_user_facing());
            let message = err.to_string();
            let api: ApiError = err.into();
            assert_eq!(api.code, code);
            assert_eq!(api.message, message);
        }
    }

    #[test]
    fn forbidden_includes_permission_in_details() {
        let api: ApiError = CoreError::Forbidden("record.write").into();
        assert_eq!(
            api.details,
            Some(serde_json::json!({ "permission": "record.write" }))
        );
    }

    #[test]
    fn login_errors_map_to_codes() {
        let api: ApiError = CoreError::InvalidCredentials.into();
        assert_eq!(api.code, ErrorCode::Unauthorized);
        assert_eq!(api.message, "帳號或密碼錯誤");
        assert_eq!(api.details, None);

        let api: ApiError = CoreError::AccountDisabled.into();
        assert_eq!(api.code, ErrorCode::AccountDisabled);
    }

    #[test]
    fn account_locked_includes_unlock_time() {
        let until = time::macros::datetime!(2026-11-13 01:05:30 UTC);
        let api: ApiError = CoreError::AccountLocked(until).into();
        assert_eq!(api.code, ErrorCode::AccountLocked);
        // 訊息以台北時間顯示
        assert!(api.message.contains("09:05"), "{}", api.message);
        assert_eq!(
            api.details,
            Some(serde_json::json!({ "locked_until": "2026-11-13T01:05:30Z" }))
        );
    }

    #[test]
    fn conflict_keeps_message() {
        let api: ApiError = CoreError::Conflict("系統已完成初始設定".into()).into();
        assert_eq!(api.code, ErrorCode::Conflict);
        assert_eq!(api.message, "系統已完成初始設定");
        assert_eq!(api.details, None);
    }

    #[test]
    fn invalid_field_includes_field_in_details() {
        let err = CoreError::Validation("此帳號已被使用".into()).with_field("username");
        assert_eq!(err.code(), ErrorCode::Validation);
        let api: ApiError = err.into();
        assert_eq!(api.code, ErrorCode::Validation);
        assert_eq!(api.message, "此帳號已被使用");
        assert_eq!(
            api.details,
            Some(serde_json::json!({ "field": "username" }))
        );
    }

    #[test]
    fn with_field_keeps_other_errors() {
        let err = CoreError::Unauthorized.with_field("username");
        assert!(matches!(err, CoreError::Unauthorized));
    }

    #[test]
    fn last_admin_is_validation_with_reason() {
        let api: ApiError = CoreError::LastAdmin.into();
        assert_eq!(api.code, ErrorCode::Validation);
        assert!(api.message.contains("管理者"));
        assert_eq!(
            api.details,
            Some(serde_json::json!({ "reason": "last_admin" }))
        );
    }

    #[test]
    fn not_found_keeps_message() {
        let err = CoreError::NotFound("找不到此帳號".into());
        assert_eq!(err.code(), ErrorCode::NotFound);
        let api: ApiError = err.into();
        assert_eq!(api.code, ErrorCode::NotFound);
        assert_eq!(api.message, "找不到此帳號");
    }

    #[test]
    fn other_db_errors_are_internal() {
        let err = CoreError::from(DbError::Sqlx(hammer_db::sqlx::Error::RowNotFound));
        assert_eq!(err.code(), ErrorCode::Internal);
        assert!(!err.is_user_facing());
    }

    #[test]
    fn invalid_formula_details() {
        let api: ApiError = CoreError::InvalidFormula(FormulaError {
            message: "第 1 個字元：找不到欄位「x」".into(),
            location: FormulaErrorLocation::Span { start: 0, end: 3 },
        })
        .into();
        assert_eq!(api.code, ErrorCode::Validation);
        assert_eq!(api.message, "第 1 個字元：找不到欄位「x」");
        assert_eq!(
            api.details,
            Some(serde_json::json!({ "field": "formula", "span": { "start": 0, "end": 3 } }))
        );

        let api: ApiError = CoreError::InvalidFormula(FormulaError {
            message: "公式有循環引用：a → b → a".into(),
            location: FormulaErrorLocation::Cycle(vec!["a".into(), "b".into(), "a".into()]),
        })
        .into();
        assert_eq!(
            api.details,
            Some(serde_json::json!({ "field": "formula", "cycle": ["a", "b", "a"] }))
        );
    }

    #[test]
    fn formula_referenced_lists_formulas() {
        let api: ApiError = CoreError::FormulaReferenced(vec![RelationReference {
            entity_key: "repair_order".into(),
            entity_label: "維修單".into(),
            field_key: "total".into(),
            field_label: "總計".into(),
        }])
        .into();
        assert_eq!(api.code, ErrorCode::Validation);
        assert!(api.message.contains("1 個公式欄位"), "{}", api.message);
        assert_eq!(
            api.details,
            Some(serde_json::json!({
                "field": "formula",
                "formulas": [{
                    "entity_key": "repair_order",
                    "entity_label": "維修單",
                    "field_key": "total",
                    "field_label": "總計",
                }],
            }))
        );
    }
}
