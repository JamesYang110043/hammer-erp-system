//! 進貨單（系統單據 `goods_receipt`，docs/architecture.md §8.8「存檔時的檢查」「採購單轉進貨單」
//! 「進貨過帳與作廢」、§9.1、§9.3）：存檔時的檢查，以及過帳、作廢（[`GoodsReceiptHandler`]，掛在
//! [`crate::documents`] 的框架上）。幣別、匯率、金額與採購單相同（[`super::save`]）。
//!
//! # 存檔時的檢查（[`check_receipt`]，存檔與過帳共用）
//!
//! - `goods`：批號有值時不可與該商品未刪除的批號重複（「批號已存在」）；同一張單據中同一商品的同一批號
//!   （不分大小寫）只能出現一次（「批號重複」）；`service`：不可填批號（「服務類商品沒有批號」）
//! - 有 `purchase_order_id`：採購單是 `confirmed` / `partially_received`；廠商、幣別與採購單相同；
//!   `purchase_order_line_id` 是該採購單的明細、商品相同；同一列採購明細的 `qty` 合計不超過
//!   `qty − received_qty`（只計算已過帳的進貨，草稿不保留數量）
//! - 沒有 `purchase_order_id`：明細不可有 `purchase_order_line_id`
//!
//! # 過帳
//!
//! 鎖定順序：進貨單（框架已 `FOR UPDATE`）→ 採購單（`FOR UPDATE`）→ 明細（`FOR UPDATE`）→ 廠商
//! （`FOR SHARE`）→ 商品（`FOR SHARE`）、指定批號的既有批號 → 採購明細（`FOR UPDATE`）。以鎖定後的資料
//! 重新執行存檔時的檢查，另外檢查匯率、至少一列明細、廠商與商品未刪除。之後依 `sort_order`：
//!
//! - `unit_cost_twd = round(unit_price × exchange_rate, 4)` 寫入明細（`service` 也寫入）
//! - `goods`：建立批號（批號留空時自動產生 `{單號}-{明細序號}`）、異動 `receipt`；明細寫入 `lot_id`
//! - 有 `purchase_order_line_id`：採購明細的 `received_qty += qty`，重新計算採購單的狀態
//!   （[`order_status`]），採購單與明細的 `version` + 1，採購單寫入操作紀錄（`update`）
//!
//! # 作廢
//!
//! 有已過帳的進貨退回 → `422 VALIDATION`（[`CoreError::DocumentsInUse`]）。`goods` 的批號沒有其他單據的
//! 有效異動時新增 `void_reversal` 異動並軟刪除；被使用時 `422 LOT_IN_USE`。採購明細的
//! `received_qty −= qty`，重新計算採購單的狀態（手動結案的維持 `closed`）。

use std::collections::{BTreeMap, BTreeSet, HashMap};

use hammer_db::inventory as inventory_db;
use hammer_db::purchasing::{self as db, OrderForReceipt, OrderLineRow, ReceiptLineRow};
use hammer_db::sqlx::PgConnection;
use hammer_shared::Id;
use hammer_shared::audit as action;
use hammer_shared::decimal::Decimal;
use hammer_shared::document::{DocumentRef, DocumentStatus};
use hammer_shared::inventory::TxnType;
use hammer_shared::schema::{DataType, FieldValue, parse_value};
use serde_json::{Map, Value, json};
use time::Date;

use super::amounts;
use super::orders::MSG_SUPPLIER_DELETED;
use super::save::{
    CURRENCY_CODE, EXCHANGE_RATE, GOODS_RECEIPT_LINE, LOT_NO, MSG_RATE_MISSING, PURCHASE_ORDER_ID,
    PURCHASE_ORDER_LINE, QTY, SUPPLIER_ID, decimal_of, id_of, line_errors,
};
use super::{GOODS_RECEIPT, PURCHASE_ORDER, PURCHASE_RETURN};
use crate::CoreError;
use crate::audit::{AuditDiff, AuditEntry};
use crate::documents::{DocumentContext, DocumentHandler, HandlerFuture};
use crate::formulas::today_of;
use crate::inventory::{GOODS, Inventory, LOT_NO_TAKEN, Line, LockRequest, Receipt, Source, qty4};
use crate::record_data::FieldError;
use crate::record_lines::{LineError, audit_key};
use crate::stock_adjustments::{MSG_LOT_DUPLICATE, MSG_NO_LINES, MSG_PRODUCT_DELETED, ProductInfo};

