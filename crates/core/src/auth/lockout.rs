//! 連續登入失敗的鎖定規則（docs/architecture.md §6.2）。
//!
//! 對應 `users.failed_login_count` 與 `users.locked_until`。規則：
//!
//! 1. 登入前先檢查 [`LoginAttemptState::is_locked`]；鎖定中直接拒絕，不驗證密碼、不累計失敗次數。
//! 2. 密碼錯誤時呼叫 [`LoginAttemptState::record_failure`]：失敗次數加 1，
//!    達到 `max_failed_logins` 時鎖定 `lockout_minutes` 分鐘，並把失敗次數歸零。
//! 3. 鎖定時間過後自動解鎖（`locked_until <= now` 即視為未鎖定），不需要額外清除。
//! 4. 登入成功時呼叫 [`LoginAttemptState::record_success`]，清除失敗次數與鎖定。

use time::OffsetDateTime;

use super::SecuritySettings;

/// 帳號的登入失敗狀態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LoginAttemptState {
    /// 連續失敗次數（`users.failed_login_count`）。
    pub failed_login_count: i32,
    /// 鎖定到此時間（`users.locked_until`）。
    pub locked_until: Option<OffsetDateTime>,
}

impl LoginAttemptState {
    pub fn new(failed_login_count: i32, locked_until: Option<OffsetDateTime>) -> Self {
        Self {
            failed_login_count,
            locked_until,
        }
    }

    /// 目前是否鎖定中。`locked_until` 剛好等於 `now` 時已解鎖。
    pub fn is_locked(&self, now: OffsetDateTime) -> bool {
        self.locked_until.is_some_and(|until| until > now)
    }

    /// 鎖定中時，回傳解鎖時間；未鎖定回傳 `None`。
    pub fn unlocks_at(&self, now: OffsetDateTime) -> Option<OffsetDateTime> {
        self.locked_until.filter(|_| self.is_locked(now))
    }

    /// 記錄一次密碼錯誤，回傳新的狀態。
    ///
    /// 鎖定中呼叫時狀態不變（不延長鎖定）。
    pub fn record_failure(&self, now: OffsetDateTime, settings: &SecuritySettings) -> Self {
        if self.is_locked(now) {
            return *self;
        }
        let count = self.failed_login_count.max(0).saturating_add(1);
        let max = i32::try_from(settings.max_failed_logins).unwrap_or(i32::MAX);
        if count >= max {
            Self {
                failed_login_count: 0,
                locked_until: Some(now + settings.lockout_duration()),
            }
        } else {
            Self {
                failed_login_count: count,
                // 之前的鎖定已過期，一併清除
                locked_until: None,
            }
        }
    }

    /// 登入成功，清除失敗次數與鎖定。
    pub fn record_success(&self) -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;
    use time::macros::datetime;

    const NOW: OffsetDateTime = datetime!(2026-11-12 09:00 +8);

    fn fail_times(n: usize, settings: &SecuritySettings) -> LoginAttemptState {
        (0..n).fold(LoginAttemptState::default(), |s, _| {
            s.record_failure(NOW, settings)
        })
    }

    #[test]
    fn fourth_failure_does_not_lock() {
        let s = fail_times(4, &SecuritySettings::default());
        assert_eq!(s.failed_login_count, 4);
        assert!(!s.is_locked(NOW));
        assert_eq!(s.locked_until, None);
    }

    #[test]
    fn fifth_failure_locks_for_lockout_minutes() {
        let s = fail_times(5, &SecuritySettings::default());
        assert!(s.is_locked(NOW));
        assert_eq!(s.locked_until, Some(NOW + Duration::minutes(15)));
        assert_eq!(s.unlocks_at(NOW), Some(NOW + Duration::minutes(15)));
        assert_eq!(s.failed_login_count, 0);
    }

    #[test]
    fn unlocks_automatically_after_lockout() {
        let s = fail_times(5, &SecuritySettings::default());
        let until = NOW + Duration::minutes(15);
        assert!(s.is_locked(until - Duration::seconds(1)));
        assert!(!s.is_locked(until), "剛好到解鎖時間即解鎖");
        assert!(!s.is_locked(until + Duration::seconds(1)));
        assert_eq!(s.unlocks_at(until), None);
    }

    #[test]
    fn failure_while_locked_does_not_extend_lock() {
        let settings = SecuritySettings::default();
        let locked = fail_times(5, &settings);
        let later = NOW + Duration::minutes(10);
        assert_eq!(locked.record_failure(later, &settings), locked);
    }

    #[test]
    fn after_unlock_counts_from_zero() {
        let settings = SecuritySettings::default();
        let locked = fail_times(5, &settings);
        let after = NOW + Duration::minutes(16);
        let s = locked.record_failure(after, &settings);
        assert_eq!(s.failed_login_count, 1);
        assert_eq!(s.locked_until, None);
        assert!(!s.is_locked(after));
    }

    #[test]
    fn success_clears_state() {
        let s = fail_times(3, &SecuritySettings::default()).record_success();
        assert_eq!(s, LoginAttemptState::default());
    }

    #[test]
    fn respects_custom_settings() {
        let settings = SecuritySettings {
            max_failed_logins: 2,
            lockout_minutes: 1,
            ..SecuritySettings::default()
        };
        let s = fail_times(1, &settings);
        assert!(!s.is_locked(NOW));
        let s = s.record_failure(NOW, &settings);
        assert_eq!(s.locked_until, Some(NOW + Duration::minutes(1)));
    }

    #[test]
    fn new_reads_existing_row() {
        let s = LoginAttemptState::new(4, None);
        let s = s.record_failure(NOW, &SecuritySettings::default());
        assert!(s.is_locked(NOW));
    }
}
