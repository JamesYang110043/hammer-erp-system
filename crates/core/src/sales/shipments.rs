//! 出貨單（系統單據 `shipment`，docs/architecture.md §8.9「存檔時的檢查」「訂單轉出貨單」「出貨的批號」
//! 「出貨過帳與作廢」、§9.2、§9.3）：存檔時的檢查，以及過帳、作廢（[`ShipmentHandler`]，掛在
//! [`crate::documents`] 的框架上）。稅別預設值與金額與訂單相同（[`super::save`]）。
//!
//! # 存檔時的檢查（[`check_shipment`]，存檔與過帳共用）
//!
//! - 商品未刪除（「商品已刪除」）
//! - `goods`：`lot_no` 有值時必須是該商品未刪除的批號（不分大小寫，「找不到此批號」）；`service`：不可填
//!   `lot_no`（「服務類商品沒有批號」）。同一個批號可以出現在多列（過帳時合計檢查）
//! - 有 `sales_order_id`：訂單是 `confirmed` / `partially_shipped`；客戶與訂單相同；`sales_order_line_id`
//!   是該訂單的明細、商品相同；同一列訂單明細的 `qty` 合計不超過 `qty − shipped_qty`（只計算已過帳的
//!   出貨，草稿不保留數量）
//! - 沒有 `sales_order_id`：明細不可有 `sales_order_line_id`
//!
//! 不檢查庫存（過帳時檢查）。
//!
//! # 過帳
//!
//! 鎖定順序：出貨單（框架已 `FOR UPDATE`）→ 訂單（`FOR UPDATE`）→ 明細（`FOR UPDATE`）→ 客戶
//! （`FOR SHARE`）→ 商品（`FOR SHARE`）、批號（指定的批號 + 先進先出列的商品所有 `qty_remaining > 0` 的
//! 批號，`FOR UPDATE`）→ 訂單明細（`FOR UPDATE`）。以鎖定後的資料重新執行存檔時的檢查，另外檢查至少一列
//! 明細、客戶未刪除。之後扣庫存（`goods` 才處理）：
//!
//! 1. 有 `lot_no` 的列，依 `sort_order`：從該批號扣除（同一批號出現在多列時依序扣）；明細寫入 `lot_id`
//! 2. 沒有 `lot_no` 的列，依 `sort_order`：先進先出（`received_date, id`），可以跨多個批號
//!
//! 有任何不足 → `422 STOCK_INSUFFICIENT`，列出所有不足（指定批號一個批號一項、先進先出一個商品一項）。
//! 足夠時每一列寫入 `cost_amount_twd = round(Σ 扣除的 qty × unit_cost_twd, 2)`（`service` 為 `NULL`），
//! 表頭寫入 `cost_total_twd`；有 `sales_order_line_id` 的列：訂單明細的 `shipped_qty += qty`，重新計算
//! 訂單的狀態（[`order_status`]），訂單寫入操作紀錄（`update`）。
//!
//! # 作廢
//!
//! 有已過帳的銷貨退回 → `422 VALIDATION`（[`CoreError::DocumentsInUse`]）。依各列的批號扣除明細把數量加回
//! 原批號（`void_reversal`、扣除時的成本、作廢當天）；訂單明細的 `shipped_qty −= qty`，重新計算訂單的
//! 狀態（手動結案的維持 `closed`）。`cost_amount_twd`、`cost_total_twd` 保留。

use std::collections::{BTreeMap, BTreeSet, HashMap};

use hammer_db::inventory as inventory_db;
use hammer_db::sales::{self as db, OrderForShipment, OrderLineRow, ShipmentLineRow};
use hammer_db::sqlx::PgConnection;
use hammer_shared::Id;
use hammer_shared::audit as action;
use hammer_shared::decimal::Decimal;
use hammer_shared::document::{DocumentRef, DocumentStatus};
use hammer_shared::inventory::TxnType;
use hammer_shared::schema::{DataType, FieldValue, parse_value};
use serde_json::{Map, Value, json};
use time::Date;

