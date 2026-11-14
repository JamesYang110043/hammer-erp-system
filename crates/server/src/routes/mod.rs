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
//!
//! Request body 上限：上傳附件（`POST /api/v1/attachments`）為附件單檔上限加上
//! [`attachments::MULTIPART_OVERHEAD_BYTES`]，其他路由為 `body_limit_bytes`。

pub mod attachments;
pub mod audit_logs;
pub mod auth;
pub mod entities;
pub mod health;
pub mod layouts;
pub mod records;
pub mod setup;
pub mod users;

use std::any::Any;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post, put};
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
    let upload_limit = usize::try_from(
        state
            .config
            .attachment_max_bytes
            .saturating_add(attachments::MULTIPART_OVERHEAD_BYTES),
    )
    .unwrap_or(usize::MAX);

    let api = public_routes()
        .merge(protected_routes())
        .layer(DefaultBodyLimit::max(body_limit))
        .layer(RequestBodyLimitLayer::new(body_limit))
        // 上傳路由只用 axum 的 DefaultBodyLimit：Multipart 讀取超過上限時，錯誤能辨識為 413
        // （RequestBodyLimitLayer 在沒有 Content-Length 時的串流錯誤會被 multer 視為一般的讀取失敗）
        .merge(upload_routes().layer(DefaultBodyLimit::max(upload_limit)));

    // `.layer()` 越後面加的越外層：請求由下往上經過，回應由上往下。
    Router::new()
        .nest(API_PREFIX, api)
        .fallback(not_found)
        .layer(axum::middleware::from_fn(client_version::layer))
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
        .route("/audit-logs", get(audit_logs::list))
        .route("/entities", get(entities::list).post(entities::create))
        .route(
            "/entities/{key}",
            get(entities::get).patch(entities::update),
        )
        .route("/entities/{key}/fields", post(entities::create_field))
        // 固定路徑 `order` 優先於 `{field_key}`。`order` 也是合法的欄位代碼，
        // 所以同一路徑的 PATCH 轉給修改欄位
        .route(
            "/entities/{key}/fields/order",
            put(entities::reorder_fields).patch(entities::update_order_field),
        )
        .route(
            "/entities/{key}/fields/{field_key}",
            patch(entities::update_field),
        )
        .route(
            "/entities/{key}/fields/{field_key}/deactivate",
            post(entities::deactivate_field),
        )
        .route(
            "/entities/{key}/fields/{field_key}/change-type",
            post(entities::change_field_type),
        )
        .route(
            "/entities/{key}/records",
            get(records::list).post(records::create),
        )
        // 固定路徑 `lookup` 優先於 `{id}`（資料 id 是 uuid，不會衝突）
        .route("/entities/{key}/records/lookup", get(records::lookup))
        .route(
            "/entities/{key}/records/{id}",
            get(records::get)
                .patch(records::update)
                .delete(records::delete),
        )
        .route(
            "/entities/{key}/layouts",
            get(layouts::list).post(layouts::create),
        )
        // 固定路徑 `default` 優先於 `{id}`（版面 id 是 uuid，不會衝突）
        .route("/entities/{key}/layouts/default", get(layouts::get_default))
        .route(
            "/entities/{key}/layouts/{id}",
            patch(layouts::update).delete(layouts::delete),
        )
        .route("/attachments/{id}", get(attachments::download))
}

/// 上傳附件的路由（需要登入），body 上限與其他路由不同。
fn upload_routes() -> Router<AppState> {
    Router::new().route("/attachments", post(attachments::upload))
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
