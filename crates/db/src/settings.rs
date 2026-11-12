//! `settings` 資料表的存取（docs/architecture.md §8.4）。
//!
//! 這裡只回傳原始的 key 與 JSON 值；轉成型別化設定（例如 `SecuritySettings`）
//! 與套用預設值是 `core` 的責任。

use sqlx::PgExecutor;

use crate::DbError;

/// 讀取 key 以 `prefix` 開頭的所有設定，例如 `"security."`。依 key 排序。
pub async fn get_settings_by_prefix<'e>(
    executor: impl PgExecutor<'e>,
    prefix: &str,
) -> Result<Vec<(String, serde_json::Value)>, DbError> {
    let rows = sqlx::query!(
        r#"
        SELECT key, value
        FROM settings
        WHERE starts_with(key, $1)
        ORDER BY key
        "#,
        prefix,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(|r| (r.key, r.value)).collect())
}
