//! 發票模組的查詢 API 型別（docs/architecture.md §8.11「查詢 API」）。
//!
//! | 端點 | 權限 | 成功回應 |
//! |---|---|---|
//! | `GET /api/v1/invoicing/uninvoiced-shipments` | `record.read` | `200` `Page<UninvoicedShipment>` |
//! | `GET /api/v1/invoicing/uninvoiced-receipts` | `record.read` | `200` `Page<UninvoicedReceipt>` |
//! | `GET /api/v1/invoicing/links` | `record.read` | `200` `Vec<InvoiceLink>` |
//!
//! 發票模組的單據本身（銷項發票、銷貨折讓、進項憑證、進貨折讓）使用通用資料 API 與單據動作。

use serde::{Deserialize, Serialize};
use specta::Type;
use time::Date;

use crate::Id;
use crate::decimal::Decimal;
use crate::document::DocumentStatus;

/// query string 的欄位名稱（錯誤的 `details.field`）。
pub mod query_field {
    pub const CUSTOMER_ID: &str = "customer_id";
    pub const SUPPLIER_ID: &str = "supplier_id";
    pub const DATE_FROM: &str = "date_from";
    pub const DATE_TO: &str = "date_to";
    pub const DOC_CLASS: &str = "doc_class";
    pub const ENTITY_KEY: &str = "entity_key";
    pub const RECORD_ID: &str = "record_id";
}

/// `GET /api/v1/invoicing/uninvoiced-shipments` 的 query string。條件都可省略。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct UninvoicedShipmentQuery {
    #[serde(default)]
    pub customer_id: Option<String>,
    /// `ship_date >= date_from`（含），`YYYY-MM-DD`。
    #[serde(default)]
    pub date_from: Option<String>,
    /// `ship_date <= date_to`（含），`YYYY-MM-DD`。
    #[serde(default)]
    pub date_to: Option<String>,
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub page_size: Option<u32>,
}

/// 尚未開立發票的已過帳出貨單（沒有被任何有效銷項發票關聯），依 `ship_date, id` 排序。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct UninvoicedShipment {
    pub id: Id,
    pub record_no: String,
    #[serde(with = "crate::time::date_str")]
    #[specta(type = String)]
    pub ship_date: Date,
    pub customer_id: Id,
    pub customer_code: String,
    pub customer_name: String,
    pub tax_type: String,
    pub tax_included: bool,
    #[serde(with = "crate::decimal::serde_str")]
    #[specta(type = String)]
    pub subtotal: Decimal,
    #[serde(with = "crate::decimal::serde_str")]
    #[specta(type = String)]
    pub tax_amount: Decimal,
    #[serde(with = "crate::decimal::serde_str")]
    #[specta(type = String)]
    pub total: Decimal,
    /// 指向此出貨單的已過帳銷貨退回 `total` 合計。
    #[serde(with = "crate::decimal::serde_str")]
    #[specta(type = String)]
    pub returned_total: Decimal,
    /// `total − returned_total`（參考用，發票金額仍由使用者決定）。
    #[serde(with = "crate::decimal::serde_str")]
    #[specta(type = String)]
    pub net_total: Decimal,
}

/// `GET /api/v1/invoicing/uninvoiced-receipts` 的 query string。條件都可省略。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct UninvoicedReceiptQuery {
    #[serde(default)]
    pub supplier_id: Option<String>,
    /// `receipt_date >= date_from`（含），`YYYY-MM-DD`。
    #[serde(default)]
    pub date_from: Option<String>,
    /// `receipt_date <= date_to`（含），`YYYY-MM-DD`。
    #[serde(default)]
    pub date_to: Option<String>,
    /// [`DocClass`]：`invoice`（預設，三聯式、電子發票）或 `customs`（海關繳納證）。
    #[serde(default)]
    pub doc_class: Option<String>,
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub page_size: Option<u32>,
}

/// 進項憑證的類別：發票類與海關繳納證各自計算「是否已取得憑證」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum DocClass {
    /// 三聯式、電子發票。
    Invoice,
    /// 海關代徵營業稅繳納證。
    Customs,
}

impl DocClass {
    /// query string 的值。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Invoice => "invoice",
            Self::Customs => "customs",
        }
    }

    /// 解析 query string 的值。
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "invoice" => Some(Self::Invoice),
            "customs" => Some(Self::Customs),
            _ => None,
        }
    }
}

/// 尚未取得該類憑證的已過帳進貨單，依 `receipt_date, id` 排序。金額為單據幣別。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct UninvoicedReceipt {
    pub id: Id,
    pub record_no: String,
    #[serde(with = "crate::time::date_str")]
    #[specta(type = String)]
    pub receipt_date: Date,
    pub supplier_id: Id,
    pub supplier_code: String,
    pub supplier_name: String,
    pub currency_code: String,
    #[serde(with = "crate::decimal::serde_str_option")]
    #[specta(type = Option<String>)]
    pub exchange_rate: Option<Decimal>,
    pub tax_type: String,
    #[serde(with = "crate::decimal::serde_str")]
    #[specta(type = String)]
    pub total: Decimal,
    #[serde(with = "crate::decimal::serde_str_option")]
    #[specta(type = Option<String>)]
    pub total_twd: Option<Decimal>,
    /// 指向此進貨單的已過帳進貨退回 `total` 合計（單據幣別）。
    #[serde(with = "crate::decimal::serde_str")]
    #[specta(type = String)]
    pub returned_total: Decimal,
    /// `total − returned_total`。
    #[serde(with = "crate::decimal::serde_str")]
    #[specta(type = String)]
    pub net_total: Decimal,
}

/// `GET /api/v1/invoicing/links` 的 query string。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct InvoiceLinkQuery {
    /// `shipment`、`goods_receipt`、`sales_return`、`purchase_return`。
    #[serde(default)]
    pub entity_key: Option<String>,
    #[serde(default)]
    pub record_id: Option<String>,
}

/// 關聯某張單據的發票、憑證或折讓（含已作廢），依 `invoice_date, id` 排序。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct InvoiceLink {
    /// `sales_invoice`、`purchase_invoice`、`sales_allowance`、`purchase_allowance`。
    pub entity_key: String,
    pub record_id: Id,
    pub record_no: String,
    /// 發票（憑證）號碼；折讓為 `null`。
    pub invoice_no: Option<String>,
    /// 發票（憑證）日期；折讓為折讓日期。
    #[serde(with = "crate::time::date_str")]
    #[specta(type = String)]
    pub invoice_date: Date,
    pub status: DocumentStatus,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn doc_class_round_trips() {
        for c in [DocClass::Invoice, DocClass::Customs] {
            assert_eq!(DocClass::parse(c.as_str()), Some(c));
            assert_eq!(serde_json::to_value(c).unwrap(), json!(c.as_str()));
        }
        assert_eq!(DocClass::parse("triplicate"), None);
    }

    #[test]
    fn link_serializes_dates_as_strings() {
        let link = InvoiceLink {
            entity_key: "sales_invoice".into(),
            record_id: Id::nil(),
            record_no: "SI202611-0001".into(),
            invoice_no: Some("AB12345678".into()),
            invoice_date: time::macros::date!(2026 - 11 - 19),
            status: DocumentStatus::Posted,
        };
        let v = serde_json::to_value(&link).unwrap();
        assert_eq!(v["invoice_date"], "2026-11-19");
        assert_eq!(v["status"], "posted");
    }
}
