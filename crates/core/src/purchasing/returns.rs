//! 進貨退回（系統單據 `purchase_return`，docs/architecture.md §8.8「進貨退回」、§9.3）：存檔時從進貨單
//! 帶入與檢查，以及過帳、作廢（[`PurchaseReturnHandler`]，掛在 [`crate::documents`] 的框架上）。
//!
//! # 存檔（[`derive_return`]）
//!
//! - 表頭的 `supplier_id`、`currency_code`、`exchange_rate`、`tax_type`，明細的 `product_id`、`lot_no`、
//!   `unit_price` 從進貨單、進貨明細帶入（唯讀的系統欄位，request 帶了一律忽略，§7.1）。金額依
//!   [`super::amounts`] 計算（[`super::save::apply_amounts`]），`total_twd` 使用進貨單的匯率
//! - 進貨單必須是已過帳（「進貨單尚未過帳」「進貨單已作廢」），建立後不可更換；退回日期不可早於進貨日期
//! - 明細的 `goods_receipt_line_id` 必須是該進貨單的明細，同一列進貨明細只能出現一次（「進貨明細重複」）
//! - 退回數量上限（[`check_quantities`]）：`可退數量 = 進貨明細的 qty − 其他已過帳退回的 qty 合計`
//!   （草稿不保留數量）。存檔時不檢查庫存
//!
//! # 過帳
//!
//! 鎖定順序：退回單（框架已 `FOR UPDATE`）→ 進貨單（`FOR UPDATE`，同一張進貨單的退回依序過帳）→ 明細
//! （`FOR UPDATE`）→ 商品（`FOR SHARE`）→ 批號（`FOR UPDATE`）。重新執行存檔時的檢查與數量上限，至少
//! 一列明細。之後依 `sort_order`：
//!
//! - `goods`：從進貨明細的批號扣除（批號扣除明細 `purchase_return_line`、異動 `purchase_return`、
//!   `txn_date = return_date`）；`qty_remaining` 不足時記下並繼續，最後 `422 STOCK_INSUFFICIENT` 列出全部
//! - `service`：不處理庫存
//! - 明細寫入 `unit_cost_twd`（`goods` 為批號的成本，`service` 為進貨明細的成本）與 `lot_id`
//!
//! 進貨退回不影響採購單的 `received_qty` 與狀態。
//!
//! # 作廢
//!
//! 隨時可以作廢已過帳的退回單：依各列的批號扣除明細把數量加回原批號，異動 `void_reversal`（作廢當天）。

use std::collections::{BTreeMap, HashMap};

use hammer_db::purchasing::{self as db, ReceiptForReturn, ReceiptLineForReturn, ReturnLineRow};
use hammer_db::sqlx::PgConnection;
use hammer_shared::Id;
use hammer_shared::decimal::Decimal;
use hammer_shared::inventory::TxnType;
use hammer_shared::schema::{DataType, FieldValue, parse_value};
use serde_json::{Map, Value};
use time::Date;

use super::save::{QTY, line_errors};
use super::{PURCHASE_RETURN, PURCHASE_RETURN_LINE};
use crate::CoreError;
use crate::documents::{DocumentContext, DocumentHandler, HandlerFuture};
use crate::formulas::today_of;
use crate::inventory::{Inventory, Line, LockRequest, Source, allocations_of_line, qty4};
use crate::record_data::FieldError;
use crate::record_lines::LineError;
use crate::stock_adjustments::MSG_NO_LINES;

const GOODS_RECEIPT_ID: &str = "goods_receipt_id";
const GOODS_RECEIPT_LINE_ID: &str = "goods_receipt_line_id";
const RETURN_DATE: &str = "return_date";

/// 進貨單不可更換（§8.8 `purchase_returns`）。
pub const MSG_RECEIPT_FIXED: &str = "建立後不可更換進貨單";
/// 進貨單是草稿。
pub const MSG_RECEIPT_DRAFT: &str = "進貨單尚未過帳";
/// 進貨單已作廢。
pub const MSG_RECEIPT_VOID: &str = "進貨單已作廢";
/// 進貨單已刪除（過帳時）。
pub const MSG_RECEIPT_DELETED: &str = "進貨單已刪除";
/// 明細不屬於表頭的進貨單。
pub const MSG_NOT_RECEIPT_LINE: &str = "不是此進貨單的明細";
/// 同一列進貨明細出現兩次。
pub const MSG_LINE_DUPLICATE: &str = "進貨明細重複";
/// 退回日期早於進貨日期。
pub const MSG_DATE_BEFORE_RECEIPT: &str = "退回日期不可早於進貨日期";

