//! 把框架或其他中介層產生的錯誤回應（例如 405、413，body 不是 JSON）
//! 轉成統一的 [`ApiError`](hammer_shared::ApiError) 格式。保留原本的狀態碼與其他標頭。

use axum::Json;
use axum::extract::Request;
use axum::http::header::{CONTENT_LENGTH, CONTENT_TYPE};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use hammer_shared::ApiError;

use crate::error::{code_for_status, default_message};

/// axum 中介層。
pub async fn layer(req: Request, next: Next) -> Response {
    let res = next.run(req).await;
    let status = res.status();
    if !(status.is_client_error() || status.is_server_error()) || is_json(&res) {
        return res;
    }

    let code = code_for_status(status);
    let (mut parts, _body) = res.into_parts();
    parts.headers.remove(CONTENT_TYPE);
    parts.headers.remove(CONTENT_LENGTH);
    (parts, Json(ApiError::new(code, default_message(code)))).into_response()
}

fn is_json(res: &Response) -> bool {
    res.headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"))
}
