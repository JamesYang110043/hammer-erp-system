//! 路由。
//!
//! 路由分成兩組：
//!
//! - [`public_routes`]：**不需要登入**的路由，只有這裡列出的可以不帶 token：
//!   - `GET /api/v1/health`
//!   - 登入（`POST /api/v1/auth/login`）
//!   - 初次啟動：`GET /api/v1/setup/status`、`POST /api/v1/setup/admin`（[`setup`]）
//! - [`protected_routes`]：其他所有路由。handler 必須帶 [`AuthUser`](crate::AuthUser)
//!   （或改密碼、登出、`/auth/me` 使用的 [`AuthUserAllowPasswordChange`](crate::AuthUserAllowPasswordChange)）
//!   參數，並以 `auth.require(...)` 檢查權限（§6.2）。

pub mod auth;
pub mod health;
pub mod setup;
pub mod users;

use std::any::Any;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
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

    let api = public_routes().merge(protected_routes());

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

/// 不需要登入的路由。新增前請確認真的不需要登入，並同步更新模組文件的清單。
fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/health", get(health::health))
        .route("/auth/login", post(auth::login))
        .route("/setup/status", get(setup::status))
        .route("/setup/admin", post(setup::create_admin))
}

/// 需要登入的路由。
fn protected_routes() -> Router<AppState> {
    Router::new()
        .route("/auth/logout", post(auth::logout))
        .route("/auth/me", get(auth::me))
        .route("/auth/change-password", post(auth::change_password))
        .route("/roles", get(users::list_roles))
        .route("/users", get(users::list).post(users::create))
        .route(
            "/users/{id}",
            get(users::get).patch(users::update).delete(users::delete),
        )
        .route("/users/{id}/reset-password", post(users::reset_password))
        .route("/users/{id}/unlock", post(users::unlock))
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
