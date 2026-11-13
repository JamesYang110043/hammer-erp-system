//! `users` 資料表的存取（docs/architecture.md §6、§8.4）。
//!
//! 目前只有認證需要的查詢與更新：
//!
//! - [`find_auth_user`]：依 id 取得使用者狀態、角色與權限集合
//! - [`find_login_user_for_update`]、[`update_login_attempts`]、[`record_login_success`]：登入
//! - [`find_password_hash_for_update`]、[`update_password`]：修改密碼
//! - [`any_user_exists`]、[`insert_user`]：初次啟動建立管理者、建立帳號
//! - [`list_users`]、[`find_user`]、[`find_user_for_update`]、[`update_user`]、
//!   [`soft_delete_user`]、[`reset_password`]、[`unlock_user`]、[`count_active_admins`]：帳號管理
//!
//! 登入失敗次數、鎖定與最後登入時間屬於系統自動維護的狀態，更新時**不**遞增 `version`，
//! 避免管理者編輯帳號時因為使用者登入而發生樂觀鎖定衝突；修改密碼則會遞增。

use sqlx::PgExecutor;
use sqlx::postgres::PgQueryResult;
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

/// 是否有任何使用者（含已停用、已刪除）。初次啟動判斷用。
pub async fn any_user_exists<'e>(executor: impl PgExecutor<'e>) -> Result<bool, DbError> {
    let exists = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM users) AS "exists!""#)
        .fetch_one(executor)
        .await?;
    Ok(exists)
}

/// 新增使用者所需的資料。
#[derive(Debug, Clone)]
pub struct NewUser<'a> {
    pub id: Uuid,
    pub username: &'a str,
    pub display_name: &'a str,
    pub password_hash: &'a str,
    /// 角色代碼（`roles.code`），對應 `hammer_shared::role`。
    pub role_code: &'a str,
    pub must_change_password: bool,
    /// 建立者；初次啟動建立第一個管理者時為 `None`。
    pub created_by: Option<Uuid>,
    pub created_at: OffsetDateTime,
}

/// `users.username` 的 UNIQUE 限制名稱，用 [`DbError::unique_violation`] 判斷帳號重複。
pub const USERNAME_UNIQUE: &str = "users_username_key";

/// 新增一個啟用中的使用者。角色代碼不存在時回傳 `false`。
///
/// 帳號重複時回傳違反 UNIQUE 限制（[`USERNAME_UNIQUE`]）的 [`DbError::Sqlx`]。
pub async fn insert_user<'e>(
    executor: impl PgExecutor<'e>,
    new: &NewUser<'_>,
) -> Result<bool, DbError> {
    let result = sqlx::query!(
        r#"
        INSERT INTO users
            (id, username, display_name, password_hash, role_id, must_change_password,
             created_at, created_by, updated_at, updated_by)
        SELECT $1, $2, $3, $4, r.id, $6, $7, $8, $7, $8
        FROM roles r
        WHERE r.code = $5
        "#,
        new.id,
        new.username,
        new.display_name,
        new.password_hash,
        new.role_code,
        new.must_change_password,
        new.created_at,
        new.created_by,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// 帳號管理用的使用者資料（不含密碼雜湊）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRow {
    pub id: Uuid,
    pub username: String,
    pub display_name: String,
    /// 角色代碼（`roles.code`）。
    pub role_code: String,
    /// 角色顯示名稱（`roles.name`）。
    pub role_name: String,
    pub is_active: bool,
    pub must_change_password: bool,
    pub failed_login_count: i32,
    pub locked_until: Option<OffsetDateTime>,
    pub last_login_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub version: i32,
}

/// 列出未刪除的使用者，依帳號排序。
pub async fn list_users<'e>(executor: impl PgExecutor<'e>) -> Result<Vec<UserRow>, DbError> {
    let rows = sqlx::query_as!(
        UserRow,
        r#"
        SELECT
            u.id, u.username, u.display_name, r.code AS role_code, r.name AS role_name,
            u.is_active, u.must_change_password, u.failed_login_count, u.locked_until,
            u.last_login_at, u.created_at, u.updated_at, u.version
        FROM users u
        JOIN roles r ON r.id = u.role_id
        WHERE u.deleted_at IS NULL
        ORDER BY u.username
        "#,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// 依 id 查詢未刪除的使用者。
pub async fn find_user<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
) -> Result<Option<UserRow>, DbError> {
    let row = sqlx::query_as!(
        UserRow,
        r#"
        SELECT
            u.id, u.username, u.display_name, r.code AS role_code, r.name AS role_name,
            u.is_active, u.must_change_password, u.failed_login_count, u.locked_until,
            u.last_login_at, u.created_at, u.updated_at, u.version
        FROM users u
        JOIN roles r ON r.id = u.role_id
        WHERE u.id = $1 AND u.deleted_at IS NULL
        "#,
        id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 依 id 查詢未刪除的使用者，並鎖定該列（`FOR UPDATE OF u`）。必須在 transaction 內呼叫。
pub async fn find_user_for_update<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
) -> Result<Option<UserRow>, DbError> {
    let row = sqlx::query_as!(
        UserRow,
        r#"
        SELECT
            u.id, u.username, u.display_name, r.code AS role_code, r.name AS role_name,
            u.is_active, u.must_change_password, u.failed_login_count, u.locked_until,
            u.last_login_at, u.created_at, u.updated_at, u.version
        FROM users u
        JOIN roles r ON r.id = u.role_id
        WHERE u.id = $1 AND u.deleted_at IS NULL
        FOR UPDATE OF u
        "#,
        id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 修改帳號的資料。三個欄位都寫入（呼叫端先合併未修改的欄位）。
#[derive(Debug, Clone)]
pub struct UserUpdate<'a> {
    pub id: Uuid,
    /// 呼叫端讀到的版本號（§9.4）。
    pub version: i32,
    pub display_name: &'a str,
    /// 角色代碼（`roles.code`），必須存在。
    pub role_code: &'a str,
    pub is_active: bool,
    pub updated_by: Uuid,
}

/// 修改顯示名稱、角色、是否停用，並遞增 `version`。
///
/// 帶版本條件（`WHERE version = $2`），把結果交給 [`crate::ensure_version_matched`]；
/// 已刪除的使用者也會影響 0 筆。
pub async fn update_user<'e>(
    executor: impl PgExecutor<'e>,
    update: &UserUpdate<'_>,
) -> Result<PgQueryResult, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE users
        SET display_name = $3,
            role_id = (SELECT id FROM roles WHERE code = $4),
            is_active = $5,
            updated_by = $6,
            version = version + 1
        WHERE id = $1 AND version = $2 AND deleted_at IS NULL
        "#,
        update.id,
        update.version,
        update.display_name,
        update.role_code,
        update.is_active,
        update.updated_by,
    )
    .execute(executor)
    .await?;
    Ok(result)
}

