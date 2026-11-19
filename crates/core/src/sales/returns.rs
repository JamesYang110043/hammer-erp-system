//! 銷貨退回（系統單據 `sales_return`，docs/architecture.md §8.9「銷貨退回」、§9.3）：存檔時從出貨單帶入
//! 與檢查，以及過帳、作廢（[`SalesReturnHandler`]，掛在 [`crate::documents`] 的框架上）。
//!
//! # 存檔（[`derive_return`]）
//!
//! - 表頭的 `customer_id`、`tax_type`、`tax_included`，明細的 `product_id`、`unit_price`、`discount_rate`
//!   從出貨單、出貨明細帶入（唯讀的系統欄位，request 帶了一律忽略，§7.1）。金額依 [`super::amounts`] 計算
//! - 出貨單必須是已過帳（「出貨單尚未過帳」「出貨單已作廢」），建立後不可更換；退回日期不可早於出貨日期
//! - 明細的 `shipment_line_id` 必須是該出貨單的明細，同一列出貨明細只能出現一次（「出貨明細重複」）
//! - 退回數量上限（[`check_quantities`]）：`可退數量 = 出貨明細的 qty − 其他已過帳退回的 qty 合計`
//!   （草稿不保留數量）
//!
//! # 過帳
//!
//! 鎖定順序：退回單（框架已 `FOR UPDATE`）→ 出貨單（`FOR UPDATE`，同一張出貨單的退回依序過帳）→ 明細
//! （`FOR UPDATE`）→ 商品（`FOR SHARE`）→ 原出貨的批號（`FOR UPDATE`）。重新執行存檔時的檢查、至少一列
//! 明細、退回數量上限。之後依 `sort_order`：
//!
//! - `goods`：加回**原出貨扣除的批號**（[`plan_return`]：依扣除的反向順序 `received_date DESC, id DESC`
//!   填滿，每個批號最多加回「扣除的數量 − 其他已過帳退回加回此批號的數量」），成本用批號扣除明細的成本；
//!   異動 `sales_return`、`txn_date = return_date`。`cost_amount_twd = round(Σ qty × unit_cost_twd, 2)`
//! - `service`：不處理庫存；`cost_amount_twd` 為 `NULL`
//!
//! 表頭寫入 `cost_total_twd`。加回不會不足，所以過帳不會有 `STOCK_INSUFFICIENT`。
//!
//! # 作廢
//!
//! 依此退回單的每一筆 `sales_return` 異動：該批號 `qty_remaining ≥ qty` 時扣回（`void_reversal`、
//! 作廢當天）；不足（加回的貨已再出貨或盤虧）時記下，最後 `422 STOCK_INSUFFICIENT`（一個批號一項）並
//! rollback。`cost_amount_twd`、`cost_total_twd` 保留。

use std::collections::{BTreeMap, HashMap, HashSet};

use hammer_db::sales::{self as db, ReturnLineRow, ShipmentForReturn, ShipmentLineForReturn};
use hammer_db::sqlx::PgConnection;
use hammer_shared::Id;
use hammer_shared::decimal::Decimal;
use hammer_shared::inventory::TxnType;
use hammer_shared::schema::{DataType, FieldValue, parse_value};
use serde_json::{Map, Value};
use time::Date;

use super::{SALES_RETURN, SALES_RETURN_LINE, SHIPMENT_LINE};
use crate::CoreError;
use crate::documents::{DocumentContext, DocumentHandler, HandlerFuture};
use crate::formulas::today_of;
use crate::inventory::{
    Allocation, Inventory, Line, LockRequest, Source, allocations_of_line, qty4,
};
use crate::purchasing::amounts::{TWD_SCALE, with_scale};
use crate::purchasing::save::line_errors;
use crate::record_data::FieldError;
use crate::record_lines::LineError;
use crate::sales::shipments::cost_amount;
use crate::stock_adjustments::MSG_NO_LINES;

