//! 發票模組單據存檔時由伺服器處理的部分（docs/architecture.md §8.11）。由 [`crate::records`] 在新增、
//! 修改時呼叫：
//!
//! 1. [`apply_tax_period`]（主表通過欄位驗證之後）：依單據日期寫入系統維護的 `tax_period`
//! 2. [`derive_allowance`]：銷貨折讓、進貨折讓從原發票（憑證）帶入 `customer_id` / `supplier_id`、
//!    `tax_type`；修改時不可更換原發票
//!
//! `tax_period`、折讓的客戶（廠商）與稅別是唯讀的系統欄位（request 帶了一律忽略，§7.1），資料表上為
//! `NOT NULL`，所以存檔時一定要寫入。金額、號碼、開立與作廢的檢查在之後的 issue 加入（#184–#186）。

use hammer_db::invoicing::{self as db, InvoiceForAllowance};
use hammer_db::sqlx::PgConnection;
use hammer_shared::Id;
use hammer_shared::schema::{DataType, FieldValue, parse_value};
use serde_json::{Map, Value};
use time::Date;

use super::{PURCHASE_ALLOWANCE, PURCHASE_INVOICE, SALES_ALLOWANCE, SALES_INVOICE};
use crate::CoreError;
use crate::record_data::FieldError;

/// 期別欄位。
pub const TAX_PERIOD: &str = "tax_period";

/// 銷貨折讓不可更換原發票（§8.11 `sales_allowances`）。
pub const MSG_INVOICE_FIXED: &str = "建立後不可更換發票";
/// 進貨折讓不可更換原憑證（§8.11 `purchase_allowances`）。
pub const MSG_PURCHASE_INVOICE_FIXED: &str = "建立後不可更換憑證";

/// 單據日期的欄位（期別由它計算）；不是發票模組的單據時為 `None`。
pub fn date_field(entity_key: &str) -> Option<&'static str> {
    match entity_key {
        SALES_INVOICE | PURCHASE_INVOICE => Some("invoice_date"),
        SALES_ALLOWANCE | PURCHASE_ALLOWANCE => Some("allowance_date"),
        _ => None,
    }
}

/// 期別（§8.11「期別」）：年（4 位）+ 該期第一個月（奇數月，2 位）。營業稅每兩個月一期，
/// 例如 2026-11-19、2026-12-05 → `202611`；2026-02-28 → `202601`。
pub fn tax_period(date: Date) -> String {
    let month = u8::from(date.month());
    let first = month - (month - 1) % 2;
    format!("{:04}{:02}", date.year(), first)
}

/// 依單據日期寫入 `tax_period`（發票模組以外的資料類別不做事；日期沒有值時不寫入，交給必填檢查）。
pub fn apply_tax_period(entity_key: &str, data: &mut Map<String, Value>) {
    let Some(field) = date_field(entity_key) else {
        return;
    };
    if let Some(date) = date_of(data.get(field)) {
        data.insert(TAX_PERIOD.into(), Value::String(tax_period(date)));
    }
}

/// 折讓的原發票欄位、帶入的對象欄位、更換時的訊息。
struct AllowanceSource {
    invoice_field: &'static str,
    party_field: &'static str,
    fixed_message: &'static str,
}

fn allowance_source(entity_key: &str) -> Option<AllowanceSource> {
    match entity_key {
        SALES_ALLOWANCE => Some(AllowanceSource {
            invoice_field: "sales_invoice_id",
            party_field: "customer_id",
            fixed_message: MSG_INVOICE_FIXED,
        }),
        PURCHASE_ALLOWANCE => Some(AllowanceSource {
            invoice_field: "purchase_invoice_id",
            party_field: "supplier_id",
            fixed_message: MSG_PURCHASE_INVOICE_FIXED,
        }),
        _ => None,
    }
}

