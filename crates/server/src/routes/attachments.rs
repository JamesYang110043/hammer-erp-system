//! 附件的上傳與下載（docs/architecture.md §7.3 `file`、§8.4 `attachments`）。
//!
//! 上傳需要 `record.write`、下載需要 `record.read`（在 `hammer_core::attachments` 檢查）。
//! 端點、流程與錯誤見 [`hammer_shared::attachment`]。
//!
//! 上傳的路由不套用全域的 body 上限（`body_limit_bytes`），改用「附件單檔上限 +
//! [`MULTIPART_OVERHEAD_BYTES`]」（見 [`super::router`]）；檔案內容的大小由 core 逐段累計檢查。

use axum::Json;
use axum::extract::multipart::{Field, MultipartError};
use axum::extract::{Multipart, Path, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE, HeaderValue,
    X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use hammer_core::attachments::{self, MSG_FILE_NAME_REQUIRED};
use hammer_shared::attachment::{Attachment, DEFAULT_MIME_TYPE, UPLOAD_FIELD};
use hammer_shared::{ErrorCode, Id};
use serde_json::json;
use time::OffsetDateTime;

use super::auth::request_client_info;
use crate::{AppError, AppState, AuthUser};

/// multipart 的邊界、欄位標頭等額外內容的容許量。上傳路由的 body 上限為附件單檔上限加上此值。
pub const MULTIPART_OVERHEAD_BYTES: u64 = 64 * 1024;

/// 沒有 [`UPLOAD_FIELD`] 欄位時的錯誤訊息。
const MSG_NO_FILE: &str = "請選擇要上傳的檔案";

/// `POST /api/v1/attachments`（`multipart/form-data`，檔案在 `file` 欄位）
///
/// 其他欄位忽略；有多個 `file` 欄位時只使用第一個。
pub async fn upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<Attachment>), AppError> {
    // 先檢查權限，沒有權限時不接收檔案
    let mut upload = state.attachments.begin_upload(&auth).await?;
    let field = loop {
        match multipart.next_field().await.map_err(multipart_error)? {
            Some(field) if field.name() == Some(UPLOAD_FIELD) => break field,
            Some(_) => continue,
            None => {
                return Err(AppError::new(ErrorCode::Validation, MSG_NO_FILE)
                    .with_details(json!({ "field": UPLOAD_FIELD })));
            }
        }
    };
    let file_name = field.file_name().map(str::to_owned);
    let mime_type = field.content_type().map(str::to_owned);
    if file_name.is_none() {
        return Err(AppError::new(ErrorCode::Validation, MSG_FILE_NAME_REQUIRED)
            .with_details(json!({ "field": UPLOAD_FIELD })));
    }
    copy_field(field, &mut upload).await?;

    let info = request_client_info(&headers);
    let attachment = upload
        .finish(
            &state.pool,
            &auth,
            file_name.as_deref(),
            mime_type.as_deref(),
            info.as_deref(),
            OffsetDateTime::now_utc(),
        )
        .await?;
    Ok((StatusCode::CREATED, Json(attachment)))
}

async fn copy_field(
    mut field: Field<'_>,
    upload: &mut attachments::Upload,
) -> Result<(), AppError> {
    while let Some(chunk) = field.chunk().await.map_err(multipart_error)? {
        upload.write(&chunk).await?;
    }
    Ok(())
}

/// multipart 解析錯誤：超過 body 上限 → `413`，其他 → `400`。
fn multipart_error(err: MultipartError) -> AppError {
    if err.status() == StatusCode::PAYLOAD_TOO_LARGE {
        AppError::new(
            ErrorCode::PayloadTooLarge,
            crate::error::default_message(ErrorCode::PayloadTooLarge),
        )
    } else {
        AppError::new(
            ErrorCode::BadRequest,
            format!("上傳的內容格式錯誤：{}", err.body_text()),
        )
    }
}

/// `GET /api/v1/attachments/{id}`
pub async fn download(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Id>,
) -> Result<Response, AppError> {
    let file = attachments::read_attachment(&state.pool, &state.attachments, &auth, id).await?;
    let mime = HeaderValue::from_str(&file.attachment.mime_type)
        .unwrap_or(HeaderValue::from_static(DEFAULT_MIME_TYPE));
    let disposition = HeaderValue::from_str(&content_disposition(&file.attachment.file_name))
        .map_err(AppError::internal)?;
    let headers = [
        (CONTENT_TYPE, mime),
        (CONTENT_DISPOSITION, disposition),
        (CONTENT_LENGTH, HeaderValue::from(file.content.len())),
        (X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
        (CACHE_CONTROL, HeaderValue::from_static("private, no-store")),
    ];
    Ok((headers, file.content).into_response())
}

/// `Content-Disposition: attachment`，帶原始檔名（RFC 6266、RFC 5987）：
///
/// - `filename`：給不支援 `filename*` 的客戶端，非 ASCII 可見字元與 `"`、`\` 換成 `_`
/// - `filename*`：UTF-8 百分比編碼，支援中文
pub fn content_disposition(file_name: &str) -> String {
    let fallback: String = file_name
        .chars()
        .map(|c| match c {
            '"' | '\\' => '_',
            c if c.is_ascii_graphic() || c == ' ' => c,
            _ => '_',
        })
        .collect();
    let mut encoded = String::with_capacity(file_name.len() * 3);
    for b in file_name.bytes() {
        // RFC 5987 attr-char
        if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) {
            encoded.push(b as char);
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    format!("attachment; filename=\"{fallback}\"; filename*=UTF-8''{encoded}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disposition_supports_chinese_names() {
        assert_eq!(
            content_disposition("報價單 v2.pdf"),
            "attachment; filename=\"___ v2.pdf\"; \
             filename*=UTF-8''%E5%A0%B1%E5%83%B9%E5%96%AE%20v2.pdf"
        );
        assert_eq!(
            content_disposition("a\"b\\c.txt"),
            "attachment; filename=\"a_b_c.txt\"; filename*=UTF-8''a%22b%5Cc.txt"
        );
        // 結果一定是合法的標頭值
        assert!(HeaderValue::from_str(&content_disposition("照片\u{7f}.jpg")).is_ok());
    }
}