/// 進貨退回存檔時從進貨單帶入並檢查（新增與修改都執行；`entity_key` 不是 [`PURCHASE_RETURN`] 時不做事）。
///
/// - `master`：主表正規化後的結果（修改時已與既有資料合併）；`existing`：修改前的資料。
/// - `lines`：通過欄位驗證的進貨退回明細 `(request 中的位置, data)`；修改時包含所有明細。
///
/// 進貨單不存在或已刪除時不帶入，交給之後的關聯檢查回報；進貨單不是已過帳、修改時更換了進貨單、退回日期
/// 早於進貨日期 → 主表的錯誤；明細不屬於該進貨單、重複、超過可退數量 → 該列的錯誤。
pub async fn derive_return(
    conn: &mut PgConnection,
    entity_key: &str,
    master: Result<Map<String, Value>, Vec<FieldError>>,
    existing: Option<&Map<String, Value>>,
    lines: Vec<(usize, &mut Map<String, Value>)>,
) -> Result<(Result<Map<String, Value>, Vec<FieldError>>, Vec<LineError>), CoreError> {
    if entity_key != PURCHASE_RETURN {
        return Ok((master, Vec::new()));
    }
    let receipt_id = match (&master, existing) {
        (Ok(data), _) => relation_id(data.get(GOODS_RECEIPT_ID)),
        (Err(_), Some(existing)) => relation_id(existing.get(GOODS_RECEIPT_ID)),
        (Err(_), None) => None,
    };
    let Some(receipt_id) = receipt_id else {
        return Ok((master, Vec::new()));
    };
    let Some(receipt) = db::find_receipt_for_return(&mut *conn, receipt_id).await? else {
        return Ok((master, Vec::new()));
    };

    let master = match master {
        Ok(mut data) => {
            let changed =
                existing.is_some_and(|e| relation_id(e.get(GOODS_RECEIPT_ID)) != Some(receipt_id));
            let mut errors: Vec<FieldError> =
                receipt_errors(&receipt, changed).into_iter().collect();
            errors.extend(date_error(date_of(data.get(RETURN_DATE)), &receipt));
            if errors.is_empty() {
                apply_receipt(&mut data, &receipt);
                Ok(data)
            } else {
                Err(errors)
            }
        }
        Err(mut errors) => {
            errors.extend(receipt_errors(&receipt, false));
            Err(errors)
        }
    };

    let ids: Vec<Id> = lines
        .iter()
        .filter_map(|(_, data)| relation_id(data.get(GOODS_RECEIPT_LINE_ID)))
        .collect();
    let found: HashMap<Id, ReceiptLineForReturn> = if ids.is_empty() {
        HashMap::new()
    } else {
        db::list_receipt_lines_for_return(&mut *conn, receipt_id, &ids)
            .await?
            .into_iter()
            .map(|row| (row.id, row))
            .collect()
    };
    let returned = returned_of(conn, &found).await?;
    let mut line_errors = Vec::new();
    let mut entries = Vec::new();
    for (index, data) in lines {
        let id = relation_id(data.get(GOODS_RECEIPT_LINE_ID));
        match id.and_then(|id| found.get(&id)) {
            Some(row) => {
                apply_receipt_line(data, row);
                let qty = match data.get(QTY) {
                    Some(Value::String(s)) => s.parse().ok(),
                    _ => None,
                };
                entries.push(ReturnEntry {
                    index,
                    receipt_line_id: row.id,
                    qty,
                });
            }
            None => line_errors.push(LineError::field(
                PURCHASE_RETURN_LINE,
                index,
                GOODS_RECEIPT_LINE_ID,
                MSG_NOT_RECEIPT_LINE,
            )),
        }
    }
    let limits = limits_of(&found, &returned);
    line_errors.extend(check_quantities(&entries, &limits));
    line_errors.sort_by_key(|e| e.index);
    Ok((master, line_errors))
}