/// 折讓存檔時從原發票（憑證）帶入表頭（新增與修改都執行；不是折讓時不做事）。
///
/// - `master`：主表正規化後的結果（修改時已與既有資料合併）；`existing`：修改前的資料。
///
/// 原發票不存在或已刪除時不帶入，交給之後的關聯檢查回報；修改時更換了原發票 → 主表的原發票欄位錯誤。
pub async fn derive_allowance(
    conn: &mut PgConnection,
    entity_key: &str,
    master: Result<Map<String, Value>, Vec<FieldError>>,
    existing: Option<&Map<String, Value>>,
) -> Result<Result<Map<String, Value>, Vec<FieldError>>, CoreError> {
    let Some(source) = allowance_source(entity_key) else {
        return Ok(master);
    };
    let Ok(mut data) = master else {
        return Ok(master);
    };
    let Some(invoice_id) = id_of(data.get(source.invoice_field)) else {
        return Ok(Ok(data));
    };
    if existing.is_some_and(|e| id_of(e.get(source.invoice_field)) != Some(invoice_id)) {
        return Ok(Err(vec![FieldError {
            field_key: source.invoice_field.into(),
            message: source.fixed_message.into(),
        }]));
    }
    let invoice = if entity_key == SALES_ALLOWANCE {
        db::find_sales_invoice_for_allowance(&mut *conn, invoice_id).await?
    } else {
        db::find_purchase_invoice_for_allowance(&mut *conn, invoice_id).await?
    };
    if let Some(invoice) = invoice {
        apply_invoice(&mut data, source.party_field, &invoice);
    }
    Ok(Ok(data))
}

/// 表頭帶入原發票的客戶（廠商）與稅別。
fn apply_invoice(data: &mut Map<String, Value>, party_field: &str, invoice: &InvoiceForAllowance) {
    data.insert(
        party_field.into(),
        Value::String(invoice.party_id.to_string()),
    );
    data.insert("tax_type".into(), Value::String(invoice.tax_type.clone()));
}

fn id_of(v: Option<&Value>) -> Option<Id> {
    match parse_value(DataType::Relation, v?) {
        Ok(Some(FieldValue::Relation(id))) => Some(id),
        _ => None,
    }
}

fn date_of(v: Option<&Value>) -> Option<Date> {
    match parse_value(DataType::Date, v?) {
        Ok(Some(FieldValue::Date(d))) => Some(d),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use time::macros::date;
    use uuid::Uuid;

    #[test]
    fn tax_period_is_first_month_of_two_month_period() {
        assert_eq!(tax_period(date!(2026 - 11 - 19)), "202611");
        assert_eq!(tax_period(date!(2026 - 12 - 05)), "202611");
        assert_eq!(tax_period(date!(2026 - 02 - 28)), "202601");
        assert_eq!(tax_period(date!(2026 - 01 - 01)), "202601");
        assert_eq!(tax_period(date!(2027 - 06 - 30)), "202705");
    }

    #[test]
    fn apply_tax_period_uses_document_date() {
        let mut data = json!({ "invoice_date": "2026-10-03", "tax_period": "x" })
            .as_object()
            .unwrap()
            .clone();
        apply_tax_period(SALES_INVOICE, &mut data);
        assert_eq!(data["tax_period"], "202609");

        let mut data = json!({ "allowance_date": "2026-08-31" })
            .as_object()
            .unwrap()
            .clone();
        apply_tax_period(PURCHASE_ALLOWANCE, &mut data);
        assert_eq!(data["tax_period"], "202607");

        // 其他資料類別、沒有日期時不寫入
        let mut data = json!({ "invoice_date": "2026-10-03" })
            .as_object()
            .unwrap()
            .clone();
        apply_tax_period("shipment", &mut data);
        assert!(!data.contains_key("tax_period"));
        let mut data = Map::new();
        apply_tax_period(SALES_INVOICE, &mut data);
        assert!(data.is_empty());
    }

    #[test]
    fn allowance_copies_party_and_tax_type() {
        let invoice = InvoiceForAllowance {
            id: Uuid::nil(),
            status: "posted".into(),
            party_id: Uuid::from_u128(7),
            tax_type: "zero_rated".into(),
            doc_type: "triplicate".into(),
        };
        let mut data = json!({ "customer_id": "ignored" })
            .as_object()
            .unwrap()
            .clone();
        apply_invoice(&mut data, "customer_id", &invoice);
        assert_eq!(
            Value::Object(data),
            json!({ "customer_id": Uuid::from_u128(7).to_string(), "tax_type": "zero_rated" })
        );
    }

    #[test]
    fn date_fields() {
        assert_eq!(date_field(SALES_INVOICE), Some("invoice_date"));
        assert_eq!(date_field(PURCHASE_INVOICE), Some("invoice_date"));
        assert_eq!(date_field(SALES_ALLOWANCE), Some("allowance_date"));
        assert_eq!(date_field(PURCHASE_ALLOWANCE), Some("allowance_date"));
        assert_eq!(date_field("shipment"), None);
    }
}
