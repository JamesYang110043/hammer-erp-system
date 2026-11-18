//! 盤點與調整單（系統單據 `stock_adjustment`，docs/architecture.md §8.10、§9.3）：存檔時的檢查，
//! 以及過帳、作廢的庫存處理（[`StockAdjustmentHandler`]，掛在 [`crate::documents`] 的框架上）。
//!
//! # 存檔時的檢查（草稿的新增與修改）
//!
//! 由 [`crate::records`] 呼叫，錯誤一律 `422 VALIDATION`（明細的錯誤為 `details.lines`）：
//!
//! - [`check_line_data`]（每一列，純邏輯）：批號去除前後空白（空字串視為沒有值）、不可有控制字元；
//!   `qty > 0`；`gain_new` 的 `unit_cost_twd` 必填；`gain_existing`、`loss` 的 `lot_no` 必填、
//!   `unit_cost_twd` 不可填
//! - [`check_save`]（所有明細，查詢資料庫）：商品必須是 `goods`；`gain_new` 的批號不可與該商品未刪除的
//!   批號重複；`gain_existing`、`loss` 的批號必須是該商品未刪除的批號；同一張單據中同一商品的同一批號
//!   只能出現一次；期初庫存只能使用 `gain_new`（規則本身在 [`check_lines`]）
//!
//! 存檔時不檢查庫存是否足夠（過帳時檢查），草稿可以先輸入盤虧。
//!
//! # 過帳
//!
//! 1. 單據已由 [`crate::documents`] 鎖定，`version`、狀態已檢查
//! 2. 鎖定明細；依 §8.10「鎖定順序」鎖定商品（`FOR SHARE`）與指定的批號（`FOR UPDATE`）
//! 3. 依鎖定後的資料重新執行全部存檔時的檢查，並檢查至少有一列明細
//! 4. 依 `sort_order` 處理每一列（[`crate::inventory`]）：
//!    - `gain_new`：建立批號（批號留空時自動產生），異動 `adjustment`、`+qty`
//!    - `gain_existing`：加回既有批號，異動 `adjustment`、`+qty`；明細寫入批號的成本
//!    - `loss`：從指定批號扣除（批號扣除明細），異動 `adjustment`、`−qty`；明細寫入批號的成本。不足時
//!      記下並繼續檢查其他列
//!    - 明細寫入 `lot_id`（與自動產生的 `lot_no`），有變動時明細的 `version` + 1
//! 5. 有任何不足 → `422 STOCK_INSUFFICIENT`，列出所有不足，整張 rollback
//!
//! # 作廢
//!
//! 異動一律為 `void_reversal`，`txn_date` 為作廢當天（Asia/Taipei）：
//!
//! - `gain_new`：批號沒有其他單據的有效異動時，新增 `−qty_remaining` 的異動並軟刪除批號；被使用時
//!   `422 LOT_IN_USE`
//! - `gain_existing`：批號的數量還在（`qty_remaining ≥ qty`）時減回；不足時 `422 STOCK_INSUFFICIENT`
//! - `loss`：依批號扣除明細把數量加回原批號
//!
//! 兩種錯誤都有時回傳 `LOT_IN_USE`；有錯誤時整張 rollback。

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use crate::formulas::today_of;
use hammer_db::inventory as inventory_db;
use hammer_db::sqlx::PgConnection;
use hammer_db::stock_adjustment::{self as db, AdjustmentLineRow};
use hammer_shared::Id;
use hammer_shared::decimal::Decimal;
use hammer_shared::inventory::TxnType;
use hammer_shared::schema::{DataType, FieldValue, parse_value};
use serde_json::{Map, Value};
use time::Date;

use crate::CoreError;
use crate::documents::{DocumentContext, DocumentHandler, HandlerFuture, STOCK_ADJUSTMENT};
use crate::inventory::{
    Inventory, LOT_NO_TAKEN, Line, LockRequest, Receipt, Source, allocations_of_line,
    normalize_lot_no,
};
use crate::master_data::MSG_REQUIRED;
use crate::record_data::FieldError;
use crate::record_lines::LineError;

