//! 盤點與調整單明細（`stock_adjustment_lines`，docs/architecture.md §8.10）在過帳、作廢時的讀寫。
//!
//! 一般的新增、修改走通用資料 API（`system_record`）；這裡只有過帳、作廢需要的、不是欄位的值
//! （`lot_id`）與系統寫入的值（自動產生的 `lot_no`、既有批號的 `unit_cost_twd`）。
//! 所有函式都在呼叫端的 transaction 內執行（傳入 `&mut *tx`）。
//!
//! - [`list_lines_for_update`]：單據的未刪除明細，依 `sort_order, id` 鎖定（`FOR UPDATE`）
//! - [`update_line_posting`]：寫入過帳的結果，`version` + 1

use rust_decimal::Decimal;
use sqlx::PgExecutor;
use uuid::Uuid;

use crate::DbError;

/// 一列盤點調整明細的系統欄位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdjustmentLineRow {
    pub id: Uuid,
    pub sort_order: i32,
    pub product_id: Uuid,
    /// `gain_new` / `gain_existing` / `loss`。
    pub line_type: String,
    pub lot_no: Option<String>,
    pub qty: Decimal,
    pub unit_cost_twd: Option<Decimal>,
    pub lot_id: Option<Uuid>,
}

/// 單據 `stock_adjustment_id` 的未刪除明細，依 `sort_order, id` 排序並鎖定（`FOR UPDATE`）。
/// 呼叫端必須已先鎖定單據主表。
pub async fn list_lines_for_update<'e>(
    executor: impl PgExecutor<'e>,
    stock_adjustment_id: Uuid,
) -> Result<Vec<AdjustmentLineRow>, DbError> {
    let rows = sqlx::query_as!(
        AdjustmentLineRow,
        r#"
        SELECT id, sort_order, product_id, line_type, lot_no, qty, unit_cost_twd, lot_id
        FROM stock_adjustment_lines
        WHERE stock_adjustment_id = $1 AND deleted_at IS NULL
        ORDER BY sort_order, id
        FOR UPDATE
        "#,
        stock_adjustment_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 寫入過帳的結果：`lot_no`、`unit_cost_twd`、`lot_id`，`version` + 1。回傳是否有更新。
pub async fn update_line_posting<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    lot_no: Option<&str>,
    unit_cost_twd: Option<Decimal>,
    lot_id: Option<Uuid>,
    updated_by: Option<Uuid>,
) -> Result<bool, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE stock_adjustment_lines
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
    Ok(result.rows_affected() > 0)
}