const SHIPMENT_ID: &str = "shipment_id";
const SHIPMENT_LINE_ID: &str = "shipment_line_id";
const RETURN_DATE: &str = "return_date";
const QTY: &str = "qty";

/// 出貨單不可更換（§8.9 `sales_returns`）。
pub const MSG_SHIPMENT_FIXED: &str = "建立後不可更換出貨單";
/// 出貨單是草稿。
pub const MSG_SHIPMENT_DRAFT: &str = "出貨單尚未過帳";
/// 出貨單已作廢。
pub const MSG_SHIPMENT_VOID: &str = "出貨單已作廢";
/// 出貨單已刪除（過帳時）。
pub const MSG_SHIPMENT_DELETED: &str = "出貨單已刪除";
/// 明細不屬於表頭的出貨單。
pub const MSG_NOT_SHIPMENT_LINE: &str = "不是此出貨單的明細";
/// 同一列出貨明細出現兩次。
pub const MSG_LINE_DUPLICATE: &str = "出貨明細重複";
/// 退回日期早於出貨日期。
pub const MSG_DATE_BEFORE_SHIPMENT: &str = "退回日期不可早於出貨日期";

/// 銷貨退回存檔時從出貨單帶入並檢查（新增與修改都執行；`entity_key` 不是 [`SALES_RETURN`] 時不做事）。
///
/// - `master`：主表正規化後的結果（修改時已與既有資料合併）；`existing`：修改前的資料。
/// - `lines`：通過欄位驗證的銷貨退回明細 `(request 中的位置, data)`；修改時包含所有明細。
///
/// 出貨單不存在或已刪除時不帶入，交給之後的關聯檢查回報；出貨單不是已過帳、修改時更換了出貨單、退回日期
/// 早於出貨日期 → 主表的錯誤；明細不屬於該出貨單、重複、超過可退數量 → 該列的錯誤。
pub async fn derive_return(
    conn: &mut PgConnection,
    entity_key: &str,
    master: Result<Map<String, Value>, Vec<FieldError>>,
    existing: Option<&Map<String, Value>>,
    lines: Vec<(usize, &mut Map<String, Value>)>,
) -> Result<(Result<Map<String, Value>, Vec<FieldError>>, Vec<LineError>), CoreError> {
    if entity_key != SALES_RETURN {
        return Ok((master, Vec::new()));
    }
    let shipment_id = match (&master, existing) {
        (Ok(data), _) => relation_id(data.get(SHIPMENT_ID)),
        (Err(_), Some(existing)) => relation_id(existing.get(SHIPMENT_ID)),
        (Err(_), None) => None,
    };
    let Some(shipment_id) = shipment_id else {
        return Ok((master, Vec::new()));
    };
    let Some(shipment) = db::find_shipment_for_return(&mut *conn, shipment_id).await? else {
        return Ok((master, Vec::new()));
    };

    let master = match master {
        Ok(mut data) => {
            let changed =
                existing.is_some_and(|e| relation_id(e.get(SHIPMENT_ID)) != Some(shipment_id));
            let mut errors: Vec<FieldError> =
                shipment_error(&shipment, changed).into_iter().collect();
            errors.extend(date_error(date_of(data.get(RETURN_DATE)), &shipment));
            if errors.is_empty() {
                apply_shipment(&mut data, &shipment);
                Ok(data)
            } else {
                Err(errors)
            }
        }
        Err(mut errors) => {
            errors.extend(shipment_error(&shipment, false));
            Err(errors)
        }
    };

    let ids: Vec<Id> = lines
        .iter()
        .filter_map(|(_, data)| relation_id(data.get(SHIPMENT_LINE_ID)))
        .collect();
    let found = shipment_lines(conn, shipment_id, &ids).await?;
    let returned = returned_of(conn, &found).await?;
    let mut line_errors = Vec::new();
    let mut entries = Vec::new();
    for (index, data) in lines {
        match relation_id(data.get(SHIPMENT_LINE_ID)).and_then(|id| found.get(&id)) {
            Some(row) => {
                apply_shipment_line(data, row);
                let qty = match data.get(QTY) {
                    Some(Value::String(s)) => s.parse().ok(),
                    _ => None,
                };
                entries.push(ReturnEntry {
                    index,
                    shipment_line_id: row.id,
                    qty,
                });
            }
            None => line_errors.push(LineError::field(
                SALES_RETURN_LINE,
                index,
                SHIPMENT_LINE_ID,
                MSG_NOT_SHIPMENT_LINE,
            )),
        }
    }
    line_errors.extend(check_quantities(&entries, &limits_of(&found, &returned)));
    line_errors.sort_by_key(|e| e.index);
    Ok((master, line_errors))
}

