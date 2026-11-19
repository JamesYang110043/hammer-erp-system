//! 發票模組的查詢 API（docs/architecture.md §8.11「查詢 API」）。都需要 `record.read`。
//!
//! - [`list_uninvoiced_shipments`]：尚未開立發票的已過帳出貨單（建立銷項發票時勾選）
//! - [`list_uninvoiced_receipts`]：尚未取得該類憑證的已過帳進貨單（建立進項憑證時勾選）
//! - [`list_links`]：關聯某張出貨單、進貨單、退回單的發票、憑證或折讓（含已作廢）

use hammer_db::invoicing::{self as db, InvoiceLinkRow, UninvoicedFilter};
use hammer_db::sqlx::PgPool;
use hammer_shared::document::DocumentStatus;
use hammer_shared::invoice::{
    DocClass, InvoiceLink, InvoiceLinkQuery, UninvoicedReceipt, UninvoicedReceiptQuery,
    UninvoicedShipment, UninvoicedShipmentQuery, query_field,
};
use hammer_shared::pagination::Page;
use hammer_shared::permission;
use time::Date;

use super::{PURCHASE_ALLOWANCE, PURCHASE_INVOICE, SALES_ALLOWANCE, SALES_INVOICE};
use crate::CoreError;
use crate::auth::AuthContext;
use crate::inventory_queries::{invalid, non_empty, page_of, parse_date, parse_id, total_of};
use crate::purchasing::amounts::{AMOUNT_SCALE, with_scale};
use crate::purchasing::{GOODS_RECEIPT, PURCHASE_RETURN};
use crate::sales::{SALES_RETURN, SHIPMENT};

/// 單據不存在或已刪除（`links`）。
pub const MSG_RECORD_NOT_FOUND: &str = "找不到此單據";
/// `links` 的 `entity_key` 不是可查詢的單據。
pub const MSG_LINK_ENTITY: &str = "必須是 shipment、goods_receipt、sales_return 或 purchase_return";

fn dates(
    date_from: Option<&str>,
    date_to: Option<&str>,
) -> Result<(Option<Date>, Option<Date>), CoreError> {
    let from = parse_date(query_field::DATE_FROM, date_from)?;
    let to = parse_date(query_field::DATE_TO, date_to)?;
    if let (Some(f), Some(t)) = (from, to)
        && f > t
    {
        return Err(invalid(query_field::DATE_FROM, "開始日期不可晚於結束日期"));
    }
    Ok((from, to))
}

/// 尚未開立發票的出貨單（分頁，依 `ship_date, id` 排序）。
pub async fn list_uninvoiced_shipments(
    pool: &PgPool,
    ctx: &AuthContext,
    query: &UninvoicedShipmentQuery,
) -> Result<Page<UninvoicedShipment>, CoreError> {
    ctx.require(permission::RECORD_READ)?;
    let page = page_of(query.page, query.page_size)?;
    let (date_from, date_to) = dates(query.date_from.as_deref(), query.date_to.as_deref())?;
    let filter = UninvoicedFilter {
        party_id: parse_id(query_field::CUSTOMER_ID, query.customer_id.as_deref())?,
        date_from,
        date_to,
        customs: false,
    };
    let total = db::count_uninvoiced_shipments(pool, &filter).await?;
    let rows = db::list_uninvoiced_shipments(pool, &filter, page.limit(), page.offset()).await?;
    let items = rows
        .into_iter()
        .map(|r| UninvoicedShipment {
            net_total: with_scale(r.total - r.returned_total, AMOUNT_SCALE),
            returned_total: with_scale(r.returned_total, AMOUNT_SCALE),
            id: r.id,
            record_no: r.record_no,
            ship_date: r.ship_date,
            customer_id: r.customer_id,
            customer_code: r.customer_code,
            customer_name: r.customer_name,
            tax_type: r.tax_type,
            tax_included: r.tax_included,
            subtotal: r.subtotal,
            tax_amount: r.tax_amount,
            total: r.total,
        })
        .collect();
    Ok(Page::new(items, total_of(total), page))
}

