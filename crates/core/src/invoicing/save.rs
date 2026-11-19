//! 發票模組單據存檔時由伺服器處理的部分（docs/architecture.md §8.11）。由 [`crate::records`] 在新增、
//! 修改時呼叫：
//!
//! 1. [`prefill_buyer`]（欄位驗證之前）：銷項發票新增時、修改時更換客戶時，沒有帶到的買受人資訊從客戶
//!    帶入（快照）
//! 2. [`check_header`] 與 [`apply_tax_period`]（主表通過欄位驗證之後）：發票號碼去除空白、轉大寫並檢查
//!    格式，統一編號格式；依單據日期寫入系統維護的 `tax_period`
//! 3. [`derive_allowance`]：銷貨折讓、進貨折讓從原發票（憑證）帶入 `customer_id` / `supplier_id`、
//!    `tax_type`；修改時不可更換原發票
//! 4. [`check_line_data`]（每一列明細）：`qty > 0`、`unit_price ≥ 0`
//! 5. [`check_links`]：銷項發票關聯的出貨單（已過帳、同客戶、不重複、不在其他有效發票中）
//! 6. [`apply_amounts`]（全部驗證通過之後）：依明細計算金額，套用手動稅額
//!
//! `tax_period`、折讓的客戶（廠商）與稅別是唯讀的系統欄位（request 帶了一律忽略，§7.1），資料表上為
//! `NOT NULL`，所以存檔時一定要寫入。

use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;

use hammer_db::invoicing::{self as db, InvoiceForAllowance, ShipmentForInvoice};
use hammer_db::purchasing as purchasing_db;
use hammer_db::sqlx::PgConnection;
use hammer_shared::Id;
use hammer_shared::decimal::Decimal;
use hammer_shared::schema::{DataType, FieldValue, parse_value};
use serde_json::{Map, Value};
use time::Date;

use super::allowances;
use super::amounts::apply_manual_tax;
use super::number::{self, MSG_INVOICE_NO_FORMAT};
use super::{
    PURCHASE_ALLOWANCE, PURCHASE_INVOICE, SALES_ALLOWANCE, SALES_ALLOWANCE_LINE, SALES_INVOICE,
    SALES_INVOICE_LINE, SALES_INVOICE_SHIPMENT,
};
use crate::CoreError;
use crate::master_data::{MSG_TAX_ID, is_tax_id};
use crate::purchasing::save::line_errors as qty_price_errors;
use crate::record_data::FieldError;
use crate::record_lines::LineError;
use crate::sales::amounts::{line_amount, totals};
use crate::sales::save::sales_places;

/// 期別欄位。
pub const TAX_PERIOD: &str = "tax_period";
pub(crate) const INVOICE_NO: &str = "invoice_no";
pub(crate) const INVOICE_TYPE: &str = "invoice_type";
pub(crate) const CUSTOMER_ID: &str = "customer_id";
pub(crate) const BUYER_TAX_ID: &str = "buyer_tax_id";
pub(crate) const BUYER_NAME: &str = "buyer_name";
pub(crate) const BUYER_ADDRESS: &str = "buyer_address";
pub(crate) const TAX_TYPE: &str = "tax_type";
pub(crate) const TAX_AMOUNT_MANUAL: &str = "tax_amount_manual";
pub(crate) const SHIPMENT_ID: &str = "shipment_id";
const QTY: &str = "qty";
const UNIT_PRICE: &str = "unit_price";
const AMOUNT: &str = "amount";

/// 二聯式（單價含稅）。
pub const DUPLICATE: &str = "duplicate";

/// 出貨單是草稿。
pub const MSG_SHIPMENT_DRAFT: &str = "出貨單尚未過帳";
/// 出貨單已作廢。
pub const MSG_SHIPMENT_VOID: &str = "出貨單已作廢";
/// 出貨單的客戶與發票不同。
pub const MSG_SHIPMENT_CUSTOMER: &str = "客戶與發票不同";
/// 同一張發票關聯同一張出貨單兩次。
pub const MSG_SHIPMENT_DUPLICATE: &str = "出貨單重複";

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

