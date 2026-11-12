//! 目前登入的使用者與權限檢查（docs/architecture.md §6.1、§6.2）。
//!
//! - [`authenticate`]：由客戶端送來的 session token 取得 [`AuthContext`]
//!   1. 計算 token 雜湊並查詢 session
//!   2. 檢查 session 有效（[`session_status`]：未撤銷、未過期、未閒置逾時）
//!   3. 檢查使用者存在、未停用、未刪除
//!   4. 距離上次更新 `last_seen_at` 超過 [`TOUCH_INTERVAL`] 才寫入（[`needs_touch`]），
//!      避免每個請求都寫資料庫
//! - [`AuthContext::require`]：檢查權限，沒有權限回傳 [`CoreError::Forbidden`]
//! - [`AuthContext::ensure_password_changed`]：`must_change_password` 時回傳
//!   [`CoreError::PasswordChangeRequired`]
//!
//! 權限每個請求都從資料庫讀取，不快取：角色或帳號狀態變更後立即生效。

use std::collections::BTreeSet;

use hammer_db::sqlx::PgPool;
use hammer_db::user::{AuthUserRow, find_auth_user};
use hammer_shared::Id;
use time::{Duration, OffsetDateTime};

use super::{SecuritySettings, SessionStatus, session, session_status};
use crate::CoreError;

/// 距離上次更新超過這個時間，才寫入 `last_seen_at`。
pub const TOUCH_INTERVAL: Duration = Duration::minutes(1);

/// 目前登入的使用者。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthContext {
    pub user_id: Id,
    pub username: String,
    pub display_name: String,
    /// 角色代碼，對應 `hammer_shared::role`。
    pub role: String,
    /// 角色擁有的權限代碼，對應 `hammer_shared::permission`。
    pub permissions: BTreeSet<String>,
    pub session_id: Id,
    pub must_change_password: bool,
}

impl AuthContext {
    /// 由資料列組成。
    pub fn new(user: AuthUserRow, session_id: Id) -> Self {
        Self {
            user_id: user.id,
            username: user.username,
            display_name: user.display_name,
            role: user.role_code,
            permissions: user.permissions.into_iter().collect(),
            session_id,
            must_change_password: user.must_change_password,
        }
    }

    /// 是否擁有某個權限。
    pub fn has_permission(&self, permission: &str) -> bool {
        self.permissions.contains(permission)
    }

    /// 檢查權限，例如 `auth.require(permission::USER_MANAGE)?`。
    pub fn require(&self, permission: &'static str) -> Result<(), CoreError> {
        if self.has_permission(permission) {
            Ok(())
        } else {
            tracing::info!(
                user_id = %self.user_id,
                role = %self.role,
                permission,
                "權限不足"
            );
            Err(CoreError::Forbidden(permission))
        }
    }

    /// `must_change_password` 時回傳 [`CoreError::PasswordChangeRequired`]。
    pub fn ensure_password_changed(&self) -> Result<(), CoreError> {
        if self.must_change_password {
            Err(CoreError::PasswordChangeRequired)
        } else {
            Ok(())
        }
    }
}

/// 是否需要更新 `last_seen_at`：距離上次更新超過 [`TOUCH_INTERVAL`]。
pub fn needs_touch(last_seen_at: OffsetDateTime, now: OffsetDateTime) -> bool {
    now - last_seen_at > TOUCH_INTERVAL
}

/// 使用者是否可以使用系統：未停用、未刪除。
pub fn user_is_usable(user: &AuthUserRow) -> bool {
    user.is_active && user.deleted_at.is_none()
}