/// 盤點調整明細的明細子表代碼。
pub const STOCK_ADJUSTMENT_LINE: &str = "stock_adjustment_line";

/// 調整類型：期初庫存。
pub const OPENING: &str = "opening";

pub const MSG_SERVICE_PRODUCT: &str = "服務類商品沒有庫存";
pub const MSG_PRODUCT_DELETED: &str = "商品已刪除";
pub const MSG_QTY_POSITIVE: &str = "數量必須大於 0";
pub const MSG_LOT_NOT_FOUND: &str = "找不到此批號";
pub const MSG_COST_FIXED: &str = "既有批號的成本不可修改";
pub const MSG_LOT_DUPLICATE: &str = "批號重複";
pub const MSG_OPENING_GAIN_NEW: &str = "期初庫存只能新增批號";
pub const MSG_NO_LINES: &str = "至少要有一筆明細";

const PRODUCT_ID: &str = "product_id";
const LINE_TYPE: &str = "line_type";
const LOT_NO: &str = "lot_no";
const QTY: &str = "qty";
const UNIT_COST: &str = "unit_cost_twd";
const ADJUSTMENT_DATE: &str = "adjustment_date";
const ADJUSTMENT_TYPE: &str = "adjustment_type";

/// 明細的調整方式（`line_type`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineType {
    /// 盤盈：新批號（也用於期初庫存）。
    GainNew,
    /// 盤盈：加回既有批號。
    GainExisting,
    /// 盤虧：扣指定批號。
    Loss,
}

impl LineType {
    /// 資料庫與 API 中的值。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GainNew => "gain_new",
            Self::GainExisting => "gain_existing",
            Self::Loss => "loss",
        }
    }

    /// 解析 `line_type`；不是有效的值時為 `None`。
    pub fn parse(s: &str) -> Option<Self> {
        [Self::GainNew, Self::GainExisting, Self::Loss]
            .into_iter()
            .find(|t| t.as_str() == s)
    }
}

/// 一列明細中檢查用到的系統欄位。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AdjustmentLine {
    pub product_id: Option<Id>,
    /// `None`：值不合法（欄位驗證已擋下，不做依調整方式的檢查）。
    pub line_type: Option<LineType>,
    /// 去除前後空白後的批號；空字串為 `None`。
    pub lot_no: Option<String>,
    pub qty: Option<Decimal>,
    pub unit_cost_twd: Option<Decimal>,
}

fn decimal_of(v: Option<&Value>) -> Option<Decimal> {
    match v? {
        Value::String(s) => Decimal::from_str(s).ok(),
        Value::Number(n) => Decimal::from_str(&n.to_string()).ok(),
        _ => None,
    }
}

impl AdjustmentLine {
    /// 從明細的 `data`（正規化後）取出。沒有 `line_type` 時為 `gain_new`（欄位的預設值）。
    pub fn from_data(data: &Map<String, Value>) -> Self {
        let line_type = match data.get(LINE_TYPE) {
            None | Some(Value::Null) => Some(LineType::GainNew),
            Some(v) => v.as_str().and_then(LineType::parse),
        };
        Self {
            product_id: data
                .get(PRODUCT_ID)
                .and_then(Value::as_str)
                .and_then(|s| Id::from_str(s).ok()),
            line_type,
            lot_no: data
                .get(LOT_NO)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            qty: decimal_of(data.get(QTY)),
            unit_cost_twd: decimal_of(data.get(UNIT_COST)),
        }
    }

    fn from_row(row: &AdjustmentLineRow) -> Self {
        Self {
            product_id: Some(row.product_id),
            line_type: LineType::parse(&row.line_type),
            lot_no: row
                .lot_no
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            qty: Some(row.qty),
            unit_cost_twd: row.unit_cost_twd,
        }
    }

