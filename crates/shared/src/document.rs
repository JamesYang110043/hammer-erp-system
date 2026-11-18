//! 系統單據的共通型別（`docs/architecture.md` §7.1「系統單據」、§8.2、§10「單據動作」）。
//!
//! 系統單據是 `kind = document` 的系統資料類別（例如盤點調整單 `stock_adjustment`）。一般的新增、修改、
//! 刪除使用通用資料 API（[`crate::record`]），過帳與作廢使用專屬端點：
//!
//! | 動作 | 端點 | request | 成功回應 | 狀態 |
//! |---|---|---|---|---|
//! | 過帳 | `POST /api/v1/entities/{key}/records/{id}/post` | [`DocumentActionRequest`] | `200` [`Record`](crate::record::Record) | `draft` → `posted` |
//! | 作廢 | `POST /api/v1/entities/{key}/records/{id}/void` | [`DocumentActionRequest`] | `200` [`Record`](crate::record::Record) | `posted` → `void` |
//!
//! - 權限：`record.post`
//! - request 只有 `version`（樂觀鎖定，§9.4），不接受其他內容；要修改單據請先用通用資料 API 存檔
//! - 回應與單筆查詢相同（含 `lines` 與 [`Record::document`](crate::record::Record::document)），
//!   `version` 已 + 1
//!
//! # 單號與系統維護的欄位
//!
//! - 單號 `record_no`：新增時依資料類別的單號規則產生，不可由客戶端指定或修改（規則同 [`crate::record`]）
//! - `status`（[`DocumentStatus`]）與金額合計是唯讀的系統欄位，出現在 `data`；request 的 `data` 帶了
//!   **一律忽略**（不回傳錯誤）。新增時 `status = draft`
//! - `posted_at`、`posted_by`、`voided_at`、`voided_by` 不是欄位，放在回應的
//!   [`Record::document`](crate::record::Record::document)（[`DocumentInfo`]）；request 的 `data` 帶了
//!   視為未定義的欄位（`422 VALIDATION`）
//!
//! # 錯誤
//!
//! 檢查順序：權限（`403`）→ 資料存在（`404`）→ `version`（`409 CONFLICT`）→ 狀態
//! （`422 INVALID_STATUS`）→ 內容與庫存（`422 VALIDATION` / `STOCK_INSUFFICIENT` / `LOT_IN_USE`）。
//! 任何錯誤都整張 rollback，單據維持原狀態。
//!
//! - 不支援此動作的資料類別（主檔、自訂資料類別、明細子表、不存在的代碼）、資料不存在或已刪除
//!   → `404 NOT_FOUND`
//! - 非 `draft` 的單據修改、刪除，過帳非 `draft`、作廢非 `posted` → `422 INVALID_STATUS`，
//!   `details` 為 [`InvalidStatusDetails`]
//! - 庫存不足 → `422 STOCK_INSUFFICIENT`，`details` 為 [`StockInsufficientDetails`]
//! - 作廢時要刪除的批號已被其他有效單據使用 → `422 LOT_IN_USE`，`details` 為 [`LotInUseDetails`]

use serde::{Deserialize, Serialize};
use specta::Type;
use time::OffsetDateTime;

use crate::Id;
use crate::decimal::Decimal;

/// 單據狀態的系統欄位代碼（`data.status`）。
pub const STATUS_FIELD: &str = "status";

/// 單據的狀態（§8.2）。JSON 中為 `draft` / `posted` / `void`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum DocumentStatus {
    /// 草稿：可以修改、刪除、過帳。
    Draft,
    /// 已過帳：不可修改、刪除；可以作廢。
    Posted,
    /// 已作廢：不可再做任何操作。
    Void,
}

impl DocumentStatus {
    /// 資料庫與 JSON 中的值。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Posted => "posted",
            Self::Void => "void",
        }
    }

    /// 由資料庫的值解析；不認得的值回傳 `None`。
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "draft" => Some(Self::Draft),
            "posted" => Some(Self::Posted),
            "void" => Some(Self::Void),
            _ => None,
        }
    }

    /// 顯示名稱（繁體中文）。
    pub fn label(self) -> &'static str {
        match self {
            Self::Draft => "草稿",
            Self::Posted => "已過帳",
            Self::Void => "已作廢",
        }
    }
}

