//! 路由。

pub mod health;

use std::any::Any;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use hammer_shared::constants::API_PREFIX;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::request_id::{PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::{DefaultOnResponse, TraceLayer};
use tracing::Level;

use crate::middleware::{client_version, error_json, request_id};
use crate::{AppError, AppState};

/// 建立完整的 router（含所有中介層）。
pub fn router(state: AppState) -> Router {
    let body_limit = state.config.body_limit_bytes;

    let api = Router::new().route("/health", get(health::health));

    // `.layer()` 越後面加的越外層：請求由下往上經過，回應由上往下。
    Router::new()
        .nest(API_PREFIX, api)
        .fallback(not_found)
        .layer(axum::middleware::from_fn(client_version::layer))
        .layer(DefaultBodyLimit::max(body_limit))
        .layer(RequestBodyLimitLayer::new(body_limit))
        .layer(CatchPanicLayer::custom(handle_panic))
        .layer(axum::middleware::from_fn(error_json::layer))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(request_id::make_span)
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .layer(SetRequestIdLayer::x_request_id(
            request_id::MakeRequestUuidV7,
        ))
        .with_state(state)
}

async fn not_found() -> AppError {
    AppError::not_found()
}

fn handle_panic(err: Box<dyn Any + Send + 'static>) -> Response {
    let detail = err
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| err.downcast_ref::<&str>().copied())
        .unwrap_or("未知的 panic");
    AppError::internal(format_args!("handler panic：{detail}")).into_response()
}
