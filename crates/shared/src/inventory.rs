//! 庫存的共通型別（`docs/architecture.md` §8.10）。
//!
//! # 庫存查詢 API
//!
//! 都需要 `record.read`；分頁依 §10（[`crate::pagination`]）。
//!
//! | 端點 | query | 回應 |
//! |---|---|---|
//! | `GET /api/v1/inventory/balances` | [`StockBalanceQuery`] | `Page<`[`StockBalance`]`>` |
//! | `GET /api/v1/inventory/products/{product_id}/lots` | [`LotQuery`] | `Page<`[`InventoryLot`]`>` |
//! | `GET /api/v1/inventory/transactions` | [`TransactionQuery`] | `Page<`[`InventoryTransaction`]`>` |
//! | `POST /api/v1/inventory/allocation-preview` | body [`AllocationPreviewRequest`] | [`AllocationPreview`] |
//!
//! - 篩選參數以字串接收，由伺服器解析：格式錯誤（uuid、日期、`true` / `false`、`txn_type`）→
//!   `422 VALIDATION`，`details.field` 為參數名稱。空字串視為未指定
//! - `page`、`page_size` 不是整數 → `400 BAD_REQUEST`（與其他列表相同）
//! - 數量、單位成本為補齊 4 位小數的字串；`stock_value_twd` 為四捨五入到 2 位小數的字串
//! - 商品不存在或已刪除（批號列表）→ `404 NOT_FOUND`
//!
//! # 建議批號 API（§8.9「建議批號 API」）
//!
//! 出貨單草稿試算先進先出的結果：只試算，不鎖定、不寫入，不考慮其他草稿；分配順序與出貨過帳相同
//! （先扣有 `lot_no` 的列，再依陣列順序以先進先出扣其他列）。庫存不足**不是錯誤**，回傳能分配的部分與
//! `shortage`。`lines` 1–[`ALLOCATION_PREVIEW_MAX_LINES`] 列；格式錯誤、商品不存在、批號找不到 →
//! `422 VALIDATION`，`details.lines` 的 `detail_key` 為 `null`、`index` 為 `lines` 中的位置。

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use specta::Type;
use time::{Date, OffsetDateTime};

use crate::Id;

/// 批號的最大長度（字元數，§8.10「批號」）。
pub const LOT_NO_MAX_CHARS: usize = 50;

/// 建議批號 API 一次最多的列數。
pub const ALLOCATION_PREVIEW_MAX_LINES: usize = 200;

/// 庫存異動的類型（`inventory_transactions.txn_type`）。JSON 中為 snake_case。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum TxnType {
    /// 進貨入庫。
    Receipt,
    /// 出貨。
    Shipment,
    /// 銷貨退回。
    SalesReturn,
    /// 進貨退回。
    PurchaseReturn,
    /// 盤點與調整（含期初庫存）。
    Adjustment,
    /// 作廢單據產生的反向異動。
    VoidReversal,
}

impl TxnType {
    /// 所有類型。
    pub const ALL: [Self; 6] = [
        Self::Receipt,
        Self::Shipment,
        Self::SalesReturn,
        Self::PurchaseReturn,
        Self::Adjustment,
        Self::VoidReversal,
    ];

    /// 資料庫與 JSON 中的值。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Receipt => "receipt",
            Self::Shipment => "shipment",
            Self::SalesReturn => "sales_return",
            Self::PurchaseReturn => "purchase_return",
            Self::Adjustment => "adjustment",
            Self::VoidReversal => "void_reversal",
        }
    }

    /// 由資料庫的值解析；不認得的值回傳 `None`。
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == s)
    }
}

/// 庫存查詢 API 的參數名稱（驗證錯誤的 `details.field`）。
pub mod query_field {
    pub const CATEGORY_ID: &str = "category_id";
    pub const IN_STOCK: &str = "in_stock";
    pub const BELOW_SAFETY_STOCK: &str = "below_safety_stock";
    pub const INCLUDE_INACTIVE: &str = "include_inactive";
    pub const INCLUDE_EMPTY: &str = "include_empty";
    pub const PRODUCT_ID: &str = "product_id";
    pub const LOT_ID: &str = "lot_id";
    pub const DATE_FROM: &str = "date_from";
    pub const DATE_TO: &str = "date_to";
    pub const TXN_TYPE: &str = "txn_type";
    pub const REF_DOC_TYPE: &str = "ref_doc_type";
    pub const REF_DOC_ID: &str = "ref_doc_id";
}

