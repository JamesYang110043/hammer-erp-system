//! `roles` 與 `role_permissions` 資料表的查詢（docs/architecture.md §6.1、§8.4）。
//!
//! 第一版只有三種內建角色（migration 寫入），不提供新增或修改。

use sqlx::PgExecutor;
use uuid::Uuid;

use crate::DbError;

/// 角色與其權限。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleRow {
    pub id: Uuid,
    /// 角色代碼，對應 `hammer_shared::role`。
    pub code: String,
    /// 顯示名稱，例如「管理者」。
    pub name: String,
    /// 權限代碼，依代碼排序。
    pub permissions: Vec<String>,
}

/// 列出所有角色與權限，依 `roles.id` 排序（migration 依管理者、可編輯者、唯讀者的順序建立）。
pub async fn list_roles<'e>(executor: impl PgExecutor<'e>) -> Result<Vec<RoleRow>, DbError> {
    let rows = sqlx::query_as!(
        RoleRow,
        r#"
        SELECT
            r.id,
            r.code,
            r.name,
            COALESCE(
                array_agg(rp.permission ORDER BY rp.permission)
                    FILTER (WHERE rp.permission IS NOT NULL),
                '{}'
            ) AS "permissions!"
        FROM roles r
        LEFT JOIN role_permissions rp ON rp.role_id = r.id
        GROUP BY r.id
        ORDER BY r.id
        "#,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}
