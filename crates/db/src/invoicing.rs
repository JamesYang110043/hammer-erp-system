//! 發票模組（銷項發票、銷貨折讓、進項憑證、進貨折讓，docs/architecture.md §8.11）的專屬讀取。
//!
//! 一般的新增、修改走通用資料 API（`system_record`）；這裡是存檔時需要的、跨單據的讀取。
//! 所有函式都在呼叫端的 transaction 內執行（傳入 `&mut *tx`）。
//!
//! - [`find_sales_invoice_for_allowance`]：銷貨折讓帶入表頭用的銷項發票（`FOR SHARE`）
//! - [`find_purchase_invoice_for_allowance`]：進貨折讓帶入表頭用的進項憑證（`FOR SHARE`）

use sqlx::PgExecutor;
use uuid::Uuid;

use crate::DbError;

/// 折讓從原發票（憑證）帶入的表頭值（§8.11「銷貨折讓」「進貨折讓」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvoiceForAllowance {
    pub id: Uuid,
    /// `draft` / `posted` / `void`。
    pub status: String,
    /// 銷項發票為客戶、進項憑證為廠商。
    pub party_id: Uuid,
    pub tax_type: String,
    /// 銷項發票為 `triplicate` / `duplicate`；進項憑證為 `triplicate` / `electronic` / `customs`。
    pub doc_type: String,
}

/// 未刪除的銷項發票 `id`，以 `FOR SHARE` 鎖定（存檔期間不會被開立、作廢或刪除）。不存在或已刪除時為
/// `None`。
pub async fn find_sales_invoice_for_allowance<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
) -> Result<Option<InvoiceForAllowance>, DbError> {
    let row = sqlx::query_as!(
        InvoiceForAllowance,
        r#"
        SELECT id, status, customer_id AS party_id, tax_type, invoice_type AS doc_type
        FROM sales_invoices
        WHERE id = $1 AND deleted_at IS NULL
        FOR SHARE
        "#,
        id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 未刪除的進項憑證 `id`，以 `FOR SHARE` 鎖定。不存在或已刪除時為 `None`。
pub async fn find_purchase_invoice_for_allowance<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
) -> Result<Option<InvoiceForAllowance>, DbError> {
    let row = sqlx::query_as!(
        InvoiceForAllowance,
        r#"
        SELECT id, status, supplier_id AS party_id, tax_type, doc_type
        FROM purchase_invoices
        WHERE id = $1 AND deleted_at IS NULL
        FOR SHARE
        "#,
        id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}