/// 折讓存檔時從原發票（憑證）帶入表頭（新增與修改都執行；不是折讓時不做事）。
///
/// - `master`：主表正規化後的結果（修改時已與既有資料合併）；`existing`：修改前的資料。
///
/// 原發票不存在或已刪除時不帶入，交給之後的關聯檢查回報；修改時更換了原發票 → 主表的原發票欄位錯誤。
/// 帶入後檢查原發票已開立、折讓日期、關聯的退回單（[`allowances`]）。
pub async fn derive_allowance(
    conn: &mut PgConnection,
    entity_key: &str,
    master: Result<Map<String, Value>, Vec<FieldError>>,
    existing: Option<&Map<String, Value>>,
) -> Result<Result<Map<String, Value>, Vec<FieldError>>, CoreError> {
    let Some(source) = allowances::source_of(entity_key) else {
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
    let Some(invoice) = invoice else {
        return Ok(Ok(data));
    };
    apply_invoice(&mut data, source.party_field, &invoice);
    let mut errors = allowances::invoice_errors(
        source,
        &invoice,
        date_of(data.get(allowances::ALLOWANCE_DATE)),
    );
    errors.extend(
        allowances::check_return(conn, source, &data, Some(invoice.party_id), None, false).await?,
    );
    if errors.is_empty() {
        Ok(Ok(data))
    } else {
        Ok(Err(errors))
    }
}

/// 表頭帶入原發票的客戶（廠商）與稅別。
fn apply_invoice(data: &mut Map<String, Value>, party_field: &str, invoice: &InvoiceForAllowance) {
    data.insert(
        party_field.into(),
        Value::String(invoice.party_id.to_string()),
    );
    data.insert("tax_type".into(), Value::String(invoice.tax_type.clone()));
}

pub(crate) fn id_of(v: Option<&Value>) -> Option<Id> {
    match parse_value(DataType::Relation, v?) {
        Ok(Some(FieldValue::Relation(id))) => Some(id),
        _ => None,
    }
}

pub(crate) fn date_of(v: Option<&Value>) -> Option<Date> {
    match parse_value(DataType::Date, v?) {
        Ok(Some(FieldValue::Date(d))) => Some(d),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// 買受人資訊、表頭檢查
// ---------------------------------------------------------------------------

/// 銷項發票：沒有帶到的買受人資訊從客戶帶入（§8.11「買受人資訊」）。新增時一律帶入；修改時只在
/// `customer_id` 有變動時帶入。`input` 為 request 的 `data`（欄位驗證之前），`existing` 為修改前的資料。
/// 客戶不存在時不帶入（交給關聯檢查）。
pub(crate) async fn prefill_buyer(
    conn: &mut PgConnection,
    entity_key: &str,
    input: &mut Map<String, Value>,
    existing: Option<&Map<String, Value>>,
) -> Result<(), CoreError> {
    if entity_key != SALES_INVOICE {
        return Ok(());
    }
    let Some(customer_id) = id_of(input.get(CUSTOMER_ID)) else {
        return Ok(());
    };
    if existing.is_some_and(|e| id_of(e.get(CUSTOMER_ID)) == Some(customer_id)) {
        return Ok(());
    }
    let Some(customer) = db::find_customer_for_invoice(&mut *conn, customer_id).await? else {
        return Ok(());
    };
    let title = customer
        .invoice_title
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| customer.name.clone());
    let address = customer
        .invoice_address
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| customer.address.clone());
    let values = [
        (BUYER_TAX_ID, customer.tax_id.clone()),
        (BUYER_NAME, Some(title)),
        (BUYER_ADDRESS, address),
    ];
    for (key, value) in values {
        if input.contains_key(key) {
            continue;
        }
        input.insert(key.into(), value.map_or(Value::Null, Value::String));
    }
    Ok(())
}

/// 表頭的固定檢查（主表通過欄位驗證之後）：銷項發票的發票號碼去除空白、轉大寫後寫回並檢查格式；
/// 買受人統一編號為 8 位數字。不是發票模組的單據時為空。
pub fn check_header(entity_key: &str, data: &mut Map<String, Value>) -> Vec<FieldError> {
    let mut errors = Vec::new();
    if entity_key != SALES_INVOICE {
        return errors;
    }
    if let Some(no) = data
        .get(INVOICE_NO)
        .and_then(Value::as_str)
        .map(number::normalize)
    {
        if !number::is_invoice_no(&no) {
            errors.push(field_error(INVOICE_NO, MSG_INVOICE_NO_FORMAT));
        }
        data.insert(INVOICE_NO.into(), Value::String(no));
    }
    if let Some(Value::String(tax_id)) = data.get(BUYER_TAX_ID)
        && !is_tax_id(tax_id)
    {
        errors.push(field_error(BUYER_TAX_ID, MSG_TAX_ID));
    }
    errors
}

// ---------------------------------------------------------------------------
// 明細與金額
// ---------------------------------------------------------------------------

/// 有明細金額的發票模組單據：`(資料類別, 明細子表)`。
const PRICED: &[(&str, &str)] = &[
    (SALES_INVOICE, SALES_INVOICE_LINE),
    (SALES_ALLOWANCE, SALES_ALLOWANCE_LINE),
];

/// 有金額計算時的明細子表代碼。
pub fn amount_lines(entity_key: &str) -> Option<&'static str> {
    PRICED
        .iter()
        .find(|(entity, _)| *entity == entity_key)
        .map(|(_, line)| *line)
}

