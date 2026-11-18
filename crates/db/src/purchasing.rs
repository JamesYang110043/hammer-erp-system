//! 進貨模組（採購單、進貨單、進貨退回，docs/architecture.md §8.8）的專屬讀取。
//!
//! 一般的新增、修改走通用資料 API（`system_record`）；這裡是存檔時需要的、跨單據的讀取。
//! 所有函式都在呼叫端的 transaction 內執行（傳入 `&mut *tx`）。
//!
//! - [`find_receipt_for_return`]：進貨退回帶入表頭用的進貨單（`FOR SHARE`）
//! - [`list_receipt_lines_for_return`]：進貨退回帶入明細用的進貨明細

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
