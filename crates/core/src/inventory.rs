//! 庫存引擎（docs/architecture.md §8.10、§9.1–9.3）：批號、扣庫存、異動流水帳。
//!
//! 各單據的過帳、作廢（[`crate::documents`] 的處理函式）呼叫這裡，不直接寫入庫存資料表。所有函式都在
//! 呼叫端的 transaction 內執行；有任何錯誤時由呼叫端 rollback 整張單據。
//!
//! # 用法
//!
//! 1. 單據主表已由 [`crate::documents`] 鎖定（`FOR UPDATE`，鎖定順序第 1 步）
//! 2. [`Inventory::lock`]：依 §8.10「鎖定順序」鎖定商品（`FOR SHARE`，第 2 步）與批號（`FOR UPDATE`，
//!    第 3 步）。範圍由 [`LockRequest`] 指定：指定的批號（id 或商品 + 批號）、先進先出的商品
//! 3. 依明細順序呼叫異動函式；鎖定後才讀取 `qty_remaining`，數量在記憶體中依序更新
//!    - 入庫：[`Inventory::receive`]（建立批號；批號留空時用 [`auto_lot_no`] 自動編號）
//!    - 加回既有批號：[`Inventory::add_to_lot`]
//!    - 扣庫存：[`Inventory::issue_from_lot`]（手動指定批號）、[`Inventory::issue_fifo`]（先進先出）
//!    - 減少批號數量但不產生批號扣除明細：[`Inventory::reduce_lot`]（例如作廢盤盈）
//!    - 反向處理：[`Inventory::reverse_allocations`]（依批號扣除明細加回原批號）、
//!      [`Inventory::remove_lot`]（刪除單據建立且未被其他單據使用的批號）
//! 4. [`Inventory::finish`]：有任何不足 → [`CoreError::StockInsufficient`]；有批號被其他單據使用 →
//!    [`CoreError::LotInUse`]（兩者都有時回傳 `LotInUse`）
//!
//! 數量不足、批號被使用時**不中斷**，記下後繼續處理其他明細（該筆不寫入），讓錯誤能列出所有項目；
//! 呼叫端在 [`Inventory::finish`] 回傳錯誤後 rollback。
//!
//! # 規則
//!
//! - 只有 `product_type = goods` 的商品有庫存；`service` 的商品一律跳過（異動函式回傳 `None` 或空陣列，
//!   不寫入任何資料）
//! - 每一筆數量變動都寫入一筆 `inventory_transactions`；每一筆扣減都寫入一筆 `lot_allocations`
//!   （[`Inventory::reduce_lot`] 除外，它用於作廢時減回自己加過的數量）
//! - 先進先出依 `received_date, id` 由舊到新扣，可以跨多個批號
//! - 金額、數量一律使用 [`Decimal`]，不使用浮點數

use std::collections::{BTreeMap, BTreeSet};

use hammer_db::DbError;
use hammer_db::document as document_db;
use hammer_db::inventory::{
    self as db, AllocationRow, LOT_NO_UNIQUE, LOT_SOURCE_LINE_UNIQUE, LotRow, NewAllocation,
    NewLot, NewTransaction,
};
use hammer_db::schema as schema_db;
use hammer_db::sqlx::PgConnection;
use hammer_shared::decimal::Decimal;
use hammer_shared::document::{
    DocumentRef, LineRef, LotInUse, LotInUseDetails, StockInsufficientDetails, StockShortage,
};
use hammer_shared::inventory::{LOT_NO_MAX_CHARS, TxnType};
use hammer_shared::{Id, new_id};
use time::Date;

use crate::CoreError;

/// 商品類型：實體商品（有庫存）。
pub const GOODS: &str = "goods";

/// 數量與單位成本的小數位數（`NUMERIC(18,4)`）。
pub const QTY_SCALE: u32 = 4;

/// 批號已存在時的錯誤訊息（§8.10「批號」）。
pub const LOT_NO_TAKEN: &str = "批號已存在";
/// 批號不可有控制字元。
pub const LOT_NO_CONTROL_CHARS: &str = "批號不可包含控制字元";

/// 去除前後空白後的批號；空字串為 `None`（過帳時自動產生）。有控制字元或超過
/// [`LOT_NO_MAX_CHARS`] 個字元時回傳錯誤訊息。
pub fn normalize_lot_no(raw: &str) -> Result<Option<String>, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Ok(None);
    }
    if s.chars().any(char::is_control) {
        return Err(LOT_NO_CONTROL_CHARS.into());
    }
    if s.chars().count() > LOT_NO_MAX_CHARS {
        return Err(format!("批號最長 {LOT_NO_MAX_CHARS} 個字元"));
    }
    Ok(Some(s.to_owned()))
}

/// 自動產生的批號：`{單號}-{明細序號}`，明細序號為 `sort_order + 1`，至少 2 位、不足補零
/// （例如 `ADJ202611-0003-01`）。
pub fn auto_lot_no(record_no: &str, sort_order: i32) -> String {
    format!("{record_no}-{:02}", i64::from(sort_order) + 1)
}

/// 補齊 4 位小數（API 中數量、單位成本的格式，§8.10）。
pub fn qty4(value: Decimal) -> Decimal {
    let mut v = value.round_dp_with_strategy(
        QTY_SCALE,
        hammer_shared::decimal::RoundingStrategy::MidpointAwayFromZero,
    );
    v.rescale(QTY_SCALE);
    v
}

/// 顯示用的數量：去掉多餘的 0（`6.0000` → `6`、`1.5000` → `1.5`）。
fn display_qty(value: Decimal) -> String {
    value.normalize().to_string()
}

