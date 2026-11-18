//! 進貨模組（採購單、進貨單、進貨退回，docs/architecture.md §8.8）的專屬讀取。
//!
//! 一般的新增、修改走通用資料 API（`system_record`）；這裡是存檔時需要的、跨單據的讀取。
//! 所有函式都在呼叫端的 transaction 內執行（傳入 `&mut *tx`）。
//!
//! - [`find_receipt_for_return`]：進貨退回帶入表頭用的進貨單（`FOR SHARE`）
//! - [`list_receipt_lines_for_return`]：進貨退回帶入明細用的進貨明細
//! - [`supplier_default_currency`]：新增採購單時帶入廠商的預設幣別
//! - [`find_tax_rate`]：稅別的稅率（金額計算）
//! - [`lock_supplier`]：確認採購單時以 `FOR SHARE` 鎖定廠商
//! - [`list_order_lines_for_update`]：確認採購單時鎖定並讀取明細
//! - [`count_supplier_documents`]、[`count_open_orders_by_product`]：廠商、商品被進貨單據引用時的限制

use rust_decimal::Decimal;
use sqlx::PgExecutor;
use uuid::Uuid;

use crate::DbError;

/// 進貨退回從進貨單帶入的表頭值（§8.8「進貨退回」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptForReturn {
    pub id: Uuid,
    /// `draft` / `posted` / `void`。
    pub status: String,
    pub supplier_id: Uuid,
    pub currency_code: String,
    /// 草稿可能沒有匯率；已過帳的一定有（`goods_receipts_posted_rate_check`）。
    pub exchange_rate: Option<Decimal>,
    pub tax_type: String,
}

/// 未刪除的進貨單 `id`，以 `FOR SHARE` 鎖定（存檔期間不會被過帳、作廢或刪除）。不存在或已刪除時為
/// `None`。
pub async fn find_receipt_for_return<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
) -> Result<Option<ReceiptForReturn>, DbError> {
    let row = sqlx::query_as!(
        ReceiptForReturn,
        r#"
        SELECT id, status, supplier_id, currency_code, exchange_rate, tax_type
        FROM goods_receipts
        WHERE id = $1 AND deleted_at IS NULL
        FOR SHARE
        "#,
        id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 進貨退回明細從進貨明細帶入的值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptLineForReturn {
    pub id: Uuid,
    pub product_id: Uuid,
    pub lot_no: Option<String>,
    pub unit_price: Decimal,
    pub lot_id: Option<Uuid>,
}

/// 進貨單 `goods_receipt_id` 中 id 在 `ids` 內的未刪除明細（依 id 排序）。不屬於該進貨單的 id 不會出現。
pub async fn list_receipt_lines_for_return<'e>(
    executor: impl PgExecutor<'e>,
    goods_receipt_id: Uuid,
    ids: &[Uuid],
) -> Result<Vec<ReceiptLineForReturn>, DbError> {
    let rows = sqlx::query_as!(
        ReceiptLineForReturn,
        r#"
        SELECT id, product_id, lot_no, unit_price, lot_id
        FROM goods_receipt_lines
        WHERE goods_receipt_id = $1 AND id = ANY($2) AND deleted_at IS NULL
        ORDER BY id
        "#,
        goods_receipt_id,
        ids,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 未刪除的廠商 `supplier_id` 的預設幣別；廠商不存在或已刪除時為 `None`。
pub async fn supplier_default_currency<'e>(
    executor: impl PgExecutor<'e>,
    supplier_id: Uuid,
) -> Result<Option<String>, DbError> {
    let code = sqlx::query_scalar!(
        r#"
        SELECT default_currency_code
        FROM suppliers
        WHERE id = $1 AND deleted_at IS NULL
        "#,
        supplier_id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(code)
}

/// 稅別 `code` 的稅率；不存在時為 `None`。
pub async fn find_tax_rate<'e>(
    executor: impl PgExecutor<'e>,
    code: &str,
) -> Result<Option<Decimal>, DbError> {
    let rate = sqlx::query_scalar!("SELECT rate FROM tax_rates WHERE code = $1", code)
        .fetch_optional(executor)
        .await?;
    Ok(rate)
}

/// 以 `FOR SHARE` 鎖定廠商（含已刪除）：回傳是否已刪除；不存在時為 `None`。鎖定後到 transaction 結束前
/// 廠商不會被刪除。
pub async fn lock_supplier<'e>(
    executor: impl PgExecutor<'e>,
    supplier_id: Uuid,
) -> Result<Option<bool>, DbError> {
    let deleted = sqlx::query_scalar!(
        r#"
        SELECT deleted_at IS NOT NULL AS "deleted!"
        FROM suppliers
        WHERE id = $1
        FOR SHARE
        "#,
        supplier_id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(deleted)
}

/// 一列採購明細的系統欄位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderLineRow {
    pub id: Uuid,
    pub sort_order: i32,
    pub product_id: Uuid,
    pub qty: Decimal,
    pub unit_price: Decimal,
    pub received_qty: Decimal,
}

/// 採購單 `purchase_order_id` 的未刪除明細，依 `sort_order, id` 排序並鎖定（`FOR UPDATE`）。
/// 呼叫端必須已先鎖定採購單。
pub async fn list_order_lines_for_update<'e>(
    executor: impl PgExecutor<'e>,
    purchase_order_id: Uuid,
) -> Result<Vec<OrderLineRow>, DbError> {
    let rows = sqlx::query_as!(
        OrderLineRow,
        r#"
        SELECT id, sort_order, product_id, qty, unit_price, received_qty
        FROM purchase_order_lines
        WHERE purchase_order_id = $1 AND deleted_at IS NULL
        ORDER BY sort_order, id
        FOR UPDATE
        "#,
        purchase_order_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 指向廠商 `supplier_id` 的未刪除採購單、進貨單、進貨退回張數（不論狀態，含草稿與已作廢）。
pub async fn count_supplier_documents<'e>(
    executor: impl PgExecutor<'e>,
    supplier_id: Uuid,
) -> Result<i64, DbError> {
    let n = sqlx::query_scalar!(
        r#"
        SELECT
            (SELECT count(*) FROM purchase_orders WHERE supplier_id = $1 AND deleted_at IS NULL)
          + (SELECT count(*) FROM goods_receipts WHERE supplier_id = $1 AND deleted_at IS NULL)
          + (SELECT count(*) FROM purchase_returns WHERE supplier_id = $1 AND deleted_at IS NULL)
            AS "n!"
        "#,
        supplier_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(n)
}

/// 明細引用商品 `product_id` 的未結案採購單（`confirmed`、`partially_received`，未刪除）張數。
pub async fn count_open_orders_by_product<'e>(
    executor: impl PgExecutor<'e>,
    product_id: Uuid,
) -> Result<i64, DbError> {
    let n = sqlx::query_scalar!(
        r#"
        SELECT count(DISTINCT o.id) AS "n!"
        FROM purchase_order_lines l
        JOIN purchase_orders o ON o.id = l.purchase_order_id
        WHERE l.product_id = $1
          AND l.deleted_at IS NULL
          AND o.deleted_at IS NULL
          AND o.status IN ('confirmed', 'partially_received')
        "#,
        product_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(n)
}
