//! 銷貨模組（訂單、出貨單、銷貨退回，docs/architecture.md §8.9）的專屬讀取。
//!
//! 一般的新增、修改走通用資料 API（`system_record`）；這裡是存檔時需要的、跨單據的讀取。
//! 所有函式都在呼叫端的 transaction 內執行（傳入 `&mut *tx`）。
//!
//! - [`find_shipment_for_return`]：銷貨退回帶入表頭用的出貨單（`FOR SHARE`）
//! - [`list_shipment_lines_for_return`]：銷貨退回帶入明細用的出貨明細
//! - [`product_tax_types`]：新增時稅別的預設值（明細商品的稅別）
//! - [`lock_customer`]、[`list_order_lines_for_update`]：確認訂單時鎖定客戶、讀取明細
//! - [`count_customer_documents`]、[`count_open_orders_by_product`]：客戶、商品被銷貨單據引用時的限制
//! - 出貨單（訂單轉出貨單、過帳、作廢）：[`find_order_for_shipment`]、[`lock_order_for_shipment`]、
//!   [`list_order_lines`]、[`list_shipment_lines_for_update`]、[`update_shipment_line_posting`]、
//!   [`set_shipment_cost_total`]、[`add_shipped_qty`]、[`set_order_status`]、[`posted_returns_of_shipment`]
//! - 銷貨退回（退回數量上限、過帳、作廢）：[`lock_shipment_for_return`]、[`returned_qty_of_lines`]、
//!   [`returned_qty_by_lot`]、[`list_return_lines_for_update`]、[`update_return_line_cost`]、
//!   [`set_return_cost_total`]、[`return_transactions`]

use rust_decimal::Decimal;
use sqlx::PgExecutor;
use uuid::Uuid;

use crate::DbError;

/// 銷貨退回從出貨單帶入的表頭值（§8.9「銷貨退回」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShipmentForReturn {
    pub id: Uuid,
    /// `draft` / `posted` / `void`。
    pub status: String,
    pub customer_id: Uuid,
    pub tax_type: String,
    pub tax_included: bool,
    pub ship_date: time::Date,
}