use super::orders::MSG_CUSTOMER_DELETED;
use super::save::{CUSTOMER_ID, LOT_NO, SALES_ORDER_ID, line_errors};
use super::{SALES_ORDER, SALES_ORDER_LINE, SALES_RETURN, SHIPMENT, SHIPMENT_LINE};
use crate::CoreError;
use crate::audit::{AuditDiff, AuditEntry};
use crate::documents::{DocumentContext, DocumentHandler, HandlerFuture};
use crate::formulas::today_of;
use crate::inventory::{
    Allocation, GOODS, Inventory, Line, LockRequest, Source, allocations_of_line, qty4,
};
use crate::purchasing::amounts::{TWD_SCALE, round_to, with_scale};
use crate::purchasing::save::{decimal_of, id_of};
use crate::record_data::FieldError;
use crate::record_lines::{LineError, audit_key};
use crate::stock_adjustments::{MSG_LOT_NOT_FOUND, MSG_NO_LINES, MSG_PRODUCT_DELETED, ProductInfo};

pub const MSG_ORDER_DRAFT: &str = "訂單尚未確認";
pub const MSG_ORDER_CLOSED: &str = "訂單已結案";
pub const MSG_ORDER_VOID: &str = "訂單已作廢";
pub const MSG_ORDER_DELETED: &str = "訂單已刪除";
pub const MSG_CUSTOMER_MISMATCH: &str = "客戶與訂單不同";
pub const MSG_NOT_ORDER_LINE: &str = "不是此訂單的明細";
pub const MSG_PRODUCT_MISMATCH: &str = "商品與訂單明細不同";
pub const MSG_NO_ORDER: &str = "沒有指定訂單";
pub const MSG_SERVICE_LOT: &str = "服務類商品沒有批號";

const PRODUCT_ID: &str = "product_id";
const SALES_ORDER_LINE_ID: &str = "sales_order_line_id";
const QTY: &str = "qty";
const SHIP_DATE: &str = "ship_date";
const SHIPPED_QTY: &str = "shipped_qty";

/// 表頭中檢查用到的值。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShipmentHeader {
    pub sales_order_id: Option<Id>,
    pub customer_id: Option<Id>,
}

impl ShipmentHeader {
    pub fn from_data(data: &Map<String, Value>) -> Self {
        Self {
            sales_order_id: id_of(data.get(SALES_ORDER_ID)),
            customer_id: id_of(data.get(CUSTOMER_ID)),
        }
    }
}

/// 一列明細中檢查用到的值。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShipmentLine {
    pub sales_order_line_id: Option<Id>,
    pub product_id: Option<Id>,
    pub qty: Option<Decimal>,
    /// 去除空白後的批號；空字串為 `None`（先進先出）。
    pub lot_no: Option<String>,
}

impl ShipmentLine {
    pub fn from_data(data: &Map<String, Value>) -> Self {
        Self {
            sales_order_line_id: id_of(data.get(SALES_ORDER_LINE_ID)),
            product_id: id_of(data.get(PRODUCT_ID)),
            qty: decimal_of(data.get(QTY)),
            lot_no: trimmed(data.get(LOT_NO).and_then(Value::as_str)),
        }
    }

    fn from_row(row: &ShipmentLineRow) -> Self {
        Self {
            sales_order_line_id: row.sales_order_line_id,
            product_id: Some(row.product_id),
            qty: Some(row.qty),
            lot_no: trimmed(row.lot_no.as_deref()),
        }
    }
}

fn trimmed(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// 訂單與其明細（檢查「訂單轉出貨單」用）。
#[derive(Debug, Clone, Copy)]
pub struct OrderRefs<'a> {
    pub order: &'a OrderForShipment,
    pub lines: &'a [OrderLineRow],
}

/// 檢查需要的資料庫內容。
pub trait ShipmentReferences {
    /// 商品；不存在時為 `None`。
    fn product(&self, id: Id) -> Option<ProductInfo>;
    /// 商品 `product_id` 是否有未刪除、批號為 `lot_no`（不分大小寫）的批號。
    fn lot_exists(&self, product_id: Id, lot_no: &str) -> bool;
}

fn order_status_error(status: &str) -> Option<&'static str> {
    match DocumentStatus::parse(status) {
        Some(DocumentStatus::Confirmed | DocumentStatus::PartiallyShipped) => None,
        Some(DocumentStatus::Closed) => Some(MSG_ORDER_CLOSED),
        Some(DocumentStatus::Void) => Some(MSG_ORDER_VOID),
        _ => Some(MSG_ORDER_DRAFT),
    }
}

fn field_error(key: &str, message: impl Into<String>) -> FieldError {
    FieldError {
        field_key: key.into(),
        message: message.into(),
    }
}

