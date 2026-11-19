//! 進項憑證的登錄（開立）與作廢（docs/architecture.md §8.11「開立與作廢」，[`crate::documents`] 的框架上）。
//!
//! 與銷項發票相同（[`super::sales_invoices`]），差別：
//!
//! - 沒有明細，不檢查「至少一筆明細」與買受人；未稅金額必須大於 0（海關繳納證也相同）
//! - 號碼的唯一範圍是同一廠商 + 號碼、只算已登錄的（作廢後可以重新登錄）
//! - 關聯的進貨單：發票類（三聯式、電子發票）與海關繳納證各自只能一張有效
//!
//! 鎖定順序：憑證（框架以 `FOR UPDATE` 鎖定）→ 廠商 + 號碼（advisory lock）→ 廠商（`FOR SHARE`）→ 關聯的
//! 進貨單（依 id `FOR UPDATE`）。

use hammer_db::invoicing::{self as db, InvoiceTable};
use hammer_db::lock::advisory_xact_lock;
use hammer_db::purchasing as purchasing_db;
use hammer_db::sqlx::PgConnection;
use hammer_shared::decimal::Decimal;
use hammer_shared::document::DocumentRef;
use serde_json::Value;

use super::sales_invoices::{invoice_no_lock_key, linked_errors};
use super::save::{
    INVOICE_NO, MSG_SUBTOTAL_POSITIVE, RECEIPT_LINKS, SUPPLIER_ID, check_header, decimal_of,
    field_error, id_of, is_customs,
};
use super::voiding;
use super::{PURCHASE_ALLOWANCE, PURCHASE_INVOICE};
use crate::CoreError;
use crate::documents::{DocumentContext, DocumentHandler, HandlerFuture};
use crate::purchasing::orders::MSG_SUPPLIER_DELETED;

/// 作廢日期早於憑證日期。
pub const MSG_VOID_BEFORE_PURCHASE_INVOICE: &str = "作廢日期不可早於憑證日期";

/// 憑證號碼 advisory lock 的鍵值前綴（與銷項發票區隔）。
const INVOICE_NO_LOCK: i64 = 0x5049_0000_0000_0000; // "PI"

/// 進項憑證的登錄、作廢處理函式（[`crate::documents::DocumentHandlers::standard`] 註冊）。
#[derive(Debug, Default, Clone, Copy)]
pub struct PurchaseInvoiceHandler;

impl DocumentHandler for PurchaseInvoiceHandler {
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

async fn post(conn: &mut PgConnection, doc: &DocumentContext<'_>) -> Result<(), CoreError> {
    let mut data = doc.data.clone();
    let mut fields = check_header(PURCHASE_INVOICE, &mut data);
    let invoice_no = data
        .get(INVOICE_NO)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let supplier_id = id_of(data.get(SUPPLIER_ID));
    let key = format!(
        "{}:{invoice_no}",
        supplier_id.map(|id| id.to_string()).unwrap_or_default()
    );
    advisory_xact_lock(&mut *conn, invoice_no_lock_key(INVOICE_NO_LOCK, &key)).await?;

    let supplier_deleted = match supplier_id {
        Some(id) => purchasing_db::lock_supplier(&mut *conn, id).await? != Some(false),
        None => true,
    };
    if supplier_deleted {
        fields.push(field_error(SUPPLIER_ID, MSG_SUPPLIER_DELETED));
    }
    if decimal_of(data.get("subtotal")).is_none_or(|v| v <= Decimal::ZERO) {
        fields.push(field_error("subtotal", MSG_SUBTOTAL_POSITIVE));
    }

    let ids = db::invoice_receipt_ids(&mut *conn, doc.record_id).await?;
    let lines = linked_errors(
        conn,
        &RECEIPT_LINKS,
        &ids,
        supplier_id,
        doc.record_id,
        is_customs(&data),
    )
    .await?;

    if let Some(supplier_id) = supplier_id
        && fields.iter().all(|f| f.field_key != INVOICE_NO)
        && let Some((_, record_no)) =
            db::find_purchase_invoice_by_no(&mut *conn, supplier_id, &invoice_no, doc.record_id)
                .await?
    {
        fields.push(field_error(
            INVOICE_NO,
            format!("此廠商的憑證號碼 {invoice_no} 已登錄（{record_no}）"),
        ));
    }
    voiding::validation(fields, lines)
}

async fn void(conn: &mut PgConnection, doc: &DocumentContext<'_>) -> Result<(), CoreError> {
    voiding::void_details(doc, "invoice_date", MSG_VOID_BEFORE_PURCHASE_INVOICE)?;
    let allowances = db::posted_allowances_of_purchase_invoice(&mut *conn, doc.record_id).await?;
    if !allowances.is_empty() {
        let numbers: Vec<&str> = allowances.iter().map(|(_, no)| no.as_str()).collect();
        return Err(CoreError::DocumentsInUse {
            message: format!("此憑證已有進貨折讓 {}，請先作廢折讓單", numbers.join("、")),
            documents: allowances
                .iter()
                .map(|(id, no)| DocumentRef {
                    entity_key: PURCHASE_ALLOWANCE.into(),
                    record_id: *id,
                    record_no: no.clone(),
                })
                .collect(),
        });
    }
    voiding::write_void_details(
        conn,
        doc,
        InvoiceTable::PurchaseInvoices,
        "invoice_date",
        MSG_VOID_BEFORE_PURCHASE_INVOICE,
    )
    .await
}