/// 未刪除的出貨單 `id`，以 `FOR SHARE` 鎖定（存檔期間不會被過帳、作廢或刪除）。不存在或已刪除時為
/// `None`。
pub async fn find_shipment_for_return<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
) -> Result<Option<ShipmentForReturn>, DbError> {
    let row = sqlx::query_as!(
        ShipmentForReturn,
        r#"
        SELECT id, status, customer_id, tax_type, tax_included, ship_date
        FROM shipments
        WHERE id = $1 AND deleted_at IS NULL
        FOR SHARE
        "#,
        id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 銷貨退回明細從出貨明細帶入的值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShipmentLineForReturn {
    pub id: Uuid,
    pub product_id: Uuid,
    /// 出貨明細的品名（M11），銷貨退回明細沿用。
    pub product_name: String,
    pub unit_price: Decimal,
    pub discount_rate: Decimal,
    pub qty: Decimal,
}

/// 出貨單 `shipment_id` 中 id 在 `ids` 內的未刪除明細（依 id 排序）。不屬於該出貨單的 id 不會出現。
pub async fn list_shipment_lines_for_return<'e>(
    executor: impl PgExecutor<'e>,
    shipment_id: Uuid,
    ids: &[Uuid],
) -> Result<Vec<ShipmentLineForReturn>, DbError> {
    let rows = sqlx::query_as!(
        ShipmentLineForReturn,
        r#"
        SELECT id, product_id, product_name, unit_price, discount_rate, qty
        FROM shipment_lines
        WHERE shipment_id = $1 AND id = ANY($2) AND deleted_at IS NULL
        ORDER BY id
        "#,
        shipment_id,
        ids,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 商品 `ids` 中未刪除商品的稅別（依 id 排序；不存在或已刪除的不列出）。
pub async fn product_tax_types<'e>(
    executor: impl PgExecutor<'e>,
    ids: &[Uuid],
) -> Result<Vec<String>, DbError> {
    let rows = sqlx::query_scalar!(
        r#"
        SELECT tax_type
        FROM products
        WHERE id = ANY($1) AND deleted_at IS NULL
        ORDER BY id
        "#,
        ids,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 未刪除商品 `product_id` 的 `name`（銷貨明細沒帶品名時的預設值，§8.9「明細的品名」）；
/// 不存在或已刪除時為 `None`。
pub async fn product_name<'e>(
    executor: impl PgExecutor<'e>,
    product_id: Uuid,
) -> Result<Option<String>, DbError> {
    let name = sqlx::query_scalar!(
        "SELECT name FROM products WHERE id = $1 AND deleted_at IS NULL",
        product_id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(name)
}

/// 以 `FOR SHARE` 鎖定客戶（含已刪除）：回傳是否已刪除；不存在時為 `None`。鎖定後到 transaction 結束前
/// 客戶不會被刪除。
pub async fn lock_customer<'e>(
    executor: impl PgExecutor<'e>,
    customer_id: Uuid,
) -> Result<Option<bool>, DbError> {
    let deleted = sqlx::query_scalar!(
        r#"
        SELECT deleted_at IS NOT NULL AS "deleted!"
        FROM customers
        WHERE id = $1
        FOR SHARE
        "#,
        customer_id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(deleted)
}

/// 一列訂單明細的系統欄位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderLineRow {
    pub id: Uuid,
    pub sort_order: i32,
    pub product_id: Uuid,
    pub qty: Decimal,
    pub unit_price: Decimal,
    pub discount_rate: Decimal,
    pub shipped_qty: Decimal,
}

/// 訂單 `sales_order_id` 的未刪除明細，依 `sort_order, id` 排序並鎖定（`FOR UPDATE`）。
/// 呼叫端必須已先鎖定訂單。
pub async fn list_order_lines_for_update<'e>(
    executor: impl PgExecutor<'e>,
    sales_order_id: Uuid,
) -> Result<Vec<OrderLineRow>, DbError> {
    let rows = sqlx::query_as!(
        OrderLineRow,
        r#"
        SELECT id, sort_order, product_id, qty, unit_price, discount_rate, shipped_qty
        FROM sales_order_lines
        WHERE sales_order_id = $1 AND deleted_at IS NULL
        ORDER BY sort_order, id
        FOR UPDATE
        "#,
        sales_order_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 指向客戶 `customer_id` 的未刪除訂單、出貨單、銷貨退回張數（不論狀態，含草稿與已作廢）。
pub async fn count_customer_documents<'e>(
    executor: impl PgExecutor<'e>,
    customer_id: Uuid,
) -> Result<i64, DbError> {
    let n = sqlx::query_scalar!(
        r#"
        SELECT
            (SELECT count(*) FROM sales_orders WHERE customer_id = $1 AND deleted_at IS NULL)
          + (SELECT count(*) FROM shipments WHERE customer_id = $1 AND deleted_at IS NULL)
          + (SELECT count(*) FROM sales_returns WHERE customer_id = $1 AND deleted_at IS NULL)
            AS "n!"
        "#,
        customer_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(n)
}

/// 明細引用商品 `product_id` 的未結案訂單（`confirmed`、`partially_shipped`，未刪除）張數。
pub async fn count_open_orders_by_product<'e>(
    executor: impl PgExecutor<'e>,
    product_id: Uuid,
) -> Result<i64, DbError> {
    let n = sqlx::query_scalar!(
        r#"
        SELECT count(DISTINCT o.id) AS "n!"
        FROM sales_order_lines l
        JOIN sales_orders o ON o.id = l.sales_order_id
        WHERE l.product_id = $1
          AND l.deleted_at IS NULL
          AND o.deleted_at IS NULL
          AND o.status IN ('confirmed', 'partially_shipped')
        "#,
        product_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(n)
}

/// 出貨單引用的訂單。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderForShipment {
    pub id: Uuid,
    pub record_no: String,
    /// `draft` / `confirmed` / `partially_shipped` / `closed` / `void`。
    pub status: String,
    pub customer_id: Uuid,
    pub tax_type: String,
    /// 是否為手動結案（`closed_at` 有值）。
    pub closed_manually: bool,
}

/// 未刪除的訂單（不鎖定；存檔時的檢查與帶入）。不存在或已刪除時為 `None`。
pub async fn find_order_for_shipment<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
) -> Result<Option<OrderForShipment>, DbError> {
    let row = sqlx::query_as!(
        OrderForShipment,
        r#"
        SELECT id, record_no, status, customer_id, tax_type,
               closed_at IS NOT NULL AS "closed_manually!"
        FROM sales_orders
        WHERE id = $1 AND deleted_at IS NULL
        "#,
        id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 同 [`find_order_for_shipment`]，並鎖定訂單（`FOR UPDATE`；出貨單過帳、作廢時）。
pub async fn lock_order_for_shipment<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
) -> Result<Option<OrderForShipment>, DbError> {
    let row = sqlx::query_as!(
        OrderForShipment,
        r#"
        SELECT id, record_no, status, customer_id, tax_type,
               closed_at IS NOT NULL AS "closed_manually!"
        FROM sales_orders
        WHERE id = $1 AND deleted_at IS NULL
        FOR UPDATE
        "#,
        id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 訂單的未刪除明細，依 `sort_order, id` 排序（不鎖定；存檔時的檢查）。
pub async fn list_order_lines<'e>(
    executor: impl PgExecutor<'e>,
    sales_order_id: Uuid,
) -> Result<Vec<OrderLineRow>, DbError> {
    let rows = sqlx::query_as!(
        OrderLineRow,
        r#"
        SELECT id, sort_order, product_id, qty, unit_price, discount_rate, shipped_qty
        FROM sales_order_lines
        WHERE sales_order_id = $1 AND deleted_at IS NULL
        ORDER BY sort_order, id
        "#,
        sales_order_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 一列出貨明細。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShipmentLineRow {
    pub id: Uuid,
    pub sort_order: i32,
    pub sales_order_line_id: Option<Uuid>,
    pub product_id: Uuid,
    pub qty: Decimal,
    pub unit_price: Decimal,
    pub discount_rate: Decimal,
    pub lot_no: Option<String>,
    pub lot_id: Option<Uuid>,
}

/// 出貨單 `shipment_id` 的未刪除明細，依 `sort_order, id` 排序並鎖定（`FOR UPDATE`）。
/// 呼叫端必須已先鎖定出貨單。
pub async fn list_shipment_lines_for_update<'e>(
    executor: impl PgExecutor<'e>,
    shipment_id: Uuid,
) -> Result<Vec<ShipmentLineRow>, DbError> {
    let rows = sqlx::query_as!(
        ShipmentLineRow,
        r#"
        SELECT id, sort_order, sales_order_line_id, product_id, qty, unit_price, discount_rate,
               lot_no, lot_id
        FROM shipment_lines
        WHERE shipment_id = $1 AND deleted_at IS NULL
        ORDER BY sort_order, id
        FOR UPDATE
        "#,
        shipment_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 寫入出貨明細過帳的結果：`lot_id`、`cost_amount_twd`，`version` + 1。回傳是否有更新。
pub async fn update_shipment_line_posting<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    lot_id: Option<Uuid>,
    cost_amount_twd: Option<Decimal>,
    updated_by: Uuid,
) -> Result<bool, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE shipment_lines
        SET lot_id = $2, cost_amount_twd = $3, updated_by = $4, version = version + 1
        WHERE id = $1 AND deleted_at IS NULL
        "#,
        id,
        lot_id,
        cost_amount_twd,
        updated_by,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// 寫入出貨單的 `cost_total_twd`（過帳時）。`version` 由單據框架變更狀態時遞增，這裡不變。
pub async fn set_shipment_cost_total<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    cost_total_twd: Decimal,
) -> Result<bool, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE shipments
        SET cost_total_twd = $2
        WHERE id = $1 AND deleted_at IS NULL
        "#,
        id,
        cost_total_twd,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// 訂單明細的 `shipped_qty += delta`（作廢時為負數），`version` + 1。回傳是否有更新。
pub async fn add_shipped_qty<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    delta: Decimal,
    updated_by: Uuid,
) -> Result<bool, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE sales_order_lines
        SET shipped_qty = shipped_qty + $2, updated_by = $3, version = version + 1
        WHERE id = $1 AND deleted_at IS NULL
        "#,
        id,
        delta,
        updated_by,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// 訂單的狀態改為 `status`（出貨過帳、作廢後重新計算的結果），`version` + 1。
pub async fn set_order_status<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    status: &str,
    updated_by: Uuid,
) -> Result<bool, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE sales_orders
        SET status = $2, updated_by = $3, version = version + 1
        WHERE id = $1 AND deleted_at IS NULL
        "#,
        id,
        status,
        updated_by,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// 指向出貨單、已過帳且未刪除的銷貨退回 `(id, record_no)`，依單號排序。
pub async fn posted_returns_of_shipment<'e>(
    executor: impl PgExecutor<'e>,
    shipment_id: Uuid,
) -> Result<Vec<(Uuid, String)>, DbError> {
    let rows = sqlx::query!(
        r#"
        SELECT id, record_no
        FROM sales_returns
        WHERE shipment_id = $1 AND status = 'posted' AND deleted_at IS NULL
        ORDER BY record_no
        "#,
        shipment_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.record_no)).collect())
}