/// `GET /api/v1/inventory/balances` 的 query string。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct StockBalanceQuery {
    /// 料號或品名包含（不分大小寫）；以空白分隔的每個詞都要出現在料號或品名中。
    #[serde(default)]
    pub q: Option<String>,
    /// 商品分類 id（只比對該分類本身，不含下層分類）。
    #[serde(default)]
    pub category_id: Option<String>,
    /// `true`：只列出庫存大於 0；`false`：只列出庫存為 0；省略：全部。
    #[serde(default)]
    pub in_stock: Option<String>,
    /// `true`：只列出有設定安全庫存且庫存低於安全庫存的商品；`false` 或省略：全部。
    #[serde(default)]
    pub below_safety_stock: Option<String>,
    /// 是否包含停用的商品；預設 `true`（停用的商品可能還有庫存）。
    #[serde(default)]
    pub include_inactive: Option<String>,
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub page_size: Option<u32>,
}

/// 一個商品的庫存（未刪除的 `goods` 商品；沒有批號時庫存為 0）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct StockBalance {
    pub product_id: Id,
    pub code: String,
    pub name: String,
    pub spec: Option<String>,
    pub unit_name: String,
    pub is_active: bool,
    /// 庫存數量（4 位小數）。
    #[serde(with = "crate::decimal::serde_str")]
    #[specta(type = String)]
    pub qty_on_hand: Decimal,
    /// 台幣庫存金額（四捨五入到 2 位小數）。
    #[serde(with = "crate::decimal::serde_str")]
    #[specta(type = String)]
    pub stock_value_twd: Decimal,
    /// 安全庫存（4 位小數）；沒有設定時為 `null`。
    #[serde(with = "crate::decimal::serde_str_option")]
    #[specta(type = Option<String>)]
    pub safety_stock: Option<Decimal>,
    /// 有設定安全庫存且庫存低於安全庫存。
    pub below_safety_stock: bool,
}

/// `GET /api/v1/inventory/products/{product_id}/lots` 的 query string。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct LotQuery {
    /// `true`：包含已用完（`qty_remaining = 0`）的批號；預設 `false`。
    #[serde(default)]
    pub include_empty: Option<String>,
    /// 批號包含（不分大小寫）。
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub page_size: Option<u32>,
}

/// 一個批號（依先進先出順序 `received_date, id` 排列）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct InventoryLot {
    pub id: Id,
    pub lot_no: String,
    #[serde(with = "crate::time::date_str")]
    #[specta(type = String)]
    pub received_date: Date,
    /// 產生此批號的單據明細子表代碼（例如 `stock_adjustment_line`）。
    pub source_type: String,
    /// 來源單據（主表）的 id。
    pub source_doc_id: Id,
    /// 來源單據的單號。
    pub source_record_no: String,
    #[serde(with = "crate::decimal::serde_str")]
    #[specta(type = String)]
    pub qty_in: Decimal,
    #[serde(with = "crate::decimal::serde_str")]
    #[specta(type = String)]
    pub qty_remaining: Decimal,
    #[serde(with = "crate::decimal::serde_str")]
    #[specta(type = String)]
    pub unit_cost_twd: Decimal,
}

/// `GET /api/v1/inventory/transactions` 的 query string。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct TransactionQuery {
    #[serde(default)]
    pub product_id: Option<String>,
    #[serde(default)]
    pub lot_id: Option<String>,
    /// `txn_date >= date_from`（含），`YYYY-MM-DD`。
    #[serde(default)]
    pub date_from: Option<String>,
    /// `txn_date <= date_to`（含），`YYYY-MM-DD`。
    #[serde(default)]
    pub date_to: Option<String>,
    /// 異動類型（[`TxnType`]）。
    #[serde(default)]
    pub txn_type: Option<String>,
    /// 來源單據的主表代碼（例如 `stock_adjustment`）。
    #[serde(default)]
    pub ref_doc_type: Option<String>,
    /// 來源單據 id。
    #[serde(default)]
    pub ref_doc_id: Option<String>,
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub page_size: Option<u32>,
}

