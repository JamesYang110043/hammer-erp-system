//! 折讓（銷貨折讓、進貨折讓，docs/architecture.md §8.11「銷貨折讓」「進貨折讓」）的檢查與開立、作廢
//! （[`SalesAllowanceHandler`]、[`PurchaseAllowanceHandler`]）。
//!
//! 進貨折讓與銷貨折讓相同，差別：原憑證不可是海關繳納證；沒有明細，未稅金額必須大於 0。
//!
//! # 檢查（存檔與開立都執行）
//!
//! - 原發票（憑證）必須已開立（[`invoice_errors`]）；折讓日期不可早於發票日期
//! - 關聯的退回單：已過帳、同客戶（廠商）、沒有其他有效的折讓關聯它（[`check_return`]）
//! - **累計上限**：`total ≤ 發票總計 − 其他已開立折讓的 total 合計`（[`cap_error`]）
//!
//! # 開立（`post`）
//!
//! 鎖定順序：折讓（框架以 `FOR UPDATE` 鎖定）→ 原發票（`FOR UPDATE`，同一張發票的折讓依序開立）→ 退回單
//! （`FOR UPDATE`）。在鎖定發票之後計算已折讓金額，所以並行開立同一張發票的折讓時，後者會看到前者。
//!
//! # 作廢（`void`）
//!
//! 隨時可以作廢；`void_reason` 必填，`void_date` 不可早於折讓日期。

use hammer_db::invoicing::{self as db, InvoiceForAllowance, InvoiceTable, ReturnForAllowance};
use hammer_db::sqlx::PgConnection;
use hammer_shared::Id;
use hammer_shared::decimal::Decimal;
use serde_json::{Map, Value};
use time::Date;

use super::amounts::display_grouped;
use super::save::{CUSTOMS, MSG_SUBTOTAL_POSITIVE, date_of, decimal_of, field_error, id_of};
use super::voiding;
use super::{PURCHASE_ALLOWANCE, SALES_ALLOWANCE, SALES_ALLOWANCE_LINE};
use crate::CoreError;
use crate::documents::{DocumentContext, DocumentHandler, HandlerFuture};
use crate::purchasing::save::line_errors as qty_price_errors;
use crate::record_data::FieldError;
use crate::record_lines::LineError;
use crate::sales::save::sales_places;
use crate::stock_adjustments::MSG_NO_LINES;

/// 折讓日期欄位。
pub const ALLOWANCE_DATE: &str = "allowance_date";
/// 進貨折讓指向海關繳納證。
pub const MSG_CUSTOMS_ALLOWANCE: &str = "海關繳納證不可折讓";
/// 作廢日期早於折讓日期。
pub const MSG_VOID_BEFORE_ALLOWANCE: &str = "作廢日期不可早於折讓日期";

/// 一種折讓與原發票、退回單的對應（欄位與訊息）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct AllowanceSource {
    pub entity_key: &'static str,
    pub invoice_field: &'static str,
    pub party_field: &'static str,
    pub return_field: &'static str,
    pub fixed_message: &'static str,
    pub draft_message: &'static str,
    pub void_message: &'static str,
    pub date_message: &'static str,
    /// 訊息中的「發票」或「憑證」。
    pub invoice_word: &'static str,
    /// 訊息中的「銷貨退回」或「進貨退回」。
    pub return_word: &'static str,
    /// 退回單的客戶（廠商）不同。
    pub party_message: &'static str,
}

/// 銷貨折讓。
pub(crate) const SALES: AllowanceSource = AllowanceSource {
    entity_key: SALES_ALLOWANCE,
    invoice_field: "sales_invoice_id",
    party_field: "customer_id",
    return_field: "sales_return_id",
    fixed_message: super::save::MSG_INVOICE_FIXED,
    draft_message: "發票尚未開立",
    void_message: "發票已作廢",
    date_message: "折讓日期不可早於發票日期",
    invoice_word: "發票",
    return_word: "銷貨退回",
    party_message: "客戶與發票不同",
};

/// 進貨折讓。
pub(crate) const PURCHASE: AllowanceSource = AllowanceSource {
    entity_key: PURCHASE_ALLOWANCE,
    invoice_field: "purchase_invoice_id",
    party_field: "supplier_id",
    return_field: "purchase_return_id",
    fixed_message: super::save::MSG_PURCHASE_INVOICE_FIXED,
    draft_message: "憑證尚未登錄",
    void_message: "憑證已作廢",
    date_message: "折讓日期不可早於憑證日期",
    invoice_word: "憑證",
    return_word: "進貨退回",
    party_message: "廠商與憑證不同",
};