pub const MSG_ORDER_DRAFT: &str = "採購單尚未確認";
pub const MSG_ORDER_CLOSED: &str = "採購單已結案";
pub const MSG_ORDER_VOID: &str = "採購單已作廢";
pub const MSG_ORDER_DELETED: &str = "採購單已刪除";
pub const MSG_SUPPLIER_MISMATCH: &str = "廠商與採購單不同";
pub const MSG_CURRENCY_MISMATCH: &str = "幣別與採購單不同";
pub const MSG_NOT_ORDER_LINE: &str = "不是此採購單的明細";
pub const MSG_PRODUCT_MISMATCH: &str = "商品與採購明細不同";
pub const MSG_NO_ORDER: &str = "沒有指定採購單";
pub const MSG_SERVICE_LOT: &str = "服務類商品沒有批號";

const PRODUCT_ID: &str = "product_id";
const PURCHASE_ORDER_LINE_ID: &str = "purchase_order_line_id";
const RECEIPT_DATE: &str = "receipt_date";
const RECEIVED_QTY: &str = "received_qty";

/// 表頭中檢查用到的值。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReceiptHeader {
    pub purchase_order_id: Option<Id>,
    pub supplier_id: Option<Id>,
    pub currency_code: Option<String>,
}

impl ReceiptHeader {
    pub fn from_data(data: &Map<String, Value>) -> Self {
        Self {
            purchase_order_id: id_of(data.get(PURCHASE_ORDER_ID)),
            supplier_id: id_of(data.get(SUPPLIER_ID)),
            currency_code: data
                .get(CURRENCY_CODE)
                .and_then(Value::as_str)
                .map(str::to_owned),
        }
    }
}

/// 一列明細中檢查用到的值。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReceiptLine {
    pub purchase_order_line_id: Option<Id>,
    pub product_id: Option<Id>,
    pub qty: Option<Decimal>,
    /// 去除空白後的批號；空字串為 `None`。
    pub lot_no: Option<String>,
}

impl ReceiptLine {
    pub fn from_data(data: &Map<String, Value>) -> Self {
        Self {
            purchase_order_line_id: id_of(data.get(PURCHASE_ORDER_LINE_ID)),
            product_id: id_of(data.get(PRODUCT_ID)),
            qty: decimal_of(data.get(QTY)),
            lot_no: trimmed(data.get(LOT_NO).and_then(Value::as_str)),
        }
    }

