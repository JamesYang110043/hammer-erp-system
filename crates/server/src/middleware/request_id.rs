//! Request id：每個請求產生 `x-request-id`（uuid v7），寫進 log span 並回傳給客戶端。
//! 客戶端已帶 `x-request-id` 時沿用。

use axum::extract::Request;
use axum::http::HeaderValue;
use tower_http::request_id::{MakeRequestId, RequestId};

/// 以 uuid v7 產生 request id。
#[derive(Debug, Clone, Copy, Default)]
pub struct MakeRequestUuidV7;

impl MakeRequestId for MakeRequestUuidV7 {
    fn make_request_id<B>(&mut self, _request: &axum::http::Request<B>) -> Option<RequestId> {
        HeaderValue::from_str(&uuid::Uuid::now_v7().to_string())
            .ok()
            .map(RequestId::new)
    }
}

/// 建立每個請求的 tracing span，帶 request id、method、path。
pub fn make_span(req: &Request) -> tracing::Span {
    let request_id = req
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-");
    tracing::info_span!(
        "request",
        request_id = %request_id,
        method = %req.method(),
        path = %req.uri().path(),
    )
}
