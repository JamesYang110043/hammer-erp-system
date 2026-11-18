//! 庫存資料表的存取（docs/architecture.md §8.10）：`inventory_lots`、`lot_allocations`、
//! `inventory_transactions` 與檢視表 `v_stock_balances`。
//!
//! 這裡只有資料表的基本讀寫；先進先出、庫存是否足夠、鎖定順序等規則在 `hammer_core::inventory`。
//! 所有函式都在呼叫端的 transaction 內執行（傳入 `&mut *tx`）。
//!
//! - 批號：[`insert_lot`]、[`find_lot`]、[`find_lot_by_no`]、[`lock_lots`]、
//!   [`update_lot_qty_remaining`]、[`soft_delete_lot`]
//! - 批號扣除：[`insert_allocation`]、[`list_allocations_by_line`]
//! - 異動：[`insert_transaction`]、[`lot_used_by_other_documents`]、[`documents_using_lot`]
//! - 庫存：[`stock_balance`]
//! - 查詢 API：[`list_stock_balances`]、[`list_lots_of_product`]、[`list_transactions`]（與對應的 `count_*`）
//! - 商品：[`lock_products`]（過帳時以 `FOR SHARE` 鎖定，§8.10「鎖定順序」第 2 步）、[`find_products`]（不鎖定）
//!
//! `lot_allocations`、`inventory_transactions` 只能新增，資料庫以 trigger 拒絕修改與刪除
//! （SQLSTATE [`APPEND_ONLY_SQLSTATE`]）。
//!
//! 已軟刪除（`deleted_at IS NOT NULL`）的批號一律視為不存在。

use rust_decimal::Decimal;
use sqlx::PgExecutor;
use time::Date;
use uuid::Uuid;

use crate::DbError;

/// 同一商品未刪除的批號不分大小寫不可重複（部分唯一索引）。
pub const LOT_NO_UNIQUE: &str = "uq_inventory_lots_product_lot_no";

/// 一列單據明細最多產生一個批號（`UNIQUE (source_type, source_line_id)`）。
pub const LOT_SOURCE_LINE_UNIQUE: &str = "inventory_lots_source_line_key";

/// 修改或刪除只新增的資料表時，trigger 回傳的 SQLSTATE（`restrict_violation`）。
pub const APPEND_ONLY_SQLSTATE: &str = "23001";

/// 批號（未刪除）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LotRow {
    pub id: Uuid,
    pub product_id: Uuid,
    pub lot_no: String,
    pub received_date: Date,
    pub source_type: String,
    pub source_doc_id: Uuid,
    pub source_line_id: Uuid,
    pub qty_in: Decimal,
    pub qty_remaining: Decimal,
    pub unit_cost_twd: Decimal,
    pub version: i32,
}

/// 新增批號的內容。`qty_in` 與 `qty_remaining` 都是 `qty`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewLot<'a> {
    pub id: Uuid,
    pub product_id: Uuid,
    pub lot_no: &'a str,
    pub received_date: Date,
    pub source_type: &'a str,
    pub source_doc_id: Uuid,
    pub source_line_id: Uuid,
    pub qty: Decimal,
    pub unit_cost_twd: Decimal,
    pub created_by: Option<Uuid>,
}

