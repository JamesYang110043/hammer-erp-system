//! 安全相關的系統設定（docs/architecture.md §6.2、§8.4 `settings`）。
//!
//! 預設值由 migration 寫入 `settings`；資料庫沒有該 key，或值不是正整數時，
//! 使用 [`SecuritySettings::default`] 的值（並寫一筆警告 log），避免設定錯誤造成無法登入。

use hammer_db::sqlx::PgExecutor;
use time::Duration;

use crate::CoreError;

/// 設定 key：連續登入失敗幾次後鎖定。
pub const KEY_MAX_FAILED_LOGINS: &str = "security.max_failed_logins";
/// 設定 key：鎖定幾分鐘。
pub const KEY_LOCKOUT_MINUTES: &str = "security.lockout_minutes";
/// 設定 key：session 閒置幾分鐘後失效。
pub const KEY_SESSION_IDLE_MINUTES: &str = "security.session_idle_minutes";

const PREFIX: &str = "security.";

/// 型別化的安全設定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecuritySettings {
    /// 連續登入失敗幾次後鎖定。
    pub max_failed_logins: u32,
    /// 鎖定幾分鐘。
    pub lockout_minutes: u32,
    /// session 閒置幾分鐘後失效。
    pub session_idle_minutes: u32,
}

impl Default for SecuritySettings {
    /// 與 migration 寫入的預設值相同（§6.2）。
    fn default() -> Self {
        Self {
            max_failed_logins: 5,
            lockout_minutes: 15,
            session_idle_minutes: 480,
        }
    }
}

impl SecuritySettings {
    /// 從 `settings` 的 key / value 組成設定。不認得的 key 忽略。
    pub fn from_entries<'a>(
        entries: impl IntoIterator<Item = (&'a str, &'a serde_json::Value)>,
    ) -> Self {
        let mut settings = Self::default();
        for (key, value) in entries {
            let slot = match key {
                KEY_MAX_FAILED_LOGINS => &mut settings.max_failed_logins,
                KEY_LOCKOUT_MINUTES => &mut settings.lockout_minutes,
                KEY_SESSION_IDLE_MINUTES => &mut settings.session_idle_minutes,
                _ => continue,
            };
            match parse_positive(value) {
                Some(v) => *slot = v,
                None => tracing::warn!(
                    key,
                    %value,
                    default = *slot,
                    "安全設定的值不是正整數，改用預設值"
                ),
            }
        }
        settings
    }

    /// 從資料庫讀取。
    pub async fn load<'e>(executor: impl PgExecutor<'e>) -> Result<Self, CoreError> {
        let rows = hammer_db::settings::get_settings_by_prefix(executor, PREFIX).await?;
        Ok(Self::from_entries(
            rows.iter().map(|(k, v)| (k.as_str(), v)),
        ))
    }

    /// 鎖定時間。
    pub fn lockout_duration(&self) -> Duration {
        Duration::minutes(i64::from(self.lockout_minutes))
    }

    /// session 閒置期限。
    pub fn session_idle_duration(&self) -> Duration {
        Duration::minutes(i64::from(self.session_idle_minutes))
    }
}

/// 只接受正整數（JSON number）。
fn parse_positive(value: &serde_json::Value) -> Option<u32> {
    value
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .filter(|v| *v > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn from(pairs: &[(&str, Value)]) -> SecuritySettings {
        SecuritySettings::from_entries(pairs.iter().map(|(k, v)| (*k, v)))
    }

    #[test]
    fn defaults_match_spec() {
        let s = SecuritySettings::default();
        assert_eq!(s.max_failed_logins, 5);
        assert_eq!(s.lockout_minutes, 15);
        assert_eq!(s.session_idle_minutes, 480);
    }

    #[test]
    fn empty_entries_use_defaults() {
        assert_eq!(from(&[]), SecuritySettings::default());
    }

    #[test]
    fn values_override_defaults() {
        let s = from(&[
            (KEY_MAX_FAILED_LOGINS, json!(3)),
            (KEY_LOCKOUT_MINUTES, json!(30)),
            (KEY_SESSION_IDLE_MINUTES, json!(60)),
        ]);
        assert_eq!(
            s,
            SecuritySettings {
                max_failed_logins: 3,
                lockout_minutes: 30,
                session_idle_minutes: 60,
            }
        );
    }

    #[test]
    fn partial_entries_keep_other_defaults() {
        let s = from(&[(KEY_LOCKOUT_MINUTES, json!(5))]);
        assert_eq!(s.lockout_minutes, 5);
        assert_eq!(s.max_failed_logins, 5);
        assert_eq!(s.session_idle_minutes, 480);
    }

    #[test]
    fn invalid_values_fall_back_to_default() {
        let s = from(&[
            (KEY_MAX_FAILED_LOGINS, json!(0)),
            (KEY_LOCKOUT_MINUTES, json!(-1)),
            (KEY_SESSION_IDLE_MINUTES, json!("480")),
        ]);
        assert_eq!(s, SecuritySettings::default());
        let s = from(&[(KEY_LOCKOUT_MINUTES, json!(1.5))]);
        assert_eq!(s.lockout_minutes, 15);
        let s = from(&[(KEY_LOCKOUT_MINUTES, json!(u64::from(u32::MAX) + 1))]);
        assert_eq!(s.lockout_minutes, 15);
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let s = from(&[("security.something_else", json!(1))]);
        assert_eq!(s, SecuritySettings::default());
    }

    #[test]
    fn durations() {
        let s = SecuritySettings::default();
        assert_eq!(s.lockout_duration(), Duration::minutes(15));
        assert_eq!(s.session_idle_duration(), Duration::hours(8));
    }
}