/// 存檔與過帳共用的檢查（見模組說明），回傳表頭與明細的錯誤。`order` 為表頭指向的訂單（不存在或已刪除
/// 時為 `None`，由關聯檢查回報）；`lines` 為 `(錯誤中的位置, 明細)`，依順序。
pub fn check_shipment(
    header: &ShipmentHeader,
    order: Option<OrderRefs<'_>>,
    lines: &[(usize, ShipmentLine)],
    refs: &impl ShipmentReferences,
) -> (Vec<FieldError>, Vec<LineError>) {
    let mut fields = Vec::new();
    let mut errors = Vec::new();
    let err = |index: usize, key: &str, message: &str| {
        LineError::field(SHIPMENT_LINE, index, key, message)
    };

    if let Some(OrderRefs { order, .. }) = order {
        if let Some(message) = order_status_error(&order.status) {
            fields.push(field_error(SALES_ORDER_ID, message));
        }
        if header.customer_id.is_some_and(|c| c != order.customer_id) {
            fields.push(field_error(CUSTOMER_ID, MSG_CUSTOMER_MISMATCH));
        }
    }

    // 同一列訂單明細的數量合計：(第一列的位置, 合計)
    let mut shipped: BTreeMap<Id, (usize, Decimal)> = BTreeMap::new();
    for (index, line) in lines {
        let index = *index;
        match (line.sales_order_line_id, order) {
            (Some(_), None) if header.sales_order_id.is_none() => {
                errors.push(err(index, SALES_ORDER_LINE_ID, MSG_NO_ORDER));
            }
            (
                Some(id),
                Some(OrderRefs {
                    lines: order_lines, ..
                }),
            ) => match order_lines.iter().find(|l| l.id == id) {
                None => errors.push(err(index, SALES_ORDER_LINE_ID, MSG_NOT_ORDER_LINE)),
                Some(order_line) => {
                    if line.product_id.is_some_and(|p| p != order_line.product_id) {
                        errors.push(err(index, PRODUCT_ID, MSG_PRODUCT_MISMATCH));
                    }
                    let entry = shipped.entry(id).or_insert((index, Decimal::ZERO));
                    entry.1 += line.qty.unwrap_or_default();
                }
            },
            _ => {}
        }

        let Some(product_id) = line.product_id else {
            continue;
        };
        let product = refs.product(product_id);
        if product.is_none_or(|p| p.deleted) {
            errors.push(err(index, PRODUCT_ID, MSG_PRODUCT_DELETED));
            continue;
        }
        let Some(lot_no) = &line.lot_no else {
            continue;
        };
        if product.is_some_and(|p| !p.is_goods) {
            errors.push(err(index, LOT_NO, MSG_SERVICE_LOT));
        } else if !refs.lot_exists(product_id, lot_no) {
            errors.push(err(index, LOT_NO, MSG_LOT_NOT_FOUND));
        }
    }

    if let Some(OrderRefs {
        lines: order_lines, ..
    }) = order
    {
        for (id, (index, qty)) in shipped {
            let Some(order_line) = order_lines.iter().find(|l| l.id == id) else {
                continue;
            };
            let remaining = order_line.qty - order_line.shipped_qty;
            if qty > remaining {
                errors.push(err(
                    index,
                    QTY,
                    &format!("超過未出貨數量 {}", qty4(remaining.max(Decimal::ZERO))),
                ));
            }
        }
    }
    errors.sort_by_key(|e| e.index);
    (fields, errors)
}

/// 訂單在出貨過帳、作廢後的狀態（§8.9「訂單的狀態」）：手動結案維持 `closed`；每一列都
/// `shipped_qty ≥ qty` → `closed`（自動結案）；有任何一列 `shipped_qty > 0` → `partially_shipped`；
/// 都是 0 → `confirmed`。`lines` 為 `(qty, shipped_qty)`。
pub fn order_status(closed_manually: bool, lines: &[(Decimal, Decimal)]) -> DocumentStatus {
    if closed_manually || (!lines.is_empty() && lines.iter().all(|(q, s)| s >= q)) {
        DocumentStatus::Closed
    } else if lines.iter().any(|(_, s)| *s > Decimal::ZERO) {
        DocumentStatus::PartiallyShipped
    } else {
        DocumentStatus::Confirmed
    }
}

/// 一列的銷貨成本：`round(Σ qty × unit_cost_twd, 2)`。
pub fn cost_amount(allocations: &[Allocation]) -> Decimal {
    let sum: Decimal = allocations.iter().map(|a| a.qty * a.unit_cost_twd).sum();
    round_to(sum, TWD_SCALE, TWD_SCALE)
}

