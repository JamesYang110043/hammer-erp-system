//! `users` 資料表的存取（docs/architecture.md §6、§8.4）。
//!
//! 目前只有認證需要的查詢：依 id 取得使用者狀態、角色與權限集合。
//! 帳號管理（新增、修改、停用）在之後的 issue 加入。

use sqlx::PgExecutor;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::DbError;

/// 認證時需要的使用者資料：狀態、角色與權限。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthUserRow {
    pub id: Uuid,
    pub username: String,
    pub display_name: String,
    pub is_active: bool,
    pub must_change_password: bool,
    pub deleted_at: Option<OffsetDateTime>,
    /// 角色代碼（`roles.code`），對應 `hammer_shared::role`。
    pub role_code: String,
    /// 角色擁有的權限代碼（`role_permissions.permission`），依代碼排序。
    pub permissions: Vec<String>,
}

/// 依 id 查詢使用者與其角色、權限。已停用或已刪除的也會回傳，是否可用由呼叫端判斷。
pub async fn find_auth_user<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
) -> Result<Option<AuthUserRow>, DbError> {
    let row = sqlx::query_as!(
        AuthUserRow,
        r#"
        SELECT
            u.id,
            u.username,
            u.display_name,
            u.is_active,
            u.must_change_password,
            u.deleted_at,
            r.code AS role_code,
            COALESCE(
                array_agg(rp.permission ORDER BY rp.permission)
                    FILTER (WHERE rp.permission IS NOT NULL),
                '{}'
            ) AS "permissions!"
        FROM users u
        JOIN roles r ON r.id = u.role_id
        LEFT JOIN role_permissions rp ON rp.role_id = r.id
        WHERE u.id = $1
        GROUP BY u.id, r.code
        "#,
        id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}