/// 折讓的資料類別對應；不是折讓時為 `None`。
pub(crate) fn source_of(entity_key: &str) -> Option<&'static AllowanceSource> {
    match entity_key {
        SALES_ALLOWANCE => Some(&SALES),
        PURCHASE_ALLOWANCE => Some(&PURCHASE),
        _ => None,
    }
}

/// 原發票的狀態與日期（純邏輯）。
pub(crate) fn invoice_errors(
    source: &AllowanceSource,
    invoice: &InvoiceForAllowance,
    allowance_date: Option<Date>,
) -> Vec<FieldError> {
    let mut errors = Vec::new();
    match invoice.status.as_str() {
        "posted" => {}
        "void" => errors.push(field_error(source.invoice_field, source.void_message)),
        _ => errors.push(field_error(source.invoice_field, source.draft_message)),
    }
    if source.entity_key == PURCHASE_ALLOWANCE && invoice.doc_type == CUSTOMS {
        errors.push(field_error(source.invoice_field, MSG_CUSTOMS_ALLOWANCE));
    }
    if allowance_date.is_some_and(|d| d < invoice.invoice_date) {
        errors.push(field_error(ALLOWANCE_DATE, source.date_message));
    }
    errors
}

/// 退回單的檢查（純邏輯）：已過帳、與原發票同一個客戶（廠商）、沒有其他有效的折讓（`taken` 為那些折讓的
/// 單號）。
pub(crate) fn return_error(
    source: &AllowanceSource,
    ret: &ReturnForAllowance,
    party_id: Option<Id>,
    taken: &[String],
) -> Option<FieldError> {
    let message = match ret.status.as_str() {
        "posted" => {
            if party_id.is_some_and(|p| p != ret.party_id) {
                source.party_message.to_owned()
            } else {
                let no = taken.first()?;
                format!("{} {} 已有折讓單 {no}", source.return_word, ret.record_no)
            }
        }
        "void" => format!("{}已作廢", source.return_word),
        _ => format!("{}尚未過帳", source.return_word),
    };
    Some(field_error(source.return_field, message))
}

/// 累計上限（純邏輯）：`total` 超過 `invoice_total − used` 時為 `total` 的欄位錯誤。
pub(crate) fn cap_error(
    source: &AllowanceSource,
    total: Decimal,
    invoice_total: Decimal,
    used: Decimal,
    decimal_places: u32,
) -> Option<FieldError> {
    let available = invoice_total - used;
    if total <= available {
        return None;
    }
    Some(field_error(
        "total",
        format!(
            "超過可折讓金額 {}（{}總計 {}，已折讓 {}）",
            display_grouped(available.max(Decimal::ZERO), decimal_places),
            source.invoice_word,
            display_grouped(invoice_total, decimal_places),
            display_grouped(used, decimal_places),
        ),
    ))
}

/// 讀取並檢查關聯的退回單（`lock` 為開立時以 `FOR UPDATE` 鎖定）。沒有關聯時為 `None`；退回單不存在交給
/// 關聯檢查。`exclude` 為不列入「其他有效折讓」的折讓（開立時為自己）。
pub(crate) async fn check_return(
    conn: &mut PgConnection,
    source: &AllowanceSource,
    data: &Map<String, Value>,
    party_id: Option<Id>,
    exclude: Option<Id>,
    lock: bool,
) -> Result<Option<FieldError>, CoreError> {
    let Some(return_id) = id_of(data.get(source.return_field)) else {
        return Ok(None);
    };
    let (ret, taken) = if source.entity_key == SALES_ALLOWANCE {
        let ret = db::find_sales_return_for_allowance(&mut *conn, return_id, lock).await?;
        let taken = db::posted_allowances_of_sales_return(&mut *conn, return_id, exclude).await?;
        (ret, taken)
    } else {
        let ret = db::find_purchase_return_for_allowance(&mut *conn, return_id, lock).await?;
        let taken =
            db::posted_allowances_of_purchase_return(&mut *conn, return_id, exclude).await?;
        (ret, taken)
    };
    let taken: Vec<String> = taken.into_iter().map(|(_, no)| no).collect();
    Ok(ret.and_then(|ret| return_error(source, &ret, party_id, &taken)))
}

