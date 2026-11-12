//! PostgreSQL advisory lock。
//!
//! 用於沒有特定資料列可以 `FOR UPDATE` 的情況，例如「資料表還沒有任何資料時才可以新增」。
//! 鍵值由呼叫端定義，每種用途使用不同的常數。

use sqlx::PgExecutor;

use crate::DbError;

/// 取得 transaction 層級的 advisory lock（`pg_advisory_xact_lock`）。
///
/// 其他持有相同鍵值的 transaction 會等到這個 transaction 結束（commit 或 rollback）
/// 才取得鎖，不需要手動釋放。必須在 transaction 內呼叫。
pub async fn advisory_xact_lock<'e>(
    executor: impl PgExecutor<'e>,
    key: i64,
) -> Result<(), DbError> {
    sqlx::query!("SELECT pg_advisory_xact_lock($1)", key)
        .execute(executor)
        .await?;
    Ok(())
}
