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
        SELECT id, status, customer_id, tax_type, tax_included
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
    pub unit_price: Decimal,
    pub discount_rate: Decimal,
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
        SELECT id, product_id, unit_price, discount_rate
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