/// 修改時是否要帶入所有明細（重新計算金額，也重新檢查關聯的出貨單）。
pub fn recalculates_lines(entity_key: &str) -> bool {
    amount_lines(entity_key).is_some()
}

/// 一列明細本身的檢查：發票明細、折讓明細的 `qty > 0`、`unit_price ≥ 0`。其他明細子表為空。
pub fn check_line_data(detail_key: &str, data: &Map<String, Value>) -> Vec<FieldError> {
    if detail_key != SALES_INVOICE_LINE && detail_key != SALES_ALLOWANCE_LINE {
        return Vec::new();
    }
    qty_price_errors(decimal_of(data.get(QTY)), decimal_of(data.get(UNIT_PRICE)))
}

/// 依明細重新計算金額並寫入 `data` 與每一列明細（全部驗證通過之後），套用手動稅額。手動稅額不符合
/// 規則時為 `tax_amount_manual` 的欄位錯誤；銷貨折讓再檢查累計上限（`total` 的欄位錯誤）。不是有金額
/// 計算的單據時不做事。
pub(crate) async fn apply_amounts(
    conn: &mut PgConnection,
    entity_key: &str,
    data: &mut Map<String, Value>,
    lines: Vec<&mut Map<String, Value>>,
) -> Result<(), CoreError> {
    if amount_lines(entity_key).is_none() {
        return Ok(());
    }
    let places = sales_places(&mut *conn).await?;
    let tax_type = data
        .get(TAX_TYPE)
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::Internal("單據沒有稅別".into()))?;
    let tax_rate = purchasing_db::find_tax_rate(&mut *conn, tax_type)
        .await?
        .ok_or_else(|| CoreError::Internal(format!("找不到稅別 {tax_type}")))?;
    let tax_included = data.get(INVOICE_TYPE).and_then(Value::as_str) == Some(DUPLICATE);

    let mut amounts = Vec::with_capacity(lines.len());
    for line in lines {
        let (Some(qty), Some(price)) =
            (decimal_of(line.get(QTY)), decimal_of(line.get(UNIT_PRICE)))
        else {
            return Err(CoreError::Internal("明細沒有數量或單價".into()));
        };
        let amount = line_amount(qty, price, Decimal::ZERO, places);
        line.insert(AMOUNT.into(), Value::String(amount.to_string()));
        amounts.push(amount);
    }
    let computed = totals(amounts, tax_rate, tax_included, places);
    let manual = decimal_of(data.get(TAX_AMOUNT_MANUAL));
    let result =
        apply_manual_tax(computed, manual, tax_rate, tax_included, places).map_err(|message| {
            CoreError::InvalidFields(vec![field_error(TAX_AMOUNT_MANUAL, message)])
        })?;
    data.insert(
        "subtotal".into(),
        Value::String(result.subtotal.to_string()),
    );
    data.insert(
        "tax_amount".into(),
        Value::String(result.tax_amount.to_string()),
    );
    data.insert("total".into(), Value::String(result.total.to_string()));
    allowances::check_cap_on_save(conn, entity_key, data).await
}

// ---------------------------------------------------------------------------
// 關聯的出貨單
// ---------------------------------------------------------------------------

/// 關聯出貨單的檢查結果：發票的客戶、各列的出貨單與其他有效發票。
pub(crate) struct LinkCheck<'a> {
    /// 發票的客戶；沒有值（欄位錯誤）時不比對客戶。
    pub customer_id: Option<Id>,
    /// 各列 `(request 中的位置, 出貨單 id)`；`None` 為沒有值或格式錯誤（交給欄位驗證）。
    pub rows: &'a [(usize, Option<Id>)],
    /// 找到的出貨單（未刪除）。
    pub shipments: &'a HashMap<Id, ShipmentForInvoice>,
    /// 已在其他有效發票中的出貨單：出貨單 id → 發票號碼。
    pub invoiced: &'a BTreeMap<Id, String>,
}

