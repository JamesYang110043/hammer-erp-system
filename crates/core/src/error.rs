//! `core` 的錯誤。
//!
//! 每個錯誤都對應一個 [`ErrorCode`]，由 `server` 轉成統一的 API 錯誤格式（§10）。

use hammer_db::DbError;
use hammer_shared::{ApiError, ErrorCode};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// 商業邏輯層的錯誤。
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// 輸入資料不符合規則，訊息直接顯示給使用者（繁體中文）。
    #[error("{0}")]
    Validation(String),

    /// 未登入、token 無效、session 已失效，或使用者已停用、已刪除。
    ///
    /// 不區分原因，避免透露帳號狀態；原因只寫進 log。
    #[error("尚未登入或登入已失效，請重新登入")]
    Unauthorized,

    /// 已登入但沒有所需的權限。內容為權限代碼。
    #[error("沒有權限執行此操作")]
    Forbidden(&'static str),

    /// 必須先修改密碼（`users.must_change_password = true`，§6.2）。
    #[error("請先修改密碼")]
    PasswordChangeRequired,

    /// 登入時帳號不存在或密碼錯誤。兩者訊息相同，不透露帳號是否存在。
    #[error("帳號或密碼錯誤")]
    InvalidCredentials,

    /// 連續登入失敗，帳號鎖定到此時間（§6.2）。
    #[error("登入失敗次數過多，帳號已暫時鎖定，請於 {} 後再試", format_taipei_time(.0))]
    AccountLocked(OffsetDateTime),

    /// 帳號已停用。
    #[error("此帳號已停用，請聯絡管理者")]
    AccountDisabled,

    /// 操作與目前的資料狀態衝突，訊息直接顯示給使用者（繁體中文）。
    /// 例如已有帳號時再呼叫初次啟動建立管理者。
    #[error("{0}")]
    Conflict(String),

    /// 資料存取錯誤。
    #[error(transparent)]
    Db(#[from] DbError),

    /// 不應發生的內部錯誤（例如資料庫中的密碼雜湊格式損壞）。訊息只寫進 log。
    #[error("內部錯誤：{0}")]
    Internal(String),
}

impl CoreError {
    /// 對應的錯誤碼。
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Validation(_) => ErrorCode::Validation,
            Self::Unauthorized => ErrorCode::Unauthorized,
            Self::Forbidden(_) => ErrorCode::Forbidden,
            Self::PasswordChangeRequired => ErrorCode::PasswordChangeRequired,
            Self::InvalidCredentials => ErrorCode::Unauthorized,
            Self::AccountLocked(_) => ErrorCode::AccountLocked,
            Self::AccountDisabled => ErrorCode::AccountDisabled,
            Self::Conflict(_) | Self::Db(DbError::Conflict) => ErrorCode::Conflict,
            Self::Db(_) | Self::Internal(_) => ErrorCode::Internal,
        }
    }

    /// 是否可以把訊息直接顯示給使用者。內部錯誤的細節不應回傳給客戶端。
    pub fn is_user_facing(&self) -> bool {
        !matches!(self.code(), ErrorCode::Internal)
    }
}

impl From<CoreError> for ApiError {
    /// 轉成 API 錯誤。內部錯誤只回傳通用訊息。
    fn from(err: CoreError) -> Self {
        let code = err.code();
        if let CoreError::Forbidden(permission) = err {
            ApiError::new(code, err.to_string())
                .with_details(serde_json::json!({ "permission": permission }))
        } else if let CoreError::AccountLocked(until) = err {
            let locked_until = until.format(&Rfc3339).unwrap_or_default();
            ApiError::new(code, err.to_string())
                .with_details(serde_json::json!({ "locked_until": locked_until }))
        } else if err.is_user_facing() {
            ApiError::new(code, err.to_string())
        } else {
            ApiError::new(code, "伺服器發生錯誤，請稍後再試")
        }
    }
}

/// 以台北時間顯示 `HH:MM`（D13）。
fn format_taipei_time(at: &OffsetDateTime) -> String {
    let t = at.to_offset(hammer_shared::time::TAIPEI_OFFSET);
    format!("{:02}:{:02}", t.hour(), t.minute())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_maps_to_validation_code() {
        let err = CoreError::Validation("密碼至少需要 10 個字元".into());
        assert_eq!(err.code(), ErrorCode::Validation);
        let api: ApiError = err.into();
        assert_eq!(api.code, ErrorCode::Validation);
        assert_eq!(api.message, "密碼至少需要 10 個字元");
    }

    #[test]
    fn db_conflict_maps_to_conflict() {
        let err = CoreError::from(DbError::Conflict);
        assert_eq!(err.code(), ErrorCode::Conflict);
        assert!(err.is_user_facing());
    }

    #[test]
    fn internal_hides_details() {
        let err = CoreError::Internal("密碼雜湊格式錯誤".into());
        assert_eq!(err.code(), ErrorCode::Internal);
        let api: ApiError = err.into();
        assert_eq!(api.code, ErrorCode::Internal);
        assert!(!api.message.contains("雜湊"));
    }

    #[test]
    fn auth_errors_map_to_codes() {
        let cases = [
            (CoreError::Unauthorized, ErrorCode::Unauthorized),
            (CoreError::Forbidden("record.write"), ErrorCode::Forbidden),
            (
                CoreError::PasswordChangeRequired,
                ErrorCode::PasswordChangeRequired,
            ),
        ];
        for (err, code) in cases {
            assert_eq!(err.code(), code);
            assert!(err.is_user_facing());
            let message = err.to_string();
            let api: ApiError = err.into();
            assert_eq!(api.code, code);
            assert_eq!(api.message, message);
        }
    }

    #[test]
    fn forbidden_includes_permission_in_details() {
        let api: ApiError = CoreError::Forbidden("record.write").into();
        assert_eq!(
            api.details,
            Some(serde_json::json!({ "permission": "record.write" }))
        );
    }

    #[test]
    fn login_errors_map_to_codes() {
        let api: ApiError = CoreError::InvalidCredentials.into();
        assert_eq!(api.code, ErrorCode::Unauthorized);
        assert_eq!(api.message, "帳號或密碼錯誤");
        assert_eq!(api.details, None);

        let api: ApiError = CoreError::AccountDisabled.into();
        assert_eq!(api.code, ErrorCode::AccountDisabled);
    }

    #[test]
    fn account_locked_includes_unlock_time() {
        let until = time::macros::datetime!(2026-11-13 01:05:30 UTC);
        let api: ApiError = CoreError::AccountLocked(until).into();
        assert_eq!(api.code, ErrorCode::AccountLocked);
        // 訊息以台北時間顯示
        assert!(api.message.contains("09:05"), "{}", api.message);
        assert_eq!(
            api.details,
            Some(serde_json::json!({ "locked_until": "2026-11-13T01:05:30Z" }))
        );
    }

    #[test]
    fn conflict_keeps_message() {
        let api: ApiError = CoreError::Conflict("系統已完成初始設定".into()).into();
        assert_eq!(api.code, ErrorCode::Conflict);
        assert_eq!(api.message, "系統已完成初始設定");
        assert_eq!(api.details, None);
    }

    #[test]
    fn other_db_errors_are_internal() {
        let err = CoreError::from(DbError::Sqlx(hammer_db::sqlx::Error::RowNotFound));
        assert_eq!(err.code(), ErrorCode::Internal);
        assert!(!err.is_user_facing());
    }
}