    /// 這一列指定（`gain_existing`、`loss`）或新建（`gain_new` 有輸入批號）的批號。
    fn lot_key(&self) -> Option<(Id, String)> {
        Some((self.product_id?, self.lot_no.as_ref()?.to_lowercase()))
    }
}

/// 一列明細本身的檢查（不需要查詢資料庫），回傳欄位錯誤。
pub fn line_field_errors(line: &AdjustmentLine) -> Vec<FieldError> {
    let mut errors = Vec::new();
    let mut error = |key: &str, message: &str| {
        errors.push(FieldError {
            field_key: key.to_owned(),
            message: message.to_owned(),
        });
    };
    if let Some(lot_no) = &line.lot_no
        && let Err(message) = normalize_lot_no(lot_no)
    {
        error(LOT_NO, &message);
    }
    if line.qty.is_some_and(|q| q <= Decimal::ZERO) {
        error(QTY, MSG_QTY_POSITIVE);
    }
    match line.line_type {
        Some(LineType::GainNew) => {
            if line.unit_cost_twd.is_none() {
                error(UNIT_COST, MSG_REQUIRED);
            }
        }
        Some(LineType::GainExisting | LineType::Loss) => {
            if line.lot_no.is_none() {
                error(LOT_NO, MSG_REQUIRED);
            }
            if line.unit_cost_twd.is_some() {
                error(UNIT_COST, MSG_COST_FIXED);
            }
        }
        None => {}
    }
    errors
}

/// 存檔時每一列明細的檢查：批號去除前後空白後寫回 `data`（空字串時移除），再做 [`line_field_errors`]。
pub fn check_line_data(data: &mut Map<String, Value>) -> Vec<FieldError> {
    if let Some(Value::String(raw)) = data.get(LOT_NO) {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            data.remove(LOT_NO);
        } else if trimmed.len() != raw.len() {
            let trimmed = trimmed.to_owned();
            data.insert(LOT_NO.to_owned(), Value::String(trimmed));
        }
    }
    line_field_errors(&AdjustmentLine::from_data(data))
}

/// 明細引用的商品。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProductInfo {
    pub is_goods: bool,
    pub deleted: bool,
}

/// [`check_lines`] 需要的資料庫內容。
pub trait References {
    /// 商品；不存在時為 `None`。
    fn product(&self, id: Id) -> Option<ProductInfo>;
    /// 商品 `product_id` 是否有未刪除、批號為 `lot_no`（不分大小寫）的批號。
    fn lot_exists(&self, product_id: Id, lot_no: &str) -> bool;
}

/// 所有明細（`(錯誤中的位置, 明細)`，依順序）之間、以及與資料庫內容的檢查，回傳所有錯誤：
///
/// - 商品必須存在、未刪除，且是 `goods`
/// - `gain_new` 有輸入批號時，不可與該商品未刪除的批號重複
/// - `gain_existing`、`loss` 的批號必須是該商品未刪除的批號
/// - 同一商品的同一批號（不分大小寫）只能出現一次；第二列起回傳錯誤（`gain_new` 留空的不比對）
/// - `adjustment_type = opening` 時只能使用 `gain_new`
///
/// 有欄位錯誤（[`line_field_errors`]）的列也一併檢查。
pub fn check_lines(
    adjustment_type: Option<&str>,
    lines: &[(usize, &AdjustmentLine)],
    refs: &impl References,
) -> Vec<LineError> {
    let mut errors = Vec::new();
    let mut seen: BTreeSet<(Id, String)> = BTreeSet::new();
    let opening = adjustment_type == Some(OPENING);
    for &(index, line) in lines {
        let err =
            |key: &str, message: &str| LineError::field(STOCK_ADJUSTMENT_LINE, index, key, message);
        if opening && line.line_type.is_some_and(|t| t != LineType::GainNew) {
            errors.push(err(LINE_TYPE, MSG_OPENING_GAIN_NEW));
        }
        let Some(product_id) = line.product_id else {
            continue;
        };
        match refs.product(product_id) {
            None => {
                errors.push(err(PRODUCT_ID, MSG_PRODUCT_DELETED));
                continue;
            }
            Some(p) if p.deleted => {
                errors.push(err(PRODUCT_ID, MSG_PRODUCT_DELETED));
                continue;
            }
            Some(p) if !p.is_goods => {
                errors.push(err(PRODUCT_ID, MSG_SERVICE_PRODUCT));
                continue;
            }
            Some(_) => {}
        }
        let Some(key) = line.lot_key() else {
            continue;
        };
        if !seen.insert(key) {
            errors.push(err(LOT_NO, MSG_LOT_DUPLICATE));
            continue;
        }
        let Some(lot_no) = &line.lot_no else {
            continue;
        };
        let exists = refs.lot_exists(product_id, lot_no);
        match line.line_type {
            Some(LineType::GainNew) if exists => errors.push(err(LOT_NO, LOT_NO_TAKEN)),
            Some(LineType::GainExisting | LineType::Loss) if !exists => {
                errors.push(err(LOT_NO, MSG_LOT_NOT_FOUND));
            }
            _ => {}
        }
    }
    errors
}