/// 關聯出貨單的檢查（§8.11「關聯出貨單、進貨單」）：已過帳、同客戶、同一張發票中不重複、不在其他有效
/// 發票中。出貨單不存在（或已刪除）不在這裡報錯（交給關聯檢查）。純邏輯。
pub(crate) fn link_errors(check: &LinkCheck<'_>) -> Vec<LineError> {
    let mut errors = Vec::new();
    let mut seen = Vec::new();
    for (index, id) in check.rows {
        let Some(id) = id else {
            continue;
        };
        let error = |message: String| {
            LineError::field(SALES_INVOICE_SHIPMENT, *index, SHIPMENT_ID, message)
        };
        if seen.contains(id) {
            errors.push(error(MSG_SHIPMENT_DUPLICATE.into()));
            continue;
        }
        seen.push(*id);
        let Some(shipment) = check.shipments.get(id) else {
            continue;
        };
        match shipment.status.as_str() {
            "posted" => {}
            "void" => {
                errors.push(error(MSG_SHIPMENT_VOID.into()));
                continue;
            }
            _ => {
                errors.push(error(MSG_SHIPMENT_DRAFT.into()));
                continue;
            }
        }
        if check.customer_id.is_some_and(|c| c != shipment.customer_id) {
            errors.push(error(MSG_SHIPMENT_CUSTOMER.into()));
            continue;
        }
        if let Some(invoice_no) = check.invoiced.get(id) {
            errors.push(error(format!(
                "出貨單 {} 已開立發票 {invoice_no}",
                shipment.record_no
            )));
        }
    }
    errors
}

/// 讀取 `ids` 的出貨單與已開立的有效發票（`lock` 為開立時以 `FOR UPDATE` 鎖定，否則 `FOR SHARE`）。
pub(crate) async fn load_links(
    conn: &mut PgConnection,
    ids: &[Id],
    exclude_invoice: Option<Id>,
    lock: bool,
) -> Result<(HashMap<Id, ShipmentForInvoice>, BTreeMap<Id, String>), CoreError> {
    let mut sorted = ids.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted.is_empty() {
        return Ok((HashMap::new(), BTreeMap::new()));
    }
    let shipments = if lock {
        db::lock_shipments_for_invoice(&mut *conn, &sorted).await?
    } else {
        db::list_shipments_for_invoice(&mut *conn, &sorted).await?
    };
    let mut invoiced = BTreeMap::new();
    for row in db::posted_invoices_of_shipments(&mut *conn, &sorted, exclude_invoice).await? {
        invoiced.entry(row.document_id).or_insert(row.invoice_no);
    }
    Ok((shipments.into_iter().map(|s| (s.id, s)).collect(), invoiced))
}

/// 銷項發票存檔時檢查關聯的出貨單（新增與修改都執行；不是銷項發票時不做事）。`rows` 為通過欄位驗證的
/// `sales_invoice_shipment` 明細 `(request 中的位置, data)`；修改時所有明細都已列入。
pub(crate) async fn check_links(
    conn: &mut PgConnection,
    entity_key: &str,
    master: &Result<Map<String, Value>, Vec<FieldError>>,
    rows: &[(usize, &Map<String, Value>)],
) -> Result<Vec<LineError>, CoreError> {
    if entity_key != SALES_INVOICE || rows.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<(usize, Option<Id>)> = rows
        .iter()
        .map(|(index, data)| (*index, id_of(data.get(SHIPMENT_ID))))
        .collect();
    let ids: Vec<Id> = rows.iter().filter_map(|(_, id)| *id).collect();
    // 存檔的發票一定是草稿，不在「有效發票」中，不需要排除自己
    let (shipments, invoiced) = load_links(conn, &ids, None, false).await?;
    let customer_id = master
        .as_ref()
        .ok()
        .and_then(|data| id_of(data.get(CUSTOMER_ID)));
    Ok(link_errors(&LinkCheck {
        customer_id,
        rows: &rows,
        shipments: &shipments,
        invoiced: &invoiced,
    }))
}

pub(crate) fn field_error(key: &str, message: impl Into<String>) -> FieldError {
    FieldError {
        field_key: key.into(),
        message: message.into(),
    }
}

pub(crate) fn decimal_of(v: Option<&Value>) -> Option<Decimal> {
    match v? {
        Value::String(s) => Decimal::from_str(s.trim()).ok(),
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
            invoice_date: date!(2026 - 11 - 19),
            total: Decimal::ZERO,
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