/// 新增批號。批號重複時為唯一限制錯誤（[`LOT_NO_UNIQUE`]、[`LOT_SOURCE_LINE_UNIQUE`]）。
pub async fn insert_lot<'e>(
    executor: impl PgExecutor<'e>,
    lot: &NewLot<'_>,
) -> Result<(), DbError> {
    sqlx::query!(
        r#"
        INSERT INTO inventory_lots
            (id, product_id, lot_no, received_date, source_type, source_doc_id, source_line_id,
             qty_in, qty_remaining, unit_cost_twd, created_by, updated_by)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8, $9, $10, $10)
        "#,
        lot.id,
        lot.product_id,
        lot.lot_no,
        lot.received_date,
        lot.source_type,
        lot.source_doc_id,
        lot.source_line_id,
        lot.qty,
        lot.unit_cost_twd,
        lot.created_by,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// 依 id 查詢未刪除的批號（不鎖定）。
pub async fn find_lot<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
) -> Result<Option<LotRow>, DbError> {
    let row = sqlx::query_as!(
        LotRow,
        r#"
        SELECT id, product_id, lot_no, received_date, source_type, source_doc_id, source_line_id,
               qty_in, qty_remaining, unit_cost_twd, version
        FROM inventory_lots
        WHERE id = $1 AND deleted_at IS NULL
        "#,
        id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 依商品與批號（不分大小寫）查詢未刪除的批號（不鎖定）。`lot_no` 應已去除前後空白。
pub async fn find_lot_by_no<'e>(
    executor: impl PgExecutor<'e>,
    product_id: Uuid,
    lot_no: &str,
) -> Result<Option<LotRow>, DbError> {
    let row = sqlx::query_as!(
        LotRow,
        r#"
        SELECT id, product_id, lot_no, received_date, source_type, source_doc_id, source_line_id,
               qty_in, qty_remaining, unit_cost_twd, version
        FROM inventory_lots
        WHERE product_id = $1 AND lower(lot_no) = lower($2) AND deleted_at IS NULL
        "#,
        product_id,
        lot_no,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 以一次查詢鎖定（`FOR UPDATE`）未刪除的批號：`lot_ids` 指定的批號，加上 `fifo_product_ids`
/// 商品所有 `qty_remaining > 0` 的批號（先進先出模式）。依 `product_id, id` 的順序鎖定與回傳
/// （§8.10「鎖定順序」第 3 步）。
///
/// 找不到（不存在或已刪除）的 `lot_ids` 不會出現在結果中，由呼叫端比對。
pub async fn lock_lots<'e>(
    executor: impl PgExecutor<'e>,
    lot_ids: &[Uuid],
    fifo_product_ids: &[Uuid],
) -> Result<Vec<LotRow>, DbError> {
    let rows = sqlx::query_as!(
        LotRow,
        r#"
        SELECT id, product_id, lot_no, received_date, source_type, source_doc_id, source_line_id,
               qty_in, qty_remaining, unit_cost_twd, version
        FROM inventory_lots
        WHERE deleted_at IS NULL
          AND (id = ANY($1) OR (product_id = ANY($2) AND qty_remaining > 0))
        ORDER BY product_id, id
        FOR UPDATE
        "#,
        lot_ids,
        fifo_product_ids,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 設定未刪除批號的 `qty_remaining`，`version` + 1。回傳是否有更新。
///
/// 呼叫端必須先以 [`lock_lots`] 鎖定該批號，並依鎖定後讀到的值計算。
pub async fn update_lot_qty_remaining<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    qty_remaining: Decimal,
    updated_by: Option<Uuid>,
) -> Result<bool, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE inventory_lots
        SET qty_remaining = $2, updated_by = $3, version = version + 1
        WHERE id = $1 AND deleted_at IS NULL
        "#,
        id,
        qty_remaining,
        updated_by,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// 軟刪除批號（作廢產生它的單據時），`qty_remaining` 設為 0，`version` + 1。回傳是否有更新。
pub async fn soft_delete_lot<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    updated_by: Option<Uuid>,
) -> Result<bool, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE inventory_lots
        SET qty_remaining = 0, deleted_at = now(), updated_by = $2, version = version + 1
        WHERE id = $1 AND deleted_at IS NULL
        "#,
        id,
        updated_by,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// 批號扣除明細。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllocationRow {
    pub id: Uuid,
    pub source_type: String,
    pub source_doc_id: Uuid,
    pub source_line_id: Uuid,
    pub lot_id: Uuid,
    pub qty: Decimal,
    pub unit_cost_twd: Decimal,
}

/// 新增批號扣除明細的內容。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewAllocation<'a> {
    pub id: Uuid,
    pub source_type: &'a str,
    pub source_doc_id: Uuid,
    pub source_line_id: Uuid,
    pub lot_id: Uuid,
    pub qty: Decimal,
    pub unit_cost_twd: Decimal,
    pub created_by: Option<Uuid>,
}