/// 存檔時查詢的資料庫內容（不鎖定）。
struct SavedReferences {
    products: BTreeMap<Id, ProductInfo>,
    lots: BTreeSet<(Id, String)>,
}

impl References for SavedReferences {
    fn product(&self, id: Id) -> Option<ProductInfo> {
        self.products.get(&id).copied()
    }

    fn lot_exists(&self, product_id: Id, lot_no: &str) -> bool {
        self.lots.contains(&(product_id, lot_no.to_lowercase()))
    }
}

/// 存檔時的檢查（[`check_lines`]）：`adjustment_type` 為主表要寫入的值，`lines` 為要寫入的所有明細
/// （`(在 request 陣列中的位置, 正規化後的 data)`，只含通過欄位驗證的列）。
pub(crate) async fn check_save(
    conn: &mut PgConnection,
    adjustment_type: Option<&str>,
    lines: &[(usize, &Map<String, Value>)],
) -> Result<Vec<LineError>, CoreError> {
    let parsed: Vec<(usize, AdjustmentLine)> = lines
        .iter()
        .map(|(i, data)| (*i, AdjustmentLine::from_data(data)))
        .collect();
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
            (
                r.id,
                ProductInfo {
                    is_goods: r.product_type == crate::inventory::GOODS,
                    deleted: r.deleted,
                },
            )
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
    let refs_lines: Vec<(usize, &AdjustmentLine)> = parsed.iter().map(|(i, l)| (*i, l)).collect();
    Ok(check_lines(adjustment_type, &refs_lines, &refs))
}

/// 過帳時以鎖定的商品與批號為準。
impl References for Inventory {
    fn product(&self, id: Id) -> Option<ProductInfo> {
        Inventory::product(self, id).map(|p| ProductInfo {
            is_goods: p.is_goods,
            deleted: p.deleted,
        })
    }

    fn lot_exists(&self, product_id: Id, lot_no: &str) -> bool {
        self.lot_by_no(product_id, lot_no).is_some()
    }
}

/// 盤點調整單的過帳、作廢處理函式（[`crate::documents::DocumentHandlers::standard`] 註冊）。
#[derive(Debug, Default, Clone, Copy)]
pub struct StockAdjustmentHandler;