/// 先進先出的扣減計畫：依序從 `lots`（`(批號 id, 目前數量)`，已依先進先出排序）扣 `qty`。
/// 足夠時回傳每個批號扣除的數量（略過數量為 0 的批號）；不足時回傳可用的總數量。
///
/// 出貨過帳（[`Inventory::issue_fifo`]）與建議批號（[`preview_allocations`]）都以 [`take_fifo`] 選擇批號。
pub fn plan_fifo(lots: &[(Id, Decimal)], qty: Decimal) -> Result<Vec<(Id, Decimal)>, Decimal> {
    let (out, shortage) = take_fifo(lots, qty);
    if shortage > Decimal::ZERO {
        Err(lots.iter().map(|(_, q)| (*q).max(Decimal::ZERO)).sum())
    } else {
        Ok(out)
    }
}

/// 依序從 `lots`（已依先進先出排序）扣 `qty`，能扣多少扣多少：回傳每個批號扣除的數量（略過數量為 0 的
/// 批號）與不足的數量（足夠時為 0）。
pub fn take_fifo(lots: &[(Id, Decimal)], qty: Decimal) -> (Vec<(Id, Decimal)>, Decimal) {
    let mut remaining = qty;
    let mut out = Vec::new();
    for &(id, available) in lots {
        if remaining <= Decimal::ZERO {
            break;
        }
        if available <= Decimal::ZERO {
            continue;
        }
        let take = remaining.min(available);
        out.push((id, take));
        remaining -= take;
    }
    (out, remaining.max(Decimal::ZERO))
}

/// 建議批號的一列（[`preview_allocations`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewLine {
    pub product_id: Id,
    /// 是否為實體商品；`service` 不分配。
    pub is_goods: bool,
    pub qty: Decimal,
    /// 手動指定的批號；`None` 為先進先出。
    pub lot_id: Option<Id>,
}

/// 建議批號用的批號狀態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewLot {
    pub id: Id,
    pub product_id: Id,
    pub received_date: Date,
    pub qty_remaining: Decimal,
}

/// 依出貨過帳的順序試算分配（§8.9「出貨過帳與作廢」）：先依陣列順序扣有指定批號的列，再依陣列順序以
/// 先進先出（`received_date, id`）扣其他列，同一商品的多列依序接著扣。數量不足時扣到 0 為止，記下不足。
///
/// 回傳每一列 `(扣除的 (批號 id, 數量), 不足的數量)`，與 `lines` 的順序相同。`lots` 為相關商品的批號
/// （指定的批號與先進先出商品所有有庫存的批號）。
pub fn preview_allocations(
    lines: &[PreviewLine],
    lots: &[PreviewLot],
) -> Vec<(Vec<(Id, Decimal)>, Decimal)> {
    let mut remaining: BTreeMap<Id, Decimal> =
        lots.iter().map(|l| (l.id, l.qty_remaining)).collect();
    let mut fifo_order: Vec<&PreviewLot> = lots.iter().collect();
    fifo_order.sort_by_key(|l| (l.received_date, l.id));
    let mut out = vec![(Vec::new(), Decimal::ZERO); lines.len()];

    for (i, line) in lines.iter().enumerate() {
        let (true, Some(lot_id)) = (line.is_goods, line.lot_id) else {
            continue;
        };
        let available = remaining.get(&lot_id).copied().unwrap_or_default();
        let (taken, shortage) = take_fifo(&[(lot_id, available)], line.qty);
        for (id, q) in &taken {
            *remaining.entry(*id).or_default() -= *q;
        }
        out[i] = (taken, shortage);
    }
    for (i, line) in lines.iter().enumerate() {
        if !line.is_goods || line.lot_id.is_some() {
            continue;
        }
        let available: Vec<(Id, Decimal)> = fifo_order
            .iter()
            .filter(|l| l.product_id == line.product_id)
            .map(|l| (l.id, remaining.get(&l.id).copied().unwrap_or_default()))
            .collect();
        let (taken, shortage) = take_fifo(&available, line.qty);
        for (id, q) in &taken {
            *remaining.entry(*id).or_default() -= *q;
        }
        out[i] = (taken, shortage);
    }
    out
}

/// 異動的來源單據。
#[derive(Debug, Clone, Copy)]
pub struct Source<'a> {
    /// 來源單據的主表代碼（`ref_doc_type`，例如 `stock_adjustment`）。
    pub doc_type: &'a str,
    /// 來源明細子表代碼（`source_type`，例如 `stock_adjustment_line`）；也是錯誤中 `lines` 的
    /// `detail_key`。
    pub line_type: &'a str,
    /// 來源單據（主表）的 id。
    pub doc_id: Id,
    /// 異動的業務日期：過帳時為單據日期，作廢時為作廢當天（Asia/Taipei）。
    pub txn_date: Date,
    /// 操作者。
    pub actor: Id,
}

/// 異動的來源明細。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Line {
    /// 明細 id（`source_line_id`、`ref_line_id`）。
    pub id: Id,
    /// 依 `sort_order` 的位置（從 0 開始），用於錯誤的 `lines`。
    pub index: u32,
}

/// [`Inventory::lock`] 要鎖定的範圍。
#[derive(Debug, Clone, Default)]
pub struct LockRequest {
    /// 單據所有明細的商品（含 `service`；去除重複由這裡處理）。
    pub product_ids: Vec<Id>,
    /// 指定的批號 id（例如作廢時明細的 `lot_id`、批號扣除明細的 `lot_id`）。
    pub lot_ids: Vec<Id>,
    /// 依商品 + 批號（不分大小寫）指定的批號；找不到的忽略。
    pub lot_numbers: Vec<(Id, String)>,
    /// 先進先出模式的商品：鎖定其所有 `qty_remaining > 0` 的批號。
    pub fifo_product_ids: Vec<Id>,
}

/// 鎖定的商品。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Product {
    pub id: Id,
    pub code: String,
    pub name: String,
    /// 是否為實體商品（`goods`）。
    pub is_goods: bool,
    /// 是否已刪除。
    pub deleted: bool,
}

/// 鎖定的批號（數量為目前在這個 transaction 中的值）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lot {
    pub id: Id,
    pub product_id: Id,
    pub lot_no: String,
    pub received_date: Date,
    pub qty_in: Decimal,
    pub qty_remaining: Decimal,
    pub unit_cost_twd: Decimal,
    /// 是否已被本次操作刪除（[`Inventory::remove_lot`]）。
    pub removed: bool,
}