/// 新增批號扣除明細。
pub async fn insert_allocation<'e>(
    executor: impl PgExecutor<'e>,
    allocation: &NewAllocation<'_>,
) -> Result<(), DbError> {
    sqlx::query!(
        r#"
        INSERT INTO lot_allocations
            (id, source_type, source_doc_id, source_line_id, lot_id, qty, unit_cost_twd, created_by)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
        allocation.id,
        allocation.source_type,
        allocation.source_doc_id,
        allocation.source_line_id,
        allocation.lot_id,
        allocation.qty,
        allocation.unit_cost_twd,
        allocation.created_by,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// 一列單據明細的批號扣除明細，依 `id`（建立順序）排序。
pub async fn list_allocations_by_line<'e>(
    executor: impl PgExecutor<'e>,
    source_type: &str,
    source_line_id: Uuid,
) -> Result<Vec<AllocationRow>, DbError> {
    let rows = sqlx::query_as!(
        AllocationRow,
        r#"
        SELECT id, source_type, source_doc_id, source_line_id, lot_id, qty, unit_cost_twd
        FROM lot_allocations
        WHERE source_type = $1 AND source_line_id = $2
        ORDER BY id
        "#,
        source_type,
        source_line_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 新增庫存異動的內容。`txn_at` 為資料庫的 `now()`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewTransaction<'a> {
    pub id: Uuid,
    pub txn_date: Date,
    pub txn_type: &'a str,
    pub product_id: Uuid,
    pub lot_id: Uuid,
    pub qty_change: Decimal,
    pub unit_cost_twd: Decimal,
    pub ref_doc_type: &'a str,
    pub ref_doc_id: Uuid,
    pub ref_line_id: Uuid,
    pub created_by: Option<Uuid>,
}

/// 新增庫存異動。
pub async fn insert_transaction<'e>(
    executor: impl PgExecutor<'e>,
    txn: &NewTransaction<'_>,
) -> Result<(), DbError> {
    sqlx::query!(
        r#"
        INSERT INTO inventory_transactions
            (id, txn_date, txn_type, product_id, lot_id, qty_change, unit_cost_twd,
             ref_doc_type, ref_doc_id, ref_line_id, created_by)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        "#,
        txn.id,
        txn.txn_date,
        txn.txn_type,
        txn.product_id,
        txn.lot_id,
        txn.qty_change,
        txn.unit_cost_twd,
        txn.ref_doc_type,
        txn.ref_doc_id,
        txn.ref_line_id,
        txn.created_by,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// 批號是否被**其他單據**的有效異動使用：`lot_id` 相同、來源不是 (`ref_doc_type`, `ref_doc_id`)，
/// 且該來源單據對此批號沒有 `void_reversal` 的異動（§8.10 `inventory_transactions`、「作廢」）。
pub async fn lot_used_by_other_documents<'e>(
    executor: impl PgExecutor<'e>,
    lot_id: Uuid,
    ref_doc_type: &str,
    ref_doc_id: Uuid,
) -> Result<bool, DbError> {
    let used = sqlx::query_scalar!(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM inventory_transactions t
            WHERE t.lot_id = $1
              AND NOT (t.ref_doc_type = $2 AND t.ref_doc_id = $3)
              AND NOT EXISTS (
                  SELECT 1
                  FROM inventory_transactions v
                  WHERE v.lot_id = t.lot_id
                    AND v.ref_doc_type = t.ref_doc_type
                    AND v.ref_doc_id = t.ref_doc_id
                    AND v.txn_type = 'void_reversal'
              )
        ) AS "used!"
        "#,
        lot_id,
        ref_doc_type,
        ref_doc_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(used)
}

/// 商品的庫存數量與台幣庫存金額（`v_stock_balances`，金額未四捨五入）。沒有批號時為 0。
pub async fn stock_balance<'e>(
    executor: impl PgExecutor<'e>,
    product_id: Uuid,
) -> Result<(Decimal, Decimal), DbError> {
    let row = sqlx::query!(
        r#"
        SELECT qty_on_hand, stock_value_twd
        FROM v_stock_balances
        WHERE product_id = $1
        "#,
        product_id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row.map_or((Decimal::ZERO, Decimal::ZERO), |r| {
        (
            r.qty_on_hand.unwrap_or_default(),
            r.stock_value_twd.unwrap_or_default(),
        )
    }))
}

/// 單據（主表）的來源：資料類別代碼與 id。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentSourceRow {
    pub ref_doc_type: String,
    pub ref_doc_id: Uuid,
}