/// 軟刪除（設定 `deleted_at`），並遞增 `version`。帶版本條件，同 [`update_user`]。
pub async fn soft_delete_user<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    version: i32,
    deleted_at: OffsetDateTime,
    deleted_by: Uuid,
) -> Result<PgQueryResult, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE users
        SET deleted_at = $3,
            updated_by = $4,
            version = version + 1
        WHERE id = $1 AND version = $2 AND deleted_at IS NULL
        "#,
        id,
        version,
        deleted_at,
        deleted_by,
    )
    .execute(executor)
    .await?;
    Ok(result)
}

/// 管理者重設密碼：更新密碼雜湊、`must_change_password = true`、解除鎖定，並遞增 `version`。
///
/// 回傳是否有更新到資料（已刪除的使用者不會更新）。
pub async fn reset_password<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    password_hash: &str,
    updated_by: Uuid,
) -> Result<bool, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE users
        SET password_hash = $2,
            must_change_password = true,
            failed_login_count = 0,
            locked_until = NULL,
            updated_by = $3,
            version = version + 1
        WHERE id = $1 AND deleted_at IS NULL
        "#,
        id,
        password_hash,
        updated_by,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// 解除鎖定：清除登入失敗次數與鎖定時間。
///
/// 與登入失敗的更新相同，屬於系統狀態，**不**遞增 `version`。
/// 回傳是否有更新到資料（已刪除的使用者不會更新）。
pub async fn unlock_user<'e>(executor: impl PgExecutor<'e>, id: Uuid) -> Result<bool, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE users
        SET failed_login_count = 0, locked_until = NULL
        WHERE id = $1 AND deleted_at IS NULL
        "#,
        id,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// 有效（啟用且未刪除）的管理者人數（§6.2：至少保留一個）。
///
/// 鎖定中的管理者仍算有效：鎖定會自動解除，也可以由其他管理者解除。
pub async fn count_active_admins<'e>(
    executor: impl PgExecutor<'e>,
    admin_role_code: &str,
) -> Result<i64, DbError> {
    let count = sqlx::query_scalar!(
        r#"
        SELECT count(*) AS "count!"
        FROM users u
        JOIN roles r ON r.id = u.role_id
        WHERE r.code = $1 AND u.is_active AND u.deleted_at IS NULL
        "#,
        admin_role_code,
    )
    .fetch_one(executor)
    .await?;
    Ok(count)
}
