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
//! - 進貨單（採購單轉進貨單、過帳、作廢）：[`find_order_for_receipt`]、[`lock_order_for_receipt`]、
//!   [`list_order_lines`]、[`list_receipt_lines_for_update`]、[`update_receipt_line_posting`]、
//!   [`add_received_qty`]、[`set_order_status`]、[`posted_returns_of_receipt`]
//! - 進貨退回（退回數量上限、過帳）：[`lock_receipt_for_return`]、[`returned_qty_of_lines`]、
//!   [`list_return_lines_for_update`]、[`update_return_line_posting`]

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
    pub receipt_date: time::Date,
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
        SELECT id, status, supplier_id, currency_code, exchange_rate, tax_type, receipt_date
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

/// 同 [`find_receipt_for_return`]，以 `FOR UPDATE` 鎖定（進貨退回過帳時；同一張進貨單的退回依序過帳）。
pub async fn lock_receipt_for_return<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
) -> Result<Option<ReceiptForReturn>, DbError> {
    let row = sqlx::query_as!(
        ReceiptForReturn,
        r#"
        SELECT id, status, supplier_id, currency_code, exchange_rate, tax_type, receipt_date
        FROM goods_receipts
        WHERE id = $1 AND deleted_at IS NULL
        FOR UPDATE
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
    pub qty: Decimal,
    /// 過帳時寫入的台幣單位成本。
    pub unit_cost_twd: Option<Decimal>,
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
        SELECT id, product_id, lot_no, unit_price, lot_id, qty, unit_cost_twd
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

/// 進貨單引用的採購單。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderForReceipt {
    pub id: Uuid,
    pub record_no: String,
    /// `draft` / `confirmed` / `partially_received` / `closed` / `void`。
    pub status: String,
    pub supplier_id: Uuid,
    pub currency_code: String,
    pub tax_type: String,
    /// 是否為手動結案（`closed_at` 有值）。
    pub closed_manually: bool,
    pub version: i32,
}

/// 未刪除的採購單（不鎖定；存檔時的檢查與帶入）。不存在或已刪除時為 `None`。
pub async fn find_order_for_receipt<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
) -> Result<Option<OrderForReceipt>, DbError> {
    let row = sqlx::query_as!(
        OrderForReceipt,
        r#"
        SELECT id, record_no, status, supplier_id, currency_code, tax_type,
               closed_at IS NOT NULL AS "closed_manually!", version
        FROM purchase_orders
        WHERE id = $1 AND deleted_at IS NULL
        "#,
        id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 同 [`find_order_for_receipt`]，並鎖定採購單（`FOR UPDATE`；進貨單過帳、作廢時）。
pub async fn lock_order_for_receipt<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
) -> Result<Option<OrderForReceipt>, DbError> {
    let row = sqlx::query_as!(
        OrderForReceipt,
        r#"
        SELECT id, record_no, status, supplier_id, currency_code, tax_type,
               closed_at IS NOT NULL AS "closed_manually!", version
        FROM purchase_orders
        WHERE id = $1 AND deleted_at IS NULL
        FOR UPDATE
        "#,
        id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 採購單的未刪除明細，依 `sort_order, id` 排序（不鎖定；存檔時的檢查）。
pub async fn list_order_lines<'e>(
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
        "#,
        purchase_order_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 一列進貨明細。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptLineRow {
    pub id: Uuid,
    pub sort_order: i32,
    pub purchase_order_line_id: Option<Uuid>,
    pub product_id: Uuid,
    pub qty: Decimal,
    pub unit_price: Decimal,
    pub lot_no: Option<String>,
    pub unit_cost_twd: Option<Decimal>,
    pub lot_id: Option<Uuid>,
}

/// 進貨單 `goods_receipt_id` 的未刪除明細，依 `sort_order, id` 排序並鎖定（`FOR UPDATE`）。
/// 呼叫端必須已先鎖定進貨單。
pub async fn list_receipt_lines_for_update<'e>(
    executor: impl PgExecutor<'e>,
    goods_receipt_id: Uuid,
) -> Result<Vec<ReceiptLineRow>, DbError> {
    let rows = sqlx::query_as!(
        ReceiptLineRow,
        r#"
        SELECT id, sort_order, purchase_order_line_id, product_id, qty, unit_price, lot_no,
               unit_cost_twd, lot_id
        FROM goods_receipt_lines
        WHERE goods_receipt_id = $1 AND deleted_at IS NULL
        ORDER BY sort_order, id
        FOR UPDATE
        "#,
        goods_receipt_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 寫入進貨明細過帳的結果：`lot_no`、`unit_cost_twd`、`lot_id`，`version` + 1。回傳是否有更新。
pub async fn update_receipt_line_posting<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    lot_no: Option<&str>,
    unit_cost_twd: Decimal,
    lot_id: Option<Uuid>,
    updated_by: Uuid,
) -> Result<bool, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE goods_receipt_lines
        SET lot_no = $2, unit_cost_twd = $3, lot_id = $4, updated_by = $5, version = version + 1
        WHERE id = $1 AND deleted_at IS NULL
        "#,
        id,
        lot_no,
        unit_cost_twd,
        lot_id,
        updated_by,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// 採購明細的 `received_qty += delta`（作廢時為負數），`version` + 1。回傳是否有更新。
pub async fn add_received_qty<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    delta: Decimal,
    updated_by: Uuid,
) -> Result<bool, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE purchase_order_lines
        SET received_qty = received_qty + $2, updated_by = $3, version = version + 1
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

/// 採購單的狀態改為 `status`（進貨過帳、作廢後重新計算的結果），`version` + 1。
pub async fn set_order_status<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    status: &str,
    updated_by: Uuid,
) -> Result<bool, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE purchase_orders
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

/// 指向進貨單、已過帳且未刪除的進貨退回 `(id, record_no)`，依單號排序。
pub async fn posted_returns_of_receipt<'e>(
    executor: impl PgExecutor<'e>,
    goods_receipt_id: Uuid,
) -> Result<Vec<(Uuid, String)>, DbError> {
    let rows = sqlx::query!(
        r#"
        SELECT id, record_no
        FROM purchase_returns
        WHERE goods_receipt_id = $1 AND status = 'posted' AND deleted_at IS NULL
        ORDER BY record_no
        "#,
        goods_receipt_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.record_no)).collect())
}