/// 存檔時的累計上限（新增與修改都執行；在金額計算之後）。原發票不是已開立時不檢查（已有欄位錯誤）。
pub(crate) async fn check_cap_on_save(
    conn: &mut PgConnection,
    entity_key: &str,
    data: &Map<String, Value>,
) -> Result<(), CoreError> {
    let Some(source) = source_of(entity_key) else {
        return Ok(());
    };
    let (Some(invoice_id), Some(total)) = (
        id_of(data.get(source.invoice_field)),
        decimal_of(data.get("total")),
    ) else {
        return Ok(());
    };
    let invoice = if source.entity_key == SALES_ALLOWANCE {
        db::find_sales_invoice_for_allowance(&mut *conn, invoice_id).await?
    } else {
        db::find_purchase_invoice_for_allowance(&mut *conn, invoice_id).await?
    };
    let Some(invoice) = invoice else {
        return Ok(());
    };
    if invoice.status != "posted" {
        return Ok(());
    }
    // 存檔的折讓一定是草稿，不在「已開立」中，不需要排除自己
    let used = used_amount(conn, source, invoice_id, None).await?;
    let places = sales_places(&mut *conn).await?;
    match cap_error(source, total, invoice.total, used, places) {
        Some(error) => Err(CoreError::InvalidFields(vec![error])),
        None => Ok(()),
    }
}

/// 原發票（憑證）已開立折讓的 `total` 合計（`exclude` 除外）。
async fn used_amount(
    conn: &mut PgConnection,
    source: &AllowanceSource,
    invoice_id: Id,
    exclude: Option<Id>,
) -> Result<Decimal, CoreError> {
    Ok(if source.entity_key == SALES_ALLOWANCE {
        db::posted_allowance_total_of_sales_invoice(&mut *conn, invoice_id, exclude).await?
    } else {
        db::posted_allowance_total_of_purchase_invoice(&mut *conn, invoice_id, exclude).await?
    })
}

/// 銷貨折讓的開立、作廢處理函式（[`crate::documents::DocumentHandlers::standard`] 註冊）。
#[derive(Debug, Default, Clone, Copy)]
pub struct SalesAllowanceHandler;

impl DocumentHandler for SalesAllowanceHandler {
    fn post<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        doc: &'a DocumentContext<'a>,
    ) -> HandlerFuture<'a> {
        Box::pin(post(conn, doc, &SALES))
    }

    fn void<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        doc: &'a DocumentContext<'a>,
    ) -> HandlerFuture<'a> {
        Box::pin(void(conn, doc, InvoiceTable::SalesAllowances))
    }

    fn accepts_void_details(&self) -> bool {
        true
    }
}

/// 進貨折讓的開立、作廢處理函式（[`crate::documents::DocumentHandlers::standard`] 註冊）。
#[derive(Debug, Default, Clone, Copy)]
pub struct PurchaseAllowanceHandler;

impl DocumentHandler for PurchaseAllowanceHandler {
    fn post<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        doc: &'a DocumentContext<'a>,
    ) -> HandlerFuture<'a> {
        Box::pin(post(conn, doc, &PURCHASE))
    }

    fn void<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        doc: &'a DocumentContext<'a>,
    ) -> HandlerFuture<'a> {
        Box::pin(void(conn, doc, InvoiceTable::PurchaseAllowances))
    }

    fn accepts_void_details(&self) -> bool {
        true
    }
}

async fn post(
    conn: &mut PgConnection,
    doc: &DocumentContext<'_>,
    source: &AllowanceSource,
) -> Result<(), CoreError> {
    let sales = source.entity_key == SALES_ALLOWANCE;
    let mut fields = Vec::new();
    let invoice_id = id_of(doc.data.get(source.invoice_field))
        .ok_or_else(|| CoreError::Internal("折讓沒有原發票".into()))?;
    let invoice = if sales {
        db::lock_sales_invoice_for_allowance(&mut *conn, invoice_id).await?
    } else {
        db::lock_purchase_invoice_for_allowance(&mut *conn, invoice_id).await?
    };
    let Some(invoice) = invoice else {
        fields.push(field_error(
            source.invoice_field,
            format!("{}已刪除", source.invoice_word),
        ));
        return voiding::validation(fields, Vec::new());
    };
    fields.extend(invoice_errors(
        source,
        &invoice,
        date_of(doc.data.get(ALLOWANCE_DATE)),
    ));
    fields.extend(
        check_return(
            conn,
            source,
            doc.data,
            Some(invoice.party_id),
            Some(doc.record_id),
            true,
        )
        .await?,
    );

    let mut lines = Vec::new();
    if sales {
        let rows = db::list_sales_allowance_lines(&mut *conn, doc.record_id).await?;
        if rows.is_empty() {
            lines.push(LineError::detail(SALES_ALLOWANCE_LINE, MSG_NO_LINES));
        }
        for (index, row) in rows.iter().enumerate() {
            lines.extend(LineError::from_fields(
                SALES_ALLOWANCE_LINE,
                index,
                qty_price_errors(Some(row.qty), Some(row.unit_price)),
            ));
        }
    } else if decimal_of(doc.data.get("subtotal")).is_none_or(|v| v <= Decimal::ZERO) {
        fields.push(field_error("subtotal", MSG_SUBTOTAL_POSITIVE));
    }

    if invoice.status == "posted" {
        let total = decimal_of(doc.data.get("total")).unwrap_or(Decimal::ZERO);
        let used = used_amount(conn, source, invoice_id, Some(doc.record_id)).await?;
        let places = sales_places(&mut *conn).await?;
        fields.extend(cap_error(source, total, invoice.total, used, places));
    }
    voiding::validation(fields, lines)
}