/// 對批號 `lot_id` 有**有效**異動的其他單據（不含 (`ref_doc_type`, `ref_doc_id`)），條件與
/// [`lot_used_by_other_documents`] 相同。依第一筆異動的時間排列。
pub async fn documents_using_lot<'e>(
    executor: impl PgExecutor<'e>,
    lot_id: Uuid,
    ref_doc_type: &str,
    ref_doc_id: Uuid,
) -> Result<Vec<DocumentSourceRow>, DbError> {
    let rows = sqlx::query_as!(
        DocumentSourceRow,
        r#"
        SELECT t.ref_doc_type, t.ref_doc_id
        FROM inventory_transactions t
        WHERE t.lot_id = $1
          AND NOT (t.ref_doc_type = $2 AND t.ref_doc_id = $3)
          AND NOT EXISTS (
              SELECT 1
              FROM inventory_transactions v
              WHERE v.lot_id = t.lot_id
                AND v.ref_doc_type = t.ref_doc_type
                AND v.ref_doc_id = t.ref_doc_id
                AND v.txn_type = 'void_reversal'
          )
        GROUP BY t.ref_doc_type, t.ref_doc_id
        ORDER BY min(t.txn_at), t.ref_doc_id
        "#,
        lot_id,
        ref_doc_type,
        ref_doc_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 過帳時鎖定的商品資訊。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductStockRow {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    /// `goods` / `service`。
    pub product_type: String,
    /// 是否已軟刪除。
    pub deleted: bool,
}

/// 以 `FOR SHARE` 依 `id` 由小到大鎖定商品（含已刪除，由呼叫端判斷），避免過帳期間商品被刪除或修改
/// 類型、單位（§7.2、§8.10「鎖定順序」第 2 步）。不存在的 id 不會出現在結果中。
pub async fn lock_products<'e>(
    executor: impl PgExecutor<'e>,
    ids: &[Uuid],
) -> Result<Vec<ProductStockRow>, DbError> {
    let rows = sqlx::query_as!(
        ProductStockRow,
        r#"
        SELECT id, code, name, product_type, deleted_at IS NOT NULL AS "deleted!"
        FROM products
        WHERE id = ANY($1)
        ORDER BY id
        FOR SHARE
        "#,
        ids,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 商品資訊（含已刪除，由呼叫端判斷），不鎖定。供存檔時的檢查使用（過帳時改用 [`lock_products`]）。
/// 不存在的 id 不會出現在結果中。
pub async fn find_products<'e>(
    executor: impl PgExecutor<'e>,
    ids: &[Uuid],
) -> Result<Vec<ProductStockRow>, DbError> {
    let rows = sqlx::query_as!(
        ProductStockRow,
        r#"
        SELECT id, code, name, product_type, deleted_at IS NOT NULL AS "deleted!"
        FROM products
        WHERE id = ANY($1)
        ORDER BY id
        "#,
        ids,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// 庫存查詢（§8.10「庫存查詢 API」）
// ---------------------------------------------------------------------------
//
// 來源單據的單號：依來源類型 LEFT JOIN 單據主表（盤點調整單、進貨單）；進貨退回、M4c 的單據之後加上。

/// 商品庫存列表的條件。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StockBalanceFilter {
    /// 每個詞都要出現在料號或品名中（不分大小寫）；空陣列為不限。
    pub terms: Vec<String>,
    pub category_id: Option<Uuid>,
    /// `Some(true)`：庫存大於 0；`Some(false)`：庫存為 0。
    pub in_stock: Option<bool>,
    /// 只列出有設定安全庫存且庫存低於安全庫存。
    pub below_safety_stock: bool,
    pub include_inactive: bool,
}

/// 商品庫存（`products LEFT JOIN v_stock_balances`，金額未四捨五入）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockBalanceRow {
    pub product_id: Uuid,
    pub code: String,
    pub name: String,
    pub spec: Option<String>,
    pub unit_name: String,
    pub is_active: bool,
    pub qty_on_hand: Decimal,
    pub stock_value_twd: Decimal,
    pub safety_stock: Option<Decimal>,
}

/// 未刪除的 `goods` 商品與其庫存，依 `lower(code), id` 排序。
pub async fn list_stock_balances<'e>(
    executor: impl PgExecutor<'e>,
    filter: &StockBalanceFilter,
    limit: i64,
    offset: i64,
) -> Result<Vec<StockBalanceRow>, DbError> {
    let rows = sqlx::query_as!(
        StockBalanceRow,
        r#"
        SELECT p.id AS product_id, p.code, p.name, p.spec, u.name AS unit_name, p.is_active,
               COALESCE(b.qty_on_hand, 0) AS "qty_on_hand!",
               COALESCE(b.stock_value_twd, 0) AS "stock_value_twd!",
               p.safety_stock
        FROM products p
        JOIN units u ON u.id = p.unit_id
        LEFT JOIN v_stock_balances b ON b.product_id = p.id
        WHERE p.deleted_at IS NULL AND p.product_type = 'goods'
          AND NOT EXISTS (
              SELECT 1 FROM unnest($1::text[]) AS w(term)
              WHERE strpos(lower(p.code), lower(w.term)) = 0
                AND strpos(lower(p.name), lower(w.term)) = 0
          )
          AND ($2::uuid IS NULL OR p.category_id = $2)
          AND ($3::bool IS NULL OR (COALESCE(b.qty_on_hand, 0) > 0) = $3)
          AND (NOT $4::bool
               OR (p.safety_stock IS NOT NULL AND COALESCE(b.qty_on_hand, 0) < p.safety_stock))
          AND ($5::bool OR p.is_active)
        ORDER BY lower(p.code), p.id
        LIMIT $6 OFFSET $7
        "#,
        &filter.terms,
        filter.category_id,
        filter.in_stock,
        filter.below_safety_stock,
        filter.include_inactive,
        limit,
        offset,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 符合條件的商品數。條件與 [`list_stock_balances`] 相同。
pub async fn count_stock_balances<'e>(
    executor: impl PgExecutor<'e>,
    filter: &StockBalanceFilter,
) -> Result<i64, DbError> {
    let n = sqlx::query_scalar!(
        r#"
        SELECT count(*) AS "n!"
        FROM products p
        LEFT JOIN v_stock_balances b ON b.product_id = p.id
        WHERE p.deleted_at IS NULL AND p.product_type = 'goods'
          AND NOT EXISTS (
              SELECT 1 FROM unnest($1::text[]) AS w(term)
              WHERE strpos(lower(p.code), lower(w.term)) = 0
                AND strpos(lower(p.name), lower(w.term)) = 0
          )
          AND ($2::uuid IS NULL OR p.category_id = $2)
          AND ($3::bool IS NULL OR (COALESCE(b.qty_on_hand, 0) > 0) = $3)
          AND (NOT $4::bool
               OR (p.safety_stock IS NOT NULL AND COALESCE(b.qty_on_hand, 0) < p.safety_stock))
          AND ($5::bool OR p.is_active)
        "#,
        &filter.terms,
        filter.category_id,
        filter.in_stock,
        filter.below_safety_stock,
        filter.include_inactive,
    )
    .fetch_one(executor)
    .await?;
    Ok(n)
}

/// 商品批號列表的條件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LotListFilter<'a> {
    pub product_id: Uuid,
    /// 包含 `qty_remaining = 0` 的批號。
    pub include_empty: bool,
    /// 批號包含（不分大小寫）。
    pub q: Option<&'a str>,
}

/// 批號列表的一列（含來源單據的單號）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LotListRow {
    pub id: Uuid,
    pub lot_no: String,
    pub received_date: Date,
    pub source_type: String,
    pub source_doc_id: Uuid,
    /// 來源單據的單號；找不到時為空字串。
    pub source_record_no: String,
    pub qty_in: Decimal,
    pub qty_remaining: Decimal,
    pub unit_cost_twd: Decimal,
}