/// 進貨明細在已過帳的退回中的數量合計。
async fn returned_of(
    conn: &mut PgConnection,
    found: &HashMap<Id, ReceiptLineForReturn>,
) -> Result<HashMap<Id, Decimal>, CoreError> {
    if found.is_empty() {
        return Ok(HashMap::new());
    }
    let ids: Vec<Id> = found.keys().copied().collect();
    Ok(db::returned_qty_of_lines(&mut *conn, &ids)
        .await?
        .into_iter()
        .collect())
}

/// 每列進貨明細的可退數量。
fn limits_of(
    found: &HashMap<Id, ReceiptLineForReturn>,
    returned: &HashMap<Id, Decimal>,
) -> HashMap<Id, Decimal> {
    found
        .values()
        .map(|row| {
            let done = returned.get(&row.id).copied().unwrap_or_default();
            (row.id, row.qty - done)
        })
        .collect()
}

/// 一列退回明細（數量檢查用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReturnEntry {
    /// 錯誤中的位置。
    pub index: usize,
    pub receipt_line_id: Id,
    pub qty: Option<Decimal>,
}

/// 同一列進貨明細只能出現一次（第二列起「進貨明細重複」）；數量不可超過可退數量（`limits`：進貨明細 id →
/// 可退數量）。
pub fn check_quantities(entries: &[ReturnEntry], limits: &HashMap<Id, Decimal>) -> Vec<LineError> {
    let mut errors = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for entry in entries {
        if !seen.insert(entry.receipt_line_id) {
            errors.push(LineError::field(
                PURCHASE_RETURN_LINE,
                entry.index,
                GOODS_RECEIPT_LINE_ID,
                MSG_LINE_DUPLICATE,
            ));
            continue;
        }
        let limit = limits
            .get(&entry.receipt_line_id)
            .copied()
            .unwrap_or_default();
        if entry.qty.is_some_and(|q| q > limit) {
            errors.push(LineError::field(
                PURCHASE_RETURN_LINE,
                entry.index,
                QTY,
                format!("超過可退數量 {}", qty4(limit.max(Decimal::ZERO))),
            ));
        }
    }
    errors
}

/// 進貨單不能用於退回的原因：修改時更換了進貨單、進貨單不是已過帳。
fn receipt_errors(receipt: &ReceiptForReturn, changed: bool) -> Option<FieldError> {
    let message = if changed {
        MSG_RECEIPT_FIXED
    } else {
        match receipt.status.as_str() {
            "posted" if receipt.exchange_rate.is_some() => return None,
            "void" => MSG_RECEIPT_VOID,
            _ => MSG_RECEIPT_DRAFT,
        }
    };
    Some(FieldError {
        field_key: GOODS_RECEIPT_ID.into(),
        message: message.into(),
    })
}

/// 退回日期不可早於進貨日期。
fn date_error(return_date: Option<Date>, receipt: &ReceiptForReturn) -> Option<FieldError> {
    return_date
        .filter(|d| *d < receipt.receipt_date)
        .map(|_| FieldError {
            field_key: RETURN_DATE.into(),
            message: MSG_DATE_BEFORE_RECEIPT.into(),
        })
}

/// 表頭帶入進貨單的廠商、幣別、匯率、稅別。
fn apply_receipt(data: &mut Map<String, Value>, receipt: &ReceiptForReturn) {
    data.insert(
        "supplier_id".into(),
        Value::String(receipt.supplier_id.to_string()),
    );
    data.insert(
        "currency_code".into(),
        Value::String(receipt.currency_code.clone()),
    );
    if let Some(rate) = receipt.exchange_rate {
        data.insert("exchange_rate".into(), Value::String(rate.to_string()));
    }
    data.insert("tax_type".into(), Value::String(receipt.tax_type.clone()));
}