impl From<LotRow> for Lot {
    fn from(r: LotRow) -> Self {
        Self {
            id: r.id,
            product_id: r.product_id,
            lot_no: r.lot_no,
            received_date: r.received_date,
            qty_in: r.qty_in,
            qty_remaining: r.qty_remaining,
            unit_cost_twd: r.unit_cost_twd,
            removed: false,
        }
    }
}

/// 一筆扣減：從某個批號扣除的數量與當時的成本。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Allocation {
    pub lot_id: Id,
    pub qty: Decimal,
    pub unit_cost_twd: Decimal,
}

/// 入庫建立批號的內容。
#[derive(Debug, Clone)]
pub struct Receipt<'a> {
    pub product_id: Id,
    /// 去除空白後的批號；`None` 時以 [`auto_lot_no`] 產生。
    pub lot_no: Option<&'a str>,
    /// 單號與明細的 `sort_order`，自動產生批號時使用。
    pub record_no: &'a str,
    pub sort_order: i32,
    /// 入庫日期（來源單據的單據日期）。
    pub received_date: Date,
    pub qty: Decimal,
    pub unit_cost_twd: Decimal,
    pub txn_type: TxnType,
}

/// 不足的統計鍵：指定批號為批號，先進先出為商品。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ShortageKey {
    Lot(Id),
    Product(Id),
}

/// 一個批號（商品）在本次操作中的扣減需求。
#[derive(Debug, Default)]
struct Demand {
    /// 第一次出現的順序（錯誤依此排序）。
    order: usize,
    required: Decimal,
    /// 鎖定時的數量加上本次操作加入的數量。
    available: Decimal,
    short: bool,
    lines: Vec<LineRef>,
}

/// 被其他單據使用、不能刪除的批號。
#[derive(Debug)]
struct InUse {
    lot_id: Id,
    documents: Vec<DocumentRef>,
}

/// 一次過帳或作廢的庫存狀態：已鎖定的商品與批號，以及記下的不足與批號被使用。
#[derive(Debug)]
pub struct Inventory {
    products: BTreeMap<Id, Product>,
    lots: BTreeMap<Id, Lot>,
    /// 每個批號鎖定時的數量加上本次操作加入的數量（計算不足時的 `available`）。
    supplied: BTreeMap<Id, Decimal>,
    /// 先進先出模式的商品。
    fifo_products: BTreeSet<Id>,
    demands: BTreeMap<ShortageKey, Demand>,
    in_use: Vec<InUse>,
}

impl Inventory {
    /// 依鎖定順序鎖定商品（`FOR SHARE`，依 id）與批號（一次查詢 `FOR UPDATE`，依 `product_id, id`）。
    /// 單據主表必須已先鎖定。
    pub async fn lock(conn: &mut PgConnection, req: &LockRequest) -> Result<Self, CoreError> {
        let product_ids: Vec<Id> = req
            .product_ids
            .iter()
            .chain(&req.fifo_product_ids)
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let products = db::lock_products(&mut *conn, &product_ids)
            .await?
            .into_iter()
            .map(|r| {
                (
                    r.id,
                    Product {
                        id: r.id,
                        is_goods: r.product_type == GOODS,
                        code: r.code,
                        name: r.name,
                        deleted: r.deleted,
                    },
                )
            })
            .collect();

        // 依商品 + 批號找出的 id 一起鎖定；鎖定後才讀取數量
        let mut lot_ids: BTreeSet<Id> = req.lot_ids.iter().copied().collect();
        for (product_id, lot_no) in &req.lot_numbers {
            if let Some(lot) = db::find_lot_by_no(&mut *conn, *product_id, lot_no).await? {
                lot_ids.insert(lot.id);
            }
        }
        let lot_ids: Vec<Id> = lot_ids.into_iter().collect();
        let fifo: BTreeSet<Id> = req.fifo_product_ids.iter().copied().collect();
        let fifo_ids: Vec<Id> = fifo.iter().copied().collect();
        let lots: BTreeMap<Id, Lot> = db::lock_lots(&mut *conn, &lot_ids, &fifo_ids)
            .await?
            .into_iter()
            .map(|r| (r.id, Lot::from(r)))
            .collect();
        let supplied = lots.values().map(|l| (l.id, l.qty_remaining)).collect();
        Ok(Self {
            products,
            lots,
            supplied,
            fifo_products: fifo,
            demands: BTreeMap::new(),
            in_use: Vec::new(),
        })
    }

    /// 鎖定的商品；沒有鎖定（或不存在）時為 `None`。
    pub fn product(&self, id: Id) -> Option<&Product> {
        self.products.get(&id)
    }

    /// 鎖定的未刪除批號。
    pub fn lot(&self, id: Id) -> Option<&Lot> {
        self.lots.get(&id).filter(|l| !l.removed)
    }

    /// 鎖定的未刪除批號中，商品 `product_id`、批號為 `lot_no`（不分大小寫）的批號。
    pub fn lot_by_no(&self, product_id: Id, lot_no: &str) -> Option<&Lot> {
        let lower = lot_no.to_lowercase();
        self.lots
            .values()
            .find(|l| !l.removed && l.product_id == product_id && l.lot_no.to_lowercase() == lower)
    }

    /// 商品 `product_id` 目前在記憶體中的庫存（鎖定的未刪除批號合計）。只有先進先出模式的商品包含
    /// 所有有庫存的批號。
    pub fn qty_on_hand(&self, product_id: Id) -> Decimal {
        self.lots
            .values()
            .filter(|l| !l.removed && l.product_id == product_id)
            .map(|l| l.qty_remaining)
            .sum()
    }

