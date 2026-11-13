//! `audit_logs` 資料表的寫入與查詢（docs/architecture.md §8.4）。
//!
//! - [`insert_audit_log`]：寫入資料列。動作代碼檢查、diff 與敏感欄位排除在 `hammer_core::audit`。
//! - [`list_audit_logs`]、[`count_audit_logs`]：依條件分頁查詢。權限與參數檢查在 `hammer_core::audit_logs`。
//!
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

/// 查詢操作紀錄的篩選條件。`None` 代表不篩選該條件。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AuditLogFilter<'a> {
    /// `occurred_at >= from`
    pub from: Option<OffsetDateTime>,
    /// `occurred_at < to`
    pub to: Option<OffsetDateTime>,
    pub user_id: Option<Uuid>,
    pub action: Option<&'a str>,
    pub entity_key: Option<&'a str>,
    pub record_id: Option<Uuid>,
}

/// 操作紀錄與操作者的帳號、顯示名稱。
#[derive(Debug, Clone, PartialEq)]
pub struct AuditLogRow {
    pub id: Uuid,
    pub occurred_at: OffsetDateTime,
    pub user_id: Option<Uuid>,
    /// 操作者的帳號（含已刪除的帳號）；沒有操作者時為 `None`。
    pub username: Option<String>,
    pub user_display_name: Option<String>,
    pub action: String,
    pub entity_key: Option<String>,
    pub record_id: Option<Uuid>,
    pub before_data: Option<Value>,
    pub after_data: Option<Value>,
    pub client_info: Option<String>,
}

/// 依條件查詢操作紀錄，依 `occurred_at` 由新到舊排序（同時間依 `id` 由新到舊，讓分頁結果穩定）。
pub async fn list_audit_logs<'e>(
    executor: impl PgExecutor<'e>,
    filter: &AuditLogFilter<'_>,
    limit: i64,
    offset: i64,
) -> Result<Vec<AuditLogRow>, DbError> {
    let rows = sqlx::query_as!(
        AuditLogRow,
        r#"
        SELECT
            a.id, a.occurred_at, a.user_id,
            u.username AS "username?", u.display_name AS "user_display_name?",
            a.action, a.entity_key, a.record_id, a.before_data, a.after_data, a.client_info
        FROM audit_logs a
        LEFT JOIN users u ON u.id = a.user_id
        WHERE ($1::timestamptz IS NULL OR a.occurred_at >= $1)
          AND ($2::timestamptz IS NULL OR a.occurred_at < $2)
          AND ($3::uuid IS NULL OR a.user_id = $3)
          AND ($4::text IS NULL OR a.action = $4)
          AND ($5::text IS NULL OR a.entity_key = $5)
          AND ($6::uuid IS NULL OR a.record_id = $6)
        ORDER BY a.occurred_at DESC, a.id DESC
        LIMIT $7 OFFSET $8
        "#,
        filter.from,
        filter.to,
        filter.user_id,
        filter.action,
        filter.entity_key,
        filter.record_id,
        limit,
        offset,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 符合條件的操作紀錄筆數。條件與 [`list_audit_logs`] 相同。
pub async fn count_audit_logs<'e>(
    executor: impl PgExecutor<'e>,
    filter: &AuditLogFilter<'_>,
) -> Result<i64, DbError> {
    let count = sqlx::query_scalar!(
        r#"
        SELECT COUNT(*) AS "count!"
        FROM audit_logs a
        WHERE ($1::timestamptz IS NULL OR a.occurred_at >= $1)
          AND ($2::timestamptz IS NULL OR a.occurred_at < $2)
          AND ($3::uuid IS NULL OR a.user_id = $3)
          AND ($4::text IS NULL OR a.action = $4)
          AND ($5::text IS NULL OR a.entity_key = $5)
          AND ($6::uuid IS NULL OR a.record_id = $6)
        "#,
        filter.from,
        filter.to,
        filter.user_id,
        filter.action,
        filter.entity_key,
        filter.record_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(count)
}
