//! 登入、登出、修改密碼（docs/architecture.md §6.2）。
//!
//! 每個動作都在單一 transaction 內完成，並在同一個 transaction 寫入操作紀錄：
//!
//! - [`login`]：寫入 `login` 或 `login_failed`。失敗時失敗次數、鎖定與紀錄仍會 commit。
//! - [`logout`]：撤銷目前的 session，寫入 `logout`。
//! - [`change_password`]：更新密碼、清除 `must_change_password`、撤銷其他 session，
//!   寫入 `password_change`（不記錄密碼內容）。
//!
//! 登入判斷順序（[`decide`]）：
//!
//! 1. 帳號不存在（或已刪除）→ 仍執行一次密碼驗證以拉平回應時間 → [`CoreError::InvalidCredentials`]
//! 2. 鎖定中 → [`CoreError::AccountLocked`]（不驗證密碼、不累計失敗次數）
//! 3. 密碼錯誤 → 失敗次數 + 1，達到上限時鎖定 → [`CoreError::InvalidCredentials`]
//! 4. 帳號停用 → [`CoreError::AccountDisabled`]（密碼正確才會告知，避免透露帳號狀態）
//! 5. 成功 → 清除失敗次數、更新 `last_login_at`、建立 session

use std::sync::OnceLock;

use hammer_db::DbError;
use hammer_db::session::revoke_other_user_sessions;
use hammer_db::sqlx::{PgPool, PgTransaction};
use hammer_db::user::{
    find_auth_user, find_login_user_for_update, find_password_hash_for_update,
    record_login_success, update_login_attempts, update_password,
};
use hammer_shared::{Id, audit as action};
use serde_json::json;
use time::OffsetDateTime;

use super::{
    AuthContext, LoginAttemptState, SecuritySettings, SessionToken, hash_password, session,
    validate_password_rules, verify_password,
};
use crate::CoreError;
use crate::audit::{self, AuditDiff, AuditEntry};

/// 操作紀錄中使用者資料的資料類別代碼。
pub const USER_ENTITY_KEY: &str = "user";

/// 登入失敗的原因，寫入 `login_failed` 紀錄的 `after_data.reason`（只給管理者查看）。
pub mod failure_reason {
    /// 帳號不存在。
    pub const UNKNOWN_USER: &str = "unknown_user";
    /// 密碼錯誤。
    pub const WRONG_PASSWORD: &str = "wrong_password";
    /// 帳號鎖定中。
    pub const LOCKED: &str = "locked";
    /// 帳號已停用。
    pub const DISABLED: &str = "disabled";
}

/// 登入成功的結果。
#[derive(Debug)]
pub struct LoginOutcome {
    /// 只在這時候取得一次，回傳給客戶端。
    pub token: SessionToken,
    pub user: AuthContext,
}

/// 帳號存在時的登入判斷結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginDecision {
    /// 鎖定中，到此時間解鎖。
    Locked(OffsetDateTime),
    /// 密碼錯誤，內容為更新後的失敗狀態（可能因此鎖定）。
    WrongPassword(LoginAttemptState),
    /// 密碼正確但帳號已停用。
    Disabled,
    /// 可以登入。
    Success,
}

/// 帳號存在時的登入判斷。純邏輯，密碼驗證由 `verify` 提供；鎖定中時不會呼叫 `verify`。
pub fn decide(
    is_active: bool,
    state: LoginAttemptState,
    now: OffsetDateTime,
    settings: &SecuritySettings,
    verify: impl FnOnce() -> Result<bool, CoreError>,
) -> Result<LoginDecision, CoreError> {
    if let Some(until) = state.unlocks_at(now) {
        return Ok(LoginDecision::Locked(until));
    }
    if !verify()? {
        return Ok(LoginDecision::WrongPassword(
            state.record_failure(now, settings),
        ));
    }
    if !is_active {
        return Ok(LoginDecision::Disabled);
    }
    Ok(LoginDecision::Success)
}