/// 商品的未刪除批號，依先進先出順序（`received_date, id`）排序。
pub async fn list_lots_of_product<'e>(
    executor: impl PgExecutor<'e>,
    filter: &LotListFilter<'_>,
    limit: i64,
    offset: i64,
) -> Result<Vec<LotListRow>, DbError> {
    let rows = sqlx::query_as!(
        LotListRow,
        r#"
        SELECT l.id, l.lot_no, l.received_date, l.source_type, l.source_doc_id,
               COALESCE(sa.record_no, gr.record_no, '') AS "source_record_no!",
               l.qty_in, l.qty_remaining, l.unit_cost_twd
        FROM inventory_lots l
        LEFT JOIN stock_adjustments sa
               ON l.source_type = 'stock_adjustment_line' AND sa.id = l.source_doc_id
        LEFT JOIN goods_receipts gr
               ON l.source_type = 'goods_receipt_line' AND gr.id = l.source_doc_id
        WHERE l.product_id = $1 AND l.deleted_at IS NULL
          AND ($2::bool OR l.qty_remaining > 0)
          AND ($3::text IS NULL OR strpos(lower(l.lot_no), lower($3)) > 0)
        ORDER BY l.received_date, l.id
        LIMIT $4 OFFSET $5
        "#,
        filter.product_id,
        filter.include_empty,
        filter.q,
        limit,
        offset,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 符合條件的批號數。條件與 [`list_lots_of_product`] 相同。
pub async fn count_lots_of_product<'e>(
    executor: impl PgExecutor<'e>,
    filter: &LotListFilter<'_>,
) -> Result<i64, DbError> {
    let n = sqlx::query_scalar!(
        r#"
        SELECT count(*) AS "n!"
        FROM inventory_lots l
        WHERE l.product_id = $1 AND l.deleted_at IS NULL
          AND ($2::bool OR l.qty_remaining > 0)
          AND ($3::text IS NULL OR strpos(lower(l.lot_no), lower($3)) > 0)
        "#,
        filter.product_id,
        filter.include_empty,
        filter.q,
    )
    .fetch_one(executor)
    .await?;
    Ok(n)
}

/// 異動流水帳的條件（`None` 為不限）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TransactionFilter<'a> {
    pub product_id: Option<Uuid>,
    pub lot_id: Option<Uuid>,
    /// `txn_date >= date_from`（含）。
    pub date_from: Option<Date>,
    /// `txn_date <= date_to`（含）。
    pub date_to: Option<Date>,
    pub txn_type: Option<&'a str>,
    pub ref_doc_type: Option<&'a str>,
    pub ref_doc_id: Option<Uuid>,
}

