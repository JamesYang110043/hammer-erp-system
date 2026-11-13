//! 回傳給前端的錯誤。
//!
//! 伺服器回傳的錯誤原封不動轉成 shared 的 [`ApiError`]（`kind: "api"`）；
//! 連線失敗等在用戶端發生的錯誤用其他 `kind` 區分。
//! 每一種都有 `message`（繁體中文），前端可以直接顯示。

use hammer_shared::ApiError;
use serde::Serialize;
use specta::Type;

#[derive(Debug, Clone, PartialEq, thiserror::Error, Serialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ClientError {
    /// 伺服器回傳的錯誤（統一錯誤格式，`docs/architecture.md` §10）。
    #[error("{0}")]
    Api(ApiError),
    /// 登入已逾時或 session 已失效（伺服器對需要登入的請求回 `401`）。
    ///
    /// 收到時本機的 token 已清除，前端應回到登入頁。
    #[error("{message}")]
    SessionExpired { message: String },
    /// 無法連線到伺服器、連線逾時。
    #[error("{message}")]
    Network { message: String },
    /// 伺服器的回應無法解析（例如網址指到別的服務）。
    #[error("{message}")]
    InvalidResponse { message: String },
    /// 使用者輸入不正確（例如伺服器網址格式錯誤）。
    #[error("{message}")]
    InvalidInput { message: String },
    /// 本機設定檔讀寫失敗。
    #[error("{message}")]
    Storage { message: String },
}

/// [`ClientError::SessionExpired`] 的訊息。
pub const SESSION_EXPIRED_MESSAGE: &str = "登入已逾時，請重新登入";

impl ClientError {
    pub fn session_expired() -> Self {
        Self::SessionExpired {
            message: SESSION_EXPIRED_MESSAGE.to_owned(),
        }
    }

    pub fn storage(message: impl Into<String>) -> Self {
        Self::Storage {
            message: message.into(),
        }
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::InvalidInput {
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hammer_shared::ErrorCode;
    use serde_json::json;

    #[test]
    fn api_error_is_flattened_with_kind() {
        let err = ClientError::Api(ApiError::new(ErrorCode::ClientOutdated, "請更新程式"));
        assert_eq!(
            serde_json::to_value(&err).unwrap(),
            json!({ "kind": "api", "code": "CLIENT_OUTDATED", "message": "請更新程式" })
        );
    }

    #[test]
    fn session_expired_kind_is_camel_case() {
        assert_eq!(
            serde_json::to_value(ClientError::session_expired()).unwrap(),
            json!({ "kind": "sessionExpired", "message": "登入已逾時，請重新登入" })
        );
    }

    #[test]
    fn client_side_errors_have_message() {
        let err = ClientError::Network {
            message: "無法連線".to_owned(),
        };
        assert_eq!(
            serde_json::to_value(&err).unwrap(),
            json!({ "kind": "network", "message": "無法連線" })
        );
    }
}