    /// 有效的實體商品；`service` 回傳 `Ok(None)`。沒有鎖定 → [`CoreError::Internal`]；已刪除 →
    /// [`CoreError::Validation`]（呼叫端應在過帳前檢查）。
    fn goods(&self, product_id: Id) -> Result<Option<&Product>, CoreError> {
        let product = self
            .products
            .get(&product_id)
            .ok_or_else(|| CoreError::Internal(format!("商品 {product_id} 沒有先鎖定")))?;
        if product.deleted {
            return Err(CoreError::Validation(format!(
                "商品 {} 已刪除",
                product.code
            )));
        }
        Ok(product.is_goods.then_some(product))
    }

    /// 鎖定的未刪除批號，且屬於 `product_id`。
    fn locked_lot(&self, product_id: Id, lot_id: Id) -> Result<&Lot, CoreError> {
        let lot = self
            .lots
            .get(&lot_id)
            .filter(|l| !l.removed)
            .ok_or_else(|| CoreError::Validation("找不到此批號".into()))?;
        if lot.product_id != product_id {
            return Err(CoreError::Validation(format!(
                "批號 {} 不屬於此商品",
                lot.lot_no
            )));
        }
        Ok(lot)
    }

    async fn write_txn(
        conn: &mut PgConnection,
        src: &Source<'_>,
        line: Line,
        txn_type: TxnType,
        lot: &Lot,
        qty_change: Decimal,
        unit_cost_twd: Decimal,
    ) -> Result<(), CoreError> {
        db::insert_transaction(
            &mut *conn,
            &NewTransaction {
                id: new_id(),
                txn_date: src.txn_date,
                txn_type: txn_type.as_str(),
                product_id: lot.product_id,
                lot_id: lot.id,
                qty_change,
                unit_cost_twd,
                ref_doc_type: src.doc_type,
                ref_doc_id: src.doc_id,
                ref_line_id: line.id,
                created_by: Some(src.actor),
            },
        )
        .await?;
        Ok(())
    }

    async fn set_remaining(
        &mut self,
        conn: &mut PgConnection,
        src: &Source<'_>,
        lot_id: Id,
        qty_remaining: Decimal,
    ) -> Result<(), CoreError> {
        if !db::update_lot_qty_remaining(&mut *conn, lot_id, qty_remaining, Some(src.actor)).await?
        {
            return Err(CoreError::Internal(format!("批號 {lot_id} 已不存在")));
        }
        if let Some(lot) = self.lots.get_mut(&lot_id) {
            lot.qty_remaining = qty_remaining;
        }
        Ok(())
    }

    /// 記下一筆扣減需求；`available` 為目前計算不足時使用的可用數量。
    fn demand(
        &mut self,
        key: ShortageKey,
        line: LineRef,
        qty: Decimal,
        available: Decimal,
    ) -> &mut Demand {
        let order = self.demands.len();
        let d = self.demands.entry(key).or_insert_with(|| Demand {
            order,
            ..Demand::default()
        });
        d.required += qty;
        d.available = available;
        if !d.lines.contains(&line) {
            d.lines.push(line);
        }
        d
    }

    fn line_ref(src: &Source<'_>, line: Line) -> LineRef {
        LineRef {
            detail_key: src.line_type.to_owned(),
            index: line.index,
        }
    }

    /// 入庫：建立批號（`qty_in = qty_remaining = qty`）並新增異動（`+qty`）。`service` 商品回傳
    /// `Ok(None)`。批號與同商品未刪除的批號重複 → [`CoreError::Validation`]（[`LOT_NO_TAKEN`]）。
    pub async fn receive(
        &mut self,
        conn: &mut PgConnection,
        src: &Source<'_>,
        line: Line,
        receipt: &Receipt<'_>,
    ) -> Result<Option<Lot>, CoreError> {
        if self.goods(receipt.product_id)?.is_none() {
            return Ok(None);
        }
        if receipt.qty <= Decimal::ZERO {
            return Err(CoreError::Validation("數量必須大於 0".into()));
        }
        let lot_no = match receipt.lot_no {
            Some(no) => normalize_lot_no(no)
                .map_err(CoreError::Validation)?
                .unwrap_or_else(|| auto_lot_no(receipt.record_no, receipt.sort_order)),
            None => auto_lot_no(receipt.record_no, receipt.sort_order),
        };
        let id = new_id();
        let inserted = db::insert_lot(
            &mut *conn,
            &NewLot {
                id,
                product_id: receipt.product_id,
                lot_no: &lot_no,
                received_date: receipt.received_date,
                source_type: src.line_type,
                source_doc_id: src.doc_id,
                source_line_id: line.id,
                qty: receipt.qty,
                unit_cost_twd: receipt.unit_cost_twd,
                created_by: Some(src.actor),
            },
        )
        .await;
        match inserted {
            Ok(()) => {}
            Err(err) => {
                return Err(match err.unique_violation() {
                    Some(LOT_NO_UNIQUE) => CoreError::Validation(LOT_NO_TAKEN.into()),
                    Some(LOT_SOURCE_LINE_UNIQUE) => {
                        CoreError::Internal(format!("明細 {} 已經建立過批號", line.id))
                    }
                    _ => err.into(),
                });
            }
        }
        let lot = Lot {
            id,
            product_id: receipt.product_id,
            lot_no,
            received_date: receipt.received_date,
            qty_in: receipt.qty,
            qty_remaining: receipt.qty,
            unit_cost_twd: receipt.unit_cost_twd,
            removed: false,
        };
        Self::write_txn(
            conn,
            src,
            line,
            receipt.txn_type,
            &lot,
            receipt.qty,
            receipt.unit_cost_twd,
        )
        .await?;
        self.supplied.insert(id, receipt.qty);
        self.lots.insert(id, lot.clone());
        Ok(Some(lot))
    }