/// 由客戶端送來的 session token 取得目前使用者。
///
/// token 不存在、session 無效、使用者停用或刪除，一律回傳 [`CoreError::Unauthorized`]，
/// 原因只寫進 log。不檢查 `must_change_password`，由呼叫端決定
/// （見 [`AuthContext::ensure_password_changed`]）。
pub async fn authenticate(
    pool: &PgPool,
    token: &str,
    now: OffsetDateTime,
) -> Result<AuthContext, CoreError> {
    let Some(session) = session::find_by_token(pool, token).await? else {
        tracing::debug!("認證失敗：token 不存在");
        return Err(CoreError::Unauthorized);
    };

    let settings = SecuritySettings::load(pool).await?;
    let status = session_status(&session, now, &settings);
    if status != SessionStatus::Active {
        tracing::debug!(session_id = %session.id, ?status, "認證失敗：session 無效");
        return Err(CoreError::Unauthorized);
    }

    let user = find_auth_user(pool, session.user_id).await?;
    let Some(user) = user.filter(user_is_usable) else {
        tracing::info!(
            session_id = %session.id,
            user_id = %session.user_id,
            "認證失敗：使用者已停用或已刪除"
        );
        return Err(CoreError::Unauthorized);
    };

    if needs_touch(session.last_seen_at, now)
        && !session::touch(pool, session.id, now, &settings).await?
    {
        // 查詢後到更新之間被撤銷（例如同時登出）
        tracing::debug!(session_id = %session.id, "認證失敗：session 已被撤銷");
        return Err(CoreError::Unauthorized);
    }

    Ok(AuthContext::new(user, session.id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hammer_shared::{new_id, permission, role};
    use time::macros::datetime;

    const T0: OffsetDateTime = datetime!(2026-11-13 09:00 +8);

    fn row(role_code: &str, permissions: &[&str]) -> AuthUserRow {
        AuthUserRow {
            id: new_id(),
            username: "alice".into(),
            display_name: "Alice".into(),
            is_active: true,
            must_change_password: false,
            deleted_at: None,
            role_code: role_code.into(),
            permissions: permissions.iter().map(|p| (*p).to_owned()).collect(),
        }
    }

    fn viewer() -> AuthContext {
        AuthContext::new(
            row(
                role::VIEWER,
                &[permission::RECORD_READ, permission::DATA_EXPORT],
            ),
            new_id(),
        )
    }

    #[test]
    fn context_from_row() {
        let user = row(
            role::EDITOR,
            &[permission::RECORD_WRITE, permission::RECORD_READ],
        );
        let session_id = new_id();
        let ctx = AuthContext::new(user.clone(), session_id);
        assert_eq!(ctx.user_id, user.id);
        assert_eq!(ctx.username, "alice");
        assert_eq!(ctx.display_name, "Alice");
        assert_eq!(ctx.role, role::EDITOR);
        assert_eq!(ctx.session_id, session_id);
        assert!(!ctx.must_change_password);
        assert_eq!(
            ctx.permissions
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec![permission::RECORD_READ, permission::RECORD_WRITE]
        );
    }

    #[test]
    fn require_granted_permission() {
        let ctx = viewer();
        assert!(ctx.has_permission(permission::RECORD_READ));
        assert!(ctx.require(permission::RECORD_READ).is_ok());
    }

    #[test]
    fn require_missing_permission_is_forbidden() {
        let ctx = viewer();
        assert!(!ctx.has_permission(permission::RECORD_WRITE));
        assert!(matches!(
            ctx.require(permission::RECORD_WRITE),
            Err(CoreError::Forbidden(p)) if p == permission::RECORD_WRITE
        ));
    }

    #[test]
    fn no_permissions_forbids_everything() {
        let ctx = AuthContext::new(row(role::VIEWER, &[]), new_id());
        for p in permission::ALL {
            assert!(ctx.require(p).is_err(), "{p}");
        }
    }

    #[test]
    fn password_change_check() {
        let mut ctx = viewer();
        assert!(ctx.ensure_password_changed().is_ok());
        ctx.must_change_password = true;
        assert!(matches!(
            ctx.ensure_password_changed(),
            Err(CoreError::PasswordChangeRequired)
        ));
    }

    #[test]
    fn touch_only_after_interval() {
        assert!(!needs_touch(T0, T0));
        assert!(!needs_touch(T0, T0 + Duration::seconds(59)));
        assert!(!needs_touch(T0, T0 + TOUCH_INTERVAL));
        assert!(needs_touch(T0, T0 + TOUCH_INTERVAL + Duration::seconds(1)));
        assert!(needs_touch(T0, T0 + Duration::hours(1)));
        // 時鐘倒退時不更新
        assert!(!needs_touch(T0, T0 - Duration::minutes(5)));
    }

    #[test]
    fn usable_user() {
        let mut user = row(role::EDITOR, &[]);
        assert!(user_is_usable(&user));
        user.is_active = false;
        assert!(!user_is_usable(&user));
        user.is_active = true;
        user.deleted_at = Some(T0);
        assert!(!user_is_usable(&user));
    }
}