/// 出貨單 `shipment_id` 中 id 在 `ids` 內的明細。
async fn shipment_lines(
    conn: &mut PgConnection,
    shipment_id: Id,
    ids: &[Id],
) -> Result<HashMap<Id, ShipmentLineForReturn>, CoreError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    Ok(
        db::list_shipment_lines_for_return(&mut *conn, shipment_id, ids)
            .await?
            .into_iter()
            .map(|row| (row.id, row))
            .collect(),
    )
}

/// 出貨明細在已過帳的退回中的數量合計。
async fn returned_of(
    conn: &mut PgConnection,
    found: &HashMap<Id, ShipmentLineForReturn>,
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

/// 每列出貨明細的可退數量。
fn limits_of(
    found: &HashMap<Id, ShipmentLineForReturn>,
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
    pub shipment_line_id: Id,
    pub qty: Option<Decimal>,
}

/// 同一列出貨明細只能出現一次（第二列起「出貨明細重複」）；數量不可超過可退數量（`limits`：出貨明細 id →
/// 可退數量）。
pub fn check_quantities(entries: &[ReturnEntry], limits: &HashMap<Id, Decimal>) -> Vec<LineError> {
    let mut errors = Vec::new();
    let mut seen = HashSet::new();
    for entry in entries {
        if !seen.insert(entry.shipment_line_id) {
            errors.push(LineError::field(
                SALES_RETURN_LINE,
                entry.index,
                SHIPMENT_LINE_ID,
                MSG_LINE_DUPLICATE,
            ));
            continue;
        }
        let limit = limits
            .get(&entry.shipment_line_id)
            .copied()
            .unwrap_or_default();
        if entry.qty.is_some_and(|q| q > limit) {
            errors.push(LineError::field(
                SALES_RETURN_LINE,
                entry.index,
                QTY,
                format!("超過可退數量 {}", qty4(limit.max(Decimal::ZERO))),
            ));
        }
    }
    errors
}

/// 一個原出貨批號的加回額度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReturnSource {
    pub lot_id: Id,
    /// 批號的入庫日期（扣除的反向順序用）。
    pub received_date: Date,
    /// 出貨時從此批號扣除的數量。
    pub issued: Decimal,
    /// 其他已過帳退回已加回此批號的數量。
    pub returned: Decimal,
    /// 扣除時的成本。
    pub unit_cost_twd: Decimal,
}

/// 退回 `qty` 加回的批號（§8.9「批號的分配」）：依扣除的反向順序（`received_date DESC, id DESC`，最後扣的
/// 先加回）填滿，每個批號最多加回 `issued − returned`。回傳加回的批號與數量；超出可加回的總量時為
/// `None`（退回數量上限保證不會發生）。
pub fn plan_return(sources: &[ReturnSource], qty: Decimal) -> Option<Vec<Allocation>> {
    let mut ordered: Vec<&ReturnSource> = sources.iter().collect();
    ordered.sort_by_key(|s| std::cmp::Reverse((s.received_date, s.lot_id)));
    let mut remaining = qty;
    let mut out = Vec::new();
    for s in ordered {
        if remaining <= Decimal::ZERO {
            break;
        }
        let capacity = s.issued - s.returned;
        if capacity <= Decimal::ZERO {
            continue;
        }
        let take = remaining.min(capacity);
        out.push(Allocation {
            lot_id: s.lot_id,
            qty: take,
            unit_cost_twd: s.unit_cost_twd,
        });
        remaining -= take;
    }
    (remaining <= Decimal::ZERO).then_some(out)
}

