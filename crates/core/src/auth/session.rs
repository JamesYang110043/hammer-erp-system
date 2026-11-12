//! Session 的有效判斷與存取（docs/architecture.md §6.2、§8.4 `sessions`）。
//!
//! - 有效判斷 [`session_status`] 是純函式：已撤銷、已過期、閒置超過
//!   `security.session_idle_minutes` 都視為無效。
//! - 沒有絕對有效期限：`expires_at` 一律是 `last_seen_at` + 閒置期限，
//!   每次 [`touch`] 時一起延長。判斷時仍以目前設定重新計算閒置期限，
//!   因此管理者縮短閒置時間後，既有 session 也立即套用。
//! - 存取函式包裝 `hammer_db::session`，負責產生 token、計算雜湊與到期時間。

use hammer_db::session::{self as db, NewSession, SessionRow};
use hammer_db::sqlx::PgExecutor;
use hammer_shared::{Id, new_id};
use time::OffsetDateTime;

use super::{SecuritySettings, SessionToken, hash_token};
use crate::CoreError;

/// Session 的狀態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    /// 有效。
    Active,
    /// 已登出或被強制登出。
    Revoked,
    /// 已超過 `expires_at`。
    Expired,
    /// 閒置超過設定的時間。
    Idle,
}

impl SessionStatus {
    pub fn is_active(self) -> bool {
        self == Self::Active
    }
}

/// 判斷 session 狀態。剛好到期（`now` 等於期限）即視為失效。
pub fn session_status(
    session: &SessionRow,
    now: OffsetDateTime,
    settings: &SecuritySettings,
) -> SessionStatus {
    if session.revoked_at.is_some() {
        SessionStatus::Revoked
    } else if now >= session.expires_at {
        SessionStatus::Expired
    } else if now >= session.last_seen_at + settings.session_idle_duration() {
        SessionStatus::Idle
    } else {
        SessionStatus::Active
    }
}

/// 建立 session，回傳 token（只在這時候取得一次）與資料列。
pub async fn create<'e>(
    executor: impl PgExecutor<'e>,
    user_id: Id,
    client_info: Option<&str>,
    now: OffsetDateTime,
    settings: &SecuritySettings,
) -> Result<(SessionToken, SessionRow), CoreError> {
    let token = SessionToken::generate()?;
    let token_hash = token.hash();
    let row = db::insert_session(
        executor,
        &NewSession {
            id: new_id(),
            user_id,
            token_hash: &token_hash,
            client_info,
            created_at: now,
            expires_at: now + settings.session_idle_duration(),
        },
    )
    .await?;
    Ok((token, row))
}

/// 依客戶端送來的 token 查詢 session。不判斷是否有效，交給 [`session_status`]。
pub async fn find_by_token<'e>(
    executor: impl PgExecutor<'e>,
    token: &str,
) -> Result<Option<SessionRow>, CoreError> {
    Ok(db::find_session_by_token_hash(executor, &hash_token(token)).await?)
}

/// 記錄 session 被使用：更新 `last_seen_at`，並把 `expires_at` 延長到 `now` + 閒置期限。
/// 已撤銷的 session 不更新，回傳 `false`。
pub async fn touch<'e>(
    executor: impl PgExecutor<'e>,
    session_id: Id,
    now: OffsetDateTime,
    settings: &SecuritySettings,
) -> Result<bool, CoreError> {
    Ok(db::touch_session(
        executor,
        session_id,
        now,
        now + settings.session_idle_duration(),
    )
    .await?)
}

/// 撤銷單一 session（登出）。已撤銷的回傳 `false`。
pub async fn revoke<'e>(
    executor: impl PgExecutor<'e>,
    session_id: Id,
    now: OffsetDateTime,
) -> Result<bool, CoreError> {
    Ok(db::revoke_session(executor, session_id, now).await?)
}

/// 撤銷某使用者全部 session，回傳撤銷的筆數。
pub async fn revoke_all_for_user<'e>(
    executor: impl PgExecutor<'e>,
    user_id: Id,
    now: OffsetDateTime,
) -> Result<u64, CoreError> {
    Ok(db::revoke_user_sessions(executor, user_id, now).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;
    use time::macros::datetime;

    const T0: OffsetDateTime = datetime!(2026-11-12 09:00 +8);

    fn settings() -> SecuritySettings {
        SecuritySettings::default()
    }

    /// 在 `last_seen_at` 使用過、依滑動規則設定 `expires_at` 的 session。
    fn session(last_seen_at: OffsetDateTime) -> SessionRow {
        SessionRow {
            id: new_id(),
            user_id: new_id(),
            token_hash: hash_token("t"),
            client_info: None,
            created_at: T0,
            last_seen_at,
            expires_at: last_seen_at + settings().session_idle_duration(),
            revoked_at: None,
        }
    }

    #[test]
    fn fresh_session_is_active() {
        assert_eq!(
            session_status(&session(T0), T0, &settings()),
            SessionStatus::Active
        );
        assert!(SessionStatus::Active.is_active());
    }

    #[test]
    fn one_second_before_idle_timeout_is_active() {
        let s = session(T0);
        let now = T0 + Duration::minutes(480) - Duration::seconds(1);
        assert_eq!(session_status(&s, now, &settings()), SessionStatus::Active);
    }

    #[test]
    fn exactly_at_idle_timeout_is_invalid() {
        let s = session(T0);
        let now = T0 + Duration::minutes(480);
        assert!(!session_status(&s, now, &settings()).is_active());
    }

    #[test]
    fn idle_uses_current_settings() {
        // expires_at 仍在未來，但管理者把閒置時間縮短為 60 分鐘
        let s = session(T0);
        let shorter = SecuritySettings {
            session_idle_minutes: 60,
            ..settings()
        };
        let now = T0 + Duration::minutes(60);
        assert_eq!(session_status(&s, now, &shorter), SessionStatus::Idle);
        let now = T0 + Duration::minutes(60) - Duration::seconds(1);
        assert_eq!(session_status(&s, now, &shorter), SessionStatus::Active);
    }

    #[test]
    fn expires_at_boundary() {
        // expires_at 比閒置期限早（例如之前設定值較短時建立），以 expires_at 為準
        let mut s = session(T0);
        s.expires_at = T0 + Duration::minutes(30);
        let at = s.expires_at;
        assert_eq!(
            session_status(&s, at - Duration::seconds(1), &settings()),
            SessionStatus::Active
        );
        assert_eq!(session_status(&s, at, &settings()), SessionStatus::Expired);
    }

    #[test]
    fn touched_session_extends_idle() {
        let s = session(T0 + Duration::hours(7));
        let now = T0 + Duration::hours(10);
        assert_eq!(session_status(&s, now, &settings()), SessionStatus::Active);
    }

    #[test]
    fn revoked_session_is_invalid() {
        let mut s = session(T0);
        s.revoked_at = Some(T0);
        assert_eq!(session_status(&s, T0, &settings()), SessionStatus::Revoked);
    }
}