/// 單據動作（§10「單據動作」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum DocumentAction {
    /// 過帳：`draft` → `posted`。
    Post,
    /// 作廢：`posted` → `void`。
    Void,
}

impl DocumentAction {
    /// 端點路徑的最後一段，也是操作紀錄的 `action`（[`crate::audit::POST`]、[`crate::audit::VOID`]）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Post => "post",
            Self::Void => "void",
        }
    }

    /// 執行此動作前必須是的狀態。
    pub fn from_status(self) -> DocumentStatus {
        match self {
            Self::Post => DocumentStatus::Draft,
            Self::Void => DocumentStatus::Posted,
        }
    }

    /// 執行此動作後的狀態。
    pub fn to_status(self) -> DocumentStatus {
        match self {
            Self::Post => DocumentStatus::Posted,
            Self::Void => DocumentStatus::Void,
        }
    }

    /// 顯示名稱（繁體中文）。
    pub fn label(self) -> &'static str {
        match self {
            Self::Post => "過帳",
            Self::Void => "作廢",
        }
    }
}

/// 過帳、作廢的 request。只有 `version`，其他內容一律拒絕（`deny_unknown_fields`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct DocumentActionRequest {
    /// 目前的版本號（樂觀鎖定，§9.4）。
    pub version: i32,
}

/// 單據主表的過帳、作廢資訊（[`Record::document`](crate::record::Record::document)）。
/// 這些值不是欄位，不出現在 `data`。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct DocumentInfo {
    /// 過帳時間（RFC 3339）；草稿為 `null`。
    #[serde(with = "time::serde::rfc3339::option", default)]
    #[specta(type = Option<String>)]
    pub posted_at: Option<OffsetDateTime>,
    /// 過帳人員。
    pub posted_by: Option<Id>,
    /// 作廢時間（RFC 3339）；未作廢為 `null`。
    #[serde(with = "time::serde::rfc3339::option", default)]
    #[specta(type = Option<String>)]
    pub voided_at: Option<OffsetDateTime>,
    /// 作廢人員。
    pub voided_by: Option<Id>,
}

/// `INVALID_STATUS` 的 `details`：單據目前的狀態。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct InvalidStatusDetails {
    pub status: DocumentStatus,
}

/// 明細的位置（錯誤 `details` 中指出是哪一列）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct LineRef {
    /// 明細子表代碼。
    pub detail_key: String,
    /// 該列依 `sort_order` 的位置，從 0 開始。
    pub index: u32,
}

/// `STOCK_INSUFFICIENT` 的 `details`：所有不足的項目。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct StockInsufficientDetails {
    pub items: Vec<StockShortage>,
}

/// 一個不足的批號（指定批號）或商品（先進先出，`lot_id`、`lot_no` 為 `null`）。
/// 同一個批號（商品）出現在多列明細時合併成一項，`required` 為合計。數量為補齊 4 位小數的字串。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct StockShortage {
    pub product_id: Id,
    pub product_code: String,
    pub product_name: String,
    pub lot_id: Option<Id>,
    pub lot_no: Option<String>,
    /// 需要的數量（合計）。
    #[serde(with = "crate::decimal::serde_str")]
    #[specta(type = String)]
    pub required: Decimal,
    /// 目前可用的數量。
    #[serde(with = "crate::decimal::serde_str")]
    #[specta(type = String)]
    pub available: Decimal,
    /// 尚缺的數量（`required − available`）。
    #[serde(with = "crate::decimal::serde_str")]
    #[specta(type = String)]
    pub shortage: Decimal,
    /// 用到此批號（商品）的明細。
    pub lines: Vec<LineRef>,
}

/// `LOT_IN_USE` 的 `details`：被其他有效單據使用的批號。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct LotInUseDetails {
    pub lots: Vec<LotInUse>,
}

/// 一個被其他有效單據使用的批號。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct LotInUse {
    pub lot_id: Id,
    pub lot_no: String,
    pub product_id: Id,
    pub product_code: String,
    /// 使用此批號、且尚未作廢的其他單據。
    pub documents: Vec<DocumentRef>,
}