/// 出貨單不能用於退回的原因：修改時更換了出貨單、出貨單不是已過帳。
fn shipment_error(shipment: &ShipmentForReturn, changed: bool) -> Option<FieldError> {
    let message = if changed {
        MSG_SHIPMENT_FIXED
    } else {
        match shipment.status.as_str() {
            "posted" => return None,
            "void" => MSG_SHIPMENT_VOID,
            _ => MSG_SHIPMENT_DRAFT,
        }
    };
    Some(FieldError {
        field_key: SHIPMENT_ID.into(),
        message: message.into(),
    })
}

/// 退回日期不可早於出貨日期。
fn date_error(return_date: Option<Date>, shipment: &ShipmentForReturn) -> Option<FieldError> {
    return_date
        .filter(|d| *d < shipment.ship_date)
        .map(|_| FieldError {
            field_key: RETURN_DATE.into(),
            message: MSG_DATE_BEFORE_SHIPMENT.into(),
        })
}

/// 表頭帶入出貨單的客戶、稅別、單價含稅。
fn apply_shipment(data: &mut Map<String, Value>, shipment: &ShipmentForReturn) {
    data.insert(
        "customer_id".into(),
        Value::String(shipment.customer_id.to_string()),
    );
    data.insert("tax_type".into(), Value::String(shipment.tax_type.clone()));
    data.insert("tax_included".into(), Value::Bool(shipment.tax_included));
}