/// 異動流水帳的一列（含商品、批號、單號與建立者名稱）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionListRow {
    pub id: Uuid,
    pub txn_at: time::OffsetDateTime,
    pub txn_date: Date,
    pub txn_type: String,
    pub product_id: Uuid,
    pub product_code: String,
    pub product_name: String,
    pub lot_id: Uuid,
    pub lot_no: String,
    pub qty_change: Decimal,
    pub unit_cost_twd: Decimal,
    pub ref_doc_type: String,
    pub ref_doc_id: Uuid,
    /// 來源單據的單號；找不到時為空字串。
    pub ref_record_no: String,
    pub ref_line_id: Uuid,
    pub created_by: Option<Uuid>,
    pub created_by_name: Option<String>,
}

/// 異動流水帳，依 `txn_date DESC, id DESC`（新到舊）排序。商品、批號含已刪除。
pub async fn list_transactions<'e>(
    executor: impl PgExecutor<'e>,
    filter: &TransactionFilter<'_>,
    limit: i64,
    offset: i64,
) -> Result<Vec<TransactionListRow>, DbError> {
    let rows = sqlx::query_as!(
        TransactionListRow,
        r#"
        SELECT t.id, t.txn_at, t.txn_date, t.txn_type, t.product_id,
               p.code AS product_code, p.name AS product_name,
               t.lot_id, l.lot_no, t.qty_change, t.unit_cost_twd,
               t.ref_doc_type, t.ref_doc_id,
               COALESCE(sa.record_no, gr.record_no, '') AS "ref_record_no!",
               t.ref_line_id, t.created_by, u.display_name AS "created_by_name?"
        FROM inventory_transactions t
        JOIN products p ON p.id = t.product_id
        JOIN inventory_lots l ON l.id = t.lot_id
        LEFT JOIN stock_adjustments sa
               ON t.ref_doc_type = 'stock_adjustment' AND sa.id = t.ref_doc_id
        LEFT JOIN goods_receipts gr
               ON t.ref_doc_type = 'goods_receipt' AND gr.id = t.ref_doc_id
        LEFT JOIN users u ON u.id = t.created_by
        WHERE ($1::uuid IS NULL OR t.product_id = $1)
          AND ($2::uuid IS NULL OR t.lot_id = $2)
          AND ($3::date IS NULL OR t.txn_date >= $3)
          AND ($4::date IS NULL OR t.txn_date <= $4)
          AND ($5::text IS NULL OR t.txn_type = $5)
          AND ($6::text IS NULL OR t.ref_doc_type = $6)
          AND ($7::uuid IS NULL OR t.ref_doc_id = $7)
        ORDER BY t.txn_date DESC, t.id DESC
        LIMIT $8 OFFSET $9
        "#,
        filter.product_id,
        filter.lot_id,
        filter.date_from,
        filter.date_to,
        filter.txn_type,
        filter.ref_doc_type,
        filter.ref_doc_id,
        limit,
        offset,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 符合條件的異動筆數。條件與 [`list_transactions`] 相同。
pub async fn count_transactions<'e>(
    executor: impl PgExecutor<'e>,
    filter: &TransactionFilter<'_>,
) -> Result<i64, DbError> {
    let n = sqlx::query_scalar!(
        r#"
        SELECT count(*) AS "n!"
        FROM inventory_transactions t
        WHERE ($1::uuid IS NULL OR t.product_id = $1)
          AND ($2::uuid IS NULL OR t.lot_id = $2)
          AND ($3::date IS NULL OR t.txn_date >= $3)
          AND ($4::date IS NULL OR t.txn_date <= $4)
          AND ($5::text IS NULL OR t.txn_type = $5)
          AND ($6::text IS NULL OR t.ref_doc_type = $6)
          AND ($7::uuid IS NULL OR t.ref_doc_id = $7)
        "#,
        filter.product_id,
        filter.lot_id,
        filter.date_from,
        filter.date_to,
        filter.txn_type,
        filter.ref_doc_type,
        filter.ref_doc_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(n)
}