/// 明細帶入進貨明細的商品、批號、單價。
fn apply_receipt_line(data: &mut Map<String, Value>, row: &ReceiptLineForReturn) {
    data.insert(
        "product_id".into(),
        Value::String(row.product_id.to_string()),
    );
    match &row.lot_no {
        Some(lot_no) => data.insert("lot_no".into(), Value::String(lot_no.clone())),
        None => data.remove("lot_no"),
    };
    data.insert(
        "unit_price".into(),
        Value::String(row.unit_price.to_string()),
    );
}

fn relation_id(value: Option<&Value>) -> Option<Id> {
    match parse_value(DataType::Relation, value?) {
        Ok(Some(FieldValue::Relation(id))) => Some(id),
        _ => None,
    }
}

fn date_of(value: Option<&Value>) -> Option<Date> {
    match parse_value(DataType::Date, value?) {
        Ok(Some(FieldValue::Date(d))) => Some(d),
        _ => None,
    }
}

/// 進貨退回的過帳、作廢處理函式（[`crate::documents::DocumentHandlers::standard`] 註冊）。
#[derive(Debug, Default, Clone, Copy)]
pub struct PurchaseReturnHandler;

impl DocumentHandler for PurchaseReturnHandler {
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
}

fn line_of(index: usize, row: &ReturnLineRow) -> Result<Line, CoreError> {
    Ok(Line {
        id: row.id,
        index: u32::try_from(index).map_err(|_| CoreError::Internal("明細的列數過多".into()))?,
    })
}

fn validation(fields: Vec<FieldError>, lines: Vec<LineError>) -> Result<(), CoreError> {
    match (fields.is_empty(), lines.is_empty()) {
        (true, true) => Ok(()),
        (false, true) => Err(CoreError::InvalidFields(fields)),
        _ => Err(CoreError::InvalidLines { fields, lines }),
    }
}

async fn post(conn: &mut PgConnection, doc: &DocumentContext<'_>) -> Result<(), CoreError> {
    let receipt_id = relation_id(doc.data.get(GOODS_RECEIPT_ID))
        .ok_or_else(|| CoreError::Internal("進貨退回沒有進貨單".into()))?;
    let Some(receipt) = db::lock_receipt_for_return(&mut *conn, receipt_id).await? else {
        return validation(
            vec![FieldError {
                field_key: GOODS_RECEIPT_ID.into(),
                message: MSG_RECEIPT_DELETED.into(),
            }],
            Vec::new(),
        );
    };
    let rows = db::list_return_lines_for_update(&mut *conn, doc.record_id).await?;

    let mut fields: Vec<FieldError> = receipt_errors(&receipt, false).into_iter().collect();
    let return_date = date_of(doc.data.get(RETURN_DATE))
        .ok_or_else(|| CoreError::Internal(format!("單據 {} 的退回日期不正確", doc.record_no)))?;
    fields.extend(date_error(Some(return_date), &receipt));

    let ids: Vec<Id> = rows.iter().map(|r| r.goods_receipt_line_id).collect();
    let found: HashMap<Id, ReceiptLineForReturn> = if ids.is_empty() {
        HashMap::new()
    } else {
        db::list_receipt_lines_for_return(&mut *conn, receipt_id, &ids)
            .await?
            .into_iter()
            .map(|row| (row.id, row))
            .collect()
    };
    let returned = returned_of(conn, &found).await?;

    let mut errors = Vec::new();
    if rows.is_empty() {
        errors.push(LineError::detail(PURCHASE_RETURN_LINE, MSG_NO_LINES));
    }
    let mut entries = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        if !found.contains_key(&row.goods_receipt_line_id) {
            errors.push(LineError::field(
                PURCHASE_RETURN_LINE,
                index,
                GOODS_RECEIPT_LINE_ID,
                MSG_NOT_RECEIPT_LINE,
            ));
            continue;
        }
        errors.extend(LineError::from_fields(
            PURCHASE_RETURN_LINE,
            index,
            line_errors(Some(row.qty), None),
        ));
        entries.push(ReturnEntry {
            index,
            receipt_line_id: row.goods_receipt_line_id,
            qty: Some(row.qty),
        });
    }
    errors.extend(check_quantities(&entries, &limits_of(&found, &returned)));
    errors.sort_by_key(|e| e.index);
    validation(fields, errors)?;

    let req = LockRequest {
        product_ids: rows.iter().map(|r| r.product_id).collect(),
        lot_ids: found.values().filter_map(|r| r.lot_id).collect(),
        ..LockRequest::default()
    };
    let mut inv = Inventory::lock(conn, &req).await?;
    let src = Source {
        doc_type: PURCHASE_RETURN,
        line_type: PURCHASE_RETURN_LINE,
        doc_id: doc.record_id,
        txn_date: return_date,
        actor: doc.actor,
    };
    for (index, row) in rows.iter().enumerate() {
        let receipt_line = &found[&row.goods_receipt_line_id];
        let unit_cost = match receipt_line.lot_id {
            Some(lot_id) => {
                let cost = inv
                    .lot(lot_id)
                    .map(|l| l.unit_cost_twd)
                    .ok_or_else(|| CoreError::Internal(format!("批號 {lot_id} 沒有先鎖定")))?;
                // 不足時記下並繼續，由 finish 回傳所有不足
                inv.issue_from_lot(
                    conn,
                    &src,
                    line_of(index, row)?,
                    receipt_line.product_id,
                    lot_id,
                    row.qty,
                    TxnType::PurchaseReturn,
                )
                .await?;
                cost
            }
            None => receipt_line.unit_cost_twd.ok_or_else(|| {
                CoreError::Internal(format!("進貨明細 {} 沒有台幣成本", receipt_line.id))
            })?,
        };
        if !db::update_return_line_posting(
            &mut *conn,
            row.id,
            unit_cost,
            receipt_line.lot_id,
            doc.actor,
        )
        .await?
        {
            return Err(CoreError::Internal(format!("明細 {} 已不存在", row.id)));
        }
    }
    inv.finish()
}