/// 指向一張單據。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct DocumentRef {
    /// 單據的資料類別代碼。
    pub entity_key: String,
    pub record_id: Id,
    pub record_no: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::str::FromStr;

    #[test]
    fn status_round_trips() {
        for status in [
            DocumentStatus::Draft,
            DocumentStatus::Posted,
            DocumentStatus::Void,
        ] {
            assert_eq!(DocumentStatus::parse(status.as_str()), Some(status));
            assert_eq!(
                serde_json::to_value(status).unwrap(),
                json!(status.as_str())
            );
        }
        assert_eq!(DocumentStatus::parse("closed"), None);
        assert_eq!(DocumentStatus::Posted.label(), "已過帳");
        assert_eq!(DocumentStatus::Void.label(), "已作廢");
        assert_eq!(DocumentStatus::Draft.label(), "草稿");
    }

    #[test]
    fn actions_move_between_statuses() {
        assert_eq!(DocumentAction::Post.from_status(), DocumentStatus::Draft);
        assert_eq!(DocumentAction::Post.to_status(), DocumentStatus::Posted);
        assert_eq!(DocumentAction::Void.from_status(), DocumentStatus::Posted);
        assert_eq!(DocumentAction::Void.to_status(), DocumentStatus::Void);
        assert_eq!(DocumentAction::Post.as_str(), crate::audit::POST);
        assert_eq!(DocumentAction::Void.as_str(), crate::audit::VOID);
        assert_eq!(DocumentAction::Post.label(), "過帳");
        assert_eq!(DocumentAction::Void.label(), "作廢");
    }

    #[test]
    fn action_request_accepts_only_version() {
        let req: DocumentActionRequest = serde_json::from_value(json!({ "version": 3 })).unwrap();
        assert_eq!(req.version, 3);
        assert!(
            serde_json::from_value::<DocumentActionRequest>(json!({ "version": 3, "data": {} }))
                .is_err()
        );
        assert!(serde_json::from_value::<DocumentActionRequest>(json!({})).is_err());
    }

    #[test]
    fn document_info_serializes_nulls() {
        assert_eq!(
            serde_json::to_value(DocumentInfo::default()).unwrap(),
            json!({ "posted_at": null, "posted_by": null, "voided_at": null, "voided_by": null })
        );
        let at = time::macros::datetime!(2026-11-18 02:03:04 UTC);
        let info = DocumentInfo {
            posted_at: Some(at),
            posted_by: Some(Id::nil()),
            ..Default::default()
        };
        let value = serde_json::to_value(&info).unwrap();
        assert_eq!(value["posted_at"], json!("2026-11-18T02:03:04Z"));
        assert_eq!(serde_json::from_value::<DocumentInfo>(value).unwrap(), info);
    }

    #[test]
    fn error_details_shapes() {
        let details = StockInsufficientDetails {
            items: vec![StockShortage {
                product_id: Id::nil(),
                product_code: "A001".into(),
                product_name: "螺絲".into(),
                lot_id: None,
                lot_no: None,
                required: Decimal::from_str("10.0000").unwrap(),
                available: Decimal::from_str("4.0000").unwrap(),
                shortage: Decimal::from_str("6.0000").unwrap(),
                lines: vec![LineRef {
                    detail_key: "stock_adjustment_line".into(),
                    index: 1,
                }],
            }],
        };
        let value = serde_json::to_value(&details).unwrap();
        assert_eq!(value["items"][0]["required"], json!("10.0000"));
        assert_eq!(value["items"][0]["lot_id"], json!(null));
        assert_eq!(
            value["items"][0]["lines"],
            json!([{ "detail_key": "stock_adjustment_line", "index": 1 }])
        );

        let details = LotInUseDetails {
            lots: vec![LotInUse {
                lot_id: Id::nil(),
                lot_no: "L-01".into(),
                product_id: Id::nil(),
                product_code: "A001".into(),
                documents: vec![DocumentRef {
                    entity_key: "stock_adjustment".into(),
                    record_id: Id::nil(),
                    record_no: "ADJ202611-0001".into(),
                }],
            }],
        };
        let value = serde_json::to_value(&details).unwrap();
        assert_eq!(
            value["lots"][0]["documents"][0]["record_no"],
            json!("ADJ202611-0001")
        );
        assert_eq!(
            serde_json::to_value(InvalidStatusDetails {
                status: DocumentStatus::Posted
            })
            .unwrap(),
            json!({ "status": "posted" })
        );
    }
}