/// 尚未取得該類憑證的進貨單（分頁，依 `receipt_date, id` 排序）。
pub async fn list_uninvoiced_receipts(
    pool: &PgPool,
    ctx: &AuthContext,
    query: &UninvoicedReceiptQuery,
) -> Result<Page<UninvoicedReceipt>, CoreError> {
    ctx.require(permission::RECORD_READ)?;
    let page = page_of(query.page, query.page_size)?;
    let (date_from, date_to) = dates(query.date_from.as_deref(), query.date_to.as_deref())?;
    let class = match non_empty(query.doc_class.as_deref()) {
        None => DocClass::Invoice,
        Some(s) => DocClass::parse(s)
            .ok_or_else(|| invalid(query_field::DOC_CLASS, "必須是 invoice 或 customs"))?,
    };
    let filter = UninvoicedFilter {
        party_id: parse_id(query_field::SUPPLIER_ID, query.supplier_id.as_deref())?,
        date_from,
        date_to,
        customs: class == DocClass::Customs,
    };
    let total = db::count_uninvoiced_receipts(pool, &filter).await?;
    let rows = db::list_uninvoiced_receipts(pool, &filter, page.limit(), page.offset()).await?;
    let items = rows
        .into_iter()
        .map(|r| UninvoicedReceipt {
            net_total: with_scale(r.total - r.returned_total, AMOUNT_SCALE),
            returned_total: with_scale(r.returned_total, AMOUNT_SCALE),
            id: r.id,
            record_no: r.record_no,
            receipt_date: r.receipt_date,
            supplier_id: r.supplier_id,
            supplier_code: r.supplier_code,
            supplier_name: r.supplier_name,
            currency_code: r.currency_code,
            exchange_rate: r.exchange_rate,
            tax_type: r.tax_type,
            total: r.total,
            total_twd: r.total_twd,
        })
        .collect();
    Ok(Page::new(items, total_of(total), page))
}

/// 關聯單據的發票、憑證或折讓（含已作廢，不分頁）。單據不存在或已刪除 → `404`。
pub async fn list_links(
    pool: &PgPool,
    ctx: &AuthContext,
    query: &InvoiceLinkQuery,
) -> Result<Vec<InvoiceLink>, CoreError> {
    ctx.require(permission::RECORD_READ)?;
    let entity_key = non_empty(query.entity_key.as_deref())
        .ok_or_else(|| invalid(query_field::ENTITY_KEY, "此欄位為必填"))?;
    let link_key = match entity_key {
        SHIPMENT => SALES_INVOICE,
        GOODS_RECEIPT => PURCHASE_INVOICE,
        SALES_RETURN => SALES_ALLOWANCE,
        PURCHASE_RETURN => PURCHASE_ALLOWANCE,
        _ => return Err(invalid(query_field::ENTITY_KEY, MSG_LINK_ENTITY)),
    };
    let id = parse_id(query_field::RECORD_ID, query.record_id.as_deref())?
        .ok_or_else(|| invalid(query_field::RECORD_ID, "此欄位為必填"))?;
    if !db::linkable_document_exists(pool, entity_key, id).await? {
        return Err(CoreError::NotFound(MSG_RECORD_NOT_FOUND.into()));
    }
    let rows = match entity_key {
        SHIPMENT => db::invoices_of_shipment(pool, id).await?,
        GOODS_RECEIPT => db::invoices_of_receipt(pool, id).await?,
        SALES_RETURN => db::allowances_of_sales_return(pool, id).await?,
        _ => db::allowances_of_purchase_return(pool, id).await?,
    };
    rows.into_iter().map(|r| to_link(link_key, r)).collect()
}

fn to_link(entity_key: &str, row: InvoiceLinkRow) -> Result<InvoiceLink, CoreError> {
    let status = DocumentStatus::parse(&row.status)
        .ok_or_else(|| CoreError::Internal(format!("不認得的單據狀態：{}", row.status)))?;
    Ok(InvoiceLink {
        entity_key: entity_key.into(),
        record_id: row.record_id,
        record_no: row.record_no,
        invoice_no: row.invoice_no,
        invoice_date: row.invoice_date,
        status,
    })
}