    /// 加回既有批號：`qty_remaining += qty`，新增異動（`+qty`、`unit_cost_twd`）。`unit_cost_twd` 為
    /// `None` 時使用批號的成本。回傳批號（加回後）；`service` 商品回傳 `Ok(None)`。
    #[allow(clippy::too_many_arguments)]
    pub async fn add_to_lot(
        &mut self,
        conn: &mut PgConnection,
        src: &Source<'_>,
        line: Line,
        product_id: Id,
        lot_id: Id,
        qty: Decimal,
        unit_cost_twd: Option<Decimal>,
        txn_type: TxnType,
    ) -> Result<Option<Lot>, CoreError> {
        if self.goods(product_id)?.is_none() {
            return Ok(None);
        }
        if qty <= Decimal::ZERO {
            return Err(CoreError::Validation("數量必須大於 0".into()));
        }
        let lot = self.locked_lot(product_id, lot_id)?.clone();
        let cost = unit_cost_twd.unwrap_or(lot.unit_cost_twd);
        self.set_remaining(conn, src, lot_id, lot.qty_remaining + qty)
            .await?;
        Self::write_txn(conn, src, line, txn_type, &lot, qty, cost).await?;
        *self.supplied.entry(lot_id).or_default() += qty;
        Ok(self.lots.get(&lot_id).cloned())
    }

    /// 從指定批號扣庫存（手動指定批號）：`qty ≤ qty_remaining` 時寫入批號扣除明細、`qty_remaining -= qty`、
    /// 異動（`−qty`），回傳扣減；不足時記下（不寫入）並回傳 `Ok(None)`。`service` 商品回傳 `Ok(None)`。
    /// 批號不存在、已刪除或不屬於該商品 → [`CoreError::Validation`]。
    #[allow(clippy::too_many_arguments)]
    pub async fn issue_from_lot(
        &mut self,
        conn: &mut PgConnection,
        src: &Source<'_>,
        line: Line,
        product_id: Id,
        lot_id: Id,
        qty: Decimal,
        txn_type: TxnType,
    ) -> Result<Option<Allocation>, CoreError> {
        let Some(lot) = self.take_from_lot(src, line, product_id, lot_id, qty)? else {
            return Ok(None);
        };
        let allocation = Allocation {
            lot_id,
            qty,
            unit_cost_twd: lot.unit_cost_twd,
        };
        self.write_issue(conn, src, line, &lot, allocation, txn_type)
            .await?;
        Ok(Some(allocation))
    }

    /// 減少指定批號的數量，但**不**寫入批號扣除明細（例如作廢盤盈加回的數量）：`qty ≤ qty_remaining` 時
    /// `qty_remaining -= qty`、異動（`−qty`、批號的成本），回傳 `true`；不足時記下並回傳 `false`。
    /// `service` 商品回傳 `false`。
    #[allow(clippy::too_many_arguments)]
    pub async fn reduce_lot(
        &mut self,
        conn: &mut PgConnection,
        src: &Source<'_>,
        line: Line,
        product_id: Id,
        lot_id: Id,
        qty: Decimal,
        txn_type: TxnType,
    ) -> Result<bool, CoreError> {
        let Some(lot) = self.take_from_lot(src, line, product_id, lot_id, qty)? else {
            return Ok(false);
        };
        self.set_remaining(conn, src, lot_id, lot.qty_remaining - qty)
            .await?;
        Self::write_txn(conn, src, line, txn_type, &lot, -qty, lot.unit_cost_twd).await?;
        Ok(true)
    }

    /// 檢查指定批號的數量是否足夠並記下需求。足夠時回傳扣減前的批號。
    fn take_from_lot(
        &mut self,
        src: &Source<'_>,
        line: Line,
        product_id: Id,
        lot_id: Id,
        qty: Decimal,
    ) -> Result<Option<Lot>, CoreError> {
        if self.goods(product_id)?.is_none() {
            return Ok(None);
        }
        if qty <= Decimal::ZERO {
            return Err(CoreError::Validation("數量必須大於 0".into()));
        }
        let lot = self.locked_lot(product_id, lot_id)?.clone();
        let supplied = self.supplied.get(&lot_id).copied().unwrap_or_default();
        let line = Self::line_ref(src, line);
        let enough = qty <= lot.qty_remaining;
        let demand = self.demand(ShortageKey::Lot(lot_id), line, qty, supplied);
        if !enough {
            demand.short = true;
            return Ok(None);
        }
        Ok(Some(lot))
    }

    async fn write_issue(
        &mut self,
        conn: &mut PgConnection,
        src: &Source<'_>,
        line: Line,
        lot: &Lot,
        allocation: Allocation,
        txn_type: TxnType,
    ) -> Result<(), CoreError> {
        db::insert_allocation(
            &mut *conn,
            &NewAllocation {
                id: new_id(),
                source_type: src.line_type,
                source_doc_id: src.doc_id,
                source_line_id: line.id,
                lot_id: lot.id,
                qty: allocation.qty,
                unit_cost_twd: allocation.unit_cost_twd,
                created_by: Some(src.actor),
            },
        )
        .await?;
        self.set_remaining(conn, src, lot.id, lot.qty_remaining - allocation.qty)
            .await?;
        Self::write_txn(
            conn,
            src,
            line,
            txn_type,
            lot,
            -allocation.qty,
            allocation.unit_cost_twd,
        )
        .await
    }