/// 一筆庫存異動（依 `txn_date DESC, id DESC` 由新到舊排列）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct InventoryTransaction {
    pub id: Id,
    #[serde(with = "time::serde::rfc3339")]
    #[specta(type = String)]
    pub txn_at: OffsetDateTime,
    #[serde(with = "crate::time::date_str")]
    #[specta(type = String)]
    pub txn_date: Date,
    pub txn_type: TxnType,
    pub product_id: Id,
    pub product_code: String,
    pub product_name: String,
    pub lot_id: Id,
    pub lot_no: String,
    /// 數量變動（入庫為正、出庫為負，4 位小數）。
    #[serde(with = "crate::decimal::serde_str")]
    #[specta(type = String)]
    pub qty_change: Decimal,
    #[serde(with = "crate::decimal::serde_str")]
    #[specta(type = String)]
    pub unit_cost_twd: Decimal,
    pub ref_doc_type: String,
    pub ref_doc_id: Id,
    /// 來源單據的單號。
    pub ref_record_no: String,
    pub ref_line_id: Id,
    pub created_by: Option<Id>,
    pub created_by_name: Option<String>,
}

/// `POST /api/v1/inventory/allocation-preview` 的 request。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct AllocationPreviewRequest {
    #[serde(default)]
    pub lines: Vec<AllocationPreviewLineInput>,
}

/// 建議批號的一列輸入。值以字串接收，由伺服器解析（格式錯誤時回傳該列的欄位錯誤）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct AllocationPreviewLineInput {
    /// 商品 id。
    #[serde(default)]
    pub product_id: Option<String>,
    /// 數量：大於 0、最多 4 位小數。
    #[serde(default)]
    pub qty: Option<String>,
    /// 手動指定的批號（去除前後空白；省略或空字串為先進先出）。
    #[serde(default)]
    pub lot_no: Option<String>,
}

/// 建議批號的結果，`lines` 與 request 的順序相同。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct AllocationPreview {
    pub lines: Vec<AllocationPreviewLine>,
}

/// 一列的建議結果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct AllocationPreviewLine {
    /// request 中的位置（從 0 開始）。
    pub index: u32,
    pub product_id: Id,
    /// `goods` / `service`。
    pub product_type: String,
    #[serde(with = "crate::decimal::serde_str")]
    #[specta(type = String)]
    pub qty: Decimal,
    /// 依扣除順序；`service` 為空陣列。
    pub allocations: Vec<PreviewAllocation>,
    /// 不足的數量（足夠時為 `"0.0000"`）。
    #[serde(with = "crate::decimal::serde_str")]
    #[specta(type = String)]
    pub shortage: Decimal,
}

/// 從一個批號扣除的建議。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct PreviewAllocation {
    pub lot_id: Id,
    pub lot_no: String,
    #[serde(with = "crate::time::date_str")]
    #[specta(type = String)]
    pub received_date: Date,
    #[serde(with = "crate::decimal::serde_str")]
    #[specta(type = String)]
    pub qty: Decimal,
    #[serde(with = "crate::decimal::serde_str")]
    #[specta(type = String)]
    pub unit_cost_twd: Decimal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn txn_type_round_trips() {
        for t in TxnType::ALL {
            assert_eq!(TxnType::parse(t.as_str()), Some(t));
            assert_eq!(
                serde_json::to_value(t).unwrap(),
                serde_json::json!(t.as_str())
            );
        }
        assert_eq!(TxnType::parse("transfer"), None);
    }

    #[test]
    fn balance_serializes_decimals_as_strings() {
        use std::str::FromStr;
        let b = StockBalance {
            product_id: Id::nil(),
            code: "A001".into(),
            name: "螺絲".into(),
            spec: None,
            unit_name: "個".into(),
            is_active: true,
            qty_on_hand: Decimal::from_str("12.0000").unwrap(),
            stock_value_twd: Decimal::from_str("3450.00").unwrap(),
            safety_stock: None,
            below_safety_stock: false,
        };
        let json = serde_json::to_value(&b).unwrap();
        assert_eq!(json["qty_on_hand"], "12.0000");
        assert_eq!(json["stock_value_twd"], "3450.00");
        assert_eq!(json["safety_stock"], serde_json::Value::Null);
        let back: StockBalance = serde_json::from_value(json).unwrap();
        assert_eq!(back, b);
    }
}
