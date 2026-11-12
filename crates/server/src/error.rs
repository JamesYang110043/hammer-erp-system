//! 統一錯誤處理：所有錯誤都轉成 shared 的 [`ApiError`] JSON（`docs/architecture.md` §10）。

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use hammer_core::db::DbError;
use hammer_shared::{ApiError, ErrorCode};

/// Handler 回傳的錯誤。轉成回應時，HTTP 狀態碼由 [`ErrorCode::http_status`] 決定。
#[derive(Debug)]
pub struct AppError(pub ApiError);

impl AppError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self(ApiError::new(code, message))
    }

    pub fn with_details(self, details: serde_json::Value) -> Self {
        Self(self.0.with_details(details))
    }

    pub fn not_found() -> Self {
        Self::new(ErrorCode::NotFound, default_message(ErrorCode::NotFound))
    }

    /// 伺服器內部錯誤。原始錯誤只寫進 log，不回傳給客戶端。
    pub fn internal(err: impl std::fmt::Display) -> Self {
        tracing::error!(error = %err, "伺服器內部錯誤");
        Self::new(ErrorCode::Internal, default_message(ErrorCode::Internal))
    }

    pub fn code(&self) -> ErrorCode {
        self.0.code
    }
}

impl From<ApiError> for AppError {
    fn from(err: ApiError) -> Self {
        Self(err)
    }
}

impl From<DbError> for AppError {
    fn from(err: DbError) -> Self {
        match err {
            DbError::Conflict => Self::new(ErrorCode::Conflict, err.to_string()),
            other => Self::internal(other),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = status_of(self.0.code);
        (status, Json(self.0)).into_response()
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for AppError {}

/// 錯誤碼對應的 HTTP 狀態碼。
pub fn status_of(code: ErrorCode) -> StatusCode {
    StatusCode::from_u16(code.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
}

/// 錯誤碼的預設訊息（繁體中文）。
pub fn default_message(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::Internal => "伺服器發生錯誤，請稍後再試",
        ErrorCode::BadRequest => "請求格式錯誤",
        ErrorCode::Validation => "輸入資料有誤",
        ErrorCode::NotFound => "找不到資料",
        ErrorCode::Unauthorized => "請先登入",
        ErrorCode::Forbidden => "沒有權限執行此操作",
        ErrorCode::MethodNotAllowed => "不支援此操作方式",
        ErrorCode::Conflict => "此資料已被其他人修改，請重新載入",
        ErrorCode::PayloadTooLarge => "上傳的資料超過大小上限",
        ErrorCode::ClientOutdated => "程式版本過舊，請更新後再使用",
    }
}

/// 由 HTTP 狀態碼推得錯誤碼。用於把框架或中介層產生的錯誤回應（非 JSON）轉成統一格式。
pub fn code_for_status(status: StatusCode) -> ErrorCode {
    match status {
        StatusCode::UNAUTHORIZED => ErrorCode::Unauthorized,
        StatusCode::FORBIDDEN => ErrorCode::Forbidden,
        StatusCode::NOT_FOUND => ErrorCode::NotFound,
        StatusCode::METHOD_NOT_ALLOWED => ErrorCode::MethodNotAllowed,
        StatusCode::CONFLICT => ErrorCode::Conflict,
        StatusCode::PAYLOAD_TOO_LARGE => ErrorCode::PayloadTooLarge,
        StatusCode::UNPROCESSABLE_ENTITY => ErrorCode::Validation,
        StatusCode::UPGRADE_REQUIRED => ErrorCode::ClientOutdated,
        s if s.is_client_error() => ErrorCode::BadRequest,
        _ => ErrorCode::Internal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_matches_shared_mapping() {
        for code in [
            ErrorCode::Internal,
            ErrorCode::BadRequest,
            ErrorCode::Validation,
            ErrorCode::NotFound,
            ErrorCode::Unauthorized,
            ErrorCode::Forbidden,
            ErrorCode::MethodNotAllowed,
            ErrorCode::Conflict,
            ErrorCode::PayloadTooLarge,
            ErrorCode::ClientOutdated,
        ] {
            assert_eq!(status_of(code).as_u16(), code.http_status());
            // 狀態碼與錯誤碼互相對應（BAD_REQUEST 以外的 4xx 不會反推回 Validation 等）
            assert_eq!(code_for_status(status_of(code)), code);
        }
    }

    #[test]
    fn unknown_statuses_fall_back() {
        assert_eq!(
            code_for_status(StatusCode::UNSUPPORTED_MEDIA_TYPE),
            ErrorCode::BadRequest
        );
        assert_eq!(
            code_for_status(StatusCode::BAD_GATEWAY),
            ErrorCode::Internal
        );
    }

    #[test]
    fn db_conflict_maps_to_conflict() {
        assert_eq!(
            AppError::from(DbError::Conflict).code(),
            ErrorCode::Conflict
        );
    }
}