impl DocumentHandler for StockAdjustmentHandler {
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

fn adjustment_date(doc: &DocumentContext<'_>) -> Result<Date, CoreError> {
    let value = doc.data.get(ADJUSTMENT_DATE).unwrap_or(&Value::Null);
    match parse_value(DataType::Date, value) {
        Ok(Some(FieldValue::Date(d))) => Ok(d),
        _ => Err(CoreError::Internal(format!(
            "單據 {} 的調整日期不正確",
            doc.record_no
        ))),
    }
}

fn line_of(index: usize, row: &AdjustmentLineRow) -> Result<Line, CoreError> {
    Ok(Line {
        id: row.id,
        index: u32::try_from(index).map_err(|_| CoreError::Internal("明細的列數過多".into()))?,
    })
}

fn invalid_lines(lines: Vec<LineError>) -> CoreError {
    CoreError::InvalidLines {
        fields: Vec::new(),
        lines,
    }
}

async fn post(conn: &mut PgConnection, doc: &DocumentContext<'_>) -> Result<(), CoreError> {
    let rows = db::list_lines_for_update(&mut *conn, doc.record_id).await?;
    if rows.is_empty() {
        return Err(invalid_lines(vec![LineError::detail(
            STOCK_ADJUSTMENT_LINE,
            MSG_NO_LINES,
        )]));
    }
    let lines: Vec<AdjustmentLine> = rows.iter().map(AdjustmentLine::from_row).collect();

    // 鎖定順序：商品（FOR SHARE）→ 指定的批號（FOR UPDATE）；gain_new 的批號一起查詢以檢查重複
    let req = LockRequest {
        product_ids: rows.iter().map(|r| r.product_id).collect(),
        lot_numbers: lines
            .iter()
            .filter_map(|l| Some((l.product_id?, l.lot_no.clone()?)))
            .collect(),
        ..LockRequest::default()
    };
    let mut inv = Inventory::lock(conn, &req).await?;

    // 依鎖定後的資料重新執行存檔時的檢查
    let mut errors = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        errors.extend(LineError::from_fields(
            STOCK_ADJUSTMENT_LINE,
            index,
            line_field_errors(line),
        ));
    }
    let adjustment_type = doc.data.get(ADJUSTMENT_TYPE).and_then(Value::as_str);
    let indexed: Vec<(usize, &AdjustmentLine)> = lines.iter().enumerate().collect();
    errors.extend(check_lines(adjustment_type, &indexed, &inv));
    if !errors.is_empty() {
        errors.sort_by_key(|e| e.index);
        return Err(invalid_lines(errors));
    }

    let src = Source {
        doc_type: STOCK_ADJUSTMENT,
        line_type: STOCK_ADJUSTMENT_LINE,
        doc_id: doc.record_id,
        txn_date: adjustment_date(doc)?,
        actor: doc.actor,
    };
    for (index, (row, line)) in rows.iter().zip(&lines).enumerate() {
        let at = line_of(index, row)?;
        let qty = row.qty;
        let (lot_no, unit_cost, lot_id) = match line.line_type {
            Some(LineType::GainNew) => {
                let unit_cost = row
                    .unit_cost_twd
                    .ok_or_else(|| CoreError::Internal("盤盈明細沒有單位成本".into()))?;
                let receipt = Receipt {
                    product_id: row.product_id,
                    lot_no: line.lot_no.as_deref(),
                    record_no: doc.record_no,
                    sort_order: row.sort_order,
                    received_date: src.txn_date,
                    qty,
                    unit_cost_twd: unit_cost,
                    txn_type: TxnType::Adjustment,
                };
                let lot = match inv.receive(conn, &src, at, &receipt).await {
                    Ok(lot) => lot,
                    // 自動產生的批號與既有的批號重複（唯一索引擋下）
                    Err(CoreError::Validation(m)) if m == LOT_NO_TAKEN => {
                        return Err(invalid_lines(vec![LineError::field(
                            STOCK_ADJUSTMENT_LINE,
                            index,
                            LOT_NO,
                            LOT_NO_TAKEN,
                        )]));
                    }
                    Err(e) => return Err(e),
                };
                let lot = lot.ok_or_else(|| CoreError::Internal("服務類商品沒有批號".into()))?;
                (Some(lot.lot_no), Some(unit_cost), lot.id)
            }
            Some(LineType::GainExisting | LineType::Loss) => {
                let lot = line
                    .lot_no
                    .as_deref()
                    .and_then(|no| inv.lot_by_no(row.product_id, no))
                    .cloned()
                    .ok_or_else(|| CoreError::Internal("指定的批號沒有先鎖定".into()))?;
                if line.line_type == Some(LineType::GainExisting) {
                    inv.add_to_lot(
                        conn,
                        &src,
                        at,
                        row.product_id,
                        lot.id,
                        qty,
                        None,
                        TxnType::Adjustment,
                    )
                    .await?;
                } else {
                    // 不足時記下，繼續處理其他列；由 finish 回傳所有不足
                    inv.issue_from_lot(
                        conn,
                        &src,
                        at,
                        row.product_id,
                        lot.id,
                        qty,
                        TxnType::Adjustment,
                    )
                    .await?;
                }
                (row.lot_no.clone(), Some(lot.unit_cost_twd), lot.id)
            }
            None => {
                return Err(CoreError::Internal(format!(
                    "明細的調整方式不正確：{}",
                    row.line_type
                )));
            }
        };
        if lot_no != row.lot_no || unit_cost != row.unit_cost_twd || Some(lot_id) != row.lot_id {
            let updated = db::update_line_posting(
                &mut *conn,
                row.id,
                lot_no.as_deref(),
                unit_cost,
                Some(lot_id),
                Some(doc.actor),
            )
            .await?;
            if !updated {
                return Err(CoreError::Internal(format!("明細 {} 已不存在", row.id)));
            }
        }
    }
    inv.finish()
}