    /// 先進先出扣庫存：依 `received_date, id` 由舊到新扣 `qty`，可以跨多個批號；每個批號寫入一筆批號
    /// 扣除明細與異動（`−qty`）。商品必須在 [`LockRequest::fifo_product_ids`] 中。不足時記下（不寫入）
    /// 並回傳空陣列。`service` 商品回傳空陣列。
    pub async fn issue_fifo(
        &mut self,
        conn: &mut PgConnection,
        src: &Source<'_>,
        line: Line,
        product_id: Id,
        qty: Decimal,
        txn_type: TxnType,
    ) -> Result<Vec<Allocation>, CoreError> {
        if self.goods(product_id)?.is_none() {
            return Ok(Vec::new());
        }
        if qty <= Decimal::ZERO {
            return Err(CoreError::Validation("數量必須大於 0".into()));
        }
        if !self.fifo_products.contains(&product_id) {
            return Err(CoreError::Internal(format!(
                "商品 {product_id} 沒有以先進先出模式鎖定批號"
            )));
        }
        let mut lots: Vec<&Lot> = self
            .lots
            .values()
            .filter(|l| !l.removed && l.product_id == product_id)
            .collect();
        lots.sort_by_key(|l| (l.received_date, l.id));
        let available: Vec<(Id, Decimal)> = lots.iter().map(|l| (l.id, l.qty_remaining)).collect();
        let supplied: Decimal = lots
            .iter()
            .map(|l| self.supplied.get(&l.id).copied().unwrap_or_default())
            .sum();
        let plan = plan_fifo(&available, qty);
        let line_ref = Self::line_ref(src, line);
        let demand = self.demand(ShortageKey::Product(product_id), line_ref, qty, supplied);
        let plan = match plan {
            Ok(plan) => plan,
            Err(_) => {
                demand.short = true;
                return Ok(Vec::new());
            }
        };
        let mut out = Vec::with_capacity(plan.len());
        for (lot_id, take) in plan {
            let lot = self.lots[&lot_id].clone();
            let allocation = Allocation {
                lot_id,
                qty: take,
                unit_cost_twd: lot.unit_cost_twd,
            };
            self.write_issue(conn, src, line, &lot, allocation, txn_type)
                .await?;
            out.push(allocation);
        }
        Ok(out)
    }

    /// 作廢扣庫存的明細：依該列的批號扣除明細（[`allocations_of_line`]），把數量加回原批號並新增異動
    /// （`void_reversal`、`+qty`、扣除時的成本）。批號扣除明細保留（成本的歷史）。原批號必須已鎖定
    /// （放在 [`LockRequest::lot_ids`]）。回傳加回的筆數。
    pub async fn reverse_allocations(
        &mut self,
        conn: &mut PgConnection,
        src: &Source<'_>,
        line: Line,
        allocations: &[AllocationRow],
    ) -> Result<usize, CoreError> {
        for a in allocations {
            if a.source_line_id != line.id {
                return Err(CoreError::Internal(format!(
                    "批號扣除明細 {} 不屬於明細 {}",
                    a.id, line.id
                )));
            }
            let lot = self
                .lots
                .get(&a.lot_id)
                .filter(|l| !l.removed)
                .cloned()
                .ok_or_else(|| CoreError::Internal(format!("批號 {} 沒有先鎖定", a.lot_id)))?;
            self.set_remaining(conn, src, lot.id, lot.qty_remaining + a.qty)
                .await?;
            Self::write_txn(
                conn,
                src,
                line,
                TxnType::VoidReversal,
                &lot,
                a.qty,
                a.unit_cost_twd,
            )
            .await?;
            *self.supplied.entry(lot.id).or_default() += a.qty;
        }
        Ok(allocations.len())
    }

    /// 作廢入庫：刪除本單據建立的批號。批號必須沒有**其他單據的有效異動**；被使用時記下（不寫入）並
    /// 回傳 `false`。可以時新增異動（`void_reversal`、`−qty_remaining`）並軟刪除批號，回傳 `true`。
    /// 數量已為 0 時只軟刪除、不新增異動（異動的數量不可為 0）。
    pub async fn remove_lot(
        &mut self,
        conn: &mut PgConnection,
        src: &Source<'_>,
        line: Line,
        lot_id: Id,
    ) -> Result<bool, CoreError> {
        let lot = self
            .lots
            .get(&lot_id)
            .filter(|l| !l.removed)
            .cloned()
            .ok_or_else(|| CoreError::Internal(format!("批號 {lot_id} 沒有先鎖定")))?;
        if self.goods(lot.product_id)?.is_none() {
            return Ok(false);
        }
        let users = db::documents_using_lot(&mut *conn, lot_id, src.doc_type, src.doc_id).await?;
        if !users.is_empty() {
            let mut documents = Vec::with_capacity(users.len());
            for u in users {
                let record_no = document_record_no(conn, &u.ref_doc_type, u.ref_doc_id).await?;
                documents.push(DocumentRef {
                    entity_key: u.ref_doc_type,
                    record_id: u.ref_doc_id,
                    record_no,
                });
            }
            self.in_use.push(InUse { lot_id, documents });
            return Ok(false);
        }
        if lot.qty_remaining > Decimal::ZERO {
            Self::write_txn(
                conn,
                src,
                line,
                TxnType::VoidReversal,
                &lot,
                -lot.qty_remaining,
                lot.unit_cost_twd,
            )
            .await?;
        }
        if !db::soft_delete_lot(&mut *conn, lot_id, Some(src.actor)).await? {
            return Err(CoreError::Internal(format!("批號 {lot_id} 已不存在")));
        }
        if let Some(l) = self.lots.get_mut(&lot_id) {
            l.qty_remaining = Decimal::ZERO;
            l.removed = true;
        }
        Ok(true)
    }

    /// 結束：有批號被其他單據使用 → [`CoreError::LotInUse`]；有不足 → [`CoreError::StockInsufficient`]
    /// （列出**所有**不足的項目，依第一次出現的順序）。呼叫端收到錯誤後 rollback。
    pub fn finish(self) -> Result<(), CoreError> {
        if !self.in_use.is_empty() {
            let lots = self
                .in_use
                .iter()
                .map(|u| {
                    let lot = &self.lots[&u.lot_id];
                    let code = self
                        .products
                        .get(&lot.product_id)
                        .map(|p| p.code.clone())
                        .unwrap_or_default();
                    LotInUse {
                        lot_id: lot.id,
                        lot_no: lot.lot_no.clone(),
                        product_id: lot.product_id,
                        product_code: code,
                        documents: u.documents.clone(),
                    }
                })
                .collect();
            return Err(lot_in_use_error(LotInUseDetails { lots }));
        }
        let mut short: Vec<(&ShortageKey, &Demand)> =
            self.demands.iter().filter(|(_, d)| d.short).collect();
        if short.is_empty() {
            return Ok(());
        }
        short.sort_by_key(|(_, d)| d.order);
        let items = short
            .into_iter()
            .map(|(key, d)| {
                let (product_id, lot) = match key {
                    ShortageKey::Lot(id) => {
                        let lot = &self.lots[id];
                        (lot.product_id, Some(lot))
                    }
                    ShortageKey::Product(id) => (*id, None),
                };
                let product = self.products.get(&product_id);
                StockShortage {
                    product_id,
                    product_code: product.map(|p| p.code.clone()).unwrap_or_default(),
                    product_name: product.map(|p| p.name.clone()).unwrap_or_default(),
                    lot_id: lot.map(|l| l.id),
                    lot_no: lot.map(|l| l.lot_no.clone()),
                    required: qty4(d.required),
                    available: qty4(d.available),
                    shortage: qty4(d.required - d.available),
                    lines: d.lines.clone(),
                }
            })
            .collect();
        Err(stock_insufficient_error(StockInsufficientDetails { items }))
    }
}

