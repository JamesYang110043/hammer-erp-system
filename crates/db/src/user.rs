//! `users` 資料表的存取（docs/architecture.md §6、§8.4）。
//!
//! 目前只有認證需要的查詢與更新：
//!
//! - [`find_auth_user`]：依 id 取得使用者狀態、角色與權限集合
//! - [`find_login_user_for_update`]、[`update_login_attempts`]、[`record_login_success`]：登入
//! - [`find_password_hash_for_update`]、[`update_password`]：修改密碼
//!
//! 帳號管理（新增、修改、停用）在之後的 issue 加入。
//!
//! 登入失敗次數、鎖定與最後登入時間屬於系統自動維護的狀態，更新時**不**遞增 `version`，
//! 避免管理者編輯帳號時因為使用者登入而發生樂觀鎖定衝突；修改密碼則會遞增。

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

/// 登入時需要的使用者資料。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginUserRow {
    pub id: Uuid,
    pub password_hash: String,
    pub is_active: bool,
    pub failed_login_count: i32,
    pub locked_until: Option<OffsetDateTime>,
}

/// 依帳號查詢登入用的資料，並鎖定該列（`FOR UPDATE`），避免同時登入失敗時次數互相覆蓋。
/// 已刪除的使用者視為不存在。必須在 transaction 內呼叫。
pub async fn find_login_user_for_update<'e>(
    executor: impl PgExecutor<'e>,
    username: &str,
) -> Result<Option<LoginUserRow>, DbError> {
    let row = sqlx::query_as!(
        LoginUserRow,
        r#"
        SELECT id, password_hash, is_active, failed_login_count, locked_until
        FROM users
        WHERE username = $1 AND deleted_at IS NULL
        FOR UPDATE
        "#,
        username,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 更新登入失敗次數與鎖定時間。
pub async fn update_login_attempts<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    failed_login_count: i32,
    locked_until: Option<OffsetDateTime>,
) -> Result<(), DbError> {
    sqlx::query!(
        r#"
        UPDATE users
        SET failed_login_count = $2, locked_until = $3
        WHERE id = $1
        "#,
        id,
        failed_login_count,
        locked_until,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// 登入成功：清除失敗次數與鎖定，更新 `last_login_at`。
pub async fn record_login_success<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    at: OffsetDateTime,
) -> Result<(), DbError> {
    sqlx::query!(
        r#"
        UPDATE users
        SET failed_login_count = 0, locked_until = NULL, last_login_at = $2
        WHERE id = $1
        "#,
        id,
        at,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// 取得使用者的密碼雜湊並鎖定該列。已刪除的使用者回傳 `None`。必須在 transaction 內呼叫。
pub async fn find_password_hash_for_update<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
) -> Result<Option<String>, DbError> {
    let hash = sqlx::query_scalar!(
        r#"
        SELECT password_hash
        FROM users
        WHERE id = $1 AND deleted_at IS NULL
        FOR UPDATE
        "#,
        id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(hash)
}

/// 更新密碼雜湊，清除 `must_change_password`，並遞增 `version`。
pub async fn update_password<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    password_hash: &str,
    updated_by: Uuid,
) -> Result<(), DbError> {
    sqlx::query!(
        r#"
        UPDATE users
        SET password_hash = $2,
            must_change_password = false,
            updated_by = $3,
            version = version + 1
        WHERE id = $1
        "#,
        id,
        password_hash,
        updated_by,
    )
    .execute(executor)
    .await?;
    Ok(())
}
