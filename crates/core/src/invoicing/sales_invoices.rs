//! 銷項發票的開立與作廢（docs/architecture.md §8.11「開立與作廢」，[`crate::documents`] 的框架上）。
//!
//! # 開立（`post`）
//!
//! 鎖定順序：發票（框架以 `FOR UPDATE` 鎖定）→ 發票號碼（advisory lock）→ 客戶（`FOR SHARE`）→ 關聯的出貨單
//! （依 id `FOR UPDATE`）。
//!
//! 1. 重新執行存檔時的檢查：號碼格式、統一編號、明細的數量與單價、關聯的出貨單（鎖定出貨單後再檢查一次
//!    「一張出貨單只能在一張有效發票中」，並行開立時後取得鎖的一方在這裡失敗）
//! 2. 三聯式的買受人統一編號、買受人必填；至少一列明細；客戶未刪除
//! 3. 發票號碼不重複（含已作廢）。以 advisory lock 鎖定號碼後查詢，所以並行開立同一個號碼時，後者在查詢時
//!    就會看到前者，不會撞上唯一索引
//!
//! 金額在存檔時已計算，開立時不重新計算（與 §8.8 相同）。
//!
//! # 作廢（`void`）
//!
//! request 帶 `void_reason`（必填）、`void_date`（省略時為作廢當天，不可早於發票日期）；有效的銷貨折讓
//! 指向此發票時不可作廢。檢查都通過後寫入作廢日期與原因，框架再變更狀態。跨期作廢不擋。

use hammer_db::invoicing::{self as db, InvoiceTable};
use hammer_db::lock::advisory_xact_lock;
use hammer_db::sales as sales_db;
use hammer_db::sqlx::PgConnection;
use hammer_shared::Id;
use hammer_shared::document::DocumentRef;
use serde_json::{Map, Value};

use super::save::{
    BUYER_NAME, BUYER_TAX_ID, CUSTOMER_ID, INVOICE_NO, INVOICE_TYPE, LinkCheck, check_header,
    field_error, id_of, link_errors, load_links,
};
use super::voiding::{self, void_details};
use super::{SALES_ALLOWANCE, SALES_INVOICE_LINE, SALES_INVOICE_SHIPMENT};
use crate::CoreError;
use crate::documents::{DocumentContext, DocumentHandler, HandlerFuture};
use crate::purchasing::save::line_errors as qty_price_errors;
use crate::record_data::FieldError;
use crate::record_lines::LineError;
use crate::sales::orders::MSG_CUSTOMER_DELETED;
use crate::stock_adjustments::MSG_NO_LINES;

/// 三聯式（單價未稅，必須有買受人統一編號）。
pub const TRIPLICATE: &str = "triplicate";
/// 三聯式沒有買受人統一編號。
pub const MSG_BUYER_TAX_ID_REQUIRED: &str = "三聯式發票必須填寫買受人統一編號";
/// 三聯式沒有買受人。
pub const MSG_BUYER_NAME_REQUIRED: &str = "三聯式發票必須填寫買受人";

/// 作廢日期早於發票日期。
pub const MSG_VOID_BEFORE_INVOICE: &str = "作廢日期不可早於發票日期";

/// 發票號碼 advisory lock 的鍵值前綴（與其他用途區隔）。
const INVOICE_NO_LOCK: i64 = 0x5349_0000_0000_0000; // "SI"

/// 銷項發票的開立、作廢處理函式（[`crate::documents::DocumentHandlers::standard`] 註冊）。
#[derive(Debug, Default, Clone, Copy)]
pub struct SalesInvoiceHandler;

impl DocumentHandler for SalesInvoiceHandler {
    fn post<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        doc: &'a DocumentContext<'a>,
    ) -> HandlerFuture<'a> {
        Box::pin(post(conn, doc))
    }

    fn void<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        doc: &'a DocumentContext<'a>,
    ) -> HandlerFuture<'a> {
        Box::pin(void(conn, doc))
    }

    fn accepts_void_details(&self) -> bool {
        true
    }
}

/// 發票號碼的 advisory lock 鍵值（FNV-1a 雜湊的低 48 位元加上前綴）。
pub(crate) fn invoice_no_lock_key(prefix: i64, invoice_no: &str) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in invoice_no.bytes() {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    prefix | i64::try_from(hash & 0x0000_ffff_ffff_ffff).unwrap_or(0)
}

/// 三聯式的買受人必填。
fn buyer_errors(data: &Map<String, Value>) -> Vec<FieldError> {
    if data.get(INVOICE_TYPE).and_then(Value::as_str) != Some(TRIPLICATE) {
        return Vec::new();
    }
    let blank = |key: &str| {
        data.get(key)
            .and_then(Value::as_str)
            .is_none_or(|s| s.trim().is_empty())
    };
    let mut errors = Vec::new();
    if blank(BUYER_TAX_ID) {
        errors.push(field_error(BUYER_TAX_ID, MSG_BUYER_TAX_ID_REQUIRED));
    }
    if blank(BUYER_NAME) {
        errors.push(field_error(BUYER_NAME, MSG_BUYER_NAME_REQUIRED));
    }
    errors
}