/// 明細的批號扣除明細（依建立順序），供作廢時 [`Inventory::reverse_allocations`] 使用；鎖定前先讀取，
/// 把其中的 `lot_id` 放進 [`LockRequest::lot_ids`]。
pub async fn allocations_of_line(
    conn: &mut PgConnection,
    line_type: &str,
    line_id: Id,
) -> Result<Vec<AllocationRow>, CoreError> {
    Ok(db::list_allocations_by_line(&mut *conn, line_type, line_id).await?)
}

/// 單據的單號（含已刪除）；資料類別不存在或不是系統資料表時為空字串。
async fn document_record_no(
    conn: &mut PgConnection,
    entity_key: &str,
    id: Id,
) -> Result<String, CoreError> {
    let Some(entity) = schema_db::find_entity_by_key(&mut *conn, entity_key).await? else {
        return Ok(String::new());
    };
    let Some(table) = entity.table_name else {
        return Ok(String::new());
    };
    match document_db::find_record_no(&mut *conn, &table, id).await {
        Ok(no) => Ok(no.unwrap_or_default()),
        Err(DbError::UnsafeIdentifier(name)) => Err(CoreError::Internal(format!(
            "資料類別 {entity_key} 的資料表名稱不安全：{name}"
        ))),
        Err(e) => Err(e.into()),
    }
}

/// `STOCK_INSUFFICIENT` 的錯誤：訊息為第一項（例如「商品 A001 批號 L-01 庫存不足，尚缺 6」），
/// 多項時加上「等 N 項」。
pub fn stock_insufficient_error(details: StockInsufficientDetails) -> CoreError {
    let message = match details.items.first() {
        None => "庫存不足".to_owned(),
        Some(first) => {
            let target = match &first.lot_no {
                Some(no) => format!("商品 {} 批號 {no}", first.product_code),
                None => format!("商品 {}", first.product_code),
            };
            let mut m = format!("{target} 庫存不足，尚缺 {}", display_qty(first.shortage));
            if details.items.len() > 1 {
                m.push_str(&format!(" 等 {} 項", details.items.len()));
            }
            m
        }
    };
    CoreError::StockInsufficient { message, details }
}