/// 進貨明細 `ids` 在**已過帳**、未刪除的進貨退回中的退回數量合計 `(進貨明細 id, 合計)`；沒有退回的不列出。
pub async fn returned_qty_of_lines<'e>(
    executor: impl PgExecutor<'e>,
    ids: &[Uuid],
) -> Result<Vec<(Uuid, Decimal)>, DbError> {
    let rows = sqlx::query!(
        r#"
        SELECT l.goods_receipt_line_id AS "id!", sum(l.qty) AS "qty!"
        FROM purchase_return_lines l
        JOIN purchase_returns r ON r.id = l.purchase_return_id
        WHERE l.goods_receipt_line_id = ANY($1)
          AND l.deleted_at IS NULL AND r.deleted_at IS NULL AND r.status = 'posted'
        GROUP BY l.goods_receipt_line_id
        "#,
        ids,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.qty)).collect())
}

/// 一列進貨退回明細。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReturnLineRow {
    pub id: Uuid,
    pub sort_order: i32,
    pub goods_receipt_line_id: Uuid,
    pub product_id: Uuid,
    pub qty: Decimal,
    pub unit_cost_twd: Option<Decimal>,
    pub lot_id: Option<Uuid>,
}

/// 進貨退回 `purchase_return_id` 的未刪除明細，依 `sort_order, id` 排序並鎖定（`FOR UPDATE`）。
pub async fn list_return_lines_for_update<'e>(
    executor: impl PgExecutor<'e>,
    purchase_return_id: Uuid,
) -> Result<Vec<ReturnLineRow>, DbError> {
    let rows = sqlx::query_as!(
        ReturnLineRow,
        r#"
        SELECT id, sort_order, goods_receipt_line_id, product_id, qty, unit_cost_twd, lot_id
        FROM purchase_return_lines
        WHERE purchase_return_id = $1 AND deleted_at IS NULL
        ORDER BY sort_order, id
        FOR UPDATE
        "#,
        purchase_return_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 寫入進貨退回明細過帳的結果：`unit_cost_twd`、`lot_id`，`version` + 1。回傳是否有更新。
pub async fn update_return_line_posting<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    unit_cost_twd: Decimal,
    lot_id: Option<Uuid>,
    updated_by: Uuid,
) -> Result<bool, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE purchase_return_lines
        SET unit_cost_twd = $2, lot_id = $3, updated_by = $4, version = version + 1
        WHERE id = $1 AND deleted_at IS NULL
        "#,
        id,
        unit_cost_twd,
        lot_id,
        updated_by,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}