/// 同 [`find_shipment_for_return`]，但以 `FOR UPDATE` 鎖定（銷貨退回過帳時；同一張出貨單的退回依序過帳）。
pub async fn lock_shipment_for_return<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
) -> Result<Option<ShipmentForReturn>, DbError> {
    let row = sqlx::query_as!(
        ShipmentForReturn,
        r#"
        SELECT id, status, customer_id, tax_type, tax_included, ship_date
        FROM shipments
        WHERE id = $1 AND deleted_at IS NULL
        FOR UPDATE
        "#,
        id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 出貨明細 `ids` 在**已過帳**、未刪除的銷貨退回中的退回數量合計 `(出貨明細 id, 合計)`；沒有退回的不列出。
pub async fn returned_qty_of_lines<'e>(
    executor: impl PgExecutor<'e>,
    ids: &[Uuid],
) -> Result<Vec<(Uuid, Decimal)>, DbError> {
    let rows = sqlx::query!(
        r#"
        SELECT l.shipment_line_id AS "id!", sum(l.qty) AS "qty!"
        FROM sales_return_lines l
        JOIN sales_returns r ON r.id = l.sales_return_id
        WHERE l.shipment_line_id = ANY($1)
          AND l.deleted_at IS NULL AND r.deleted_at IS NULL AND r.status = 'posted'
        GROUP BY l.shipment_line_id
        "#,
        ids,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.qty)).collect())
}