/// `LOT_IN_USE` 的錯誤：訊息為第一個批號與第一張單據（例如「批號 L-01 已被其他單據使用，請先作廢
/// SH202611-0005」），多個批號時加上「等 N 個批號」。
pub fn lot_in_use_error(details: LotInUseDetails) -> CoreError {
    let message = match details.lots.first() {
        None => "批號已被其他單據使用".to_owned(),
        Some(first) => {
            let mut m = format!("批號 {} 已被其他單據使用", first.lot_no);
            let nos: Vec<&str> = first
                .documents
                .iter()
                .map(|d| d.record_no.as_str())
                .filter(|n| !n.is_empty())
                .collect();
            if !nos.is_empty() {
                m.push_str(&format!("，請先作廢 {}", nos.join("、")));
            }
            if details.lots.len() > 1 {
                m.push_str(&format!("（等 {} 個批號）", details.lots.len()));
            }
            m
        }
    };
    CoreError::LotInUse { message, details }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn id(n: u128) -> Id {
        Id::from_u128(n)
    }

    #[test]
    fn fifo_spans_multiple_lots_in_order() {
        let lots = [
            (id(1), d("3")),
            (id(2), d("0")),
            (id(3), d("5")),
            (id(4), d("10")),
        ];
        assert_eq!(
            plan_fifo(&lots, d("7.5")).unwrap(),
            [(id(1), d("3")), (id(3), d("4.5"))]
        );
    }

    #[test]
    fn fifo_exactly_uses_up_stock() {
        let lots = [(id(1), d("3")), (id(2), d("5"))];
        assert_eq!(
            plan_fifo(&lots, d("8")).unwrap(),
            [(id(1), d("3")), (id(2), d("5"))]
        );
        assert_eq!(plan_fifo(&lots, d("3")).unwrap(), [(id(1), d("3"))]);
    }

    #[test]
    fn fifo_reports_available_when_insufficient() {
        let lots = [(id(1), d("3")), (id(2), d("5.25"))];
        assert_eq!(plan_fifo(&lots, d("10")), Err(d("8.25")));
        assert_eq!(plan_fifo(&[], d("1")), Err(Decimal::ZERO));
    }

    #[test]
    fn take_fifo_takes_what_is_available() {
        let lots = [(id(1), d("3")), (id(2), d("0")), (id(3), d("5.25"))];
        assert_eq!(
            take_fifo(&lots, d("10")),
            (vec![(id(1), d("3")), (id(3), d("5.25"))], d("1.75"))
        );
        assert_eq!(take_fifo(&lots, d("2")), (vec![(id(1), d("2"))], d("0")));
        assert_eq!(take_fifo(&[], d("1")), (vec![], d("1")));
    }

    fn preview_lot(n: u128, product: u128, date: &str, qty: &str) -> PreviewLot {
        PreviewLot {
            id: id(n),
            product_id: id(product),
            received_date: Date::parse(
                date,
                time::macros::format_description!("[year]-[month]-[day]"),
            )
            .unwrap(),
            qty_remaining: d(qty),
        }
    }

    fn preview_line(product: u128, qty: &str, lot: Option<u128>) -> PreviewLine {
        PreviewLine {
            product_id: id(product),
            is_goods: product != 9,
            qty: d(qty),
            lot_id: lot.map(id),
        }
    }

    #[test]
    fn preview_issues_manual_lots_before_fifo() {
        // 商品 1：L-01（10-02，5）、L-02（11-01，7）；id 順序與日期相反，確認依日期排序
        let lots = [
            preview_lot(20, 1, "2026-10-02", "5"),
            preview_lot(10, 1, "2026-11-01", "7"),
            preview_lot(30, 2, "2026-10-01", "1"),
        ];
        let lines = [
            // 先進先出 3：指定批號的列先扣 L-01 4，剩 1 → L-01 1 + L-02 2
            preview_line(1, "3", None),
            preview_line(1, "4", Some(20)),
            // 同一商品接著扣：L-02 剩 5，要 6 → 不足 1
            preview_line(1, "6", None),
            // 服務類商品不分配
            preview_line(9, "1", None),
            // 其他商品
            preview_line(2, "1", None),
        ];
        let out = preview_allocations(&lines, &lots);
        assert_eq!(out[0], (vec![(id(20), d("1")), (id(10), d("2"))], d("0")));
        assert_eq!(out[1], (vec![(id(20), d("4"))], d("0")));
        assert_eq!(out[2], (vec![(id(10), d("5"))], d("1")));
        assert_eq!(out[3], (vec![], d("0")));
        assert_eq!(out[4], (vec![(id(30), d("1"))], d("0")));
    }

    #[test]
    fn preview_manual_lot_shortage() {
        let lots = [preview_lot(1, 1, "2026-10-02", "5")];
        let lines = [
            preview_line(1, "3", Some(1)),
            preview_line(1, "4", Some(1)),
            preview_line(1, "1", None),
        ];
        let out = preview_allocations(&lines, &lots);
        assert_eq!(out[0], (vec![(id(1), d("3"))], d("0")));
        assert_eq!(out[1], (vec![(id(1), d("2"))], d("2")));
        assert_eq!(out[2], (vec![], d("1")));
    }

    #[test]
    fn lot_no_is_trimmed_and_validated() {
        assert_eq!(
            normalize_lot_no("  L-01 ").unwrap().as_deref(),
            Some("L-01")
        );
        assert_eq!(normalize_lot_no("   ").unwrap(), None);
        assert_eq!(
            normalize_lot_no("L\u{0007}1").unwrap_err(),
            LOT_NO_CONTROL_CHARS
        );
        assert!(normalize_lot_no(&"批".repeat(50)).unwrap().is_some());
        assert!(normalize_lot_no(&"批".repeat(51)).is_err());
    }

    #[test]
    fn auto_lot_no_pads_line_number() {
        assert_eq!(auto_lot_no("ADJ202611-0003", 0), "ADJ202611-0003-01");
        assert_eq!(auto_lot_no("ADJ202611-0003", 11), "ADJ202611-0003-12");
        assert_eq!(auto_lot_no("ADJ202611-0003", 99), "ADJ202611-0003-100");
    }

    #[test]
    fn quantities_are_padded_to_four_places() {
        assert_eq!(qty4(d("6")).to_string(), "6.0000");
        assert_eq!(qty4(d("1.23456")).to_string(), "1.2346");
        assert_eq!(qty4(d("0.00005")).to_string(), "0.0001");
        assert_eq!(display_qty(d("6.0000")), "6");
        assert_eq!(display_qty(d("1.5000")), "1.5");
    }

    fn shortage(code: &str, lot_no: Option<&str>, short: &str) -> StockShortage {
        StockShortage {
            product_id: id(1),
            product_code: code.into(),
            product_name: "螺絲".into(),
            lot_id: lot_no.map(|_| id(9)),
            lot_no: lot_no.map(str::to_owned),
            required: d("10.0000"),
            available: d("4.0000"),
            shortage: d(short),
            lines: Vec::new(),
        }
    }

    #[test]
    fn stock_insufficient_message_uses_first_item() {
        let err = stock_insufficient_error(StockInsufficientDetails {
            items: vec![shortage("A001", Some("L-01"), "6.0000")],
        });
        assert_eq!(err.to_string(), "商品 A001 批號 L-01 庫存不足，尚缺 6");
        let err = stock_insufficient_error(StockInsufficientDetails {
            items: vec![
                shortage("A001", None, "1.5000"),
                shortage("A002", None, "1.0000"),
                shortage("A003", None, "2.0000"),
            ],
        });
        assert_eq!(err.to_string(), "商品 A001 庫存不足，尚缺 1.5 等 3 項");
        assert_eq!(
            stock_insufficient_error(StockInsufficientDetails { items: vec![] }).to_string(),
            "庫存不足"
        );
    }

    #[test]
    fn lot_in_use_message_names_documents() {
        let lot = |no: &str, docs: &[&str]| LotInUse {
            lot_id: id(1),
            lot_no: no.into(),
            product_id: id(2),
            product_code: "A001".into(),
            documents: docs
                .iter()
                .map(|n| DocumentRef {
                    entity_key: "shipment".into(),
                    record_id: id(3),
                    record_no: (*n).into(),
                })
                .collect(),
        };
        let err = lot_in_use_error(LotInUseDetails {
            lots: vec![lot("L-01", &["SH202611-0005"])],
        });
        assert_eq!(
            err.to_string(),
            "批號 L-01 已被其他單據使用，請先作廢 SH202611-0005"
        );
        let err = lot_in_use_error(LotInUseDetails {
            lots: vec![lot("L-01", &["SH-1", "SH-2"]), lot("L-02", &[])],
        });
        assert_eq!(
            err.to_string(),
            "批號 L-01 已被其他單據使用，請先作廢 SH-1、SH-2（等 2 個批號）"
        );
        assert_eq!(
            lot_in_use_error(LotInUseDetails { lots: vec![] }).to_string(),
            "批號已被其他單據使用"
        );
    }
}