async fn void(
    conn: &mut PgConnection,
    doc: &DocumentContext<'_>,
    table: InvoiceTable,
) -> Result<(), CoreError> {
    voiding::write_void_details(conn, doc, table, ALLOWANCE_DATE, MSG_VOID_BEFORE_ALLOWANCE).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;
    use time::macros::date;
    use uuid::Uuid;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn invoice(status: &str) -> InvoiceForAllowance {
        InvoiceForAllowance {
            id: Uuid::nil(),
            status: status.into(),
            party_id: Uuid::from_u128(1),
            tax_type: "taxable".into(),
            doc_type: "triplicate".into(),
            invoice_date: date!(2026 - 11 - 19),
            total: d("2100"),
        }
    }

    fn ret(status: &str, party: u128) -> ReturnForAllowance {
        ReturnForAllowance {
            id: Uuid::nil(),
            record_no: "SR202611-0002".into(),
            status: status.into(),
            party_id: Uuid::from_u128(party),
        }
    }

    fn messages(errors: Vec<FieldError>) -> Vec<(String, String)> {
        errors
            .into_iter()
            .map(|e| (e.field_key, e.message))
            .collect()
    }

    #[test]
    fn invoice_must_be_posted_and_not_later() {
        assert!(invoice_errors(&SALES, &invoice("posted"), Some(date!(2026 - 11 - 19))).is_empty());
        assert_eq!(
            messages(invoice_errors(
                &SALES,
                &invoice("draft"),
                Some(date!(2026 - 11 - 18))
            )),
            [
                ("sales_invoice_id".into(), "發票尚未開立".into()),
                ("allowance_date".into(), "折讓日期不可早於發票日期".into()),
            ]
        );
        assert_eq!(
            messages(invoice_errors(&PURCHASE, &invoice("void"), None)),
            [("purchase_invoice_id".into(), "憑證已作廢".into())]
        );
    }

    #[test]
    fn return_must_be_posted_same_party_and_free() {
        let party = Some(Uuid::from_u128(1));
        assert_eq!(return_error(&SALES, &ret("posted", 1), party, &[]), None);
        let msg = |r: &ReturnForAllowance, taken: &[String]| {
            return_error(&SALES, r, party, taken).unwrap().message
        };
        assert_eq!(msg(&ret("draft", 1), &[]), "銷貨退回尚未過帳");
        assert_eq!(msg(&ret("void", 1), &[]), "銷貨退回已作廢");
        assert_eq!(msg(&ret("posted", 2), &[]), "客戶與發票不同");
        assert_eq!(
            msg(&ret("posted", 1), &["SA202611-0001".into()]),
            "銷貨退回 SR202611-0002 已有折讓單 SA202611-0001"
        );
        assert_eq!(
            return_error(&SALES, &ret("void", 1), party, &[])
                .unwrap()
                .field_key,
            "sales_return_id"
        );
    }

    #[test]
    fn cap_lists_available_amount() {
        assert_eq!(cap_error(&SALES, d("1050"), d("2100"), d("1050"), 0), None);
        let error = cap_error(&SALES, d("1051"), d("2100"), d("1050"), 0).unwrap();
        assert_eq!(error.field_key, "total");
        assert_eq!(
            error.message,
            "超過可折讓金額 1,050（發票總計 2,100，已折讓 1,050）"
        );
        assert_eq!(
            cap_error(&PURCHASE, d("1"), d("100"), d("100"), 0)
                .unwrap()
                .message,
            "超過可折讓金額 0（憑證總計 100，已折讓 100）"
        );
    }
}
