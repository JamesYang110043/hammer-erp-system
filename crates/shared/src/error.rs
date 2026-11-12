//! 統一錯誤格式（`docs/architecture.md` §10）。
//!
//! ```json
//! { "code": "NOT_FOUND", "message": "找不到資料", "details": { … } }
//! ```
//!
//! `details` 沒有內容時省略。

use serde::{Deserialize, Serialize};
use specta::Type;

/// 錯誤碼。JSON 中為 `SCREAMING_SNAKE_CASE` 字串。
///
/// 之後各模組的業務錯誤碼（例如 `STOCK_INSUFFICIENT`）加在這裡。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// 伺服器內部錯誤。
    Internal,
    /// 請求格式錯誤（例如 JSON 無法解析、標頭格式錯誤）。
    BadRequest,
    /// 輸入資料驗證失敗。
    Validation,
    /// 找不到資料。
    NotFound,
    /// 未登入或 session 失效。
    Unauthorized,
    /// 沒有權限。
    Forbidden,
    /// 不支援此 HTTP 方法。
    MethodNotAllowed,
    /// 樂觀鎖定衝突：資料已被其他人修改（§9.4）。
    Conflict,
    /// 請求內容超過大小上限。
    PayloadTooLarge,
    /// 客戶端版本過舊，需要更新（§10）。
    ClientOutdated,
}

impl ErrorCode {
    /// 對應的 HTTP 狀態碼。
    pub fn http_status(self) -> u16 {
        match self {
            Self::Internal => 500,
            Self::BadRequest => 400,
            Self::Validation => 422,
            Self::NotFound => 404,
            Self::Unauthorized => 401,
            Self::Forbidden => 403,
            Self::MethodNotAllowed => 405,
            Self::Conflict => 409,
            Self::PayloadTooLarge => 413,
            Self::ClientOutdated => 426,
        }
    }
}

/// API 錯誤回應的內容。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct ApiError {
    pub code: ErrorCode,
    /// 給使用者看的訊息（繁體中文）。
    pub message: String,
    /// 額外資訊，例如驗證失敗的欄位。
    ///
    /// TypeScript 中為 `unknown`：`serde_json::Value` 可能含 64 位元整數，specta 預設禁止匯出。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[specta(type = Option<specta_typescript::Unknown>)]
    pub details: Option<serde_json::Value>,
}

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ApiError {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn json_shape_matches_architecture_doc() {
        let err = ApiError::new(ErrorCode::Validation, "數量必須大於 0")
            .with_details(json!({ "field": "quantity" }));
        assert_eq!(
            serde_json::to_value(&err).unwrap(),
            json!({
                "code": "VALIDATION",
                "message": "數量必須大於 0",
                "details": { "field": "quantity" }
            })
        );
    }

    #[test]
    fn details_omitted_when_none() {
        let err = ApiError::new(ErrorCode::NotFound, "找不到資料");
        assert_eq!(
            serde_json::to_value(&err).unwrap(),
            json!({ "code": "NOT_FOUND", "message": "找不到資料" })
        );
    }

    #[test]
    fn deserializes_from_server_response() {
        let err: ApiError = serde_json::from_value(json!({
            "code": "CLIENT_OUTDATED",
            "message": "請更新程式"
        }))
        .unwrap();
        assert_eq!(err, ApiError::new(ErrorCode::ClientOutdated, "請更新程式"));
    }

    #[test]
    fn all_codes_serialize_as_screaming_snake_case() {
        let cases = [
            (ErrorCode::Internal, "INTERNAL"),
            (ErrorCode::BadRequest, "BAD_REQUEST"),
            (ErrorCode::Validation, "VALIDATION"),
            (ErrorCode::NotFound, "NOT_FOUND"),
            (ErrorCode::Unauthorized, "UNAUTHORIZED"),
            (ErrorCode::Forbidden, "FORBIDDEN"),
            (ErrorCode::MethodNotAllowed, "METHOD_NOT_ALLOWED"),
            (ErrorCode::Conflict, "CONFLICT"),
            (ErrorCode::PayloadTooLarge, "PAYLOAD_TOO_LARGE"),
            (ErrorCode::ClientOutdated, "CLIENT_OUTDATED"),
        ];
        for (code, s) in cases {
            assert_eq!(serde_json::to_value(code).unwrap(), json!(s));
        }
    }

    #[test]
    fn http_status_mapping() {
        assert_eq!(ErrorCode::BadRequest.http_status(), 400);
        assert_eq!(ErrorCode::MethodNotAllowed.http_status(), 405);
        assert_eq!(ErrorCode::Conflict.http_status(), 409);
        assert_eq!(ErrorCode::PayloadTooLarge.http_status(), 413);
        assert_eq!(ErrorCode::ClientOutdated.http_status(), 426);
    }
}
