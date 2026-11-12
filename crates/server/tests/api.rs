//! API Server 整合測試：health、錯誤格式、版本檢查、body 大小限制。
//!
//! 「資料庫正常」的 health 測試使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README）。
//! 其他測試使用指向不存在資料庫的 lazy 連線池，不需要資料庫。

#![allow(clippy::unwrap_used)]

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_server::routes::health::{HealthResponse, Status};
use hammer_server::{AppState, Config, router};
use hammer_shared::constants::{CLIENT_VERSION_HEADER, MIN_CLIENT_VERSION};
use hammer_shared::{ApiError, ErrorCode};
use http_body_util::BodyExt;
use serde::de::DeserializeOwned;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

const BODY_LIMIT: usize = 1024;

fn test_config() -> Config {
    Config::from_sources(None, |name| match name {
        "DATABASE_URL" => Some("postgres://unused".to_owned()),
        "HAMMER_BODY_LIMIT_BYTES" => Some(BODY_LIMIT.to_string()),
        _ => None,
    })
    .unwrap()
}

fn app(pool: PgPool) -> Router {
    router(AppState::new(pool, test_config()))
}

/// 指向沒有在監聽的埠號：模擬資料庫關閉。
fn unreachable_pool() -> PgPool {
    PgPoolOptions::new()
        .acquire_timeout(Duration::from_secs(1))
        .connect_lazy("postgres://nobody:nothing@127.0.0.1:1/none")
        .unwrap()
}

async fn send(app: Router, req: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let body = res.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, headers, body)
}

fn json<T: DeserializeOwned>(headers: &axum::http::HeaderMap, body: &[u8]) -> T {
    let ct = headers.get(header::CONTENT_TYPE).unwrap().to_str().unwrap();
    assert!(ct.starts_with("application/json"), "content-type：{ct}");
    serde_json::from_slice(body).unwrap()
}

fn get(uri: &str) -> Request<Body> {
    Request::get(uri).body(Body::empty()).unwrap()
}

#[sqlx::test(migrations = false)]
async fn health_ok_when_database_up(pool: PgPool) {
    let (status, headers, body) = send(app(pool), get("/api/v1/health")).await;
    assert_eq!(status, StatusCode::OK);
    let health: HealthResponse = json(&headers, &body);
    assert_eq!(health.status, Status::Ok);
    assert_eq!(health.database, Status::Ok);
    assert_eq!(health.version, env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn health_reports_database_down_without_crashing() {
    let app = app(unreachable_pool());

    // 連續請求兩次：第一次失敗後伺服器仍可正常回應
    for _ in 0..2 {
        let (status, headers, body) = send(app.clone(), get("/api/v1/health")).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        let health: HealthResponse = json(&headers, &body);
        assert_eq!(health.status, Status::Error);
        assert_eq!(health.database, Status::Error);
        assert_eq!(health.version, env!("CARGO_PKG_VERSION"));
    }
}

#[tokio::test]
async fn unknown_route_returns_not_found_api_error() {
    for uri in ["/api/v1/nope", "/nope"] {
        let (status, headers, body) = send(app(unreachable_pool()), get(uri)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        let err: ApiError = json(&headers, &body);
        assert_eq!(err.code, ErrorCode::NotFound);
        assert!(!err.message.is_empty());
    }
}

#[tokio::test]
async fn wrong_method_returns_method_not_allowed_api_error() {
    let req = Request::delete("/api/v1/health")
        .body(Body::empty())
        .unwrap();
    let (status, headers, body) = send(app(unreachable_pool()), req).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    let err: ApiError = json(&headers, &body);
    assert_eq!(err.code, ErrorCode::MethodNotAllowed);
    assert!(headers.contains_key(header::ALLOW));
}

#[tokio::test]
async fn outdated_client_version_returns_426() {
    let req = Request::get("/api/v1/health")
        .header(CLIENT_VERSION_HEADER, "0.0.1")
        .body(Body::empty())
        .unwrap();
    let (status, headers, body) = send(app(unreachable_pool()), req).await;
    assert_eq!(status, StatusCode::UPGRADE_REQUIRED);
    let err: ApiError = json(&headers, &body);
    assert_eq!(err.code, ErrorCode::ClientOutdated);
    let details = err.details.unwrap();
    assert_eq!(details["client_version"], "0.0.1");
    assert_eq!(details["min_version"], MIN_CLIENT_VERSION);
}

#[tokio::test]
async fn current_client_version_passes() {
    // 通過版本檢查後到達 health（資料庫不可用，所以是 503 而不是 426）
    let req = Request::get("/api/v1/health")
        .header(CLIENT_VERSION_HEADER, MIN_CLIENT_VERSION)
        .body(Body::empty())
        .unwrap();
    let (status, _, _) = send(app(unreachable_pool()), req).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn missing_client_version_passes() {
    let (status, _, _) = send(app(unreachable_pool()), get("/api/v1/health")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn invalid_client_version_returns_400() {
    let req = Request::get("/api/v1/health")
        .header(CLIENT_VERSION_HEADER, "abc")
        .body(Body::empty())
        .unwrap();
    let (status, headers, body) = send(app(unreachable_pool()), req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let err: ApiError = json(&headers, &body);
    assert_eq!(err.code, ErrorCode::BadRequest);
}

#[tokio::test]
async fn oversized_body_returns_413_api_error() {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/health")
        .header(header::CONTENT_LENGTH, BODY_LIMIT + 1)
        .body(Body::from(vec![b'x'; BODY_LIMIT + 1]))
        .unwrap();
    let (status, headers, body) = send(app(unreachable_pool()), req).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    let err: ApiError = json(&headers, &body);
    assert_eq!(err.code, ErrorCode::PayloadTooLarge);
}

#[tokio::test]
async fn response_has_request_id() {
    let (_, headers, _) = send(app(unreachable_pool()), get("/nope")).await;
    let id = headers.get("x-request-id").unwrap().to_str().unwrap();
    assert!(uuid::Uuid::parse_str(id).is_ok(), "request id：{id}");

    // 客戶端帶來的 request id 沿用
    let req = Request::get("/nope")
        .header("x-request-id", "abc-123")
        .body(Body::empty())
        .unwrap();
    let (_, headers, _) = send(app(unreachable_pool()), req).await;
    assert_eq!(headers.get("x-request-id").unwrap(), "abc-123");
}