/// 存檔時查詢資料庫的內容。
struct SavedReferences {
    products: HashMap<Id, ProductInfo>,
    lots: BTreeSet<(Id, String)>,
}

impl ShipmentReferences for SavedReferences {
    fn product(&self, id: Id) -> Option<ProductInfo> {
        self.products.get(&id).copied()
    }
    fn lot_exists(&self, product_id: Id, lot_no: &str) -> bool {
        self.lots.contains(&(product_id, lot_no.to_lowercase()))
    }
}

/// 過帳時以鎖定的商品與批號為準。
struct LockedReferences<'a>(&'a Inventory);

impl ShipmentReferences for LockedReferences<'_> {
    fn product(&self, id: Id) -> Option<ProductInfo> {
        self.0.product(id).map(|p| ProductInfo {
            is_goods: p.is_goods,
            deleted: p.deleted,
        })
    }
    fn lot_exists(&self, product_id: Id, lot_no: &str) -> bool {
        self.0.lot_by_no(product_id, lot_no).is_some()
    }
}

/// 出貨單存檔時的檢查（`entity_key` 不是出貨單、或主表欄位已有錯誤時不檢查）。`lines` 為通過欄位驗證的
/// 出貨明細 `(request 中的位置, data)`；修改時包含所有明細。表頭的錯誤併入 `master`。
pub(crate) async fn check_save(
    conn: &mut PgConnection,
    entity_key: &str,
    master: Result<Map<String, Value>, Vec<FieldError>>,
    lines: &[(usize, &Map<String, Value>)],
) -> Result<(Result<Map<String, Value>, Vec<FieldError>>, Vec<LineError>), CoreError> {
    if entity_key != SHIPMENT {
        return Ok((master, Vec::new()));
    }
    let Ok(data) = master else {
        return Ok((master, Vec::new()));
    };
    let header = ShipmentHeader::from_data(&data);
    let parsed: Vec<(usize, ShipmentLine)> = lines
        .iter()
        .map(|(i, d)| (*i, ShipmentLine::from_data(d)))
        .collect();

    let order = match header.sales_order_id {
        Some(id) => db::find_order_for_shipment(&mut *conn, id).await?,
        None => None,
    };
    let order_lines = match &order {
        Some(o) => db::list_order_lines(&mut *conn, o.id).await?,
        None => Vec::new(),
    };
    let product_ids: Vec<Id> = parsed
        .iter()
        .filter_map(|(_, l)| l.product_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let products = inventory_db::find_products(&mut *conn, &product_ids)
        .await?
        .into_iter()
        .map(|r| {
            let info = ProductInfo {
                is_goods: r.product_type == GOODS,
                deleted: r.deleted,
            };
            (r.id, info)
        })
        .collect();
    let mut lots = BTreeSet::new();
    for (_, line) in &parsed {
        if let (Some(product_id), Some(lot_no)) = (line.product_id, &line.lot_no)
            && inventory_db::find_lot_by_no(&mut *conn, product_id, lot_no)
                .await?
                .is_some()
        {
            lots.insert((product_id, lot_no.to_lowercase()));
        }
    }
    let refs = SavedReferences { products, lots };
    let order_refs = order.as_ref().map(|order| OrderRefs {
        order,
        lines: &order_lines,
    });
    let (fields, line_errors) = check_shipment(&header, order_refs, &parsed, &refs);
    let master = if fields.is_empty() {
        Ok(data)
    } else {
        Err(fields)
    };
    Ok((master, line_errors))
}

/// 出貨單的過帳、作廢處理函式（[`crate::documents::DocumentHandlers::standard`] 註冊）。
#[derive(Debug, Default, Clone, Copy)]
pub struct ShipmentHandler;

impl DocumentHandler for ShipmentHandler {
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

fn ship_date(doc: &DocumentContext<'_>) -> Result<Date, CoreError> {
    match parse_value(
        DataType::Date,
        doc.data.get(SHIP_DATE).unwrap_or(&Value::Null),
    ) {
        Ok(Some(FieldValue::Date(d))) => Ok(d),
        _ => Err(CoreError::Internal(format!(
            "單據 {} 的出貨日期不正確",
            doc.record_no
        ))),
    }
}

fn line_of(index: usize, row: &ShipmentLineRow) -> Result<Line, CoreError> {
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

/// 鎖定表頭指向的訂單（`FOR UPDATE`）。訂單已刪除時為錯誤。
async fn lock_order(
    conn: &mut PgConnection,
    header: &ShipmentHeader,
) -> Result<Option<OrderForShipment>, CoreError> {
    let Some(id) = header.sales_order_id else {
        return Ok(None);
    };
    match db::lock_order_for_shipment(&mut *conn, id).await? {
        Some(order) => Ok(Some(order)),
        None => Err(CoreError::InvalidFields(vec![field_error(
            SALES_ORDER_ID,
            MSG_ORDER_DELETED,
        )])),
    }
}

async fn post(conn: &mut PgConnection, doc: &DocumentContext<'_>) -> Result<(), CoreError> {
    let header = ShipmentHeader::from_data(doc.data);
    let order = lock_order(conn, &header).await?;
    let rows = db::list_shipment_lines_for_update(&mut *conn, doc.record_id).await?;
    let lines: Vec<(usize, ShipmentLine)> = rows
        .iter()
        .map(ShipmentLine::from_row)
        .enumerate()
        .collect();

    let mut fields = Vec::new();
    let customer_deleted = match header.customer_id {
        Some(id) => db::lock_customer(&mut *conn, id).await? != Some(false),
        None => true,
    };
    if customer_deleted {
        fields.push(field_error(CUSTOMER_ID, MSG_CUSTOMER_DELETED));
    }

    let req = LockRequest {
        product_ids: rows.iter().map(|r| r.product_id).collect(),
        lot_numbers: lines
            .iter()
            .filter_map(|(_, l)| Some((l.product_id?, l.lot_no.clone()?)))
            .collect(),
        fifo_product_ids: lines
            .iter()
            .filter(|(_, l)| l.lot_no.is_none())
            .filter_map(|(_, l)| l.product_id)
            .collect(),
        ..LockRequest::default()
    };
    let mut inv = Inventory::lock(conn, &req).await?;
    let order_lines = match &order {
        Some(o) => db::list_order_lines_for_update(&mut *conn, o.id).await?,
        None => Vec::new(),
    };

    let order_refs = order.as_ref().map(|order| OrderRefs {
        order,
        lines: &order_lines,
    });
    let (more_fields, mut errors) =
        check_shipment(&header, order_refs, &lines, &LockedReferences(&inv));
    fields.extend(more_fields);
    if rows.is_empty() {
        errors.insert(0, LineError::detail(SHIPMENT_LINE, MSG_NO_LINES));
    }
    for (index, row) in rows.iter().enumerate() {
        errors.extend(LineError::from_fields(
            SHIPMENT_LINE,
            index,
            line_errors(Some(row.qty), Some(row.unit_price), Some(row.discount_rate)),
        ));
    }
    validation(fields, errors)?;

    let src = Source {
        doc_type: SHIPMENT,
        line_type: SHIPMENT_LINE,
        doc_id: doc.record_id,
        txn_date: ship_date(doc)?,
        actor: doc.actor,
    };
    // 每一列扣除的結果：(指定的批號, 批號扣除)
    let mut issued: Vec<(Option<Id>, Vec<Allocation>)> = vec![(None, Vec::new()); rows.len()];
    // 先處理指定批號的列，避免先進先出先用掉使用者指定的批號
    for (index, (row, (_, line))) in rows.iter().zip(&lines).enumerate() {
        let Some(lot_no) = &line.lot_no else {
            continue;
        };
        let lot_id = inv
            .lot_by_no(row.product_id, lot_no)
            .map(|l| l.id)
            .ok_or_else(|| CoreError::Internal(format!("批號 {lot_no} 沒有先鎖定")))?;
        let at = line_of(index, row)?;
        if let Some(a) = inv
            .issue_from_lot(
                conn,
                &src,
                at,
                row.product_id,
                lot_id,
                row.qty,
                TxnType::Shipment,
            )
            .await?
        {
            issued[index] = (Some(lot_id), vec![a]);
        }
    }
    for (index, (row, (_, line))) in rows.iter().zip(&lines).enumerate() {
        if line.lot_no.is_some() {
            continue;
        }
        let at = line_of(index, row)?;
        let allocations = inv
            .issue_fifo(conn, &src, at, row.product_id, row.qty, TxnType::Shipment)
            .await?;
        issued[index] = (None, allocations);
    }
    let goods: BTreeSet<Id> = rows
        .iter()
        .filter(|r| inv.product(r.product_id).is_some_and(|p| p.is_goods))
        .map(|r| r.product_id)
        .collect();
    inv.finish()?;

    let mut cost_total = Decimal::ZERO;
    let mut deltas: BTreeMap<Id, Decimal> = BTreeMap::new();
    for (row, (lot_id, allocations)) in rows.iter().zip(&issued) {
        let cost = goods
            .contains(&row.product_id)
            .then(|| cost_amount(allocations));
        cost_total += cost.unwrap_or_default();
        if !db::update_shipment_line_posting(&mut *conn, row.id, *lot_id, cost, doc.actor).await? {
            return Err(CoreError::Internal(format!("明細 {} 已不存在", row.id)));
        }
        if let Some(id) = row.sales_order_line_id {
            *deltas.entry(id).or_default() += row.qty;
        }
    }
    if !db::set_shipment_cost_total(&mut *conn, doc.record_id, with_scale(cost_total, TWD_SCALE))
        .await?
    {
        return Err(CoreError::Internal(format!(
            "出貨單 {} 已不存在",
            doc.record_id
        )));
    }
    if let Some(order) = &order {
        apply_shipped(conn, doc, order, &order_lines, &deltas).await?;
    }
    Ok(())
}

async fn void(conn: &mut PgConnection, doc: &DocumentContext<'_>) -> Result<(), CoreError> {
    let returns = db::posted_returns_of_shipment(&mut *conn, doc.record_id).await?;
    if !returns.is_empty() {
        let numbers: Vec<&str> = returns.iter().map(|(_, no)| no.as_str()).collect();
        return Err(CoreError::DocumentsInUse {
            message: format!(
                "此出貨單已有銷貨退回 {}，請先作廢退回單",
                numbers.join("、")
            ),
            documents: returns
                .iter()
                .map(|(id, no)| DocumentRef {
                    entity_key: SALES_RETURN.into(),
                    record_id: *id,
                    record_no: no.clone(),
                })
                .collect(),
        });
    }

    let header = ShipmentHeader::from_data(doc.data);
    let order = match header.sales_order_id {
        Some(id) => db::lock_order_for_shipment(&mut *conn, id).await?,
        None => None,
    };
    let rows = db::list_shipment_lines_for_update(&mut *conn, doc.record_id).await?;
    let mut allocations = Vec::with_capacity(rows.len());
    for row in &rows {
        allocations.push(allocations_of_line(conn, SHIPMENT_LINE, row.id).await?);
    }
    let req = LockRequest {
        product_ids: rows.iter().map(|r| r.product_id).collect(),
        lot_ids: allocations.iter().flatten().map(|a| a.lot_id).collect(),
        ..LockRequest::default()
    };
    let mut inv = Inventory::lock(conn, &req).await?;
    let order_lines = match &order {
        Some(o) => db::list_order_lines_for_update(&mut *conn, o.id).await?,
        None => Vec::new(),
    };

    let src = Source {
        doc_type: SHIPMENT,
        line_type: SHIPMENT_LINE,
        doc_id: doc.record_id,
        txn_date: today_of(doc.now),
        actor: doc.actor,
    };
    let mut deltas: BTreeMap<Id, Decimal> = BTreeMap::new();
    for (index, (row, allocs)) in rows.iter().zip(&allocations).enumerate() {
        inv.reverse_allocations(conn, &src, line_of(index, row)?, allocs)
            .await?;
        if let Some(id) = row.sales_order_line_id {
            *deltas.entry(id).or_default() -= row.qty;
        }
    }
    inv.finish()?;
    if let Some(order) = &order {
        apply_shipped(conn, doc, order, &order_lines, &deltas).await?;
    }
    Ok(())
}

/// 訂單明細的 `shipped_qty` 加上 `deltas`，重新計算訂單的狀態，訂單 `version` + 1 並寫入操作紀錄
/// （`update`，diff 為 `status` 與 `lines.sales_order_line` 的 `shipped_qty`）。
async fn apply_shipped(
    conn: &mut PgConnection,
    doc: &DocumentContext<'_>,
    order: &OrderForShipment,
    order_lines: &[OrderLineRow],
    deltas: &BTreeMap<Id, Decimal>,
) -> Result<(), CoreError> {
    if deltas.is_empty() {
        return Ok(());
    }
    let shipped = |l: &OrderLineRow| l.shipped_qty + deltas.get(&l.id).copied().unwrap_or_default();
    for line in order_lines {
        let delta = deltas.get(&line.id).copied().unwrap_or_default();
        if delta != Decimal::ZERO
            && !db::add_shipped_qty(&mut *conn, line.id, delta, doc.actor).await?
        {
            return Err(CoreError::Internal(format!(
                "訂單明細 {} 已不存在",
                line.id
            )));
        }
    }
    let status = if order.status == DocumentStatus::Void.as_str() {
        DocumentStatus::Void
    } else {
        let pairs: Vec<(Decimal, Decimal)> =
            order_lines.iter().map(|l| (l.qty, shipped(l))).collect();
        order_status(order.closed_manually, &pairs)
    };
    if !db::set_order_status(&mut *conn, order.id, status.as_str(), doc.actor).await? {
        return Err(CoreError::Internal(format!("訂單 {} 已不存在", order.id)));
    }

    let key = audit_key(SALES_ORDER_LINE);
    let lines_value = |value: &dyn Fn(&OrderLineRow) -> Decimal| {
        Value::Array(
            order_lines
                .iter()
                .map(|l| json!({ "id": l.id, "data": { SHIPPED_QTY: qty4(value(l)).to_string() } }))
                .collect(),
        )
    };
    let before = json!({
        "status": order.status,
        key.clone(): lines_value(&|l| l.shipped_qty),
    });
    let after = json!({
        "status": status.as_str(),
        key: lines_value(&shipped),
    });
    crate::audit::record(
        conn,
        &AuditEntry {
            user_id: Some(doc.actor),
            action: action::UPDATE,
            entity_key: Some(SALES_ORDER),
            record_id: Some(order.id),
            diff: AuditDiff::between(Some(&before), Some(&after))?,
            client_info: doc.client_info,
            occurred_at: doc.now,
        },
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn id(n: u128) -> Id {
        Id::from_u128(n)
    }

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    struct Refs;

    impl ShipmentReferences for Refs {
        fn product(&self, pid: Id) -> Option<ProductInfo> {
            match pid.as_u128() {
                1 | 2 => Some(ProductInfo {
                    is_goods: true,
                    deleted: false,
                }),
                3 => Some(ProductInfo {
                    is_goods: false,
                    deleted: false,
                }),
                4 => Some(ProductInfo {
                    is_goods: true,
                    deleted: true,
                }),
                _ => None,
            }
        }
        fn lot_exists(&self, product_id: Id, lot_no: &str) -> bool {
            product_id == id(1) && lot_no.eq_ignore_ascii_case("L-01")
        }
    }

    fn order(status: &str) -> OrderForShipment {
        OrderForShipment {
            id: id(100),
            record_no: "SO202611-0001".into(),
            status: status.into(),
            customer_id: id(50),
            tax_type: "taxable".into(),
            closed_manually: false,
        }
    }

    fn order_line(n: u128, product: u128, qty: &str, shipped: &str) -> OrderLineRow {
        OrderLineRow {
            id: id(n),
            sort_order: 0,
            product_id: id(product),
            qty: d(qty),
            unit_price: d("1"),
            discount_rate: d("0"),
            shipped_qty: d(shipped),
        }
    }

    fn line(so_line: Option<u128>, product: u128, qty: &str, lot: Option<&str>) -> ShipmentLine {
        ShipmentLine {
            sales_order_line_id: so_line.map(id),
            product_id: Some(id(product)),
            qty: Some(d(qty)),
            lot_no: lot.map(str::to_owned),
        }
    }

    fn header(order: bool) -> ShipmentHeader {
        ShipmentHeader {
            sales_order_id: order.then(|| id(100)),
            customer_id: Some(id(50)),
        }
    }

    fn messages(errors: &[LineError]) -> Vec<(Option<usize>, String)> {
        errors
            .iter()
            .map(|e| (e.index, e.message.clone()))
            .collect()
    }

    #[test]
    fn lots_are_checked_by_product_type() {
        let lines = vec![
            (0, line(None, 1, "1", Some("l-01"))),
            // 同一批號可以出現在多列
            (1, line(None, 1, "1", Some("L-01"))),
            (2, line(None, 1, "1", Some("NOPE"))),
            (3, line(None, 2, "1", Some("L-01"))),
            (4, line(None, 3, "1", Some("X"))),
            (5, line(None, 4, "1", None)),
            (6, line(None, 3, "1", None)),
            (7, line(None, 1, "1", None)),
        ];
        let (fields, errors) = check_shipment(&header(false), None, &lines, &Refs);
        assert!(fields.is_empty());
        assert_eq!(
            messages(&errors),
            [
                (Some(2), MSG_LOT_NOT_FOUND.to_owned()),
                (Some(3), MSG_LOT_NOT_FOUND.to_owned()),
                (Some(4), MSG_SERVICE_LOT.to_owned()),
                (Some(5), MSG_PRODUCT_DELETED.to_owned()),
            ]
        );
    }

    #[test]
    fn order_line_requires_order() {
        let lines = vec![(0, line(Some(9), 1, "1", None))];
        let (_, errors) = check_shipment(&header(false), None, &lines, &Refs);
        assert_eq!(messages(&errors), [(Some(0), MSG_NO_ORDER.to_owned())]);
        assert_eq!(errors[0].field_key.as_deref(), Some(SALES_ORDER_LINE_ID));
    }

    #[test]
    fn order_header_must_match() {
        let order = order("confirmed");
        let refs = OrderRefs {
            order: &order,
            lines: &[],
        };
        let mut h = header(true);
        h.customer_id = Some(id(51));
        let (fields, _) = check_shipment(&h, Some(refs), &[], &Refs);
        assert_eq!(fields[0].field_key, CUSTOMER_ID);
        assert_eq!(fields[0].message, MSG_CUSTOMER_MISMATCH);

        for (status, message) in [
            ("draft", Some(MSG_ORDER_DRAFT)),
            ("confirmed", None),
            ("partially_shipped", None),
            ("closed", Some(MSG_ORDER_CLOSED)),
            ("void", Some(MSG_ORDER_VOID)),
        ] {
            let order = self::order(status);
            let refs = OrderRefs {
                order: &order,
                lines: &[],
            };
            let (fields, _) = check_shipment(&header(true), Some(refs), &[], &Refs);
            assert_eq!(
                fields.first().map(|f| f.message.as_str()),
                message,
                "{status}"
            );
        }
    }

    #[test]
    fn order_lines_must_match_and_not_over_ship() {
        let order = order("partially_shipped");
        let order_lines = [order_line(10, 1, "10", "7"), order_line(11, 2, "5", "0")];
        let refs = OrderRefs {
            order: &order,
            lines: &order_lines,
        };
        let lines = vec![
            // 10：未出貨 3，拆成兩列（不同批號）合計 4 → 第一列錯誤
            (0, line(Some(10), 1, "2", Some("L-01"))),
            (1, line(Some(10), 1, "2", None)),
            // 11：商品不同
            (2, line(Some(11), 1, "1", None)),
            // 不是此訂單的明細
            (3, line(Some(99), 1, "1", None)),
            // 客戶多要的：沒有訂單明細
            (4, line(None, 2, "100", None)),
        ];
        let (fields, errors) = check_shipment(&header(true), Some(refs), &lines, &Refs);
        assert!(fields.is_empty());
        assert_eq!(
            messages(&errors),
            [
                (Some(0), "超過未出貨數量 3.0000".to_owned()),
                (Some(2), MSG_PRODUCT_MISMATCH.to_owned()),
                (Some(3), MSG_NOT_ORDER_LINE.to_owned()),
            ]
        );

        let lines = vec![
            (0, line(Some(10), 1, "1", Some("L-01"))),
            (1, line(Some(10), 1, "2", None)),
        ];
        let (_, errors) = check_shipment(&header(true), Some(refs), &lines, &Refs);
        assert!(errors.is_empty());
    }

    #[test]
    fn order_status_is_recalculated() {
        use DocumentStatus::*;
        let l = |q: &str, s: &str| (d(q), d(s));
        assert_eq!(order_status(false, &[l("10", "0"), l("5", "0")]), Confirmed);
        assert_eq!(
            order_status(false, &[l("10", "3"), l("5", "0")]),
            PartiallyShipped
        );
        assert_eq!(order_status(false, &[l("10", "10"), l("5", "5")]), Closed);
        assert_eq!(order_status(true, &[l("10", "0")]), Closed);
    }

    #[test]
    fn cost_is_rounded_to_two_places() {
        let a = |q: &str, c: &str| Allocation {
            lot_id: id(1),
            qty: d(q),
            unit_cost_twd: d(c),
        };
        // 5 × 80 + 7 × 82.5 = 977.5
        assert_eq!(
            cost_amount(&[a("5", "80.0000"), a("7", "82.5000")]).to_string(),
            "977.50"
        );
        // 3 × 33.3333 = 99.9999 → 100.00；0.005 → 0.01（MidpointAwayFromZero）
        assert_eq!(cost_amount(&[a("3", "33.3333")]).to_string(), "100.00");
        assert_eq!(cost_amount(&[a("1", "0.0050")]).to_string(), "0.01");
        assert_eq!(cost_amount(&[]).to_string(), "0.00");
    }
}
