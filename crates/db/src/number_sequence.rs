//! 單號流水號（`number_sequences`，docs/architecture.md §8.4）。
//!
//! 單號規則的解析與期間的計算在 `hammer_core::numbering`，這裡只負責取號。

use sqlx::PgExecutor;
use uuid::Uuid;

use crate::DbError;

/// 取得資料類別在某個期間的下一個流水號（從 1 開始），並鎖定該列直到 transaction 結束。
/// 必須在新增資料的同一個 transaction 內呼叫。
///
/// 以 `INSERT … ON CONFLICT DO UPDATE` 一次完成「沒有該期間的列時新增、有則遞增」：
/// 已有的列被鎖定（效果與 `SELECT … FOR UPDATE` 後再更新相同），其他 transaction 要等這個
/// transaction 結束才能取號，因此不會取到重複的號碼；transaction rollback 時號碼也一併還原。
/// 兩個 transaction 同時為新的期間新增列時，後者會等前者結束後改走遞增，不會違反唯一限制。
///
/// `new_id` 只在新增列時使用。
pub async fn next_value<'e>(
    executor: impl PgExecutor<'e>,
    entity_id: Uuid,
    period_key: &str,
    new_id: Uuid,
) -> Result<i32, DbError> {
    let value = sqlx::query_scalar!(
        r#"
        INSERT INTO number_sequences (id, entity_id, period_key, current_value)
        VALUES ($1, $2, $3, 1)
        ON CONFLICT (entity_id, period_key)
            DO UPDATE SET current_value = number_sequences.current_value + 1
        RETURNING current_value
        "#,
        new_id,
        entity_id,
        period_key,
    )
    .fetch_one(executor)
    .await?;
    Ok(value)
}