async fn void(conn: &mut PgConnection, doc: &DocumentContext<'_>) -> Result<(), CoreError> {
    let rows = db::list_return_lines_for_update(&mut *conn, doc.record_id).await?;
    // 鎖定前先讀取批號扣除明細，把原批號一起鎖定
    let mut allocations = BTreeMap::new();
    for row in &rows {
        allocations.insert(
            row.id,
            allocations_of_line(conn, PURCHASE_RETURN_LINE, row.id).await?,
        );
    }
    let req = LockRequest {
        product_ids: rows.iter().map(|r| r.product_id).collect(),
        lot_ids: allocations.values().flatten().map(|a| a.lot_id).collect(),
        ..LockRequest::default()
    };
    let mut inv = Inventory::lock(conn, &req).await?;
    let src = Source {
        doc_type: PURCHASE_RETURN,
        line_type: PURCHASE_RETURN_LINE,
        doc_id: doc.record_id,
        txn_date: today_of(doc.now),
        actor: doc.actor,
    };
    for (index, row) in rows.iter().enumerate() {
        let allocs = allocations.get(&row.id).map(Vec::as_slice).unwrap_or(&[]);
        inv.reverse_allocations(conn, &src, line_of(index, row)?, allocs)
            .await?;
    }
    inv.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    fn receipt(status: &str, rate: Option<&str>) -> ReceiptForReturn {
        ReceiptForReturn {
            id: Uuid::nil(),
            status: status.into(),
            supplier_id: Uuid::from_u128(7),
            currency_code: "USD".into(),
            exchange_rate: rate.map(|r| r.parse().unwrap()),
            tax_type: "zero_rated".into(),
            receipt_date: time::macros::date!(2026 - 11 - 17),
        }
    }

    fn d(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    #[test]
    fn only_posted_receipts_can_be_returned() {
        assert_eq!(
            receipt_errors(&receipt("posted", Some("32.150000")), false),
            None
        );
        let message = |r: &ReceiptForReturn, changed| receipt_errors(r, changed).unwrap().message;
        assert_eq!(message(&receipt("draft", None), false), MSG_RECEIPT_DRAFT);
        assert_eq!(
            message(&receipt("void", Some("1")), false),
            MSG_RECEIPT_VOID
        );
        assert_eq!(
            message(&receipt("posted", Some("1")), true),
            MSG_RECEIPT_FIXED
        );
        assert_eq!(
            receipt_errors(&receipt("posted", None), false)
                .unwrap()
                .field_key,
            GOODS_RECEIPT_ID
        );
    }

    #[test]
    fn return_date_not_before_receipt_date() {
        let r = receipt("posted", Some("1"));
        assert!(date_error(Some(time::macros::date!(2026 - 11 - 17)), &r).is_none());
        assert!(date_error(None, &r).is_none());
        let err = date_error(Some(time::macros::date!(2026 - 11 - 16)), &r).unwrap();
        assert_eq!(err.field_key, RETURN_DATE);
        assert_eq!(err.message, MSG_DATE_BEFORE_RECEIPT);
    }

    #[test]
    fn header_copies_receipt_values() {
        let mut data = json!({ "goods_receipt_id": "x", "supplier_id": "ignored" })
            .as_object()
            .unwrap()
            .clone();
        apply_receipt(&mut data, &receipt("posted", Some("32.150000")));
        assert_eq!(
            Value::Object(data),
            json!({
                "goods_receipt_id": "x",
                "supplier_id": Uuid::from_u128(7).to_string(),
                "currency_code": "USD",
                "exchange_rate": "32.150000",
                "tax_type": "zero_rated",
            })
        );
    }

    #[test]
    fn line_copies_receipt_line_values() {
        let row = ReceiptLineForReturn {
            id: Uuid::nil(),
            product_id: Uuid::from_u128(9),
            lot_no: Some("L-01".into()),
            unit_price: "12.3450".parse().unwrap(),
            lot_id: None,
            qty: d("5"),
            unit_cost_twd: None,
        };
        let mut data = json!({ "qty": "2.0000" }).as_object().unwrap().clone();
        apply_receipt_line(&mut data, &row);
        assert_eq!(
            Value::Object(data.clone()),
            json!({
                "qty": "2.0000",
                "product_id": Uuid::from_u128(9).to_string(),
                "lot_no": "L-01",
                "unit_price": "12.3450",
            })
        );
        // 服務類沒有批號
        let service = ReceiptLineForReturn {
            lot_no: None,
            ..row
        };
        apply_receipt_line(&mut data, &service);
        assert!(!data.contains_key("lot_no"));
    }

    #[test]
    fn quantities_are_limited_and_lines_unique() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let limits = HashMap::from([(a, d("4")), (b, d("0"))]);
        let entry = |index, id, qty: &str| ReturnEntry {
            index,
            receipt_line_id: id,
            qty: Some(d(qty)),
        };
        assert!(check_quantities(&[entry(0, a, "4")], &limits).is_empty());
        let errors = check_quantities(
            &[entry(0, a, "4.5"), entry(1, a, "1"), entry(2, b, "1")],
            &limits,
        );
        let got: Vec<(Option<usize>, Option<&str>, &str)> = errors
            .iter()
            .map(|e| (e.index, e.field_key.as_deref(), e.message.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                (Some(0), Some(QTY), "超過可退數量 4.0000"),
                (Some(1), Some(GOODS_RECEIPT_LINE_ID), MSG_LINE_DUPLICATE),
                (Some(2), Some(QTY), "超過可退數量 0.0000"),
            ]
        );
    }

    #[test]
    fn limits_subtract_posted_returns() {
        let row = ReceiptLineForReturn {
            id: Uuid::from_u128(1),
            product_id: Uuid::nil(),
            lot_no: None,
            unit_price: d("1"),
            lot_id: None,
            qty: d("10"),
            unit_cost_twd: None,
        };
        let found = HashMap::from([(row.id, row)]);
        let returned = HashMap::from([(Uuid::from_u128(1), d("6"))]);
        assert_eq!(limits_of(&found, &returned)[&Uuid::from_u128(1)], d("4"));
        assert_eq!(
            limits_of(&found, &HashMap::new())[&Uuid::from_u128(1)],
            d("10")
        );
    }

    #[test]
    fn relation_id_parses_uuid_strings() {
        let id = Uuid::from_u128(3);
        assert_eq!(relation_id(Some(&json!(id.to_string()))), Some(id));
        assert_eq!(relation_id(Some(&json!("not-a-uuid"))), None);
        assert_eq!(relation_id(None), None);
    }
}