    fn from_row(row: &ReceiptLineRow) -> Self {
        Self {
            purchase_order_line_id: row.purchase_order_line_id,
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

/// 採購單與其明細（檢查「採購單轉進貨單」用）。
#[derive(Debug, Clone, Copy)]
pub struct OrderRefs<'a> {
    pub order: &'a OrderForReceipt,
    pub lines: &'a [OrderLineRow],
}

/// 檢查需要的資料庫內容。
pub trait ReceiptReferences {
    /// 商品；不存在時為 `None`。
    fn product(&self, id: Id) -> Option<ProductInfo>;
    /// 商品 `product_id` 是否有未刪除、批號為 `lot_no`（不分大小寫）的批號。
    fn lot_exists(&self, product_id: Id, lot_no: &str) -> bool;
}

fn order_status_error(status: &str) -> Option<&'static str> {
    match DocumentStatus::parse(status) {
        Some(DocumentStatus::Confirmed | DocumentStatus::PartiallyReceived) => None,
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

/// 存檔與過帳共用的檢查（見模組說明），回傳表頭與明細的錯誤。`order` 為表頭指向的採購單（不存在或
/// 已刪除時為 `None`，由關聯檢查回報）；`lines` 為 `(錯誤中的位置, 明細)`，依順序。
pub fn check_receipt(
    header: &ReceiptHeader,
    order: Option<OrderRefs<'_>>,
    lines: &[(usize, ReceiptLine)],
    refs: &impl ReceiptReferences,
) -> (Vec<FieldError>, Vec<LineError>) {
    let mut fields = Vec::new();
    let mut errors = Vec::new();
    let err = |index: usize, key: &str, message: &str| {
        LineError::field(GOODS_RECEIPT_LINE, index, key, message)
    };

    if let Some(OrderRefs { order, .. }) = order {
        if let Some(message) = order_status_error(&order.status) {
            fields.push(field_error(PURCHASE_ORDER_ID, message));
        }
        if header.supplier_id.is_some_and(|s| s != order.supplier_id) {
            fields.push(field_error(SUPPLIER_ID, MSG_SUPPLIER_MISMATCH));
        }
        if header
            .currency_code
            .as_deref()
            .is_some_and(|c| c != order.currency_code)
        {
            fields.push(field_error(CURRENCY_CODE, MSG_CURRENCY_MISMATCH));
        }
    }

    // 同一列採購明細的數量合計：(第一列的位置, 合計)
    let mut received: BTreeMap<Id, (usize, Decimal)> = BTreeMap::new();
    let mut seen: BTreeSet<(Id, String)> = BTreeSet::new();
    for (index, line) in lines {
        let index = *index;
        match (line.purchase_order_line_id, order) {
            (Some(_), None) if header.purchase_order_id.is_none() => {
                errors.push(err(index, PURCHASE_ORDER_LINE_ID, MSG_NO_ORDER));
            }
            (
                Some(id),
                Some(OrderRefs {
                    lines: order_lines, ..
                }),
            ) => match order_lines.iter().find(|l| l.id == id) {
                None => errors.push(err(index, PURCHASE_ORDER_LINE_ID, MSG_NOT_ORDER_LINE)),
                Some(order_line) => {
                    if line.product_id.is_some_and(|p| p != order_line.product_id) {
                        errors.push(err(index, PRODUCT_ID, MSG_PRODUCT_MISMATCH));
                    }
                    let entry = received.entry(id).or_insert((index, Decimal::ZERO));
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
            continue;
        }
        if !seen.insert((product_id, lot_no.to_lowercase())) {
            errors.push(err(index, LOT_NO, MSG_LOT_DUPLICATE));
        } else if refs.lot_exists(product_id, lot_no) {
            errors.push(err(index, LOT_NO, LOT_NO_TAKEN));
        }
    }

    if let Some(OrderRefs {
        lines: order_lines, ..
    }) = order
    {
        for (id, (index, qty)) in received {
            let Some(order_line) = order_lines.iter().find(|l| l.id == id) else {
                continue;
            };
            let remaining = order_line.qty - order_line.received_qty;
            if qty > remaining {
                errors.push(err(
                    index,
                    QTY,
                    &format!("超過未進貨數量 {}", qty4(remaining.max(Decimal::ZERO))),
                ));
            }
        }
    }
    errors.sort_by_key(|e| e.index);
    (fields, errors)
}

/// 採購單在進貨過帳、作廢後的狀態（§8.8「狀態的重新計算」）：手動結案維持 `closed`；每一列都
/// `received_qty ≥ qty` → `closed`（自動結案）；有任何一列 `received_qty > 0` → `partially_received`；
/// 都是 0 → `confirmed`。`lines` 為 `(qty, received_qty)`。
pub fn order_status(closed_manually: bool, lines: &[(Decimal, Decimal)]) -> DocumentStatus {
    if closed_manually || (!lines.is_empty() && lines.iter().all(|(q, r)| r >= q)) {
        DocumentStatus::Closed
    } else if lines.iter().any(|(_, r)| *r > Decimal::ZERO) {
        DocumentStatus::PartiallyReceived
    } else {
        DocumentStatus::Confirmed
    }
}

/// 存檔時查詢資料庫的內容。
struct SavedReferences {
    products: HashMap<Id, ProductInfo>,
    lots: BTreeSet<(Id, String)>,
}

impl ReceiptReferences for SavedReferences {
    fn product(&self, id: Id) -> Option<ProductInfo> {
        self.products.get(&id).copied()
    }
    fn lot_exists(&self, product_id: Id, lot_no: &str) -> bool {
        self.lots.contains(&(product_id, lot_no.to_lowercase()))
    }
}

/// 過帳時以鎖定的商品與批號為準。
struct LockedReferences<'a>(&'a Inventory);

impl ReceiptReferences for LockedReferences<'_> {
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

/// 進貨單存檔時的檢查（`entity_key` 不是進貨單、或主表欄位已有錯誤時不檢查）。`lines` 為通過欄位驗證的
/// 進貨明細 `(request 中的位置, data)`；修改時包含所有明細。表頭的錯誤併入 `master`。
pub(crate) async fn check_save(
    conn: &mut PgConnection,
    entity_key: &str,
    master: Result<Map<String, Value>, Vec<FieldError>>,
    lines: &[(usize, &Map<String, Value>)],
) -> Result<(Result<Map<String, Value>, Vec<FieldError>>, Vec<LineError>), CoreError> {
    if entity_key != GOODS_RECEIPT {
        return Ok((master, Vec::new()));
    }
    let Ok(data) = master else {
        return Ok((master, Vec::new()));
    };
    let header = ReceiptHeader::from_data(&data);
    let parsed: Vec<(usize, ReceiptLine)> = lines
        .iter()
        .map(|(i, d)| (*i, ReceiptLine::from_data(d)))
        .collect();

    let order = match header.purchase_order_id {
        Some(id) => db::find_order_for_receipt(&mut *conn, id).await?,
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
    let (fields, line_errors) = check_receipt(&header, order_refs, &parsed, &refs);
    let master = if fields.is_empty() {
        Ok(data)
    } else {
        Err(fields)
    };
    Ok((master, line_errors))
}

/// 進貨單的過帳、作廢處理函式（[`crate::documents::DocumentHandlers::standard`] 註冊）。
#[derive(Debug, Default, Clone, Copy)]
pub struct GoodsReceiptHandler;

impl DocumentHandler for GoodsReceiptHandler {
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

fn receipt_date(doc: &DocumentContext<'_>) -> Result<Date, CoreError> {
    match parse_value(
        DataType::Date,
        doc.data.get(RECEIPT_DATE).unwrap_or(&Value::Null),
    ) {
        Ok(Some(FieldValue::Date(d))) => Ok(d),
        _ => Err(CoreError::Internal(format!(
            "單據 {} 的進貨日期不正確",
            doc.record_no
        ))),
    }
}

fn line_of(index: usize, row: &ReceiptLineRow) -> Result<Line, CoreError> {
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

/// 鎖定表頭指向的採購單（`FOR UPDATE`）。採購單已刪除時為錯誤。
async fn lock_order(
    conn: &mut PgConnection,
    header: &ReceiptHeader,
) -> Result<Option<OrderForReceipt>, CoreError> {
    let Some(id) = header.purchase_order_id else {
        return Ok(None);
    };
    match db::lock_order_for_receipt(&mut *conn, id).await? {
        Some(order) => Ok(Some(order)),
        None => Err(CoreError::InvalidFields(vec![field_error(
            PURCHASE_ORDER_ID,
            MSG_ORDER_DELETED,
        )])),
    }
}

async fn post(conn: &mut PgConnection, doc: &DocumentContext<'_>) -> Result<(), CoreError> {
    let header = ReceiptHeader::from_data(doc.data);
    let order = lock_order(conn, &header).await?;
    let rows = db::list_receipt_lines_for_update(&mut *conn, doc.record_id).await?;
    let lines: Vec<(usize, ReceiptLine)> =
        rows.iter().map(ReceiptLine::from_row).enumerate().collect();

    let mut fields = Vec::new();
    let supplier_deleted = match header.supplier_id {
        Some(id) => db::lock_supplier(&mut *conn, id).await? != Some(false),
        None => true,
    };
    if supplier_deleted {
        fields.push(field_error(SUPPLIER_ID, MSG_SUPPLIER_DELETED));
    }
    let rate = decimal_of(doc.data.get(EXCHANGE_RATE));
    if rate.is_none() {
        fields.push(field_error(EXCHANGE_RATE, MSG_RATE_MISSING));
    }

    let req = LockRequest {
        product_ids: rows.iter().map(|r| r.product_id).collect(),
        lot_numbers: lines
            .iter()
            .filter_map(|(_, l)| Some((l.product_id?, l.lot_no.clone()?)))
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
        check_receipt(&header, order_refs, &lines, &LockedReferences(&inv));
    fields.extend(more_fields);
    if rows.is_empty() {
        errors.insert(0, LineError::detail(GOODS_RECEIPT_LINE, MSG_NO_LINES));
    }
    for (index, row) in rows.iter().enumerate() {
        errors.extend(LineError::from_fields(
            GOODS_RECEIPT_LINE,
            index,
            line_errors(Some(row.qty), Some(row.unit_price)),
        ));
    }
    validation(fields, errors)?;
    let rate = rate.ok_or_else(|| CoreError::Internal("進貨單沒有匯率".into()))?;

    let src = Source {
        doc_type: GOODS_RECEIPT,
        line_type: GOODS_RECEIPT_LINE,
        doc_id: doc.record_id,
        txn_date: receipt_date(doc)?,
        actor: doc.actor,
    };
    let mut deltas: BTreeMap<Id, Decimal> = BTreeMap::new();
    for (index, (row, (_, line))) in rows.iter().zip(&lines).enumerate() {
        let at = line_of(index, row)?;
        let unit_cost = amounts::unit_cost_twd(row.unit_price, rate);
        let receipt = Receipt {
            product_id: row.product_id,
            lot_no: line.lot_no.as_deref(),
            record_no: doc.record_no,
            sort_order: row.sort_order,
            received_date: src.txn_date,
            qty: row.qty,
            unit_cost_twd: unit_cost,
            txn_type: TxnType::Receipt,
        };
        let lot = match inv.receive(conn, &src, at, &receipt).await {
            Ok(lot) => lot,
            // 自動產生的批號與既有的批號重複（唯一索引擋下）
            Err(CoreError::Validation(m)) if m == LOT_NO_TAKEN => {
                return validation(
                    Vec::new(),
                    vec![LineError::field(
                        GOODS_RECEIPT_LINE,
                        index,
                        LOT_NO,
                        LOT_NO_TAKEN,
                    )],
                );
            }
            Err(e) => return Err(e),
        };
        let (lot_no, lot_id) = match &lot {
            Some(lot) => (Some(lot.lot_no.as_str()), Some(lot.id)),
            None => (None, None),
        };
        if !db::update_receipt_line_posting(
            &mut *conn, row.id, lot_no, unit_cost, lot_id, doc.actor,
        )
        .await?
        {
            return Err(CoreError::Internal(format!("明細 {} 已不存在", row.id)));
        }
        if let Some(id) = row.purchase_order_line_id {
            *deltas.entry(id).or_default() += row.qty;
        }
    }
    inv.finish()?;
    if let Some(order) = &order {
        apply_received(conn, doc, order, &order_lines, &deltas).await?;
    }
    Ok(())
}

async fn void(conn: &mut PgConnection, doc: &DocumentContext<'_>) -> Result<(), CoreError> {
    let returns = db::posted_returns_of_receipt(&mut *conn, doc.record_id).await?;
    if !returns.is_empty() {
        let numbers: Vec<&str> = returns.iter().map(|(_, no)| no.as_str()).collect();
        return Err(CoreError::DocumentsInUse {
            message: format!(
                "此進貨單已有進貨退回 {}，請先作廢退回單",
                numbers.join("、")
            ),
            documents: returns
                .iter()
                .map(|(id, no)| DocumentRef {
                    entity_key: PURCHASE_RETURN.into(),
                    record_id: *id,
                    record_no: no.clone(),
                })
                .collect(),
        });
    }

    let header = ReceiptHeader::from_data(doc.data);
    let order = match header.purchase_order_id {
        Some(id) => db::lock_order_for_receipt(&mut *conn, id).await?,
        None => None,
    };
    let rows = db::list_receipt_lines_for_update(&mut *conn, doc.record_id).await?;
    let req = LockRequest {
        product_ids: rows.iter().map(|r| r.product_id).collect(),
        lot_ids: rows.iter().filter_map(|r| r.lot_id).collect(),
        ..LockRequest::default()
    };
    let mut inv = Inventory::lock(conn, &req).await?;
    let order_lines = match &order {
        Some(o) => db::list_order_lines_for_update(&mut *conn, o.id).await?,
        None => Vec::new(),
    };

    let src = Source {
        doc_type: GOODS_RECEIPT,
        line_type: GOODS_RECEIPT_LINE,
        doc_id: doc.record_id,
        txn_date: today_of(doc.now),
        actor: doc.actor,
    };
    let mut deltas: BTreeMap<Id, Decimal> = BTreeMap::new();
    for (index, row) in rows.iter().enumerate() {
        if let Some(lot_id) = row.lot_id {
            inv.remove_lot(conn, &src, line_of(index, row)?, lot_id)
                .await?;
        }
        if let Some(id) = row.purchase_order_line_id {
            *deltas.entry(id).or_default() -= row.qty;
        }
    }
    inv.finish()?;
    if let Some(order) = &order {
        apply_received(conn, doc, order, &order_lines, &deltas).await?;
    }
    Ok(())
}

/// 採購明細的 `received_qty` 加上 `deltas`，重新計算採購單的狀態，採購單 `version` + 1 並寫入操作紀錄
/// （`update`，diff 為 `status` 與 `lines.purchase_order_line` 的 `received_qty`）。
async fn apply_received(
    conn: &mut PgConnection,
    doc: &DocumentContext<'_>,
    order: &OrderForReceipt,
    order_lines: &[OrderLineRow],
    deltas: &BTreeMap<Id, Decimal>,
) -> Result<(), CoreError> {
    if deltas.is_empty() {
        return Ok(());
    }
    let mut after_lines = Vec::with_capacity(order_lines.len());
    for line in order_lines {
        let delta = deltas.get(&line.id).copied().unwrap_or_default();
        if delta != Decimal::ZERO
            && !db::add_received_qty(&mut *conn, line.id, delta, doc.actor).await?
        {
            return Err(CoreError::Internal(format!(
                "採購明細 {} 已不存在",
                line.id
            )));
        }
        after_lines.push((line, qty4(line.received_qty + delta)));
    }
    let status = if order.status == DocumentStatus::Void.as_str() {
        DocumentStatus::Void
    } else {
        let pairs: Vec<(Decimal, Decimal)> = after_lines.iter().map(|(l, r)| (l.qty, *r)).collect();
        order_status(order.closed_manually, &pairs)
    };
    if !db::set_order_status(&mut *conn, order.id, status.as_str(), doc.actor).await? {
        return Err(CoreError::Internal(format!("採購單 {} 已不存在", order.id)));
    }

    let key = audit_key(PURCHASE_ORDER_LINE);
    let lines_value = |received: &dyn Fn(&OrderLineRow) -> Decimal| {
        Value::Array(
            order_lines
                .iter()
                .map(|l| json!({ "id": l.id, "data": { RECEIVED_QTY: qty4(received(l)).to_string() } }))
                .collect(),
        )
    };
    let before = json!({
        "status": order.status,
        key.clone(): lines_value(&|l| l.received_qty),
    });
    let after = json!({
        "status": status.as_str(),
        key: lines_value(&|l| l.received_qty + deltas.get(&l.id).copied().unwrap_or_default()),
    });
    crate::audit::record(
        conn,
        &AuditEntry {
            user_id: Some(doc.actor),
            action: action::UPDATE,
            entity_key: Some(PURCHASE_ORDER),
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

    impl ReceiptReferences for Refs {
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
            product_id == id(1) && lot_no.eq_ignore_ascii_case("OLD")
        }
    }

    fn order(status: &str) -> OrderForReceipt {
        OrderForReceipt {
            id: id(100),
            record_no: "PO202611-0001".into(),
            status: status.into(),
            supplier_id: id(50),
            currency_code: "USD".into(),
            tax_type: "zero_rated".into(),
            closed_manually: false,
            version: 2,
        }
    }

    fn order_line(n: u128, product: u128, qty: &str, received: &str) -> OrderLineRow {
        OrderLineRow {
            id: id(n),
            sort_order: 0,
            product_id: id(product),
            qty: d(qty),
            unit_price: d("1"),
            received_qty: d(received),
        }
    }

    fn line(po_line: Option<u128>, product: u128, qty: &str, lot: Option<&str>) -> ReceiptLine {
        ReceiptLine {
            purchase_order_line_id: po_line.map(id),
            product_id: Some(id(product)),
            qty: Some(d(qty)),
            lot_no: lot.map(str::to_owned),
        }
    }

    fn header(order: bool) -> ReceiptHeader {
        ReceiptHeader {
            purchase_order_id: order.then(|| id(100)),
            supplier_id: Some(id(50)),
            currency_code: Some("USD".into()),
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
            (0, line(None, 1, "1", Some("OLD"))),
            (1, line(None, 1, "1", Some("L-1"))),
            (2, line(None, 1, "1", Some("l-1"))),
            (3, line(None, 2, "1", Some("L-1"))),
            (4, line(None, 3, "1", Some("X"))),
            (5, line(None, 4, "1", None)),
            (6, line(None, 1, "1", None)),
            (7, line(None, 1, "1", None)),
        ];
        let (fields, errors) = check_receipt(&header(false), None, &lines, &Refs);
        assert!(fields.is_empty());
        assert_eq!(
            messages(&errors),
            [
                (Some(0), LOT_NO_TAKEN.to_owned()),
                (Some(2), MSG_LOT_DUPLICATE.to_owned()),
                (Some(4), MSG_SERVICE_LOT.to_owned()),
                (Some(5), MSG_PRODUCT_DELETED.to_owned()),
            ]
        );
    }

    #[test]
    fn order_line_requires_order() {
        let lines = vec![(0, line(Some(9), 1, "1", None))];
        let (_, errors) = check_receipt(&header(false), None, &lines, &Refs);
        assert_eq!(messages(&errors), [(Some(0), MSG_NO_ORDER.to_owned())]);
        assert_eq!(errors[0].field_key.as_deref(), Some(PURCHASE_ORDER_LINE_ID));
    }

    #[test]
    fn order_header_must_match() {
        let order = order("confirmed");
        let refs = OrderRefs {
            order: &order,
            lines: &[],
        };
        let mut h = header(true);
        h.supplier_id = Some(id(51));
        h.currency_code = Some("JPY".into());
        let (fields, _) = check_receipt(&h, Some(refs), &[], &Refs);
        let keys: Vec<&str> = fields.iter().map(|f| f.field_key.as_str()).collect();
        assert_eq!(keys, [SUPPLIER_ID, CURRENCY_CODE]);

        for (status, message) in [
            ("draft", Some(MSG_ORDER_DRAFT)),
            ("confirmed", None),
            ("partially_received", None),
            ("closed", Some(MSG_ORDER_CLOSED)),
            ("void", Some(MSG_ORDER_VOID)),
        ] {
            let order = self::order(status);
            let refs = OrderRefs {
                order: &order,
                lines: &[],
            };
            let (fields, _) = check_receipt(&header(true), Some(refs), &[], &Refs);
            assert_eq!(
                fields.first().map(|f| f.message.as_str()),
                message,
                "{status}"
            );
        }
    }

    #[test]
    fn order_lines_must_match_and_not_over_receive() {
        let order = order("partially_received");
        let order_lines = [order_line(10, 1, "10", "7"), order_line(11, 2, "5", "0")];
        let refs = OrderRefs {
            order: &order,
            lines: &order_lines,
        };
        let lines = vec![
            // 10：未進貨 3，兩列合計 4 → 第一列錯誤
            (0, line(Some(10), 1, "2", None)),
            (1, line(Some(10), 1, "2", None)),
            // 11：商品不同
            (2, line(Some(11), 1, "1", None)),
            // 不是此採購單的明細
            (3, line(Some(99), 1, "1", None)),
            // 沒有採購明細的贈品
            (4, line(None, 2, "100", None)),
        ];
        let (fields, errors) = check_receipt(&header(true), Some(refs), &lines, &Refs);
        assert!(fields.is_empty());
        assert_eq!(
            messages(&errors),
            [
                (Some(0), "超過未進貨數量 3.0000".to_owned()),
                (Some(2), MSG_PRODUCT_MISMATCH.to_owned()),
                (Some(3), MSG_NOT_ORDER_LINE.to_owned()),
            ]
        );

        // 剛好等於未進貨數量可以
        let lines = vec![(0, line(Some(10), 1, "3", None))];
        let (_, errors) = check_receipt(&header(true), Some(refs), &lines, &Refs);
        assert!(errors.is_empty());
    }

    #[test]
    fn order_status_is_recalculated() {
        use DocumentStatus::*;
        let l = |q: &str, r: &str| (d(q), d(r));
        assert_eq!(order_status(false, &[l("10", "0"), l("5", "0")]), Confirmed);
        assert_eq!(
            order_status(false, &[l("10", "3"), l("5", "0")]),
            PartiallyReceived
        );
        assert_eq!(order_status(false, &[l("10", "10"), l("5", "5")]), Closed);
        // 超收（沒有採購明細的另外輸入，這裡只是保險）也算結案
        assert_eq!(order_status(false, &[l("10", "12")]), Closed);
        // 手動結案維持結案（作廢進貨後也是）
        assert_eq!(order_status(true, &[l("10", "0")]), Closed);
    }
}