/// 以帳號密碼登入。
///
/// 失敗時也會 commit 失敗次數與 `login_failed` 紀錄，再回傳錯誤。
pub async fn login(
    pool: &PgPool,
    username: &str,
    password: &str,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<LoginOutcome, CoreError> {
    let username = username.trim();
    let settings = SecuritySettings::load(pool).await?;
    let mut tx = pool.begin().await.map_err(DbError::from)?;

    let Some(user) = find_login_user_for_update(&mut *tx, username).await? else {
        // 拉平帳號存在與否的回應時間
        let _ = verify_password(password, dummy_hash());
        record_failure(
            &mut tx,
            None,
            username,
            failure_reason::UNKNOWN_USER,
            client_info,
            now,
        )
        .await?;
        commit(tx).await?;
        tracing::info!(username, "登入失敗：帳號不存在");
        return Err(CoreError::InvalidCredentials);
    };

    let state = LoginAttemptState::new(user.failed_login_count, user.locked_until);
    let decision = decide(user.is_active, state, now, &settings, || {
        verify_password(password, &user.password_hash)
    })?;

    match decision {
        LoginDecision::Locked(until) => {
            record_failure(
                &mut tx,
                Some(user.id),
                username,
                failure_reason::LOCKED,
                client_info,
                now,
            )
            .await?;
            commit(tx).await?;
            tracing::info!(user_id = %user.id, %until, "登入失敗：帳號鎖定中");
            Err(CoreError::AccountLocked(until))
        }
        LoginDecision::WrongPassword(next) => {
            update_login_attempts(
                &mut *tx,
                user.id,
                next.failed_login_count,
                next.locked_until,
            )
            .await?;
            record_failure(
                &mut tx,
                Some(user.id),
                username,
                failure_reason::WRONG_PASSWORD,
                client_info,
                now,
            )
            .await?;
            commit(tx).await?;
            tracing::info!(
                user_id = %user.id,
                locked = next.is_locked(now),
                "登入失敗：密碼錯誤"
            );
            Err(CoreError::InvalidCredentials)
        }
        LoginDecision::Disabled => {
            record_failure(
                &mut tx,
                Some(user.id),
                username,
                failure_reason::DISABLED,
                client_info,
                now,
            )
            .await?;
            commit(tx).await?;
            tracing::info!(user_id = %user.id, "登入失敗：帳號已停用");
            Err(CoreError::AccountDisabled)
        }
        LoginDecision::Success => {
            record_login_success(&mut *tx, user.id, now).await?;
            let (token, session_row) =
                session::create(&mut *tx, user.id, client_info, now, &settings).await?;
            audit::record(
                &mut tx,
                &AuditEntry {
                    user_id: Some(user.id),
                    action: action::LOGIN,
                    entity_key: None,
                    record_id: None,
                    diff: AuditDiff::default(),
                    client_info,
                    occurred_at: now,
                },
            )
            .await?;
            let row = find_auth_user(&mut *tx, user.id)
                .await?
                .ok_or_else(|| CoreError::Internal("登入後找不到使用者".into()))?;
            commit(tx).await?;
            tracing::info!(user_id = %user.id, session_id = %session_row.id, "登入成功");
            Ok(LoginOutcome {
                token,
                user: AuthContext::new(row, session_row.id),
            })
        }
    }
}

/// 登出：撤銷目前的 session。
pub async fn logout(
    pool: &PgPool,
    ctx: &AuthContext,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<(), CoreError> {
    let mut tx = pool.begin().await.map_err(DbError::from)?;
    session::revoke(&mut *tx, ctx.session_id, now).await?;
    audit::record(
        &mut tx,
        &AuditEntry {
            user_id: Some(ctx.user_id),
            action: action::LOGOUT,
            entity_key: None,
            record_id: None,
            diff: AuditDiff::default(),
            client_info,
            occurred_at: now,
        },
    )
    .await?;
    commit(tx).await?;
    tracing::info!(user_id = %ctx.user_id, session_id = %ctx.session_id, "登出");
    Ok(())
}

/// 修改自己的密碼。
///
/// - 目前密碼錯誤、新密碼不符合規則、新舊密碼相同 → [`CoreError::Validation`]
/// - 成功後清除 `must_change_password`，撤銷該使用者**其他**的 session，目前的 session 保留
pub async fn change_password(
    pool: &PgPool,
    ctx: &AuthContext,
    current_password: &str,
    new_password: &str,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<(), CoreError> {
    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let Some(hash) = find_password_hash_for_update(&mut *tx, ctx.user_id).await? else {
        return Err(CoreError::Unauthorized);
    };
    if !verify_password(current_password, &hash)? {
        return Err(CoreError::Validation("目前的密碼不正確".into()));
    }
    validate_password_rules(new_password)?;
    if new_password == current_password {
        return Err(CoreError::Validation("新密碼不可與目前的密碼相同".into()));
    }

    let new_hash = hash_password(new_password)?;
    update_password(&mut *tx, ctx.user_id, &new_hash, ctx.user_id).await?;
    let revoked = revoke_other_user_sessions(&mut *tx, ctx.user_id, ctx.session_id, now).await?;
    audit::record(
        &mut tx,
        &AuditEntry {
            user_id: Some(ctx.user_id),
            action: action::PASSWORD_CHANGE,
            entity_key: Some(USER_ENTITY_KEY),
            record_id: Some(ctx.user_id),
            diff: AuditDiff::default(),
            client_info,
            occurred_at: now,
        },
    )
    .await?;
    commit(tx).await?;
    tracing::info!(user_id = %ctx.user_id, revoked_sessions = revoked, "已修改密碼");
    Ok(())
}

/// 寫入 `login_failed` 紀錄。`after_data` 為嘗試的帳號與失敗原因，不含密碼。
async fn record_failure(
    tx: &mut PgTransaction<'_>,
    user_id: Option<Id>,
    username: &str,
    reason: &str,
    client_info: Option<&str>,
    now: OffsetDateTime,
) -> Result<(), CoreError> {
    audit::record(
        tx,
        &AuditEntry {
            user_id,
            action: action::LOGIN_FAILED,
            entity_key: None,
            record_id: None,
            diff: AuditDiff {
                before: None,
                after: Some(json!({ "username": username, "reason": reason })),
            },
            client_info,
            occurred_at: now,
        },
    )
    .await?;
    Ok(())
}

async fn commit(tx: PgTransaction<'_>) -> Result<(), CoreError> {
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// 帳號不存在時用來驗證的雜湊，讓回應時間與密碼錯誤相近。
fn dummy_hash() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| hash_password("hammer-erp-dummy-password").unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;
    use time::macros::datetime;

    const NOW: OffsetDateTime = datetime!(2026-11-13 09:00 +8);

    fn settings() -> SecuritySettings {
        SecuritySettings::default()
    }

    fn ok(result: bool) -> impl FnOnce() -> Result<bool, CoreError> {
        move || Ok(result)
    }

    #[test]
    fn correct_password_succeeds() {
        let d = decide(
            true,
            LoginAttemptState::default(),
            NOW,
            &settings(),
            ok(true),
        )
        .unwrap();
        assert_eq!(d, LoginDecision::Success);
    }

    #[test]
    fn success_with_previous_failures() {
        let state = LoginAttemptState::new(4, None);
        let d = decide(true, state, NOW, &settings(), ok(true)).unwrap();
        assert_eq!(d, LoginDecision::Success);
    }

    #[test]
    fn wrong_password_counts_failure() {
        let d = decide(
            true,
            LoginAttemptState::new(1, None),
            NOW,
            &settings(),
            ok(false),
        )
        .unwrap();
        assert_eq!(
            d,
            LoginDecision::WrongPassword(LoginAttemptState::new(2, None))
        );
    }

    #[test]
    fn fifth_wrong_password_locks() {
        let d = decide(
            true,
            LoginAttemptState::new(4, None),
            NOW,
            &settings(),
            ok(false),
        )
        .unwrap();
        let LoginDecision::WrongPassword(next) = d else {
            panic!("{d:?}");
        };
        assert!(next.is_locked(NOW));
        assert_eq!(next.locked_until, Some(NOW + Duration::minutes(15)));
    }

    #[test]
    fn locked_does_not_verify_password() {
        let until = NOW + Duration::minutes(5);
        let state = LoginAttemptState::new(0, Some(until));
        let d = decide(true, state, NOW, &settings(), || {
            panic!("鎖定中不應驗證密碼")
        })
        .unwrap();
        assert_eq!(d, LoginDecision::Locked(until));
    }

    #[test]
    fn expired_lock_allows_login() {
        let state = LoginAttemptState::new(0, Some(NOW - Duration::seconds(1)));
        let d = decide(true, state, NOW, &settings(), ok(true)).unwrap();
        assert_eq!(d, LoginDecision::Success);
    }

    #[test]
    fn disabled_only_revealed_with_correct_password() {
        let d = decide(
            false,
            LoginAttemptState::default(),
            NOW,
            &settings(),
            ok(true),
        )
        .unwrap();
        assert_eq!(d, LoginDecision::Disabled);
        let d = decide(
            false,
            LoginAttemptState::default(),
            NOW,
            &settings(),
            ok(false),
        )
        .unwrap();
        assert!(matches!(d, LoginDecision::WrongPassword(_)));
    }

    #[test]
    fn verify_error_propagates() {
        let err = decide(true, LoginAttemptState::default(), NOW, &settings(), || {
            Err(CoreError::Internal("bad hash".into()))
        })
        .unwrap_err();
        assert!(matches!(err, CoreError::Internal(_)));
    }

    #[test]
    fn dummy_hash_is_valid_and_rejects_passwords() {
        let hash = dummy_hash();
        assert!(hash.starts_with("$argon2id$"));
        assert!(!verify_password("anything", hash).unwrap());
    }
}