async fn post(conn: &mut PgConnection, doc: &DocumentContext<'_>) -> Result<(), CoreError> {
    let mut data = doc.data.clone();
    let mut fields = check_header(super::SALES_INVOICE, &mut data);
    fields.extend(buyer_errors(&data));
    let invoice_no = data
        .get(INVOICE_NO)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    advisory_xact_lock(
        &mut *conn,
        invoice_no_lock_key(INVOICE_NO_LOCK, &invoice_no),
    )
    .await?;

    let customer_id = id_of(data.get(CUSTOMER_ID));
    let customer_deleted = match customer_id {
        Some(id) => sales_db::lock_customer(&mut *conn, id).await? != Some(false),
        None => true,
    };
    if customer_deleted {
        fields.push(field_error(CUSTOMER_ID, MSG_CUSTOMER_DELETED));
    }

    let mut lines = Vec::new();
    let rows = db::list_sales_invoice_lines(&mut *conn, doc.record_id).await?;
    if rows.is_empty() {
        lines.push(LineError::detail(SALES_INVOICE_LINE, MSG_NO_LINES));
    }
    for (index, row) in rows.iter().enumerate() {
        lines.extend(LineError::from_fields(
            SALES_INVOICE_LINE,
            index,
            qty_price_errors(Some(row.qty), Some(row.unit_price)),
        ));
    }

    let ids = db::invoice_shipment_ids(&mut *conn, doc.record_id).await?;
    let (shipments, invoiced) = load_links(conn, &ids, Some(doc.record_id), true).await?;
    let link_rows: Vec<(usize, Option<Id>)> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| (i, Some(*id)))
        .collect();
    let mut link_lines = link_errors(&LinkCheck {
        customer_id,
        rows: &link_rows,
        shipments: &shipments,
        invoiced: &invoiced,
    });
    for (index, id) in &link_rows {
        if id.is_some_and(|id| !shipments.contains_key(&id)) {
            link_lines.push(LineError::field(
                SALES_INVOICE_SHIPMENT,
                *index,
                super::save::SHIPMENT_ID,
                "出貨單已刪除",
            ));
        }
    }
    lines.extend(link_lines);

    if fields.iter().all(|f| f.field_key != INVOICE_NO)
        && let Some((_, record_no)) =
            db::find_sales_invoice_by_no(&mut *conn, &invoice_no, doc.record_id).await?
    {
        fields.push(field_error(
            INVOICE_NO,
            format!("發票號碼 {invoice_no} 已使用（{record_no}）"),
        ));
    }
    voiding::validation(fields, lines)
}

async fn void(conn: &mut PgConnection, doc: &DocumentContext<'_>) -> Result<(), CoreError> {
    void_details(doc, "invoice_date", MSG_VOID_BEFORE_INVOICE)?;
    let allowances = db::posted_allowances_of_sales_invoice(&mut *conn, doc.record_id).await?;
    if !allowances.is_empty() {
        let numbers: Vec<&str> = allowances.iter().map(|(_, no)| no.as_str()).collect();
        return Err(CoreError::DocumentsInUse {
            message: format!("此發票已有銷貨折讓 {}，請先作廢折讓單", numbers.join("、")),
            documents: allowances
                .iter()
                .map(|(id, no)| DocumentRef {
                    entity_key: SALES_ALLOWANCE.into(),
                    record_id: *id,
                    record_no: no.clone(),
                })
                .collect(),
        });
    }
    voiding::write_void_details(
        conn,
        doc,
        InvoiceTable::SalesInvoices,
        "invoice_date",
        MSG_VOID_BEFORE_INVOICE,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn map(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn triplicate_requires_buyer() {
        let errors = buyer_errors(&map(
            json!({ "invoice_type": "triplicate", "buyer_name": " " }),
        ));
        let keys: Vec<&str> = errors.iter().map(|e| e.field_key.as_str()).collect();
        assert_eq!(keys, [BUYER_TAX_ID, BUYER_NAME]);
        assert_eq!(errors[0].message, MSG_BUYER_TAX_ID_REQUIRED);
        assert!(
            buyer_errors(&map(json!({
                "invoice_type": "triplicate", "buyer_tax_id": "12345678", "buyer_name": "甲"
            })))
            .is_empty()
        );
        assert!(buyer_errors(&map(json!({ "invoice_type": "duplicate" }))).is_empty());
    }

    #[test]
    fn lock_keys_differ_by_number_and_keep_prefix() {
        let a = invoice_no_lock_key(INVOICE_NO_LOCK, "AB12345678");
        let b = invoice_no_lock_key(INVOICE_NO_LOCK, "AB12345679");
        assert_ne!(a, b);
        assert_eq!(a & !0x0000_ffff_ffff_ffff, INVOICE_NO_LOCK);
        assert_eq!(a, invoice_no_lock_key(INVOICE_NO_LOCK, "AB12345678"));
    }
}
