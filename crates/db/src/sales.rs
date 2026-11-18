//! 銷貨模組（訂單、出貨單、銷貨退回，docs/architecture.md §8.9）的專屬讀取。
//!
//! 一般的新增、修改走通用資料 API（`system_record`）；這裡是存檔時需要的、跨單據的讀取。
//! 所有函式都在呼叫端的 transaction 內執行（傳入 `&mut *tx`）。
//!
//! - [`find_shipment_for_return`]：銷貨退回帶入表頭用的出貨單（`FOR SHARE`）
//! - [`list_shipment_lines_for_return`]：銷貨退回帶入明細用的出貨明細

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
