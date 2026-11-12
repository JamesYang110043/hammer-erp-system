//! `audit_logs` 資料表的寫入（docs/architecture.md §8.4）。
//!
//! 這裡只負責寫入資料列。動作代碼檢查、diff 與敏感欄位排除在 `hammer_core::audit`。
//! 時間由呼叫端傳入，不使用資料庫的 `now()`。

use serde_json::Value;
use sqlx::PgExecutor;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::DbError;

/// 新增操作紀錄所需的資料。
#[derive(Debug, Clone)]
pub struct NewAuditLog<'a> {
    pub id: Uuid,
    pub occurred_at: OffsetDateTime,
    /// 操作者；帳號不存在的登入失敗為 `None`。
    pub user_id: Option<Uuid>,
    pub action: &'a str,
    pub entity_key: Option<&'a str>,
    pub record_id: Option<Uuid>,
    pub before_data: Option<&'a Value>,
    pub after_data: Option<&'a Value>,
    pub client_info: Option<&'a str>,
}

/// 新增一筆操作紀錄。
pub async fn insert_audit_log<'e>(
    executor: impl PgExecutor<'e>,
    new: &NewAuditLog<'_>,
) -> Result<(), DbError> {
    sqlx::query!(
        r#"
        INSERT INTO audit_logs
            (id, occurred_at, user_id, action, entity_key, record_id, before_data, after_data, client_info)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
        new.id,
        new.occurred_at,
        new.user_id,
        new.action,
        new.entity_key,
        new.record_id,
        new.before_data,
        new.after_data,
        new.client_info,
    )
    .execute(executor)
    .await?;
    Ok(())
}