/// 出貨明細 `shipment_line_id` 的**已過帳**銷貨退回加回各批號的數量 `(批號 id, 合計)`（由
/// `txn_type = sales_return` 的異動計算）。
pub async fn returned_qty_by_lot<'e>(
    executor: impl PgExecutor<'e>,
    shipment_line_id: Uuid,
) -> Result<Vec<(Uuid, Decimal)>, DbError> {
    let rows = sqlx::query!(
        r#"
        SELECT t.lot_id, sum(t.qty_change) AS "qty!"
        FROM inventory_transactions t
        JOIN sales_return_lines l ON l.id = t.ref_line_id
        JOIN sales_returns r ON r.id = l.sales_return_id
        WHERE l.shipment_line_id = $1
          AND t.txn_type = 'sales_return'
          AND l.deleted_at IS NULL AND r.deleted_at IS NULL AND r.status = 'posted'
        GROUP BY t.lot_id
        "#,
        shipment_line_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(|r| (r.lot_id, r.qty)).collect())
}

/// 一列銷貨退回明細。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReturnLineRow {
    pub id: Uuid,
    pub sort_order: i32,
    pub shipment_line_id: Uuid,
    pub product_id: Uuid,
    pub qty: Decimal,
}

/// 銷貨退回 `sales_return_id` 的未刪除明細，依 `sort_order, id` 排序並鎖定（`FOR UPDATE`）。
pub async fn list_return_lines_for_update<'e>(
    executor: impl PgExecutor<'e>,
    sales_return_id: Uuid,
) -> Result<Vec<ReturnLineRow>, DbError> {
    let rows = sqlx::query_as!(
        ReturnLineRow,
        r#"
        SELECT id, sort_order, shipment_line_id, product_id, qty
        FROM sales_return_lines
        WHERE sales_return_id = $1 AND deleted_at IS NULL
        ORDER BY sort_order, id
        FOR UPDATE
        "#,
        sales_return_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 寫入銷貨退回明細過帳的 `cost_amount_twd`，`version` + 1。回傳是否有更新。
pub async fn update_return_line_cost<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    cost_amount_twd: Option<Decimal>,
    updated_by: Uuid,
) -> Result<bool, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE sales_return_lines
        SET cost_amount_twd = $2, updated_by = $3, version = version + 1
        WHERE id = $1 AND deleted_at IS NULL
        "#,
        id,
        cost_amount_twd,
        updated_by,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// 寫入銷貨退回的 `cost_total_twd`（過帳時）。`version` 由單據框架變更狀態時遞增，這裡不變。
pub async fn set_return_cost_total<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    cost_total_twd: Decimal,
) -> Result<bool, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE sales_returns
        SET cost_total_twd = $2
        WHERE id = $1 AND deleted_at IS NULL
        "#,
        id,
        cost_total_twd,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// 銷貨退回加回批號的一筆異動。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReturnTransactionRow {
    pub ref_line_id: Uuid,
    pub product_id: Uuid,
    pub lot_id: Uuid,
    pub qty_change: Decimal,
}

/// 銷貨退回 `sales_return_id` 過帳時加回批號的異動（`txn_type = sales_return`），依建立順序。
pub async fn return_transactions<'e>(
    executor: impl PgExecutor<'e>,
    sales_return_id: Uuid,
) -> Result<Vec<ReturnTransactionRow>, DbError> {
    let rows = sqlx::query_as!(
        ReturnTransactionRow,
        r#"
        SELECT ref_line_id, product_id, lot_id, qty_change
        FROM inventory_transactions
        WHERE ref_doc_type = 'sales_return' AND ref_doc_id = $1 AND txn_type = 'sales_return'
        ORDER BY id
        "#,
        sales_return_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}