/// 明細帶入出貨明細的商品、單價、折扣率。
fn apply_shipment_line(data: &mut Map<String, Value>, row: &ShipmentLineForReturn) {
    data.insert(
        "product_id".into(),
        Value::String(row.product_id.to_string()),
    );
    data.insert(
        "unit_price".into(),
        Value::String(row.unit_price.to_string()),
    );
    data.insert(
        "discount_rate".into(),
        Value::String(row.discount_rate.to_string()),
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

/// 銷貨退回的過帳、作廢處理函式（[`crate::documents::DocumentHandlers::standard`] 註冊）。
#[derive(Debug, Default, Clone, Copy)]
pub struct SalesReturnHandler;

impl DocumentHandler for SalesReturnHandler {
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
    let shipment_id = relation_id(doc.data.get(SHIPMENT_ID))
        .ok_or_else(|| CoreError::Internal("銷貨退回沒有出貨單".into()))?;
    let Some(shipment) = db::lock_shipment_for_return(&mut *conn, shipment_id).await? else {
        return validation(
            vec![FieldError {
                field_key: SHIPMENT_ID.into(),
                message: MSG_SHIPMENT_DELETED.into(),
            }],
            Vec::new(),
        );
    };
    let rows = db::list_return_lines_for_update(&mut *conn, doc.record_id).await?;

    let mut fields: Vec<FieldError> = shipment_error(&shipment, false).into_iter().collect();
    let return_date = date_of(doc.data.get(RETURN_DATE))
        .ok_or_else(|| CoreError::Internal(format!("單據 {} 的退回日期不正確", doc.record_no)))?;
    fields.extend(date_error(Some(return_date), &shipment));

    let ids: Vec<Id> = rows.iter().map(|r| r.shipment_line_id).collect();
    let found = shipment_lines(conn, shipment_id, &ids).await?;
    let returned = returned_of(conn, &found).await?;

    let mut errors = Vec::new();
    if rows.is_empty() {
        errors.push(LineError::detail(SALES_RETURN_LINE, MSG_NO_LINES));
    }
    let mut entries = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        if !found.contains_key(&row.shipment_line_id) {
            errors.push(LineError::field(
                SALES_RETURN_LINE,
                index,
                SHIPMENT_LINE_ID,
                MSG_NOT_SHIPMENT_LINE,
            ));
            continue;
        }
        errors.extend(LineError::from_fields(
            SALES_RETURN_LINE,
            index,
            line_errors(Some(row.qty), None),
        ));
        entries.push(ReturnEntry {
            index,
            shipment_line_id: row.shipment_line_id,
            qty: Some(row.qty),
        });
    }
    errors.extend(check_quantities(&entries, &limits_of(&found, &returned)));
    errors.sort_by_key(|e| e.index);
    validation(fields, errors)?;

    // 鎖定前先讀取原出貨的批號扣除明細與已加回的數量，把原批號一起鎖定
    let mut issued = Vec::with_capacity(rows.len());
    for row in &rows {
        let allocations = allocations_of_line(conn, SHIPMENT_LINE, row.shipment_line_id).await?;
        let returned: HashMap<Id, Decimal> =
            db::returned_qty_by_lot(&mut *conn, row.shipment_line_id)
                .await?
                .into_iter()
                .collect();
        issued.push((allocations, returned));
    }
    let req = LockRequest {
        product_ids: rows.iter().map(|r| r.product_id).collect(),
        lot_ids: issued
            .iter()
            .flat_map(|(a, _)| a.iter().map(|a| a.lot_id))
            .collect(),
        ..LockRequest::default()
    };
    let mut inv = Inventory::lock(conn, &req).await?;
    let src = Source {
        doc_type: SALES_RETURN,
        line_type: SALES_RETURN_LINE,
        doc_id: doc.record_id,
        txn_date: return_date,
        actor: doc.actor,
    };
    let mut cost_total = Decimal::ZERO;
    for (index, (row, (allocations, returned))) in rows.iter().zip(&issued).enumerate() {
        let is_goods = inv
            .product(row.product_id)
            .ok_or_else(|| CoreError::Internal(format!("商品 {} 沒有先鎖定", row.product_id)))?
            .is_goods;
        let cost = if is_goods {
            // 同一個批號可能有多筆扣除明細：合計
            let mut by_lot: BTreeMap<Id, ReturnSource> = BTreeMap::new();
            for a in allocations {
                let lot = inv
                    .lot(a.lot_id)
                    .ok_or_else(|| CoreError::Internal(format!("批號 {} 沒有先鎖定", a.lot_id)))?;
                let entry = by_lot.entry(a.lot_id).or_insert(ReturnSource {
                    lot_id: a.lot_id,
                    received_date: lot.received_date,
                    issued: Decimal::ZERO,
                    returned: returned.get(&a.lot_id).copied().unwrap_or_default(),
                    unit_cost_twd: a.unit_cost_twd,
                });
                entry.issued += a.qty;
            }
            let sources: Vec<ReturnSource> = by_lot.into_values().collect();
            let plan = plan_return(&sources, row.qty).ok_or_else(|| {
                CoreError::Internal(format!(
                    "出貨明細 {} 可加回的數量不足",
                    row.shipment_line_id
                ))
            })?;
            let at = line_of(index, row)?;
            for a in &plan {
                inv.add_to_lot(
                    conn,
                    &src,
                    at,
                    row.product_id,
                    a.lot_id,
                    a.qty,
                    Some(a.unit_cost_twd),
                    TxnType::SalesReturn,
                )
                .await?;
            }
            Some(cost_amount(&plan))
        } else {
            None
        };
        cost_total += cost.unwrap_or_default();
        if !db::update_return_line_cost(&mut *conn, row.id, cost, doc.actor).await? {
            return Err(CoreError::Internal(format!("明細 {} 已不存在", row.id)));
        }
    }
    inv.finish()?;
    if !db::set_return_cost_total(&mut *conn, doc.record_id, with_scale(cost_total, TWD_SCALE))
        .await?
    {
        return Err(CoreError::Internal(format!(
            "銷貨退回 {} 已不存在",
            doc.record_id
        )));
    }
    Ok(())
}

async fn void(conn: &mut PgConnection, doc: &DocumentContext<'_>) -> Result<(), CoreError> {
    let rows = db::list_return_lines_for_update(&mut *conn, doc.record_id).await?;
    let txns = db::return_transactions(&mut *conn, doc.record_id).await?;
    let req = LockRequest {
        product_ids: rows.iter().map(|r| r.product_id).collect(),
        lot_ids: txns.iter().map(|t| t.lot_id).collect(),
        ..LockRequest::default()
    };
    let mut inv = Inventory::lock(conn, &req).await?;
    let src = Source {
        doc_type: SALES_RETURN,
        line_type: SALES_RETURN_LINE,
        doc_id: doc.record_id,
        txn_date: today_of(doc.now),
        actor: doc.actor,
    };
    let index_of: HashMap<Id, (usize, &ReturnLineRow)> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| (r.id, (i, r)))
        .collect();
    for t in &txns {
        let (index, row) = index_of
            .get(&t.ref_line_id)
            .copied()
            .ok_or_else(|| CoreError::Internal(format!("異動的明細 {} 不存在", t.ref_line_id)))?;
        // 不足時記下並繼續，由 finish 回傳所有不足
        inv.reduce_lot(
            conn,
            &src,
            line_of(index, row)?,
            t.product_id,
            t.lot_id,
            t.qty_change,
            TxnType::VoidReversal,
        )
        .await?;
    }
    inv.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    fn shipment(status: &str) -> ShipmentForReturn {
        ShipmentForReturn {
            id: Uuid::nil(),
            status: status.into(),
            customer_id: Uuid::from_u128(7),
            tax_type: "taxable".into(),
            tax_included: true,
            ship_date: time::macros::date!(2026 - 11 - 10),
        }
    }

    #[test]
    fn only_posted_shipments_can_be_returned() {
        assert_eq!(shipment_error(&shipment("posted"), false), None);
        let message = |s: &ShipmentForReturn, changed| shipment_error(s, changed).unwrap().message;
        assert_eq!(message(&shipment("draft"), false), MSG_SHIPMENT_DRAFT);
        assert_eq!(message(&shipment("void"), false), MSG_SHIPMENT_VOID);
        assert_eq!(message(&shipment("posted"), true), MSG_SHIPMENT_FIXED);
        assert_eq!(
            shipment_error(&shipment("draft"), false).unwrap().field_key,
            SHIPMENT_ID
        );
    }

    #[test]
    fn header_copies_shipment_values() {
        let mut data =
            json!({ "shipment_id": "x", "customer_id": "ignored", "tax_included": false })
                .as_object()
                .unwrap()
                .clone();
        apply_shipment(&mut data, &shipment("posted"));
        assert_eq!(
            Value::Object(data),
            json!({
                "shipment_id": "x",
                "customer_id": Uuid::from_u128(7).to_string(),
                "tax_type": "taxable",
                "tax_included": true,
            })
        );
    }

    #[test]
    fn line_copies_shipment_line_values() {
        let row = ShipmentLineForReturn {
            id: Uuid::nil(),
            product_id: Uuid::from_u128(9),
            unit_price: "105.0000".parse().unwrap(),
            discount_rate: "0.1000".parse().unwrap(),
            qty: "10.0000".parse().unwrap(),
        };
        let mut data = json!({ "qty": "2.0000", "unit_price": "1" })
            .as_object()
            .unwrap()
            .clone();
        apply_shipment_line(&mut data, &row);
        assert_eq!(
            Value::Object(data),
            json!({
                "qty": "2.0000",
                "product_id": Uuid::from_u128(9).to_string(),
                "unit_price": "105.0000",
                "discount_rate": "0.1000",
            })
        );
    }

    #[test]
    fn relation_id_parses_uuid_strings() {
        let id = Uuid::from_u128(3);
        assert_eq!(relation_id(Some(&json!(id.to_string()))), Some(id));
        assert_eq!(relation_id(Some(&json!("not-a-uuid"))), None);
        assert_eq!(relation_id(None), None);
    }

    fn d(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    #[test]
    fn return_date_not_before_ship_date() {
        let sh = shipment("posted");
        assert!(date_error(Some(time::macros::date!(2026 - 11 - 10)), &sh).is_none());
        assert!(date_error(None, &sh).is_none());
        let e = date_error(Some(time::macros::date!(2026 - 11 - 09)), &sh).unwrap();
        assert_eq!(e.field_key, RETURN_DATE);
        assert_eq!(e.message, MSG_DATE_BEFORE_SHIPMENT);
    }

    #[test]
    fn quantities_limited_and_lines_unique() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let limits = HashMap::from([(a, d("4")), (b, d("0"))]);
        let entries = [
            ReturnEntry {
                index: 0,
                shipment_line_id: a,
                qty: Some(d("4")),
            },
            ReturnEntry {
                index: 1,
                shipment_line_id: a,
                qty: Some(d("1")),
            },
            ReturnEntry {
                index: 2,
                shipment_line_id: b,
                qty: Some(d("0.5")),
            },
        ];
        let errors = check_quantities(&entries, &limits);
        let got: Vec<(Option<usize>, String)> =
            errors.into_iter().map(|e| (e.index, e.message)).collect();
        assert_eq!(
            got,
            [
                (Some(1), MSG_LINE_DUPLICATE.to_owned()),
                (Some(2), "超過可退數量 0.0000".to_owned()),
            ]
        );
    }

    fn source(n: u128, date: Date, issued: &str, returned: &str, cost: &str) -> ReturnSource {
        ReturnSource {
            lot_id: Uuid::from_u128(n),
            received_date: date,
            issued: d(issued),
            returned: d(returned),
            unit_cost_twd: d(cost),
        }
    }

    fn plan(sources: &[ReturnSource], qty: &str) -> Vec<(u128, String)> {
        plan_return(sources, d(qty))
            .unwrap()
            .into_iter()
            .map(|a| (a.lot_id.as_u128(), a.qty.to_string()))
            .collect()
    }

    #[test]
    fn returns_refill_lots_in_reverse_issue_order() {
        use time::macros::date;
        // §8.9 範例：出貨 12（L-01 扣 5、L-02 扣 7）
        let l1 = |returned| source(1, date!(2026 - 10 - 02), "5", returned, "80");
        let l2 = |returned| source(2, date!(2026 - 11 - 01), "7", returned, "82.5");
        // 第一次退 8 → L-02 加回 7、L-01 加回 1
        assert_eq!(
            plan(&[l1("0"), l2("0")], "8"),
            [(2, "7".to_owned()), (1, "1".to_owned())]
        );
        // 第二次退 4 → L-01 加回 4
        assert_eq!(plan(&[l1("1"), l2("7")], "4"), [(1, "4".to_owned())]);
        // 超出可加回的總量
        assert!(plan_return(&[l1("1"), l2("7")], d("5")).is_none());
        // 同一天時依 id 由大到小
        let same = [
            source(1, date!(2026 - 10 - 02), "1", "0", "1"),
            source(3, date!(2026 - 10 - 02), "1", "0", "1"),
        ];
        assert_eq!(plan(&same, "1"), [(3, "1".to_owned())]);
        // 成本用扣除時的成本
        let out = plan_return(&[l2("0")], d("2")).unwrap();
        assert_eq!(cost_amount(&out).to_string(), "165.00");
    }
}
