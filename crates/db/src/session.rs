//! `sessions` 資料表的存取（docs/architecture.md §6.2、§8.4）。
//!
//! 這裡只負責讀寫資料列。token 的產生與雜湊、session 是否有效的判斷
//! 在 `hammer_core::auth`。資料庫只保存 token 的 SHA-256（`token_hash`），
//! 不保存 token 本身。
//!
//! 所有時間都由呼叫端傳入，不使用資料庫的 `now()`，方便測試與保持時間來源一致。

use sqlx::PgExecutor;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::DbError;

/// `sessions` 的一筆資料。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub token_hash: String,
    pub client_info: Option<String>,
    pub created_at: OffsetDateTime,
    pub last_seen_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    pub revoked_at: Option<OffsetDateTime>,
}

/// 新增 session 所需的資料。`last_seen_at` 與 `created_at` 相同。
#[derive(Debug, Clone)]
pub struct NewSession<'a> {
    pub id: Uuid,
    pub user_id: Uuid,
    pub token_hash: &'a str,
    pub client_info: Option<&'a str>,
    pub created_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
}

/// 新增一筆 session。
pub async fn insert_session<'e>(
    executor: impl PgExecutor<'e>,
    new: &NewSession<'_>,
) -> Result<SessionRow, DbError> {
    let row = sqlx::query_as!(
        SessionRow,
        r#"
        INSERT INTO sessions (id, user_id, token_hash, client_info, created_at, last_seen_at, expires_at)
        VALUES ($1, $2, $3, $4, $5, $5, $6)
        RETURNING id, user_id, token_hash, client_info, created_at, last_seen_at, expires_at, revoked_at
        "#,
        new.id,
        new.user_id,
        new.token_hash,
        new.client_info,
        new.created_at,
        new.expires_at,
    )
    .fetch_one(executor)
    .await?;
    Ok(row)
}

/// 依 token 雜湊查詢 session。已撤銷或過期的也會回傳，是否有效由呼叫端判斷。
pub async fn find_session_by_token_hash<'e>(
    executor: impl PgExecutor<'e>,
    token_hash: &str,
) -> Result<Option<SessionRow>, DbError> {
    let row = sqlx::query_as!(
        SessionRow,
        r#"
        SELECT id, user_id, token_hash, client_info, created_at, last_seen_at, expires_at, revoked_at
        FROM sessions
        WHERE token_hash = $1
        "#,
        token_hash,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// 更新 session 的最後使用時間與到期時間（閒置期限隨使用延長）。
///
/// 已撤銷的 session 不會被更新。回傳是否有更新到資料。
pub async fn touch_session<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    last_seen_at: OffsetDateTime,
    expires_at: OffsetDateTime,
) -> Result<bool, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE sessions
        SET last_seen_at = $2, expires_at = $3
        WHERE id = $1 AND revoked_at IS NULL
        "#,
        id,
        last_seen_at,
        expires_at,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// 撤銷單一 session（登出）。已撤銷的不會改變撤銷時間。回傳是否有更新到資料。
pub async fn revoke_session<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    revoked_at: OffsetDateTime,
) -> Result<bool, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE sessions
        SET revoked_at = $2
        WHERE id = $1 AND revoked_at IS NULL
        "#,
        id,
        revoked_at,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// 撤銷某使用者所有尚未撤銷的 session（強制登出、改密碼、停用帳號）。
/// 回傳撤銷的筆數。
pub async fn revoke_user_sessions<'e>(
    executor: impl PgExecutor<'e>,
    user_id: Uuid,
    revoked_at: OffsetDateTime,
) -> Result<u64, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE sessions
        SET revoked_at = $2
        WHERE user_id = $1 AND revoked_at IS NULL
        "#,
        user_id,
        revoked_at,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

/// 撤銷某使用者除了 `keep_id` 以外所有尚未撤銷的 session（修改密碼後保留目前的 session）。
/// 回傳撤銷的筆數。
pub async fn revoke_other_user_sessions<'e>(
    executor: impl PgExecutor<'e>,
    user_id: Uuid,
    keep_id: Uuid,
    revoked_at: OffsetDateTime,
) -> Result<u64, DbError> {
    let result = sqlx::query!(
        r#"
        UPDATE sessions
        SET revoked_at = $3
        WHERE user_id = $1 AND id <> $2 AND revoked_at IS NULL
        "#,
        user_id,
        keep_id,
        revoked_at,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}