async fn void(conn: &mut PgConnection, doc: &DocumentContext<'_>) -> Result<(), CoreError> {
    let rows = db::list_lines_for_update(&mut *conn, doc.record_id).await?;
    // 鎖定前先讀取盤虧的批號扣除明細，把原批號一起鎖定
    let mut allocations = Vec::with_capacity(rows.len());
    for row in &rows {
        let list = if row.line_type == LineType::Loss.as_str() {
            allocations_of_line(conn, STOCK_ADJUSTMENT_LINE, row.id).await?
        } else {
            Vec::new()
        };
        allocations.push(list);
    }
    let req = LockRequest {
        product_ids: rows.iter().map(|r| r.product_id).collect(),
        lot_ids: rows
            .iter()
            .filter_map(|r| r.lot_id)
            .chain(allocations.iter().flatten().map(|a| a.lot_id))
            .collect(),
        ..LockRequest::default()
    };
    let mut inv = Inventory::lock(conn, &req).await?;

    let src = Source {
        doc_type: STOCK_ADJUSTMENT,
        line_type: STOCK_ADJUSTMENT_LINE,
        doc_id: doc.record_id,
        txn_date: today_of(doc.now),
        actor: doc.actor,
    };
    for (index, (row, allocs)) in rows.iter().zip(&allocations).enumerate() {
        let at = line_of(index, row)?;
        let lot_id = row
            .lot_id
            .ok_or_else(|| CoreError::Internal(format!("已過帳的明細 {} 沒有批號", row.id)))?;
        match LineType::parse(&row.line_type) {
            Some(LineType::GainNew) => {
                inv.remove_lot(conn, &src, at, lot_id).await?;
            }
            Some(LineType::GainExisting) => {
                inv.reduce_lot(
                    conn,
                    &src,
                    at,
                    row.product_id,
                    lot_id,
                    row.qty,
                    TxnType::VoidReversal,
                )
                .await?;
            }
            Some(LineType::Loss) => {
                inv.reverse_allocations(conn, &src, at, allocs).await?;
            }
            None => {
                return Err(CoreError::Internal(format!(
                    "明細的調整方式不正確：{}",
                    row.line_type
                )));
            }
        }
    }
    inv.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn id(n: u128) -> Id {
        Id::from_u128(n)
    }

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn line(t: LineType, product: u128, lot: Option<&str>) -> AdjustmentLine {
        AdjustmentLine {
            product_id: Some(id(product)),
            line_type: Some(t),
            lot_no: lot.map(str::to_owned),
            qty: Some(d("1")),
            unit_cost_twd: (t == LineType::GainNew).then(|| d("10")),
        }
    }

    struct Refs {
        products: BTreeMap<Id, ProductInfo>,
        lots: Vec<(Id, &'static str)>,
    }

    impl References for Refs {
        fn product(&self, id: Id) -> Option<ProductInfo> {
            self.products.get(&id).copied()
        }
        fn lot_exists(&self, product_id: Id, lot_no: &str) -> bool {
            self.lots
                .iter()
                .any(|(p, n)| *p == product_id && n.eq_ignore_ascii_case(lot_no))
        }
    }

    fn refs() -> Refs {
        let goods = ProductInfo {
            is_goods: true,
            deleted: false,
        };
        Refs {
            products: BTreeMap::from([
                (id(1), goods),
                (id(2), goods),
                (
                    id(3),
                    ProductInfo {
                        is_goods: false,
                        deleted: false,
                    },
                ),
                (
                    id(4),
                    ProductInfo {
                        is_goods: true,
                        deleted: true,
                    },
                ),
            ]),
            lots: vec![(id(1), "L-01"), (id(2), "L-01")],
        }
    }

    fn errors(adjustment_type: &str, lines: &[AdjustmentLine]) -> Vec<(usize, String, String)> {
        let indexed: Vec<(usize, &AdjustmentLine)> = lines.iter().enumerate().collect();
        check_lines(Some(adjustment_type), &indexed, &refs())
            .into_iter()
            .map(|e| (e.index.unwrap(), e.field_key.unwrap(), e.message))
            .collect()
    }

    fn e(index: usize, key: &str, message: &str) -> (usize, String, String) {
        (index, key.to_owned(), message.to_owned())
    }

    #[test]
    fn line_type_round_trips() {
        for t in [LineType::GainNew, LineType::GainExisting, LineType::Loss] {
            assert_eq!(LineType::parse(t.as_str()), Some(t));
        }
        assert_eq!(LineType::parse("gain"), None);
    }

    #[test]
    fn line_from_data_trims_lot_no_and_defaults_line_type() {
        let data = json!({ "product_id": id(1).to_string(), "lot_no": "  ", "qty": "2.5" });
        let line = AdjustmentLine::from_data(data.as_object().unwrap());
        assert_eq!(line.product_id, Some(id(1)));
        assert_eq!(line.line_type, Some(LineType::GainNew));
        assert_eq!(line.lot_no, None);
        assert_eq!(line.qty, Some(d("2.5")));
        assert_eq!(line.unit_cost_twd, None);
        let data = json!({ "line_type": "loss", "lot_no": " L-01 ", "unit_cost_twd": 3 });
        let line = AdjustmentLine::from_data(data.as_object().unwrap());
        assert_eq!(line.line_type, Some(LineType::Loss));
        assert_eq!(line.lot_no.as_deref(), Some("L-01"));
        assert_eq!(line.unit_cost_twd, Some(d("3")));
    }

    #[test]
    fn check_line_data_writes_trimmed_lot_no() {
        let mut data = json!({ "lot_no": " L-01 ", "qty": "1", "unit_cost_twd": "1" });
        let data = data.as_object_mut().unwrap();
        assert!(check_line_data(data).is_empty());
        assert_eq!(data["lot_no"], json!("L-01"));
        let mut blank = json!({ "lot_no": "   ", "qty": "1", "unit_cost_twd": "1" });
        let blank = blank.as_object_mut().unwrap();
        assert!(check_line_data(blank).is_empty());
        assert!(!blank.contains_key("lot_no"));
    }

    fn field_errors(line: &AdjustmentLine) -> Vec<(String, String)> {
        line_field_errors(line)
            .into_iter()
            .map(|e| (e.field_key, e.message))
            .collect()
    }

    fn fe(key: &str, message: &str) -> (String, String) {
        (key.to_owned(), message.to_owned())
    }

    #[test]
    fn gain_new_requires_unit_cost() {
        assert!(field_errors(&line(LineType::GainNew, 1, None)).is_empty());
        let mut l = line(LineType::GainNew, 1, None);
        l.unit_cost_twd = None;
        assert_eq!(field_errors(&l), [fe("unit_cost_twd", MSG_REQUIRED)]);
    }

    #[test]
    fn existing_lot_lines_require_lot_no_without_cost() {
        for t in [LineType::GainExisting, LineType::Loss] {
            assert!(field_errors(&line(t, 1, Some("L-01"))).is_empty());
            let mut l = line(t, 1, None);
            l.unit_cost_twd = Some(d("5"));
            assert_eq!(
                field_errors(&l),
                [
                    fe("lot_no", MSG_REQUIRED),
                    fe("unit_cost_twd", MSG_COST_FIXED)
                ]
            );
        }
    }

    #[test]
    fn qty_must_be_positive_and_lot_no_valid() {
        let mut l = line(LineType::GainNew, 1, Some("L\u{0007}"));
        l.qty = Some(Decimal::ZERO);
        assert_eq!(
            field_errors(&l),
            [
                fe("lot_no", crate::inventory::LOT_NO_CONTROL_CHARS),
                fe("qty", MSG_QTY_POSITIVE)
            ]
        );
        l.qty = Some(d("-1"));
        l.lot_no = None;
        assert_eq!(field_errors(&l), [fe("qty", MSG_QTY_POSITIVE)]);
    }

    #[test]
    fn products_must_be_existing_goods() {
        let lines = [
            line(LineType::GainNew, 1, None),
            line(LineType::GainNew, 3, None),
            line(LineType::GainNew, 4, None),
            line(LineType::GainNew, 9, None),
        ];
        assert_eq!(
            errors("count", &lines),
            [
                e(1, "product_id", MSG_SERVICE_PRODUCT),
                e(2, "product_id", MSG_PRODUCT_DELETED),
                e(3, "product_id", MSG_PRODUCT_DELETED),
            ]
        );
    }

    #[test]
    fn lot_numbers_are_checked_against_existing_lots() {
        let lines = [
            line(LineType::GainNew, 1, Some("l-01")),
            line(LineType::GainNew, 1, Some("L-02")),
            line(LineType::GainExisting, 1, Some("L-03")),
            line(LineType::Loss, 2, Some("L-01")),
            line(LineType::Loss, 2, Some("L-09")),
        ];
        assert_eq!(
            errors("count", &lines),
            [
                e(0, "lot_no", LOT_NO_TAKEN),
                e(2, "lot_no", MSG_LOT_NOT_FOUND),
                e(4, "lot_no", MSG_LOT_NOT_FOUND),
            ]
        );
    }

    #[test]
    fn same_lot_only_once_per_document() {
        let lines = [
            line(LineType::Loss, 1, Some("L-01")),
            line(LineType::GainExisting, 1, Some("l-01")),
            // 不同商品的同一批號可以
            line(LineType::Loss, 2, Some("L-01")),
            // 留空的批號不比對
            line(LineType::GainNew, 1, None),
            line(LineType::GainNew, 1, None),
        ];
        assert_eq!(errors("count", &lines), [e(1, "lot_no", MSG_LOT_DUPLICATE)]);
    }

    #[test]
    fn opening_allows_only_gain_new() {
        let lines = [
            line(LineType::GainNew, 1, None),
            line(LineType::Loss, 1, Some("L-01")),
            line(LineType::GainExisting, 2, Some("L-01")),
        ];
        assert_eq!(
            errors(OPENING, &lines),
            [
                e(1, "line_type", MSG_OPENING_GAIN_NEW),
                e(2, "line_type", MSG_OPENING_GAIN_NEW),
            ]
        );
        assert!(errors("adjustment", &lines).is_empty());
    }
}
